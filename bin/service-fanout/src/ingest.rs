/// POST /ingest — receives events from the execution engine's event sink.
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use tracing::warn;

use crate::app::AppState;
use crate::redis_pubsub;
use crate::types::EventEnvelope;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn check_ingest_secret(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    if let Some(expected) = &state.config.ingest_secret {
        let provided = headers
            .get("x-ingest-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected.as_str() {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(())
}

async fn dispatch_event(state: &AppState, ev: EventEnvelope) -> Result<usize, StatusCode> {
    if let Some(publisher) = &state.redis_publisher {
        let market = ev.market().unwrap_or_default().to_string();
        let payload = ev.payload().to_string();

        let mut conn = publisher.lock().await;
        match redis_pubsub::publish(
            &mut conn,
            &state.config.redis_stream_key,
            state.config.redis_stream_maxlen,
            &market,
            &payload,
        )
        .await
        {
            Ok(_) => {
                state.metrics.inc_redis_published();
                Ok(0)
            }
            Err(err) => {
                warn!("ingest: Redis append failed ({err})");
                state.metrics.inc_redis_publish_error();
                Err(StatusCode::SERVICE_UNAVAILABLE)
            }
        }
    } else {
        Ok(state.channels.dispatch(ev).await)
    }
}

fn spawn_upstream_forward(state: &AppState, payload: String) {
    let Some(url) = state.config.upstream_ingest_url.clone() else {
        return;
    };

    let client = state.upstream_client.clone();
    let auth_token = state.config.upstream_auth_token.clone();
    let metrics = state.metrics.clone();

    tokio::spawn(async move {
        let mut request = client
            .post(&url)
            .header("content-type", "application/json")
            .body(payload);
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }

        match request.send().await {
            Ok(response) if response.status().is_success() => {
                metrics.inc_upstream_forwarded();
            }
            Ok(response) => {
                warn!(
                    status = %response.status(),
                    url = %url,
                    "ingest: upstream forward failed"
                );
                metrics.inc_upstream_forward_error();
            }
            Err(err) => {
                warn!(url = %url, "ingest: upstream request failed ({err})");
                metrics.inc_upstream_forward_error();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Single-event handler
// ---------------------------------------------------------------------------

pub async fn ingest_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if let Err(code) = check_ingest_secret(&state, &headers) {
        return code.into_response();
    }

    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            warn!("ingest: failed to deserialize body: {err}");
            state.metrics.inc_ingest_error();
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let ev = match EventEnvelope::from_value(value) {
        Ok(ev) => ev,
        Err(err) => {
            warn!("ingest: failed to normalize event: {err}");
            state.metrics.inc_ingest_error();
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    if ev.market().is_none() {
        warn!(
            "ingest: event missing 'market' field (event_type={:?})",
            ev.event_type()
        );
        state.metrics.inc_ingest_error();
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let market = ev.market().unwrap_or("?").to_string();
    let event_type = ev.event_type().unwrap_or("?").to_string();
    let sequence = ev.sequence().unwrap_or(0);
    let upstream_payload = ev.payload().to_string();

    spawn_upstream_forward(&state, upstream_payload);

    let receiver_count = match dispatch_event(&state, ev).await {
        Ok(receiver_count) => receiver_count,
        Err(code) => {
            state.metrics.inc_ingest_error();
            return code.into_response();
        }
    };

    state.metrics.inc_ingested();
    if receiver_count == 0 && state.redis_publisher.is_none() {
        state.metrics.inc_no_subscriber();
    }

    tracing::debug!(
        market = %market,
        event_type = %event_type,
        sequence,
        receivers = receiver_count,
        "ingest ok"
    );

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Batch handler
// ---------------------------------------------------------------------------

pub async fn ingest_batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(events): Json<Vec<serde_json::Value>>,
) -> impl IntoResponse {
    if let Err(code) = check_ingest_secret(&state, &headers) {
        return code.into_response();
    }

    let mut ok = 0usize;
    let mut errors = 0usize;

    for value in events {
        let ev = match EventEnvelope::from_value(value) {
            Ok(ev) => ev,
            Err(err) => {
                warn!("ingest/batch: failed to normalize event: {err}");
                errors += 1;
                state.metrics.inc_ingest_error();
                continue;
            }
        };

        if ev.market().is_none() {
            errors += 1;
            state.metrics.inc_ingest_error();
            continue;
        }

        let upstream_payload = ev.payload().to_string();
        spawn_upstream_forward(&state, upstream_payload);

        match dispatch_event(&state, ev).await {
            Ok(receiver_count) => {
                state.metrics.inc_ingested();
                if receiver_count == 0 && state.redis_publisher.is_none() {
                    state.metrics.inc_no_subscriber();
                }
                ok += 1;
            }
            Err(code) => {
                warn!("ingest/batch: dispatch failed with {code}");
                errors += 1;
                state.metrics.inc_ingest_error();
            }
        }
    }

    if errors > 0 {
        warn!("ingest/batch: {errors} events skipped, {ok} dispatched");
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({ "dispatched": ok, "errors": errors })),
    )
        .into_response()
}
