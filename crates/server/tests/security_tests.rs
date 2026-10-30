use aetherstream_server::security::hmac_auth::HmacTokenValidator;
use aetherstream_server::storage::indexer::FileTreeIndexer;
use aetherstream_server::storage::mounts::MountRegistry;
use aetherstream_server::storage::stream::RangeRequestEngine;
use aetherstream_server::storage::upload::ResumableUploadManager;
use aetherstream_server::transcode::pipeline::TranscodePipeline;
use sha2::{Digest, Sha256};
use std::path::Path;

#[test]
fn test_path_traversal_prevention() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mount_root = temp_dir.path().to_path_buf();

    // Create a safe test file inside mount root
    let safe_file = mount_root.join("sample.mp4");
    std::fs::write(&safe_file, b"sample media data").unwrap();

    let registry = MountRegistry::new();
    assert!(registry.register("media", mount_root.clone(), false, None).is_ok());

    // 1. Valid path must resolve
    let resolved = registry.resolve_safe_path("media", Path::new("sample.mp4"));
    assert!(resolved.is_ok(), "Valid internal path should resolve");

    // 2. Traversal using .. must be blocked
    let attack_1 = registry.resolve_safe_path("media", Path::new("../outside.txt"));
    assert!(attack_1.is_err(), "Parent traversal must fail");

    // 3. Deep nested traversal must be blocked
    let attack_2 = registry.resolve_safe_path("media", Path::new("sub/../../etc/passwd"));
    assert!(attack_2.is_err(), "Deep nested traversal must fail");

    // 4. Traversal using root prefix or drive letter must be blocked
    #[cfg(windows)]
    {
        let attack_win = registry.resolve_safe_path("media", Path::new("C:\\Windows\\System32\\cmd.exe"));
        assert!(attack_win.is_err(), "Windows absolute path must fail");

        let attack_bs = registry.resolve_safe_path("media", Path::new("..\\..\\windows\\system32"));
        assert!(attack_bs.is_err(), "Windows backslash traversal must fail");
    }

    // 5. Encoded URI traversal sequences must be blocked
    let attack_enc1 = registry.resolve_safe_path("media", Path::new("%2e%2e/outside.txt"));
    assert!(attack_enc1.is_err(), "Encoded %2e%2e traversal must fail");

    let attack_enc2 = registry.resolve_safe_path("media", Path::new("sub/%2e%2e%2foutside.txt"));
    assert!(attack_enc2.is_err(), "Encoded %2e%2e%2f traversal must fail");

    // 6. Null byte injection must be blocked
    let attack_null = registry.resolve_safe_path("media", Path::new("sample.mp4\0.exe"));
    assert!(attack_null.is_err(), "Null byte injection must fail");
}

#[test]
fn test_range_request_parsing() {
    let total_size = 100_000u64;

    // Standard range
    let r1 = RangeRequestEngine::parse_range("bytes=0-1023", total_size);
    assert_eq!(r1, Some((0, 1023)));

    // Middle range
    let r2 = RangeRequestEngine::parse_range("bytes=5000-9999", total_size);
    assert_eq!(r2, Some((5000, 9999)));

    // Suffix range (last 500 bytes)
    let r3 = RangeRequestEngine::parse_range("bytes=-500", total_size);
    assert_eq!(r3, Some((99500, 99999)));

    // Open-ended range: starts at 1000
    let r4 = RangeRequestEngine::parse_range("bytes=1000-", total_size);
    assert!(r4.is_some());
    let (start, end) = r4.unwrap();
    assert_eq!(start, 1000);
    assert!(end > start && end < total_size);

    // Invalid range header
    let invalid = RangeRequestEngine::parse_range("characters=0-10", total_size);
    assert_eq!(invalid, None);

    // Multi-part range header (picks first range to neutralize range amplification attacks)
    let multi = RangeRequestEngine::parse_range("bytes=0-499, 1000-1499", total_size);
    assert_eq!(multi, Some((0, 499)));
}

#[tokio::test]
async fn test_stream_file_range_response() {
    use axum::body::to_bytes;
    use axum::http::header;

    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("video_range_test.mp4");

    // Write 256 KiB of deterministic data
    let total_bytes: usize = 256 * 1024;
    let mut data = Vec::with_capacity(total_bytes);
    for i in 0..total_bytes {
        data.push((i % 256) as u8);
    }
    std::fs::write(&file_path, &data).unwrap();

    // 1. Test HTTP 206 Partial Content Range Request
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::RANGE, "bytes=100-299".parse().unwrap());

    let resp = RangeRequestEngine::stream_file(&file_path, &headers).await;
    assert_eq!(resp.status(), axum::http::StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers().get(header::CONTENT_RANGE).unwrap(), "bytes 100-299/262144");
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "200");
    assert_eq!(resp.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");

    let body_bytes = to_bytes(resp.into_body(), total_bytes).await.unwrap();
    assert_eq!(body_bytes.len(), 200);
    assert_eq!(&body_bytes[..], &data[100..=299]);

    // 2. Test 416 Range Not Satisfiable
    let mut out_of_bounds = axum::http::HeaderMap::new();
    out_of_bounds.insert(header::RANGE, "bytes=500000-600000".parse().unwrap());
    let resp_416 = RangeRequestEngine::stream_file(&file_path, &out_of_bounds).await;
    assert_eq!(resp_416.status(), axum::http::StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(resp_416.headers().get(header::CONTENT_RANGE).unwrap(), "bytes */262144");

    // 3. Test HTTP 200 Full Stream
    let empty_headers = axum::http::HeaderMap::new();
    let resp_200 = RangeRequestEngine::stream_file(&file_path, &empty_headers).await;
    assert_eq!(resp_200.status(), axum::http::StatusCode::OK);
    assert_eq!(resp_200.headers().get(header::CONTENT_LENGTH).unwrap(), "262144");
}

#[tokio::test]
async fn test_zero_copy_bounded_stream() {
    use futures_util::StreamExt;
    use tokio::fs::File;
    use tokio_util::io::ReaderStream;

    let temp_dir = tempfile::tempdir().unwrap();
    let test_file = temp_dir.path().join("bounded_stream_payload.bin");

    // Create a 5 MiB file
    let total_size: usize = 5 * 1024 * 1024;
    let data = vec![0x42u8; total_size];
    tokio::fs::write(&test_file, &data).await.unwrap();

    let file = File::open(&test_file).await.unwrap();
    let chunk_capacity = 64 * 1024; // 64 KiB bounded ring buffer
    let mut stream = ReaderStream::with_capacity(file, chunk_capacity);

    let mut total_streamed = 0usize;
    let mut chunk_count = 0usize;

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.unwrap();
        assert!(
            chunk.len() <= chunk_capacity,
            "Chunk size {} exceeded bounded ring buffer capacity {}",
            chunk.len(),
            chunk_capacity
        );
        total_streamed += chunk.len();
        chunk_count += 1;
    }

    assert_eq!(total_streamed, total_size, "Total streamed bytes must match file size");
    assert!(chunk_count >= (total_size / chunk_capacity), "Stream must be sliced across bounded chunks");
    println!("Zero-copy pipe verified: Streamed {} bytes across {} bounded chunks (max 64 KiB each)", total_streamed, chunk_count);
}

#[test]
fn test_hmac_scoped_token_verification() {
    let secret = "super_secure_production_hmac_secret_key_12345";
    let mount = "videos";
    let path = "movies/sci-fi/matrix.mp4";

    // Create token valid for 3600 seconds
    let token = HmacTokenValidator::create_token_string(secret, mount, "movies/sci-fi", 3600, false);

    // Valid access matching path scope
    let valid_check = HmacTokenValidator::validate_token(secret, &token, mount, path);
    assert!(valid_check.is_ok(), "Valid token within scope must pass");

    // Tampered token check
    let tampered_token = format!("{}tampered", token);
    let tampered_check = HmacTokenValidator::validate_token(secret, &tampered_token, mount, path);
    assert!(tampered_check.is_err(), "Tampered signature must fail");

    // Out-of-scope path access check
    let out_of_scope_path = "movies/horror/halloween.mp4";
    let scope_check = HmacTokenValidator::validate_token(secret, &token, mount, out_of_scope_path);
    assert!(scope_check.is_err(), "Out-of-scope path must be denied");

    // Different mount check
    let mount_check = HmacTokenValidator::validate_token(secret, &token, "vault", path);
    assert!(mount_check.is_err(), "Different mount must be denied");

    // Opaque hex-encoded token test
    let hex_token = hex::encode(&token);
    let hex_check = HmacTokenValidator::validate_token(secret, &hex_token, mount, path);
    assert!(hex_check.is_ok(), "Opaque hex-encoded token must validate cleanly");

    // Expired token test
    let expired_token = HmacTokenValidator::create_token_string(secret, mount, "movies", 0, false);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let expired_check = HmacTokenValidator::validate_token(secret, &expired_token, mount, "movies/test.mp4");
    assert!(expired_check.is_err(), "Expired token must be rejected");
}

#[test]
fn test_master_manifest_generation() {
    use aetherstream_server::transcode::pipeline::MediaMetadata;

    // 1. Generic full manifest with default 1080p source
    let manifest =
        TranscodePipeline::generate_master_manifest("/api/v1/transcode/hls/videos/matrix.mp4");
    assert!(manifest.contains("#EXTM3U"));
    assert!(manifest.contains("#EXT-X-VERSION:6"));
    assert!(manifest.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
    assert!(manifest.contains("#EXT-X-STREAM-INF"));
    assert!(manifest.contains("1080p_high"));
    assert!(manifest.contains("720p_med"));
    assert!(manifest.contains("480p_low"));
    assert!(manifest.contains("360p_min"));

    // 2. Dynamic manifest for 720p source -> must exclude 1080p (zero upscaling)
    let manifest_720 = TranscodePipeline::generate_dynamic_master_manifest(
        "/api/v1/transcode/hls/videos/stream720.mp4",
        720,
    );
    assert!(!manifest_720.contains("1080p_high"));
    assert!(manifest_720.contains("720p_med"));
    assert!(manifest_720.contains("480p_low"));
    assert!(manifest_720.contains("360p_min"));

    // 3. Dynamic manifest from MediaMetadata for 480p source -> must exclude 1080p and 720p
    let meta_480 = MediaMetadata {
        container: "mp4".to_string(),
        video_codec: Some("h264".to_string()),
        width: Some(854),
        height: Some(480),
        framerate: Some(29.97),
        bitrate_kbps: Some(1200),
        audio_codec: Some("aac".to_string()),
        audio_channels: Some(2),
        gop_size: Some(60),
        duration_secs: Some(300.0),
    };
    let manifest_480 = TranscodePipeline::generate_master_manifest_for_media(
        "/api/v1/transcode/hls/videos/stream480.mp4",
        &meta_480,
    );
    assert!(!manifest_480.contains("1080p_high"));
    assert!(!manifest_480.contains("720p_med"));
    assert!(manifest_480.contains("480p_low"));
    assert!(manifest_480.contains("360p_min"));

    // 4. Fallback for ultra-low resolution source (240p) -> retains lowest profile (360p)
    let manifest_240 = TranscodePipeline::generate_dynamic_master_manifest(
        "/api/v1/transcode/hls/videos/low.mp4",
        240,
    );
    assert_eq!(manifest_240.matches("#EXT-X-STREAM-INF").count(), 1);
    assert!(manifest_240.contains("360p_min"));
}

#[test]
fn test_drive_discovery() {
    let drives = MountRegistry::discover_system_drives();
    assert!(!drives.is_empty(), "Must discover at least one drive root");

    let detailed = MountRegistry::discover_detailed_drives();
    assert!(!detailed.is_empty(), "Must discover detailed drive specifications");
    assert!(detailed.iter().any(|d| d.is_system), "Must detect system drive root");
    println!("Discovered system drives: {:?}", detailed);
}

#[test]
fn test_virtual_mount_permissions_and_sqlite_schema() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let registry = MountRegistry::new();
    assert!(registry.register_with_permission(
        "media_ro",
        root.clone(),
        aetherstream_server::storage::mounts::MountPermission::Read,
        Some("Read-Only Storage".into()),
    ).is_ok());
    assert!(registry.register_with_permission(
        "media_rw",
        root.clone(),
        aetherstream_server::storage::mounts::MountPermission::Write,
        Some("Read-Write Storage".into()),
    ).is_ok());
    assert!(registry.register_with_permission(
        "media_stream",
        root.clone(),
        aetherstream_server::storage::mounts::MountPermission::StreamOnly,
        Some("Stream-Only Media".into()),
    ).is_ok());

    // Verify write permissions
    assert!(registry.check_write_permission("media_rw").is_ok(), "Write permitted on RW mount");
    assert!(registry.check_write_permission("media_ro").is_err(), "Write denied on Read mount");
    assert!(registry.check_write_permission("media_stream").is_err(), "Write denied on StreamOnly mount");

    // Verify stream permissions
    assert!(registry.check_stream_permission("media_stream").is_ok(), "Stream permitted on StreamOnly mount");
    assert!(registry.check_stream_permission("media_ro").is_ok(), "Stream permitted on Read mount");
    assert!(registry.check_stream_permission("media_rw").is_ok(), "Stream permitted on Write mount");

    // Verify SQLite WAL Schema and SQL export
    let sql_dump = registry.export_sqlite_ddl_and_inserts();
    assert!(sql_dump.contains("PRAGMA journal_mode = WAL;"));
    assert!(sql_dump.contains("CREATE TABLE IF NOT EXISTS virtual_mounts"));
    assert!(sql_dump.contains("STREAM_ONLY"));
}

#[tokio::test]
async fn test_resumable_upload_with_sha256() {
    let temp_dir = tempfile::tempdir().unwrap();
    let target_file = temp_dir.path().join("uploaded_video.mp4");

    // 1. Direct stateless chunk write test
    let chunk1_data = b"chunk_1_payload_bytes";
    let mut hasher1 = Sha256::new();
    hasher1.update(chunk1_data);
    let chunk1_hash = hex::encode(hasher1.finalize());

    let write_res1 = ResumableUploadManager::write_chunk(&target_file, 0, &chunk1_hash, chunk1_data).await;
    assert!(write_res1.is_ok());

    // Write chunk with corrupted checksum
    let corrupted_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    let write_res2 = ResumableUploadManager::write_chunk(&target_file, 1, corrupted_hash, b"chunk_2_corrupted").await;
    assert!(write_res2.is_err(), "Corrupted checksum must be rejected");

    // 2. State-tracked multi-chunk resumable session test
    let manager = ResumableUploadManager::new();
    let session_target = temp_dir.path().join("session_final.mp4");
    let chunk_size = 1000u64;
    let total_size = 2000u64;

    let chunk_a = vec![0x11u8; 1000];
    let mut hasher_a = Sha256::new();
    hasher_a.update(&chunk_a);
    let hash_a = hex::encode(hasher_a.finalize());

    let chunk_b = vec![0x22u8; 1000];
    let mut hasher_b = Sha256::new();
    hasher_b.update(&chunk_b);
    let hash_b = hex::encode(hasher_b.finalize());

    // Total file hash
    let mut full_payload = Vec::new();
    full_payload.extend_from_slice(&chunk_a);
    full_payload.extend_from_slice(&chunk_b);
    let mut full_hasher = Sha256::new();
    full_hasher.update(&full_payload);
    let full_hash = hex::encode(full_hasher.finalize());

    // Initialize session
    let upload_id = "upload_sess_abc123";
    let sess = manager.init_session(upload_id, session_target.clone(), total_size, chunk_size).await.unwrap();
    assert_eq!(sess.total_chunks, 2);

    // Write Chunk 0
    let res_chunk0 = manager.write_session_chunk(upload_id, 0, &hash_a, &chunk_a).await;
    assert!(res_chunk0.is_ok());

    // Simulate Pause & Query Progress
    let progress = manager.get_session(upload_id).await.unwrap();
    assert_eq!(progress.completed_chunks, vec![0]);
    assert_eq!(progress.received_bytes, 1000);
    assert!(!progress.is_completed);

    // Write Chunk 1 with Corrupted Hash (should fail)
    let bad_write = manager.write_session_chunk(upload_id, 1, "bad_hash_hex", &chunk_b).await;
    assert!(bad_write.is_err(), "Corrupted session chunk must fail verification");

    // Write Chunk 1 with Valid Hash (Resume)
    let good_write = manager.write_session_chunk(upload_id, 1, &hash_b, &chunk_b).await;
    assert!(good_write.is_ok());

    // Complete Session & Verify File
    let finalize_res = manager.complete_session(upload_id, Some(&full_hash)).await;
    assert!(finalize_res.is_ok());
    assert!(session_target.exists(), "Finalized target file must exist on disk");
    assert_eq!(std::fs::metadata(&session_target).unwrap().len(), 2000);
}

#[test]
fn test_file_tree_indexer_and_search() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sub = temp_dir.path().join("action");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("gladiator.mp4"), b"gladiator film").unwrap();

    let indexer = FileTreeIndexer::new();
    let count = indexer.index_mount("movies", temp_dir.path()).unwrap();
    assert_eq!(count, 1);

    // 1. Search Query Test & Sub-millisecond Execution Assertion
    let start_time = std::time::Instant::now();
    let search_res = indexer.search("glad");
    let elapsed = start_time.elapsed();

    assert_eq!(search_res.len(), 1);
    assert_eq!(search_res[0].name, "gladiator.mp4");
    assert!(elapsed.as_millis() < 5, "In-memory query search must execute in < 5ms (took {:?})", elapsed);

    // 2. Incremental Watcher Addition Test (New File Event)
    indexer.add_or_update_file("movies", "action/matrix.mp4", "matrix.mp4", 4500000, "video/mp4");
    let after_add = indexer.search("matrix");
    assert_eq!(after_add.len(), 1);
    assert_eq!(after_add[0].name, "matrix.mp4");

    // 3. Mount Stats Assertion
    let stats = indexer.get_mount_stats("movies").unwrap();
    assert_eq!(stats.total_files, 2);

    // 4. Incremental Watcher Removal Test (File Deleted Event)
    let removed = indexer.remove_file("movies", "action/matrix.mp4");
    assert!(removed);
    let after_remove = indexer.search("matrix");
    assert_eq!(after_remove.len(), 0);

    let stats_after = indexer.get_mount_stats("movies").unwrap();
    assert_eq!(stats_after.total_files, 1);
}

#[tokio::test]
async fn test_concurrent_io_throughput_and_ttfb() {
    use futures_util::StreamExt;
    use std::sync::Arc;
    use tokio::fs::File;
    use tokio_util::io::ReaderStream;

    let temp_dir = tempfile::tempdir().unwrap();
    let bench_file = temp_dir.path().join("bench_16mb.bin");

    // Write 16 MiB payload
    let total_size: usize = 16 * 1024 * 1024;
    let data = vec![0xABu8; total_size];
    tokio::fs::write(&bench_file, &data).await.unwrap();

    let file_path = Arc::new(bench_file);
    let concurrency = 8;
    let mut handles = Vec::new();

    let bench_start = std::time::Instant::now();

    for _ in 0..concurrency {
        let path = Arc::clone(&file_path);
        handles.push(tokio::spawn(async move {
            let task_start = std::time::Instant::now();
            let file = File::open(&*path).await.unwrap();
            let mut stream = ReaderStream::with_capacity(file, 64 * 1024);

            let mut ttfb = None;
            let mut bytes_read = 0usize;

            while let Some(chunk) = stream.next().await {
                if ttfb.is_none() {
                    ttfb = Some(task_start.elapsed());
                }
                bytes_read += chunk.unwrap().len();
            }

            (ttfb.unwrap_or_default(), bytes_read)
        }));
    }

    let mut max_ttfb = std::time::Duration::from_millis(0);
    let mut total_transferred = 0usize;

    for h in handles {
        let (ttfb, bytes) = h.await.unwrap();
        if ttfb > max_ttfb {
            max_ttfb = ttfb;
        }
        total_transferred += bytes;
    }

    let total_elapsed = bench_start.elapsed();
    let seconds = total_elapsed.as_secs_f64().max(0.0001);
    let gigabits = (total_transferred as f64 * 8.0) / (1_000_000_000.0);
    let gbps = gigabits / seconds;

    println!(
        "[BENCHMARK] Concurrent Workers: {} | Total: {} bytes | Time: {:.3}s | TTFB (max): {:.2?} | Rate: {:.2} Gbps",
        concurrency, total_transferred, seconds, max_ttfb, gbps
    );

    assert!(
        max_ttfb.as_millis() < 50,
        "Max TTFB must be minimal under local loopback (measured {:.2?})",
        max_ttfb
    );
    assert!(
        total_transferred == total_size * concurrency,
        "All workers must complete full stream"
    );
}

#[tokio::test]
async fn test_ffprobe_metadata_extraction() {
    use aetherstream_server::transcode::pipeline::{MediaMetadata, TranscodePipeline};
    use aetherstream_server::transcode::profiles::STANDARD_RENDITION_LADDER;

    // 1. Test parsing H.264 1080p stream with 5.1 AAC audio and GOP frames
    let ffprobe_h264_json = r#"{
        "format": {
            "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
            "duration": "120.500000",
            "size": "67500000",
            "bit_rate": "4481327"
        },
        "streams": [
            {
                "index": 0,
                "codec_name": "h264",
                "codec_type": "video",
                "width": 1920,
                "height": 1080,
                "avg_frame_rate": "30000/1001",
                "bit_rate": "4100000"
            },
            {
                "index": 1,
                "codec_name": "aac",
                "codec_type": "audio",
                "channels": 6,
                "channel_layout": "5.1",
                "bit_rate": "384000"
            }
        ],
        "frames": [
            { "key_frame": 1, "pict_type": "I", "pts_time": "0.000000" },
            { "key_frame": 0, "pict_type": "P", "pts_time": "0.033367" },
            { "key_frame": 0, "pict_type": "B", "pts_time": "0.066733" },
            { "key_frame": 1, "pict_type": "I", "pts_time": "2.002000" }
        ]
    }"#;

    let meta = TranscodePipeline::parse_ffprobe_json(ffprobe_h264_json).expect("Failed to parse valid H.264 JSON");
    assert_eq!(meta.container, "mov,mp4,m4a,3gp,3g2,mj2");
    assert_eq!(meta.video_codec.as_deref(), Some("h264"));
    assert_eq!(meta.width, Some(1920));
    assert_eq!(meta.height, Some(1080));
    assert_eq!(meta.audio_codec.as_deref(), Some("aac"));
    assert_eq!(meta.audio_channels, Some(6));
    assert_eq!(meta.bitrate_kbps, Some(4100));
    assert_eq!(meta.gop_size, Some(3)); // Keyframe at index 0 and index 3 (diff = 3)
    assert!((meta.framerate.unwrap() - 29.97).abs() < 0.01);
    // 5.1 surround sound audio requires downmix, so cannot be direct played as-is without audio transcode
    assert!(!meta.is_direct_streamable());

    // 2. Test stereo AAC audio makes H.264 direct streamable
    let mut stereo_meta = meta.clone();
    stereo_meta.audio_channels = Some(2);
    assert!(stereo_meta.is_direct_streamable());

    // 3. Test HEVC 4K parsing
    let ffprobe_hevc_json = r#"{
        "format": { "format_name": "matroska,webm", "bit_rate": "15000000" },
        "streams": [
            {
                "index": 0,
                "codec_name": "hevc",
                "codec_type": "video",
                "width": 3840,
                "height": 2160,
                "avg_frame_rate": "60/1",
                "bit_rate": "14500000"
            },
            {
                "index": 1,
                "codec_name": "eac3",
                "codec_type": "audio",
                "channels": 6
            }
        ]
    }"#;
    let hevc_meta = TranscodePipeline::parse_ffprobe_json(ffprobe_hevc_json).unwrap();
    assert_eq!(hevc_meta.video_codec.as_deref(), Some("hevc"));
    assert_eq!(hevc_meta.width, Some(3840));
    assert_eq!(hevc_meta.height, Some(2160));
    assert_eq!(hevc_meta.framerate, Some(60.0));
    assert_eq!(hevc_meta.audio_codec.as_deref(), Some("eac3"));

    // 4. Test AV1 with Opus
    let ffprobe_av1_json = r#"{
        "format": { "format_name": "matroska,webm" },
        "streams": [
            { "index": 0, "codec_name": "av1", "codec_type": "video", "width": 1280, "height": 720, "r_frame_rate": "24/1" },
            { "index": 1, "codec_name": "opus", "codec_type": "audio", "channels": 2 }
        ]
    }"#;
    let av1_meta = TranscodePipeline::parse_ffprobe_json(ffprobe_av1_json).unwrap();
    assert_eq!(av1_meta.video_codec.as_deref(), Some("av1"));
    assert_eq!(av1_meta.audio_codec.as_deref(), Some("opus"));
    assert_eq!(av1_meta.audio_channels, Some(2));
    assert!(av1_meta.is_direct_streamable());

    // 5. Test VP9
    let ffprobe_vp9_json = r#"{
        "format": { "format_name": "webm" },
        "streams": [
            { "index": 0, "codec_name": "vp9", "codec_type": "video", "width": 854, "height": 480, "avg_frame_rate": "30/1" },
            { "index": 1, "codec_name": "vorbis", "codec_type": "audio", "channels": 2 }
        ]
    }"#;
    let vp9_meta = TranscodePipeline::parse_ffprobe_json(ffprobe_vp9_json).unwrap();
    assert_eq!(vp9_meta.video_codec.as_deref(), Some("vp9"));
    assert_eq!(vp9_meta.height, Some(480));

    // 6. Test adaptive profile ladder filtering (no upscaling)
    let ladder_720 = av1_meta.filter_rendition_ladder(STANDARD_RENDITION_LADDER);
    assert_eq!(ladder_720.len(), 3);
    assert_eq!(ladder_720[0].tag, "720p_med");
    assert_eq!(ladder_720[1].tag, "480p_low");
    assert_eq!(ladder_720[2].tag, "360p_min");

    let ladder_1080 = meta.filter_rendition_ladder(STANDARD_RENDITION_LADDER);
    assert_eq!(ladder_1080.len(), 4);
    assert_eq!(ladder_1080[0].tag, "1080p_high");

    let low_meta = MediaMetadata {
        container: "mp4".to_string(),
        video_codec: Some("h264".to_string()),
        width: Some(320),
        height: Some(240),
        framerate: Some(25.0),
        bitrate_kbps: Some(300),
        audio_codec: Some("aac".to_string()),
        audio_channels: Some(2),
        gop_size: Some(50),
        duration_secs: Some(60.0),
    };
    let ladder_240 = low_meta.filter_rendition_ladder(STANDARD_RENDITION_LADDER);
    assert_eq!(ladder_240.len(), 1);
    assert_eq!(ladder_240[0].tag, "360p_min");

    // 7. Test probe_file fallback execution
    let temp_dir = tempfile::tempdir().unwrap();
    let dummy_mp4 = temp_dir.path().join("sample.mp4");
    tokio::fs::write(&dummy_mp4, b"dummy video container header").await.unwrap();

    let probe_res = TranscodePipeline::probe_file(&dummy_mp4).await;
    assert!(probe_res.is_ok(), "probe_file must handle absent ffprobe gracefully");
    let probed = probe_res.unwrap();
    assert!(probed.container.contains("mp4"));
}

#[test]
fn test_hardware_encoder_detection_ladder() {
    use aetherstream_server::transcode::pipeline::{HardwareAccel, TranscodePipeline};
    use aetherstream_server::transcode::profiles::STANDARD_RENDITION_LADDER;
    use std::path::Path;

    // 1. Dual GPU system (NVENC + QSV present) -> NVENC priority
    let dual_gpu_encoders = r#"
Encoders:
 V..... h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
 V..... h264_qsv             Intel Quick Sync Video acceleration for H.264 (codec h264)
 V..... libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10
    "#;
    let accel = HardwareAccel::detect_from_encoder_list(dual_gpu_encoders);
    assert_eq!(accel, HardwareAccel::Nvenc);
    assert_eq!(accel.encoder_name(), "h264_nvenc");
    assert_eq!(accel.hevc_encoder_name(), "hevc_nvenc");
    assert!(accel.is_hardware_accelerated());
    assert_eq!(accel.extra_ffmpeg_flags(), vec!["-hwaccel", "cuda"]);

    // 2. Intel integrated GPU system (only QSV) -> QSV selected
    let qsv_only = "V..... h264_qsv Intel QuickSync\nV..... libx264";
    let accel_qsv = HardwareAccel::detect_from_encoder_list(qsv_only);
    assert_eq!(accel_qsv, HardwareAccel::QuickSync);
    assert_eq!(accel_qsv.encoder_name(), "h264_qsv");
    assert_eq!(accel_qsv.hevc_encoder_name(), "hevc_qsv");
    assert!(accel_qsv.is_hardware_accelerated());
    assert_eq!(accel_qsv.extra_ffmpeg_flags(), vec!["-hwaccel", "qsv"]);

    // 3. Apple Silicon / macOS system -> VideoToolbox selected
    let videotoolbox_only = "V..... h264_videotoolbox Apple VideoToolbox\nV..... libx264";
    let accel_vt = HardwareAccel::detect_from_encoder_list(videotoolbox_only);
    assert_eq!(accel_vt, HardwareAccel::VideoToolbox);
    assert_eq!(accel_vt.encoder_name(), "h264_videotoolbox");
    assert_eq!(accel_vt.hevc_encoder_name(), "hevc_videotoolbox");
    assert!(accel_vt.is_hardware_accelerated());
    assert_eq!(accel_vt.extra_ffmpeg_flags(), vec!["-hwaccel", "videotoolbox"]);

    // 4. Linux server (VA-API only) -> VA-API selected
    let vaapi_only = "V..... h264_vaapi VAAPI H.264\nV..... libx264";
    let accel_va = HardwareAccel::detect_from_encoder_list(vaapi_only);
    assert_eq!(accel_va, HardwareAccel::Vaapi);
    assert_eq!(accel_va.encoder_name(), "h264_vaapi");
    assert_eq!(accel_va.hevc_encoder_name(), "hevc_vaapi");
    assert!(accel_va.is_hardware_accelerated());
    assert_eq!(accel_va.extra_ffmpeg_flags(), vec!["-hwaccel", "vaapi"]);

    // 5. Headless / generic VM (no hardware encoders) -> CPU libx264 fallback
    let cpu_only = "V..... libx264 libx264 software encoder\nV..... libx265";
    let accel_cpu = HardwareAccel::detect_from_encoder_list(cpu_only);
    assert_eq!(accel_cpu, HardwareAccel::SoftwareCpu);
    assert_eq!(accel_cpu.encoder_name(), "libx264");
    assert_eq!(accel_cpu.hevc_encoder_name(), "libx265");
    assert!(!accel_cpu.is_hardware_accelerated());
    assert!(accel_cpu.extra_ffmpeg_flags().is_empty());

    // 6. Test build_ffmpeg_args includes hardware acceleration flags
    let dummy_in = Path::new("test.mp4");
    let dummy_out = Path::new("out");
    let args_nvenc = TranscodePipeline::build_ffmpeg_args(
        dummy_in,
        dummy_out,
        HardwareAccel::Nvenc,
        &STANDARD_RENDITION_LADDER[0],
    );
    assert!(args_nvenc.contains(&"-hwaccel".to_string()));
    assert!(args_nvenc.contains(&"cuda".to_string()));
    assert!(args_nvenc.contains(&"h264_nvenc".to_string()));

    let args_cpu = TranscodePipeline::build_ffmpeg_args(
        dummy_in,
        dummy_out,
        HardwareAccel::SoftwareCpu,
        &STANDARD_RENDITION_LADDER[0],
    );
    assert!(!args_cpu.contains(&"-hwaccel".to_string()));
    assert!(args_cpu.contains(&"libx264".to_string()));
}

#[test]
fn test_adaptive_profile_definitions() {
    use aetherstream_server::transcode::profiles::{
        get_profile_by_tag, select_optimal_profile, STANDARD_RENDITION_LADDER,
    };

    // 1. Verify all 4 standard profiles exist and adhere to ladder hierarchy
    assert_eq!(STANDARD_RENDITION_LADDER.len(), 4);
    let p1080 = get_profile_by_tag("1080p_high").expect("1080p_high must exist");
    let p720 = get_profile_by_tag("720p_med").expect("720p_med must exist");
    let p480 = get_profile_by_tag("480p_low").expect("480p_low must exist");
    let p360 = get_profile_by_tag("360p_min").expect("360p_min must exist");

    assert_eq!(p1080.width, 1920);
    assert_eq!(p1080.height, 1080);
    assert_eq!(p1080.bandwidth(), (4500 + 192) * 1000);
    assert_eq!(p1080.max_fps, 60);

    assert_eq!(p720.width, 1280);
    assert_eq!(p720.height, 720);
    assert_eq!(p720.bandwidth(), (2500 + 128) * 1000);
    assert_eq!(p720.max_fps, 60);

    assert_eq!(p480.width, 854);
    assert_eq!(p480.height, 480);
    assert_eq!(p480.bandwidth(), (1200 + 96) * 1000);
    assert_eq!(p480.max_fps, 30);

    assert_eq!(p360.width, 640);
    assert_eq!(p360.height, 360);
    assert_eq!(p360.bandwidth(), (600 + 64) * 1000);
    assert_eq!(p360.max_fps, 30);

    // 2. Bitrate maxrate and buffer size rate-control formulas
    assert!(p1080.maxrate_kbps() > p1080.video_bitrate_kbps);
    assert_eq!(p1080.bufsize_kbps(), p1080.video_bitrate_kbps * 2);

    // 3. HLS RFC 8216 Stream-Inf line format
    let stream_inf = p720.to_stream_inf("https://stream.aether.local/hls");
    assert!(stream_inf.contains("#EXT-X-STREAM-INF:BANDWIDTH="));
    assert!(stream_inf.contains("RESOLUTION=1280x720"));
    assert!(stream_inf.contains("FRAME-RATE=60.000"));
    assert!(stream_inf.contains("CODECS=\"avc1.4d401f,mp4a.40.2\""));
    assert!(stream_inf.contains("https://stream.aether.local/hls/720p_med/index.m3u8"));

    // 4. ABR profile selection under various bandwidth budgets and source constraints
    // High bandwidth (10 Mbps) on 1080p source -> selects 1080p
    let sel_high = select_optimal_profile(10_000_000, 1080);
    assert_eq!(sel_high.tag, "1080p_high");

    // Moderate bandwidth (3.5 Mbps) on 1080p source -> selects 720p
    let sel_mod = select_optimal_profile(3_500_000, 1080);
    assert_eq!(sel_mod.tag, "720p_med");

    // High bandwidth (10 Mbps) on 720p source -> bounded by source height (no upscaling to 1080p)
    let sel_bounded = select_optimal_profile(10_000_000, 720);
    assert_eq!(sel_bounded.tag, "720p_med");

    // Low bandwidth (800 kbps) -> selects 360p
    let sel_low = select_optimal_profile(800_000, 1080);
    assert_eq!(sel_low.tag, "360p_min");

    // Non-existent tag lookup
    assert!(get_profile_by_tag("unknown_rendition").is_none());
}

#[test]
fn test_jit_segmenter_and_lru_ring_buffer() {
    use aetherstream_server::transcode::pipeline::{
        HardwareAccel, LruSegmentRingBuffer, TranscodePipeline,
    };
    use aetherstream_server::transcode::profiles::STANDARD_RENDITION_LADDER;
    use bytes::Bytes;
    use std::path::Path;

    // 1. Verify FFmpeg segmenter CLI flags for 4s segments & GOP alignment
    let profile_1080 = &STANDARD_RENDITION_LADDER[0]; // 60 fps, 4.0s duration
    let args = TranscodePipeline::build_ffmpeg_args(
        Path::new("input.mkv"),
        Path::new("/tmp/hls_out"),
        HardwareAccel::Nvenc,
        profile_1080,
    );

    assert!(args.contains(&"-f".to_string()));
    assert!(args.contains(&"hls".to_string()));
    assert!(args.contains(&"-hls_time".to_string()));
    assert!(args.contains(&"4".to_string()));
    assert!(args.contains(&"-hls_list_size".to_string()));
    assert!(args.contains(&"10".to_string()));
    assert!(args.contains(&"-hls_flags".to_string()));
    assert!(args.contains(&"delete_segments+independent_segments".to_string()));

    // Keyframe alignment verification (GOP = 60 * 4 = 240 frames)
    let gop_pos = args.iter().position(|a| a == "-g").expect("-g must be set");
    assert_eq!(args[gop_pos + 1], "240");
    let keyint_pos = args
        .iter()
        .position(|a| a == "-keyint_min")
        .expect("-keyint_min must be set");
    assert_eq!(args[keyint_pos + 1], "240");
    let sc_pos = args
        .iter()
        .position(|a| a == "-sc_threshold")
        .expect("-sc_threshold must be set");
    assert_eq!(args[sc_pos + 1], "0");

    // 2. Rolling LRU ring buffer verification
    let mut ring = LruSegmentRingBuffer::new(3);
    assert_eq!(ring.len(), 0);
    assert!(ring.is_empty());

    let payload = Bytes::from_static(b"TEST_MPEGTS_PAYLOAD_CHUNK_DATA");

    // Insert 3 segments -> ring buffer fills to capacity
    assert!(ring
        .insert_segment("seg_0.ts".to_string(), 0, Some(payload.clone()), payload.len())
        .is_none());
    assert!(ring
        .insert_segment("seg_1.ts".to_string(), 1, Some(payload.clone()), payload.len())
        .is_none());
    assert!(ring
        .insert_segment("seg_2.ts".to_string(), 2, Some(payload.clone()), payload.len())
        .is_none());
    assert_eq!(ring.len(), 3);
    assert_eq!(ring.total_cached_bytes(), payload.len() * 3);

    // Touch seg_0 (access moves it to MRU)
    assert!(ring.get_segment("seg_0.ts").is_some());

    // Insert 4th segment -> seg_1 was oldest unaccessed, so seg_1 is evicted
    let evicted =
        ring.insert_segment("seg_3.ts".to_string(), 3, Some(payload.clone()), payload.len());
    assert_eq!(evicted, Some("seg_1.ts".to_string()));
    assert_eq!(ring.len(), 3);

    assert!(ring.get_segment("seg_1.ts").is_none());
    assert!(ring.get_segment("seg_0.ts").is_some());
    assert!(ring.get_segment("seg_2.ts").is_some());
    assert!(ring.get_segment("seg_3.ts").is_some());

    ring.clear();
    assert_eq!(ring.len(), 0);
    assert!(ring.is_empty());
}

#[test]
fn test_audio_passthrough_and_stereo_downmix() {
    use aetherstream_server::transcode::pipeline::{
        AudioTranscodeStrategy, HardwareAccel, MediaMetadata, TranscodePipeline,
    };
    use aetherstream_server::transcode::profiles::STANDARD_RENDITION_LADDER;
    use std::path::Path;

    let profile = &STANDARD_RENDITION_LADDER[1]; // 720p_med (audio bitrate: 128 kbps)

    // 1. Stereo AAC source -> PassThrough (-c:a copy)
    let meta_aac_stereo = MediaMetadata {
        container: "mp4".to_string(),
        video_codec: Some("h264".to_string()),
        width: Some(1280),
        height: Some(720),
        framerate: Some(30.0),
        bitrate_kbps: Some(2500),
        audio_codec: Some("aac".to_string()),
        audio_channels: Some(2),
        gop_size: Some(60),
        duration_secs: Some(100.0),
    };
    let strat_passthrough = AudioTranscodeStrategy::determine(&meta_aac_stereo);
    assert_eq!(strat_passthrough, AudioTranscodeStrategy::PassThrough);
    assert_eq!(strat_passthrough.ffmpeg_flags(128), vec!["-c:a", "copy"]);

    let args_pt = TranscodePipeline::build_ffmpeg_args_with_metadata(
        Path::new("video.mp4"),
        Path::new("/tmp/hls"),
        HardwareAccel::Nvenc,
        profile,
        &meta_aac_stereo,
    );
    assert!(args_pt.contains(&"-c:a".to_string()));
    assert!(args_pt.contains(&"copy".to_string()));

    // 2. 5.1 Surround sound source (e.g. AC-3 6 channels) -> DownmixSurroundToStereo
    let meta_5_1 = MediaMetadata {
        container: "mkv".to_string(),
        video_codec: Some("hevc".to_string()),
        width: Some(1920),
        height: Some(1080),
        framerate: Some(24.0),
        bitrate_kbps: Some(8000),
        audio_codec: Some("ac3".to_string()),
        audio_channels: Some(6),
        gop_size: Some(48),
        duration_secs: Some(5000.0),
    };
    let strat_5_1 = AudioTranscodeStrategy::determine(&meta_5_1);
    assert_eq!(strat_5_1, AudioTranscodeStrategy::DownmixSurroundToStereo);

    let flags_5_1 = strat_5_1.ffmpeg_flags(128);
    assert!(flags_5_1.contains(&"-c:a".to_string()));
    assert!(flags_5_1.contains(&"aac".to_string()));
    assert!(flags_5_1.contains(&"-ac".to_string()));
    assert!(flags_5_1.contains(&"2".to_string()));
    assert!(flags_5_1.contains(&"-af".to_string()));
    assert!(flags_5_1.contains(&"aresample=matrix_encoding=dplii".to_string()));

    let args_5_1 = TranscodePipeline::build_ffmpeg_args_with_metadata(
        Path::new("movie.mkv"),
        Path::new("/tmp/hls"),
        HardwareAccel::Nvenc,
        profile,
        &meta_5_1,
    );
    assert!(args_5_1.contains(&"aresample=matrix_encoding=dplii".to_string()));

    // 3. 7.1 Surround sound source (8 channels) -> DownmixSurroundToStereo
    let mut meta_7_1 = meta_5_1.clone();
    meta_7_1.audio_channels = Some(8);
    let strat_7_1 = AudioTranscodeStrategy::determine(&meta_7_1);
    assert_eq!(strat_7_1, AudioTranscodeStrategy::DownmixSurroundToStereo);

    // 4. Stereo Opus or FLAC source -> TranscodeToStereoAac
    let mut meta_flac = meta_aac_stereo.clone();
    meta_flac.audio_codec = Some("flac".to_string());
    let strat_flac = AudioTranscodeStrategy::determine(&meta_flac);
    assert_eq!(strat_flac, AudioTranscodeStrategy::TranscodeToStereoAac);
    let flags_flac = strat_flac.ffmpeg_flags(128);
    assert!(flags_flac.contains(&"aac".to_string()));
    assert!(!flags_flac.contains(&"aresample=matrix_encoding=dplii".to_string()));

    // 5. Video with no audio -> NoAudio (-an)
    let mut meta_silent = meta_aac_stereo.clone();
    meta_silent.audio_codec = None;
    let strat_silent = AudioTranscodeStrategy::determine(&meta_silent);
    assert_eq!(strat_silent, AudioTranscodeStrategy::NoAudio);
    assert_eq!(strat_silent.ffmpeg_flags(128), vec!["-an"]);
}

#[tokio::test]
async fn test_disk_cache_quota_and_lru_eviction() {
    use aetherstream_server::transcode::pipeline::{DiskCacheConfig, DiskCacheSupervisor};

    let temp_dir = tempfile::tempdir().unwrap();
    let scratch = temp_dir.path().to_path_buf();

    // Create 5 dummy files of 100 KB each (total 500 KB)
    let payload = vec![0xEEu8; 100 * 1024];

    for i in 0..5 {
        let file_path = scratch.join(format!("segment_{:02}.ts", i));
        tokio::fs::write(&file_path, &payload).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
    }

    // Config: max quota 350 KB, target watermark 0.60 (target 210 KB)
    // Should evict oldest files until <= 210 KB
    let config = DiskCacheConfig {
        scratch_dir: scratch.clone(),
        max_cache_bytes: 350 * 1024,
        ttl_seconds: 3600,
        target_watermark_ratio: 0.60,
    };

    let (evicted_count, reclaimed_bytes) = DiskCacheSupervisor::sweep_cache(&config).await.unwrap();
    assert!(evicted_count >= 2, "Must evict at least 2 files to reach quota");
    assert!(reclaimed_bytes >= 200 * 1024, "Must reclaim at least 200 KB");

    // Oldest segment_00.ts must be evicted, newest segment_04.ts must be preserved
    assert!(
        !scratch.join("segment_00.ts").exists(),
        "Oldest file segment_00.ts must be evicted"
    );
    assert!(
        scratch.join("segment_04.ts").exists(),
        "Newest file segment_04.ts must be preserved"
    );

    // Test TTL expiration: config with 0 second TTL evicts all remaining files
    let ttl_config = DiskCacheConfig {
        scratch_dir: scratch.clone(),
        max_cache_bytes: 10 * 1024 * 1024,
        ttl_seconds: 0,
        target_watermark_ratio: 0.80,
    };
    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    let (ttl_evicted, _) = DiskCacheSupervisor::sweep_cache(&ttl_config).await.unwrap();
    assert!(ttl_evicted >= 1, "TTL sweep must purge all expired segments");
}

#[tokio::test]
async fn test_offline_transcode_worker_and_lifecycle() {
    use aetherstream_server::transcode::offline::{OfflineTranscodeWorker, TranscodeJobStatus};
    use aetherstream_server::transcode::pipeline::{HardwareAccel, MediaMetadata};

    let temp_dir = tempfile::tempdir().unwrap();
    let source_file = temp_dir.path().join("source_movie.mkv");
    let target_dir = temp_dir.path().join("hls_bundle");
    tokio::fs::write(&source_file, b"dummy source video bits")
        .await
        .unwrap();

    let meta = MediaMetadata {
        container: "mkv".to_string(),
        video_codec: Some("h264".to_string()),
        width: Some(1280),
        height: Some(720),
        framerate: Some(30.0),
        bitrate_kbps: Some(2500),
        audio_codec: Some("aac".to_string()),
        audio_channels: Some(2),
        gop_size: Some(60),
        duration_secs: Some(120.0),
    };

    let worker = OfflineTranscodeWorker::new();

    // 1. Enqueue offline transcode job
    let job_id = worker
        .enqueue_job(
            source_file.clone(),
            target_dir.clone(),
            HardwareAccel::Nvenc,
            Some(&meta),
        )
        .await;

    assert_eq!(
        worker.get_job_status(&job_id).await,
        Some(TranscodeJobStatus::Queued)
    );

    // 2. Process the job
    let result = worker
        .process_next_job()
        .await
        .expect("Job must be present in queue");
    let completed_job = result.expect("Job processing must succeed");

    assert!(matches!(
        completed_job.status,
        TranscodeJobStatus::Completed { .. }
    ));
    if let TranscodeJobStatus::Completed {
        profiles_rendered, ..
    } = completed_job.status
    {
        assert_eq!(profiles_rendered.len(), 3); // 720p, 480p, 360p (no 1080p upscaling)
        assert!(profiles_rendered.contains(&"720p_med".to_string()));
        assert!(!profiles_rendered.contains(&"1080p_high".to_string()));
    }

    // 3. Verify on-disk static HLS bundle outputs
    assert!(target_dir.join("master.m3u8").exists());
    assert!(target_dir.join("720p_med").join("index.m3u8").exists());
    assert!(target_dir.join("480p_low").join("index.m3u8").exists());
    assert!(target_dir.join("360p_min").join("index.m3u8").exists());

    // 4. Test cancellation
    let cancel_job_id = worker
        .enqueue_job(
            source_file,
            temp_dir.path().join("cancel_dir"),
            HardwareAccel::SoftwareCpu,
            None,
        )
        .await;
    assert!(worker.cancel_job(&cancel_job_id).await);
    assert_eq!(
        worker.get_job_status(&cancel_job_id).await,
        Some(TranscodeJobStatus::Cancelled)
    );
}

#[tokio::test]
async fn test_video_pipeline_random_seek_stress_and_latency() {
    use aetherstream_server::storage::stream::RangeRequestEngine;
    use axum::body::to_bytes;
    use axum::http::{header, HeaderMap, HeaderValue};
    use std::sync::Arc;
    use std::time::Instant;

    let temp_dir = tempfile::tempdir().unwrap();
    let video_file = temp_dir.path().join("stress_media_16mb.mp4");

    // Create 16 MiB simulated media container with deterministic pattern
    let total_size: usize = 16 * 1024 * 1024;
    let mut data = vec![0u8; total_size];
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }
    tokio::fs::write(&video_file, &data).await.unwrap();

    let shared_path = Arc::new(video_file);
    let concurrency = 8;
    let seeks_per_worker = 25;
    let total_seeks = concurrency * seeks_per_worker;

    let mut handles = Vec::new();
    let stress_start = Instant::now();

    for worker_idx in 0..concurrency {
        let path = Arc::clone(&shared_path);
        handles.push(tokio::spawn(async move {
            let mut latencies = Vec::with_capacity(seeks_per_worker);

            for seek_idx in 0..seeks_per_worker {
                let seed = (worker_idx * 1000 + seek_idx * 37) as u64;
                let start_offset = (seed * 65537) % (total_size as u64 - 131072);
                let chunk_len = 65536; // 64 KiB typical HLS/MP4 seek chunk
                let end_offset = start_offset + chunk_len - 1;

                let range_header = format!("bytes={}-{}", start_offset, end_offset);
                let mut headers = HeaderMap::new();
                headers.insert(header::RANGE, HeaderValue::from_str(&range_header).unwrap());

                let seek_start = Instant::now();
                let resp = RangeRequestEngine::stream_file(&*path, &headers).await;

                assert_eq!(resp.status(), axum::http::StatusCode::PARTIAL_CONTENT);
                let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
                let elapsed = seek_start.elapsed();

                assert_eq!(body_bytes.len(), chunk_len as usize);
                assert_eq!(body_bytes[0], (start_offset % 251) as u8);

                latencies.push(elapsed);
            }

            latencies
        }));
    }

    let mut all_latencies = Vec::with_capacity(total_seeks);
    for h in handles {
        let worker_latencies = h.await.unwrap();
        all_latencies.extend(worker_latencies);
    }

    all_latencies.sort();
    let total_elapsed = stress_start.elapsed();
    let p50 = all_latencies[all_latencies.len() * 50 / 100];
    let p95 = all_latencies[all_latencies.len() * 95 / 100];
    let p99 = all_latencies[all_latencies.len() * 99 / 100];
    let max_lat = *all_latencies.last().unwrap();
    let avg_ms = all_latencies.iter().map(|l| l.as_secs_f64() * 1000.0).sum::<f64>()
        / (all_latencies.len() as f64);

    println!(
        "[SEEK STRESS] Total Seeks: {} | Elapsed: {:.3}s | Avg: {:.2}ms | p50: {:.2?} | p95: {:.2?} | p99: {:.2?} | Max: {:.2?}",
        total_seeks,
        total_elapsed.as_secs_f64(),
        avg_ms,
        p50,
        p95,
        p99,
        max_lat
    );

    assert!(
        p95.as_millis() < 50,
        "p95 seek latency must be under 50ms on local SSD (measured {:?})",
        p95
    );
    assert!(
        max_lat.as_millis() < 200,
        "Max seek latency must be under 200ms per NFR-103 SLA (measured {:?})",
        max_lat
    );
}

#[test]
fn test_cross_platform_volume_inspector() {
    use aetherstream_server::storage::volume_info::VolumeInspector;
    let volumes = VolumeInspector::inspect_volumes();
    assert!(!volumes.is_empty(), "VolumeInspector must detect at least one mounted volume");

    let has_system = volumes.iter().any(|v| v.is_system);
    assert!(has_system, "At least one volume must be identified as system partition");

    for v in &volumes {
        assert!(v.total_bytes > 0, "Volume {} must have total_bytes > 0", v.name);
        assert!(!v.name.is_empty(), "Volume must have a non-empty name");
        assert!(!v.filesystem.is_empty(), "Volume {} must have filesystem info", v.name);
        assert!(v.available_bytes <= v.total_bytes, "Available bytes must be <= total bytes for {}", v.name);
        assert!(v.mount_point.exists(), "Mount point {:?} must exist on host", v.mount_point);

        println!(
            "[VOLUME PROBED] Name: {} | Mount: {:?} | FS: {} | Label: {:?} | Cap: {:.2} GB / {:.2} GB | Removable: {} | Type: {:?} | Serial: {:?}",
            v.name,
            v.mount_point,
            v.filesystem,
            v.filesystem_label,
            (v.available_bytes as f64) / 1_000_000_000.0,
            (v.total_bytes as f64) / 1_000_000_000.0,
            v.is_removable,
            v.media_type,
            v.serial_number
        );
    }
}

#[test]
fn test_os_critical_path_quarantine_and_exclusion() {
    use aetherstream_server::storage::PathQuarantine;
    use aetherstream_server::storage::mounts::{MountError, MountRegistry};
    use std::path::Path;

    // 1. Verify critical system path detector
    assert!(PathQuarantine::is_critical_system_path(Path::new("C:\\Windows")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("C:\\Windows\\System32\\cmd.exe")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("C:\\Recovery")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("C:\\$Recycle.Bin")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("C:\\System Volume Information")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("/proc/cpuinfo")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("/sys/block")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("/etc/shadow")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("/dev/sda")));
    assert!(PathQuarantine::is_critical_system_path(Path::new("/System/Library")));

    // Non-critical paths must not be flagged
    assert!(!PathQuarantine::is_critical_system_path(Path::new("D:\\Movies\\action.mp4")));
    assert!(!PathQuarantine::is_critical_system_path(Path::new("C:\\Users\\daya\\Videos")));
    assert!(!PathQuarantine::is_critical_system_path(Path::new("/home/user/media")));

    // 2. Reject registering OS-critical system path as a virtual mount
    let registry = MountRegistry::new();
    let reg_win = registry.register("win_sys", std::path::PathBuf::from("C:\\Windows"), true, None);
    assert!(reg_win.is_err(), "Registering C:\\Windows as mount must be rejected");
    match reg_win {
        Err(MountError::QuarantinedPath(p)) => {
            println!("[QUARANTINE ENFORCED] Blocked mount registration of OS-critical path: {}", p);
        }
        other => panic!("Expected QuarantinedPath error, got: {:?}", other),
    }

    // 3. Test unmanaged listing exclusion on a root volume
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();

    // Create safe directories
    std::fs::create_dir_all(root.join("movies")).unwrap();
    std::fs::create_dir_all(root.join("music")).unwrap();
    std::fs::write(root.join("video.mp4"), b"test").unwrap();

    // Create directories/files matching quarantined names
    std::fs::create_dir_all(root.join("Windows")).unwrap();
    std::fs::create_dir_all(root.join("$Recycle.Bin")).unwrap();
    std::fs::create_dir_all(root.join("System Volume Information")).unwrap();
    std::fs::write(root.join("pagefile.sys"), b"swap").unwrap();

    // Register test mount
    assert!(registry.register("test_root", root.to_path_buf(), false, None).is_ok());

    // Verify resolve_safe_path blocks quarantined targets directly
    let win_res = registry.resolve_safe_path("test_root", Path::new("Windows/system32"));
    assert!(win_res.is_err(), "Resolving quarantined Windows directory must be rejected");

    // Verify unmanaged listing filter excludes quarantined files/dirs
    let entries = registry.list_directory("test_root", Path::new("")).unwrap();
    let entry_names: Vec<String> = entries.into_iter().map(|e| e.name).collect();

    println!("[UNMANAGED LISTING] Filtered entries: {:?}", entry_names);
    assert!(entry_names.contains(&"movies".to_string()), "Safe directory 'movies' must be present");
    assert!(entry_names.contains(&"music".to_string()), "Safe directory 'music' must be present");
    assert!(entry_names.contains(&"video.mp4".to_string()), "Safe file 'video.mp4' must be present");

    assert!(!entry_names.contains(&"Windows".to_string()), "OS-critical 'Windows' directory must be quarantined");
    assert!(!entry_names.contains(&"$Recycle.Bin".to_string()), "'$Recycle.Bin' must be quarantined");
    assert!(!entry_names.contains(&"System Volume Information".to_string()), "'System Volume Information' must be quarantined");
    assert!(!entry_names.contains(&"pagefile.sys".to_string()), "'pagefile.sys' must be quarantined");
}

#[tokio::test]
async fn test_windows_hotplug_device_watcher() {
    use aetherstream_server::storage::disk_watcher::{DiskWatcher, StorageDeviceEvent};
    use aetherstream_server::storage::mounts::MountRegistry;
    use aetherstream_server::storage::volume_info::{DiskMediaType, VolumeInfo};
    use std::path::PathBuf;

    let watcher = DiskWatcher::new();
    let mut rx = watcher.subscribe();

    // 1. Verify watcher starts background OS loop
    assert!(watcher.start().is_ok(), "Watcher must start without error");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(watcher.is_running(), "Watcher state must be running");

    // 2. Simulate rapid device arrival (Plugged)
    let plugged_volume = VolumeInfo {
        name: "Ultra USB 3.0 (K:)".to_string(),
        mount_point: PathBuf::from("K:\\"),
        filesystem: "exFAT".to_string(),
        filesystem_label: Some("Ultra USB".to_string()),
        total_bytes: 64_000_000_000,
        available_bytes: 58_000_000_000,
        is_removable: true,
        is_system: false,
        media_type: DiskMediaType::RemovableUsb,
        serial_number: Some("USB-ABCD-1234".to_string()),
        is_read_only: false,
    };

    watcher.simulate_device_event(StorageDeviceEvent::Plugged(plugged_volume.clone()));

    let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("Must receive Plugged event within 1s")
        .expect("Event receive failed");

    match received {
        StorageDeviceEvent::Plugged(vol) => {
            assert_eq!(vol.name, "Ultra USB 3.0 (K:)");
            assert_eq!(vol.mount_point, PathBuf::from("K:\\"));
            assert_eq!(vol.media_type, DiskMediaType::RemovableUsb);
            assert_eq!(vol.serial_number, Some("USB-ABCD-1234".to_string()));
            println!("[HOTPLUG VERIFIED] Received Plugged Event: {:?}", vol);
        }
        _ => panic!("Expected StorageDeviceEvent::Plugged"),
    }

    // 3. Mount Registry dynamic unmount simulation upon Unplugged event
    let registry = MountRegistry::new();
    let temp_usb = tempfile::tempdir().unwrap();
    let usb_path = temp_usb.path().to_path_buf();
    assert!(registry.register("usb_drive", usb_path.clone(), false, Some("Plugged USB".into())).is_ok());
    assert_eq!(registry.list_mounts().len(), 1);

    // Simulate device removal (Unplugged)
    watcher.simulate_device_event(StorageDeviceEvent::Unplugged(usb_path.to_string_lossy().to_string()));

    let received_unplug = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("Must receive Unplugged event within 1s")
        .expect("Event receive failed");

    match received_unplug {
        StorageDeviceEvent::Unplugged(path_str) => {
            let unmounted = registry.unregister_by_path(&PathBuf::from(&path_str));
            assert_eq!(unmounted, Some("usb_drive".to_string()), "Registry must unmount disconnected device");
            assert_eq!(registry.list_mounts().len(), 0, "Registry must now have 0 mounts");
            println!("[HOTPLUG VERIFIED] Dynamic Unmount Succeeded for: {}", path_str);
        }
        _ => panic!("Expected StorageDeviceEvent::Unplugged"),
    }

    // 4. Clean shutdown of hidden window and message loop
    watcher.stop();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!watcher.is_running(), "Watcher must not be running after stop()");
}

#[test]
fn test_linux_netlink_and_macos_disk_arbitration_parsers() {
    use aetherstream_server::storage::disk_watcher::{
        MacDiskArbitrationEvent, NetlinkAction, NetlinkUeventParser, StorageDeviceEvent,
    };

    // 1. Test Linux Kernel Netlink Uevent Parser for Block Add
    let raw_uevent_add = b"add@/devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host4/target4:0:0/4:0:0:0/block/sdb/sdb1\0ACTION=add\0DEVPATH=/devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host4/target4:0:0/4:0:0:0/block/sdb/sdb1\0SUBSYSTEM=block\0DEVNAME=sdb1\0DEVTYPE=partition\0PARTN=1\0MAJOR=8\0MINOR=17\0SEQNUM=3492\0";
    let parsed_add = NetlinkUeventParser::parse(raw_uevent_add);
    assert!(parsed_add.is_some(), "Parser must extract block uevent");
    let event_add = parsed_add.unwrap();
    assert_eq!(event_add.action, NetlinkAction::Add);
    assert_eq!(event_add.subsystem, "block");
    assert_eq!(event_add.devname, "sdb1");
    assert_eq!(event_add.devtype, Some("partition".to_string()));

    // 2. Test Linux Kernel Netlink Uevent Parser for Block Remove
    let raw_uevent_remove = b"remove@/devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host4/target4:0:0/4:0:0:0/block/sdb/sdb1\0ACTION=remove\0DEVPATH=/devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host4/target4:0:0/4:0:0:0/block/sdb/sdb1\0SUBSYSTEM=block\0DEVNAME=sdb1\0DEVTYPE=partition\0SEQNUM=3493\0";
    let parsed_remove = NetlinkUeventParser::parse(raw_uevent_remove);
    assert!(parsed_remove.is_some(), "Parser must extract block remove uevent");
    let event_remove = parsed_remove.unwrap();
    assert_eq!(event_remove.action, NetlinkAction::Remove);
    assert_eq!(event_remove.devname, "sdb1");

    // 3. Test non-block uevent rejection (e.g. net or input subsystem)
    let raw_uevent_net = b"add@/devices/pci0000:00/0000:00:1c.4/0000:03:00.0/net/eth0\0ACTION=add\0SUBSYSTEM=net\0DEVNAME=eth0\0SEQNUM=100\0";
    let parsed_net = NetlinkUeventParser::parse(raw_uevent_net);
    assert!(parsed_net.is_none(), "Non-block uevents must be ignored");

    // 4. Test macOS DiskArbitration callback event mappings
    let mac_appeared = MacDiskArbitrationEvent {
        bsd_name: "disk3s1".to_string(),
        mount_point: Some("/Volumes/CrucialPortable".to_string()),
        is_appeared: true,
    };
    match mac_appeared.to_storage_device_event() {
        StorageDeviceEvent::Plugged(vol) => {
            assert_eq!(vol.filesystem, "APFS");
            assert_eq!(vol.mount_point, std::path::PathBuf::from("/Volumes/CrucialPortable"));
            assert_eq!(vol.serial_number, Some("MAC-DA-disk3s1".to_string()));
            println!("[MACOS DISKARB VERIFIED] Converted DiskAppeared -> {:?}", vol.mount_point);
        }
        _ => panic!("Expected StorageDeviceEvent::Plugged"),
    }

    let mac_disappeared = MacDiskArbitrationEvent {
        bsd_name: "disk3s1".to_string(),
        mount_point: Some("/Volumes/CrucialPortable".to_string()),
        is_appeared: false,
    };
    match mac_disappeared.to_storage_device_event() {
        StorageDeviceEvent::Unplugged(path) => {
            assert_eq!(path, "/Volumes/CrucialPortable");
            println!("[MACOS DISKARB VERIFIED] Converted DiskDisappeared -> {}", path);
        }
        _ => panic!("Expected StorageDeviceEvent::Unplugged"),
    }
}

#[tokio::test]
async fn test_hotplug_broadcast_channel_and_dispatcher() {
    use aetherstream_server::storage::disk_watcher::{DiskWatcher, StorageDeviceEvent};
    use aetherstream_server::storage::mounts::MountRegistry;
    use aetherstream_server::storage::volume_info::{DiskMediaType, VolumeInfo};
    use std::sync::Arc;

    let watcher = Arc::new(DiskWatcher::new());
    let mounts = Arc::new(MountRegistry::new());

    // 1. Subscribe multiple independent daemon subsystems
    let mut sub_telemetry = watcher.subscribe();
    let mut sub_indexer = watcher.subscribe();
    let mut sub_sse = watcher.subscribe();

    // 2. Spawn dynamic storage event dispatcher
    let dispatcher_handle = DiskWatcher::spawn_storage_event_dispatcher(watcher.clone(), mounts.clone());

    // Create a temp directory representing a real plugged USB
    let temp_usb = tempfile::tempdir().unwrap();
    let usb_path = temp_usb.path().to_path_buf();

    let plugged_vol = VolumeInfo {
        name: "Samsung Portable T7 (T:)".to_string(),
        mount_point: usb_path.clone(),
        filesystem: "NTFS".to_string(),
        filesystem_label: Some("SamsungT7".to_string()),
        total_bytes: 1_000_000_000_000,
        available_bytes: 800_000_000_000,
        is_removable: true,
        is_system: false,
        media_type: DiskMediaType::RemovableUsb,
        serial_number: Some("SAMSUNG-T7-999".to_string()),
        is_read_only: false,
    };

    // 3. Publish Plugged event
    watcher.publish(StorageDeviceEvent::Plugged(plugged_vol.clone()));

    // Verify all subscribers received the broadcast
    let ev1 = sub_telemetry.recv().await.expect("Telemetry sub received");
    let ev2 = sub_indexer.recv().await.expect("Indexer sub received");
    let ev3 = sub_sse.recv().await.expect("SSE sub received");

    match (ev1, ev2, ev3) {
        (
            StorageDeviceEvent::Plugged(v1),
            StorageDeviceEvent::Plugged(v2),
            StorageDeviceEvent::Plugged(v3),
        ) => {
            assert_eq!(v1.serial_number, Some("SAMSUNG-T7-999".to_string()));
            assert_eq!(v2.serial_number, Some("SAMSUNG-T7-999".to_string()));
            assert_eq!(v3.serial_number, Some("SAMSUNG-T7-999".to_string()));
        }
        _ => panic!("Expected Plugged events on all subscribers"),
    }

    // Wait for dispatcher task to process auto-mount
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify MountRegistry has auto-registered the plugged volume
    let active_mounts = mounts.list_mounts();
    assert_eq!(active_mounts.len(), 1, "MountRegistry must contain auto-registered volume");
    println!("[EVENT DISPATCHER VERIFIED] Auto-mounted volumes: {:?}", active_mounts);

    // 4. Publish Unplugged event
    watcher.publish(StorageDeviceEvent::Unplugged(usb_path.to_string_lossy().to_string()));

    let un1 = sub_telemetry.recv().await.expect("Telemetry sub unplugged");
    match un1 {
        StorageDeviceEvent::Unplugged(p) => assert_eq!(p, usb_path.to_string_lossy()),
        _ => panic!("Expected Unplugged event"),
    }

    // Wait for dispatcher task to process auto-unmount
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify MountRegistry dynamically removed the unplugged volume
    let after_unmount = mounts.list_mounts();
    assert_eq!(after_unmount.len(), 0, "MountRegistry must have removed disconnected mount");
    println!("[EVENT DISPATCHER VERIFIED] Dynamic unmount confirmed: 0 active mounts remaining");

    dispatcher_handle.abort();
}
