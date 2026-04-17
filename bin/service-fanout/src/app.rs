/// `AppState` — the single, cheaply-cloneable handle injected into every
/// Axum handler via `State<AppState>`.
///
/// All heavy objects are wrapped in `Arc` so that `Clone` is O(1) and no
/// data is duplicated across tasks.
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::auth::ConnectionState;
use crate::channels::ChannelRegistry;
use crate::metrics::Metrics;
use crate::types::Config;

#[derive(Clone)]
pub struct AppState {
    pub channels: ChannelRegistry,
    pub metrics: Arc<Metrics>,
    pub connections: Arc<ConnectionState>,
    pub config: Arc<Config>,
    pub upstream_client: reqwest::Client,
    /// Redis connection manager for stream appends in multi-instance mode.
    ///
    /// `Some` when `FANOUT_REDIS_URL` is configured. The ingest handler appends
    /// events to the Redis stream; the background subscriber task tails the
    /// same stream and dispatches locally.
    ///
    /// Wrapped in `Arc<Mutex>` so `AppState: Clone` without constraining
    /// `ConnectionManager: Clone` across redis crate versions.
    pub redis_publisher: Option<Arc<Mutex<redis::aio::ConnectionManager>>>,
}

impl AppState {
    pub fn new(
        config: Config,
        redis_publisher: Option<Arc<Mutex<redis::aio::ConnectionManager>>>,
    ) -> anyhow::Result<Self> {
        let channels = ChannelRegistry::new(config.channel_capacity, config.snapshot_max_events);
        let metrics = Metrics::new();
        let connections =
            ConnectionState::new(config.max_connections, config.max_connections_per_user);
        let upstream_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.upstream_timeout_ms))
            .build()?;
        Ok(Self {
            channels,
            metrics,
            connections,
            config: Arc::new(config),
            upstream_client,
            redis_publisher,
        })
    }
}
