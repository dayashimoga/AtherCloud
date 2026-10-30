use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

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
    #[error("Incomplete upload: {0} chunks missing ({1:?})")]
    IncompleteUpload(usize, Vec<usize>),
    #[error("Invalid chunk index {0}: exceeds total chunks {1}")]
    InvalidChunkIndex(usize, usize),
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

impl Default for ResumableUploadManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResumableUploadManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initializes a new resumable upload session with disk pre-allocation for concurrent multi-part slices
    pub async fn init_session(
        &self,
        upload_id: &str,
        target_path: PathBuf,
        total_size: u64,
        chunk_size: u64,
    ) -> Result<UploadSession, UploadError> {
        let chunk_size = if chunk_size == 0 {
            2 * 1024 * 1024 // 2 MiB default slice
        } else {
            chunk_size
        };
        let total_chunks = if total_size == 0 {
            1
        } else {
            ((total_size as f64) / (chunk_size as f64)).ceil() as usize
        };

        let mut temp_path = target_path.clone();
        temp_path.set_extension(format!("part_{}", upload_id));

        if let Some(parent) = temp_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Pre-allocate destination file to total size to avoid write race conditions across non-overlapping slices
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .await?;
        if total_size > 0 {
            file.set_len(total_size).await?;
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
        info!(
            "Initialized pre-allocated resumable upload session '{}' (total={} bytes, chunks={}, slice_size={})",
            upload_id, total_size, total_chunks, chunk_size
        );

        Ok(session)
    }

    /// Retrieves session status for pausing, querying, and resuming
    pub async fn get_session(&self, upload_id: &str) -> Option<UploadSession> {
        let sessions = self.sessions.read().await;
        sessions.get(upload_id).cloned()
    }

    /// Computes missing chunk indices required to resume an incomplete upload session
    pub async fn get_missing_chunks(&self, upload_id: &str) -> Result<Vec<usize>, UploadError> {
        let session = self
            .get_session(upload_id)
            .await
            .ok_or_else(|| UploadError::SessionNotFound(upload_id.to_string()))?;

        let missing: Vec<usize> = (0..session.total_chunks)
            .filter(|idx| !session.completed_chunks.contains(idx))
            .collect();
        Ok(missing)
    }

    /// Writes a chunk directly to target path (stateless / simple mode) with Web Crypto SHA-256 validation
    pub async fn write_chunk(
        target_path: &Path,
        chunk_index: usize,
        expected_sha256: &str,
        data: &[u8],
    ) -> Result<u64, UploadError> {
        let calculated_hash = Self::hash_chunk(data).await;

        if !expected_sha256.is_empty()
            && calculated_hash.to_lowercase() != expected_sha256.to_lowercase()
        {
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

    /// Writes a verified chunk into a tracked session at exact byte offset with concurrent hashing & I/O
    pub async fn write_session_chunk(
        &self,
        upload_id: &str,
        chunk_index: usize,
        expected_sha256: &str,
        data: &[u8],
    ) -> Result<UploadSession, UploadError> {
        // 1. Snapshot session geometry without holding write lock during expensive I/O
        let (temp_path, chunk_size, total_chunks, is_completed) = {
            let sessions = self.sessions.read().await;
            let sess = sessions
                .get(upload_id)
                .ok_or_else(|| UploadError::SessionNotFound(upload_id.to_string()))?;
            (
                sess.temp_path.clone(),
                sess.chunk_size,
                sess.total_chunks,
                sess.is_completed,
            )
        };

        if is_completed {
            return Err(UploadError::AlreadyCompleted);
        }

        if chunk_index >= total_chunks {
            return Err(UploadError::InvalidChunkIndex(chunk_index, total_chunks));
        }

        // 2. Concurrently compute & verify Web Crypto SHA-256 checksum
        let calculated_hash = Self::hash_chunk(data).await;
        if !expected_sha256.is_empty()
            && calculated_hash.to_lowercase() != expected_sha256.to_lowercase()
        {
            return Err(UploadError::ChecksumMismatch(
                chunk_index,
                expected_sha256.to_string(),
                calculated_hash,
            ));
        }

        // 3. Write chunk at calculated byte offset using dedicated handle
        let offset = (chunk_index as u64) * chunk_size;
        let mut file = OpenOptions::new()
            .write(true)
            .open(&temp_path)
            .await?;

        file.seek(SeekFrom::Start(offset)).await?;
        file.write_all(data).await?;
        file.flush().await?;

        // 4. Update session tracking under quick write lock
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(upload_id)
            .ok_or_else(|| UploadError::SessionNotFound(upload_id.to_string()))?;

        if !session.completed_chunks.contains(&chunk_index) {
            session.completed_chunks.push(chunk_index);
            session.completed_chunks.sort_unstable();
            session.received_bytes += data.len() as u64;
        }

        debug!(
            "Session '{}' chunk {}/{} verified and persisted (progress={}/{} bytes)",
            upload_id,
            chunk_index + 1,
            session.total_chunks,
            session.received_bytes,
            session.total_size
        );

        Ok(session.clone())
    }

    /// Asynchronously computes SHA-256 for chunk data, offloading to worker pool for chunks > 256 KiB
    pub async fn hash_chunk(data: &[u8]) -> String {
        if data.len() > 256 * 1024 {
            let chunk_vec = data.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut hasher = Sha256::new();
                hasher.update(&chunk_vec);
                hex::encode(hasher.finalize())
            })
            .await
            .unwrap_or_else(|_| {
                let mut hasher = Sha256::new();
                hasher.update(data);
                hex::encode(hasher.finalize())
            })
        } else {
            let mut hasher = Sha256::new();
            hasher.update(data);
            hex::encode(hasher.finalize())
        }
    }

    /// Finalizes the upload by verifying completeness, full-file SHA-256 integrity,
    /// and executing atomic assembly via filesystem rename.
    pub async fn complete_session(
        &self,
        upload_id: &str,
        expected_file_sha256: Option<&str>,
    ) -> Result<PathBuf, UploadError> {
        let (temp_path, target_path, total_chunks, completed_chunks) = {
            let sessions = self.sessions.read().await;
            let sess = sessions
                .get(upload_id)
                .ok_or_else(|| UploadError::SessionNotFound(upload_id.to_string()))?;
            (
                sess.temp_path.clone(),
                sess.target_path.clone(),
                sess.total_chunks,
                sess.completed_chunks.clone(),
            )
        };

        // 1. Verify all chunk slices have been received
        let missing: Vec<usize> = (0..total_chunks)
            .filter(|idx| !completed_chunks.contains(idx))
            .collect();
        if !missing.is_empty() {
            return Err(UploadError::IncompleteUpload(missing.len(), missing));
        }

        // 2. If full file checksum provided, verify entire assembled file using bounded 64 KiB chunks
        if let Some(expected_hash) = expected_file_sha256 {
            let mut file = File::open(&temp_path).await?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];

            loop {
                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }
                hasher.update(&buffer[..bytes_read]);
            }

            let actual_hash = hex::encode(hasher.finalize());
            if actual_hash.to_lowercase() != expected_hash.to_lowercase() {
                return Err(UploadError::ChecksumMismatch(
                    0,
                    expected_hash.to_string(),
                    actual_hash,
                ));
            }
        }

        // 3. Atomically assemble part file into target canonical path
        tokio::fs::rename(&temp_path, &target_path).await?;

        // 4. Mark session completed
        let mut sessions = self.sessions.write().await;
        if let Some(sess) = sessions.get_mut(upload_id) {
            sess.is_completed = true;
        }

        info!(
            "Resumable upload session '{}' atomically assembled -> {:?}",
            upload_id, target_path
        );

        Ok(target_path)
    }
}

fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("UTC-EPOCH-{}", duration.as_secs())
}
