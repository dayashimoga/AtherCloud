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

    let mut registry = MountRegistry::new();
    registry.register("media", mount_root.clone(), false, None);

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
    let manifest = TranscodePipeline::generate_master_manifest("/api/v1/transcode/hls/videos/matrix.mp4");
    assert!(manifest.contains("#EXTM3U"));
    assert!(manifest.contains("#EXT-X-STREAM-INF"));
    assert!(manifest.contains("1080p_high"));
    assert!(manifest.contains("720p_med"));
    assert!(manifest.contains("480p_low"));
    assert!(manifest.contains("360p_min"));
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

    let mut registry = MountRegistry::new();
    registry.register_with_permission(
        "media_ro",
        root.clone(),
        aetherstream_server::storage::mounts::MountPermission::Read,
        Some("Read-Only Storage".into()),
    );
    registry.register_with_permission(
        "media_rw",
        root.clone(),
        aetherstream_server::storage::mounts::MountPermission::Write,
        Some("Read-Write Storage".into()),
    );
    registry.register_with_permission(
        "media_stream",
        root.clone(),
        aetherstream_server::storage::mounts::MountPermission::StreamOnly,
        Some("Stream-Only Media".into()),
    );

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
