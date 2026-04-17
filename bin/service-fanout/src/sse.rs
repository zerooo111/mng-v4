/// GET /stream/{market} — SSE (Server-Sent Events) handler.
///
/// Client flow:
/// 1. `GET /snapshot/{market}` and note `last_cursor` or legacy `last_sequence`
/// 2. `GET /stream/{market}?cursor=<last_cursor>` for exact replay semantics
/// 3. On `event: resync`, fetch a fresh snapshot and reconnect
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use serde::Deserialize;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tracing::debug;

use crate::app::AppState;
use crate::auth::{AuthClaims, ConnectionGuard};
use crate::types::MarketSnapshot;

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Legacy sequence watermark from a prior snapshot fetch.
    pub from: Option<u64>,
    /// Precise fanout watermark from `/snapshot/{market}`.
    pub cursor: Option<u64>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn sse_handler(
    Path(market): Path<String>,
    Query(params): Query<StreamQuery>,
    State(state): State<AppState>,
    claims: AuthClaims,
) -> impl IntoResponse {
    let tx = match state.channels.get_sender(&market) {
        Some(tx) => tx,
        None => {
            return (
                StatusCode::NOT_FOUND,
                "market not found; fetch /markets or /snapshot first",
            )
                .into_response()
        }
    };
    let snap_lock = match state.channels.get_snapshot(&market) {
        Some(snap) => snap,
        None => {
            return (
                StatusCode::NOT_FOUND,
                "market not found; fetch /markets or /snapshot first",
            )
                .into_response()
        }
    };

    let guard = match state
        .connections
        .acquire(claims.user_id.clone(), &state.metrics)
        .await
    {
        Ok(g) => g,
        Err((code, msg)) => return (code, msg).into_response(),
    };

    debug!(
        user = %claims.user_id,
        market = %market,
        from = ?params.from,
        cursor = ?params.cursor,
        "SSE subscriber connected"
    );

    // Subscribe before reading the snapshot so live events that arrive during
    // snapshot cloning remain buffered in the broadcast ring.
    let raw = BroadcastStream::new(tx.subscribe());
    let snapshot = snap_lock.read().await.clone();
    let snapshot_cursor = snapshot.last_cursor;
    let requested_cursor = params.cursor;
    let requested_sequence = params.from;
    let metrics = Arc::clone(&state.metrics);

    let mut initial_events: Vec<Result<Event, Infallible>> = Vec::new();
    if is_cursor_too_old(&snapshot, requested_cursor) {
        initial_events.push(Ok(resync_event("cursor_out_of_replay_window")));
    } else {
        initial_events.extend(
            snapshot
                .replay_since(requested_cursor, requested_sequence)
                .into_iter()
                .map(|ev| Ok(data_event(ev.payload.as_ref()))),
        );
    }

    let live = raw.filter_map(move |result| match result {
        Ok(ev) => {
            if ev.cursor() <= snapshot_cursor {
                return None;
            }
            Some(Ok::<Event, Infallible>(data_event(ev.payload())))
        }
        Err(BroadcastStreamRecvError::Lagged(n)) => {
            metrics.inc_lagged();
            Some(Ok(resync_event(&format!("lagged_by={n}"))))
        }
    });

    let stream = tokio_stream::iter(initial_events).chain(live);

    let guarded: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(GuardedStream {
            inner: Box::pin(stream),
            _guard: guard,
        });

    Sse::new(guarded)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn data_event(payload: &str) -> Event {
    Event::default().event("event").data(payload)
}

fn resync_event(reason: &str) -> Event {
    Event::default().event("resync").data(reason)
}

fn is_cursor_too_old(snapshot: &MarketSnapshot, cursor: Option<u64>) -> bool {
    let Some(cursor) = cursor else {
        return false;
    };
    let Some(first_cursor) = snapshot.first_cursor() else {
        return false;
    };
    snapshot.last_cursor > cursor && cursor.saturating_add(1) < first_cursor
}

// ---------------------------------------------------------------------------
// GuardedStream — RAII wrapper
// ---------------------------------------------------------------------------

struct GuardedStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>,
    _guard: ConnectionGuard,
}

impl Stream for GuardedStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Unpin for GuardedStream {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventEnvelope;

    #[test]
    fn detects_when_cursor_falls_outside_snapshot_replay_window() {
        let mut snapshot = MarketSnapshot::new("0".to_string());
        snapshot.max_events = 2;
        snapshot.apply(
            EventEnvelope::from_value(serde_json::json!({
                "market": "0",
                "sequence": "1",
                "ts_ms": 1
            }))
            .unwrap(),
        );
        snapshot.apply(
            EventEnvelope::from_value(serde_json::json!({
                "market": "0",
                "sequence": "2",
                "ts_ms": 2
            }))
            .unwrap(),
        );
        snapshot.apply(
            EventEnvelope::from_value(serde_json::json!({
                "market": "0",
                "sequence": "3",
                "ts_ms": 3
            }))
            .unwrap(),
        );

        assert!(is_cursor_too_old(&snapshot, Some(0)));
        assert!(!is_cursor_too_old(&snapshot, Some(2)));
        assert!(!is_cursor_too_old(&snapshot, None));
    }
}
