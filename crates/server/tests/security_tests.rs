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
    }
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

    let chunk1_data = b"chunk_1_payload_bytes";
    let mut hasher1 = Sha256::new();
    hasher1.update(chunk1_data);
    let chunk1_hash = hex::encode(hasher1.finalize());

    // Write valid chunk 1
    let write_res1 = ResumableUploadManager::write_chunk(&target_file, 0, &chunk1_hash, chunk1_data).await;
    assert!(write_res1.is_ok());

    // Write chunk 2 with corrupted checksum
    let corrupted_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    let write_res2 = ResumableUploadManager::write_chunk(&target_file, 1, corrupted_hash, b"chunk_2_corrupted").await;
    assert!(write_res2.is_err(), "Corrupted checksum must be rejected");
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

    let search_res = indexer.search("glad");
    assert_eq!(search_res.len(), 1);
    assert_eq!(search_res[0].name, "gladiator.mp4");
}
