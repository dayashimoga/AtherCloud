#![allow(clippy::result_large_err)]

use crate::security::hmac_auth::TokenAuthority;
use crate::security::share_link::PasskeyProtection;
use crate::security::ProblemDetails;
use crate::tunnel::edge_router::ClientNetworkContext;
use axum::{
    extract::{Path as AxPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Pairing failure: {0}")]
    PairingFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyRecord {
    pub passkey_id: String,
    pub code: String,
    pub salt_hash: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub max_attempts: u32,
    pub attempts_used: u32,
    pub consumed: bool,
    pub client_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub session_token: String,
    pub paired_at: u64,
    pub last_seen_at: u64,
    pub ip_address: Option<String>,
    pub is_revoked: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateInvitationRequest {
    pub ttl_seconds: Option<u64>,
    pub client_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvitationResponse {
    pub passkey: String,
    pub expires_at: u64,
    pub ttl_seconds: u64,
    pub client_label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DevicePairingRequest {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub passkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePairingResponse {
    pub status: String,
    pub session_token: String,
    pub device_id: String,
    pub expires_at: u64,
    pub paired_at: u64,
}

pub struct DevicePairingAuthority {
    secret: String,
    passkeys: RwLock<HashMap<String, PasskeyRecord>>,
    devices: RwLock<HashMap<String, PairedDevice>>,
    conn: Option<Arc<Mutex<Connection>>>,
}

impl DevicePairingAuthority {
    pub fn new(secret: impl Into<String>, db_path: Option<PathBuf>) -> Result<Self, PairingError> {
        let secret = secret.into();

        let (conn, loaded_devices) = match db_path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let c = Connection::open(&path)?;
                c.execute_batch(
                    "PRAGMA journal_mode = WAL;
                     PRAGMA synchronous = NORMAL;
                     CREATE TABLE IF NOT EXISTS paired_devices (
                         device_id TEXT PRIMARY KEY NOT NULL,
                         device_name TEXT NOT NULL,
                         platform TEXT NOT NULL,
                         session_token TEXT NOT NULL,
                         paired_at INTEGER NOT NULL,
                         last_seen_at INTEGER NOT NULL,
                         ip_address TEXT,
                         is_revoked INTEGER NOT NULL DEFAULT 0
                     );",
                )?;

                let dev_map = {
                    let mut stmt = c.prepare(
                        "SELECT device_id, device_name, platform, session_token, paired_at, last_seen_at, ip_address, is_revoked FROM paired_devices"
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok(PairedDevice {
                            device_id: row.get(0)?,
                            device_name: row.get(1)?,
                            platform: row.get(2)?,
                            session_token: row.get(3)?,
                            paired_at: row.get(4)?,
                            last_seen_at: row.get(5)?,
                            ip_address: row.get(6)?,
                            is_revoked: row.get::<_, i64>(7)? != 0,
                        })
                    })?;

                    let mut map = HashMap::new();
                    for dev_res in rows {
                        let dev = dev_res?;
                        map.insert(dev.device_id.clone(), dev);
                    }
                    map
                };

                (Some(Arc::new(Mutex::new(c))), dev_map)
            }
            None => (None, HashMap::new()),
        };

        Ok(Self {
            secret,
            passkeys: RwLock::new(HashMap::new()),
            devices: RwLock::new(loaded_devices),
            conn,
        })
    }

    /// Generates a clean, 6-digit formatted passkey (e.g. "849-204")
    pub fn generate_passkey_code(&self) -> String {
        let nonce = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let payload = format!("passkey|{}|{}|{}", nanos, nonce, std::process::id());
        let hash = TokenAuthority::sign_canonical_payload(&self.secret, &payload);
        let num = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]) % 900_000 + 100_000;
        format!("{:03}-{:03}", num / 1000, num % 1000)
    }

    /// Creates a single-use, time-bounded pairing invitation passkey
    pub fn create_invitation(
        &self,
        ttl_seconds: Option<u64>,
        client_label: Option<String>,
    ) -> (String, PasskeyRecord) {
        let code = self.generate_passkey_code();
        let clean_code = code.replace('-', "").trim().to_string();
        let salt_hash = PasskeyProtection::hash_passkey(&self.secret, &clean_code);

        let now = current_epoch_secs();
        let ttl = ttl_seconds.unwrap_or(600); // 10 minutes default
        let passkey_id = hex::encode(TokenAuthority::sign_canonical_payload(
            &self.secret,
            &format!("pid|{}|{}", clean_code, now),
        ))[..16]
            .to_string();

        let record = PasskeyRecord {
            passkey_id: passkey_id.clone(),
            code: code.clone(),
            salt_hash,
            created_at: now,
            expires_at: now + ttl,
            max_attempts: 5,
            attempts_used: 0,
            consumed: false,
            client_label,
        };

        self.passkeys
            .write()
            .unwrap()
            .insert(passkey_id, record.clone());

        (code, record)
    }

    /// Verifies candidate passkey in constant time, consumes it, and issues a paired session credential
    pub fn pair_device(
        &self,
        req: DevicePairingRequest,
        client_ip: Option<String>,
    ) -> Result<DevicePairingResponse, ProblemDetails> {
        let clean_candidate = req.passkey.replace('-', "").trim().to_string();
        let now = current_epoch_secs();

        let mut passkeys = self.passkeys.write().unwrap();

        // Find candidate passkey
        let mut matched_id = None;
        for (id, record) in passkeys.iter() {
            if record.consumed {
                continue;
            }
            if PasskeyProtection::verify_passkey(&clean_candidate, &record.salt_hash) {
                matched_id = Some(id.clone());
                break;
            }
        }

        let record_id = match matched_id {
            Some(id) => id,
            None => {
                return Err(ProblemDetails::new(
                    StatusCode::UNAUTHORIZED,
                    "https://aetherstream.dev/errors/invalid-passkey",
                    "Invalid Pairing Passkey",
                    "The provided passkey is incorrect or has expired.",
                ));
            }
        };

        let record = passkeys.get_mut(&record_id).unwrap();

        // Expiry check
        if now > record.expires_at {
            return Err(ProblemDetails::new(
                StatusCode::GONE,
                "https://aetherstream.dev/errors/passkey-expired",
                "Pairing Passkey Expired",
                "The pairing passkey has expired. Please generate a new invitation.",
            ));
        }

        // Exhaustion check
        if record.attempts_used >= record.max_attempts {
            return Err(ProblemDetails::new(
                StatusCode::TOO_MANY_REQUESTS,
                "https://aetherstream.dev/errors/rate-limit-exceeded",
                "Passkey Attempts Exceeded",
                "Maximum verification attempts exceeded for this passkey.",
            ));
        }

        // Single-use consumption
        if record.consumed {
            return Err(ProblemDetails::new(
                StatusCode::CONFLICT,
                "https://aetherstream.dev/errors/passkey-consumed",
                "Passkey Already Consumed",
                "This single-use passkey has already been redeemed.",
            ));
        }

        record.consumed = true;
        record.attempts_used += 1;

        // Generate long-lived session token (30 days validity)
        let token_ttl = 30 * 86400;
        let expires_at = now + token_ttl;
        let payload = format!(
            "session|{}|{}|{}|{}",
            req.device_id, req.platform, now, expires_at
        );
        let sig = hex::encode(TokenAuthority::sign_canonical_payload(
            &self.secret,
            &payload,
        ));
        let session_token = format!("{}.{}", hex::encode(payload.as_bytes()), sig);

        let device = PairedDevice {
            device_id: req.device_id.clone(),
            device_name: req.device_name,
            platform: req.platform,
            session_token: session_token.clone(),
            paired_at: now,
            last_seen_at: now,
            ip_address: client_ip,
            is_revoked: false,
        };

        // Cache in memory
        self.devices
            .write()
            .unwrap()
            .insert(req.device_id.clone(), device.clone());

        // Persist to SQLite if enabled
        if let Some(ref conn_arc) = self.conn {
            let conn = conn_arc.lock().unwrap();
            let _ = conn.execute(
                "INSERT INTO paired_devices (device_id, device_name, platform, session_token, paired_at, last_seen_at, ip_address, is_revoked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(device_id) DO UPDATE SET
                    session_token = excluded.session_token,
                    last_seen_at = excluded.last_seen_at,
                    ip_address = excluded.ip_address,
                    is_revoked = 0;",
                params![
                    device.device_id,
                    device.device_name,
                    device.platform,
                    device.session_token,
                    device.paired_at,
                    device.last_seen_at,
                    device.ip_address,
                    0,
                ],
            );
        }

        Ok(DevicePairingResponse {
            status: "paired".to_string(),
            session_token,
            device_id: req.device_id,
            expires_at,
            paired_at: now,
        })
    }

    /// Validates session token in constant-time and returns paired device details
    pub fn verify_session_token(&self, token: &str) -> Result<PairedDevice, ProblemDetails> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 2 {
            return Err(ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/invalid-token",
                "Malformed Session Token",
                "The bearer pairing token format is invalid.",
            ));
        }

        let raw_payload = hex::decode(parts[0]).map_err(|_| {
            ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/invalid-token",
                "Malformed Session Token",
                "Failed to decode token payload.",
            )
        })?;
        let payload_str = String::from_utf8(raw_payload).map_err(|_| {
            ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/invalid-token",
                "Malformed Session Token",
                "Invalid payload character encoding.",
            )
        })?;

        let candidate_sig = hex::decode(parts[1]).map_err(|_| {
            ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/invalid-token",
                "Malformed Session Token",
                "Failed to decode token signature bytes.",
            )
        })?;

        if !TokenAuthority::verify_signature_constant_time(&self.secret, &payload_str, &candidate_sig) {
            return Err(ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/invalid-token",
                "Invalid Session Signature",
                "Token signature verification failed.",
            ));
        }

        // Parse payload format: session|<device_id>|<platform>|<paired_at>|<expires_at>
        let fields: Vec<&str> = payload_str.split('|').collect();
        if fields.len() != 5 || fields[0] != "session" {
            return Err(ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/invalid-token",
                "Invalid Session Payload",
                "Token payload contains unexpected fields.",
            ));
        }

        let device_id = fields[1];
        let expires_at: u64 = fields[4].parse().unwrap_or(0);
        let now = current_epoch_secs();

        if now > expires_at {
            return Err(ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/session-expired",
                "Device Session Expired",
                "Paired device session token has expired. Please re-pair device.",
            ));
        }

        let mut devices = self.devices.write().unwrap();
        let device = match devices.get_mut(device_id) {
            Some(d) => {
                if d.is_revoked {
                    return Err(ProblemDetails::new(
                        StatusCode::FORBIDDEN,
                        "https://aetherstream.dev/errors/device-revoked",
                        "Paired Device Revoked",
                        "This device's pairing authorization has been revoked by host.",
                    ));
                }
                d.last_seen_at = now;
                d.clone()
            }
            None => {
                return Err(ProblemDetails::new(
                    StatusCode::UNAUTHORIZED,
                    "https://aetherstream.dev/errors/unknown-device",
                    "Unregistered Device",
                    "Device record is not registered in pairing authority.",
                ));
            }
        };

        Ok(device)
    }

    /// Evaluates remote WAN sessions: local clients pass automatically; tunnel clients require paired session
    pub fn is_remote_authorized(
        &self,
        headers: &HeaderMap,
        ctx: &ClientNetworkContext,
    ) -> Result<Option<PairedDevice>, ProblemDetails> {
        if ctx.is_trusted_local() {
            return Ok(None);
        }

        // Remote tunnel client: Extract token from Authorization: Bearer <token> or X-AetherStream-Pairing-Token
        let token_opt = headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .or_else(|| {
                headers
                    .get("x-aetherstream-pairing-token")
                    .and_then(|h| h.to_str().ok())
            });

        match token_opt {
            Some(tok) => {
                let dev = self.verify_session_token(tok.trim())?;
                Ok(Some(dev))
            }
            None => Err(ProblemDetails::new(
                StatusCode::UNAUTHORIZED,
                "https://aetherstream.dev/errors/pairing-required",
                "Remote Pairing Required",
                "Remote sessions outside local subnet require device pairing. Please scan QR or enter pairing passkey.",
            )),
        }
    }

    pub fn list_paired_devices(&self) -> Vec<PairedDevice> {
        self.devices.read().unwrap().values().cloned().collect()
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<bool, PairingError> {
        let mut devices = self.devices.write().unwrap();
        if let Some(dev) = devices.get_mut(device_id) {
            dev.is_revoked = true;
            if let Some(ref conn_arc) = self.conn {
                let conn = conn_arc.lock().unwrap();
                let _ = conn.execute(
                    "UPDATE paired_devices SET is_revoked = 1 WHERE device_id = ?1;",
                    params![device_id],
                );
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// ----------------------------------------------------------------------------
// REST API Handlers & Router
// ----------------------------------------------------------------------------

/// POST /api/v1/tunnel/pair/invite: Generates a single-use passkey invitation
pub async fn create_invitation_handler(
    State(auth): State<Arc<DevicePairingAuthority>>,
    Json(payload): Json<Option<CreateInvitationRequest>>,
) -> impl IntoResponse {
    let (ttl, label) = payload
        .map(|p| (p.ttl_seconds, p.client_label))
        .unwrap_or((None, None));

    let (code, record) = auth.create_invitation(ttl, label.clone());
    let ttl_secs = record.expires_at.saturating_sub(record.created_at);

    (
        StatusCode::CREATED,
        Json(CreateInvitationResponse {
            passkey: code,
            expires_at: record.expires_at,
            ttl_seconds: ttl_secs,
            client_label: label,
        }),
    )
}

/// POST /api/v1/tunnel/pair: Consumes passkey and pairs remote client device
pub async fn pair_device_handler(
    State(auth): State<Arc<DevicePairingAuthority>>,
    Json(req): Json<DevicePairingRequest>,
) -> Response {
    match auth.pair_device(req, None) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(problem) => problem.into_response(),
    }
}

/// GET /api/v1/tunnel/pair/devices: Lists all paired devices
pub async fn list_devices_handler(
    State(auth): State<Arc<DevicePairingAuthority>>,
) -> impl IntoResponse {
    let devices = auth.list_paired_devices();
    Json(devices)
}

/// DELETE /api/v1/tunnel/pair/devices/:device_id: Revokes authorization for a device
pub async fn revoke_device_handler(
    State(auth): State<Arc<DevicePairingAuthority>>,
    AxPath(device_id): AxPath<String>,
) -> Response {
    match auth.revoke_device(&device_id) {
        Ok(true) => (StatusCode::NO_CONTENT).into_response(),
        Ok(false) => ProblemDetails::new(
            StatusCode::NOT_FOUND,
            "https://aetherstream.dev/errors/device-not-found",
            "Device Not Found",
            format!("No device with ID '{}' exists.", device_id),
        )
        .into_response(),
        Err(e) => ProblemDetails::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://aetherstream.dev/errors/db-error",
            "Database Error",
            e.to_string(),
        )
        .into_response(),
    }
}

/// Standalone Axum Router for `/api/v1/tunnel/pair/*`
pub fn tunnel_pairing_router(authority: Arc<DevicePairingAuthority>) -> Router {
    Router::new()
        .route("/api/v1/tunnel/pair/invite", post(create_invitation_handler))
        .route("/api/v1/tunnel/pair", post(pair_device_handler))
        .route("/api/v1/tunnel/pair/devices", get(list_devices_handler))
        .route(
            "/api/v1/tunnel/pair/devices/:device_id",
            delete(revoke_device_handler),
        )
        .with_state(authority)
}
