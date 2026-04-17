/// Authentication and connection control.
///
/// Phase 2 implements two auth mechanisms, both via `Authorization: Bearer
/// <token>`:
///
/// 1. **API key** — static pre-shared keys loaded from `FANOUT_API_KEYS`.
///    O(1) `HashMap` lookup.
///
/// 2. **JWT (HS256)** — signed with `FANOUT_JWT_SECRET`.  Manual HS256
///    verification using `hmac` + `sha2` + `base64` — avoids the transitive
///    `time` / `time-macros` dependency that `jsonwebtoken` pulls in (which
///    requires Rust edition 2024, incompatible with our 1.70 toolchain).
///
/// The `AuthClaims` extractor is the gatekeeper for SSE and snapshot routes.
/// It also enforces connection limits:
///
/// - Per-user: `DashMap<user_id, AtomicU32>` tracks live SSE connections.
/// - Global: `tokio::sync::Semaphore` caps total concurrent connections.
///
/// A `ConnectionGuard` (RAII) decrements the per-user counter and releases the
/// semaphore permit when the SSE stream is dropped (client disconnect).
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use base64::Engine as _;
use dashmap::DashMap;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::app::AppState;
use crate::types::{now_ms, Tier};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Claims / identity
// ---------------------------------------------------------------------------

/// Verified identity attached to every authenticated request.
#[derive(Debug, Clone)]
pub struct AuthClaims {
    pub user_id: String,
    pub tier: Tier,
}

#[derive(Debug, Serialize, Deserialize)]
struct JwtPayload {
    /// Subject — used as user_id.
    sub: String,
    /// Expiry as UNIX timestamp (seconds).
    exp: u64,
    /// Optional tier claim.
    #[serde(default)]
    tier: Option<String>,
}

// ---------------------------------------------------------------------------
// FromRequestParts — axum extractor
// ---------------------------------------------------------------------------

#[async_trait]
impl FromRequestParts<AppState> for AuthClaims {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Fast-path: auth disabled (dev / internal deployments).
        if state.config.auth_disabled {
            return Ok(AuthClaims {
                user_id: "anonymous".to_string(),
                tier: Tier::Free,
            });
        }

        let token = extract_bearer(&parts.headers).ok_or((
            StatusCode::UNAUTHORIZED,
            "missing or malformed Authorization: Bearer header",
        ))?;

        // Try API key first (cheaper — O(1) HashMap lookup).
        if let Some(entry) = state.config.api_keys.get(token) {
            return Ok(AuthClaims {
                user_id: entry.user_id.clone(),
                tier: entry.tier,
            });
        }

        // Fall through to JWT if a secret is configured.
        if let Some(secret) = &state.config.jwt_secret {
            return verify_jwt_hs256(token, secret).map_err(|_| {
                state.metrics.inc_auth_failure();
                (StatusCode::UNAUTHORIZED, "invalid or expired JWT")
            });
        }

        state.metrics.inc_auth_failure();
        Err((StatusCode::UNAUTHORIZED, "invalid API key"))
    }
}

// ---------------------------------------------------------------------------
// JWT HS256 — manual implementation
// ---------------------------------------------------------------------------
//
// We implement only the subset required for HS256 / compact serialisation:
//   header.payload.signature  (all parts base64url-encoded, no padding)
//
// Steps:
//   1. Split into three parts.
//   2. Recompute HMAC-SHA256 over "header.payload" using the shared secret.
//   3. Constant-time compare with the provided signature.
//   4. Decode and deserialise the payload.
//   5. Validate `exp`.

fn verify_jwt_hs256(token: &str, secret: &str) -> Result<AuthClaims, &'static str> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() != 3 {
        return Err("invalid JWT structure");
    }
    let (header_b64, payload_b64, sig_b64) = (parts[0], parts[1], parts[2]);

    // --- Verify signature ---
    let signing_input = format!("{header_b64}.{payload_b64}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| "invalid HMAC key")?;
    mac.update(signing_input.as_bytes());
    let expected = mac.finalize().into_bytes();

    let provided = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| "invalid base64 in signature")?;

    // Constant-time comparison to prevent timing attacks.
    if !constant_time_eq(&expected, &provided) {
        return Err("signature mismatch");
    }

    // --- Decode payload ---
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| "invalid base64 in payload")?;

    let payload: JwtPayload =
        serde_json::from_slice(&payload_bytes).map_err(|_| "invalid payload JSON")?;

    // --- Validate expiry ---
    let now_secs = now_ms() / 1000;
    if payload.exp < now_secs {
        return Err("token expired");
    }

    let tier = payload
        .tier
        .as_deref()
        .map(tier_from_str)
        .unwrap_or(Tier::Free);

    Ok(AuthClaims {
        user_id: payload.sub,
        tier,
    })
}

/// Constant-time byte slice equality to avoid timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // XOR all bytes; any difference sets a bit.
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn tier_from_str(s: &str) -> Tier {
    match s {
        "pro" => Tier::Pro,
        "enterprise" => Tier::Enterprise,
        _ => Tier::Free,
    }
}

// ---------------------------------------------------------------------------
// Bearer token extraction
// ---------------------------------------------------------------------------

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// Per-user and global connection tracking shared across all handler tasks.
pub struct ConnectionState {
    /// Number of live SSE connections keyed by `user_id`.
    pub user_connections: Arc<DashMap<String, AtomicU32>>,
    pub max_per_user: u32,
    /// Global cap on concurrent SSE streams.
    pub semaphore: Arc<Semaphore>,
}

impl ConnectionState {
    pub fn new(max_connections: usize, max_per_user: u32) -> Arc<Self> {
        Arc::new(Self {
            user_connections: Arc::new(DashMap::new()),
            max_per_user,
            semaphore: Arc::new(Semaphore::new(max_connections)),
        })
    }

    /// Try to acquire a connection slot for `user_id`.
    ///
    /// Returns a `ConnectionGuard` on success.  Its `Drop` impl releases the
    /// slot and semaphore permit automatically when the SSE task ends.
    pub async fn acquire(
        self: &Arc<Self>,
        user_id: String,
        metrics: &Arc<crate::metrics::Metrics>,
    ) -> Result<ConnectionGuard, (StatusCode, &'static str)> {
        // --- Global semaphore ---
        let permit = Arc::clone(&self.semaphore)
            .try_acquire_owned()
            .map_err(|_| {
                metrics.inc_connections_rejected();
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "server connection limit reached",
                )
            })?;

        // --- Per-user counter ---
        // `fetch_add` returns the PREVIOUS value; we accept if prev < max.
        let counter = self
            .user_connections
            .entry(user_id.clone())
            .or_insert_with(|| AtomicU32::new(0));

        let prev = counter.fetch_add(1, Ordering::Relaxed);
        if prev >= self.max_per_user {
            counter.fetch_sub(1, Ordering::Relaxed);
            metrics.inc_connections_rejected();
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "per-user connection limit reached",
            ));
        }

        metrics.inc_subscriber();

        Ok(ConnectionGuard {
            user_id,
            user_connections: Arc::clone(&self.user_connections),
            _permit: permit,
            metrics: Arc::clone(metrics),
        })
    }
}

// ---------------------------------------------------------------------------
// ConnectionGuard — RAII cleanup
// ---------------------------------------------------------------------------

/// Holds the per-user connection slot and the global semaphore permit.
/// Dropping this struct (client disconnect / stream end) frees both.
pub struct ConnectionGuard {
    user_id: String,
    user_connections: Arc<DashMap<String, AtomicU32>>,
    _permit: OwnedSemaphorePermit,
    metrics: Arc<crate::metrics::Metrics>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Some(counter) = self.user_connections.get(&self.user_id) {
            counter.fetch_sub(1, Ordering::Relaxed);
        }
        self.metrics.dec_subscriber();
    }
}
