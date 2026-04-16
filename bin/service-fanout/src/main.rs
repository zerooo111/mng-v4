/// service-fanout — real-time event fanout for the CTM execution engine.
///
/// Receives events from the execution engine via `POST /ingest` and fans them
/// out to 100-1000+ concurrent authenticated SSE subscribers without touching
/// the engine's hot path.
///
/// See `fanout.md` in the repo root for the full architectural rationale.
///
/// ## Environment variables
///
/// | Variable                          | Default        | Description                        |
/// |-----------------------------------|----------------|------------------------------------|
/// | `FANOUT_BIND_ADDR`                | `0.0.0.0:8080` | TCP listen address                 |
/// | `FANOUT_CHANNEL_CAPACITY`         | `4096`         | Broadcast ring size per market     |
/// | `FANOUT_SNAPSHOT_MAX_EVENTS`      | `256`          | Recent-event ring per snapshot     |
/// | `FANOUT_MAX_CONNECTIONS`          | `2000`         | Global SSE connection cap          |
/// | `FANOUT_MAX_CONNECTIONS_PER_USER` | `10`           | Per-user SSE connection cap        |
/// | `FANOUT_AUTH_DISABLED`            | `false`        | Skip auth (local dev only)         |
/// | `FANOUT_API_KEYS`                 | *(none)*       | `key[:user[:tier]],...`            |
/// | `FANOUT_JWT_SECRET`               | *(none)*       | HMAC-SHA256 secret for JWT auth    |
/// | `FANOUT_INGEST_SECRET`            | *(none)*       | `X-Ingest-Secret` header value     |
/// | `RUST_LOG`                        | `info`         | Tracing filter                     |

mod app;
mod auth;
mod channels;
mod ingest;
mod metrics;
mod snapshot;
mod sse;
mod types;

use axum::{
    routing::{get, post},
    Router,
};
use tracing::info;
use tracing_subscriber::{fmt, EnvFilter};

use app::AppState;
use types::Config;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

fn build_router(state: AppState) -> Router {
    Router::new()
        // ── Ingest (called by execution engine, no auth) ──────────────────
        .route("/ingest", post(ingest::ingest_handler))
        .route("/ingest/batch", post(ingest::ingest_batch_handler))
        // ── SSE stream (authenticated) ────────────────────────────────────
        .route("/stream/:market", get(sse::sse_handler))
        // ── Snapshot / discovery (authenticated) ─────────────────────────
        .route("/snapshot/:market", get(snapshot::snapshot_handler))
        .route("/markets", get(snapshot::markets_handler))
        // ── Observability (no auth) ───────────────────────────────────────
        .route("/healthz", get(healthz_handler))
        .route("/metrics", get(metrics::metrics_handler))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

async fn healthz_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    // Tracing — mirrors execution engine style: RUST_LOG controls the filter.
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let config = Config::from_env()?;

    info!(
        bind_addr = %config.bind_addr,
        channel_capacity = config.channel_capacity,
        max_connections = config.max_connections,
        max_per_user = config.max_connections_per_user,
        auth_disabled = config.auth_disabled,
        api_key_count = config.api_keys.len(),
        jwt_auth = config.jwt_secret.is_some(),
        ingest_secret = config.ingest_secret.is_some(),
        "service-fanout starting"
    );

    let state = AppState::new(config);
    let bind_addr = state.config.bind_addr;
    let router = build_router(state);

    // Multi-thread runtime with 8 MB stack — mirrors the execution engine
    // runtime configuration.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()?
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(bind_addr).await?;
            info!("listening on {bind_addr}");
            axum::Server::from_tcp(listener.into_std()?)?
                .serve(router.into_make_service())
                .await?;
            Ok::<_, anyhow::Error>(())
        })
}
