use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::gpu_probe::{
    get_capability_report, get_validated_ladder, DryRunResult, GpuProbeEngine,
    ValidatedEncoderLadder,
};
use super::pipeline::HardwareAccel;

/// Telemetry payload for frontend HUD displaying active GPU/CPU hardware acceleration status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscodeStatusResponse {
    pub active_accel: HardwareAccel,
    pub active_encoder: String,
    pub active_hevc_encoder: Option<String>,
    pub active_av1_encoder: Option<String>,
    pub is_hardware_accelerated: bool,
    pub description: String,
    pub extra_ffmpeg_flags: Vec<String>,
    pub failover_occurred: bool,
    pub dry_run_history: Vec<DryRunResult>,
    pub os_platform: String,
    pub dri_device_accessible: bool,
    pub validated_at: String,
    pub supported_encoders: Vec<String>,
    pub supported_hwaccels: Vec<String>,
    pub status: String,
}

/// Constructs the current telemetry report from the validated ladder and capability probe
pub fn build_transcode_status_response() -> TranscodeStatusResponse {
    let ladder = get_validated_ladder().unwrap_or_else(|| {
        let report = GpuProbeEngine::parse_capabilities("", "");
        ValidatedEncoderLadder {
            active_accel: report.primary_accel,
            active_h264_encoder: "libx264".to_string(),
            active_hevc_encoder: None,
            active_av1_encoder: None,
            dry_run_history: vec![],
            failover_occurred: false,
            validation_timestamp: "0".to_string(),
        }
    });

    let report = get_capability_report().unwrap_or_else(|| {
        GpuProbeEngine::parse_capabilities("", "")
    });

    let extra_flags = ladder
        .active_accel
        .extra_ffmpeg_flags()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let status = if ladder.active_accel.is_hardware_accelerated() {
        if ladder.failover_occurred {
            "operational_with_failover".to_string()
        } else {
            "optimal".to_string()
        }
    } else if ladder.failover_occurred {
        "degraded_cpu_fallback".to_string()
    } else {
        "operational".to_string()
    };

    TranscodeStatusResponse {
        active_accel: ladder.active_accel,
        active_encoder: ladder.active_h264_encoder,
        active_hevc_encoder: ladder.active_hevc_encoder,
        active_av1_encoder: ladder.active_av1_encoder,
        is_hardware_accelerated: ladder.active_accel.is_hardware_accelerated(),
        description: ladder.active_accel.description().to_string(),
        extra_ffmpeg_flags: extra_flags,
        failover_occurred: ladder.failover_occurred,
        dry_run_history: ladder.dry_run_history,
        os_platform: report.os_platform,
        dri_device_accessible: report.dri_device_accessible,
        validated_at: ladder.validation_timestamp,
        supported_encoders: report.supported_encoders,
        supported_hwaccels: report.supported_hwaccels,
        status,
    }
}

/// Axum GET handler exposing hardware transcoding status at `/api/v1/transcode/status`
pub async fn transcode_status_handler() -> impl IntoResponse {
    Json(build_transcode_status_response())
}

/// Standalone Axum Router for `/api/v1/transcode/status`
pub fn transcode_status_router() -> Router {
    Router::new().route("/api/v1/transcode/status", get(transcode_status_handler))
}
