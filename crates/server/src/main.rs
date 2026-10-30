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
use std::path::Path;
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
use storage::disk_watcher::DiskWatcher;
use storage::mounts::MountRegistry;
use storage::stream::RangeRequestEngine;
use storage::upload::ResumableUploadManager;
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

    // Probe hardware acceleration
    let hw_accel = TranscodePipeline::probe_hardware().await;
    info!("Hardware Acceleration Strategy: {:?}", hw_accel);

    // Load configuration
    let mut config = ServerConfig::default();
    config.host = args.host;
    config.port = args.port;

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

    let mounts_arc = Arc::new(mounts);
    DiskWatcher::spawn_storage_event_dispatcher(disk_watcher.clone(), mounts_arc.clone());

    let upload_manager = Arc::new(ResumableUploadManager::new());

    let app_state = AppState {
        config: config.clone(),
        mounts: mounts_arc,
        hw_accel,
        upload_manager,
        disk_watcher,
    };

    // CORS configuration
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Assemble Axum Router
    let mut app = Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/mounts", get(list_mounts_handler))
        .route("/api/v1/system/drives", get(system_drives_handler))
        .route("/api/v1/storage/events", get(storage_events_sse_handler))
        .route("/api/v1/browse/:mount", get(browse_root_handler))
        .route("/api/v1/browse/:mount/*path", get(browse_path_handler))
        .route("/api/v1/stream/:mount/*path", get(stream_file_handler))
        .route("/api/v1/share/create", post(create_share_link_handler))
        .route("/api/v1/upload/:mount/init", post(init_upload_handler))
        .route("/api/v1/upload/:mount/chunk", post(upload_chunk_handler))
        .route("/api/v1/upload/:mount/complete", post(complete_upload_handler))
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
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

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
    let drives = MountRegistry::discover_detailed_drives();
    Json(serde_json::json!({
        "drives": drives
    }))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
    download: Option<bool>,
}

async fn browse_root_handler(
    AxPath(mount): AxPath<String>,
    State(state): State<AppState>,
) -> Response {
    browse_internal(&mount, Path::new(""), &state).await
}

async fn browse_path_handler(
    AxPath((mount, subpath)): AxPath<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    browse_internal(&mount, Path::new(&subpath), &state).await
}

async fn browse_internal(mount: &str, subpath: &Path, state: &AppState) -> Response {
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
struct CreateShareRequest {
    mount: String,
    path: String,
    ttl_seconds: Option<u64>,
    allow_write: Option<bool>,
}

async fn create_share_link_handler(
    State(state): State<AppState>,
    Json(payload): Json<CreateShareRequest>,
) -> Response {
    let ttl = payload.ttl_seconds.unwrap_or(86400); // 24 hours default
    let allow_write = payload.allow_write.unwrap_or(false);

    let token = HmacTokenValidator::create_token_string(
        &state.config.hmac_secret,
        &payload.mount,
        &payload.path,
        ttl,
        allow_write,
    );

    let clean_path = payload.path.trim_start_matches('/');
    let stream_url = format!("/api/v1/stream/{}/{}?token={}", payload.mount, clean_path, token);
    let download_url = format!(
        "/api/v1/stream/{}/{}?token={}&download=true",
        payload.mount, clean_path, token
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "token": token,
            "stream_url": stream_url,
            "download_url": download_url,
            "ttl_seconds": ttl,
            "allow_write": allow_write
        })),
    )
        .into_response()
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
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{:?}", e) })),
        )
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
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{:?}", e) })),
        )
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
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{:?}", e) })),
        )
            .into_response(),
    }
}

async fn hls_master_manifest_handler(
    AxPath((mount, subpath)): AxPath<(String, String)>,
) -> Response {
    let clean_subpath = subpath
        .strip_suffix("/master.m3u8")
        .or_else(|| subpath.strip_suffix("master.m3u8"))
        .unwrap_or(&subpath);
    let base_prefix = format!("/api/v1/transcode/hls/{}/{}", mount, clean_subpath);
    let manifest = TranscodePipeline::generate_master_manifest(&base_prefix);

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
