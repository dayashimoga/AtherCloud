use crate::tunnel::edge_router::EdgeRoutingClassifier;
use crate::tunnel::heartbeat::{TunnelHealthState, TunnelHeartbeatWorker};
use crate::tunnel::mdns::detect_local_ip;
use crate::tunnel::supervisor::{CloudflaredSupervisor, TunnelState};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Telemetry payload returned by GET /api/v1/tunnel/status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TunnelStatusResponse {
    pub state: TunnelState,
    pub health: TunnelHealthState,
    pub is_running: bool,
    pub is_healthy: bool,
    pub pid: Option<u32>,
    pub public_url: Option<String>,
    pub target_url: String,
    pub lan_url: String,
    pub loopback_url: String,
    pub uptime_secs: u64,
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    pub binary_path: Option<String>,
    pub latency_ms: Option<f64>,
    pub avg_latency_ms: Option<f64>,
    pub min_latency_ms: Option<f64>,
    pub max_latency_ms: Option<f64>,
    pub total_pings: u64,
    pub total_ping_failures: u64,
    pub consecutive_failures: u32,
    pub last_ping_epoch_ms: Option<u64>,
    pub last_healthy_epoch_ms: Option<u64>,
    pub last_error: Option<String>,
}

/// Shared application state powering the tunnel REST API
#[derive(Clone)]
pub struct TunnelApiState {
    pub supervisor: CloudflaredSupervisor,
    pub heartbeat: Option<TunnelHeartbeatWorker>,
    pub classifier: Option<Arc<EdgeRoutingClassifier>>,
    pub daemon_port: u16,
}

impl TunnelApiState {
    pub fn new(supervisor: CloudflaredSupervisor, daemon_port: u16) -> Self {
        Self {
            supervisor,
            heartbeat: None,
            classifier: None,
            daemon_port,
        }
    }

    pub fn with_heartbeat(mut self, heartbeat: TunnelHeartbeatWorker) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }

    pub fn with_classifier(mut self, classifier: Arc<EdgeRoutingClassifier>) -> Self {
        self.classifier = Some(classifier);
        self
    }

    /// Constructs the current comprehensive tunnel telemetry status report
    pub async fn build_status(&self) -> TunnelStatusResponse {
        let sup_status = self.supervisor.status().await;

        let (
            health,
            latency_ms,
            avg_latency_ms,
            min_latency_ms,
            max_latency_ms,
            total_pings,
            total_ping_failures,
            consecutive_failures,
            last_ping_epoch_ms,
            last_healthy_epoch_ms,
            last_error,
        ) = match &self.heartbeat {
            Some(hb) => {
                let m = hb.metrics();
                (
                    m.state,
                    m.last_latency_ms,
                    m.avg_latency_ms,
                    m.min_latency_ms,
                    m.max_latency_ms,
                    m.total_pings,
                    m.total_failures,
                    m.consecutive_failures,
                    m.last_ping_at_epoch_ms,
                    m.last_healthy_epoch_ms,
                    m.last_error,
                )
            }
            None => (
                if sup_status.state == TunnelState::Running {
                    TunnelHealthState::Healthy
                } else {
                    TunnelHealthState::Paused
                },
                None,
                None,
                None,
                None,
                0,
                0,
                0,
                None,
                None,
                None,
            ),
        };

        let lan_ip = match &self.classifier {
            Some(c) => c.config().lan_ip.unwrap_or_else(detect_local_ip),
            None => detect_local_ip(),
        };

        let is_running = sup_status.state == TunnelState::Running;
        let is_healthy = health == TunnelHealthState::Healthy;

        TunnelStatusResponse {
            state: sup_status.state,
            health,
            is_running,
            is_healthy,
            pid: sup_status.pid,
            public_url: sup_status.public_url,
            target_url: sup_status.target_url,
            lan_url: format!("http://{}:{}", lan_ip, self.daemon_port),
            loopback_url: format!("http://127.0.0.1:{}", self.daemon_port),
            uptime_secs: sup_status.uptime_secs,
            restart_count: sup_status.restart_count,
            last_exit_code: sup_status.last_exit_code,
            binary_path: sup_status.binary_path.map(|p| p.to_string_lossy().to_string()),
            latency_ms,
            avg_latency_ms,
            min_latency_ms,
            max_latency_ms,
            total_pings,
            total_ping_failures,
            consecutive_failures,
            last_ping_epoch_ms,
            last_healthy_epoch_ms,
            last_error,
        }
    }
}

/// GET /api/v1/tunnel/status: Exposes active tunnel state, public URL, uptime, and latency
pub async fn tunnel_status_handler(
    State(state): State<TunnelApiState>,
) -> impl IntoResponse {
    (StatusCode::OK, Json(state.build_status().await))
}

/// Standalone Axum Router for `/api/v1/tunnel/status`
pub fn tunnel_status_router(state: TunnelApiState) -> Router {
    Router::new()
        .route("/api/v1/tunnel/status", get(tunnel_status_handler))
        .with_state(state)
}

