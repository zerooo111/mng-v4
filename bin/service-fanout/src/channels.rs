/// Per-market broadcast channels and snapshot cache.
///
/// `ChannelRegistry` is the single source of truth for:
/// - The `broadcast::Sender<Arc<EventEnvelope>>` that fans out events O(1)
///   to all SSE subscribers of a given market.
/// - The `RwLock<MarketSnapshot>` that stores the latest state for new
///   client onboarding (snapshot-then-stream reconnect pattern).
///
/// Both maps are `DashMap`-backed for lock-free concurrent access from
/// the ingest handler (writer) and many SSE handlers (readers).
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{broadcast, RwLock};

use crate::types::{EventEnvelope, MarketSnapshot};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub type Sender = broadcast::Sender<Arc<EventEnvelope>>;

#[derive(Clone)]
pub struct ChannelRegistry {
    /// One broadcast sender per market (keyed by market pubkey string).
    /// Cloning a `Sender` is cheap — all clones share the same channel.
    senders: Arc<DashMap<String, Sender>>,

    /// Per-market snapshot behind a `RwLock` — many concurrent reads for
    /// `/snapshot/{market}`, single writer in the ingest path.
    snapshots: Arc<DashMap<String, Arc<RwLock<MarketSnapshot>>>>,

    /// Capacity of each broadcast ring.  Set once at construction.
    channel_capacity: usize,

    /// Maximum recent-event ring size in each snapshot.
    snapshot_max_events: usize,
}

impl ChannelRegistry {
    pub fn new(channel_capacity: usize, snapshot_max_events: usize) -> Self {
        Self {
            senders: Arc::new(DashMap::new()),
            snapshots: Arc::new(DashMap::new()),
            channel_capacity,
            snapshot_max_events,
        }
    }

    // -----------------------------------------------------------------------
    // Sender access
    // -----------------------------------------------------------------------

    /// Return the sender for `market`, creating a fresh broadcast channel if
    /// this is the first time we have seen this market.
    ///
    /// Creating a channel allocates a fixed ring buffer of
    /// `channel_capacity` slots; the initial `_rx` is dropped immediately
    /// since we only need the sender.  The channel stays alive as long as at
    /// least one `Sender` clone exists (held by this registry).
    pub fn get_or_create_sender(&self, market: &str) -> Sender {
        if let Some(tx) = self.senders.get(market) {
            return tx.value().clone();
        }
        let (tx, _rx) = broadcast::channel(self.channel_capacity);
        self.senders
            .entry(market.to_string())
            .or_insert(tx)
            .value()
            .clone()
    }

    /// Return the sender for `market` only if it already exists.
    /// Used by the SSE handler to avoid silently creating phantom channels
    /// for markets the engine has never touched.
    pub fn get_sender(&self, market: &str) -> Option<Sender> {
        self.senders.get(market).map(|e| e.value().clone())
    }

    /// Number of active receivers across all markets (approximate — counts
    /// subscribers registered with each sender).
    pub fn total_receiver_count(&self) -> usize {
        self.senders
            .iter()
            .map(|e| e.value().receiver_count())
            .sum()
    }

    /// Names of all markets that have received at least one event.
    pub fn market_keys(&self) -> Vec<String> {
        self.senders.iter().map(|e| e.key().clone()).collect()
    }

    // -----------------------------------------------------------------------
    // Snapshot access
    // -----------------------------------------------------------------------

    /// Return (or create) the snapshot handle for `market`.
    pub fn get_or_create_snapshot(&self, market: &str) -> Arc<RwLock<MarketSnapshot>> {
        if let Some(snap) = self.snapshots.get(market) {
            return snap.value().clone();
        }
        let mut s = MarketSnapshot::new(market.to_string());
        s.max_events = self.snapshot_max_events;
        let arc = Arc::new(RwLock::new(s));
        self.snapshots
            .entry(market.to_string())
            .or_insert(arc)
            .value()
            .clone()
    }

    /// Return the snapshot handle only if the market exists.
    pub fn get_snapshot(&self, market: &str) -> Option<Arc<RwLock<MarketSnapshot>>> {
        self.snapshots.get(market).map(|e| e.value().clone())
    }

    // -----------------------------------------------------------------------
    // Combined ingest helper
    // -----------------------------------------------------------------------

    /// Called by the ingest handler for every incoming event.
    ///
    /// 1. Ensures sender + snapshot exist for the market.
    /// 2. Broadcasts `ev` to all current subscribers (O(1)).
    /// 3. Applies `ev` to the snapshot under a write lock (held for
    ///    microseconds — never across an `.await`).
    ///
    /// Returns the number of active receivers the event was delivered to
    /// (may be 0 if nobody is subscribed yet).
    pub async fn dispatch(&self, ev: Arc<EventEnvelope>) -> usize {
        let market = match ev.market() {
            Some(m) => m.to_string(),
            None => return 0,
        };

        let tx = self.get_or_create_sender(&market);
        let snap = self.get_or_create_snapshot(&market);

        // Broadcast first (O(1), non-blocking).
        let receiver_count = tx.send(Arc::clone(&ev)).unwrap_or(0);

        // Update snapshot (write lock, no await inside).
        snap.write().await.apply(&ev);

        receiver_count
    }
}
