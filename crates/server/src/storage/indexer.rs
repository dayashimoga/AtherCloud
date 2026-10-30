use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFileRecord {
    pub mount_id: String,
    pub relative_path: String,
    pub name: String,
    pub size: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountIndexStats {
    pub total_files: usize,
    pub total_bytes: u64,
}

pub struct FileTreeIndexer {
    cache: RwLock<HashMap<String, Vec<IndexedFileRecord>>>,
}

impl FileTreeIndexer {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for FileTreeIndexer {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTreeIndexer {

    /// Indexes an entire mount directory recursively into in-memory search index
    pub fn index_mount(&self, mount_id: &str, root_path: &Path) -> std::io::Result<usize> {
        let mut records = Vec::new();
        self.traverse_dir(mount_id, root_path, Path::new(""), &mut records)?;
        let count = records.len();

        let mut lock = self.cache.write().unwrap();
        lock.insert(mount_id.to_string(), records);
        info!("Indexed {} items for mount '{}'", count, mount_id);

        Ok(count)
    }

    fn traverse_dir(
        &self,
        mount_id: &str,
        root: &Path,
        sub: &Path,
        out: &mut Vec<IndexedFileRecord>,
    ) -> std::io::Result<()> {
        let current = root.join(sub);
        if !current.exists() || !current.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(current)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }

            let rel = sub.join(&name);
            let meta = entry.metadata()?;

            if meta.is_dir() {
                self.traverse_dir(mount_id, root, &rel, out)?;
            } else {
                let mime = mime_guess::from_path(entry.path())
                    .first_or_octet_stream()
                    .to_string();

                out.push(IndexedFileRecord {
                    mount_id: mount_id.to_string(),
                    relative_path: rel.to_string_lossy().replace('\\', "/"),
                    name,
                    size: meta.len(),
                    mime_type: mime,
                });
            }
        }
        Ok(())
    }

    /// Fast search query across all cached mount indices or scoped to a specific mount
    pub fn search(&self, query: &str) -> Vec<IndexedFileRecord> {
        self.search_scoped(query, None)
    }

    /// Fast search query with optional mount scope
    pub fn search_scoped(&self, query: &str, scope_mount: Option<&str>) -> Vec<IndexedFileRecord> {
        let lock = self.cache.read().unwrap();
        let q = query.to_lowercase();
        let mut results = Vec::new();

        for (mount, records) in lock.iter() {
            if let Some(target_mount) = scope_mount {
                if mount != target_mount {
                    continue;
                }
            }
            for item in records {
                if item.name.to_lowercase().contains(&q) || item.relative_path.to_lowercase().contains(&q) {
                    results.push(item.clone());
                }
            }
        }

        results
    }

    /// Incremental real-time watcher update: registers or updates an individual file in the index
    pub fn add_or_update_file(
        &self,
        mount_id: &str,
        relative_path: &str,
        name: &str,
        size: u64,
        mime_type: &str,
    ) {
        let norm_path = relative_path.replace('\\', "/");
        let mut lock = self.cache.write().unwrap();
        let list = lock.entry(mount_id.to_string()).or_default();

        if let Some(pos) = list.iter().position(|r| r.relative_path == norm_path) {
            list[pos] = IndexedFileRecord {
                mount_id: mount_id.to_string(),
                relative_path: norm_path,
                name: name.to_string(),
                size,
                mime_type: mime_type.to_string(),
            };
        } else {
            list.push(IndexedFileRecord {
                mount_id: mount_id.to_string(),
                relative_path: norm_path,
                name: name.to_string(),
                size,
                mime_type: mime_type.to_string(),
            });
        }
    }

    /// Incremental real-time watcher update: removes a deleted file from the index
    pub fn remove_file(&self, mount_id: &str, relative_path: &str) -> bool {
        let norm_path = relative_path.replace('\\', "/");
        let mut lock = self.cache.write().unwrap();
        if let Some(list) = lock.get_mut(mount_id) {
            let before = list.len();
            list.retain(|r| r.relative_path != norm_path);
            list.len() < before
        } else {
            false
        }
    }

    /// Computes summary statistics for a mount index
    pub fn get_mount_stats(&self, mount_id: &str) -> Option<MountIndexStats> {
        let lock = self.cache.read().unwrap();
        lock.get(mount_id).map(|records| {
            let total_files = records.len();
            let total_bytes: u64 = records.iter().map(|r| r.size).sum();
            MountIndexStats {
                total_files,
                total_bytes,
            }
        })
    }
}
