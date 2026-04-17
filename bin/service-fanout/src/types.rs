/// Core types shared across all fanout modules.
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EventEnvelope
// ---------------------------------------------------------------------------

/// Opaque event wrapper around the raw JSON payload emitted by the execution
/// engine. The fanout service precomputes the serialized payload once and
/// assigns its own per-market cursor so reconnect semantics do not depend on
/// the upstream schema always carrying a `sequence`.
#[derive(Debug, Clone)]
pub struct EventEnvelope {
    value: serde_json::Value,
    payload: Arc<str>,
    market: Option<String>,
    sequence: Option<u64>,
    ts_ms: Option<u64>,
    cursor: u64,
}

impl EventEnvelope {
    pub fn from_value(value: serde_json::Value) -> anyhow::Result<Self> {
        let payload = serde_json::to_string(&value)?;
        Ok(Self::from_value_and_payload(value, payload))
    }

    pub fn from_payload(payload: String) -> anyhow::Result<Self> {
        let value: serde_json::Value = serde_json::from_str(&payload)?;
        Ok(Self::from_value_and_payload(value, payload))
    }

    fn from_value_and_payload(value: serde_json::Value, payload: String) -> Self {
        let market = value
            .get("market")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let sequence = parse_optional_u64(value.get("sequence"));
        let ts_ms = parse_optional_u64(value.get("ts_ms"));
        Self {
            value,
            payload: Arc::<str>::from(payload),
            market,
            sequence,
            ts_ms,
            cursor: 0,
        }
    }

    pub fn event_type(&self) -> Option<&str> {
        self.value.get("event_type")?.as_str()
    }

    pub fn market(&self) -> Option<&str> {
        self.market.as_deref()
    }

    pub fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    pub fn ts_ms(&self) -> Option<u64> {
        self.ts_ms
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub fn raw_value(&self) -> &serde_json::Value {
        &self.value
    }

    pub fn with_cursor(mut self, cursor: u64) -> Self {
        self.cursor = cursor;
        self
    }
}

// ---------------------------------------------------------------------------
// MarketSnapshot
// ---------------------------------------------------------------------------

const SNAPSHOT_MAX_EVENTS_DEFAULT: usize = 256;
const REDIS_STREAM_KEY_DEFAULT: &str = "fanout:events";
const REDIS_STREAM_MAXLEN_DEFAULT: usize = 100_000;
const REDIS_STREAM_BLOCK_MS_DEFAULT: usize = 1_000;
const UPSTREAM_TIMEOUT_MS_DEFAULT: u64 = 1_000;

#[derive(Debug, Clone)]
pub struct RecentEvent {
    pub cursor: u64,
    pub sequence: Option<u64>,
    pub payload: Arc<str>,
    pub value: serde_json::Value,
}

impl RecentEvent {
    fn from_envelope(ev: &EventEnvelope) -> Self {
        Self {
            cursor: ev.cursor(),
            sequence: ev.sequence(),
            payload: Arc::clone(&ev.payload),
            value: ev.raw_value().clone(),
        }
    }
}

/// Per-market in-memory snapshot updated on every ingest.
#[derive(Debug, Clone)]
pub struct MarketSnapshot {
    pub market: String,
    pub last_sequence: u64,
    pub last_cursor: u64,
    pub updated_at_ms: u64,
    pub event_count: u64,
    pub recent_events: VecDeque<RecentEvent>,
    pub max_events: usize,
}

impl MarketSnapshot {
    pub fn new(market: String) -> Self {
        Self {
            market,
            last_sequence: 0,
            last_cursor: 0,
            updated_at_ms: 0,
            event_count: 0,
            recent_events: VecDeque::new(),
            max_events: SNAPSHOT_MAX_EVENTS_DEFAULT,
        }
    }

    /// Apply a new event to the snapshot, assign the next per-market cursor,
    /// and retain a bounded replay ring for reconnects.
    pub fn apply(&mut self, ev: EventEnvelope) -> Arc<EventEnvelope> {
        self.event_count += 1;
        self.updated_at_ms = ev.ts_ms().unwrap_or_else(now_ms);
        self.last_cursor += 1;

        if let Some(seq) = ev.sequence() {
            if seq > self.last_sequence {
                self.last_sequence = seq;
            }
        }

        let ev = Arc::new(ev.with_cursor(self.last_cursor));

        if self.recent_events.len() >= self.max_events {
            self.recent_events.pop_front();
        }
        self.recent_events
            .push_back(RecentEvent::from_envelope(&ev));

        ev
    }

    pub fn first_cursor(&self) -> Option<u64> {
        self.recent_events.front().map(|ev| ev.cursor)
    }

    /// Replay the subset of recent events after a known client watermark.
    ///
    /// `cursor` is the precise watermark exposed by `/snapshot/{market}` and
    /// should be preferred by new clients. `sequence` remains as a best-effort
    /// compatibility path for older clients.
    pub fn replay_since(&self, cursor: Option<u64>, sequence: Option<u64>) -> Vec<RecentEvent> {
        if let Some(cursor) = cursor {
            return self
                .recent_events
                .iter()
                .filter(|ev| ev.cursor > cursor)
                .cloned()
                .collect();
        }

        let Some(sequence) = sequence else {
            return Vec::new();
        };

        self.recent_events
            .iter()
            .filter(|ev| ev.sequence.map(|seq| seq > sequence).unwrap_or(false))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotResponse {
    pub market: String,
    pub last_sequence: u64,
    pub last_cursor: u64,
    pub first_cursor: Option<u64>,
    pub updated_at_ms: u64,
    pub event_count: u64,
    pub recent_events: Vec<serde_json::Value>,
}

impl From<&MarketSnapshot> for SnapshotResponse {
    fn from(snapshot: &MarketSnapshot) -> Self {
        Self {
            market: snapshot.market.clone(),
            last_sequence: snapshot.last_sequence,
            last_cursor: snapshot.last_cursor,
            first_cursor: snapshot.first_cursor(),
            updated_at_ms: snapshot.updated_at_ms,
            event_count: snapshot.event_count,
            recent_events: snapshot
                .recent_events
                .iter()
                .map(|event| event.value.clone())
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Runtime configuration loaded from environment variables at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// `FANOUT_BIND_ADDR` — default `0.0.0.0:9094`
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
    pub api_keys: HashMap<String, ApiKeyEntry>,
    /// `FANOUT_JWT_SECRET` — HMAC-SHA256 secret for JWT verification.
    pub jwt_secret: Option<String>,
    /// `FANOUT_INGEST_SECRET` — optional `X-Ingest-Secret` value.
    pub ingest_secret: Option<String>,
    /// `FANOUT_REDIS_URL` — Redis connection URL for multi-instance mode.
    pub redis_url: Option<String>,
    /// `FANOUT_REDIS_STREAM_KEY` — Redis stream name for durable fanout.
    pub redis_stream_key: String,
    /// `FANOUT_REDIS_STREAM_MAXLEN` — approximate cap on retained events.
    pub redis_stream_maxlen: usize,
    /// `FANOUT_REDIS_STREAM_BLOCK_MS` — subscriber XREAD block timeout.
    pub redis_stream_block_ms: usize,
    /// `FANOUT_UPSTREAM_INGEST_URL` — optional upstream relay-intent sink.
    pub upstream_ingest_url: Option<String>,
    /// `FANOUT_UPSTREAM_AUTH_TOKEN` — optional bearer token for upstream ingest.
    pub upstream_auth_token: Option<String>,
    /// `FANOUT_UPSTREAM_TIMEOUT_MS` — HTTP timeout for upstream forwarding.
    pub upstream_timeout_ms: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr: SocketAddr = std::env::var("FANOUT_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:9094".into())
            .parse()?;

        let channel_capacity = parse_env_usize("FANOUT_CHANNEL_CAPACITY", 4096)?;
        let snapshot_max_events = parse_env_usize("FANOUT_SNAPSHOT_MAX_EVENTS", 256)?;
        let max_connections = parse_env_usize("FANOUT_MAX_CONNECTIONS", 2000)?;
        let max_connections_per_user = parse_env_u32("FANOUT_MAX_CONNECTIONS_PER_USER", 10)?;

        let auth_disabled = std::env::var("FANOUT_AUTH_DISABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let api_keys = parse_api_keys(&std::env::var("FANOUT_API_KEYS").unwrap_or_default());
        let jwt_secret = std::env::var("FANOUT_JWT_SECRET")
            .ok()
            .filter(|s| !s.is_empty());
        let ingest_secret = std::env::var("FANOUT_INGEST_SECRET")
            .ok()
            .filter(|s| !s.is_empty());
        let redis_url = std::env::var("FANOUT_REDIS_URL")
            .ok()
            .filter(|s| !s.is_empty());
        let redis_stream_key = std::env::var("FANOUT_REDIS_STREAM_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| REDIS_STREAM_KEY_DEFAULT.to_string());
        let redis_stream_maxlen =
            parse_env_usize("FANOUT_REDIS_STREAM_MAXLEN", REDIS_STREAM_MAXLEN_DEFAULT)?;
        let redis_stream_block_ms = parse_env_usize(
            "FANOUT_REDIS_STREAM_BLOCK_MS",
            REDIS_STREAM_BLOCK_MS_DEFAULT,
        )?;
        let upstream_ingest_url = std::env::var("FANOUT_UPSTREAM_INGEST_URL")
            .ok()
            .filter(|s| !s.is_empty());
        let upstream_auth_token = std::env::var("FANOUT_UPSTREAM_AUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        let upstream_timeout_ms =
            parse_env_u64("FANOUT_UPSTREAM_TIMEOUT_MS", UPSTREAM_TIMEOUT_MS_DEFAULT)?;

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
            redis_url,
            redis_stream_key,
            redis_stream_maxlen,
            redis_stream_block_ms,
            upstream_ingest_url,
            upstream_auth_token,
            upstream_timeout_ms,
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

fn parse_env_u64(name: &str, default: u64) -> anyhow::Result<u64> {
    match std::env::var(name) {
        Ok(v) => Ok(v.parse()?),
        Err(_) => Ok(default),
    }
}

fn parse_optional_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    match value {
        Some(serde_json::Value::String(v)) => v.parse().ok(),
        Some(serde_json::Value::Number(v)) => v.as_u64(),
        _ => None,
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
        let tier = parts
            .get(2)
            .copied()
            .map(Tier::from_str)
            .unwrap_or(Tier::Free);
        map.insert(key, ApiKeyEntry { user_id, tier });
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_envelope_parses_string_and_numeric_fields() {
        let string_ev = EventEnvelope::from_value(serde_json::json!({
            "market": "1",
            "sequence": "42",
            "ts_ms": 1234
        }))
        .unwrap();
        assert_eq!(string_ev.market(), Some("1"));
        assert_eq!(string_ev.sequence(), Some(42));
        assert_eq!(string_ev.ts_ms(), Some(1234));

        let numeric_ev = EventEnvelope::from_value(serde_json::json!({
            "market": "2",
            "sequence": 43,
            "ts_ms": "999"
        }))
        .unwrap();
        assert_eq!(numeric_ev.sequence(), Some(43));
        assert_eq!(numeric_ev.ts_ms(), Some(999));
    }

    #[test]
    fn snapshot_tracks_cursor_and_replays_since_cursor_or_sequence() {
        let mut snapshot = MarketSnapshot::new("0".to_string());
        snapshot.max_events = 3;

        let ev1 = snapshot.apply(
            EventEnvelope::from_value(serde_json::json!({
                "market": "0",
                "sequence": "10",
                "ts_ms": 1
            }))
            .unwrap(),
        );
        let ev2 = snapshot.apply(
            EventEnvelope::from_value(serde_json::json!({
                "market": "0",
                "sequence": null,
                "ts_ms": 2
            }))
            .unwrap(),
        );
        let ev3 = snapshot.apply(
            EventEnvelope::from_value(serde_json::json!({
                "market": "0",
                "sequence": 11,
                "ts_ms": 3
            }))
            .unwrap(),
        );

        assert_eq!(ev1.cursor(), 1);
        assert_eq!(ev2.cursor(), 2);
        assert_eq!(ev3.cursor(), 3);
        assert_eq!(snapshot.last_sequence, 11);
        assert_eq!(snapshot.last_cursor, 3);

        let replay_by_cursor = snapshot.replay_since(Some(1), None);
        assert_eq!(replay_by_cursor.len(), 2);
        assert_eq!(replay_by_cursor[0].cursor, 2);
        assert_eq!(replay_by_cursor[1].cursor, 3);

        let replay_by_sequence = snapshot.replay_since(None, Some(10));
        assert_eq!(replay_by_sequence.len(), 1);
        assert_eq!(replay_by_sequence[0].sequence, Some(11));

        assert!(snapshot.replay_since(None, None).is_empty());
    }
}
