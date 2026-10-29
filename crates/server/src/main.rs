#![allow(dead_code, unused_imports)]

use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
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
use tower_http::trace::TraceLayer;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod config;
mod security;
mod storage;
mod transcode;

use config::ServerConfig;
use security::hmac_auth::HmacTokenValidator;
use storage::mounts::MountRegistry;
use storage::stream::RangeRequestEngine;
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
    let mut mounts = MountRegistry::new();
    for (name, mount_cfg) in &config.mounts {
        mounts.register(
            name,
            mount_cfg.path.clone(),
            mount_cfg.read_only,
            mount_cfg.description.clone(),
        );
        info!("Registered Virtual Mount '{}' -> {:?}", name, mount_cfg.path);
    }

    // Launch background segment cleanup sweeper
    TranscodePipeline::spawn_cleanup_worker(config.scratch_dir.clone(), 60).await;

    let app_state = AppState {
        config: config.clone(),
        mounts: Arc::new(mounts),
        hw_accel,
    };

    // CORS configuration
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Assemble Axum Router
    let app = Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/mounts", get(list_mounts_handler))
        .route("/api/v1/browse/:mount", get(browse_root_handler))
        .route("/api/v1/browse/:mount/*path", get(browse_path_handler))
        .route("/api/v1/stream/:mount/*path", get(stream_file_handler))
        .route("/api/v1/share/create", post(create_share_link_handler))
        .route(
            "/api/v1/transcode/hls/:mount/*path/master.m3u8",
            get(hls_master_manifest_handler),
        )
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

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
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
        Ok(physical_path) => RangeRequestEngine::stream_file(&physical_path, &headers).await,
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

    let stream_url = format!(
        "/api/v1/stream/{}/{}?token={}",
        payload.mount,
        payload.path.trim_start_matches('/'),
        token
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "token": token,
            "stream_url": stream_url,
            "ttl_seconds": ttl,
            "allow_write": allow_write
        })),
    )
        .into_response()
}

async fn hls_master_manifest_handler(
    AxPath((mount, subpath)): AxPath<(String, String)>,
) -> Response {
    let base_prefix = format!("/api/v1/transcode/hls/{}/{}", mount, subpath);
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
