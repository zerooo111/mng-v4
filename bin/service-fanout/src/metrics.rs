/// Prometheus-format metrics with plain atomics — no external registry crate.
///
/// All hot-path increments use `Relaxed` ordering (counter-only, no
/// synchronisation required).  The `/metrics` handler serialises them
/// into OpenMetrics text format on each scrape.
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::app::AppState;

// ---------------------------------------------------------------------------
// Metrics struct
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Metrics {
    // --- ingest side ---
    /// Total events received at POST /ingest.
    pub events_ingested: AtomicU64,
    /// Events rejected (unknown market / deserialization error).
    pub ingest_errors: AtomicU64,
    /// Events dropped because no subscriber was listening at the time.
    pub events_no_subscriber: AtomicU64,

    // --- subscriber side ---
    /// Current number of open SSE connections.
    pub active_subscribers: AtomicI64,
    /// Times a subscriber was notified it had fallen behind the ring buffer.
    pub lagged_events: AtomicU64,
    /// Connection attempts rejected due to per-user or global limit.
    pub connections_rejected: AtomicU64,

    // --- auth side ---
    /// Requests that failed authentication.
    pub auth_failures: AtomicU64,

    // --- redis side (only meaningful when FANOUT_REDIS_URL is set) ---
    /// Events successfully published to Redis.
    pub redis_published: AtomicU64,
    /// Failed Redis PUBLISH calls (connection error / timeout).
    pub redis_publish_errors: AtomicU64,
    /// Events received from Redis subscriber and dispatched locally.
    pub redis_subscriber_dispatched: AtomicU64,
    /// Events successfully forwarded upstream to the harness ingest path.
    pub upstream_forwarded: AtomicU64,
    /// Failed upstream forward attempts (HTTP error / non-2xx).
    pub upstream_forward_errors: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    // Convenience increment helpers (avoid scattering `fetch_add` everywhere).
    pub fn inc_ingested(&self) {
        self.events_ingested.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_ingest_error(&self) {
        self.ingest_errors.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_no_subscriber(&self) {
        self.events_no_subscriber.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_subscriber(&self) {
        self.active_subscribers.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_subscriber(&self) {
        self.active_subscribers.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn inc_lagged(&self) {
        self.lagged_events.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_connections_rejected(&self) {
        self.connections_rejected.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_auth_failure(&self) {
        self.auth_failures.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_redis_published(&self) {
        self.redis_published.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_redis_publish_error(&self) {
        self.redis_publish_errors.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_redis_subscriber_dispatched(&self) {
        self.redis_subscriber_dispatched
            .fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_upstream_forwarded(&self) {
        self.upstream_forwarded.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_upstream_forward_error(&self) {
        self.upstream_forward_errors.fetch_add(1, Ordering::Relaxed);
    }

    // -----------------------------------------------------------------------
    // Render
    // -----------------------------------------------------------------------

    /// Render all metrics in OpenMetrics / Prometheus text format.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1024);

        write_counter(
            &mut out,
            "fanout_events_ingested_total",
            "Total events received at POST /ingest",
            self.events_ingested.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_ingest_errors_total",
            "Events rejected at ingest (bad JSON, missing market field)",
            self.ingest_errors.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_events_no_subscriber_total",
            "Events broadcast to zero active subscribers",
            self.events_no_subscriber.load(Ordering::Relaxed),
        );
        write_gauge(
            &mut out,
            "fanout_active_subscribers",
            "Current number of open SSE connections",
            self.active_subscribers.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_lagged_events_total",
            "Times a subscriber received a Lagged error from the broadcast ring",
            self.lagged_events.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_connections_rejected_total",
            "SSE connection attempts rejected by per-user or global limit",
            self.connections_rejected.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_auth_failures_total",
            "Requests that failed JWT or API-key authentication",
            self.auth_failures.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_redis_published_total",
            "Events successfully published to Redis (multi-instance mode only)",
            self.redis_published.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_redis_publish_errors_total",
            "Failed Redis PUBLISH calls (connection error / timeout)",
            self.redis_publish_errors.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_redis_subscriber_dispatched_total",
            "Events received from Redis subscriber and dispatched locally",
            self.redis_subscriber_dispatched.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_upstream_forwarded_total",
            "Events successfully forwarded to the upstream ingest endpoint",
            self.upstream_forwarded.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "fanout_upstream_forward_errors_total",
            "Failed forward attempts to the upstream ingest endpoint",
            self.upstream_forward_errors.load(Ordering::Relaxed),
        );

        out
    }
}

fn write_counter(buf: &mut String, name: &str, help: &str, value: u64) {
    buf.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
    ));
}

fn write_gauge(buf: &mut String, name: &str, help: &str, value: i64) {
    buf.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
    ));
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /metrics` — Prometheus scrape endpoint.  No auth required (scraper
/// runs on the internal network and cannot learn user data from counters).
pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}
