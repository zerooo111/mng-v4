use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    str::FromStr,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anchor_lang::prelude::Pubkey;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use fixed::types::I80F48;
use mango_v4::state::{PlaceOrderType, SelfTradeBehavior, Side};
use serde_json::{json, Value};

use crate::types::{
    AccountMetaWire, AccountPerpPositionState, AccountProjectedState,
    MarginSummaryAccountPerpPosition, MarginSummaryEmptyTotals, MarketLevel, MarketWatermarks,
    PerpMarketSyncState, TokenBankSyncState, UserBalanceTotals, UserPerMarketState,
};
use crate::{
    logging, CanonicalIntentState, DivergenceEvent, EngineSnapshot, ExecutionResult, HarnessEngine,
    HarnessError, MarginSummary, MarginSummaryAccount, MarginSummaryEmpty, MarginSummaryOk,
    MarginSummaryPlaceholder, MarketCandle, MarketConfig, MarketState, MarketTrade,
    OpenOrderSummary, QueueItemEnqueuedEvent, QueueItemProcessedEvent, QueuePayload, QueueView,
    RelayIntentAcceptedEvent, Result, UserBalances, UserState,
};

const QUEUE_ITEM_KIND_CTM_WRAPPED: u8 = 0;
const QUEUE_PROCESS_EXECUTED: u8 = 2;
const QUEUE_PROCESS_FAILED: u8 = 3;
const QUEUE_PROCESS_SKIPPED: u8 = 4;
#[cfg(not(test))]
const MAX_TRADE_HISTORY: usize = 5_000;
#[cfg(test)]
const MAX_TRADE_HISTORY: usize = 256;
#[cfg(not(test))]
const MAX_DIVERGENCE_HISTORY: usize = 2_048;
#[cfg(test)]
const MAX_DIVERGENCE_HISTORY: usize = 32;
#[cfg(not(test))]
const MAX_INTENT_HISTORY: usize = 20_000;
#[cfg(test)]
const MAX_INTENT_HISTORY: usize = 64;
#[cfg(not(test))]
const MAX_EVENT_ID_HISTORY: usize = 50_000;
#[cfg(test)]
const MAX_EVENT_ID_HISTORY: usize = 64;
const RUST_REPLAY_MARGIN_SOURCE: &str = "rust-replay-perp-token-health";
const RUST_REPLAY_MARGIN_SOURCE_PARTIAL: &str = "rust-replay-perp-token-health-partial";

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

#[derive(Debug, Clone, Copy)]
enum ReplayHealthType {
    Init,
    Maint,
}

#[derive(Debug, Clone, Copy)]
struct ReplayPrices {
    oracle: I80F48,
    stable: I80F48,
}

impl ReplayPrices {
    fn liab(self, health_type: ReplayHealthType) -> I80F48 {
        match health_type {
            ReplayHealthType::Maint => self.oracle,
            ReplayHealthType::Init => self.oracle.max(self.stable),
        }
    }

    fn asset(self, health_type: ReplayHealthType) -> I80F48 {
        match health_type {
            ReplayHealthType::Maint => self.oracle,
            ReplayHealthType::Init => self.oracle.min(self.stable),
        }
    }
}

#[derive(Debug, Clone)]
struct ReplayTokenInfo {
    maint_asset_weight: I80F48,
    init_scaled_asset_weight: I80F48,
    maint_liab_weight: I80F48,
    init_scaled_liab_weight: I80F48,
    prices: ReplayPrices,
}

impl ReplayTokenInfo {
    fn asset_weight(&self, health_type: ReplayHealthType) -> I80F48 {
        match health_type {
            ReplayHealthType::Init => self.init_scaled_asset_weight,
            ReplayHealthType::Maint => self.maint_asset_weight,
        }
    }

    fn liab_weight(&self, health_type: ReplayHealthType) -> I80F48 {
        match health_type {
            ReplayHealthType::Init => self.init_scaled_liab_weight,
            ReplayHealthType::Maint => self.maint_liab_weight,
        }
    }

    fn asset_weighted_price(&self, health_type: ReplayHealthType) -> I80F48 {
        self.asset_weight(health_type) * self.prices.asset(health_type)
    }

    fn liab_weighted_price(&self, health_type: ReplayHealthType) -> I80F48 {
        self.liab_weight(health_type) * self.prices.liab(health_type)
    }
}

#[derive(Debug, Clone)]
struct ReplayPerpInfo {
    settle_token_index: u64,
    maint_base_asset_weight: I80F48,
    init_base_asset_weight: I80F48,
    maint_base_liab_weight: I80F48,
    init_base_liab_weight: I80F48,
    maint_overall_asset_weight: I80F48,
    init_overall_asset_weight: I80F48,
    base_lot_size: i64,
    quote_lot_size: i64,
    long_funding: I80F48,
    short_funding: I80F48,
    base_prices: ReplayPrices,
    base_lots: i64,
    bids_base_lots: i64,
    asks_base_lots: i64,
    quote_current: I80F48,
}

impl ReplayPerpInfo {
    fn with_position(mut self, position: &AccountPerpPositionState) -> Result<Self> {
        let base_position_lots =
            parse_i64_field("base_position_lots", &position.base_position_lots)?;
        let taker_base_lots = parse_i64_field("taker_base_lots", &position.taker_base_lots)?;
        let long_settled_funding =
            parse_i80f48_field("long_settled_funding", &position.long_settled_funding)?;
        let short_settled_funding =
            parse_i80f48_field("short_settled_funding", &position.short_settled_funding)?;
        let unsettled_funding = if base_position_lots > 0 {
            (self.long_funding - long_settled_funding) * I80F48::from_num(base_position_lots)
        } else if base_position_lots < 0 {
            (self.short_funding - short_settled_funding) * I80F48::from_num(base_position_lots)
        } else {
            I80F48::ZERO
        };
        let taker_quote_native =
            parse_i80f48_field("taker_quote_lots", &position.taker_quote_lots)?
                * I80F48::from_num(self.quote_lot_size);

        self.base_lots = base_position_lots + taker_base_lots;
        self.bids_base_lots = parse_i64_field("open_bid_base_lots", &position.open_bid_base_lots)?;
        self.asks_base_lots = parse_i64_field("open_ask_base_lots", &position.open_ask_base_lots)?;
        self.quote_current =
            parse_i80f48_field("quote_position_native", &position.quote_position_native)?
                - unsettled_funding
                + taker_quote_native;
        Ok(self)
    }

    fn health_unsettled_pnl(&self, health_type: ReplayHealthType) -> I80F48 {
        let bids_case = self.order_execution_case(
            self.bids_base_lots,
            self.base_prices.liab(health_type),
            health_type,
        );
        let asks_case = self.order_execution_case(
            -self.asks_base_lots,
            self.base_prices.asset(health_type),
            health_type,
        );
        let worst_case = bids_case.min(asks_case);
        self.weigh_health_contribution_overall(self.quote_current + worst_case, health_type)
    }

    fn order_execution_case(
        &self,
        orders_base_lots: i64,
        order_price: I80F48,
        health_type: ReplayHealthType,
    ) -> I80F48 {
        let net_base_native = I80F48::from_num(self.base_lots + orders_base_lots)
            * I80F48::from_num(self.base_lot_size);

        let (weight, base_price) = if net_base_native < I80F48::ZERO {
            (
                match health_type {
                    ReplayHealthType::Init => self.init_base_liab_weight,
                    ReplayHealthType::Maint => self.maint_base_liab_weight,
                },
                self.base_prices.liab(health_type),
            )
        } else {
            (
                match health_type {
                    ReplayHealthType::Init => self.init_base_asset_weight,
                    ReplayHealthType::Maint => self.maint_base_asset_weight,
                },
                self.base_prices.asset(health_type),
            )
        };
        let base_health = net_base_native * weight * base_price;
        let orders_base_native =
            I80F48::from_num(orders_base_lots) * I80F48::from_num(self.base_lot_size);
        let order_quote = -orders_base_native * order_price;
        base_health + order_quote
    }

    fn weigh_health_contribution_overall(
        &self,
        unweighted: I80F48,
        health_type: ReplayHealthType,
    ) -> I80F48 {
        if unweighted > I80F48::ZERO {
            let overall_weight = match health_type {
                ReplayHealthType::Init => self.init_overall_asset_weight,
                ReplayHealthType::Maint => self.maint_overall_asset_weight,
            };
            overall_weight * unweighted
        } else {
            unweighted
        }
    }
}

pub struct ContinuumStateEngine {
    intents_by_key: HashMap<String, CanonicalIntent>,
    processed_event_ids: HashSet<String>,
    processed_event_id_order: VecDeque<String>,
    enqueued_event_ids: HashSet<String>,
    enqueued_event_id_order: VecDeque<String>,
    divergences: Vec<DivergenceEvent>,
    revision: u64,
    last_seen_slot: u64,
    cached_confirmed: Option<ViewStateCache>,
    cached_optimistic: Option<ViewStateCache>,
    baseline_positions: BTreeMap<(String, String), BaselinePosition>,
    baseline_orders: BTreeMap<String, OpenOrderSummary>,
    baseline_margin_accounts: BTreeMap<String, MarginSummaryAccount>,
    baseline_confirmed_seq: BTreeMap<String, u64>,
    baseline_user_accounts: BTreeMap<String, BTreeSet<String>>,
    baseline_accounts: BTreeMap<String, AccountProjectedState>,
    baseline_perp_markets: BTreeMap<String, PerpMarketSyncState>,
    baseline_token_banks: BTreeMap<String, TokenBankSyncState>,
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
            processed_event_id_order: VecDeque::new(),
            enqueued_event_ids: HashSet::new(),
            enqueued_event_id_order: VecDeque::new(),
            divergences: Vec::new(),
            revision: 0,
            last_seen_slot: 0,
            cached_confirmed: None,
            cached_optimistic: None,
            baseline_positions: BTreeMap::new(),
            baseline_orders: BTreeMap::new(),
            baseline_margin_accounts: BTreeMap::new(),
            baseline_confirmed_seq: BTreeMap::new(),
            baseline_user_accounts: BTreeMap::new(),
            baseline_accounts: BTreeMap::new(),
            baseline_perp_markets: BTreeMap::new(),
            baseline_token_banks: BTreeMap::new(),
            baseline_bootstrapped: false,
        }
    }

    pub fn bootstrap_from_onchain_snapshot(&mut self, snapshot: EngineSnapshot) -> Result<()> {
        self.baseline_positions.clear();
        self.baseline_orders.clear();
        self.baseline_margin_accounts.clear();
        self.baseline_confirmed_seq.clear();
        self.baseline_user_accounts.clear();
        self.baseline_accounts.clear();
        self.baseline_perp_markets.clear();
        self.baseline_token_banks.clear();

        let EngineSnapshot {
            users,
            markets,
            accounts,
            perp_markets,
            token_banks,
            ..
        } = snapshot;

        let mut baseline_last_slot = 0u64;

        for (owner, user_state) in users {
            let owner_accounts = self
                .baseline_user_accounts
                .entry(owner.clone())
                .or_default();
            owner_accounts.extend(user_state.mango_accounts.iter().cloned());
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
            if let MarginSummary::Ok(summary) = user_state.margin_summary {
                for account in summary.accounts {
                    self.baseline_margin_accounts
                        .insert(account.mango_account.clone(), account);
                }
            }
        }

        for (mango_account, account_state) in accounts {
            self.baseline_user_accounts
                .entry(account_state.owner.clone())
                .or_default()
                .insert(mango_account.clone());
            for order in &account_state.open_orders {
                self.baseline_orders
                    .insert(order.order_id.clone(), order.clone());
            }
            self.baseline_accounts.insert(mango_account, account_state);
        }

        for (market, market_state) in markets {
            self.baseline_confirmed_seq.insert(
                market.clone(),
                parse_u64_field("confirmed_seq", &market_state.watermarks.confirmed_seq)?,
            );
            baseline_last_slot = baseline_last_slot.max(parse_u64_field(
                "last_slot",
                &market_state.watermarks.last_slot,
            )?);
            for order in market_state.open_orders {
                self.baseline_user_accounts
                    .entry(order.owner.clone())
                    .or_default()
                    .insert(order.mango_account.clone());
                self.baseline_orders.insert(order.order_id.clone(), order);
            }
        }

        self.baseline_perp_markets.extend(perp_markets);
        self.baseline_token_banks.extend(token_banks);

        self.last_seen_slot = self.last_seen_slot.max(baseline_last_slot);
        self.baseline_bootstrapped = true;
        self.prune_rebased_intents();
        self.enforce_retention_limits();
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
        self.enforce_retention_limits();
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
        if !Self::track_event_id(
            &mut self.enqueued_event_ids,
            &mut self.enqueued_event_id_order,
            id,
        ) {
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

        self.enforce_retention_limits();
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
        if !Self::track_event_id(
            &mut self.processed_event_ids,
            &mut self.processed_event_id_order,
            id,
        ) {
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

        self.enforce_retention_limits();
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

    pub fn find_intent(
        &self,
        group: &str,
        sequence: impl ToString,
        kind: u8,
    ) -> Result<Option<CanonicalIntentState>> {
        let key = queue_item_key(
            group,
            parse_u64_field("sequence", &sequence.to_string())?,
            kind,
        );
        Ok(self
            .list_intents()
            .into_iter()
            .find(|intent| intent.key == key))
    }

    pub fn find_intent_json(
        &self,
        group: &str,
        sequence: impl ToString,
        kind: u8,
    ) -> Result<Option<String>> {
        self.find_intent(group, sequence, kind)?
            .map(|intent| serde_json::to_string(&intent))
            .transpose()
            .map_err(Into::into)
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

    pub fn get_orders_json(
        &mut self,
        market: &str,
        owner: Option<&str>,
        view: QueueView,
    ) -> Result<String> {
        Ok(serde_json::to_string(
            &self.get_orders(market, owner, view)?,
        )?)
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

    pub fn get_all_trades(&mut self, view: QueueView, limit: usize) -> Result<Vec<MarketTrade>> {
        let state = self.view_state(view)?;
        let mut trades: Vec<_> = state
            .trades
            .values()
            .flatten()
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
            .collect();
        trades.sort_by(compare_trades_chronologically);
        let start = trades.len().saturating_sub(limit);
        Ok(trades[start..].to_vec())
    }

    pub fn get_all_trades_json(&mut self, view: QueueView, limit: usize) -> Result<String> {
        Ok(serde_json::to_string(&self.get_all_trades(view, limit)?)?)
    }

    pub fn get_trades_filtered(
        &mut self,
        market: Option<&str>,
        owner: Option<&str>,
        view: QueueView,
        limit: usize,
    ) -> Result<Vec<MarketTrade>> {
        let mut trades = self.get_all_trades(view, MAX_TRADE_HISTORY)?;
        if let Some(owner) = owner {
            trades.retain(|trade| trade.maker_owner == owner || trade.taker_owner == owner);
        }
        if let Some(market) = market {
            trades.retain(|trade| trade.market == market);
        }
        let start = trades.len().saturating_sub(limit);
        Ok(trades[start..].to_vec())
    }

    pub fn get_trades_filtered_json(
        &mut self,
        market: Option<&str>,
        owner: Option<&str>,
        view: QueueView,
        limit: usize,
    ) -> Result<String> {
        Ok(serde_json::to_string(
            &self.get_trades_filtered(market, owner, view, limit)?,
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
        let started = Instant::now();
        let now_ts_ms = now_ts_ms();
        let now_ts = now_ts_ms / 1_000;
        let projection_started = Instant::now();
        let mut projection = self.build_projection(view, now_ts)?;
        let projection_elapsed = projection_started.elapsed();
        let prune_started = Instant::now();
        projection.engine.prune_expired_orders(now_ts)?;
        let prune_elapsed = prune_started.elapsed();
        let snapshot_started = Instant::now();
        let snapshot = self.snapshot_from_projection(view, &projection, now_ts_ms, now_ts)?;
        let snapshot_elapsed = snapshot_started.elapsed();
        logging::log_view_build(
            match view {
                QueueView::Confirmed => "confirmed",
                QueueView::Optimistic => "optimistic",
            },
            started.elapsed(),
            projection_elapsed,
            prune_elapsed,
            snapshot_elapsed,
            snapshot.markets.len(),
            snapshot.users.len(),
            snapshot.accounts.len(),
        );
        Ok(ViewState {
            snapshot,
            trades: projection.trades,
        })
    }

    fn build_projection(&self, view: QueueView, current_now_ts: u64) -> Result<Projection> {
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
        for market in self.baseline_perp_markets.keys() {
            projection.active_markets.insert(market.clone());
        }
        for account in self.baseline_accounts.values() {
            projection
                .owner_accounts
                .entry(account.owner.clone())
                .or_default()
                .insert(account.mango_account.clone());
            for position in &account.perp_positions {
                projection
                    .active_markets
                    .insert(position.market_index.to_string());
            }
            for order in &account.open_orders {
                projection.active_markets.insert(order.market.clone());
            }
        }

        for market in projection.active_markets.clone() {
            if let Ok(market_index) = parse_market_index(&market) {
                ensure_projection_market(
                    &mut projection.engine,
                    market_index,
                    self.baseline_perp_markets.get(&market),
                )?;
            }
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

            ensure_projection_market(
                &mut projection.engine,
                market_index,
                self.baseline_perp_markets.get(&baseline_order.market),
            )?;
            projection
                .engine
                .ensure_account(mango_account_pubkey, owner_pubkey)?;
            projection.engine.import_open_order(
                market_index,
                mango_account_pubkey,
                parse_side("baseline_order.side", &baseline_order.side)?,
                parse_composite_order_id(&baseline_order.order_id)?,
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

        for account in self.baseline_accounts.values() {
            let owner_pubkey = match parse_pubkey("baseline_account.owner", &account.owner) {
                Ok(owner) => owner,
                Err(_) => continue,
            };
            let mango_account_pubkey =
                match parse_pubkey("baseline_account.mango_account", &account.mango_account) {
                    Ok(account_key) => account_key,
                    Err(_) => continue,
                };
            projection.account_identities.insert(
                mango_account_pubkey,
                AccountIdentity {
                    owner: account.owner.clone(),
                },
            );
            projection
                .engine
                .ensure_account(mango_account_pubkey, owner_pubkey)?;

            for position in &account.perp_positions {
                let market_key = position.market_index.to_string();
                let settle_token_index = self
                    .baseline_perp_markets
                    .get(&market_key)
                    .map(|market| market.settle_token_index as u16)
                    .unwrap_or_default();
                ensure_projection_market(
                    &mut projection.engine,
                    position.market_index as u16,
                    self.baseline_perp_markets.get(&market_key),
                )?;
                projection.engine.import_perp_position_state(
                    mango_account_pubkey,
                    owner_pubkey,
                    position.market_index as u16,
                    settle_token_index,
                    position,
                )?;
            }
        }

        let intents = self.sorted_replay_intents();

        for intent in intents
            .iter()
            .copied()
            .filter(|intent| intent.processed_status == Some(QUEUE_PROCESS_EXECUTED))
        {
            self.apply_intent(intent, &mut projection, intent.accepted_ts_ms / 1_000)?;
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
                self.apply_intent(intent, &mut projection, current_now_ts)?;
            }
        }

        Ok(projection)
    }

    fn apply_intent(
        &self,
        intent: &CanonicalIntent,
        projection: &mut Projection,
        simulation_now_ts: u64,
    ) -> Result<()> {
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

        ensure_projection_market(
            &mut projection.engine,
            market_index,
            self.baseline_perp_markets.get(&intent.market),
        )?;
        if let QueuePayload::PerpPlaceOrderV2(place) = payload {
            if !self.baseline_perp_markets.contains_key(&intent.market) {
                projection
                    .engine
                    .set_oracle_price(market_index, place.price_lots.max(1) as f64)?;
            }
        }
        projection
            .engine
            .ensure_account(mango_account_pubkey, owner_pubkey)?;
        projection.engine.prune_expired_orders(simulation_now_ts)?;

        let result = projection.engine.execute_decoded_payload(
            market_index,
            mango_account_pubkey,
            payload,
            simulation_now_ts,
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

        let accounts = self.build_projected_accounts(projection, now_ts)?;
        let mut all_owners: BTreeSet<String> = projection.owner_accounts.keys().cloned().collect();
        for account in accounts.values() {
            all_owners.insert(account.owner.clone());
        }
        for (owner, _) in self.baseline_positions.keys() {
            all_owners.insert(owner.clone());
        }

        let mut users = HashMap::new();
        for owner in all_owners {
            let mut mango_accounts = projection
                .owner_accounts
                .get(&owner)
                .cloned()
                .unwrap_or_default();
            for (mango_account, account_state) in &accounts {
                if account_state.owner == owner {
                    mango_accounts.insert(mango_account.clone());
                }
            }

            let mut open_orders = Vec::new();
            let mut per_market = BTreeMap::<String, UserPerMarketAggregate>::new();

            for mango_account in &mango_accounts {
                if let Some(account_state) = accounts.get(mango_account) {
                    self.merge_projected_account_state(
                        account_state,
                        &mut open_orders,
                        &mut per_market,
                    )?;
                }
            }

            if self.baseline_accounts.is_empty() {
                for ((entry_owner, market), position) in &self.baseline_positions {
                    if entry_owner != &owner {
                        continue;
                    }
                    let entry = per_market.entry(market.clone()).or_default();
                    entry.base_position_lots += position.base_position_lots as i128;
                    entry.quote_position_native += position.quote_position_native;
                }
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
            let margin_summary = self.build_owner_margin_summary(
                &owner,
                &mango_accounts,
                &accounts,
                &projection.engine,
            )?;

            users.insert(
                owner.clone(),
                UserState {
                    owner: owner.clone(),
                    mango_accounts: mango_accounts.into_iter().collect(),
                    open_orders,
                    per_market: per_market_vec,
                    margin_summary,
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
            accounts,
            perp_markets: self
                .baseline_perp_markets
                .iter()
                .map(|(market, state)| (market.clone(), state.clone()))
                .collect(),
            token_banks: self
                .baseline_token_banks
                .iter()
                .map(|(token, state)| (token.clone(), state.clone()))
                .collect(),
            generated_ts_ms,
        })
    }

    fn build_projected_accounts(
        &self,
        projection: &Projection,
        now_ts: u64,
    ) -> Result<HashMap<String, AccountProjectedState>> {
        let mut account_keys = BTreeSet::new();
        account_keys.extend(self.baseline_accounts.keys().cloned());
        for accounts in projection.owner_accounts.values() {
            account_keys.extend(accounts.iter().cloned());
        }

        let mut projected_accounts = HashMap::new();
        for mango_account in account_keys {
            let baseline = self.baseline_accounts.get(&mango_account);
            let snapshot = match Pubkey::from_str(&mango_account) {
                Ok(mango_account_pubkey) => match projection
                    .engine
                    .account_snapshot(mango_account_pubkey, now_ts)
                {
                    Ok(snapshot) => Some(snapshot),
                    Err(HarnessError::UnknownAccount(_)) => None,
                    Err(err) => return Err(err),
                },
                Err(_) => None,
            };

            if snapshot.is_none() && baseline.is_none() {
                continue;
            }

            let owner = baseline
                .map(|state| state.owner.clone())
                .or_else(|| snapshot.as_ref().map(|state| state.owner.to_string()))
                .or_else(|| {
                    projection
                        .owner_accounts
                        .iter()
                        .find_map(|(owner, accounts)| {
                            accounts.contains(&mango_account).then(|| owner.clone())
                        })
                })
                .unwrap_or_else(|| "unknown".to_string());

            let open_orders = snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .open_orders
                        .iter()
                        .cloned()
                        .map(open_order_snapshot_to_summary)
                        .collect()
                })
                .or_else(|| baseline.map(|state| state.open_orders.clone()))
                .unwrap_or_default();
            let perp_positions = snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .perp_positions
                        .iter()
                        .cloned()
                        .map(account_perp_position_from_snapshot)
                        .collect()
                })
                .or_else(|| baseline.map(|state| state.perp_positions.clone()))
                .unwrap_or_default();

            projected_accounts.insert(
                mango_account.clone(),
                AccountProjectedState {
                    owner,
                    mango_account: mango_account.clone(),
                    net_deposits: baseline
                        .map(|state| state.net_deposits.clone())
                        .unwrap_or_else(|| "0".to_string()),
                    open_orders,
                    token_positions: baseline
                        .map(|state| state.token_positions.clone())
                        .unwrap_or_default(),
                    perp_positions,
                    unsupported_exposures: baseline
                        .map(|state| state.unsupported_exposures.clone())
                        .unwrap_or_default(),
                },
            );
        }

        Ok(projected_accounts)
    }

    fn merge_projected_account_state(
        &self,
        account_state: &AccountProjectedState,
        open_orders: &mut Vec<OpenOrderSummary>,
        per_market: &mut BTreeMap<String, UserPerMarketAggregate>,
    ) -> Result<()> {
        for order in &account_state.open_orders {
            open_orders.push(order.clone());
            let entry = per_market.entry(order.market.clone()).or_default();
            entry.quote_reserved += parse_i128_generated(&order.quote_lots)?;
        }

        for position in &account_state.perp_positions {
            let market_key = position.market_index.to_string();
            let entry = per_market.entry(market_key).or_default();
            entry.open_bid += parse_i128_generated(&position.open_bid_base_lots)?;
            entry.open_ask += parse_i128_generated(&position.open_ask_base_lots)?;
            entry.base_position_lots += parse_i128_generated(&position.base_position_lots)?;
            entry.quote_position_native +=
                parse_i80f48_field("quote_position_native", &position.quote_position_native)?;
        }

        Ok(())
    }

    fn build_owner_margin_summary(
        &self,
        owner: &str,
        mango_accounts: &BTreeSet<String>,
        projected_accounts: &HashMap<String, AccountProjectedState>,
        engine: &HarnessEngine,
    ) -> Result<MarginSummary> {
        let mut accounts = Vec::new();
        let mut partial = false;

        for mango_account in mango_accounts {
            if let Some(account_state) = projected_accounts.get(mango_account) {
                partial |= !account_state.unsupported_exposures.is_empty();
                let summary = if self.has_rich_health_inputs() {
                    self.build_margin_account_summary_rich(account_state)?
                } else {
                    self.build_margin_account_summary_legacy(
                        owner,
                        mango_account,
                        Some(account_state),
                        self.baseline_margin_accounts.get(mango_account),
                        engine,
                    )?
                };
                accounts.push(summary);
            } else if let Some(baseline) = self.baseline_margin_accounts.get(mango_account) {
                accounts.push(baseline.clone());
            }
        }

        if accounts.is_empty() {
            return Ok(empty_margin_summary(if partial {
                RUST_REPLAY_MARGIN_SOURCE_PARTIAL
            } else {
                RUST_REPLAY_MARGIN_SOURCE
            }));
        }

        accounts.sort_by(|a, b| a.mango_account.cmp(&b.mango_account));
        let totals = margin_totals_from_accounts(&accounts)?;
        Ok(MarginSummary::Ok(MarginSummaryOk {
            source: if partial {
                RUST_REPLAY_MARGIN_SOURCE_PARTIAL.to_string()
            } else {
                RUST_REPLAY_MARGIN_SOURCE.to_string()
            },
            account_count: accounts.len() as u64,
            totals: totals.clone(),
            equity_native_quote: Some(totals.equity_native_quote.clone()),
            pnl_native_quote: Some(totals.pnl_native_quote.clone()),
            assets_native_quote: Some(totals.assets_native_quote.clone()),
            liabs_native_quote: Some(totals.liabs_native_quote.clone()),
            init_health_native_quote: Some(totals.init_health_native_quote.clone()),
            maint_health_native_quote: Some(totals.maint_health_native_quote.clone()),
            margin_usage_fraction: Some(totals.margin_usage_fraction),
            accounts,
        }))
    }

    fn build_margin_account_summary_rich(
        &self,
        account_state: &AccountProjectedState,
    ) -> Result<MarginSummaryAccount> {
        let equity_native_quote = self.account_equity_native_quote(account_state)?;
        let pnl_native_quote =
            equity_native_quote - parse_i80f48_field("net_deposits", &account_state.net_deposits)?;
        let (init_assets_native_quote, init_liabs_native_quote) =
            self.account_health_assets_and_liabs(account_state, ReplayHealthType::Init)?;
        let (maint_assets_native_quote, maint_liabs_native_quote) =
            self.account_health_assets_and_liabs(account_state, ReplayHealthType::Maint)?;
        let init_health_native_quote = init_assets_native_quote - init_liabs_native_quote;
        let maint_health_native_quote = maint_assets_native_quote - maint_liabs_native_quote;

        let mut perp_positions: Vec<_> = account_state
            .perp_positions
            .iter()
            .map(|position| MarginSummaryAccountPerpPosition {
                market_index: position.market_index,
                base_position_lots: position.base_position_lots.clone(),
                quote_position_native: position.quote_position_native.clone(),
            })
            .collect();
        perp_positions.sort_by_key(|position| position.market_index);

        let margin_usage_fraction =
            ratio_or_zero(init_liabs_native_quote, init_assets_native_quote).to_num::<f64>();

        Ok(MarginSummaryAccount {
            mango_account: account_state.mango_account.clone(),
            owner: account_state.owner.clone(),
            equity_native_quote: equity_native_quote.to_string(),
            pnl_native_quote: pnl_native_quote.to_string(),
            assets_native_quote: init_assets_native_quote.to_string(),
            liabs_native_quote: init_liabs_native_quote.to_string(),
            init_health_native_quote: init_health_native_quote.to_string(),
            maint_health_native_quote: maint_health_native_quote.to_string(),
            init_health_ratio: health_ratio_string(
                init_health_native_quote,
                init_liabs_native_quote,
            ),
            maint_health_ratio: health_ratio_string(
                maint_health_native_quote,
                maint_liabs_native_quote,
            ),
            margin_usage_fraction: if margin_usage_fraction.is_finite() {
                margin_usage_fraction
            } else {
                0.0
            },
            perp_positions,
        })
    }

    fn build_margin_account_summary_legacy(
        &self,
        owner: &str,
        mango_account: &str,
        snapshot: Option<&AccountProjectedState>,
        baseline: Option<&MarginSummaryAccount>,
        engine: &HarnessEngine,
    ) -> Result<MarginSummaryAccount> {
        if snapshot.is_none() {
            if let Some(baseline) = baseline {
                return Ok(baseline.clone());
            }
        }

        let mut combined_positions = BTreeMap::<u16, (i64, I80F48)>::new();
        if let Some(baseline) = baseline {
            for position in &baseline.perp_positions {
                let entry = combined_positions
                    .entry(position.market_index as u16)
                    .or_insert((0, I80F48::ZERO));
                entry.0 +=
                    parse_i64_field("baseline.base_position_lots", &position.base_position_lots)?;
                entry.1 += parse_i80f48_field(
                    "baseline.quote_position_native",
                    &position.quote_position_native,
                )?;
            }
        }
        if let Some(snapshot) = snapshot {
            for position in &snapshot.perp_positions {
                let entry = combined_positions
                    .entry(position.market_index as u16)
                    .or_insert((0, I80F48::ZERO));
                entry.0 += parse_i64_field("base_position_lots", &position.base_position_lots)?;
                entry.1 +=
                    parse_i80f48_field("quote_position_native", &position.quote_position_native)?;
            }
        }

        let mut perp_positions = Vec::new();
        let mut perp_equity_native_quote = I80F48::ZERO;
        for (market_index, (base_position_lots, quote_position_native)) in combined_positions {
            let marked_base_native_quote =
                market_value_native_quote(engine, market_index, base_position_lots)?;
            perp_equity_native_quote += quote_position_native + marked_base_native_quote;
            perp_positions.push(MarginSummaryAccountPerpPosition {
                market_index: market_index as u64,
                base_position_lots: base_position_lots.to_string(),
                quote_position_native: quote_position_native.to_string(),
            });
        }
        perp_positions.sort_by_key(|position| position.market_index);

        let mut open_order_risk_native_quote = I80F48::ZERO;
        if let Some(snapshot) = snapshot {
            for order in &snapshot.open_orders {
                open_order_risk_native_quote +=
                    order_summary_quote_notional_native_quote(engine, order)?;
            }
        }

        let equity_native_quote = perp_equity_native_quote;
        let pnl_native_quote = perp_equity_native_quote;
        let assets_native_quote = if equity_native_quote > I80F48::ZERO {
            equity_native_quote
        } else {
            I80F48::ZERO
        };
        let liabs_native_quote = if equity_native_quote < I80F48::ZERO {
            -equity_native_quote
        } else {
            I80F48::ZERO
        };
        let init_health_native_quote = equity_native_quote - open_order_risk_native_quote;
        let maint_health_native_quote = init_health_native_quote;
        let margin_usage_fraction =
            ratio_or_zero(liabs_native_quote, assets_native_quote).to_num::<f64>();

        Ok(MarginSummaryAccount {
            mango_account: mango_account.to_string(),
            owner: owner.to_string(),
            equity_native_quote: equity_native_quote.to_string(),
            pnl_native_quote: pnl_native_quote.to_string(),
            assets_native_quote: assets_native_quote.to_string(),
            liabs_native_quote: liabs_native_quote.to_string(),
            init_health_native_quote: init_health_native_quote.to_string(),
            maint_health_native_quote: maint_health_native_quote.to_string(),
            init_health_ratio: health_ratio_string(init_health_native_quote, liabs_native_quote),
            maint_health_ratio: health_ratio_string(maint_health_native_quote, liabs_native_quote),
            margin_usage_fraction: if margin_usage_fraction.is_finite() {
                margin_usage_fraction
            } else {
                0.0
            },
            perp_positions,
        })
    }

    fn has_rich_health_inputs(&self) -> bool {
        !self.baseline_token_banks.is_empty()
    }

    fn account_equity_native_quote(&self, account_state: &AccountProjectedState) -> Result<I80F48> {
        let mut equity = I80F48::ZERO;

        for token_position in &account_state.token_positions {
            let token_info = self.token_bank_info(token_position.token_index)?;
            let native_balance =
                parse_i80f48_field("native_balance", &token_position.native_balance)?;
            equity += native_balance * token_info.prices.oracle;
        }

        for perp_position in &account_state.perp_positions {
            equity += self.perp_equity_native_quote(perp_position)?;
        }

        Ok(equity)
    }

    fn account_health_assets_and_liabs(
        &self,
        account_state: &AccountProjectedState,
        health_type: ReplayHealthType,
    ) -> Result<(I80F48, I80F48)> {
        let mut token_balances = HashMap::<u64, I80F48>::new();
        for token_position in &account_state.token_positions {
            token_balances.insert(
                token_position.token_index,
                parse_i80f48_field("native_balance", &token_position.native_balance)?,
            );
        }

        let mut perp_by_settle_token = HashMap::<u64, Vec<ReplayPerpInfo>>::new();
        for perp_position in &account_state.perp_positions {
            let perp_info = self.perp_market_info(perp_position.market_index)?;
            perp_by_settle_token
                .entry(perp_info.settle_token_index)
                .or_default()
                .push(perp_info.with_position(perp_position)?);
        }

        let mut total_assets = I80F48::ZERO;
        let mut total_liabs = I80F48::ZERO;

        for token_bank in self.baseline_token_banks.values() {
            let token_info = replay_token_info(token_bank)?;
            let mut asset_balance = I80F48::ZERO;
            let mut liab_balance = I80F48::ZERO;

            let spot_balance = token_balances
                .get(&token_bank.token_index)
                .copied()
                .unwrap_or(I80F48::ZERO);
            if spot_balance > I80F48::ZERO {
                asset_balance += spot_balance;
            } else {
                liab_balance -= spot_balance;
            }

            if let Some(perps) = perp_by_settle_token.get(&token_bank.token_index) {
                for perp_info in perps {
                    let health_unsettled = perp_info.health_unsettled_pnl(health_type);
                    if health_unsettled > I80F48::ZERO {
                        asset_balance += health_unsettled;
                    } else {
                        liab_balance -= health_unsettled;
                    }
                }
            }

            let liab_weighted_price = token_info.liab_weighted_price(health_type);
            let liabs = liab_balance * liab_weighted_price;
            total_liabs += liabs;
            if asset_balance >= liab_balance {
                let asset_weighted_price = token_info.asset_weighted_price(health_type);
                total_assets += liabs + (asset_balance - liab_balance) * asset_weighted_price;
            } else {
                total_assets += asset_balance * liab_weighted_price;
            }
        }

        Ok((total_assets, total_liabs))
    }

    fn token_bank_info(&self, token_index: u64) -> Result<ReplayTokenInfo> {
        let token_key = token_index.to_string();
        let token_bank = self.baseline_token_banks.get(&token_key).ok_or_else(|| {
            HarnessError::InvalidInteger {
                field: "token_index",
                value: token_key.clone(),
            }
        })?;
        replay_token_info(token_bank)
    }

    fn perp_market_info(&self, market_index: u64) -> Result<ReplayPerpInfo> {
        let market_key = market_index.to_string();
        let market = self.baseline_perp_markets.get(&market_key).ok_or_else(|| {
            HarnessError::InvalidInteger {
                field: "market_index",
                value: market_key.clone(),
            }
        })?;
        replay_perp_info(market)
    }

    fn perp_equity_native_quote(&self, perp_position: &AccountPerpPositionState) -> Result<I80F48> {
        let perp_info = self
            .perp_market_info(perp_position.market_index)?
            .with_position(perp_position)?;
        let base_lots = parse_i64_field("base_position_lots", &perp_position.base_position_lots)?
            + parse_i64_field("taker_base_lots", &perp_position.taker_base_lots)?;
        let base_native = I80F48::from_num(base_lots) * I80F48::from_num(perp_info.base_lot_size);
        Ok(base_native * perp_info.base_prices.oracle + perp_info.quote_current)
    }

    fn prune_rebased_intents(&mut self) {
        self.intents_by_key.retain(|_, intent| {
            let baseline_seq = self
                .baseline_confirmed_seq
                .get(&intent.market)
                .copied()
                .unwrap_or(0);
            intent.market == "unknown" || intent.sequence > baseline_seq
        });
    }

    fn track_event_id(ids: &mut HashSet<String>, order: &mut VecDeque<String>, id: String) -> bool {
        if !ids.insert(id.clone()) {
            return false;
        }
        order.push_back(id);
        while order.len() > MAX_EVENT_ID_HISTORY {
            if let Some(evicted) = order.pop_front() {
                ids.remove(&evicted);
            }
        }
        true
    }

    fn enforce_retention_limits(&mut self) {
        self.prune_rebased_intents();
        self.trim_divergences();
        self.trim_intents();
    }

    fn trim_divergences(&mut self) {
        if self.divergences.len() > MAX_DIVERGENCE_HISTORY {
            let drain = self.divergences.len() - MAX_DIVERGENCE_HISTORY;
            self.divergences.drain(0..drain);
        }
    }

    fn trim_intents(&mut self) {
        if self.intents_by_key.len() <= MAX_INTENT_HISTORY {
            return;
        }

        let mut candidates: Vec<_> = self
            .intents_by_key
            .values()
            .map(|intent| {
                let baseline_seq = self.baseline_seq_for_market(&intent.market);
                let terminal = matches!(
                    intent.processed_status,
                    Some(QUEUE_PROCESS_EXECUTED | QUEUE_PROCESS_FAILED | QUEUE_PROCESS_SKIPPED)
                );
                let absorbed = intent.market == "unknown" || intent.sequence <= baseline_seq;
                (
                    intent.key.clone(),
                    absorbed,
                    terminal,
                    intent.processed_slot.unwrap_or(0),
                    intent.accepted_ts_ms,
                    intent.sequence,
                )
            })
            .collect();

        candidates.sort_by(|a, b| {
            let a_priority = if a.1 {
                0u8
            } else if a.2 {
                1u8
            } else {
                2u8
            };
            let b_priority = if b.1 {
                0u8
            } else if b.2 {
                1u8
            } else {
                2u8
            };
            a_priority
                .cmp(&b_priority)
                .then_with(|| a.3.cmp(&b.3))
                .then_with(|| a.4.cmp(&b.4))
                .then_with(|| a.5.cmp(&b.5))
        });

        let overflow = self.intents_by_key.len().saturating_sub(MAX_INTENT_HISTORY);
        if overflow == 0 {
            return;
        }

        let mut removed = 0usize;
        let mut removed_pending = 0usize;
        for (key, _absorbed, terminal, ..) in candidates.into_iter().take(overflow) {
            if let Some(intent) = self.intents_by_key.remove(&key) {
                removed += 1;
                if !terminal {
                    removed_pending += 1;
                }
                let _ = intent;
            }
        }

        if removed > 0 {
            self.emit_divergence(
                "intent_history_trimmed",
                "retention:intents",
                HashMap::from([
                    ("removed".to_string(), removed.to_string()),
                    ("removed_pending".to_string(), removed_pending.to_string()),
                    ("limit".to_string(), MAX_INTENT_HISTORY.to_string()),
                ]),
            );
            self.trim_divergences();
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
        self.trim_divergences();
    }

    fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.cached_confirmed = None;
        self.cached_optimistic = None;
    }
}

fn ensure_projection_market(
    engine: &mut HarnessEngine,
    market_index: u16,
    sync_state: Option<&PerpMarketSyncState>,
) -> Result<()> {
    if let Some(sync_state) = sync_state {
        engine.configure_market(market_index, market_config_from_sync_state(sync_state)?)?;
    } else if !engine.has_market(market_index) {
        engine.register_market(market_index, MarketConfig::default())?;
    }
    Ok(())
}

fn empty_margin_summary(source: &str) -> MarginSummary {
    MarginSummary::Empty(MarginSummaryEmpty {
        source: source.to_string(),
        account_count: 0,
        totals: MarginSummaryEmptyTotals {
            equity_native_quote: "0".to_string(),
            pnl_native_quote: "0".to_string(),
            assets_native_quote: "0".to_string(),
            liabs_native_quote: "0".to_string(),
            init_health_native_quote: "0".to_string(),
            maint_health_native_quote: "0".to_string(),
            margin_usage_fraction: 0.0,
        },
        accounts: Vec::new(),
    })
}

fn placeholder_margin_summary() -> MarginSummary {
    MarginSummary::Placeholder(MarginSummaryPlaceholder {
        source: "queue-replay".to_string(),
    })
}

fn margin_totals_from_accounts(
    accounts: &[MarginSummaryAccount],
) -> Result<MarginSummaryEmptyTotals> {
    let mut equity_native_quote = I80F48::ZERO;
    let mut pnl_native_quote = I80F48::ZERO;
    let mut assets_native_quote = I80F48::ZERO;
    let mut liabs_native_quote = I80F48::ZERO;
    let mut init_health_native_quote = I80F48::ZERO;
    let mut maint_health_native_quote = I80F48::ZERO;

    for account in accounts {
        equity_native_quote +=
            parse_i80f48_field("equity_native_quote", &account.equity_native_quote)?;
        pnl_native_quote += parse_i80f48_field("pnl_native_quote", &account.pnl_native_quote)?;
        assets_native_quote +=
            parse_i80f48_field("assets_native_quote", &account.assets_native_quote)?;
        liabs_native_quote +=
            parse_i80f48_field("liabs_native_quote", &account.liabs_native_quote)?;
        init_health_native_quote += parse_i80f48_field(
            "init_health_native_quote",
            &account.init_health_native_quote,
        )?;
        maint_health_native_quote += parse_i80f48_field(
            "maint_health_native_quote",
            &account.maint_health_native_quote,
        )?;
    }

    Ok(MarginSummaryEmptyTotals {
        equity_native_quote: equity_native_quote.to_string(),
        pnl_native_quote: pnl_native_quote.to_string(),
        assets_native_quote: assets_native_quote.to_string(),
        liabs_native_quote: liabs_native_quote.to_string(),
        init_health_native_quote: init_health_native_quote.to_string(),
        maint_health_native_quote: maint_health_native_quote.to_string(),
        margin_usage_fraction: ratio_or_zero(liabs_native_quote, assets_native_quote)
            .to_num::<f64>(),
    })
}

fn ratio_or_zero(numerator: I80F48, denominator: I80F48) -> I80F48 {
    if denominator <= I80F48::ZERO {
        I80F48::ZERO
    } else {
        numerator / denominator
    }
}

fn health_ratio_string(health: I80F48, liabs: I80F48) -> String {
    if liabs > I80F48::from_num(0.001) {
        ((health / liabs) * I80F48::from_num(100)).to_string()
    } else {
        I80F48::MAX.to_string()
    }
}

fn market_value_native_quote(
    engine: &HarnessEngine,
    market_index: u16,
    base_position_lots: i64,
) -> Result<I80F48> {
    let oracle_price_lots = parse_i80f48_field(
        "oracle_price_lots",
        &engine.market_oracle_price_lots(market_index)?,
    )?;
    let quote_lot_size = I80F48::from_num(engine.market_quote_lot_size(market_index)?);
    Ok(I80F48::from_num(base_position_lots) * oracle_price_lots * quote_lot_size)
}

fn order_summary_quote_notional_native_quote(
    engine: &HarnessEngine,
    order: &OpenOrderSummary,
) -> Result<I80F48> {
    let market_index = parse_market_index(&order.market)?;
    let quote_lot_size = I80F48::from_num(engine.market_quote_lot_size(market_index)?);
    Ok(parse_i80f48_field("quote_lots", &order.quote_lots)? * quote_lot_size)
}

fn market_config_from_sync_state(sync_state: &PerpMarketSyncState) -> Result<MarketConfig> {
    Ok(MarketConfig {
        oracle_price: parse_i80f48_field("oracle_price", &sync_state.oracle_price)?.to_num(),
        stable_price: parse_i80f48_field("stable_price", &sync_state.stable_price)?.to_num(),
        base_lot_size: parse_i64_field("base_lot_size", &sync_state.base_lot_size)?,
        quote_lot_size: parse_i64_field("quote_lot_size", &sync_state.quote_lot_size)?,
        settle_token_index: parse_u16_from_u64(
            "settle_token_index",
            sync_state.settle_token_index,
        )?,
        maint_base_asset_weight: parse_i80f48_field(
            "maint_base_asset_weight",
            &sync_state.maint_base_asset_weight,
        )?,
        init_base_asset_weight: parse_i80f48_field(
            "init_base_asset_weight",
            &sync_state.init_base_asset_weight,
        )?,
        maint_base_liab_weight: parse_i80f48_field(
            "maint_base_liab_weight",
            &sync_state.maint_base_liab_weight,
        )?,
        init_base_liab_weight: parse_i80f48_field(
            "init_base_liab_weight",
            &sync_state.init_base_liab_weight,
        )?,
        maint_overall_asset_weight: parse_i80f48_field(
            "maint_overall_asset_weight",
            &sync_state.maint_overall_asset_weight,
        )?,
        init_overall_asset_weight: parse_i80f48_field(
            "init_overall_asset_weight",
            &sync_state.init_overall_asset_weight,
        )?,
        long_funding: parse_i80f48_field("long_funding", &sync_state.long_funding)?,
        short_funding: parse_i80f48_field("short_funding", &sync_state.short_funding)?,
        maker_fee: parse_i80f48_field("maker_fee", &sync_state.maker_fee)?,
        taker_fee: parse_i80f48_field("taker_fee", &sync_state.taker_fee)?,
    })
}

fn replay_token_info(token_bank: &TokenBankSyncState) -> Result<ReplayTokenInfo> {
    Ok(ReplayTokenInfo {
        maint_asset_weight: parse_i80f48_field(
            "maint_asset_weight",
            &token_bank.maint_asset_weight,
        )?,
        init_scaled_asset_weight: parse_i80f48_field(
            "init_scaled_asset_weight",
            &token_bank.init_scaled_asset_weight,
        )?,
        maint_liab_weight: parse_i80f48_field("maint_liab_weight", &token_bank.maint_liab_weight)?,
        init_scaled_liab_weight: parse_i80f48_field(
            "init_scaled_liab_weight",
            &token_bank.init_scaled_liab_weight,
        )?,
        prices: ReplayPrices {
            oracle: parse_i80f48_field("oracle_price", &token_bank.oracle_price)?,
            stable: parse_i80f48_field("stable_price", &token_bank.stable_price)?,
        },
    })
}

fn replay_perp_info(sync_state: &PerpMarketSyncState) -> Result<ReplayPerpInfo> {
    Ok(ReplayPerpInfo {
        settle_token_index: sync_state.settle_token_index,
        maint_base_asset_weight: parse_i80f48_field(
            "maint_base_asset_weight",
            &sync_state.maint_base_asset_weight,
        )?,
        init_base_asset_weight: parse_i80f48_field(
            "init_base_asset_weight",
            &sync_state.init_base_asset_weight,
        )?,
        maint_base_liab_weight: parse_i80f48_field(
            "maint_base_liab_weight",
            &sync_state.maint_base_liab_weight,
        )?,
        init_base_liab_weight: parse_i80f48_field(
            "init_base_liab_weight",
            &sync_state.init_base_liab_weight,
        )?,
        maint_overall_asset_weight: parse_i80f48_field(
            "maint_overall_asset_weight",
            &sync_state.maint_overall_asset_weight,
        )?,
        init_overall_asset_weight: parse_i80f48_field(
            "init_overall_asset_weight",
            &sync_state.init_overall_asset_weight,
        )?,
        base_lot_size: parse_i64_field("base_lot_size", &sync_state.base_lot_size)?,
        quote_lot_size: parse_i64_field("quote_lot_size", &sync_state.quote_lot_size)?,
        long_funding: parse_i80f48_field("long_funding", &sync_state.long_funding)?,
        short_funding: parse_i80f48_field("short_funding", &sync_state.short_funding)?,
        base_prices: ReplayPrices {
            oracle: parse_i80f48_field("oracle_price", &sync_state.oracle_price)?,
            stable: parse_i80f48_field("stable_price", &sync_state.stable_price)?,
        },
        base_lots: 0,
        bids_base_lots: 0,
        asks_base_lots: 0,
        quote_current: I80F48::ZERO,
    })
}

fn account_perp_position_from_snapshot(
    snapshot: crate::PerpPositionSnapshot,
) -> AccountPerpPositionState {
    AccountPerpPositionState {
        market_index: snapshot.market_index as u64,
        settle_pnl_limit_window: snapshot.settle_pnl_limit_window,
        settle_pnl_limit_settled_in_current_window_native: snapshot
            .settle_pnl_limit_settled_in_current_window_native,
        base_position_lots: snapshot.base_position_lots.to_string(),
        quote_position_native: snapshot.quote_position_native,
        quote_running_native: snapshot.quote_running_native,
        long_settled_funding: snapshot.long_settled_funding,
        short_settled_funding: snapshot.short_settled_funding,
        open_bid_base_lots: snapshot.open_bid_base_lots.to_string(),
        open_ask_base_lots: snapshot.open_ask_base_lots.to_string(),
        taker_base_lots: snapshot.taker_base_lots.to_string(),
        taker_quote_lots: snapshot.taker_quote_lots.to_string(),
        cumulative_long_funding: snapshot.cumulative_long_funding,
        cumulative_short_funding: snapshot.cumulative_short_funding,
        maker_volume: snapshot.maker_volume,
        taker_volume: snapshot.taker_volume,
        perp_spot_transfers: snapshot.perp_spot_transfers,
        avg_entry_price_per_base_lot: snapshot.avg_entry_price_per_base_lot,
        oneshot_settle_pnl_allowance: snapshot.oneshot_settle_pnl_allowance,
        recurring_settle_pnl_allowance: snapshot.recurring_settle_pnl_allowance,
        realized_pnl_for_position_native: snapshot.realized_pnl_for_position_native,
    }
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

fn parse_u16_from_u64(field: &'static str, value: u64) -> Result<u16> {
    u16::try_from(value).map_err(|_| HarnessError::InvalidInteger {
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

/// Parse a composite order_id like "group:market:numericId" or "group:market:seq:clientId".
/// Falls back to parsing as plain u128 if no colons present.
fn parse_composite_order_id(value: &str) -> Result<u128> {
    // Try plain numeric first
    if let Ok(v) = value.parse::<u128>() {
        return Ok(v);
    }
    // Extract last colon-separated component and try as u128
    if let Some(last) = value.rsplit(':').next() {
        if let Ok(v) = last.parse::<u128>() {
            return Ok(v);
        }
    }
    // Try second-to-last (the on-chain order_id in group:market:orderId:clientOrderId format)
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() >= 3 {
        if let Ok(v) = parts[parts.len() - 2].parse::<u128>() {
            return Ok(v);
        }
    }
    Err(HarnessError::InvalidInteger {
        field: "baseline_order.order_id",
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

fn compare_trades_chronologically(a: &MarketTrade, b: &MarketTrade) -> Ordering {
    a.ts_ms
        .cmp(&b.ts_ms)
        .then_with(|| compare_market_strings(&a.market, &b.market))
        .then_with(|| a.trade_id.cmp(&b.trade_id))
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
                    accounts: HashMap::new(),
                    perp_markets: HashMap::new(),
                    token_banks: HashMap::new(),
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

    #[test]
    fn rich_bootstrap_projects_accounts_and_token_perp_health() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let owner = key();
            let mango_account = key();
            let market = "0".to_string();

            let mut users = HashMap::new();
            users.insert(
                owner.clone(),
                UserState {
                    owner: owner.clone(),
                    mango_accounts: vec![mango_account.clone()],
                    open_orders: Vec::new(),
                    per_market: Vec::new(),
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
                    open_orders: Vec::new(),
                    watermarks: crate::types::MarketWatermarks {
                        optimistic_seq: "9".to_string(),
                        confirmed_seq: "9".to_string(),
                        last_slot: "50".to_string(),
                    },
                },
            );

            let mut accounts = HashMap::new();
            accounts.insert(
                mango_account.clone(),
                AccountProjectedState {
                    owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    net_deposits: "1000".to_string(),
                    open_orders: Vec::new(),
                    token_positions: vec![crate::AccountTokenPositionState {
                        token_index: 0,
                        indexed_position: "1000".to_string(),
                        native_balance: "1000".to_string(),
                        previous_index: "1".to_string(),
                        cumulative_deposit_interest: "0".to_string(),
                        cumulative_borrow_interest: "0".to_string(),
                        in_use_count: 1,
                    }],
                    perp_positions: vec![crate::AccountPerpPositionState {
                        market_index: 0,
                        settle_pnl_limit_window: 0,
                        settle_pnl_limit_settled_in_current_window_native: "0".to_string(),
                        base_position_lots: "2".to_string(),
                        quote_position_native: "-100".to_string(),
                        quote_running_native: "0".to_string(),
                        long_settled_funding: "0".to_string(),
                        short_settled_funding: "0".to_string(),
                        open_bid_base_lots: "0".to_string(),
                        open_ask_base_lots: "0".to_string(),
                        taker_base_lots: "0".to_string(),
                        taker_quote_lots: "0".to_string(),
                        cumulative_long_funding: "0".to_string(),
                        cumulative_short_funding: "0".to_string(),
                        maker_volume: "0".to_string(),
                        taker_volume: "0".to_string(),
                        perp_spot_transfers: "0".to_string(),
                        avg_entry_price_per_base_lot: "0".to_string(),
                        oneshot_settle_pnl_allowance: "0".to_string(),
                        recurring_settle_pnl_allowance: "0".to_string(),
                        realized_pnl_for_position_native: "0".to_string(),
                    }],
                    unsupported_exposures: Vec::new(),
                },
            );

            let mut perp_markets = HashMap::new();
            perp_markets.insert(
                market.clone(),
                crate::PerpMarketSyncState {
                    market: market.clone(),
                    market_index: 0,
                    settle_token_index: 0,
                    oracle_price: "100".to_string(),
                    stable_price: "100".to_string(),
                    base_lot_size: "1".to_string(),
                    quote_lot_size: "1".to_string(),
                    maint_base_asset_weight: "1".to_string(),
                    init_base_asset_weight: "1".to_string(),
                    maint_base_liab_weight: "1".to_string(),
                    init_base_liab_weight: "1".to_string(),
                    maint_overall_asset_weight: "1".to_string(),
                    init_overall_asset_weight: "1".to_string(),
                    long_funding: "0".to_string(),
                    short_funding: "0".to_string(),
                    maker_fee: "0".to_string(),
                    taker_fee: "0".to_string(),
                },
            );

            let mut token_banks = HashMap::new();
            token_banks.insert(
                "0".to_string(),
                crate::TokenBankSyncState {
                    token_index: 0,
                    mint: key(),
                    deposit_index: "1".to_string(),
                    borrow_index: "1".to_string(),
                    oracle_price: "1".to_string(),
                    stable_price: "1".to_string(),
                    maint_asset_weight: "1".to_string(),
                    init_asset_weight: "1".to_string(),
                    init_scaled_asset_weight: "1".to_string(),
                    maint_liab_weight: "1".to_string(),
                    init_liab_weight: "1".to_string(),
                    init_scaled_liab_weight: "1".to_string(),
                },
            );

            engine
                .bootstrap_from_onchain_snapshot(EngineSnapshot {
                    view: QueueView::Confirmed,
                    markets,
                    users,
                    queue: HashMap::new(),
                    accounts,
                    perp_markets,
                    token_banks,
                    generated_ts_ms: 0,
                })
                .unwrap();

            let snapshot = engine.get_snapshot(QueueView::Confirmed).unwrap();
            let account = snapshot.accounts.get(&mango_account).unwrap();
            assert_eq!(account.token_positions.len(), 1);
            assert_eq!(account.perp_positions.len(), 1);
            let user = snapshot.users.get(&owner).unwrap();
            assert_eq!(user.per_market.len(), 1);
            assert_eq!(user.per_market[0].base_position_lots, "2");
            match &user.margin_summary {
                MarginSummary::Ok(summary) => {
                    assert_eq!(summary.source, RUST_REPLAY_MARGIN_SOURCE);
                    assert_eq!(summary.account_count, 1);
                    assert_eq!(summary.accounts[0].equity_native_quote, "1100");
                    assert_eq!(summary.accounts[0].pnl_native_quote, "100");
                    assert_eq!(summary.accounts[0].assets_native_quote, "1100");
                    assert_eq!(summary.accounts[0].liabs_native_quote, "0");
                    assert_eq!(summary.accounts[0].init_health_native_quote, "1100");
                    assert_eq!(summary.accounts[0].maint_health_native_quote, "1100");
                }
                other => panic!("expected ok margin summary, got {other:?}"),
            }
        });
    }

    #[test]
    fn rebase_prunes_absorbed_pending_intents() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "1".to_string();

            let mut markets = HashMap::new();
            markets.insert(
                market.clone(),
                MarketState {
                    market: market.clone(),
                    bids: Vec::new(),
                    asks: Vec::new(),
                    open_orders: Vec::new(),
                    watermarks: crate::types::MarketWatermarks {
                        optimistic_seq: "1".to_string(),
                        confirmed_seq: "1".to_string(),
                        last_slot: "10".to_string(),
                    },
                },
            );

            engine
                .bootstrap_from_onchain_snapshot(EngineSnapshot {
                    view: QueueView::Confirmed,
                    markets: markets.clone(),
                    users: HashMap::new(),
                    queue: HashMap::new(),
                    accounts: HashMap::new(),
                    perp_markets: HashMap::new(),
                    token_banks: HashMap::new(),
                    generated_ts_ms: 0,
                })
                .unwrap();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: 1_000,
                    group,
                    execution_queue,
                    market: market.clone(),
                    sequence: "2".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Bid,
                        100,
                        1,
                        100,
                        55,
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
                    enqueue_tx_signature: "tx-rebase-pending".to_string(),
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

            markets.get_mut(&market).unwrap().watermarks.confirmed_seq = "2".to_string();
            markets.get_mut(&market).unwrap().watermarks.optimistic_seq = "2".to_string();

            engine
                .bootstrap_from_onchain_snapshot(EngineSnapshot {
                    view: QueueView::Confirmed,
                    markets,
                    users: HashMap::new(),
                    queue: HashMap::new(),
                    accounts: HashMap::new(),
                    perp_markets: HashMap::new(),
                    token_banks: HashMap::new(),
                    generated_ts_ms: 0,
                })
                .unwrap();

            assert!(engine.list_intents().is_empty());
            assert_eq!(
                engine
                    .get_market_state(&market, QueueView::Optimistic)
                    .unwrap()
                    .open_orders
                    .len(),
                0
            );
        });
    }

    #[test]
    fn caps_intent_and_divergence_history() {
        run_with_large_stack(|| {
            let group = key();
            let mut engine = ContinuumStateEngine::new();

            for sequence in 0..(MAX_INTENT_HISTORY + MAX_DIVERGENCE_HISTORY + 16) {
                engine
                    .ingest_queue_processed(QueueItemProcessedEvent {
                        event_type: "queue_item_processed".to_string(),
                        ts_ms: sequence as u64,
                        group: group.clone(),
                        sequence: sequence.to_string(),
                        kind: 0,
                        status: QUEUE_PROCESS_FAILED,
                        slot: sequence.to_string(),
                        tx_signature: format!("tx-{sequence}"),
                    })
                    .unwrap();
            }

            assert!(engine.list_intents().len() <= MAX_INTENT_HISTORY);
            assert!(
                engine.list_divergences(MAX_DIVERGENCE_HISTORY * 4).len() <= MAX_DIVERGENCE_HISTORY
            );
        });
    }

    #[test]
    fn optimistic_pending_place_order_reclaims_now_expired_slots() {
        run_with_large_stack(|| {
            let mut engine = ContinuumStateEngine::new();
            let group = key();
            let execution_queue = key();
            let owner = key();
            let mango_account = key();
            let market = "7".to_string();
            let now_ts = now_ts_ms() / 1_000;
            let accepted_ts_ms = now_ts.saturating_sub(60) * 1_000;
            let expired_ts = now_ts.saturating_sub(10);

            let mut users = HashMap::new();
            users.insert(
                owner.clone(),
                UserState {
                    owner: owner.clone(),
                    mango_accounts: vec![mango_account.clone()],
                    open_orders: Vec::new(),
                    per_market: Vec::new(),
                    margin_summary: placeholder_margin_summary(),
                },
            );

            let mut expired_orders = Vec::new();
            for sequence in 0..64u64 {
                let order_id = ((100u128) << 64) | (!(sequence + 1) as u128);
                expired_orders.push(OpenOrderSummary {
                    order_id: order_id.to_string(),
                    owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    market: market.clone(),
                    side: "bid".to_string(),
                    price_lots: "100".to_string(),
                    base_lots: "1".to_string(),
                    quote_lots: "100".to_string(),
                    client_order_id: (1_000 + sequence).to_string(),
                    sequence: (sequence + 1).to_string(),
                    expiry_timestamp: expired_ts.to_string(),
                    status: "open".to_string(),
                });
            }

            let mut markets = HashMap::new();
            markets.insert(
                market.clone(),
                MarketState {
                    market: market.clone(),
                    bids: Vec::new(),
                    asks: Vec::new(),
                    open_orders: expired_orders,
                    watermarks: crate::types::MarketWatermarks {
                        optimistic_seq: "64".to_string(),
                        confirmed_seq: "64".to_string(),
                        last_slot: "99".to_string(),
                    },
                },
            );

            engine
                .bootstrap_from_onchain_snapshot(EngineSnapshot {
                    view: QueueView::Confirmed,
                    markets,
                    users,
                    queue: HashMap::new(),
                    accounts: HashMap::new(),
                    perp_markets: HashMap::new(),
                    token_banks: HashMap::new(),
                    generated_ts_ms: accepted_ts_ms,
                })
                .unwrap();

            engine
                .ingest_relay_intent(RelayIntentAcceptedEvent {
                    event_type: "relay_intent_accepted".to_string(),
                    ts_ms: accepted_ts_ms,
                    group,
                    execution_queue,
                    market: market.clone(),
                    sequence: "65".to_string(),
                    kind: 0,
                    payload_b64: place_order_payload(
                        Side::Bid,
                        101,
                        1,
                        101,
                        9_999,
                        PlaceOrderType::Limit,
                        SelfTradeBehavior::DecrementTake,
                        false,
                        0,
                        10,
                    ),
                    remaining_accounts: Vec::new(),
                    min_execute_slot: "100".to_string(),
                    expires_at_slot: "0".to_string(),
                    user_owner: owner.clone(),
                    mango_account: mango_account.clone(),
                    enqueue_tx_signature: "tx-expired-slot-reclaim".to_string(),
                })
                .unwrap();

            let optimistic = engine
                .get_market_state(&market, QueueView::Optimistic)
                .unwrap();
            assert_eq!(optimistic.open_orders.len(), 1);
            assert_eq!(optimistic.open_orders[0].client_order_id, "9999");

            let account = engine
                .get_snapshot(QueueView::Optimistic)
                .unwrap()
                .accounts
                .get(&mango_account)
                .cloned()
                .unwrap();
            assert_eq!(account.open_orders.len(), 1);
        });
    }
}
