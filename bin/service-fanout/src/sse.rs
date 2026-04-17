/// GET /stream/{market} — SSE (Server-Sent Events) handler.
///
/// ## Client protocol
///
/// 1. Fetch the current snapshot: `GET /snapshot/{market}`
///    → note `last_sequence: N`
/// 2. Open SSE stream: `GET /stream/{market}?from=N`
///    → the handler skips buffered events with `sequence <= N` so the client
///    receives only events that occurred *after* the snapshot.
/// 3. If the client receives an `event: resync` frame it has fallen behind
///    the broadcast ring buffer.  It should re-fetch the snapshot and
///    re-subscribe.
///
/// ## Backpressure
///
/// The broadcast ring (4096 events per market) acts as the only buffer.  Slow
/// clients that fall more than 4096 events behind are notified via the resync
/// event; the publisher is never stalled.
///
/// ## Connection cleanup
///
/// `ConnectionGuard` (RAII) releases the semaphore permit and decrements the
/// per-user counter when the stream is dropped — covers both graceful client
/// disconnects and server-side cancellation.
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use serde::Deserialize;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tracing::debug;

use crate::app::AppState;
use crate::auth::{AuthClaims, ConnectionGuard};

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Sequence watermark from a prior snapshot fetch.  Events with
    /// `sequence <= from` are skipped to avoid replaying already-seen data.
    pub from: Option<u64>,
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
    // --- Resolve channel ---
    // Use get_or_create so clients can subscribe before the first event
    // arrives (e.g. during service start-up).
    let tx = state.channels.get_or_create_sender(&market);

    // --- Connection limiting ---
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
        "SSE subscriber connected"
    );

    let from_seq = params.from.unwrap_or(0);
    let metrics = Arc::clone(&state.metrics);

    // --- Build mapped stream ---
    let raw = BroadcastStream::new(tx.subscribe());

    // tokio_stream::StreamExt::filter_map takes a sync FnMut → Option<T>;
    // all our mapping operations are synchronous (serialise + Arc ops), so no
    // async wrapper is needed here.
    let mapped = raw.filter_map(move |result| {
        match result {
            Ok(ev) => {
                // Skip events already covered by the snapshot the client
                // fetched before subscribing.
                if from_seq > 0 {
                    if let Some(seq) = ev.sequence() {
                        if seq <= from_seq {
                            return None;
                        }
                    }
                }
                let data = serde_json::to_string(&ev.0).unwrap_or_default();
                Some(Ok::<Event, Infallible>(
                    Event::default().event("event").data(data),
                ))
            }
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                metrics.inc_lagged();
                // Tell the client to re-fetch snapshot and re-subscribe.
                Some(Ok(Event::default()
                    .event("resync")
                    .data(format!("lagged_by={n}"))))
            }
        }
    });

    // --- Wrap with RAII guard so cleanup fires on disconnect ---
    let guarded: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(GuardedStream {
            inner: Box::pin(mapped),
            _guard: guard,
        });

    Sse::new(guarded).keep_alive(KeepAlive::default()).into_response()
}

// ---------------------------------------------------------------------------
// GuardedStream — RAII wrapper
// ---------------------------------------------------------------------------

/// Wraps an inner `Stream` and holds a `ConnectionGuard`.  When the stream is
/// dropped (client disconnect, handler cancellation, or normal EOF) the guard
/// fires its `Drop` impl, decrementing the per-user counter and releasing the
/// global semaphore permit.
struct GuardedStream {
    /// Pinned, boxed to avoid complex generic bounds — the allocation is
    /// negligible compared to a live SSE connection.
    inner: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>,
    _guard: ConnectionGuard,
}

impl Stream for GuardedStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `self` is `Pin<&mut GuardedStream>`.  `inner` is already `Pin<Box<...>>`,
        // so we can call `as_mut().poll_next` directly.
        self.inner.as_mut().poll_next(cx)
    }
}

// GuardedStream is Unpin because Pin<Box<T>>: Unpin for any T.
// The compiler may not derive this automatically due to the dyn field;
// assert it explicitly.
impl Unpin for GuardedStream {}
