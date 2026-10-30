use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use tracing::{error, warn};

#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("Mount '{0}' not found in registry")]
    MountNotFound(String),
    #[error("Path traversal detected: path escapes mount root")]
    PathTraversalAttempt,
    #[error("Target does not exist: {0}")]
    NotFound(String),
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Mount '{0}' is read-only")]
    ReadOnlyMount(String),
    #[error("OS-critical system path is quarantined: {0}")]
    QuarantinedPath(String),
}

impl axum::response::IntoResponse for MountError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::Json;
        let (status, msg) = match self {
            MountError::MountNotFound(m) => (StatusCode::NOT_FOUND, format!("Mount '{}' not registered", m)),
            MountError::PathTraversalAttempt => (StatusCode::FORBIDDEN, "Security violation: Path traversal forbidden".to_string()),
            MountError::NotFound(p) => (StatusCode::NOT_FOUND, format!("Resource not found: {}", p)),
            MountError::ReadOnlyMount(m) => (StatusCode::METHOD_NOT_ALLOWED, format!("Mount '{}' is read-only", m)),
            MountError::QuarantinedPath(p) => (StatusCode::FORBIDDEN, format!("Security violation: OS-critical path quarantined ({})", p)),
            MountError::IoError(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Filesystem I/O error: {}", e)),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_utc: Option<String>,
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MountPermission {
    Read,
    Write,
    StreamOnly,
}

pub const SQLITE_WAL_SCHEMA: &str = r#"PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS virtual_mounts (
    mount_id TEXT PRIMARY KEY NOT NULL,
    physical_root TEXT NOT NULL,
    permission TEXT NOT NULL CHECK(permission IN ('READ', 'WRITE', 'STREAM_ONLY')),
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'utc'))
);

CREATE INDEX IF NOT EXISTS idx_mount_permission ON virtual_mounts(permission);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDrive {
    pub path: PathBuf,
    pub name: String,
    pub drive_type: String,
    pub is_system: bool,
    pub is_removable: bool,
}

#[derive(Debug, Clone)]
pub struct MountEntry {
    pub base_path: PathBuf,
    pub permission: MountPermission,
    pub description: Option<String>,
}

#[derive(Debug)]
pub struct MountRegistry {
    mounts: std::sync::RwLock<HashMap<String, MountEntry>>,
}

impl Clone for MountRegistry {
    fn clone(&self) -> Self {
        let guard = self.mounts.read().unwrap();
        Self {
            mounts: std::sync::RwLock::new(guard.clone()),
        }
    }
}

impl MountRegistry {
    pub fn new() -> Self {
        Self {
            mounts: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Discovers system drive roots across Windows and POSIX systems with detailed metadata
    pub fn discover_detailed_drives() -> Vec<DiscoveredDrive> {
        let inspected = super::VolumeInspector::inspect_volumes();
        if !inspected.is_empty() {
            return inspected
                .into_iter()
                .map(|v| DiscoveredDrive {
                    path: v.mount_point,
                    name: v.name,
                    drive_type: format!("{:?}", v.media_type).to_lowercase(),
                    is_system: v.is_system,
                    is_removable: v.is_removable,
                })
                .collect();
        }

        let mut list = Vec::new();

        #[cfg(windows)]
        {
            for letter in b'A'..=b'Z' {
                let drive_str = format!("{}:\\", letter as char);
                let p = PathBuf::from(&drive_str);
                if p.exists() {
                    let is_system = (letter as char) == 'C' || (letter as char) == 'c';
                    let (drive_type, is_removable) = if is_system {
                        ("system".to_string(), false)
                    } else if (letter as char) == 'D' || (letter as char) == 'E' || (letter as char) == 'F' {
                        ("secondary_partition".to_string(), false)
                    } else {
                        ("external_or_usb".to_string(), true)
                    };

                    list.push(DiscoveredDrive {
                        path: p,
                        name: format!("Local Disk ({}:)", letter as char),
                        drive_type,
                        is_system,
                        is_removable,
                    });
                }
            }
        }

        #[cfg(not(windows))]
        {
            list.push(DiscoveredDrive {
                path: PathBuf::from("/"),
                name: "Root System Partition (/)".to_string(),
                drive_type: "system".to_string(),
                is_system: true,
                is_removable: false,
            });

            let candidates = [
                ("/media", "removable_usb", true),
                ("/mnt", "network_or_secondary_mount", false),
                ("/Volumes", "macos_volume", true),
            ];
            for (cand, dtype, removable) in candidates {
                let p = Path::new(cand);
                if p.exists() && p.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(p) {
                        for e in entries.flatten() {
                            let ep = e.path();
                            let name = ep.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| cand.to_string());
                            list.push(DiscoveredDrive {
                                path: ep,
                                name,
                                drive_type: dtype.to_string(),
                                is_system: false,
                                is_removable: removable,
                            });
                        }
                    }
                }
            }
        }

        list
    }

    /// Discovers system drive root paths across Windows and POSIX systems
    pub fn discover_system_drives() -> Vec<PathBuf> {
        Self::discover_detailed_drives().into_iter().map(|d| d.path).collect()
    }

    pub fn register(&self, name: &str, path: PathBuf, read_only: bool, description: Option<String>) -> Result<(), MountError> {
        let permission = if read_only {
            MountPermission::Read
        } else {
            MountPermission::Write
        };
        self.register_with_permission(name, path, permission, description)
    }

    pub fn register_with_permission(
        &self,
        name: &str,
        path: PathBuf,
        permission: MountPermission,
        description: Option<String>,
    ) -> Result<(), MountError> {
        let canonical = dunce::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if super::PathQuarantine::is_critical_system_path(&canonical) {
            error!(
                "Cannot register mount '{}': target {:?} is an OS-critical path and has been quarantined.",
                name, canonical
            );
            return Err(MountError::QuarantinedPath(canonical.to_string_lossy().to_string()));
        }

        if !path.exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        let mut guard = self.mounts.write().unwrap();
        guard.insert(
            name.to_string(),
            MountEntry {
                base_path: canonical,
                permission,
                description,
            },
        );
        Ok(())
    }

    /// Dynamically unregisters a virtual mount by its mount identifier
    pub fn unregister(&self, name: &str) -> Option<MountEntry> {
        let mut guard = self.mounts.write().unwrap();
        guard.remove(name)
    }

    /// Dynamically unregisters any virtual mount matching or residing within the given physical path
    pub fn unregister_by_path(&self, path: &Path) -> Option<String> {
        let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut guard = self.mounts.write().unwrap();
        let to_remove: Option<String> = guard
            .iter()
            .find(|(_, entry)| entry.base_path == canonical || entry.base_path.starts_with(&canonical))
            .map(|(k, _)| k.clone());

        if let Some(ref key) = to_remove {
            guard.remove(key);
        }
        to_remove
    }

    pub fn list_mounts(&self) -> Vec<serde_json::Value> {
        let guard = self.mounts.read().unwrap();
        guard
            .iter()
            .map(|(k, v)| {
                serde_json::json!({
                    "id": k,
                    "permission": format!("{:?}", v.permission),
                    "read_only": v.permission == MountPermission::Read || v.permission == MountPermission::StreamOnly,
                    "stream_only": v.permission == MountPermission::StreamOnly,
                    "description": v.description,
                    "path_display": v.base_path.to_string_lossy(),
                })
            })
            .collect()
    }

    /// Verifies that the targeted virtual mount has WRITE permissions
    pub fn check_write_permission(&self, mount_id: &str) -> Result<(), MountError> {
        let guard = self.mounts.read().unwrap();
        let entry = guard
            .get(mount_id)
            .ok_or_else(|| MountError::MountNotFound(mount_id.to_string()))?;
        if entry.permission != MountPermission::Write {
            return Err(MountError::ReadOnlyMount(mount_id.to_string()));
        }
        Ok(())
    }

    /// Verifies that the targeted virtual mount allows media streaming
    pub fn check_stream_permission(&self, mount_id: &str) -> Result<(), MountError> {
        let guard = self.mounts.read().unwrap();
        let _ = guard
            .get(mount_id)
            .ok_or_else(|| MountError::MountNotFound(mount_id.to_string()))?;
        Ok(())
    }

    /// Exports SQLite DDL table definition and INSERT statements reflecting registered mounts
    pub fn export_sqlite_ddl_and_inserts(&self) -> String {
        let mut sql = String::from(SQLITE_WAL_SCHEMA);
        sql.push('\n');
        let guard = self.mounts.read().unwrap();
        for (id, entry) in guard.iter() {
            let perm_str = match entry.permission {
                MountPermission::Read => "READ",
                MountPermission::Write => "WRITE",
                MountPermission::StreamOnly => "STREAM_ONLY",
            };
            let desc = entry.description.as_deref().unwrap_or("");
            sql.push_str(&format!(
                "INSERT OR REPLACE INTO virtual_mounts (mount_id, physical_root, permission, description) VALUES ('{}', '{}', '{}', '{}');\n",
                id.replace('\'', "''"),
                entry.base_path.to_string_lossy().replace('\'', "''"),
                perm_str,
                desc.replace('\'', "''")
            ));
        }
        sql
    }

    /// Resolves a relative path within a virtual mount and ensures no traversal escapes the mount root.
    pub fn resolve_safe_path(&self, mount_id: &str, relative_path: &Path) -> Result<PathBuf, MountError> {
        let entry = {
            let guard = self.mounts.read().unwrap();
            guard
                .get(mount_id)
                .cloned()
                .ok_or_else(|| MountError::MountNotFound(mount_id.to_string()))?
        };

        // 1. String-level injection and encoded sequence detection
        let path_str = relative_path.to_string_lossy();
        if path_str.contains('\0') {
            warn!("Null byte injection detected in path: {:?}", relative_path);
            return Err(MountError::PathTraversalAttempt);
        }

        let lower = path_str.to_lowercase();
        if lower.contains("%2e%2e")
            || lower.contains("..%2f")
            || lower.contains("..%5c")
            || lower.contains("%2f..")
            || lower.contains("%5c..")
        {
            warn!("Encoded traversal sequence detected in path: {:?}", relative_path);
            return Err(MountError::PathTraversalAttempt);
        }

        // 2. Lexical component checks for malicious parent navigation or root-prefixing
        for comp in relative_path.components() {
            match comp {
                Component::ParentDir => {
                    warn!("Parent directory traversal sequence detected in requested path: {:?}", relative_path);
                    return Err(MountError::PathTraversalAttempt);
                }
                Component::RootDir | Component::Prefix(_) => {
                    warn!("Absolute or prefix path sequence detected in requested relative path: {:?}", relative_path);
                    return Err(MountError::PathTraversalAttempt);
                }
                Component::Normal(os_str) => {
                    let name = os_str.to_string_lossy();
                    if super::PathQuarantine::is_quarantined_name(&name) {
                        warn!("Quarantine intercept: Blocked access to quarantined component '{}'", name);
                        return Err(MountError::QuarantinedPath(name.to_string()));
                    }
                }
                _ => {}
            }
        }

        // 3. Synthesize tentative target path
        let candidate = entry.base_path.join(relative_path);

        // 3. If file exists, canonicalize and verify boundary confinement
        if candidate.exists() {
            let canonical_target = dunce::canonicalize(&candidate)
                .map_err(|e| MountError::IoError(e))?;

            if !canonical_target.starts_with(&entry.base_path) {
                error!(
                    "Path traversal exploit averted! Canonical target {:?} escapes mount base {:?}",
                    canonical_target, entry.base_path
                );
                return Err(MountError::PathTraversalAttempt);
            }

            if super::PathQuarantine::is_critical_system_path(&canonical_target) {
                warn!(
                    "Quarantine boundary enforced: Blocked access to OS-critical path {:?} within mount '{}'",
                    canonical_target, mount_id
                );
                return Err(MountError::QuarantinedPath(canonical_target.to_string_lossy().to_string()));
            }

            Ok(canonical_target)
        } else {
            // For non-existent targets (e.g. prospective upload path), verify normalized parent
            let normalized = candidate;
            let parent_canonical = match normalized.parent() {
                Some(p) if p.exists() => dunce::canonicalize(p)?,
                _ => entry.base_path.clone(),
            };

            if !parent_canonical.starts_with(&entry.base_path) {
                return Err(MountError::PathTraversalAttempt);
            }

            if super::PathQuarantine::is_critical_system_path(&normalized) {
                warn!(
                    "Quarantine boundary enforced: Blocked prospective OS-critical target {:?}",
                    normalized
                );
                return Err(MountError::QuarantinedPath(normalized.to_string_lossy().to_string()));
            }

            Ok(normalized)
        }
    }

    /// Lists directory contents securely with rich metadata
    pub fn list_directory(&self, mount_id: &str, subpath: &Path) -> Result<Vec<FileMetadata>, MountError> {
        let safe_dir = self.resolve_safe_path(mount_id, subpath)?;
        if !safe_dir.is_dir() {
            return Err(MountError::NotFound(safe_dir.to_string_lossy().to_string()));
        }

        let mut results = Vec::new();
        let read_dir = std::fs::read_dir(&safe_dir)?;

        for entry in read_dir.flatten() {
            let meta = entry.metadata()?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden or governance/git files
            if name.starts_with('.') {
                continue;
            }

            // Exclude or quarantine OS-critical paths from raw unmanaged listing
            if !super::PathQuarantine::can_expose_in_unmanaged_listing(&safe_dir, &name, &entry.path()) {
                continue;
            }

            let mime = if meta.is_dir() {
                "inode/directory".to_string()
            } else {
                mime_guess::from_path(entry.path())
                    .first_or_octet_stream()
                    .to_string()
            };

            let modified_utc = meta.modified().ok().map(|st| {
                let dur = st.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                format!("{}s", dur.as_secs())
            });

            results.push(FileMetadata {
                name,
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified_utc,
                mime_type: mime,
            });
        }

        // Sort directories first, then alphabetically
        results.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(results)
    }
}
