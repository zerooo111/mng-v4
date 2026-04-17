//! Redis-backed multi-instance fanout.
//!
//! The ingest side appends each event to a Redis Stream. Every fanout instance
//! tails that stream with `XREAD`, preserving a replay path across transient
//! disconnects instead of relying on ephemeral pub/sub.

use std::sync::Arc;
use std::time::Duration;

use redis::streams::{StreamMaxlen, StreamReadOptions, StreamReadReply};
use redis::AsyncCommands;
use tracing::{error, info, warn};

use crate::channels::ChannelRegistry;
use crate::metrics::Metrics;
use crate::types::EventEnvelope;

// ---------------------------------------------------------------------------
// Publisher helper
// ---------------------------------------------------------------------------

pub async fn publish(
    conn: &mut redis::aio::ConnectionManager,
    stream_key: &str,
    maxlen: usize,
    market: &str,
    payload: &str,
) -> redis::RedisResult<String> {
    conn.xadd_maxlen(
        stream_key,
        StreamMaxlen::Approx(maxlen),
        "*",
        &[("market", market), ("payload", payload)],
    )
    .await
}

// ---------------------------------------------------------------------------
// Subscriber task
// ---------------------------------------------------------------------------

pub async fn run_subscriber(
    url: String,
    stream_key: String,
    block_ms: usize,
    channels: ChannelRegistry,
    metrics: Arc<Metrics>,
) {
    let mut backoff_secs: u64 = 1;
    let mut last_id = "$".to_string();

    loop {
        info!(
            "redis-sub: connecting to {} and tailing stream {} from {}",
            url, stream_key, last_id
        );
        match read_loop(
            &url,
            &stream_key,
            block_ms,
            &channels,
            &metrics,
            &mut last_id,
        )
        .await
        {
            Ok(()) => {
                warn!(
                    "redis-sub: connection closed cleanly, reconnecting in {}s",
                    backoff_secs
                );
                backoff_secs = 1;
            }
            Err(err) => {
                error!("redis-sub: {err}, reconnecting in {backoff_secs}s");
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(30);
    }
}

async fn read_loop(
    url: &str,
    stream_key: &str,
    block_ms: usize,
    channels: &ChannelRegistry,
    metrics: &Metrics,
    last_id: &mut String,
) -> redis::RedisResult<()> {
    let client = redis::Client::open(url)?;
    let mut conn = redis::aio::ConnectionManager::new(client).await?;
    let opts = StreamReadOptions::default().block(block_ms).count(256);

    loop {
        let reply: StreamReadReply = conn
            .xread_options(&[stream_key], &[last_id.as_str()], &opts)
            .await?;

        if reply.keys.is_empty() {
            continue;
        }

        for key in reply.keys {
            for entry in key.ids {
                *last_id = entry.id.clone();

                let payload: String = match entry.get("payload") {
                    Some(payload) => payload,
                    None => {
                        warn!(
                            "redis-sub: missing payload field in stream entry {}",
                            entry.id
                        );
                        continue;
                    }
                };

                let ev = match EventEnvelope::from_payload(payload) {
                    Ok(ev) => ev,
                    Err(err) => {
                        warn!("redis-sub: invalid event payload at {}: {err}", entry.id);
                        continue;
                    }
                };

                if ev.market().is_none() {
                    warn!("redis-sub: event at {} missing market field", entry.id);
                    continue;
                }

                channels.dispatch(ev).await;
                metrics.inc_redis_subscriber_dispatched();
            }
        }
    }
}
