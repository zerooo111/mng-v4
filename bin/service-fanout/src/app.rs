/// `AppState` — the single, cheaply-cloneable handle injected into every
/// Axum handler via `State<AppState>`.
///
/// All heavy objects are wrapped in `Arc` so that `Clone` is O(1) and no
/// data is duplicated across tasks.
use std::sync::Arc;

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
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let channels = ChannelRegistry::new(config.channel_capacity, config.snapshot_max_events);
        let metrics = Metrics::new();
        let connections = ConnectionState::new(config.max_connections, config.max_connections_per_user);
        Self {
            channels,
            metrics,
            connections,
            config: Arc::new(config),
        }
    }
}
