use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("Checksum mismatch for chunk index {0}: expected {1}, got {2}")]
    ChecksumMismatch(usize, String, String),
    #[error("I/O error during chunk write: {0}")]
    Io(#[from] std::io::Error),
    #[error("Upload session not found: {0}")]
    SessionNotFound(String),
    #[error("Invalid chunk offset {0} for chunk index {1}")]
    InvalidOffset(u64, usize),
    #[error("Upload session already completed")]
    AlreadyCompleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    pub upload_id: String,
    pub target_path: PathBuf,
    pub temp_path: PathBuf,
    pub total_size: u64,
    pub chunk_size: u64,
    pub total_chunks: usize,
    pub received_bytes: u64,
    pub completed_chunks: Vec<usize>,
    pub is_completed: bool,
    pub created_at_utc: String,
}

#[derive(Debug, Clone)]
pub struct ResumableUploadManager {
    sessions: Arc<RwLock<HashMap<String, UploadSession>>>,
}

impl ResumableUploadManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initializes a new resumable upload session
    pub async fn init_session(
        &self,
        upload_id: &str,
        target_path: PathBuf,
        total_size: u64,
        chunk_size: u64,
    ) -> Result<UploadSession, UploadError> {
        let chunk_size = if chunk_size == 0 { 2 * 1024 * 1024 } else { chunk_size }; // default 2 MiB
        let total_chunks = ((total_size as f64) / (chunk_size as f64)).ceil() as usize;
        let total_chunks = if total_chunks == 0 { 1 } else { total_chunks };

        let mut temp_path = target_path.clone();
        temp_path.set_extension(format!("part_{}", upload_id));

        if let Some(parent) = temp_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let session = UploadSession {
            upload_id: upload_id.to_string(),
            target_path,
            temp_path,
            total_size,
            chunk_size,
            total_chunks,
            received_bytes: 0,
            completed_chunks: Vec::new(),
            is_completed: false,
            created_at_utc: chrono_like_now(),
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(upload_id.to_string(), session.clone());
        info!("Initialized resumable upload session '{}' (total={} bytes, chunks={})", upload_id, total_size, total_chunks);

        Ok(session)
    }

    /// Retrieves session status for pausing and resuming
    pub async fn get_session(&self, upload_id: &str) -> Option<UploadSession> {
        let sessions = self.sessions.read().await;
        sessions.get(upload_id).cloned()
    }

    /// Writes a chunk directly to target path (stateless / simple mode)
    pub async fn write_chunk(
        target_path: &Path,
        chunk_index: usize,
        expected_sha256: &str,
        data: &[u8],
    ) -> Result<u64, UploadError> {
        // 1. Verify chunk SHA-256
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
        Ok(new_size)
    }

    /// Writes a verified chunk into a tracked session at exact byte offset (resumable mode)
    pub async fn write_session_chunk(
        &self,
        upload_id: &str,
        chunk_index: usize,
        expected_sha256: &str,
        data: &[u8],
    ) -> Result<UploadSession, UploadError> {
        // 1. Retrieve session
        let mut session = {
            let sessions = self.sessions.read().await;
            sessions
                .get(upload_id)
                .cloned()
                .ok_or_else(|| UploadError::SessionNotFound(upload_id.to_string()))?
        };

        if session.is_completed {
            return Err(UploadError::AlreadyCompleted);
        }

        // 2. Verify chunk SHA-256 checksum
        let mut hasher = Sha256::new();
        hasher.update(data);
        let calculated_hash = hex::encode(hasher.finalize());

        if !expected_sha256.is_empty() && calculated_hash.to_lowercase() != expected_sha256.to_lowercase() {
            return Err(UploadError::ChecksumMismatch(
                chunk_index,
                expected_sha256.to_string(),
                calculated_hash,
            ));
        }

        // 3. Write chunk at calculated byte offset
        let offset = (chunk_index as u64) * session.chunk_size;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&session.temp_path)
            .await?;

        file.seek(SeekFrom::Start(offset)).await?;
        file.write_all(data).await?;
        file.flush().await?;

        // 4. Update session tracking
        if !session.completed_chunks.contains(&chunk_index) {
            session.completed_chunks.push(chunk_index);
            session.completed_chunks.sort_unstable();
            session.received_bytes += data.len() as u64;
        }

        let mut sessions = self.sessions.write().await;
        sessions.insert(upload_id.to_string(), session.clone());

        info!(
            "Session '{}' chunk {}/{} verified and persisted (progress={}/{} bytes)",
            upload_id,
            chunk_index + 1,
            session.total_chunks,
            session.received_bytes,
            session.total_size
        );

        Ok(session)
    }

    /// Finalizes the upload by moving the completed .part file to its canonical target destination
    pub async fn complete_session(
        &self,
        upload_id: &str,
        expected_file_sha256: Option<&str>,
    ) -> Result<PathBuf, UploadError> {
        let mut session = {
            let sessions = self.sessions.read().await;
            sessions
                .get(upload_id)
                .cloned()
                .ok_or_else(|| UploadError::SessionNotFound(upload_id.to_string()))?
        };

        // If file checksum provided, verify entire assembled file
        if let Some(expected_hash) = expected_file_sha256 {
            let data = tokio::fs::read(&session.temp_path).await?;
            let mut hasher = Sha256::new();
            hasher.update(&data);
            let actual_hash = hex::encode(hasher.finalize());
            if actual_hash.to_lowercase() != expected_hash.to_lowercase() {
                return Err(UploadError::ChecksumMismatch(
                    0,
                    expected_hash.to_string(),
                    actual_hash,
                ));
            }
        }

        // Atomically rename .part file to target
        tokio::fs::rename(&session.temp_path, &session.target_path).await?;
        session.is_completed = true;

        let mut sessions = self.sessions.write().await;
        sessions.insert(upload_id.to_string(), session.clone());
        info!("Resumable upload session '{}' finalized -> {:?}", upload_id, session.target_path);

        Ok(session.target_path)
    }
}

fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("UTC-EPOCH-{}", duration.as_secs())
}
