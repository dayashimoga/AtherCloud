use axum::{
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::security::ProblemDetails;
use crate::storage::mount_manager::{
    MountManager, PermissionMode, ShareRuleRecord, VirtualMountRecord,
};
use crate::storage::mounts::{MountPermission, MountRegistry};
use crate::storage::quarantine::PathQuarantine;
use crate::storage::volume_info::VolumeInspector;

#[derive(Clone)]
pub struct StorageApiState {
    pub mounts: Arc<MountRegistry>,
    pub mount_manager: Arc<MountManager>,
}

impl StorageApiState {
    pub fn new(mounts: Arc<MountRegistry>, mount_manager: Arc<MountManager>) -> Self {
        Self {
            mounts,
            mount_manager,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateMountRequest {
    pub id: String,
    pub name: String,
    pub physical_path: String,
    pub permission_mode: Option<String>,
    pub allowed_subpaths: Option<Vec<String>>,
    pub drive_serial: Option<String>,
    pub description: Option<String>,
    pub expiration_ts: Option<i64>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdateMountRequest {
    pub name: Option<String>,
    pub physical_path: Option<String>,
    pub permission_mode: Option<String>,
    pub allowed_subpaths: Option<Vec<String>>,
    pub description: Option<String>,
    pub expiration_ts: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateShareRuleRequest {
    pub id: String,
    pub name: String,
    pub physical_path: String,
    pub permission_mode: Option<String>,
    pub allowed_subpaths: Option<Vec<String>>,
    pub drive_serial: Option<String>,
    pub expiration_ts: Option<i64>,
    pub password: Option<String>,
}

fn current_utc_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}s", now)
}

/// Builds the Axum router for the Storage & Virtual Mounts Management REST API
pub fn storage_api_router(state: StorageApiState) -> Router {
    Router::new()
        .route("/api/v1/storage/drives", get(storage_drives_handler))
        .route(
            "/api/v1/storage/mounts",
            get(get_storage_mounts_handler).post(create_storage_mount_handler),
        )
        .route(
            "/api/v1/storage/mounts/:id",
            get(get_storage_mount_by_id_handler)
                .put(update_storage_mount_handler)
                .delete(delete_storage_mount_handler),
        )
        .route(
            "/api/v1/storage/shares",
            get(get_storage_shares_handler).post(create_storage_share_handler),
        )
        .route(
            "/api/v1/storage/shares/:id",
            delete(delete_storage_share_handler),
        )
        .with_state(state)
}

/// GET /api/v1/storage/drives: Inspect all physical & virtual storage volumes
pub async fn storage_drives_handler() -> Json<serde_json::Value> {
    let drives = VolumeInspector::inspect_volumes();
    Json(serde_json::json!({
        "drives": drives
    }))
}

/// GET /api/v1/storage/mounts: List all persisted virtual mounts
pub async fn get_storage_mounts_handler(State(state): State<StorageApiState>) -> Response {
    match state.mount_manager.list_mounts() {
        Ok(mounts) => Json(serde_json::json!({ "mounts": mounts })).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/storage/mounts/:id: Lookup a mount by ID or name
pub async fn get_storage_mount_by_id_handler(
    AxPath(id): AxPath<String>,
    State(state): State<StorageApiState>,
) -> Response {
    let mount_opt = match state.mount_manager.get_mount(&id) {
        Ok(Some(mount)) => Some(mount),
        Ok(None) => match state.mount_manager.get_mount_by_name(&id) {
            Ok(m) => m,
            Err(e) => return e.into_response(),
        },
        Err(e) => return e.into_response(),
    };

    match mount_opt {
        Some(mount) => Json(serde_json::json!({ "mount": mount })).into_response(),
        None => ProblemDetails::mount_not_found(&id, None).into_response(),
    }
}

/// POST /api/v1/storage/mounts: Register a new virtual mount in SQLite and in-memory registry
pub async fn create_storage_mount_handler(
    State(state): State<StorageApiState>,
    Json(payload): Json<CreateMountRequest>,
) -> Response {
    let path = PathBuf::from(&payload.physical_path);

    // 1. Critical OS system quarantine interception
    if PathQuarantine::is_critical_system_path(&path) {
        return ProblemDetails::quarantined_path(None).into_response();
    }

    // 2. Resolve canonical path with Dunce
    let canonical = match dunce::canonicalize(&path) {
        Ok(c) => c,
        Err(_) => {
            if let Err(e) = std::fs::create_dir_all(&path) {
                return ProblemDetails::new(
                    StatusCode::BAD_REQUEST,
                    "https://aetherstream.dev/errors/invalid-physical-path",
                    "Invalid Physical Path",
                    format!("Physical path cannot be accessed or created: {}", e),
                )
                .with_code("INVALID_PHYSICAL_PATH")
                .into_response();
            }
            dunce::canonicalize(&path).unwrap_or(path.clone())
        }
    };

    // 3. Re-verify canonical target against quarantine
    if PathQuarantine::is_critical_system_path(&canonical) {
        return ProblemDetails::quarantined_path(None).into_response();
    }

    // 4. Validate permission mode
    let perm_mode = match PermissionMode::from_str_strict(
        payload.permission_mode.as_deref().unwrap_or("read_only"),
    ) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    let allowed_subpaths = payload.allowed_subpaths.unwrap_or_default();
    let record = VirtualMountRecord {
        id: payload.id.clone(),
        name: payload.name.clone(),
        drive_serial: payload.drive_serial.clone(),
        physical_path: canonical.to_string_lossy().to_string(),
        permission_mode: perm_mode,
        allowed_subpaths: allowed_subpaths.clone(),
        password_hash: payload.password.map(|p| format!("sha256:{}", p)),
        expiration_ts: payload.expiration_ts,
        created_at: current_utc_timestamp(),
    };

    // 5. Persist to SQLite WAL
    if let Err(e) = state.mount_manager.upsert_mount(&record) {
        return e.into_response();
    }

    // 6. Register into live MountRegistry
    let mount_perm = match perm_mode {
        PermissionMode::StreamOnly => MountPermission::StreamOnly,
        PermissionMode::ReadOnly => MountPermission::Read,
        PermissionMode::ReadWrite => MountPermission::Write,
    };
    let subpath_bufs = allowed_subpaths.iter().map(PathBuf::from).collect();
    let _ = state.mounts.register_with_whitelist(
        &payload.name,
        canonical,
        mount_perm,
        subpath_bufs,
        payload.description,
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "mount": record })),
    )
        .into_response()
}

/// PUT /api/v1/storage/mounts/:id: Update existing virtual mount configuration
pub async fn update_storage_mount_handler(
    AxPath(id): AxPath<String>,
    State(state): State<StorageApiState>,
    Json(payload): Json<UpdateMountRequest>,
) -> Response {
    let existing = match state.mount_manager.get_mount(&id) {
        Ok(Some(m)) => m,
        Ok(None) => match state.mount_manager.get_mount_by_name(&id) {
            Ok(Some(m)) => m,
            Ok(None) => return ProblemDetails::mount_not_found(&id, None).into_response(),
            Err(e) => return e.into_response(),
        },
        Err(e) => return e.into_response(),
    };

    let perm_mode = match payload.permission_mode {
        Some(ref p) => match PermissionMode::from_str_strict(p) {
            Ok(pm) => pm,
            Err(e) => return e.into_response(),
        },
        None => existing.permission_mode,
    };

    let new_name = payload.name.unwrap_or(existing.name.clone());
    let new_subpaths = payload.allowed_subpaths.unwrap_or(existing.allowed_subpaths);
    let new_exp = payload.expiration_ts.or(existing.expiration_ts);

    let (new_path, canonical_path) = if let Some(ref p_str) = payload.physical_path {
        let p = PathBuf::from(p_str);
        if PathQuarantine::is_critical_system_path(&p) {
            return ProblemDetails::quarantined_path(None).into_response();
        }
        let canonical = match dunce::canonicalize(&p) {
            Ok(c) => c,
            Err(_) => {
                if let Err(e) = std::fs::create_dir_all(&p) {
                    return ProblemDetails::new(
                        StatusCode::BAD_REQUEST,
                        "https://aetherstream.dev/errors/invalid-physical-path",
                        "Invalid Physical Path",
                        format!("Physical path cannot be accessed or created: {}", e),
                    )
                    .with_code("INVALID_PHYSICAL_PATH")
                    .into_response();
                }
                dunce::canonicalize(&p).unwrap_or(p)
            }
        };
        if PathQuarantine::is_critical_system_path(&canonical) {
            return ProblemDetails::quarantined_path(None).into_response();
        }
        (canonical.to_string_lossy().to_string(), canonical)
    } else {
        (
            existing.physical_path.clone(),
            PathBuf::from(&existing.physical_path),
        )
    };

    let updated = VirtualMountRecord {
        id: existing.id.clone(),
        name: new_name.clone(),
        drive_serial: existing.drive_serial,
        physical_path: new_path,
        permission_mode: perm_mode,
        allowed_subpaths: new_subpaths.clone(),
        password_hash: existing.password_hash,
        expiration_ts: new_exp,
        created_at: existing.created_at,
    };

    if let Err(e) = state.mount_manager.upsert_mount(&updated) {
        return e.into_response();
    }

    if existing.name != new_name {
        state.mounts.unregister(&existing.name);
    }

    let mount_perm = match perm_mode {
        PermissionMode::StreamOnly => MountPermission::StreamOnly,
        PermissionMode::ReadOnly => MountPermission::Read,
        PermissionMode::ReadWrite => MountPermission::Write,
    };
    let subpath_bufs = new_subpaths.iter().map(PathBuf::from).collect();
    let _ = state.mounts.register_with_whitelist(
        &new_name,
        canonical_path,
        mount_perm,
        subpath_bufs,
        payload.description,
    );

    Json(serde_json::json!({ "mount": updated })).into_response()
}

/// DELETE /api/v1/storage/mounts/:id: Unmount and delete virtual mount record
pub async fn delete_storage_mount_handler(
    AxPath(id): AxPath<String>,
    State(state): State<StorageApiState>,
) -> Response {
    let mount_opt = match state.mount_manager.get_mount(&id) {
        Ok(Some(m)) => Some(m),
        Ok(None) => match state.mount_manager.get_mount_by_name(&id) {
            Ok(m) => m,
            Err(e) => return e.into_response(),
        },
        Err(e) => return e.into_response(),
    };

    let mount = match mount_opt {
        Some(m) => m,
        None => return ProblemDetails::mount_not_found(&id, None).into_response(),
    };

    match state.mount_manager.delete_mount(&mount.id) {
        Ok(true) => {
            state.mounts.unregister(&mount.name);
            state.mounts.unregister(&mount.id);
            Json(serde_json::json!({
                "status": "deleted",
                "id": mount.id,
                "name": mount.name
            }))
            .into_response()
        }
        Ok(false) => ProblemDetails::mount_not_found(&id, None).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/storage/shares: List all active share rules
pub async fn get_storage_shares_handler(State(state): State<StorageApiState>) -> Response {
    match state.mount_manager.list_share_rules() {
        Ok(shares) => Json(serde_json::json!({ "shares": shares })).into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/storage/shares: Create or upsert a share rule
pub async fn create_storage_share_handler(
    State(state): State<StorageApiState>,
    Json(payload): Json<CreateShareRuleRequest>,
) -> Response {
    let path = PathBuf::from(&payload.physical_path);
    if PathQuarantine::is_critical_system_path(&path) {
        return ProblemDetails::quarantined_path(None).into_response();
    }

    let perm_mode = match PermissionMode::from_str_strict(
        payload.permission_mode.as_deref().unwrap_or("stream_only"),
    ) {
        Ok(pm) => pm,
        Err(e) => return e.into_response(),
    };

    let record = ShareRuleRecord {
        id: payload.id.clone(),
        name: payload.name.clone(),
        drive_serial: payload.drive_serial.clone(),
        physical_path: payload.physical_path.clone(),
        permission_mode: perm_mode,
        allowed_subpaths: payload.allowed_subpaths.unwrap_or_default(),
        password_hash: payload.password.map(|p| format!("sha256:{}", p)),
        expiration_ts: payload.expiration_ts,
        created_at: current_utc_timestamp(),
    };

    match state.mount_manager.upsert_share_rule(&record) {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "share": record })),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/v1/storage/shares/:id: Delete a share rule
pub async fn delete_storage_share_handler(
    AxPath(id): AxPath<String>,
    State(state): State<StorageApiState>,
) -> Response {
    match state.mount_manager.delete_share_rule(&id) {
        Ok(true) => Json(serde_json::json!({ "status": "deleted", "id": id })).into_response(),
        Ok(false) => ProblemDetails::new(
            StatusCode::NOT_FOUND,
            "https://aetherstream.dev/errors/share-not-found",
            "Share Rule Not Found",
            format!("Share rule '{}' was not found.", id),
        )
        .with_code("SHARE_NOT_FOUND")
        .into_response(),
        Err(e) => e.into_response(),
    }
}

