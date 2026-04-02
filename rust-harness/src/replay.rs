use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use anchor_lang::prelude::Pubkey;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use fixed::types::I80F48;
use mango_v4::state::{PlaceOrderType, SelfTradeBehavior, Side};
use serde_json::{json, Value};

use crate::types::{
    AccountMetaWire, MarketLevel, MarketWatermarks, UserBalanceTotals, UserPerMarketState,
};
use crate::{
    AccountSnapshot, CanonicalIntentState, DivergenceEvent, EngineSnapshot, ExecutionResult,
    HarnessEngine, HarnessError, MarginSummary, MarginSummaryPlaceholder, MarketCandle,
    MarketConfig, MarketState, MarketTrade, OpenOrderSummary, QueueItemEnqueuedEvent,
    QueueItemProcessedEvent, QueuePayload, QueueView, RelayIntentAcceptedEvent, Result,
    UserBalances, UserState,
};

const QUEUE_ITEM_KIND_CTM_WRAPPED: u8 = 0;
const QUEUE_PROCESS_EXECUTED: u8 = 2;
const QUEUE_PROCESS_FAILED: u8 = 3;
const QUEUE_PROCESS_SKIPPED: u8 = 4;
const MAX_TRADE_HISTORY: usize = 5_000;

#[derive(Debug, Clone)]
struct CanonicalIntent {
    key: String,
    group: String,
    execution_queue: String,
    market: String,
    market_index: Option<u16>,
    sequence: u64,
    kind: u8,
    payload_b64: String,
    decoded_payload: Option<QueuePayload>,
    remaining_accounts: Vec<AccountMetaWire>,
    min_execute_slot: u64,
    expires_at_slot: u64,
    user_owner: String,
    user_owner_pubkey: Option<Pubkey>,
    mango_account: String,
    mango_account_pubkey: Option<Pubkey>,
    enqueue_tx_signature: String,
    accepted_ts_ms: u64,
    enqueued_slot: Option<u64>,
    processed_slot: Option<u64>,
    processed_status: Option<u8>,
    processed_tx_signature: Option<String>,
}

#[derive(Debug, Clone)]
struct BaselinePosition {
    base_position_lots: i64,
    quote_position_native: I80F48,
}

#[derive(Debug, Clone)]
struct AccountIdentity {
    owner: String,
}

#[derive(Debug, Clone)]
struct InternalTrade {
    trade_id: String,
    market: String,
    price_lots: i64,
    base_lots: i64,
    quote_lots: i64,
    taker_side: String,
    maker_owner: String,
    taker_owner: String,
    maker_order_id: String,
    taker_sequence: u64,
    ts_ms: u64,
}

#[derive(Debug, Clone, Default)]
struct QueueAggregate {
    pending_count: u64,
    processed_count: u64,
    failed_count: u64,
    skipped_count: u64,
    last_processed_sequence: u64,
    min_pending_execute_slot: Option<u64>,
    unmatched_processed_count: u64,
}

struct Projection {
    engine: HarnessEngine,
    active_markets: BTreeSet<String>,
    owner_accounts: BTreeMap<String, BTreeSet<String>>,
    account_identities: HashMap<Pubkey, AccountIdentity>,
    queue: BTreeMap<String, QueueAggregate>,
    trades: BTreeMap<String, Vec<InternalTrade>>,
    last_slot: u64,
}

#[derive(Debug, Clone)]
struct ViewState {
    snapshot: EngineSnapshot,
    trades: BTreeMap<String, Vec<InternalTrade>>,
}

#[derive(Debug, Clone)]
struct ViewStateCache {
    revision: u64,
    state: ViewState,
}

#[derive(Debug, Clone, Default)]
struct UserPerMarketAggregate {
    open_bid: i128,
    open_ask: i128,
    quote_reserved: i128,
    base_position_lots: i128,
    quote_position_native: I80F48,
}

pub struct ContinuumStateEngine {
    intents_by_key: HashMap<String, CanonicalIntent>,
    processed_event_ids: HashSet<String>,
    enqueued_event_ids: HashSet<String>,
    divergences: Vec<DivergenceEvent>,
    revision: u64,
    last_seen_slot: u64,
    cached_confirmed: Option<ViewStateCache>,
    cached_optimistic: Option<ViewStateCache>,
    baseline_positions: BTreeMap<(String, String), BaselinePosition>,
    baseline_orders: BTreeMap<String, OpenOrderSummary>,
    baseline_confirmed_seq: BTreeMap<String, u64>,
    baseline_user_accounts: BTreeMap<String, BTreeSet<String>>,
    baseline_bootstrapped: bool,
}

impl Default for ContinuumStateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ContinuumStateEngine {
    pub fn new() -> Self {
        Self {
            intents_by_key: HashMap::new(),
            processed_event_ids: HashSet::new(),
            enqueued_event_ids: HashSet::new(),
            divergences: Vec::new(),
            revision: 0,
            last_seen_slot: 0,
            cached_confirmed: None,
            cached_optimistic: None,
            baseline_positions: BTreeMap::new(),
            baseline_orders: BTreeMap::new(),
            baseline_confirmed_seq: BTreeMap::new(),
            baseline_user_accounts: BTreeMap::new(),
            baseline_bootstrapped: false,
        }
    }

    pub fn bootstrap_from_onchain_snapshot(&mut self, snapshot: EngineSnapshot) -> Result<()> {
        if self.baseline_bootstrapped {
            return Ok(());
        }

        for (owner, user_state) in snapshot.users {
            let owner_accounts = self
                .baseline_user_accounts
                .entry(owner.clone())
                .or_default();
            owner_accounts.extend(user_state.mango_accounts);
            for per_market in user_state.per_market {
                let key = (owner.clone(), per_market.market.clone());
                let entry = self
                    .baseline_positions
                    .entry(key)
                    .or_insert(BaselinePosition {
                        base_position_lots: 0,
                        quote_position_native: I80F48::ZERO,
                    });
                entry.base_position_lots +=
                    parse_i64_field("base_position_lots", &per_market.base_position_lots)?;
                entry.quote_position_native +=
                    parse_i80f48_field("quote_position_native", &per_market.quote_position_native)?;
            }
        }

        for (market, market_state) in snapshot.markets {
            self.baseline_confirmed_seq.insert(
                market.clone(),
                parse_u64_field("confirmed_seq", &market_state.watermarks.confirmed_seq)?,
            );
            for order in market_state.open_orders {
                self.baseline_user_accounts
                    .entry(order.owner.clone())
                    .or_default()
                    .insert(order.mango_account.clone());
                self.baseline_orders.insert(order.order_id.clone(), order);
            }
        }

        self.baseline_bootstrapped = true;
        self.touch();
        Ok(())
    }

    pub fn bootstrap_from_onchain_snapshot_json(&mut self, snapshot_json: &str) -> Result<()> {
        let snapshot: EngineSnapshot = serde_json::from_str(snapshot_json)?;
        self.bootstrap_from_onchain_snapshot(snapshot)
    }

    pub fn ingest_relay_intent(&mut self, event: RelayIntentAcceptedEvent) -> Result<()> {
        let payload = BASE64_STANDARD
            .decode(event.payload_b64.as_bytes())
            .unwrap_or_default();
        let decoded_payload = QueuePayload::decode(&payload).ok();

        let sequence = parse_u64_field("sequence", &event.sequence)?;
        let market_index = parse_market_index(&event.market)?;
        let user_owner_pubkey = parse_pubkey("user_owner", &event.user_owner)?;
        let mango_account_pubkey = parse_pubkey("mango_account", &event.mango_account)?;
        let key = queue_item_key(&event.group, sequence, event.kind);
        let existing = self.intents_by_key.get(&key).cloned();

        let canonical = CanonicalIntent {
            key: key.clone(),
            group: event.group.clone(),
            execution_queue: event.execution_queue,
            market: event.market.clone(),
            market_index: Some(market_index),
            sequence,
            kind: event.kind,
            payload_b64: event.payload_b64,
            decoded_payload,
            remaining_accounts: event.remaining_accounts,
            min_execute_slot: parse_u64_field("min_execute_slot", &event.min_execute_slot)?,
            expires_at_slot: parse_u64_field("expires_at_slot", &event.expires_at_slot)?,
            user_owner: event.user_owner.clone(),
            user_owner_pubkey: Some(user_owner_pubkey),
            mango_account: event.mango_account.clone(),
            mango_account_pubkey: Some(mango_account_pubkey),
            enqueue_tx_signature: event.enqueue_tx_signature.clone(),
            accepted_ts_ms: event.ts_ms,
            enqueued_slot: existing.as_ref().and_then(|it| it.enqueued_slot),
            processed_slot: existing.as_ref().and_then(|it| it.processed_slot),
            processed_status: existing.as_ref().and_then(|it| it.processed_status),
            processed_tx_signature: existing
                .as_ref()
                .and_then(|it| it.processed_tx_signature.clone()),
        };

        if let Some(existing) = existing {
            if existing.enqueue_tx_signature != canonical.enqueue_tx_signature {
                self.emit_divergence(
                    "duplicate_key_different_signature",
                    &key,
                    HashMap::from([
                        (
                            "existing_signature".to_string(),
                            existing.enqueue_tx_signature.clone(),
                        ),
                        (
                            "incoming_signature".to_string(),
                            canonical.enqueue_tx_signature.clone(),
                        ),
                    ]),
                );
            }
        }

        self.intents_by_key.insert(key, canonical);
        self.touch();
        Ok(())
    }

    pub fn ingest_relay_intent_json(&mut self, event_json: &str) -> Result<()> {
        let event: RelayIntentAcceptedEvent = serde_json::from_str(event_json)?;
        self.ingest_relay_intent(event)
    }

    pub fn ingest_queue_enqueued(&mut self, event: QueueItemEnqueuedEvent) -> Result<()> {
        let id = format!(
            "{}:{}:{}:{}",
            event.tx_signature, event.group, event.sequence, event.kind
        );
        if !self.enqueued_event_ids.insert(id) {
            return Ok(());
        }

        let sequence = parse_u64_field("sequence", &event.sequence)?;
        let slot = parse_u64_field("slot", &event.slot)?;
        if slot > self.last_seen_slot {
            self.last_seen_slot = slot;
        }

        let key = queue_item_key(&event.group, sequence, event.kind);
        if let Some(intent) = self.intents_by_key.get_mut(&key) {
            intent.enqueued_slot = Some(slot);
            intent.min_execute_slot = parse_u64_field("min_execute_slot", &event.min_execute_slot)?;
        }

        self.touch();
        Ok(())
    }

    pub fn ingest_queue_enqueued_json(&mut self, event_json: &str) -> Result<()> {
        let event: QueueItemEnqueuedEvent = serde_json::from_str(event_json)?;
        self.ingest_queue_enqueued(event)
    }

    pub fn ingest_queue_processed(&mut self, event: QueueItemProcessedEvent) -> Result<()> {
        let id = format!(
            "{}:{}:{}:{}:{}",
            event.tx_signature, event.group, event.sequence, event.kind, event.status
        );
        if !self.processed_event_ids.insert(id) {
            return Ok(());
        }

        let sequence = parse_u64_field("sequence", &event.sequence)?;
        let slot = parse_u64_field("slot", &event.slot)?;
        if slot > self.last_seen_slot {
            self.last_seen_slot = slot;
        }

        let key = queue_item_key(&event.group, sequence, event.kind);
        if let Some(intent) = self.intents_by_key.get_mut(&key) {
            let previous_status = intent.processed_status;

            intent.processed_status = Some(event.status);
            intent.processed_slot = Some(slot);
            intent.processed_tx_signature = Some(event.tx_signature);
            let _ = intent;
            if let Some(previous_status) = previous_status.filter(|status| *status != event.status)
            {
                self.emit_divergence(
                    "processed_status_changed",
                    &key,
                    HashMap::from([
                        ("previous_status".to_string(), previous_status.to_string()),
                        ("incoming_status".to_string(), event.status.to_string()),
                    ]),
                );
            }
        } else {
            self.emit_divergence(
                "processed_without_relay_intent",
                &key,
                HashMap::from([
                    ("status".to_string(), event.status.to_string()),
                    ("slot".to_string(), event.slot.clone()),
                    ("tx_signature".to_string(), event.tx_signature.clone()),
                ]),
            );

            self.intents_by_key.insert(
                key.clone(),
                CanonicalIntent {
                    key,
                    group: event.group,
                    execution_queue: "unknown".to_string(),
                    market: "unknown".to_string(),
                    market_index: None,
                    sequence,
                    kind: event.kind,
                    payload_b64: String::new(),
                    decoded_payload: None,
                    remaining_accounts: Vec::new(),
                    min_execute_slot: 0,
                    expires_at_slot: 0,
                    user_owner: "unknown".to_string(),
                    user_owner_pubkey: None,
                    mango_account: "unknown".to_string(),
                    mango_account_pubkey: None,
                    enqueue_tx_signature: "unknown".to_string(),
                    accepted_ts_ms: event.ts_ms,
                    enqueued_slot: None,
                    processed_slot: Some(slot),
                    processed_status: Some(event.status),
                    processed_tx_signature: Some(event.tx_signature),
                },
            );
        }

        self.touch();
        Ok(())
    }

    pub fn ingest_queue_processed_json(&mut self, event_json: &str) -> Result<()> {
        let event: QueueItemProcessedEvent = serde_json::from_str(event_json)?;
        self.ingest_queue_processed(event)
    }

    pub fn list_divergences(&self, limit: usize) -> Vec<DivergenceEvent> {
        let start = self.divergences.len().saturating_sub(limit);
        self.divergences[start..].to_vec()
    }

    pub fn list_divergences_json(&self, limit: usize) -> Result<String> {
        Ok(serde_json::to_string(&self.list_divergences(limit))?)
    }

    pub fn report_external_divergence(
        &mut self,
        reason: &str,
        key: &str,
        details: HashMap<String, String>,
    ) {
        self.emit_divergence(reason, key, details);
    }

    pub fn list_intents(&self) -> Vec<CanonicalIntentState> {
        let mut intents: Vec<_> = self.intents_by_key.values().cloned().collect();
        intents.sort_by(|a, b| {
            a.group
                .cmp(&b.group)
                .then_with(|| compare_market_strings(&a.market, &b.market))
                .then_with(|| a.sequence.cmp(&b.sequence))
                .then_with(|| a.accepted_ts_ms.cmp(&b.accepted_ts_ms))
                .then_with(|| a.key.cmp(&b.key))
        });
        intents
            .into_iter()
            .map(|intent| CanonicalIntentState {
                key: intent.key,
                group: intent.group,
                execution_queue: intent.execution_queue,
                market: intent.market,
                sequence: intent.sequence.to_string(),
                kind: intent.kind,
                payload_b64: intent.payload_b64,
                decoded_payload: intent.decoded_payload.map(decoded_payload_to_json),
                remaining_accounts: intent.remaining_accounts,
                min_execute_slot: intent.min_execute_slot.to_string(),
                expires_at_slot: intent.expires_at_slot.to_string(),
                user_owner: intent.user_owner,
                mango_account: intent.mango_account,
                enqueue_tx_signature: intent.enqueue_tx_signature,
                accepted_ts_ms: intent.accepted_ts_ms,
                enqueued_slot: intent.enqueued_slot.map(|slot| slot.to_string()),
                processed_slot: intent.processed_slot.map(|slot| slot.to_string()),
                processed_status: intent.processed_status,
                processed_tx_signature: intent.processed_tx_signature,
            })
            .collect()
    }

    pub fn list_intents_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.list_intents())?)
    }

    pub fn get_market_state(&mut self, market: &str, view: QueueView) -> Result<MarketState> {
        let snapshot = self.get_snapshot(view)?;
        Ok(snapshot
            .markets
            .get(market)
            .cloned()
            .unwrap_or_else(|| empty_market_state(market, self.last_seen_slot)))
    }

    pub fn get_market_state_json(&mut self, market: &str, view: QueueView) -> Result<String> {
        Ok(serde_json::to_string(
            &self.get_market_state(market, view)?,
        )?)
    }

    pub fn get_user_state(&mut self, owner: &str, view: QueueView) -> Result<UserState> {
        let snapshot = self.get_snapshot(view)?;
        Ok(snapshot
            .users
            .get(owner)
            .cloned()
            .unwrap_or_else(|| empty_user_state(owner)))
    }

    pub fn get_user_state_json(&mut self, owner: &str, view: QueueView) -> Result<String> {
        Ok(serde_json::to_string(&self.get_user_state(owner, view)?)?)
    }

    pub fn get_orders(
        &mut self,
        market: &str,
        owner: Option<&str>,
        view: QueueView,
    ) -> Result<Vec<OpenOrderSummary>> {
        let market_state = self.get_market_state(market, view)?;
        Ok(match owner {
            Some(owner) => market_state
                .open_orders
                .into_iter()
                .filter(|order| order.owner == owner)
                .collect(),
            None => market_state.open_orders,
        })
    }

    pub fn get_queue_state(&mut self, market: &str) -> Result<crate::QueueState> {
        let snapshot = self.get_snapshot(QueueView::Optimistic)?;
        Ok(snapshot
            .queue
            .get(market)
            .cloned()
            .unwrap_or_else(|| empty_queue_state(market)))
    }

    pub fn get_queue_state_json(&mut self, market: &str) -> Result<String> {
        Ok(serde_json::to_string(&self.get_queue_state(market)?)?)
    }

    pub fn get_balances(&mut self, owner: &str, view: QueueView) -> Result<UserBalances> {
        let user = self.get_user_state(owner, view)?;
        let mut total_open_bid = 0i128;
        let mut total_open_ask = 0i128;
        let mut total_quote_reserved = 0i128;
        for entry in &user.per_market {
            total_open_bid += parse_i128_generated(&entry.open_order_base_lots_bid)?;
            total_open_ask += parse_i128_generated(&entry.open_order_base_lots_ask)?;
            total_quote_reserved += parse_i128_generated(&entry.quote_reserved_lots)?;
        }

        Ok(UserBalances {
            owner: user.owner.clone(),
            mango_accounts: user.mango_accounts.clone(),
            per_market: user.per_market,
            totals: UserBalanceTotals {
                total_open_order_base_lots_bid: total_open_bid.to_string(),
                total_open_order_base_lots_ask: total_open_ask.to_string(),
                total_quote_reserved_lots: total_quote_reserved.to_string(),
            },
            margin_summary: user.margin_summary,
            view,
        })
    }

    pub fn get_balances_json(&mut self, owner: &str, view: QueueView) -> Result<String> {
        Ok(serde_json::to_string(&self.get_balances(owner, view)?)?)
    }

    pub fn get_trades(
        &mut self,
        market: &str,
        view: QueueView,
        limit: usize,
    ) -> Result<Vec<MarketTrade>> {
        let state = self.view_state(view)?;
        let trades = state.trades.get(market).cloned().unwrap_or_default();
        let start = trades.len().saturating_sub(limit);
        Ok(trades[start..]
            .iter()
            .map(|trade| MarketTrade {
                trade_id: trade.trade_id.clone(),
                market: trade.market.clone(),
                price_lots: trade.price_lots.to_string(),
                base_lots: trade.base_lots.to_string(),
                quote_lots: trade.quote_lots.to_string(),
                taker_side: trade.taker_side.clone(),
                maker_owner: trade.maker_owner.clone(),
                taker_owner: trade.taker_owner.clone(),
                maker_order_id: trade.maker_order_id.clone(),
                taker_sequence: trade.taker_sequence.to_string(),
                ts_ms: trade.ts_ms,
                view,
            })
            .collect())
    }

    pub fn get_trades_json(
        &mut self,
        market: &str,
        view: QueueView,
        limit: usize,
    ) -> Result<String> {
        Ok(serde_json::to_string(
            &self.get_trades(market, view, limit)?,
        )?)
    }

    pub fn get_candles(
        &mut self,
        market: &str,
        view: QueueView,
        resolution_sec: u64,
        limit: usize,
    ) -> Result<Vec<MarketCandle>> {
        let safe_resolution = if resolution_sec == 0 {
            60
        } else {
            resolution_sec
        };
        let trades = self.get_trades(market, view, 10_000)?;
        if trades.is_empty() {
            return Ok(Vec::new());
        }

        let bucket_size_ms = safe_resolution.saturating_mul(1_000);
        let mut buckets = BTreeMap::<u64, MarketCandle>::new();
        for trade in trades {
            let bucket_start = (trade.ts_ms / bucket_size_ms) * bucket_size_ms;
            let price = parse_i128_generated(&trade.price_lots)?;
            let base_lots = parse_i128_generated(&trade.base_lots)?;
            let quote_lots = parse_i128_generated(&trade.quote_lots)?;
            let entry = buckets.entry(bucket_start).or_insert(MarketCandle {
                market: market.to_string(),
                bucket_start_ts_ms: bucket_start,
                resolution_sec: safe_resolution,
                open_price_lots: trade.price_lots.clone(),
                high_price_lots: trade.price_lots.clone(),
                low_price_lots: trade.price_lots.clone(),
                close_price_lots: trade.price_lots.clone(),
                base_volume_lots: "0".to_string(),
                quote_volume_lots: "0".to_string(),
                trade_count: 0,
                view,
            });

            entry.close_price_lots = trade.price_lots.clone();
            if price > parse_i128_generated(&entry.high_price_lots)? {
                entry.high_price_lots = trade.price_lots.clone();
            }
            if price < parse_i128_generated(&entry.low_price_lots)? {
                entry.low_price_lots = trade.price_lots.clone();
            }
            let current_base = parse_i128_generated(&entry.base_volume_lots)?;
            let current_quote = parse_i128_generated(&entry.quote_volume_lots)?;
            entry.base_volume_lots = (current_base + base_lots).to_string();
            entry.quote_volume_lots = (current_quote + quote_lots).to_string();
            entry.trade_count += 1;
        }

        let candles: Vec<_> = buckets.into_values().collect();
        let start = candles.len().saturating_sub(limit);
        Ok(candles[start..].to_vec())
    }

    pub fn get_candles_json(
        &mut self,
        market: &str,
        view: QueueView,
        resolution_sec: u64,
        limit: usize,
    ) -> Result<String> {
        Ok(serde_json::to_string(&self.get_candles(
            market,
            view,
            resolution_sec,
            limit,
        )?)?)
    }

    pub fn get_snapshot(&mut self, view: QueueView) -> Result<EngineSnapshot> {
        Ok(self.view_state(view)?.snapshot.clone())
    }

    pub fn get_snapshot_json(&mut self, view: QueueView) -> Result<String> {
        Ok(serde_json::to_string(&self.get_snapshot(view)?)?)
    }

    fn view_state(&mut self, view: QueueView) -> Result<&ViewState> {
        let revision = self.revision;
        match view {
            QueueView::Confirmed => {
                let rebuild = self
                    .cached_confirmed
                    .as_ref()
                    .map(|cache| cache.revision != revision)
                    .unwrap_or(true);
                if rebuild {
                    let state = self.build_view_state(QueueView::Confirmed)?;
                    self.cached_confirmed = Some(ViewStateCache { revision, state });
                }
                Ok(&self.cached_confirmed.as_ref().unwrap().state)
            }
            QueueView::Optimistic => {
                let rebuild = self
                    .cached_optimistic
                    .as_ref()
                    .map(|cache| cache.revision != revision)
                    .unwrap_or(true);
                if rebuild {
                    let state = self.build_view_state(QueueView::Optimistic)?;
                    self.cached_optimistic = Some(ViewStateCache { revision, state });
                }
                Ok(&self.cached_optimistic.as_ref().unwrap().state)
            }
        }
    }

    fn build_view_state(&self, view: QueueView) -> Result<ViewState> {
        let now_ts_ms = now_ts_ms();
        let now_ts = now_ts_ms / 1_000;
        let mut projection = self.build_projection(view)?;
        projection.engine.prune_expired_orders(now_ts)?;
        let snapshot = self.snapshot_from_projection(view, &projection, now_ts_ms, now_ts)?;
        Ok(ViewState {
            snapshot,
            trades: projection.trades,
        })
    }

    fn build_projection(&self, view: QueueView) -> Result<Projection> {
        let mut projection = Projection {
            engine: HarnessEngine::new(),
            active_markets: BTreeSet::new(),
            owner_accounts: self.baseline_user_accounts.clone(),
            account_identities: HashMap::new(),
            queue: BTreeMap::new(),
            trades: BTreeMap::new(),
            last_slot: self.last_seen_slot,
        };

        for market in self.baseline_confirmed_seq.keys() {
            projection.active_markets.insert(market.clone());
        }

        for baseline_order in self.baseline_orders.values() {
            projection
                .active_markets
                .insert(baseline_order.market.clone());
            projection
                .owner_accounts
                .entry(baseline_order.owner.clone())
                .or_default()
                .insert(baseline_order.mango_account.clone());

            let market_index = parse_market_index(&baseline_order.market)?;
            let owner_pubkey = parse_pubkey("baseline_order.owner", &baseline_order.owner)?;
            let mango_account_pubkey = parse_pubkey(
                "baseline_order.mango_account",
                &baseline_order.mango_account,
            )?;
            projection.account_identities.insert(
                mango_account_pubkey,
                AccountIdentity {
                    owner: baseline_order.owner.clone(),
                },
            );

            ensure_projection_market(&mut projection.engine, market_index)?;
            projection.engine.set_oracle_price(
                market_index,
                parse_i64_field("baseline_order.price_lots", &baseline_order.price_lots)?.max(1)
                    as f64,
            )?;
            projection
                .engine
                .ensure_account(mango_account_pubkey, owner_pubkey)?;
            projection.engine.import_open_order(
                market_index,
                mango_account_pubkey,
                parse_side("baseline_order.side", &baseline_order.side)?,
                parse_u128_field("baseline_order.order_id", &baseline_order.order_id)?,
                parse_u64_field(
                    "baseline_order.client_order_id",
                    &baseline_order.client_order_id,
                )?,
                parse_i64_field("baseline_order.price_lots", &baseline_order.price_lots)?,
                parse_i64_field("baseline_order.base_lots", &baseline_order.base_lots)?,
                parse_u64_field(
                    "baseline_order.expiry_timestamp",
                    &baseline_order.expiry_timestamp,
                )?,
            )?;
        }

        let intents = self.sorted_replay_intents();

        for intent in intents
            .iter()
            .copied()
            .filter(|intent| intent.processed_status == Some(QUEUE_PROCESS_EXECUTED))
        {
            self.apply_intent(intent, &mut projection)?;
            let queue = projection
                .queue
                .entry(intent.market.clone())
                .or_insert_with(QueueAggregate::default);
            queue.processed_count += 1;
            queue.last_processed_sequence = queue.last_processed_sequence.max(intent.sequence);
        }

        for intent in &intents {
            let queue = projection
                .queue
                .entry(intent.market.clone())
                .or_insert_with(QueueAggregate::default);
            if intent.market == "unknown" && intent.processed_status.is_some() {
                queue.unmatched_processed_count += 1;
            }
            match intent.processed_status {
                Some(QUEUE_PROCESS_FAILED) => {
                    queue.processed_count += 1;
                    queue.failed_count += 1;
                }
                Some(QUEUE_PROCESS_SKIPPED) => {
                    queue.processed_count += 1;
                    queue.skipped_count += 1;
                }
                None => {
                    queue.pending_count += 1;
                    queue.min_pending_execute_slot = Some(match queue.min_pending_execute_slot {
                        Some(existing) => existing.min(intent.min_execute_slot),
                        None => intent.min_execute_slot,
                    });
                }
                _ => {}
            }
        }

        if view == QueueView::Optimistic {
            for intent in intents
                .iter()
                .copied()
                .filter(|intent| intent.processed_status.is_none())
            {
                self.apply_intent(intent, &mut projection)?;
            }
        }

        Ok(projection)
    }

    fn apply_intent(&self, intent: &CanonicalIntent, projection: &mut Projection) -> Result<()> {
        let Some(payload) = intent.decoded_payload else {
            return Ok(());
        };

        projection.active_markets.insert(intent.market.clone());
        projection
            .owner_accounts
            .entry(intent.user_owner.clone())
            .or_default()
            .insert(intent.mango_account.clone());

        if let Some(mango_account_pubkey) = intent.mango_account_pubkey {
            projection.account_identities.insert(
                mango_account_pubkey,
                AccountIdentity {
                    owner: intent.user_owner.clone(),
                },
            );
        }

        match payload {
            QueuePayload::LiquidityDeposit(_) | QueuePayload::LiquidityWithdraw(_) => {
                return Ok(());
            }
            _ => {}
        }

        let market_index = intent
            .market_index
            .ok_or_else(|| HarnessError::InvalidInteger {
                field: "market",
                value: intent.market.clone(),
            })?;
        let owner_pubkey = intent
            .user_owner_pubkey
            .ok_or_else(|| HarnessError::InvalidPubkey {
                field: "user_owner",
                value: intent.user_owner.clone(),
            })?;
        let mango_account_pubkey =
            intent
                .mango_account_pubkey
                .ok_or_else(|| HarnessError::InvalidPubkey {
                    field: "mango_account",
                    value: intent.mango_account.clone(),
                })?;

        ensure_projection_market(&mut projection.engine, market_index)?;
        if let QueuePayload::PerpPlaceOrderV2(place) = payload {
            projection
                .engine
                .set_oracle_price(market_index, place.price_lots.max(1) as f64)?;
        }
        projection
            .engine
            .ensure_account(mango_account_pubkey, owner_pubkey)?;
        projection
            .engine
            .prune_expired_orders(intent.accepted_ts_ms / 1_000)?;

        let result = projection.engine.execute_decoded_payload(
            market_index,
            mango_account_pubkey,
            payload,
            intent.accepted_ts_ms / 1_000,
        )?;
        self.record_execution_trades(intent, projection, result);
        Ok(())
    }

    fn record_execution_trades(
        &self,
        intent: &CanonicalIntent,
        projection: &mut Projection,
        result: ExecutionResult,
    ) {
        if result.fills.is_empty() {
            return;
        }

        let market_trades = projection.trades.entry(intent.market.clone()).or_default();
        for fill in result.fills {
            let maker_owner = projection
                .account_identities
                .get(&fill.maker)
                .map(|identity| identity.owner.clone())
                .unwrap_or_else(|| fill.maker.to_string());
            let taker_owner = projection
                .account_identities
                .get(&fill.taker)
                .map(|identity| identity.owner.clone())
                .unwrap_or_else(|| fill.taker.to_string());
            let ordinal = market_trades.len();
            market_trades.push(InternalTrade {
                trade_id: format!(
                    "{}:{}:{}:{}:{}",
                    intent.group, intent.market, intent.sequence, fill.maker_order_id, ordinal
                ),
                market: intent.market.clone(),
                price_lots: fill.price_lots,
                base_lots: fill.base_lots,
                quote_lots: fill.quote_lots,
                taker_side: side_to_str(fill.taker_side).to_string(),
                maker_owner,
                taker_owner,
                maker_order_id: fill.maker_order_id.to_string(),
                taker_sequence: intent.sequence,
                ts_ms: intent.accepted_ts_ms,
            });
        }
        if market_trades.len() > MAX_TRADE_HISTORY {
            let drain = market_trades.len() - MAX_TRADE_HISTORY;
            market_trades.drain(0..drain);
        }
    }

    fn snapshot_from_projection(
        &self,
        view: QueueView,
        projection: &Projection,
        generated_ts_ms: u64,
        now_ts: u64,
    ) -> Result<EngineSnapshot> {
        let mut markets = HashMap::new();
        for market in &projection.active_markets {
            let (bids, asks, open_orders) = if let Ok(market_index) = parse_market_index(market) {
                if projection.engine.has_market(market_index) {
                    let book = projection.engine.orderbook_snapshot(market_index, now_ts)?;
                    let orders = projection
                        .engine
                        .open_orders_snapshot_for_market(market_index, now_ts)?;
                    (
                        book.bids
                            .into_iter()
                            .map(|level| MarketLevel {
                                price_lots: level.price_lots.to_string(),
                                base_lots: level.base_lots.to_string(),
                            })
                            .collect(),
                        book.asks
                            .into_iter()
                            .map(|level| MarketLevel {
                                price_lots: level.price_lots.to_string(),
                                base_lots: level.base_lots.to_string(),
                            })
                            .collect(),
                        orders
                            .into_iter()
                            .map(open_order_snapshot_to_summary)
                            .collect(),
                    )
                } else {
                    (Vec::new(), Vec::new(), Vec::new())
                }
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };

            let (optimistic_seq, confirmed_seq) = self.market_watermarks(market);
            markets.insert(
                market.clone(),
                MarketState {
                    market: market.clone(),
                    bids,
                    asks,
                    open_orders,
                    watermarks: MarketWatermarks {
                        optimistic_seq: optimistic_seq.to_string(),
                        confirmed_seq: confirmed_seq.to_string(),
                        last_slot: projection.last_slot.to_string(),
                    },
                },
            );
        }

        let mut all_owners: BTreeSet<String> = projection.owner_accounts.keys().cloned().collect();
        for (owner, _) in self.baseline_positions.keys() {
            all_owners.insert(owner.clone());
        }

        let mut users = HashMap::new();
        for owner in all_owners {
            let mango_accounts = projection
                .owner_accounts
                .get(&owner)
                .cloned()
                .unwrap_or_default();
            let mut open_orders = Vec::new();
            let mut per_market = BTreeMap::<String, UserPerMarketAggregate>::new();

            for mango_account in &mango_accounts {
                let mango_account_pubkey = match Pubkey::from_str(mango_account) {
                    Ok(key) => key,
                    Err(_) => continue,
                };
                let snapshot = match projection
                    .engine
                    .account_snapshot(mango_account_pubkey, now_ts)
                {
                    Ok(snapshot) => snapshot,
                    Err(HarnessError::UnknownAccount(_)) => continue,
                    Err(err) => return Err(err),
                };
                self.merge_account_snapshot(&snapshot, &mut open_orders, &mut per_market);
            }

            for ((entry_owner, market), position) in &self.baseline_positions {
                if entry_owner != &owner {
                    continue;
                }
                let entry = per_market.entry(market.clone()).or_default();
                entry.base_position_lots += position.base_position_lots as i128;
                entry.quote_position_native += position.quote_position_native;
            }

            open_orders.sort_by(order_summary_cmp);
            let mut per_market_vec: Vec<_> = per_market
                .into_iter()
                .map(|(market, aggregate)| UserPerMarketState {
                    market,
                    open_order_base_lots_bid: aggregate.open_bid.to_string(),
                    open_order_base_lots_ask: aggregate.open_ask.to_string(),
                    quote_reserved_lots: aggregate.quote_reserved.to_string(),
                    base_position_lots: aggregate.base_position_lots.to_string(),
                    quote_position_native: aggregate.quote_position_native.to_string(),
                })
                .collect();
            per_market_vec.sort_by(|a, b| compare_market_strings(&a.market, &b.market));

            users.insert(
                owner.clone(),
                UserState {
                    owner: owner.clone(),
                    mango_accounts: mango_accounts.into_iter().collect(),
                    open_orders,
                    per_market: per_market_vec,
                    margin_summary: placeholder_margin_summary(),
                },
            );
        }

        let mut queue = HashMap::new();
        for (market, aggregate) in &projection.queue {
            let lag_slots = aggregate
                .min_pending_execute_slot
                .filter(|min_pending| projection.last_slot > *min_pending)
                .map(|min_pending| projection.last_slot - min_pending)
                .unwrap_or(0);
            queue.insert(
                market.clone(),
                crate::QueueState {
                    market: market.clone(),
                    pending_count: aggregate.pending_count,
                    processed_count: aggregate.processed_count,
                    failed_count: aggregate.failed_count,
                    skipped_count: aggregate.skipped_count,
                    last_processed_sequence: aggregate.last_processed_sequence.to_string(),
                    lag_slots: lag_slots.to_string(),
                    unmatched_processed_count: aggregate.unmatched_processed_count,
                },
            );
        }

        Ok(EngineSnapshot {
            view,
            markets,
            users,
            queue,
            generated_ts_ms,
        })
    }

    fn merge_account_snapshot(
        &self,
        snapshot: &AccountSnapshot,
        open_orders: &mut Vec<OpenOrderSummary>,
        per_market: &mut BTreeMap<String, UserPerMarketAggregate>,
    ) {
        for order in &snapshot.open_orders {
            open_orders.push(open_order_snapshot_to_summary(order.clone()));
            let market_key = order.market_index.to_string();
            let entry = per_market.entry(market_key).or_default();
            entry.quote_reserved += order.quote_lots as i128;
        }

        for position in &snapshot.perp_positions {
            let market_key = position.market_index.to_string();
            let entry = per_market.entry(market_key).or_default();
            entry.open_bid += position.open_bid_base_lots as i128;
            entry.open_ask += position.open_ask_base_lots as i128;
            entry.base_position_lots += position.base_position_lots as i128;
            entry.quote_position_native +=
                I80F48::from_str(&position.quote_position_native).unwrap_or(I80F48::ZERO);
        }
    }

    fn market_watermarks(&self, market: &str) -> (u64, u64) {
        let baseline = self
            .baseline_confirmed_seq
            .get(market)
            .copied()
            .unwrap_or(0);
        let mut optimistic = baseline;
        let mut confirmed = baseline;
        for intent in self.intents_by_key.values() {
            if intent.market != market || intent.kind != QUEUE_ITEM_KIND_CTM_WRAPPED {
                continue;
            }
            if intent.sequence > optimistic {
                optimistic = intent.sequence;
            }
            if intent.processed_status == Some(QUEUE_PROCESS_EXECUTED)
                && intent.sequence > confirmed
            {
                confirmed = intent.sequence;
            }
        }
        (optimistic, confirmed)
    }

    fn sorted_replay_intents(&self) -> Vec<&CanonicalIntent> {
        let mut intents: Vec<_> = self
            .intents_by_key
            .values()
            .filter(|intent| intent.sequence > self.baseline_seq_for_market(&intent.market))
            .collect();
        intents.sort_by(order_intents_deterministically);
        intents
    }

    fn baseline_seq_for_market(&self, market: &str) -> u64 {
        if self.baseline_bootstrapped {
            self.baseline_confirmed_seq
                .get(market)
                .copied()
                .unwrap_or(0)
        } else {
            0
        }
    }

    fn emit_divergence(&mut self, reason: &str, key: &str, details: HashMap<String, String>) {
        self.divergences.push(DivergenceEvent {
            event_type: "divergence_event".to_string(),
            ts_ms: now_ts_ms(),
            reason: reason.to_string(),
            key: key.to_string(),
            details,
        });
    }

    fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.cached_confirmed = None;
        self.cached_optimistic = None;
    }
}

fn ensure_projection_market(engine: &mut HarnessEngine, market_index: u16) -> Result<()> {
    if !engine.has_market(market_index) {
        engine.register_market(market_index, MarketConfig::default())?;
    }
    Ok(())
}

fn placeholder_margin_summary() -> MarginSummary {
    MarginSummary::Placeholder(MarginSummaryPlaceholder {
        source: "queue-replay".to_string(),
    })
}

fn empty_market_state(market: &str, last_seen_slot: u64) -> MarketState {
    MarketState {
        market: market.to_string(),
        bids: Vec::new(),
        asks: Vec::new(),
        open_orders: Vec::new(),
        watermarks: MarketWatermarks {
            optimistic_seq: "0".to_string(),
            confirmed_seq: "0".to_string(),
            last_slot: last_seen_slot.to_string(),
        },
    }
}

fn empty_user_state(owner: &str) -> UserState {
    UserState {
        owner: owner.to_string(),
        mango_accounts: Vec::new(),
        open_orders: Vec::new(),
        per_market: Vec::new(),
        margin_summary: placeholder_margin_summary(),
    }
}

fn empty_queue_state(market: &str) -> crate::QueueState {
    crate::QueueState {
        market: market.to_string(),
        pending_count: 0,
        processed_count: 0,
        failed_count: 0,
        skipped_count: 0,
        last_processed_sequence: "0".to_string(),
        lag_slots: "0".to_string(),
        unmatched_processed_count: 0,
    }
}

fn open_order_snapshot_to_summary(order: crate::OpenOrderSnapshot) -> OpenOrderSummary {
    OpenOrderSummary {
        order_id: order.order_id.to_string(),
        owner: order.owner.to_string(),
        mango_account: order.mango_account.to_string(),
        market: order.market_index.to_string(),
        side: side_to_str(order.side).to_string(),
        price_lots: order.price_lots.to_string(),
        base_lots: order.base_lots.to_string(),
        quote_lots: order.quote_lots.to_string(),
        client_order_id: order.client_order_id.to_string(),
        sequence: restored_sequence(order.side, order.order_id).to_string(),
        expiry_timestamp: order.expiry_timestamp.to_string(),
        status: "open".to_string(),
    }
}

fn restored_sequence(side: Side, order_id: u128) -> u64 {
    let low = order_id as u64;
    match side {
        Side::Bid => !low,
        Side::Ask => low,
    }
}

fn queue_item_key(group: &str, sequence: u64, kind: u8) -> String {
    format!("{group}:{sequence}:{kind}")
}

fn parse_market_index(value: &str) -> Result<u16> {
    value.parse().map_err(|_| HarnessError::InvalidInteger {
        field: "market",
        value: value.to_string(),
    })
}

fn parse_pubkey(field: &'static str, value: &str) -> Result<Pubkey> {
    Pubkey::from_str(value).map_err(|_| HarnessError::InvalidPubkey {
        field,
        value: value.to_string(),
    })
}

fn parse_u64_field(field: &'static str, value: &str) -> Result<u64> {
    value.parse().map_err(|_| HarnessError::InvalidInteger {
        field,
        value: value.to_string(),
    })
}

fn parse_u128_field(field: &'static str, value: &str) -> Result<u128> {
    value.parse().map_err(|_| HarnessError::InvalidInteger {
        field,
        value: value.to_string(),
    })
}

fn parse_i64_field(field: &'static str, value: &str) -> Result<i64> {
    value.parse().map_err(|_| HarnessError::InvalidInteger {
        field,
        value: value.to_string(),
    })
}

fn parse_i80f48_field(field: &'static str, value: &str) -> Result<I80F48> {
    I80F48::from_str(value).map_err(|_| HarnessError::InvalidInteger {
        field,
        value: value.to_string(),
    })
}

fn parse_i128_generated(value: &str) -> Result<i128> {
    value.parse().map_err(|_| HarnessError::InvalidInteger {
        field: "generated_value",
        value: value.to_string(),
    })
}

fn parse_side(field: &'static str, value: &str) -> Result<Side> {
    match value {
        "bid" => Ok(Side::Bid),
        "ask" => Ok(Side::Ask),
        _ => Err(HarnessError::InvalidInteger {
            field,
            value: value.to_string(),
        }),
    }
}

fn side_to_str(side: Side) -> &'static str {
    match side {
        Side::Bid => "bid",
        Side::Ask => "ask",
    }
}

fn decoded_payload_to_json(payload: QueuePayload) -> Value {
    match payload {
        QueuePayload::PerpPlaceOrderV2(place) => json!({
            "variant": 0,
            "side": match place.side { Side::Bid => 0, Side::Ask => 1 },
            "price_lots": place.price_lots.to_string(),
            "max_base_lots": place.max_base_lots.to_string(),
            "max_quote_lots": place.max_quote_lots.to_string(),
            "client_order_id": place.client_order_id.to_string(),
            "order_type": place_order_type_code(place.order_type),
            "self_trade_behavior": self_trade_behavior_code(place.self_trade_behavior),
            "reduce_only": place.reduce_only,
            "expiry_timestamp": place.expiry_timestamp.to_string(),
            "limit": place.limit,
        }),
        QueuePayload::PerpCancelOrder(cancel) => json!({
            "variant": 1,
            "order_id": cancel.order_id.to_string(),
        }),
        QueuePayload::PerpCancelOrderByClientOrderId(cancel) => json!({
            "variant": 2,
            "client_order_id": cancel.client_order_id.to_string(),
        }),
        QueuePayload::PerpCancelAllOrders(cancel) => json!({
            "variant": 3,
            "limit": cancel.limit,
        }),
        QueuePayload::PerpCancelAllOrdersBySide(cancel) => json!({
            "variant": 4,
            "side_option": cancel.side.map(|side| if side == Side::Bid { 0 } else { 1 }),
            "limit": cancel.limit,
        }),
        QueuePayload::LiquidityDeposit(payload) => json!({
            "variant": 5,
            "amount": payload.amount.to_string(),
            "reduce_only": payload.reduce_only,
        }),
        QueuePayload::LiquidityWithdraw(payload) => json!({
            "variant": 6,
            "amount": payload.amount.to_string(),
            "allow_borrow": payload.allow_borrow,
        }),
    }
}

fn place_order_type_code(order_type: PlaceOrderType) -> u8 {
    order_type as u8
}

fn self_trade_behavior_code(self_trade_behavior: SelfTradeBehavior) -> u8 {
    self_trade_behavior as u8
}

fn order_intents_deterministically(a: &&CanonicalIntent, b: &&CanonicalIntent) -> Ordering {
    a.group
        .cmp(&b.group)
        .then_with(|| compare_market_strings(&a.market, &b.market))
        .then_with(|| a.kind.cmp(&b.kind))
        .then_with(|| {
            if a.kind == QUEUE_ITEM_KIND_CTM_WRAPPED && b.kind == QUEUE_ITEM_KIND_CTM_WRAPPED {
                a.sequence.cmp(&b.sequence)
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| a.accepted_ts_ms.cmp(&b.accepted_ts_ms))
        .then_with(|| a.key.cmp(&b.key))
}

fn order_summary_cmp(a: &OpenOrderSummary, b: &OpenOrderSummary) -> Ordering {
    compare_market_strings(&a.market, &b.market)
        .then_with(|| side_rank(&a.side).cmp(&side_rank(&b.side)))
        .then_with(|| {
            let a_price = parse_i128_generated(&a.price_lots).unwrap_or_default();
            let b_price = parse_i128_generated(&b.price_lots).unwrap_or_default();
            a_price.cmp(&b_price)
        })
        .then_with(|| a.order_id.cmp(&b.order_id))
}

fn side_rank(side: &str) -> u8 {
    match side {
        "bid" => 0,
        "ask" => 1,
        _ => u8::MAX,
    }
}

fn compare_market_strings(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a_num), Ok(b_num)) => a_num.cmp(&b_num),
        _ => a.cmp(b),
    }
}

fn now_ts_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    fn place_order_payload(
        side: Side,
        price_lots: i64,
        max_base_lots: i64,
        max_quote_lots: i64,
        client_order_id: u64,
        order_type: PlaceOrderType,
        self_trade_behavior: SelfTradeBehavior,
        reduce_only: bool,
        expiry_timestamp: u64,
        limit: u8,
    ) -> String {
        let mut payload = Vec::with_capacity(4 + 45);
        payload.push(1);
        payload.push(0);
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.push(match side {
            Side::Bid => 0,
            Side::Ask => 1,
        });
        payload.extend_from_slice(&price_lots.to_le_bytes());
        payload.extend_from_slice(&max_base_lots.to_le_bytes());
        payload.extend_from_slice(&max_quote_lots.to_le_bytes());
        payload.extend_from_slice(&client_order_id.to_le_bytes());
        payload.push(order_type as u8);
        payload.push(self_trade_behavior as u8);
        payload.push(u8::from(reduce_only));
        payload.extend_from_slice(&expiry_timestamp.to_le_bytes());
        payload.push(limit);
        BASE64_STANDARD.encode(payload)
    }

    fn cancel_all_payload(limit: u8) -> String {
        let mut payload = Vec::with_capacity(5);
        payload.push(1);
        payload.push(3);
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.push(limit);
        BASE64_STANDARD.encode(payload)
    }

    fn key() -> String {
        Pubkey::new_unique().to_string()
    }

    fn run_with_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .name("rust-harness-replay-test".to_string())
            .stack_size(32 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn builds_optimistic_and_confirmed_views_from_relay_and_processed_events() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "0".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1_000,
                    group: group.clone(),
                    execution_queue: execution_queue.clone(),
                    market: market.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Bid,
                        100,
                        2,
                        100,
                        1,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "10".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "tx-enqueue-1".to_string(),
                })
                .unwrap();

            assert_eq!(
                engine
                    .get_market_state(&market, QueueView::Confirmed)
                    .unwrap()
                    .open_orders
                    .len(),
                0
            );
            assert_eq!(
                engine
                    .get_market_state(&market, QueueView::Optimistic)
                    .unwrap()
                    .open_orders
                    .len(),
                1
            );

            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 2_000,
                    group: group.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "20".to_string(),
                    tx_signature: "tx-exec-1".to_string(),
                })
                .unwrap();
            assert_eq!(
                engine
                    .get_market_state(&market, QueueView::Confirmed)
                    .unwrap()
                    .open_orders
                    .len(),
                1
            );

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 3_000,
                    group: group.clone(),
                    execution_queue,
                    market: market.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    payload_b64: cancel_all_payload(20),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "10".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner,
                    mango_account,
                    enqueue_tx_signature: "tx-enqueue-2".to_string(),
                })
                .unwrap();
            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 4_000,
                    group,
                    sequence: "2".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "21".to_string(),
                    tx_signature: "tx-exec-2".to_string(),
                })
                .unwrap();

            assert_eq!(
                engine
                    .get_market_state(&market, QueueView::Confirmed)
                    .unwrap()
                    .open_orders
                    .len(),
                0
            );
        });
    }

    #[test]
    fn replays_deterministically_regardless_of_ingestion_order() {
        run_with_large_stack(|| {
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "7".to_string();
            let place_payload = place_order_payload(
                Side::Ask,
                88,
                3,
                100,
                44,
                PlaceOrderType::Limit,
                SelfTradeBehavior::DecrementTake,
                false,
                0,
                10,
            );
            let cancel_payload = cancel_all_payload(10);

            let mut ordered = ContinuumStateEngine::new();
            for event in [
                RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group: group.clone(),
                    execution_queue: execution_queue.clone(),
                    market: market.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    payload_b64: place_payload.clone(),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "a".to_string(),
                },
                RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 2,
                    group: group.clone(),
                    execution_queue: execution_queue.clone(),
                    market: market.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    payload_b64: cancel_payload.clone(),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "b".to_string(),
                },
            ] {
                ordered.ingest_relay_intent(event).unwrap();
            }
            for event in [
                QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 3,
                    group: group.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "10".to_string(),
                    tx_signature: "c".to_string(),
                },
                QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 4,
                    group: group.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "11".to_string(),
                    tx_signature: "d".to_string(),
                },
            ] {
                ordered.ingest_queue_processed(event).unwrap();
            }

            let mut out_of_order = ContinuumStateEngine::new();
            out_of_order
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 2,
                    group: group.clone(),
                    execution_queue,
                    market: market.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    payload_b64: cancel_payload,
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "b".to_string(),
                })
                .unwrap();
            out_of_order
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 4,
                    group: group.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "11".to_string(),
                    tx_signature: "d".to_string(),
                })
                .unwrap();
            out_of_order
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group: group.clone(),
                    execution_queue: key(),
                    market: market.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    payload_b64: place_payload,
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "a".to_string(),
                })
                .unwrap();
            out_of_order
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 3,
                    group,
                    sequence: "1".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "10".to_string(),
                    tx_signature: "c".to_string(),
                })
                .unwrap();

            let mut ordered_snapshot = ordered.get_snapshot(QueueView::Confirmed).unwrap();
            let mut out_of_order_snapshot =
                out_of_order.get_snapshot(QueueView::Confirmed).unwrap();
            ordered_snapshot.generated_ts_ms = 0;
            out_of_order_snapshot.generated_ts_ms = 0;
            assert_eq!(ordered_snapshot, out_of_order_snapshot);
        });
    }

    #[test]
    fn tracks_processed_without_relay_intent_divergence() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();

            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 1,
                    group: group.clone(),
                    sequence: "99".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_FAILED,
                    slot: "123".to_string(),
                    tx_signature: "unknown-processed".to_string(),
                })
                .unwrap();

            let divergences = engine.list_divergences(10);
            assert_eq!(divergences.len(), 1);
            assert_eq!(divergences[0].reason, "processed_without_relay_intent");
            assert_eq!(
                engine
                    .get_queue_state("unknown")
                    .unwrap()
                    .unmatched_processed_count,
                1
            );
        });
    }

    #[test]
    fn builds_trade_prints_and_candles_from_crossing_orders() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let maker_owner = key();
            let taker_owner = key();
            let maker_mango = key();
            let taker_mango = key();
            let market = "0".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1_000,
                    group: group.clone(),
                    execution_queue: execution_queue.clone(),
                    market: market.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Bid,
                        100,
                        2,
                        500,
                        1,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: maker_owner.clone(),
                    mango_account: maker_mango.clone(),
                    enqueue_tx_signature: "mk".to_string(),
                })
                .unwrap();
            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 2_000,
                    group: group.clone(),
                    execution_queue,
                    market: market.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Ask,
                        99,
                        1,
                        500,
                        2,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: taker_owner.clone(),
                    mango_account: taker_mango.clone(),
                    enqueue_tx_signature: "tk".to_string(),
                })
                .unwrap();
            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 3_000,
                    group: group.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "10".to_string(),
                    tx_signature: "mk-exec".to_string(),
                })
                .unwrap();
            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 4_000,
                    group,
                    sequence: "2".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "11".to_string(),
                    tx_signature: "tk-exec".to_string(),
                })
                .unwrap();

            let trades = engine
                .get_trades(&market, QueueView::Confirmed, 10)
                .unwrap();
            assert_eq!(trades.len(), 1);
            assert_eq!(trades[0].price_lots, "100");
            assert_eq!(trades[0].base_lots, "1");
            assert_eq!(trades[0].taker_side, "ask");

            let candles = engine
                .get_candles(&market, QueueView::Confirmed, 60, 10)
                .unwrap();
            assert_eq!(candles.len(), 1);
            assert_eq!(candles[0].open_price_lots, "100");
            assert_eq!(candles[0].close_price_lots, "100");
            assert_eq!(candles[0].base_volume_lots, "1");

            let balances = engine
                .get_balances(&maker_owner, QueueView::Confirmed)
                .unwrap();
            assert_eq!(balances.totals.total_open_order_base_lots_bid, "1");
        });
    }

    #[test]
    fn deduplicates_processed_events_and_tracks_skips() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "12".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group: group.clone(),
                    execution_queue,
                    market: market.clone(),
                    sequence: "5".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Bid,
                        100,
                        1,
                        100,
                        5,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner,
                    mango_account,
                    enqueue_tx_signature: "enqueue-5".to_string(),
                })
                .unwrap();

            let processed = QueueItemProcessedEvent {
                event_type: "queue_item_processed".to_string(),
                ts_ms: 2,
                group,
                sequence: "5".to_string(),
                kind: 0,
                status: QUEUE_PROCESS_SKIPPED,
                slot: "33".to_string(),
                tx_signature: "skip-5".to_string(),
            };
            engine.ingest_queue_processed(processed.clone()).unwrap();
            engine.ingest_queue_processed(processed).unwrap();

            let queue = engine.get_queue_state(&market).unwrap();
            assert_eq!(queue.processed_count, 1);
            assert_eq!(queue.skipped_count, 1);
            assert_eq!(queue.failed_count, 0);
            assert_eq!(queue.pending_count, 0);
            assert!(engine
                .get_market_state(&market, QueueView::Confirmed)
                .unwrap()
                .open_orders
                .is_empty());
        });
    }

    #[test]
    fn removes_optimistic_orders_after_failed_processed_status() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "14".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group: group.clone(),
                    execution_queue,
                    market: market.clone(),
                    sequence: "9".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Bid,
                        101,
                        4,
                        1_000,
                        9,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "5".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "enqueue-9".to_string(),
                })
                .unwrap();

            assert_eq!(
                engine
                    .get_market_state(&market, QueueView::Optimistic)
                    .unwrap()
                    .open_orders
                    .len(),
                1
            );

            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 2,
                    group,
                    sequence: "9".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_FAILED,
                    slot: "55".to_string(),
                    tx_signature: "failed-9".to_string(),
                })
                .unwrap();

            assert!(engine
                .get_market_state(&market, QueueView::Optimistic)
                .unwrap()
                .open_orders
                .is_empty());
            assert!(engine
                .get_market_state(&market, QueueView::Confirmed)
                .unwrap()
                .open_orders
                .is_empty());
            let queue = engine.get_queue_state(&market).unwrap();
            assert_eq!(queue.pending_count, 0);
            assert_eq!(queue.processed_count, 1);
            assert_eq!(queue.failed_count, 1);
        });
    }

    #[test]
    fn emits_divergence_when_processed_status_changes() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "13".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group: group.clone(),
                    execution_queue,
                    market,
                    sequence: "8".to_string(),
                    kind: 0,
                    payload_b64: cancel_all_payload(10),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "1".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner,
                    mango_account,
                    enqueue_tx_signature: "enqueue-8".to_string(),
                })
                .unwrap();
            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 2,
                    group: group.clone(),
                    sequence: "8".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_FAILED,
                    slot: "41".to_string(),
                    tx_signature: "failed-8".to_string(),
                })
                .unwrap();
            engine
                .ingest_queue_processed(QueueItemProcessedEvent {
                    event_type: "queue_item_processed".to_string(),
                    ts_ms: 3,
                    group: group.clone(),
                    sequence: "8".to_string(),
                    kind: 0,
                    status: QUEUE_PROCESS_EXECUTED,
                    slot: "42".to_string(),
                    tx_signature: "executed-8".to_string(),
                })
                .unwrap();

            let divergences: Vec<_> = engine
                .list_divergences(20)
                .into_iter()
                .filter(|event| event.reason == "processed_status_changed")
                .collect();
            assert_eq!(divergences.len(), 1);
            assert_eq!(divergences[0].key, format!("{}:8:0", group));
        });
    }

    #[test]
    fn state_projection_after_enqueue() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "20".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group,
                    execution_queue,
                    market: market.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Ask,
                        55,
                        10,
                        1_000,
                        42,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "10".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner,
                    mango_account,
                    enqueue_tx_signature: "tx-enqueue-state-1".to_string(),
                })
                .unwrap();

            let optimistic = engine
                .get_market_state(&market, QueueView::Optimistic)
                .unwrap();
            assert_eq!(optimistic.open_orders.len(), 1);
            assert_eq!(optimistic.open_orders[0].side, "ask");
            assert_eq!(optimistic.open_orders[0].price_lots, "55");
            assert_eq!(optimistic.open_orders[0].base_lots, "10");
            assert_eq!(optimistic.open_orders[0].client_order_id, "42");
            assert!(engine
                .get_market_state(&market, QueueView::Confirmed)
                .unwrap()
                .open_orders
                .is_empty());
        });
    }

    #[test]
    fn malformed_payload_is_graceful() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "21".to_string();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1,
                    group,
                    execution_queue,
                    market: market.clone(),
                    sequence: "1".to_string(),
                    kind: 0,
                    payload_b64: BASE64_STANDARD.encode([0xff, 0xff]),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "10".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner,
                    mango_account,
                    enqueue_tx_signature: "tx-malformed-1".to_string(),
                })
                .unwrap();

            let intents = engine.list_intents();
            assert_eq!(intents.len(), 1);
            assert!(intents[0].decoded_payload.is_none());
            assert!(engine
                .get_market_state(&market, QueueView::Optimistic)
                .unwrap()
                .open_orders
                .is_empty());
        });
    }

    #[test]
    fn bootstraps_baseline_orders_and_positions() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let owner = key();
            let mango_account = key();
            let market = "3".to_string();
            let order_id = ((100u128) << 64) | (!5u64 as u128);
            let mut users = HashMap::new();
            users.insert(
                owner.clone(),
                UserState {
                    owner: owner.clone(),
                    mango_accounts: vec![mango_account.clone()],
                    open_orders: Vec::new(),
                    per_market: vec![crate::types::UserPerMarketState {
                        market: market.clone(),
                        open_order_base_lots_bid: "0".to_string(),
                        open_order_base_lots_ask: "0".to_string(),
                        quote_reserved_lots: "0".to_string(),
                        base_position_lots: "7".to_string(),
                        quote_position_native: "-700".to_string(),
                    }],
                    margin_summary: placeholder_margin_summary(),
                },
            );
            let mut markets = HashMap::new();
            markets.insert(
                market.clone(),
                MarketState {
                    market: market.clone(),
                    bids: Vec::new(),
                    asks: Vec::new(),
                    open_orders: vec![OpenOrderSummary {
                        order_id: order_id.to_string(),
                        owner: owner.clone(),
                        mango_account: mango_account.clone(),
                        market: market.clone(),
                        side: "bid".to_string(),
                        price_lots: "100".to_string(),
                        base_lots: "2".to_string(),
                        quote_lots: "200".to_string(),
                        client_order_id: "99".to_string(),
                        sequence: "5".to_string(),
                        expiry_timestamp: "0".to_string(),
                        status: "open".to_string(),
                    }],
                    watermarks: crate::types::MarketWatermarks {
                        optimistic_seq: "5".to_string(),
                        confirmed_seq: "5".to_string(),
                        last_slot: "10".to_string(),
                    },
                },
            );

            engine
                .bootstrap_from_onchain_snapshot(EngineSnapshot {
                    view: QueueView::Confirmed,
                    markets,
                    users,
                    queue: HashMap::new(),
                    generated_ts_ms: 0,
                })
                .unwrap();

            let confirmed = engine
                .get_market_state(&market, QueueView::Confirmed)
                .unwrap();
            assert_eq!(confirmed.open_orders.len(), 1);
            assert_eq!(confirmed.watermarks.confirmed_seq, "5");
            let user = engine.get_user_state(&owner, QueueView::Confirmed).unwrap();
            assert_eq!(user.per_market[0].base_position_lots, "7");
            assert_eq!(user.open_orders.len(), 1);
        });
    }
}
