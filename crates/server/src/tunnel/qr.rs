use crate::tunnel::status_api::TunnelApiState;
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use image::ImageEncoder;
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Debug, thiserror::Error)]
pub enum QrError {
    #[error("QR encoding failed: {0}")]
    Encoding(#[from] qrcode::types::QrError),
    #[error("PNG generation failed: {0}")]
    Image(#[from] image::ImageError),
    #[error("No target URL available for QR generation: {0}")]
    NoTargetUrl(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct QrQueryParams {
    pub format: Option<String>,
    pub target: Option<String>,
    pub size: Option<u32>,
    pub margin: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrJsonResponse {
    pub target_url: String,
    pub format: String,
    pub svg: Option<String>,
    pub ascii: Option<String>,
}

/// Generates a dense unicode double-block ASCII string suitable for terminal display
pub fn generate_ascii_qr(data: &str) -> Result<String, QrError> {
    let code = QrCode::new(data.as_bytes())?;
    let image = code
        .render::<char>()
        .quiet_zone(true)
        .module_dimensions(2, 1)
        .build();
    Ok(image)
}

/// Generates a scalable vector graphic (SVG) XML string
pub fn generate_svg_qr(data: &str, size: Option<u32>, margin: Option<u32>) -> Result<String, QrError> {
    let code = QrCode::new(data.as_bytes())?;
    let dimension = size.unwrap_or(256);
    let quiet_zone = margin.unwrap_or(2);

    let svg_xml = code
        .render::<svg::Color>()
        .min_dimensions(dimension, dimension)
        .quiet_zone(quiet_zone > 0)
        .build();

    Ok(svg_xml)
}

/// Generates PNG raster image bytes
pub fn generate_png_qr(data: &str, size: Option<u32>, margin: Option<u32>) -> Result<Vec<u8>, QrError> {
    let code = QrCode::new(data.as_bytes())?;
    let dimension = size.unwrap_or(256);
    let quiet_zone = margin.unwrap_or(2);

    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(dimension, dimension)
        .quiet_zone(quiet_zone > 0)
        .build();

    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut buf));
    encoder.write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::L8,
    )?;

    Ok(buf)
}

/// Formats a clean ASCII QR banner for logging to terminal stdout on server boot
pub fn format_boot_qr_banner(url: &str, title: &str) -> String {
    let qr_art = generate_ascii_qr(url).unwrap_or_else(|_| "[QR Code Error]".to_string());
    let separator = "─".repeat(56);

    format!(
        "\n┌{sep}┐\n\
         │  {title:<52}  │\n\
         │  Direct Connect URL: {url:<32} │\n\
         ├{sep}┤\n\
         {qr}\n\
         └{sep}┘\n",
        sep = separator,
        title = title,
        url = url,
        qr = qr_art
    )
}

/// GET /api/v1/tunnel/qr: Dynamic QR code endpoint supporting SVG, PNG, ASCII, and JSON
pub async fn tunnel_qr_handler(
    State(state): State<TunnelApiState>,
    Query(params): Query<QrQueryParams>,
) -> Response {
    let format = params
        .format
        .as_deref()
        .unwrap_or("svg")
        .to_ascii_lowercase();

    let target_type = params
        .target
        .as_deref()
        .unwrap_or("auto")
        .to_ascii_lowercase();

    // Determine target URL
    let target_url = match target_type.as_str() {
        "wan" | "tunnel" => match state.supervisor.public_url().await {
            Some(u) => u,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("content-type", "application/json")],
                    r#"{"error":"Public tunnel URL has not been assigned yet"}"#,
                )
                    .into_response();
            }
        },
        "loopback" => format!("http://127.0.0.1:{}", state.daemon_port),
        "lan" => {
            let status = state.build_status().await;
            status.lan_url
        }
        "auto" => {
            // Prefer public tunnel URL if available; otherwise local LAN
            match state.supervisor.public_url().await {
                Some(u) => u,
                None => {
                    let status = state.build_status().await;
                    status.lan_url
                }
            }
        }
        custom => custom.to_string(),
    };

    let size = params.size;
    let margin = params.margin;

    match format.as_str() {
        "png" => match generate_png_qr(&target_url, size, margin) {
            Ok(bytes) => {
                let mut resp = bytes.into_response();
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("image/png"),
                );
                resp.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=60"),
                );
                resp
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to generate PNG QR: {e}"),
            )
                .into_response(),
        },
        "ascii" | "txt" => match generate_ascii_qr(&target_url) {
            Ok(ascii) => {
                let mut resp = ascii.into_response();
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                );
                resp
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to generate ASCII QR: {e}"),
            )
                .into_response(),
        },
        "json" => {
            let svg = generate_svg_qr(&target_url, size, margin).ok();
            let ascii = generate_ascii_qr(&target_url).ok();
            Json(QrJsonResponse {
                target_url,
                format,
                svg,
                ascii,
            })
            .into_response()
        }
        // Default to SVG
        _ => match generate_svg_qr(&target_url, size, margin) {
            Ok(svg_xml) => {
                let mut resp = svg_xml.into_response();
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("image/svg+xml"),
                );
                resp.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=60"),
                );
                resp
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to generate SVG QR: {e}"),
            )
                .into_response(),
        },
    }
}

/// Standalone Axum Router for `/api/v1/tunnel/qr`
pub fn tunnel_qr_router(state: TunnelApiState) -> Router {
    Router::new()
        .route("/api/v1/tunnel/qr", get(tunnel_qr_handler))
        .with_state(state)
}

