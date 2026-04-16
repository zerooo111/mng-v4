/// POST /ingest — receives events from the execution engine's event_sink_url.
///
/// This is the only write path in the fanout service.  It is intentionally
/// unauthenticated (called from localhost / internal network).  An optional
/// `FANOUT_INGEST_SECRET` check can be enabled for defence-in-depth.
///
/// On each call the handler:
/// 1. Validates the optional ingest secret header.
/// 2. Deserialises the body into an `EventEnvelope` (raw JSON — no schema
///    coupling with the engine).
/// 3. Calls `ChannelRegistry::dispatch`, which broadcasts to all subscribers
///    and updates the snapshot — both O(1) / sub-millisecond.
/// 4. Updates metrics counters (relaxed atomics, negligible cost).
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use tracing::warn;

use crate::app::AppState;
use crate::types::EventEnvelope;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn ingest_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // --- Optional ingest secret check ---
    if let Some(expected) = &state.config.ingest_secret {
        let provided = headers
            .get("x-ingest-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected.as_str() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    // --- Deserialise ---
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            warn!("ingest: failed to deserialise body: {err}");
            state.metrics.inc_ingest_error();
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let ev = Arc::new(EventEnvelope(value));

    // Reject events with no market field — we can't route them.
    if ev.market().is_none() {
        warn!(
            "ingest: event missing 'market' field (event_type={:?})",
            ev.event_type()
        );
        state.metrics.inc_ingest_error();
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    // --- Broadcast + snapshot update ---
    let receiver_count = state.channels.dispatch(Arc::clone(&ev)).await;

    state.metrics.inc_ingested();
    if receiver_count == 0 {
        state.metrics.inc_no_subscriber();
    }

    tracing::debug!(
        market = ev.market().unwrap_or("?"),
        event_type = ev.event_type().unwrap_or("?"),
        sequence = ev.sequence().unwrap_or(0),
        receivers = receiver_count,
        "ingest ok"
    );

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Batch ingest (optional — accepts a JSON array of events in one POST)
// ---------------------------------------------------------------------------

/// `POST /ingest/batch` — for callers that want to amortise HTTP overhead by
/// sending multiple events per request.  Processes each element with the same
/// logic as the single-event handler.
pub async fn ingest_batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(events): Json<Vec<serde_json::Value>>,
) -> impl IntoResponse {
    // --- Optional ingest secret check ---
    if let Some(expected) = &state.config.ingest_secret {
        let provided = headers
            .get("x-ingest-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected.as_str() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    let mut ok = 0usize;
    let mut errors = 0usize;

    for value in events {
        let ev = Arc::new(EventEnvelope(value));
        if ev.market().is_none() {
            errors += 1;
            state.metrics.inc_ingest_error();
            continue;
        }
        let receiver_count = state.channels.dispatch(Arc::clone(&ev)).await;
        state.metrics.inc_ingested();
        if receiver_count == 0 {
            state.metrics.inc_no_subscriber();
        }
        ok += 1;
    }

    if errors > 0 {
        warn!("ingest/batch: {errors} events skipped (missing 'market'), {ok} dispatched");
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({ "dispatched": ok, "errors": errors })),
    )
        .into_response()
}
