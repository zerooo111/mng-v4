use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueView {
    Optimistic,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayIntentAcceptedEvent {
    pub event_type: String,
    pub ts_ms: u64,
    pub group: String,
    pub execution_queue: String,
    pub market: String,
    pub sequence: String,
    pub kind: u8,
    pub payload_b64: String,
    #[serde(default)]
    pub remaining_accounts: Vec<AccountMetaWire>,
    pub min_execute_slot: String,
    pub expires_at_slot: String,
    pub user_owner: String,
    pub mango_account: String,
    pub enqueue_tx_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItemEnqueuedEvent {
    pub event_type: String,
    pub ts_ms: u64,
    pub group: String,
    pub sequence: String,
    pub kind: u8,
    pub min_execute_slot: String,
    pub slot: String,
    pub tx_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItemProcessedEvent {
    pub event_type: String,
    pub ts_ms: u64,
    pub group: String,
    pub sequence: String,
    pub kind: u8,
    pub status: u8,
    pub slot: String,
    pub tx_signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMetaWire {
    pub pubkey: String,
    pub is_signer: bool,
    pub is_writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenOrderSummary {
    pub order_id: String,
    pub owner: String,
    pub mango_account: String,
    pub market: String,
    pub side: String,
    pub price_lots: String,
    pub base_lots: String,
    pub quote_lots: String,
    pub client_order_id: String,
    pub sequence: String,
    pub expiry_timestamp: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketLevel {
    pub price_lots: String,
    pub base_lots: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketWatermarks {
    pub optimistic_seq: String,
    pub confirmed_seq: String,
    pub last_slot: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketState {
    pub market: String,
    pub bids: Vec<MarketLevel>,
    pub asks: Vec<MarketLevel>,
    pub open_orders: Vec<OpenOrderSummary>,
    pub watermarks: MarketWatermarks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPerMarketState {
    pub market: String,
    pub open_order_base_lots_bid: String,
    pub open_order_base_lots_ask: String,
    pub quote_reserved_lots: String,
    pub base_position_lots: String,
    pub quote_position_native: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSummaryPlaceholder {
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSummaryEmptyTotals {
    pub equity_native_quote: String,
    pub pnl_native_quote: String,
    pub assets_native_quote: String,
    pub liabs_native_quote: String,
    pub init_health_native_quote: String,
    pub maint_health_native_quote: String,
    pub margin_usage_fraction: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSummaryEmpty {
    pub source: String,
    pub account_count: u64,
    pub totals: MarginSummaryEmptyTotals,
    pub accounts: Vec<MarginSummaryAccount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSummaryAccountPerpPosition {
    pub market_index: u64,
    pub base_position_lots: String,
    pub quote_position_native: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSummaryAccount {
    pub mango_account: String,
    pub owner: String,
    pub equity_native_quote: String,
    pub pnl_native_quote: String,
    pub assets_native_quote: String,
    pub liabs_native_quote: String,
    pub init_health_native_quote: String,
    pub maint_health_native_quote: String,
    pub init_health_ratio: String,
    pub maint_health_ratio: String,
    pub margin_usage_fraction: f64,
    pub perp_positions: Vec<MarginSummaryAccountPerpPosition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSummaryOk {
    pub source: String,
    pub account_count: u64,
    pub totals: MarginSummaryEmptyTotals,
    #[serde(default)]
    pub equity_native_quote: Option<String>,
    #[serde(default)]
    pub pnl_native_quote: Option<String>,
    #[serde(default)]
    pub assets_native_quote: Option<String>,
    #[serde(default)]
    pub liabs_native_quote: Option<String>,
    #[serde(default)]
    pub init_health_native_quote: Option<String>,
    #[serde(default)]
    pub maint_health_native_quote: Option<String>,
    #[serde(default)]
    pub margin_usage_fraction: Option<f64>,
    pub accounts: Vec<MarginSummaryAccount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum MarginSummary {
    #[serde(rename = "placeholder")]
    Placeholder(MarginSummaryPlaceholder),
    #[serde(rename = "empty")]
    Empty(MarginSummaryEmpty),
    #[serde(rename = "ok")]
    Ok(MarginSummaryOk),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserState {
    pub owner: String,
    pub mango_accounts: Vec<String>,
    pub open_orders: Vec<OpenOrderSummary>,
    pub per_market: Vec<UserPerMarketState>,
    pub margin_summary: MarginSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueState {
    pub market: String,
    pub pending_count: u64,
    pub processed_count: u64,
    pub failed_count: u64,
    pub skipped_count: u64,
    pub last_processed_sequence: String,
    pub lag_slots: String,
    pub unmatched_processed_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketTrade {
    pub trade_id: String,
    pub market: String,
    pub price_lots: String,
    pub base_lots: String,
    pub quote_lots: String,
    pub taker_side: String,
    pub maker_owner: String,
    pub taker_owner: String,
    pub maker_order_id: String,
    pub taker_sequence: String,
    pub ts_ms: u64,
    pub view: QueueView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketCandle {
    pub market: String,
    pub bucket_start_ts_ms: u64,
    pub resolution_sec: u64,
    pub open_price_lots: String,
    pub high_price_lots: String,
    pub low_price_lots: String,
    pub close_price_lots: String,
    pub base_volume_lots: String,
    pub quote_volume_lots: String,
    pub trade_count: u64,
    pub view: QueueView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserBalanceTotals {
    pub total_open_order_base_lots_bid: String,
    pub total_open_order_base_lots_ask: String,
    pub total_quote_reserved_lots: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserBalances {
    pub owner: String,
    pub mango_accounts: Vec<String>,
    pub per_market: Vec<UserPerMarketState>,
    pub totals: UserBalanceTotals,
    pub margin_summary: MarginSummary,
    pub view: QueueView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub view: QueueView,
    pub markets: HashMap<String, MarketState>,
    pub users: HashMap<String, UserState>,
    pub queue: HashMap<String, QueueState>,
    pub generated_ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergenceEvent {
    pub event_type: String,
    pub ts_ms: u64,
    pub reason: String,
    pub key: String,
    pub details: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalIntentState {
    pub key: String,
    pub group: String,
    pub execution_queue: String,
    pub market: String,
    pub sequence: String,
    pub kind: u8,
    pub payload_b64: String,
    #[serde(default)]
    pub decoded_payload: Option<Value>,
    #[serde(default)]
    pub remaining_accounts: Vec<AccountMetaWire>,
    pub min_execute_slot: String,
    pub expires_at_slot: String,
    pub user_owner: String,
    pub mango_account: String,
    pub enqueue_tx_signature: String,
    pub accepted_ts_ms: u64,
    #[serde(default)]
    pub enqueued_slot: Option<String>,
    #[serde(default)]
    pub processed_slot: Option<String>,
    #[serde(default)]
    pub processed_status: Option<u8>,
    #[serde(default)]
    pub processed_tx_signature: Option<String>,
}
