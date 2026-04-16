/// GET /snapshot/{market} — point-in-time market state for SSE onboarding.
///
/// Clients call this before opening the SSE stream to get:
/// - `last_sequence` — the sequence watermark to pass as `?from=N` to
///   `/stream/{market}` so they don't replay already-seen events.
/// - `recent_events` — the last ≤256 events, for immediate display and
///   offline replay without needing a separate history API.
///
/// The read lock on `RwLock<MarketSnapshot>` is held only for the duration of
/// a `clone()` — microseconds — and is never held across any `.await`.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use crate::app::AppState;
use crate::auth::AuthClaims;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn snapshot_handler(
    Path(market): Path<String>,
    State(state): State<AppState>,
    _claims: AuthClaims,
) -> impl IntoResponse {
    match state.channels.get_snapshot(&market) {
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "market not found",
                "market": market,
                "hint": "no events have been ingested for this market yet"
            })),
        )
            .into_response(),

        Some(snap_lock) => {
            // Read lock — clone the snapshot, drop the lock, then serialise.
            let snap = snap_lock.read().await.clone();
            Json(snap).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /markets — enumerate known markets
// ---------------------------------------------------------------------------

/// Returns the list of market pubkey strings that have received at least one
/// event.  Useful for clients to discover available markets without out-of-band
/// configuration.
pub async fn markets_handler(
    State(state): State<AppState>,
    _claims: AuthClaims,
) -> impl IntoResponse {
    let mut markets = state.channels.market_keys();
    markets.sort();
    Json(serde_json::json!({ "markets": markets }))
}
