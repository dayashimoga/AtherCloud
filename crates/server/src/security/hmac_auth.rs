use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedTokenClaims {
    pub mount_id: String,
    pub path_prefix: String,
    pub expires_at: u64,
    pub allow_write: bool,
    pub signature_hex: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Missing authorization token")]
    MissingToken,
    #[error("Malformed authorization token: {0}")]
    MalformedToken(String),
    #[error("Token expired at unix timestamp: {0}")]
    ExpiredToken(u64),
    #[error("Invalid cryptographic signature")]
    InvalidSignature,
    #[error("Access denied: requested path '{0}' is outside authorized scope '{1}'")]
    PathOutOfScope(String, String),
    #[error("Access denied: token is scoped to mount '{0}', but '{1}' was requested")]
    MountMismatch(String, String),
}

impl axum::response::IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::Json;
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}

pub struct HmacTokenValidator;

impl HmacTokenValidator {
    /// Generates an HMAC-SHA256 signature for a token payload
    pub fn sign(secret: &str, mount_id: &str, path_prefix: &str, expires_at: u64, allow_write: bool) -> String {
        let payload = format!("{}:{}:{}:{}", mount_id, path_prefix, expires_at, allow_write);
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Formats an opaque base64-style token string for client transmission:
    /// format: mount_id:path_prefix:expires_at:allow_write:signature_hex
    pub fn create_token_string(
        secret: &str,
        mount_id: &str,
        path_prefix: &str,
        ttl_seconds: u64,
        allow_write: bool,
    ) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_at = now + ttl_seconds;
        let sig = Self::sign(secret, mount_id, path_prefix, expires_at, allow_write);
        format!("{}:{}:{}:{}:{}", mount_id, path_prefix, expires_at, allow_write, sig)
    }

    /// Validates an incoming token string against the secret, requested mount, and target path
    pub fn validate_token(
        secret: &str,
        token_str: &str,
        requested_mount: &str,
        requested_path: &str,
    ) -> Result<ScopedTokenClaims, AuthError> {
        let parts: Vec<&str> = token_str.split(':').collect();
        if parts.len() != 5 {
            return Err(AuthError::MalformedToken("Expected 5 colon-separated fields".into()));
        }

        let mount_id = parts[0].to_string();
        let path_prefix = parts[1].to_string();
        let expires_at: u64 = parts[2]
            .parse()
            .map_err(|_| AuthError::MalformedToken("Invalid expires_at epoch".into()))?;
        let allow_write: bool = parts[3]
            .parse()
            .map_err(|_| AuthError::MalformedToken("Invalid allow_write flag".into()))?;
        let signature_hex = parts[4].to_string();

        // 1. Check expiration
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > expires_at {
            return Err(AuthError::ExpiredToken(expires_at));
        }

        // 2. Validate cryptographic signature
        let expected_sig = Self::sign(secret, &mount_id, &path_prefix, expires_at, allow_write);
        if signature_hex != expected_sig {
            warn!("HMAC signature verification failed!");
            return Err(AuthError::InvalidSignature);
        }

        // 3. Mount match
        if mount_id != requested_mount && mount_id != "*" {
            return Err(AuthError::MountMismatch(mount_id, requested_mount.to_string()));
        }

        // 4. Path prefix scoping
        let norm_req = requested_path.trim_start_matches('/').trim_start_matches('\\');
        let norm_prefix = path_prefix.trim_start_matches('/').trim_start_matches('\\');
        if !norm_prefix.is_empty() && !norm_req.starts_with(norm_prefix) {
            return Err(AuthError::PathOutOfScope(
                requested_path.to_string(),
                path_prefix,
            ));
        }

        Ok(ScopedTokenClaims {
            mount_id,
            path_prefix,
            expires_at,
            allow_write,
            signature_hex,
        })
    }
}
