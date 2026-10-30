use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum MountManagerError {
    #[error("SQLite database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Invalid permission mode '{0}'")]
    InvalidPermission(String),
    #[error("Mount '{0}' not found")]
    NotFound(String),
}

#[derive(Debug, Clone)]
pub struct CachedMountRoute {
    pub mount_id: String,
    pub mount_name: String,
    pub drive_serial: Option<String>,
    pub physical_root: PathBuf,
    pub permission_mode: PermissionMode,
    pub allowed_subpaths: Vec<PathBuf>,
    pub password_hash: Option<String>,
    pub expiration_ts: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRouteAuthorization {
    pub mount_id: String,
    pub mount_name: String,
    pub canonical_target: PathBuf,
    pub permission_mode: PermissionMode,
    pub is_stream_allowed: bool,
    pub is_read_allowed: bool,
    pub is_write_allowed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteAuthError {
    #[error("Mount '{0}' not found in cached registry")]
    MountNotFound(String),
    #[error("Mount access expired at {0}")]
    Expired(i64),
    #[error("Subpath '{0}' forbidden by whitelist policy")]
    SubpathForbidden(String),
    #[error("Path traversal detected: {0}")]
    PathTraversal(String),
    #[error("Write operation not allowed for permission mode {0:?}")]
    WriteDenied(PermissionMode),
}

impl axum::response::IntoResponse for RouteAuthError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::Json;
        let (status, msg) = match &self {
            RouteAuthError::MountNotFound(m) => (StatusCode::NOT_FOUND, format!("Mount '{}' not registered", m)),
            RouteAuthError::Expired(ts) => (StatusCode::FORBIDDEN, format!("Virtual mount access expired at timestamp {}", ts)),
            RouteAuthError::SubpathForbidden(p) => (StatusCode::FORBIDDEN, format!("Subpath '{}' is forbidden by access policy", p)),
            RouteAuthError::PathTraversal(p) => (StatusCode::FORBIDDEN, format!("Security violation: Path traversal forbidden ({})", p)),
            RouteAuthError::WriteDenied(mode) => (StatusCode::METHOD_NOT_ALLOWED, format!("Write denied on mount with mode {:?}", mode)),
        };
        (status, Json(serde_json::json!({
            "type": "https://aetherstream.dev/errors/authorization",
            "title": "Route Authorization Violation",
            "status": status.as_u16(),
            "detail": msg
        }))).into_response()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    StreamOnly,
    ReadOnly,
    ReadWrite,
}

impl PermissionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionMode::StreamOnly => "stream_only",
            PermissionMode::ReadOnly => "read_only",
            PermissionMode::ReadWrite => "read_write",
        }
    }

    pub fn from_str_strict(s: &str) -> Result<Self, MountManagerError> {
        match s.to_lowercase().as_str() {
            "stream_only" => Ok(PermissionMode::StreamOnly),
            "read_only" => Ok(PermissionMode::ReadOnly),
            "read_write" => Ok(PermissionMode::ReadWrite),
            other => Err(MountManagerError::InvalidPermission(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualMountRecord {
    pub id: String,
    pub name: String,
    pub drive_serial: Option<String>,
    pub physical_path: String,
    pub permission_mode: PermissionMode,
    pub allowed_subpaths: Vec<String>,
    pub password_hash: Option<String>,
    pub expiration_ts: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareRuleRecord {
    pub id: String,
    pub name: String,
    pub drive_serial: Option<String>,
    pub physical_path: String,
    pub permission_mode: PermissionMode,
    pub allowed_subpaths: Vec<String>,
    pub password_hash: Option<String>,
    pub expiration_ts: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalCheckpointMode {
    Passive,
    Full,
    Restart,
    Truncate,
}

impl WalCheckpointMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            WalCheckpointMode::Passive => "PASSIVE",
            WalCheckpointMode::Full => "FULL",
            WalCheckpointMode::Restart => "RESTART",
            WalCheckpointMode::Truncate => "TRUNCATE",
        }
    }
}

#[derive(Clone)]
pub struct MountManager {
    conn: Arc<Mutex<Connection>>,
    route_cache: Arc<RwLock<HashMap<String, CachedMountRoute>>>,
}

impl MountManager {
    /// Opens or creates an SQLite-backed MountManager with WAL mode and runs migrations
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, MountManagerError> {
        let conn = Connection::open(db_path)?;
        Self::init_with_connection(conn)
    }

    /// Creates an in-memory SQLite MountManager for testing and ephemeral environments
    pub fn in_memory() -> Result<Self, MountManagerError> {
        let conn = Connection::open_in_memory()?;
        Self::init_with_connection(conn)
    }

    fn init_with_connection(conn: Connection) -> Result<Self, MountManagerError> {
        // Enforce WAL mode, normal synchronous for high-throughput ACID reliability, foreign keys,
        // memory-mapped I/O (256 MiB mmap_size), in-memory temp store, and 64 MiB page cache.
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA mmap_size = 268435456;
            PRAGMA temp_store = MEMORY;
            PRAGMA cache_size = -64000;
            "#,
        )?;

        let manager = Self {
            conn: Arc::new(Mutex::new(conn)),
            route_cache: Arc::new(RwLock::new(HashMap::new())),
        };

        manager.run_migrations()?;
        manager.warm_cache_sync()?;
        Ok(manager)
    }

    /// Synchronously warms the in-memory route cache from SQLite
    pub fn warm_cache_sync(&self) -> Result<(), MountManagerError> {
        let mounts = self.list_mounts()?;
        let mut map = HashMap::new();
        for m in mounts {
            map.insert(
                m.name.clone(),
                CachedMountRoute {
                    mount_id: m.id,
                    mount_name: m.name,
                    drive_serial: m.drive_serial,
                    physical_root: PathBuf::from(m.physical_path),
                    permission_mode: m.permission_mode,
                    allowed_subpaths: m.allowed_subpaths.into_iter().map(PathBuf::from).collect(),
                    password_hash: m.password_hash,
                    expiration_ts: m.expiration_ts,
                },
            );
        }
        if let Ok(mut cache) = self.route_cache.try_write() {
            *cache = map;
        }
        Ok(())
    }

    /// Asynchronously flushes and refreshes the in-memory route cache from SQLite
    pub async fn refresh_cache(&self) -> Result<(), MountManagerError> {
        let mounts = self.list_mounts()?;
        let mut map = HashMap::new();
        for m in mounts {
            map.insert(
                m.name.clone(),
                CachedMountRoute {
                    mount_id: m.id,
                    mount_name: m.name,
                    drive_serial: m.drive_serial,
                    physical_root: PathBuf::from(m.physical_path),
                    permission_mode: m.permission_mode,
                    allowed_subpaths: m.allowed_subpaths.into_iter().map(PathBuf::from).collect(),
                    password_hash: m.password_hash,
                    expiration_ts: m.expiration_ts,
                },
            );
        }
        let mut cache = self.route_cache.write().await;
        *cache = map;
        Ok(())
    }

    /// Resolves and authorizes a requested subpath against the in-memory cache in sub-microsecond time (<1 µs)
    pub async fn resolve_cached_route(
        &self,
        mount_name: &str,
        requested_subpath: &Path,
    ) -> Result<CachedRouteAuthorization, RouteAuthError> {
        let cache = self.route_cache.read().await;
        let route = cache
            .get(mount_name)
            .ok_or_else(|| RouteAuthError::MountNotFound(mount_name.to_string()))?;

        // 1. Validate expiration timestamp
        if let Some(exp) = route.expiration_ts {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if now > exp {
                return Err(RouteAuthError::Expired(exp));
            }
        }

        // 2. Traversal defenses and subpath whitelist verification via PathSandbox
        if let Err(e) = crate::security::PathSandbox::validate_subpath_whitelist(
            &route.allowed_subpaths,
            requested_subpath,
        ) {
            return match e {
                crate::security::SandboxError::SubpathForbidden(p) => {
                    Err(RouteAuthError::SubpathForbidden(p.to_string_lossy().into_owned()))
                }
                crate::security::SandboxError::NullByteInjection(s)
                | crate::security::SandboxError::DirectoryTraversal(s)
                | crate::security::SandboxError::UncDeviceBypass(s)
                | crate::security::SandboxError::ReservedDeviceName(s) => {
                    Err(RouteAuthError::PathTraversal(s))
                }
                _ => Err(RouteAuthError::SubpathForbidden(requested_subpath.to_string_lossy().into_owned())),
            };
        }

        let canonical_target = route.physical_root.join(requested_subpath);
        let is_stream_allowed = true;
        let is_read_allowed = matches!(
            route.permission_mode,
            PermissionMode::ReadOnly | PermissionMode::ReadWrite
        );
        let is_write_allowed = matches!(route.permission_mode, PermissionMode::ReadWrite);

        Ok(CachedRouteAuthorization {
            mount_id: route.mount_id.clone(),
            mount_name: route.mount_name.clone(),
            canonical_target,
            permission_mode: route.permission_mode,
            is_stream_allowed,
            is_read_allowed,
            is_write_allowed,
        })
    }

    /// Returns the number of mounts currently in the memory-cached registry
    pub async fn cached_mount_count(&self) -> usize {
        self.route_cache.read().await.len()
    }

    /// Retrieves a cloned cached route definition for inspection
    pub async fn get_cached_route(&self, mount_name: &str) -> Option<CachedMountRoute> {
        self.route_cache.read().await.get(mount_name).cloned()
    }

    /// Executes SQLite schema migrations idempotently
    pub fn run_migrations(&self) -> Result<(), MountManagerError> {
        let conn = self.conn.lock().unwrap();

        // Migration tracking table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now', 'utc'))
            );
            "#,
            [],
        )?;

        // Check if version 1 is applied
        let v1_applied: bool = conn
            .query_row(
                "SELECT COUNT(1) FROM schema_migrations WHERE version = 1;",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|cnt| cnt > 0)
            .unwrap_or(false);

        if !v1_applied {
            info!("Executing SQLite Migration 001: mounts and share_rules tables...");
            let tx = conn.unchecked_transaction()?;

            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS mounts (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL UNIQUE,
                    drive_serial TEXT,
                    physical_path TEXT NOT NULL,
                    permission_mode TEXT NOT NULL CHECK(permission_mode IN ('stream_only', 'read_only', 'read_write')),
                    allowed_subpaths TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    expiration_ts INTEGER,
                    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'utc'))
                );

                CREATE INDEX IF NOT EXISTS idx_mounts_name ON mounts(name);
                CREATE INDEX IF NOT EXISTS idx_mounts_drive_serial ON mounts(drive_serial);
                CREATE INDEX IF NOT EXISTS idx_mounts_permission ON mounts(permission_mode);

                CREATE TABLE IF NOT EXISTS share_rules (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    drive_serial TEXT,
                    physical_path TEXT NOT NULL,
                    permission_mode TEXT NOT NULL CHECK(permission_mode IN ('stream_only', 'read_only', 'read_write')),
                    allowed_subpaths TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    expiration_ts INTEGER,
                    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'utc'))
                );

                CREATE INDEX IF NOT EXISTS idx_share_rules_name ON share_rules(name);
                CREATE INDEX IF NOT EXISTS idx_share_rules_expiration ON share_rules(expiration_ts);

                INSERT INTO schema_migrations (version, name) VALUES (1, '001_init_mounts_and_share_rules');
                "#,
            )?;

            tx.commit()?;
            info!("SQLite Migration 001 committed successfully.");
        }

        Ok(())
    }

    /// Inserts or replaces a virtual mount record
    pub fn upsert_mount(&self, mount: &VirtualMountRecord) -> Result<(), MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let allowed_json = serde_json::to_string(&mount.allowed_subpaths)?;

        conn.execute(
            r#"
            INSERT INTO mounts (
                id, name, drive_serial, physical_path, permission_mode,
                allowed_subpaths, password_hash, expiration_ts, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now', 'utc'))
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                drive_serial = excluded.drive_serial,
                physical_path = excluded.physical_path,
                permission_mode = excluded.permission_mode,
                allowed_subpaths = excluded.allowed_subpaths,
                password_hash = excluded.password_hash,
                expiration_ts = excluded.expiration_ts,
                updated_at = datetime('now', 'utc');
            "#,
            params![
                mount.id,
                mount.name,
                mount.drive_serial,
                mount.physical_path,
                mount.permission_mode.as_str(),
                allowed_json,
                mount.password_hash,
                mount.expiration_ts,
                mount.created_at,
            ],
        )?;

        // Update in-memory route cache
        let cached = CachedMountRoute {
            mount_id: mount.id.clone(),
            mount_name: mount.name.clone(),
            drive_serial: mount.drive_serial.clone(),
            physical_root: PathBuf::from(&mount.physical_path),
            permission_mode: mount.permission_mode,
            allowed_subpaths: mount.allowed_subpaths.iter().map(PathBuf::from).collect(),
            password_hash: mount.password_hash.clone(),
            expiration_ts: mount.expiration_ts,
        };
        if let Ok(mut cache) = self.route_cache.try_write() {
            cache.insert(mount.name.clone(), cached);
        }

        Ok(())
    }

    /// Retrieves a virtual mount by its unique ID
    pub fn get_mount(&self, id: &str) -> Result<Option<VirtualMountRecord>, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, drive_serial, physical_path, permission_mode,
                   allowed_subpaths, password_hash, expiration_ts, created_at
            FROM mounts WHERE id = ?1;
            "#,
        )?;

        let mount = stmt
            .query_row(params![id], |row| {
                let perm_str: String = row.get(4)?;
                let allowed_str: String = row.get(5)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    perm_str,
                    allowed_str,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .optional()?;

        match mount {
            Some((id, name, serial, path, perm_str, allowed_str, pass, exp, created)) => {
                let permission_mode = PermissionMode::from_str_strict(&perm_str)?;
                let allowed_subpaths: Vec<String> =
                    serde_json::from_str(&allowed_str).unwrap_or_default();
                Ok(Some(VirtualMountRecord {
                    id,
                    name,
                    drive_serial: serial,
                    physical_path: path,
                    permission_mode,
                    allowed_subpaths,
                    password_hash: pass,
                    expiration_ts: exp,
                    created_at: created,
                }))
            }
            None => Ok(None),
        }
    }

    /// Retrieves a virtual mount by its unique human-readable name (e.g. "movies", "photos")
    pub fn get_mount_by_name(&self, name: &str) -> Result<Option<VirtualMountRecord>, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, drive_serial, physical_path, permission_mode,
                   allowed_subpaths, password_hash, expiration_ts, created_at
            FROM mounts WHERE name = ?1;
            "#,
        )?;

        let mount = stmt
            .query_row(params![name], |row| {
                let perm_str: String = row.get(4)?;
                let allowed_str: String = row.get(5)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    perm_str,
                    allowed_str,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .optional()?;

        match mount {
            Some((id, name, serial, path, perm_str, allowed_str, pass, exp, created)) => {
                let permission_mode = PermissionMode::from_str_strict(&perm_str)?;
                let allowed_subpaths: Vec<String> =
                    serde_json::from_str(&allowed_str).unwrap_or_default();
                Ok(Some(VirtualMountRecord {
                    id,
                    name,
                    drive_serial: serial,
                    physical_path: path,
                    permission_mode,
                    allowed_subpaths,
                    password_hash: pass,
                    expiration_ts: exp,
                    created_at: created,
                }))
            }
            None => Ok(None),
        }
    }

    /// Lists all active virtual mount records
    pub fn list_mounts(&self) -> Result<Vec<VirtualMountRecord>, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, drive_serial, physical_path, permission_mode,
                   allowed_subpaths, password_hash, expiration_ts, created_at
            FROM mounts ORDER BY name ASC;
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let perm_str: String = row.get(4)?;
            let allowed_str: String = row.get(5)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                perm_str,
                allowed_str,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })?;

        let mut list = Vec::new();
        for r in rows {
            let (id, name, serial, path, perm_str, allowed_str, pass, exp, created) = r?;
            let permission_mode = PermissionMode::from_str_strict(&perm_str)?;
            let allowed_subpaths: Vec<String> =
                serde_json::from_str(&allowed_str).unwrap_or_default();
            list.push(VirtualMountRecord {
                id,
                name,
                drive_serial: serial,
                physical_path: path,
                permission_mode,
                allowed_subpaths,
                password_hash: pass,
                expiration_ts: exp,
                created_at: created,
            });
        }

        Ok(list)
    }

    /// Deletes a virtual mount by ID
    pub fn delete_mount(&self, id: &str) -> Result<bool, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM mounts WHERE id = ?1;", params![id])?;
        if deleted > 0 {
            if let Ok(mut cache) = self.route_cache.try_write() {
                cache.retain(|_, v| v.mount_id != id);
            }
        }
        Ok(deleted > 0)
    }

    /// Inserts or replaces a share rule record
    pub fn upsert_share_rule(&self, rule: &ShareRuleRecord) -> Result<(), MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let allowed_json = serde_json::to_string(&rule.allowed_subpaths)?;

        conn.execute(
            r#"
            INSERT INTO share_rules (
                id, name, drive_serial, physical_path, permission_mode,
                allowed_subpaths, password_hash, expiration_ts, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now', 'utc'))
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                drive_serial = excluded.drive_serial,
                physical_path = excluded.physical_path,
                permission_mode = excluded.permission_mode,
                allowed_subpaths = excluded.allowed_subpaths,
                password_hash = excluded.password_hash,
                expiration_ts = excluded.expiration_ts,
                updated_at = datetime('now', 'utc');
            "#,
            params![
                rule.id,
                rule.name,
                rule.drive_serial,
                rule.physical_path,
                rule.permission_mode.as_str(),
                allowed_json,
                rule.password_hash,
                rule.expiration_ts,
                rule.created_at,
            ],
        )?;

        Ok(())
    }

    /// Retrieves a share rule by ID
    pub fn get_share_rule(&self, id: &str) -> Result<Option<ShareRuleRecord>, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, drive_serial, physical_path, permission_mode,
                   allowed_subpaths, password_hash, expiration_ts, created_at
            FROM share_rules WHERE id = ?1;
            "#,
        )?;

        let rule = stmt
            .query_row(params![id], |row| {
                let perm_str: String = row.get(4)?;
                let allowed_str: String = row.get(5)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    perm_str,
                    allowed_str,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .optional()?;

        match rule {
            Some((id, name, serial, path, perm_str, allowed_str, pass, exp, created)) => {
                let permission_mode = PermissionMode::from_str_strict(&perm_str)?;
                let allowed_subpaths: Vec<String> =
                    serde_json::from_str(&allowed_str).unwrap_or_default();
                Ok(Some(ShareRuleRecord {
                    id,
                    name,
                    drive_serial: serial,
                    physical_path: path,
                    permission_mode,
                    allowed_subpaths,
                    password_hash: pass,
                    expiration_ts: exp,
                    created_at: created,
                }))
            }
            None => Ok(None),
        }
    }

    /// Lists all active share rules
    pub fn list_share_rules(&self) -> Result<Vec<ShareRuleRecord>, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, drive_serial, physical_path, permission_mode,
                   allowed_subpaths, password_hash, expiration_ts, created_at
            FROM share_rules ORDER BY created_at DESC;
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let perm_str: String = row.get(4)?;
            let allowed_str: String = row.get(5)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                perm_str,
                allowed_str,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })?;

        let mut list = Vec::new();
        for r in rows {
            let (id, name, serial, path, perm_str, allowed_str, pass, exp, created) = r?;
            let permission_mode = PermissionMode::from_str_strict(&perm_str)?;
            let allowed_subpaths: Vec<String> =
                serde_json::from_str(&allowed_str).unwrap_or_default();
            list.push(ShareRuleRecord {
                id,
                name,
                drive_serial: serial,
                physical_path: path,
                permission_mode,
                allowed_subpaths,
                password_hash: pass,
                expiration_ts: exp,
                created_at: created,
            });
        }

        Ok(list)
    }

    /// Deletes a share rule by ID
    pub fn delete_share_rule(&self, id: &str) -> Result<bool, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM share_rules WHERE id = ?1;", params![id])?;
        Ok(deleted > 0)
    }

    /// Returns the currently active memory-mapped I/O byte size
    pub fn get_mmap_size(&self) -> Result<i64, MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let mmap_size: i64 = conn.query_row("PRAGMA mmap_size;", [], |row| row.get(0))?;
        Ok(mmap_size)
    }

    /// Performs an explicit WAL checkpoint in the specified mode (PASSIVE, FULL, RESTART, TRUNCATE),
    /// returning (busy_status, log_frames, checkpointed_frames).
    pub fn checkpoint_wal(&self, mode: WalCheckpointMode) -> Result<(i32, i32, i32), MountManagerError> {
        let conn = self.conn.lock().unwrap();
        let query = format!("PRAGMA wal_checkpoint({});", mode.as_str());
        let res = conn.query_row(&query, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(res)
    }

    /// Executes an atomic ACID transaction batch-upserting multiple virtual mount records.
    /// If any insert fails, the entire transaction is rolled back cleanly.
    pub fn transactional_batch_upsert(&self, mounts: &[VirtualMountRecord]) -> Result<(), MountManagerError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO mounts (
                    id, name, drive_serial, physical_path, permission_mode,
                    allowed_subpaths, password_hash, expiration_ts, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now', 'utc'))
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    drive_serial = excluded.drive_serial,
                    physical_path = excluded.physical_path,
                    permission_mode = excluded.permission_mode,
                    allowed_subpaths = excluded.allowed_subpaths,
                    password_hash = excluded.password_hash,
                    expiration_ts = excluded.expiration_ts,
                    updated_at = datetime('now', 'utc');
                "#,
            )?;

            for mount in mounts {
                let allowed_json = serde_json::to_string(&mount.allowed_subpaths)?;
                stmt.execute(params![
                    mount.id,
                    mount.name,
                    mount.drive_serial,
                    mount.physical_path,
                    mount.permission_mode.as_str(),
                    allowed_json,
                    mount.password_hash,
                    mount.expiration_ts,
                    mount.created_at,
                ])?;
            }
        }

        tx.commit()?;

        // Synchronously update in-memory route cache
        if let Ok(mut cache) = self.route_cache.try_write() {
            for mount in mounts {
                let cached = CachedMountRoute {
                    mount_id: mount.id.clone(),
                    mount_name: mount.name.clone(),
                    drive_serial: mount.drive_serial.clone(),
                    physical_root: PathBuf::from(&mount.physical_path),
                    permission_mode: mount.permission_mode,
                    allowed_subpaths: mount.allowed_subpaths.iter().map(PathBuf::from).collect(),
                    password_hash: mount.password_hash.clone(),
                    expiration_ts: mount.expiration_ts,
                };
                cache.insert(mount.name.clone(), cached);
            }
        }

        Ok(())
    }

    /// Atomically creates or updates a virtual mount along with a set of associated share rules
    /// inside a single ACID transaction. If any step fails, changes roll back completely.
    pub fn transactional_mount_with_share_rules(
        &self,
        mount: &VirtualMountRecord,
        rules: &[ShareRuleRecord],
    ) -> Result<(), MountManagerError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // 1. Upsert mount
        let allowed_json = serde_json::to_string(&mount.allowed_subpaths)?;
        tx.execute(
            r#"
            INSERT INTO mounts (
                id, name, drive_serial, physical_path, permission_mode,
                allowed_subpaths, password_hash, expiration_ts, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now', 'utc'))
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                drive_serial = excluded.drive_serial,
                physical_path = excluded.physical_path,
                permission_mode = excluded.permission_mode,
                allowed_subpaths = excluded.allowed_subpaths,
                password_hash = excluded.password_hash,
                expiration_ts = excluded.expiration_ts,
                updated_at = datetime('now', 'utc');
            "#,
            params![
                mount.id,
                mount.name,
                mount.drive_serial,
                mount.physical_path,
                mount.permission_mode.as_str(),
                allowed_json,
                mount.password_hash,
                mount.expiration_ts,
                mount.created_at,
            ],
        )?;

        // 2. Upsert associated share rules
        for rule in rules {
            let rule_allowed_json = serde_json::to_string(&rule.allowed_subpaths)?;
            tx.execute(
                r#"
                INSERT INTO share_rules (
                    id, name, drive_serial, physical_path, permission_mode,
                    allowed_subpaths, password_hash, expiration_ts, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now', 'utc'))
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    drive_serial = excluded.drive_serial,
                    physical_path = excluded.physical_path,
                    permission_mode = excluded.permission_mode,
                    allowed_subpaths = excluded.allowed_subpaths,
                    password_hash = excluded.password_hash,
                    expiration_ts = excluded.expiration_ts,
                    updated_at = datetime('now', 'utc');
                "#,
                params![
                    rule.id,
                    rule.name,
                    rule.drive_serial,
                    rule.physical_path,
                    rule.permission_mode.as_str(),
                    rule_allowed_json,
                    rule.password_hash,
                    rule.expiration_ts,
                    rule.created_at,
                ],
            )?;
        }

        tx.commit()?;

        // Synchronously update in-memory route cache
        if let Ok(mut cache) = self.route_cache.try_write() {
            let cached = CachedMountRoute {
                mount_id: mount.id.clone(),
                mount_name: mount.name.clone(),
                drive_serial: mount.drive_serial.clone(),
                physical_root: PathBuf::from(&mount.physical_path),
                permission_mode: mount.permission_mode,
                allowed_subpaths: mount.allowed_subpaths.iter().map(PathBuf::from).collect(),
                password_hash: mount.password_hash.clone(),
                expiration_ts: mount.expiration_ts,
            };
            cache.insert(mount.name.clone(), cached);
        }

        Ok(())
    }

    /// Atomically replaces an existing virtual mount with a new mount record
    pub fn atomic_replace_mount(&self, old_id: &str, new_mount: &VirtualMountRecord) -> Result<(), MountManagerError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        tx.execute("DELETE FROM mounts WHERE id = ?1;", params![old_id])?;

        let allowed_json = serde_json::to_string(&new_mount.allowed_subpaths)?;
        tx.execute(
            r#"
            INSERT INTO mounts (
                id, name, drive_serial, physical_path, permission_mode,
                allowed_subpaths, password_hash, expiration_ts, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now', 'utc'));
            "#,
            params![
                new_mount.id,
                new_mount.name,
                new_mount.drive_serial,
                new_mount.physical_path,
                new_mount.permission_mode.as_str(),
                allowed_json,
                new_mount.password_hash,
                new_mount.expiration_ts,
                new_mount.created_at,
            ],
        )?;

        tx.commit()?;

        // Synchronously update in-memory route cache
        if let Ok(mut cache) = self.route_cache.try_write() {
            cache.retain(|_, v| v.mount_id != old_id);
            let cached = CachedMountRoute {
                mount_id: new_mount.id.clone(),
                mount_name: new_mount.name.clone(),
                drive_serial: new_mount.drive_serial.clone(),
                physical_root: PathBuf::from(&new_mount.physical_path),
                permission_mode: new_mount.permission_mode,
                allowed_subpaths: new_mount.allowed_subpaths.iter().map(PathBuf::from).collect(),
                password_hash: new_mount.password_hash.clone(),
                expiration_ts: new_mount.expiration_ts,
            };
            cache.insert(new_mount.name.clone(), cached);
        }

        Ok(())
    }
}

