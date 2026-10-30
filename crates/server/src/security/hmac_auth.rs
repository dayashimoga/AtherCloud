use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tracing::warn;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenPermission {
    StreamOnly,
    ReadOnly,
    ReadWrite,
}

impl TokenPermission {
    pub fn is_stream_allowed(&self) -> bool {
        true
    }

    pub fn is_read_allowed(&self) -> bool {
        matches!(self, TokenPermission::ReadOnly | TokenPermission::ReadWrite)
    }

    pub fn is_write_allowed(&self) -> bool {
        matches!(self, TokenPermission::ReadWrite)
    }

    pub fn from_allow_write(allow_write: bool) -> Self {
        if allow_write {
            TokenPermission::ReadWrite
        } else {
            TokenPermission::ReadOnly
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TokenPermission::StreamOnly => "stream_only",
            TokenPermission::ReadOnly => "read_only",
            TokenPermission::ReadWrite => "read_write",
        }
    }

    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "stream_only" | "stream" => Some(TokenPermission::StreamOnly),
            "read_only" | "read" | "ro" => Some(TokenPermission::ReadOnly),
            "read_write" | "write" | "rw" => Some(TokenPermission::ReadWrite),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopedTokenClaims {
    pub mount_id: String,
    pub path_prefix: String,
    pub permission: TokenPermission,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: String,
    pub allow_write: bool, // backwards-compatible field
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
    #[error("Access denied: token does not grant requested permission level")]
    InsufficientPermission,
    #[error("Internal cryptographic error")]
    InternalError,
}

impl axum::response::IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        let (detail, code) = match &self {
            AuthError::MissingToken => ("Missing authorization token in query or Authorization header.", "MISSING_TOKEN"),
            AuthError::MalformedToken(_) => ("Authorization token is malformed.", "MALFORMED_TOKEN"),
            AuthError::ExpiredToken(_) => ("Authorization token has expired.", "TOKEN_EXPIRED"),
            AuthError::InvalidSignature => ("Cryptographic HMAC-SHA256 signature verification failed.", "INVALID_SIGNATURE"),
            AuthError::PathOutOfScope(_, _) => ("Requested subpath is outside authorized token scope.", "PATH_OUT_OF_SCOPE"),
            AuthError::MountMismatch(_, _) => ("Token scope does not match requested virtual mount.", "MOUNT_MISMATCH"),
            AuthError::InsufficientPermission => ("Token permissions are insufficient for the requested operation.", "INSUFFICIENT_PERMISSION"),
            AuthError::InternalError => ("Internal authentication error occurred.", "INTERNAL_AUTH_ERROR"),
        };
        super::ProblemDetails::unauthorized("Token Authorization Failure", detail, code, None).into_response()
    }
}

/// Cryptographic Scoped Token Authority utilizing constant-time HMAC-SHA256
pub struct TokenAuthority;

impl TokenAuthority {
    /// Computes canonical 32-byte HMAC-SHA256 tag for arbitrary payload string
    pub fn sign_canonical_payload(secret: &str, payload: &str) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC-SHA256 accepts keys of any length");
        mac.update(payload.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    /// Verifies HMAC signature using constant-time equality checks
    pub fn verify_signature_constant_time(secret: &str, payload: &str, signature_bytes: &[u8]) -> bool {
        let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(payload.as_bytes());
        mac.verify_slice(signature_bytes).is_ok()
    }

    /// Generates a URL-safe Base64URL scoped token embedding granular permissions and nonce
    pub fn generate_scoped_token(
        secret: &str,
        mount_id: &str,
        path_prefix: &str,
        permission: TokenPermission,
        ttl_seconds: u64,
        nonce: Option<&str>,
    ) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_at = now + ttl_seconds;
        let effective_nonce = nonce.unwrap_or_default();

        let canonical_payload = format!(
            "v1|{}|{}|{}|{}|{}|{}",
            mount_id,
            path_prefix,
            permission.as_str(),
            now,
            expires_at,
            effective_nonce
        );

        let sig_bytes = Self::sign_canonical_payload(secret, &canonical_payload);
        let sig_hex = hex::encode(sig_bytes);

        let token_raw = format!("{}|{}", canonical_payload, sig_hex);
        URL_SAFE_NO_PAD.encode(token_raw.as_bytes())
    }

    /// Validates any token string (Base64URL v1, raw v1, opaque hex, or colon-delimited legacy)
    pub fn validate_scoped_token(
        secret: &str,
        token_str: &str,
        requested_mount: &str,
        requested_path: &str,
        required_permission: Option<TokenPermission>,
    ) -> Result<ScopedTokenClaims, AuthError> {
        let token_trimmed = token_str.trim();
        if token_trimmed.is_empty() {
            return Err(AuthError::MissingToken);
        }

        // 1. Try decoding as URL-safe Base64 v1 token
        if let Ok(decoded_bytes) = URL_SAFE_NO_PAD.decode(token_trimmed) {
            if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                if decoded_str.starts_with("v1|") {
                    return Self::validate_v1_token(
                        secret,
                        &decoded_str,
                        requested_mount,
                        requested_path,
                        required_permission,
                    );
                }
            }
        }

        // 2. Try raw v1 pipe-delimited format
        if token_trimmed.starts_with("v1|") {
            return Self::validate_v1_token(
                secret,
                token_trimmed,
                requested_mount,
                requested_path,
                required_permission,
            );
        }

        // 3. Fallback to legacy format (colon-delimited or opaque hex)
        Self::validate_legacy_token(
            secret,
            token_trimmed,
            requested_mount,
            requested_path,
            required_permission,
        )
    }

    fn validate_v1_token(
        secret: &str,
        token_str: &str,
        requested_mount: &str,
        requested_path: &str,
        required_permission: Option<TokenPermission>,
    ) -> Result<ScopedTokenClaims, AuthError> {
        let parts: Vec<&str> = token_str.split('|').collect();
        if parts.len() != 8 {
            return Err(AuthError::MalformedToken("Invalid v1 token fields".into()));
        }

        let mount_id = parts[1].to_string();
        let path_prefix = parts[2].to_string();
        let permission = TokenPermission::from_str_opt(parts[3])
            .ok_or_else(|| AuthError::MalformedToken("Invalid permission mode".into()))?;
        let issued_at: u64 = parts[4]
            .parse()
            .map_err(|_| AuthError::MalformedToken("Invalid issued_at".into()))?;
        let expires_at: u64 = parts[5]
            .parse()
            .map_err(|_| AuthError::MalformedToken("Invalid expires_at".into()))?;
        let nonce = parts[6].to_string();
        let signature_hex = parts[7].to_string();

        // 1. Validate expiration
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > expires_at {
            return Err(AuthError::ExpiredToken(expires_at));
        }

        // 2. Constant-time signature verification
        let canonical_payload = format!(
            "v1|{}|{}|{}|{}|{}|{}",
            mount_id, path_prefix, parts[3], issued_at, expires_at, nonce
        );
        let sig_bytes = hex::decode(&signature_hex)
            .map_err(|_| AuthError::InvalidSignature)?;

        if !Self::verify_signature_constant_time(secret, &canonical_payload, &sig_bytes) {
            warn!("TokenAuthority: Constant-time HMAC signature verification failed");
            return Err(AuthError::InvalidSignature);
        }

        // 3. Mount match
        if mount_id != requested_mount && mount_id != "*" {
            return Err(AuthError::MountMismatch(mount_id, requested_mount.to_string()));
        }

        // 4. Path prefix scoping (component-aware)
        if !Self::is_path_within_scope(&path_prefix, requested_path) {
            return Err(AuthError::PathOutOfScope(
                requested_path.to_string(),
                path_prefix,
            ));
        }

        // 5. Permission level check
        if let Some(req_perm) = required_permission {
            match req_perm {
                TokenPermission::ReadWrite => {
                    if !permission.is_write_allowed() {
                        return Err(AuthError::InsufficientPermission);
                    }
                }
                TokenPermission::ReadOnly => {
                    if !permission.is_read_allowed() {
                        return Err(AuthError::InsufficientPermission);
                    }
                }
                TokenPermission::StreamOnly => {
                    if !permission.is_stream_allowed() {
                        return Err(AuthError::InsufficientPermission);
                    }
                }
            }
        }

        Ok(ScopedTokenClaims {
            mount_id,
            path_prefix,
            permission,
            issued_at,
            expires_at,
            nonce,
            allow_write: permission.is_write_allowed(),
            signature_hex,
        })
    }

    fn validate_legacy_token(
        secret: &str,
        token_str: &str,
        requested_mount: &str,
        requested_path: &str,
        required_permission: Option<TokenPermission>,
    ) -> Result<ScopedTokenClaims, AuthError> {
        let decoded = if !token_str.contains(':') {
            hex::decode(token_str)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_else(|| token_str.to_string())
        } else {
            token_str.to_string()
        };

        let parts: Vec<&str> = decoded.split(':').collect();
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

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > expires_at {
            return Err(AuthError::ExpiredToken(expires_at));
        }

        let canonical_payload = format!("{}:{}:{}:{}", mount_id, path_prefix, expires_at, allow_write);
        let sig_bytes = hex::decode(&signature_hex).map_err(|_| AuthError::InvalidSignature)?;

        if !Self::verify_signature_constant_time(secret, &canonical_payload, &sig_bytes) {
            warn!("TokenAuthority: Legacy HMAC signature verification failed");
            return Err(AuthError::InvalidSignature);
        }

        if mount_id != requested_mount && mount_id != "*" {
            return Err(AuthError::MountMismatch(mount_id, requested_mount.to_string()));
        }

        if !Self::is_path_within_scope(&path_prefix, requested_path) {
            return Err(AuthError::PathOutOfScope(
                requested_path.to_string(),
                path_prefix,
            ));
        }

        let permission = TokenPermission::from_allow_write(allow_write);
        if let Some(req_perm) = required_permission {
            if req_perm == TokenPermission::ReadWrite && !allow_write {
                return Err(AuthError::InsufficientPermission);
            }
        }

        Ok(ScopedTokenClaims {
            mount_id,
            path_prefix,
            permission,
            issued_at: 0,
            expires_at,
            nonce: String::new(),
            allow_write,
            signature_hex,
        })
    }

    /// Evaluates whether a requested subpath falls within an authorized prefix scope
    /// using path component boundaries rather than raw string prefixes
    pub fn is_path_within_scope(scope_prefix: &str, requested_subpath: &str) -> bool {
        let norm_prefix = scope_prefix.trim_matches(|c| c == '/' || c == '\\');
        if norm_prefix.is_empty() {
            return true;
        }

        let prefix_path = Path::new(norm_prefix);
        let requested_path = Path::new(requested_subpath.trim_matches(|c| c == '/' || c == '\\'));

        let prefix_comps: Vec<_> = prefix_path.components().filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        }).collect();

        let req_comps: Vec<_> = requested_path.components().filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        }).collect();

        if prefix_comps.len() > req_comps.len() {
            return false;
        }

        for (i, p_comp) in prefix_comps.iter().enumerate() {
            #[cfg(windows)]
            {
                if p_comp.to_lowercase() != req_comps[i].to_lowercase() {
                    return false;
                }
            }
            #[cfg(not(windows))]
            {
                if p_comp != &req_comps[i] {
                    return false;
                }
            }
        }

        true
    }
}

/// Backwards-compatible validator adapter
pub struct HmacTokenValidator;

impl HmacTokenValidator {
    pub fn sign(secret: &str, mount_id: &str, path_prefix: &str, expires_at: u64, allow_write: bool) -> String {
        let payload = format!("{}:{}:{}:{}", mount_id, path_prefix, expires_at, allow_write);
        let sig_bytes = TokenAuthority::sign_canonical_payload(secret, &payload);
        hex::encode(sig_bytes)
    }

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

    pub fn validate_token(
        secret: &str,
        token_str: &str,
        requested_mount: &str,
        requested_path: &str,
    ) -> Result<ScopedTokenClaims, AuthError> {
        TokenAuthority::validate_scoped_token(secret, token_str, requested_mount, requested_path, None)
    }
}
