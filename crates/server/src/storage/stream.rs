use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::io::SeekFrom;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncSeekExt;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info};

const DEFAULT_CHUNK_BURST: u64 = 2 * 1024 * 1024; // 2 MiB burst default

pub struct RangeRequestEngine;

impl RangeRequestEngine {
    /// Serves a physical file either as a full 200 stream or 206 Partial Content range stream
    pub async fn stream_file(file_path: &Path, headers: &HeaderMap) -> Response {
        let meta = match tokio::fs::metadata(file_path).await {
            Ok(m) => m,
            Err(e) => {
                error!("Unable to retrieve metadata for {:?}: {}", file_path, e);
                return (StatusCode::NOT_FOUND, "File not found").into_response();
            }
        };

        let file_size = meta.len();
        let mime_type = mime_guess::from_path(file_path)
            .first_or_octet_stream()
            .to_string();

        let mut file = match File::open(file_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open file {:?}: {}", file_path, e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "I/O read failure").into_response();
            }
        };

        // Inspect Range header
        if let Some(range_header) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
            if let Some((start, end)) = Self::parse_range(range_header, file_size) {
                if start >= file_size || end >= file_size || start > end {
                    let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                    resp.headers_mut().insert(
                        header::CONTENT_RANGE,
                        HeaderValue::from_str(&format!("bytes */{}", file_size)).unwrap(),
                    );
                    return resp;
                }

                let chunk_len = end - start + 1;
                debug!(
                    "Serving HTTP 206: range {}-{}/{} (len={}) for {:?}",
                    start, end, file_size, chunk_len, file_path
                );

                if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                    error!("Failed to seek to offset {}: {}", start, e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Seek failure").into_response();
                }

                let limited_file = file.take(chunk_len);
                let stream = ReaderStream::with_capacity(limited_file, 64 * 1024);
                let body = Body::from_stream(stream);

                let resp = Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, mime_type)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{}", start, end, file_size),
                    )
                    .header(header::CONTENT_LENGTH, chunk_len.to_string())
                    .header(header::CACHE_CONTROL, "public, max-age=3600")
                    .body(body)
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());

                return resp;
            }
        }

        // Full file stream (HTTP 200)
        info!("Serving HTTP 200 full stream (size={}) for {:?}", file_size, file_path);
        let stream = ReaderStream::with_capacity(file, 64 * 1024);
        let body = Body::from_stream(stream);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, file_size.to_string())
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(body)
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }

    /// Parses standard RFC 7233 byte ranges e.g. "bytes=0-1024", "bytes=1000-", "bytes=-500"
    pub fn parse_range(range_header: &str, total_size: u64) -> Option<(u64, u64)> {
        if !range_header.starts_with("bytes=") {
            return None;
        }

        let range_specs = &range_header["bytes=".len()..];
        // If multiple ranges (comma-separated), pick the first to prevent DoS attacks
        let first_range = range_specs.split(',').next()?.trim();
        let parts: Vec<&str> = first_range.split('-').collect();

        if parts.len() != 2 {
            return None;
        }

        let start_str = parts[0].trim();
        let end_str = parts[1].trim();

        if start_str.is_empty() && !end_str.is_empty() {
            // Suffix range: bytes=-500 (last 500 bytes)
            let suffix_len: u64 = end_str.parse().ok()?;
            if suffix_len == 0 {
                return None;
            }
            let start = total_size.saturating_sub(suffix_len);
            let end = total_size.saturating_sub(1);
            Some((start, end))
        } else if !start_str.is_empty() && end_str.is_empty() {
            // Open-ended range: bytes=1000- (from 1000 to end or chunk burst)
            let start: u64 = start_str.parse().ok()?;
            if start >= total_size {
                return None;
            }
            let end = (start + DEFAULT_CHUNK_BURST).min(total_size.saturating_sub(1));
            Some((start, end))
        } else if !start_str.is_empty() && !end_str.is_empty() {
            // Explicit range: bytes=0-1024
            let start: u64 = start_str.parse().ok()?;
            let end: u64 = end_str.parse().ok()?;
            Some((start, end))
        } else {
            None
        }
    }
}

use tokio::io::AsyncReadExt;
