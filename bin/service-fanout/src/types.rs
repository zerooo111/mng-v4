/// Core types shared across all fanout modules.
///
/// `EventEnvelope` is a transparent wrapper around `serde_json::Value` so the
/// fanout service is completely decoupled from the execution engine's concrete
/// struct definitions.  We only need to inspect three fields at the fanout
/// boundary (`event_type`, `market`, `sequence`); everything else is forwarded
/// verbatim to subscribers.
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EventEnvelope
// ---------------------------------------------------------------------------

/// Opaque wrapper around the raw JSON body POSTed by the execution engine to
/// `/ingest`.  Storing the value as `serde_json::Value` means:
///
/// - No coupling to the engine's struct layout.
/// - Unknown / future event types are handled gracefully.
/// - Subscribers receive the original bytes with zero re-serialisation cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope(pub serde_json::Value);

impl EventEnvelope {
    /// The `event_type` field: `"relay_intent_accepted"` or
    /// `"relay_intent_status"`.
    pub fn event_type(&self) -> Option<&str> {
        self.0.get("event_type")?.as_str()
    }

    /// Routing key — the `market` pubkey string present in every event the
    /// engine emits.  Returns `None` only if the field is absent (malformed
    /// event).
    pub fn market(&self) -> Option<&str> {
        self.0.get("market")?.as_str()
    }

    /// Monotonic sequence number for gap detection / snapshot alignment.
    /// The engine encodes this as a decimal string (`"sequence": "12345"`).
    pub fn sequence(&self) -> Option<u64> {
        self.0.get("sequence")?.as_str()?.parse().ok()
    }

    /// Event timestamp in milliseconds since UNIX epoch.
    pub fn ts_ms(&self) -> Option<u64> {
        self.0.get("ts_ms")?.as_u64()
    }
}

// ---------------------------------------------------------------------------
// MarketSnapshot
// ---------------------------------------------------------------------------

const SNAPSHOT_MAX_EVENTS_DEFAULT: usize = 256;

/// Per-market in-memory snapshot updated on every ingest.
///
/// Serves two purposes:
/// 1. New subscribers call `GET /snapshot/{market}` to get `last_sequence` +
///    recent events, then open `GET /stream/{market}?from={last_sequence}`.
///    The broadcast channel buffer (4096) covers the overlap window.
/// 2. Simple observability — how many events have been processed per market.
#[derive(Debug, Clone, Serialize)]
pub struct MarketSnapshot {
    pub market: String,
    pub last_sequence: u64,
    pub updated_at_ms: u64,
    pub event_count: u64,
    /// Ring buffer of the last `snapshot_max_events` raw event values.
    /// Clients use this to replay recent history on reconnect.
    pub recent_events: VecDeque<serde_json::Value>,
    #[serde(skip)]
    pub max_events: usize,
}

impl MarketSnapshot {
    pub fn new(market: String) -> Self {
        Self {
            market,
            last_sequence: 0,
            updated_at_ms: 0,
            event_count: 0,
            recent_events: VecDeque::new(),
            max_events: SNAPSHOT_MAX_EVENTS_DEFAULT,
        }
    }

    /// Apply a new event to the snapshot, updating the ring buffer and
    /// sequence watermark.
    pub fn apply(&mut self, ev: &EventEnvelope) {
        self.event_count += 1;
        self.updated_at_ms = ev.ts_ms().unwrap_or_else(now_ms);

        if let Some(seq) = ev.sequence() {
            if seq > self.last_sequence {
                self.last_sequence = seq;
            }
        }

        // Maintain bounded ring buffer
        if self.recent_events.len() >= self.max_events {
            self.recent_events.pop_front();
        }
        self.recent_events.push_back(ev.0.clone());
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Runtime configuration loaded from environment variables at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// `FANOUT_BIND_ADDR` — default `0.0.0.0:8080`
    pub bind_addr: SocketAddr,

    /// `FANOUT_CHANNEL_CAPACITY` — broadcast ring size per market, default 4096
    pub channel_capacity: usize,

    /// `FANOUT_SNAPSHOT_MAX_EVENTS` — recent-event ring size per snapshot, default 256
    pub snapshot_max_events: usize,

    /// `FANOUT_MAX_CONNECTIONS` — global SSE connection cap, default 2000
    pub max_connections: usize,

    /// `FANOUT_MAX_CONNECTIONS_PER_USER` — per-user cap, default 10
    pub max_connections_per_user: u32,

    /// `FANOUT_AUTH_DISABLED=true` — skip auth entirely (local dev only)
    pub auth_disabled: bool,

    /// `FANOUT_API_KEYS` — comma-separated `key:user_id:tier` entries.
    /// Short form `key` is accepted; user_id defaults to the key itself,
    /// tier defaults to `free`.
    /// Example: `sk-abc:alice:pro,sk-def:bob:free,sk-anon`
    pub api_keys: HashMap<String, ApiKeyEntry>,

    /// `FANOUT_JWT_SECRET` — HMAC-SHA256 secret for JWT verification.
    /// If absent, JWT auth is disabled (API key only).
    pub jwt_secret: Option<String>,

    /// `FANOUT_INGEST_SECRET` — if set, POST /ingest must supply
    /// `X-Ingest-Secret: <value>`.  If unset, /ingest is open (trust the
    /// network boundary).
    pub ingest_secret: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr: SocketAddr = std::env::var("FANOUT_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()?;

        let channel_capacity = parse_env_usize("FANOUT_CHANNEL_CAPACITY", 4096)?;
        let snapshot_max_events = parse_env_usize("FANOUT_SNAPSHOT_MAX_EVENTS", 256)?;
        let max_connections = parse_env_usize("FANOUT_MAX_CONNECTIONS", 2000)?;
        let max_connections_per_user = parse_env_u32("FANOUT_MAX_CONNECTIONS_PER_USER", 10)?;

        let auth_disabled = std::env::var("FANOUT_AUTH_DISABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let api_keys = parse_api_keys(&std::env::var("FANOUT_API_KEYS").unwrap_or_default());

        let jwt_secret = std::env::var("FANOUT_JWT_SECRET").ok().filter(|s| !s.is_empty());
        let ingest_secret = std::env::var("FANOUT_INGEST_SECRET").ok().filter(|s| !s.is_empty());

        Ok(Self {
            bind_addr,
            channel_capacity,
            snapshot_max_events,
            max_connections,
            max_connections_per_user,
            auth_disabled,
            api_keys,
            jwt_secret,
            ingest_secret,
        })
    }
}

// ---------------------------------------------------------------------------
// Tier + ApiKeyEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Free,
    Pro,
    Enterprise,
}

impl Tier {
    fn from_str(s: &str) -> Self {
        match s {
            "pro" => Tier::Pro,
            "enterprise" => Tier::Enterprise,
            _ => Tier::Free,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    pub user_id: String,
    pub tier: Tier,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_env_usize(name: &str, default: usize) -> anyhow::Result<usize> {
    match std::env::var(name) {
        Ok(v) => Ok(v.parse()?),
        Err(_) => Ok(default),
    }
}

fn parse_env_u32(name: &str, default: u32) -> anyhow::Result<u32> {
    match std::env::var(name) {
        Ok(v) => Ok(v.parse()?),
        Err(_) => Ok(default),
    }
}

/// Parse `FANOUT_API_KEYS` value.
/// Format: `key[:user_id[:tier]]` entries separated by commas.
fn parse_api_keys(raw: &str) -> HashMap<String, ApiKeyEntry> {
    let mut map = HashMap::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let parts: Vec<&str> = entry.splitn(3, ':').collect();
        let key = parts[0].to_string();
        let user_id = parts.get(1).copied().unwrap_or(parts[0]).to_string();
        let tier = parts.get(2).copied().map(Tier::from_str).unwrap_or(Tier::Free);
        map.insert(key, ApiKeyEntry { user_id, tier });
    }
    map
}
