use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("Checksum mismatch for chunk index {0}: expected {1}, got {2}")]
    ChecksumMismatch(usize, String, String),
    #[error("I/O error during chunk write: {0}")]
    Io(#[from] std::io::Error),
    #[error("Upload session invalid: {0}")]
    SessionInvalid(String),
}

pub struct ResumableUploadManager;

impl ResumableUploadManager {
    /// Appends a verified chunk to target destination with SHA-256 hash verification
    pub async fn write_chunk(
        target_path: &Path,
        chunk_index: usize,
        expected_sha256: &str,
        data: &[u8],
    ) -> Result<u64, UploadError> {
        // 1. Verify chunk hash
        let mut hasher = Sha256::new();
        hasher.update(data);
        let calculated_hash = hex::encode(hasher.finalize());

        if !expected_sha256.is_empty() && calculated_hash.to_lowercase() != expected_sha256.to_lowercase() {
            error!(
                "Chunk {} checksum failed! Expected {}, got {}",
                chunk_index, expected_sha256, calculated_hash
            );
            return Err(UploadError::ChecksumMismatch(
                chunk_index,
                expected_sha256.to_string(),
                calculated_hash,
            ));
        }

        // 2. Append chunk to file
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(target_path)
            .await?;

        file.write_all(data).await?;
        file.flush().await?;

        let new_size = file.metadata().await?.len();
        info!(
            "Appended chunk {} ({} bytes). Total file size: {} bytes at {:?}",
            chunk_index,
            data.len(),
            new_size,
            target_path
        );

        Ok(new_size)
    }
}
