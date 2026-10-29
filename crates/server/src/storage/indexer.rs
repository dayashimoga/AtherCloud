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

pub struct FileTreeIndexer {
    cache: RwLock<HashMap<String, Vec<IndexedFileRecord>>>,
}

impl FileTreeIndexer {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

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

    /// Fast search query across all cached mount indices
    pub fn search(&self, query: &str) -> Vec<IndexedFileRecord> {
        let lock = self.cache.read().unwrap();
        let q = query.to_lowercase();
        let mut results = Vec::new();

        for records in lock.values() {
            for item in records {
                if item.name.to_lowercase().contains(&q) || item.relative_path.to_lowercase().contains(&q) {
                    results.push(item.clone());
                }
            }
        }

        results
    }
}
