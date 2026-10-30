use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

use crate::security::hmac_auth::{AuthError, ScopedTokenClaims, TokenAuthority, TokenPermission};
use crate::security::ProblemDetails;
use crate::storage::mount_manager::{MountManager, PermissionMode, ShareRuleRecord};
use crate::storage::mounts::MountRegistry;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Cryptographic Passkey Protection Engine utilizing salted SHA-256 and constant-time verification
pub struct PasskeyProtection;

impl PasskeyProtection {
    /// Generates a unique, cryptographically strong salt derived from the server secret,
    /// high-resolution nanosecond timestamp, and atomic sequence counter
    pub fn generate_salt(secret: &str) -> String {
        let count = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let payload = format!("salt|{}|{}|{}", nanos, count, std::process::id());
        let sig = TokenAuthority::sign_canonical_payload(secret, &payload);
        hex::encode(&sig[..16])
    }

    /// Computes a salted cryptographic hash of the provided passkey
    pub fn hash_passkey(secret: &str, passkey: &str) -> String {
        let salt = Self::generate_salt(secret);
        let mut hasher = Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(b":");
        hasher.update(passkey.as_bytes());
        let digest = hasher.finalize();
        format!("{}:{}", salt, hex::encode(digest))
    }

    /// Verifies candidate passkey against stored salt:hash in strict constant time
    pub fn verify_passkey(provided_passkey: &str, stored_hash: &str) -> bool {
        let parts: Vec<&str> = stored_hash.split(':').collect();
        if parts.len() != 2 {
            return false;
        }
        let salt = parts[0];
        let expected_hex = parts[1];
        let expected_bytes = match hex::decode(expected_hex) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let mut hasher = Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(b":");
        hasher.update(provided_passkey.as_bytes());
        let candidate = hasher.finalize();

        // Constant-time equality check to prevent timing side-channels
        candidate.as_slice().ct_eq(&expected_bytes).into()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateShareLinkRequest {
    pub mount: String,
    pub path: String,
    pub permission_mode: Option<String>,
    pub ttl_seconds: Option<u64>,
    pub passkey: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicShareDetails {
    pub share_id: String,
    pub token: String,
    pub mount: String,
    pub path: String,
    pub permission_mode: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub ttl_seconds: u64,
    pub has_passkey: bool,
    pub share_url: String,
    pub stream_url: String,
    pub download_url: String,
    pub browse_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareTokenInfo {
    pub mount: String,
    pub path: String,
    pub permission_mode: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub ttl_remaining_seconds: u64,
    pub has_passkey: bool,
    pub is_expired: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifyShareRequest {
    pub token: String,
    pub passkey: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyShareResponse {
    pub valid: bool,
    pub mount: String,
    pub path: String,
    pub permission_mode: String,
    pub stream_url: String,
    pub download_url: String,
    pub browse_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShareAccessQuery {
    pub passkey: Option<String>,
    pub download: Option<bool>,
}

/// Shared application state for public share operations
#[derive(Clone)]
pub struct PublicShareState {
    pub hmac_secret: String,
    pub mount_manager: Arc<MountManager>,
    pub mounts: Arc<MountRegistry>,
}

impl PublicShareState {
    pub fn new(
        hmac_secret: String,
        mount_manager: Arc<MountManager>,
        mounts: Arc<MountRegistry>,
    ) -> Self {
        Self {
            hmac_secret,
            mount_manager,
            mounts,
        }
    }
}

/// Helper function to create a public share and persist its rule to SQLite WAL
#[allow(clippy::result_large_err)]
pub fn create_public_share_link(
    state: &PublicShareState,
    payload: &CreateShareLinkRequest,
) -> Result<PublicShareDetails, ProblemDetails> {
    let clean_path = payload.path.trim_matches(|c| c == '/' || c == '\\').to_string();

    let perm_mode = match payload
        .permission_mode
        .as_deref()
        .unwrap_or("stream_only")
        .to_lowercase()
        .as_str()
    {
        "read_write" | "write" | "rw" => TokenPermission::ReadWrite,
        "read_only" | "read" | "ro" => TokenPermission::ReadOnly,
        _ => TokenPermission::StreamOnly,
    };

    let ttl = payload.ttl_seconds.unwrap_or(86400); // 24 hours default
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expires_at = now + ttl;

    // Generate unique share ID
    let count = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let share_id = format!("sh_{:016x}", count);

    let (nonce, passkey_hash, has_passkey) = match &payload.passkey {
        Some(pk) if !pk.trim().is_empty() => {
            let hash = PasskeyProtection::hash_passkey(&state.hmac_secret, pk);
            let nonce_str = format!("pk:{}:{}", share_id, &hash[..16]);
            (nonce_str, Some(hash), true)
        }
        _ => (format!("open:{}", share_id), None, false),
    };

    // Generate signed, scoped Base64URL token
    let token = TokenAuthority::generate_scoped_token(
        &state.hmac_secret,
        &payload.mount,
        &clean_path,
        perm_mode,
        ttl,
        Some(&nonce),
    );

    // Persist into SQLite WAL share_rules
    let record = ShareRuleRecord {
        id: share_id.clone(),
        name: format!("{}:{}", payload.mount, clean_path),
        drive_serial: None,
        physical_path: format!("{}/{}", payload.mount, clean_path),
        permission_mode: match perm_mode {
            TokenPermission::StreamOnly => PermissionMode::StreamOnly,
            TokenPermission::ReadOnly => PermissionMode::ReadOnly,
            TokenPermission::ReadWrite => PermissionMode::ReadWrite,
        },
        allowed_subpaths: vec![clean_path.clone()],
        password_hash: passkey_hash,
        expiration_ts: Some(expires_at as i64),
        created_at: format!("{}s", now),
    };

    if let Err(e) = state.mount_manager.upsert_share_rule(&record) {
        tracing::error!("Failed to persist share rule to SQLite: {}", e);
        return Err(ProblemDetails::internal_io_error(None));
    }

    let share_url = format!("/api/v1/share/access/{}", token);
    let stream_url = format!("/api/v1/stream/{}/{}?token={}", payload.mount, clean_path, token);
    let download_url = format!(
        "/api/v1/stream/{}/{}?token={}&download=true",
        payload.mount, clean_path, token
    );
    let browse_url = format!("/api/v1/browse/{}/{}?token={}", payload.mount, clean_path, token);

    Ok(PublicShareDetails {
        share_id,
        token,
        mount: payload.mount.clone(),
        path: clean_path,
        permission_mode: perm_mode.as_str().to_string(),
        issued_at: now,
        expires_at,
        ttl_seconds: ttl,
        has_passkey,
        share_url,
        stream_url,
        download_url,
        browse_url,
    })
}

/// Validates a share token and its passkey (if required) in constant time
pub fn verify_share_token_and_passkey(
    state: &PublicShareState,
    token: &str,
    provided_passkey: Option<&str>,
) -> Result<ScopedTokenClaims, AuthError> {
    // 1. Validate cryptographic token structure and signature
    let claims = TokenAuthority::inspect_token(&state.hmac_secret, token)?;

    // 2. Check if token requires passkey
    if claims.nonce.starts_with("pk:") {
        let parts: Vec<&str> = claims.nonce.split(':').collect();
        if parts.len() < 2 {
            return Err(AuthError::MalformedToken("Invalid passkey nonce format".to_string()));
        }
        let share_id = parts[1];

        // Retrieve stored hash from SQLite WAL
        let stored_rule = state
            .mount_manager
            .get_share_rule(share_id)
            .map_err(|_| AuthError::InternalError)?
            .ok_or(AuthError::MissingToken)?;

        let stored_hash = match stored_rule.password_hash {
            Some(ref h) => h,
            None => return Err(AuthError::PasskeyRequired),
        };

        // Validate provided passkey
        let pass = match provided_passkey {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Err(AuthError::PasskeyRequired),
        };

        if !PasskeyProtection::verify_passkey(pass, stored_hash) {
            return Err(AuthError::InvalidPasskey);
        }
    }

    Ok(claims)
}

/// Builds the public share Axum router
pub fn public_share_router(state: PublicShareState) -> Router {
    Router::new()
        .route("/api/v1/share/create", post(create_share_handler))
        .route("/api/v1/share/info/:token", get(share_info_handler))
        .route("/api/v1/share/verify", post(share_verify_handler))
        .route("/api/v1/share/access/:token", get(share_access_handler))
        .with_state(state)
}

/// POST /api/v1/share/create: Generates public sharing link with optional passkey and expiration
pub async fn create_share_handler(
    State(state): State<PublicShareState>,
    Json(payload): Json<CreateShareLinkRequest>,
) -> Response {
    match create_public_share_link(&state, &payload) {
        Ok(details) => (StatusCode::CREATED, Json(details)).into_response(),
        Err(prob) => prob.into_response(),
    }
}

/// GET /api/v1/share/info/:token: Inspect public share metadata and expiration status
pub async fn share_info_handler(
    AxPath(token): AxPath<String>,
    State(state): State<PublicShareState>,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let claims = match TokenAuthority::inspect_token(&state.hmac_secret, &token) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let has_passkey = claims.nonce.starts_with("pk:");
    let is_expired = now > claims.expires_at;
    let ttl_remaining = if is_expired {
        0
    } else {
        claims.expires_at.saturating_sub(now)
    };

    Json(ShareTokenInfo {
        mount: claims.mount_id,
        path: claims.path_prefix,
        permission_mode: claims.permission.as_str().to_string(),
        issued_at: claims.issued_at,
        expires_at: claims.expires_at,
        ttl_remaining_seconds: ttl_remaining,
        has_passkey,
        is_expired,
    })
    .into_response()
}

/// POST /api/v1/share/verify: Verifies token validity, expiration, and passkey
pub async fn share_verify_handler(
    State(state): State<PublicShareState>,
    Json(payload): Json<VerifyShareRequest>,
) -> Response {
    match verify_share_token_and_passkey(&state, &payload.token, payload.passkey.as_deref()) {
        Ok(claims) => {
            let clean_path = claims.path_prefix.trim_matches(|c| c == '/' || c == '\\');
            let stream_url = format!(
                "/api/v1/stream/{}/{}?token={}",
                claims.mount_id, clean_path, payload.token
            );
            let download_url = format!(
                "/api/v1/stream/{}/{}?token={}&download=true",
                claims.mount_id, clean_path, payload.token
            );
            let browse_url = format!(
                "/api/v1/browse/{}/{}?token={}",
                claims.mount_id, clean_path, payload.token
            );

            Json(VerifyShareResponse {
                valid: true,
                mount: claims.mount_id,
                path: clean_path.to_string(),
                permission_mode: claims.permission.as_str().to_string(),
                stream_url,
                download_url,
                browse_url,
            })
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/share/access/:token: Public landing endpoint with passkey validation
pub async fn share_access_handler(
    AxPath(token): AxPath<String>,
    Query(query): Query<ShareAccessQuery>,
    headers: HeaderMap,
    State(state): State<PublicShareState>,
) -> Response {
    let passkey = query
        .passkey
        .as_deref()
        .or_else(|| headers.get("X-Share-Passkey").and_then(|h| h.to_str().ok()));

    match verify_share_token_and_passkey(&state, &token, passkey) {
        Ok(claims) => {
            let clean_path = claims.path_prefix.trim_matches(|c| c == '/' || c == '\\');
            let download = query.download.unwrap_or(false);
            let stream_url = format!(
                "/api/v1/stream/{}/{}?token={}{}",
                claims.mount_id,
                clean_path,
                token,
                if download { "&download=true" } else { "" }
            );

            Json(serde_json::json!({
                "status": "authorized",
                "mount": claims.mount_id,
                "path": clean_path,
                "permission_mode": claims.permission.as_str(),
                "resource_url": stream_url,
                "download": download
            }))
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

