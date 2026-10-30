#![allow(dead_code, unused_imports)]

use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

mod config;
mod security;
mod storage;
mod transcode;

use config::ServerConfig;
use security::hmac_auth::HmacTokenValidator;
use security::ProblemDetails;
use storage::disk_watcher::DiskWatcher;
use storage::mount_manager::{MountManager, PermissionMode, ShareRuleRecord, VirtualMountRecord};
use storage::mounts::{MountPermission, MountRegistry};
use storage::stream::RangeRequestEngine;
use storage::upload::ResumableUploadManager;
use storage::volume_info::VolumeInspector;
use transcode::pipeline::{HardwareAccel, TranscodePipeline};

#[derive(Parser, Debug)]
#[command(name = "aetherstream-server", version = "1.0.0", about = "AetherStream Core Daemon")]
struct CliArgs {
    #[arg(short, long, default_value = "0.0.0.0")]
    host: String,

    #[arg(short, long, default_value_t = 8080)]
    port: u16,

    #[arg(long, default_value = "config/mounts.json")]
    config_file: String,
}

#[derive(Clone)]
pub struct AppState {
    pub config: ServerConfig,
    pub mounts: Arc<MountRegistry>,
    pub mount_manager: Arc<MountManager>,
    pub hw_accel: HardwareAccel,
    pub upload_manager: Arc<ResumableUploadManager>,
    pub disk_watcher: Arc<DiskWatcher>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to initialize tracing subscriber");

    let args = CliArgs::parse();

    info!("Starting AetherStream Core Daemon v1.0.0-PROD...");

    // Probe and validate hardware acceleration ladder with 1-frame transcode dry-run
    let (_gpu_report, validated_ladder) = TranscodePipeline::probe_and_validate_hardware().await;
    let hw_accel = validated_ladder.active_accel;
    info!(
        "Hardware Acceleration Strategy: {:?} (Active Encoder: {}, Failover Occurred: {})",
        hw_accel, validated_ladder.active_h264_encoder, validated_ladder.failover_occurred
    );

    // Load configuration
    let config = ServerConfig {
        host: args.host,
        port: args.port,
        ..Default::default()
    };

    // Initialize Mount Registry
    let mounts = MountRegistry::new();
    for (name, mount_cfg) in &config.mounts {
        match mounts.register(
            name,
            mount_cfg.path.clone(),
            mount_cfg.read_only,
            mount_cfg.description.clone(),
        ) {
            Ok(_) => info!("Registered Virtual Mount '{}' -> {:?}", name, mount_cfg.path),
            Err(e) => warn!("Skipped registering Virtual Mount '{}': {}", name, e),
        }
    }

    // Launch background segment cleanup sweeper
    TranscodePipeline::spawn_cleanup_worker(config.scratch_dir.clone(), 60).await;

    // Initialize OS Storage Bus & Disk Watcher
    let disk_watcher = Arc::new(DiskWatcher::new());
    if let Err(e) = disk_watcher.start() {
        warn!("Failed to initialize OS disk watcher: {}", e);
    } else {
        info!("OS Storage Bus & Hot-Plug Event Watcher active");
    }

    let db_path = config.scratch_dir.join("mounts.db");
    let mount_manager = Arc::new(
        MountManager::new(&db_path)
            .unwrap_or_else(|_| MountManager::in_memory().expect("In-memory MountManager")),
    );

    // Sync persisted SQLite mounts into in-memory MountRegistry
    if let Ok(persisted_mounts) = mount_manager.list_mounts() {
        for m in persisted_mounts {
            let perm = match m.permission_mode {
                PermissionMode::StreamOnly => MountPermission::StreamOnly,
                PermissionMode::ReadOnly => MountPermission::Read,
                PermissionMode::ReadWrite => MountPermission::Write,
            };
            let subpaths = m.allowed_subpaths.into_iter().map(PathBuf::from).collect();
            let _ = mounts.register_with_whitelist(
                &m.name,
                PathBuf::from(m.physical_path),
                perm,
                subpaths,
                None,
            );
        }
    }

    let mounts_arc = Arc::new(mounts);
    DiskWatcher::spawn_storage_event_dispatcher(disk_watcher.clone(), mounts_arc.clone());

    let upload_manager = Arc::new(ResumableUploadManager::new());

    let app_state = AppState {
        config: config.clone(),
        mounts: mounts_arc.clone(),
        mount_manager: mount_manager.clone(),
        hw_accel,
        upload_manager,
        disk_watcher,
    };

    // CORS configuration
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let storage_api_state =
        storage::StorageApiState::new(mounts_arc.clone(), mount_manager.clone());
    let storage_routes = storage::storage_api_router(storage_api_state);

    let share_state = security::PublicShareState::new(
        config.hmac_secret.clone(),
        mount_manager.clone(),
        mounts_arc.clone(),
    );
    let share_routes = security::public_share_router(share_state);

    // Assemble Axum Router
    let mut app = Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/mounts", get(list_mounts_handler))
        .route("/api/v1/system/drives", get(system_drives_handler))
        .route("/api/v1/storage/events", get(storage_events_sse_handler))
        .route("/api/v1/storage/telemetry", get(storage_telemetry_handler))
        .route("/api/v1/browse/:mount", get(browse_root_handler))
        .route("/api/v1/browse/:mount/*path", get(browse_path_handler))
        .route("/api/v1/stream/:mount/*path", get(stream_file_handler))
        .route("/api/v1/upload/:mount/init", post(init_upload_handler))
        .route("/api/v1/upload/:mount/chunk", post(upload_chunk_handler))
        .route(
            "/api/v1/upload/:mount/status/:upload_id",
            get(upload_status_handler),
        )
        .route("/api/v1/upload/:mount/complete", post(complete_upload_handler))
        .route(
            "/api/v1/transcode/status",
            get(transcode::transcode_status_handler),
        )
        .route(
            "/api/v1/transcode/hls/:mount/*path",
            get(hls_master_manifest_handler),
        );

    if Path::new("client/dist").exists() {
        info!("Serving static universal client UI from 'client/dist'...");
        app = app.fallback_service(
            ServeDir::new("client/dist").not_found_service(ServeFile::new("client/dist/index.html")),
        );
    }

    let app = app
        .with_state(app_state)
        .merge(storage_routes)
        .merge(share_routes)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let bind_addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    info!("AetherStream Server listening on http://{}", bind_addr);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// Handlers

async fn health_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "aetherstream-core",
        "version": "1.0.0-PROD",
        "hardware_acceleration": format!("{:?}", state.hw_accel),
        "active_mounts": state.mounts.list_mounts().len()
    }))
}

async fn list_mounts_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "mounts": state.mounts.list_mounts()
    }))
}

async fn system_drives_handler() -> Json<serde_json::Value> {
    let drives = VolumeInspector::inspect_volumes();
    Json(serde_json::json!({
        "drives": drives
    }))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
    download: Option<bool>,
    passkey: Option<String>,
}

async fn browse_root_handler(
    AxPath(mount): AxPath<String>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    browse_internal(&mount, Path::new(""), &query, &headers, &state).await
}

async fn browse_path_handler(
    AxPath((mount, subpath)): AxPath<(String, String)>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    browse_internal(&mount, Path::new(&subpath), &query, &headers, &state).await
}

async fn browse_internal(
    mount: &str,
    subpath: &Path,
    query: &TokenQuery,
    headers: &HeaderMap,
    state: &AppState,
) -> Response {
    if let Some(token) = &query.token {
        let passkey = query.passkey.as_deref().or_else(|| {
            headers
                .get("X-Share-Passkey")
                .and_then(|h| h.to_str().ok())
        });

        let share_state = security::PublicShareState::new(
            state.config.hmac_secret.clone(),
            state.mount_manager.clone(),
            state.mounts.clone(),
        );

        if let Err(auth_err) = security::verify_share_token_and_passkey(&share_state, token, passkey) {
            return auth_err.into_response();
        }

        let subpath_str = subpath.to_string_lossy();
        if let Err(auth_err) = HmacTokenValidator::validate_token(
            &state.config.hmac_secret,
            token,
            mount,
            &subpath_str,
        ) {
            return auth_err.into_response();
        }
    }

    match state.mounts.list_directory(mount, subpath) {
        Ok(items) => Json(serde_json::json!({
            "mount": mount,
            "path": subpath.to_string_lossy(),
            "items": items
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

async fn stream_file_handler(
    AxPath((mount, subpath)): AxPath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<AppState>,
) -> Response {
    // If HMAC token is provided or configured, validate
    if let Some(token) = &query.token {
        let passkey = query.passkey.as_deref().or_else(|| {
            headers
                .get("X-Share-Passkey")
                .and_then(|h| h.to_str().ok())
        });

        let share_state = security::PublicShareState::new(
            state.config.hmac_secret.clone(),
            state.mount_manager.clone(),
            state.mounts.clone(),
        );

        if let Err(auth_err) = security::verify_share_token_and_passkey(&share_state, token, passkey) {
            return auth_err.into_response();
        }

        if let Err(auth_err) = HmacTokenValidator::validate_token(
            &state.config.hmac_secret,
            token,
            &mount,
            &subpath,
        ) {
            return auth_err.into_response();
        }
    }

    match state.mounts.resolve_safe_path(&mount, Path::new(&subpath)) {
        Ok(physical_path) => {
            let mut resp = RangeRequestEngine::stream_file(&physical_path, &headers).await;
            if query.download == Some(true) {
                if let Some(file_name) = physical_path.file_name() {
                    let disp = format!("attachment; filename=\"{}\"", file_name.to_string_lossy());
                    if let Ok(hdr) = HeaderValue::from_str(&disp) {
                        resp.headers_mut().insert(axum::http::header::CONTENT_DISPOSITION, hdr);
                    }
                }
            }
            resp
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct InitUploadRequest {
    upload_id: String,
    path: String,
    total_size: u64,
    chunk_size: Option<u64>,
}

async fn init_upload_handler(
    AxPath(mount): AxPath<String>,
    State(state): State<AppState>,
    Json(payload): Json<InitUploadRequest>,
) -> Response {
    if let Err(e) = state.mounts.check_write_permission(&mount) {
        return e.into_response();
    }
    let target_path = match state.mounts.resolve_safe_path(&mount, Path::new(&payload.path)) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    let chunk_size = payload.chunk_size.unwrap_or(2 * 1024 * 1024);
    match state
        .upload_manager
        .init_session(&payload.upload_id, target_path, payload.total_size, chunk_size)
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "upload_id": session.upload_id,
                "total_chunks": session.total_chunks,
                "chunk_size": session.chunk_size,
                "total_size": session.total_size,
            })),
        )
            .into_response(),
        Err(e) => ProblemDetails::new(
            StatusCode::BAD_REQUEST,
            "https://aetherstream.dev/errors/upload-init-failed",
            "Upload Initialization Failed",
            format!("{:?}", e),
        )
        .with_code("UPLOAD_INIT_FAILED")
        .into_response(),
    }
}

#[derive(Deserialize)]
struct ChunkQuery {
    upload_id: String,
    chunk_index: usize,
    sha256: Option<String>,
}

async fn upload_chunk_handler(
    AxPath(mount): AxPath<String>,
    Query(query): Query<ChunkQuery>,
    State(state): State<AppState>,
    bytes: axum::body::Bytes,
) -> Response {
    if let Err(e) = state.mounts.check_write_permission(&mount) {
        return e.into_response();
    }

    let expected_sha256 = query.sha256.as_deref().unwrap_or("");
    match state
        .upload_manager
        .write_session_chunk(&query.upload_id, query.chunk_index, expected_sha256, &bytes)
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "upload_id": session.upload_id,
                "chunk_index": query.chunk_index,
                "received_bytes": session.received_bytes,
                "completed_chunks": session.completed_chunks.len(),
                "total_chunks": session.total_chunks,
            })),
        )
            .into_response(),
        Err(e) => ProblemDetails::new(
            StatusCode::BAD_REQUEST,
            "https://aetherstream.dev/errors/upload-chunk-failed",
            "Upload Chunk Failed",
            format!("{:?}", e),
        )
        .with_code("UPLOAD_CHUNK_FAILED")
        .into_response(),
    }
}

async fn upload_status_handler(
    AxPath((mount, upload_id)): AxPath<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    if let Err(e) = state.mounts.check_write_permission(&mount) {
        return e.into_response();
    }

    match state.upload_manager.get_session(&upload_id).await {
        Some(session) => {
            let missing = state
                .upload_manager
                .get_missing_chunks(&upload_id)
                .await
                .unwrap_or_default();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "upload_id": session.upload_id,
                    "total_chunks": session.total_chunks,
                    "completed_chunks": session.completed_chunks,
                    "missing_chunks": missing,
                    "received_bytes": session.received_bytes,
                    "total_size": session.total_size,
                    "is_completed": session.is_completed,
                })),
            )
                .into_response()
        }
        None => ProblemDetails::new(
            StatusCode::NOT_FOUND,
            "https://aetherstream.dev/errors/upload-not-found",
            "Upload Session Not Found",
            "The requested upload session ID does not exist or has expired.",
        )
        .with_code("UPLOAD_SESSION_NOT_FOUND")
        .into_response(),
    }
}

#[derive(Deserialize)]
struct CompleteUploadRequest {
    upload_id: String,
    expected_sha256: Option<String>,
}

async fn complete_upload_handler(
    AxPath(mount): AxPath<String>,
    State(state): State<AppState>,
    Json(payload): Json<CompleteUploadRequest>,
) -> Response {
    if let Err(e) = state.mounts.check_write_permission(&mount) {
        return e.into_response();
    }

    match state
        .upload_manager
        .complete_session(&payload.upload_id, payload.expected_sha256.as_deref())
        .await
    {
        Ok(final_path) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "completed",
                "upload_id": payload.upload_id,
                "path": final_path.to_string_lossy(),
            })),
        )
            .into_response(),
        Err(e) => ProblemDetails::new(
            StatusCode::BAD_REQUEST,
            "https://aetherstream.dev/errors/upload-complete-failed",
            "Upload Completion Failed",
            format!("{:?}", e),
        )
        .with_code("UPLOAD_COMPLETE_FAILED")
        .into_response(),
    }
}

async fn hls_master_manifest_handler(
    AxPath((mount, subpath)): AxPath<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let clean_subpath = subpath
        .strip_suffix("/master.m3u8")
        .or_else(|| subpath.strip_suffix("master.m3u8"))
        .unwrap_or(&subpath);

    let safe_path = match state.mounts.resolve_safe_path(&mount, Path::new(clean_subpath)) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    let base_prefix = format!("/api/v1/transcode/hls/{}/{}", mount, clean_subpath);
    let manifest = match TranscodePipeline::probe_file(&safe_path).await {
        Ok(meta) => TranscodePipeline::generate_master_manifest_for_media(&base_prefix, &meta),
        Err(_) => TranscodePipeline::generate_master_manifest(&base_prefix),
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            "application/vnd.apple.mpegurl",
        )
        .header(
            axum::http::header::CACHE_CONTROL,
            "no-cache, no-store, must-revalidate",
        )
        .body(manifest.into())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn storage_events_sse_handler(
    State(state): State<AppState>,
) -> Response {
    let rx = state.disk_watcher.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => {
                let json = serde_json::to_string(&event).unwrap_or_default();
                let event_type = match &event {
                    storage::disk_watcher::StorageDeviceEvent::Plugged(_) => "storage_plugged",
                    storage::disk_watcher::StorageDeviceEvent::Unplugged(_) => "storage_unplugged",
                };
                let sse_chunk = format!("event: {}\ndata: {}\n\n", event_type, json);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(sse_chunk)), rx))
            }
            Err(_) => None,
        }
    });

    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .header(axum::http::header::CACHE_CONTROL, "no-cache")
        .header(axum::http::header::CONNECTION, "keep-alive")
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn storage_telemetry_handler() -> Response {
    let snapshot = storage::StorageTelemetryEngine::global().snapshot();
    (StatusCode::OK, Json(snapshot)).into_response()
}
