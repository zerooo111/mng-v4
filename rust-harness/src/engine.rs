use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use anchor_lang::{prelude::Pubkey, AnchorSerialize};
use bytemuck::Zeroable;
use fixed::types::I80F48;
use mango_v4::{
    error::MangoError,
    state::{
        BookSide, BookSideOrderTree, EventQueue, EventType, FillEvent, Group, LeafNode,
        MangoAccount, MangoAccountValue, Orderbook, OutEvent, PerpMarket, PerpMarketIndex,
        PostOrderType, Side, TokenIndex,
    },
};

use crate::{
    error::{HarnessError, Result},
    payload::QueuePayload,
};

#[derive(Debug, Clone, Copy)]
pub struct MarketConfig {
    pub oracle_price: f64,
    pub stable_price: f64,
    pub base_lot_size: i64,
    pub quote_lot_size: i64,
    pub settle_token_index: TokenIndex,
    pub maint_base_asset_weight: I80F48,
    pub init_base_asset_weight: I80F48,
    pub maint_base_liab_weight: I80F48,
    pub init_base_liab_weight: I80F48,
    pub maint_overall_asset_weight: I80F48,
    pub init_overall_asset_weight: I80F48,
    pub long_funding: I80F48,
    pub short_funding: I80F48,
    pub maker_fee: I80F48,
    pub taker_fee: I80F48,
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            oracle_price: 1.0,
            stable_price: 1.0,
            base_lot_size: 1,
            quote_lot_size: 1,
            settle_token_index: 0,
            maint_base_asset_weight: I80F48::ONE,
            init_base_asset_weight: I80F48::ONE,
            maint_base_liab_weight: I80F48::ONE,
            init_base_liab_weight: I80F48::ONE,
            maint_overall_asset_weight: I80F48::ONE,
            init_overall_asset_weight: I80F48::ONE,
            long_funding: I80F48::ZERO,
            short_funding: I80F48::ZERO,
            maker_fee: I80F48::ZERO,
            taker_fee: I80F48::ZERO,
        }
    }
}

#[derive(Clone)]
struct MarketState {
    market: PerpMarket,
    oracle_price: I80F48,
    bids: RefCell<BookSide>,
    asks: RefCell<BookSide>,
    event_queue: EventQueue,
}

#[derive(Clone)]
struct AccountState {
    owner: Pubkey,
    account: MangoAccountValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookLevel {
    pub price_lots: i64,
    pub base_lots: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderbookSnapshot {
    pub market_index: PerpMarketIndex,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub open_interest: i64,
    pub event_queue_seq_num: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenOrderSnapshot {
    pub owner: Pubkey,
    pub mango_account: Pubkey,
    pub market_index: PerpMarketIndex,
    pub side: Side,
    pub order_id: u128,
    pub client_order_id: u64,
    pub price_lots: i64,
    pub base_lots: i64,
    pub quote_lots: i64,
    pub expiry_timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerpPositionSnapshot {
    pub market_index: PerpMarketIndex,
    pub settle_pnl_limit_window: u64,
    pub settle_pnl_limit_settled_in_current_window_native: String,
    pub base_position_lots: i64,
    pub quote_position_native: String,
    pub quote_running_native: String,
    pub long_settled_funding: String,
    pub short_settled_funding: String,
    pub open_bid_base_lots: i64,
    pub open_ask_base_lots: i64,
    pub taker_base_lots: i64,
    pub taker_quote_lots: i64,
    pub cumulative_long_funding: String,
    pub cumulative_short_funding: String,
    pub maker_volume: String,
    pub taker_volume: String,
    pub perp_spot_transfers: String,
    pub avg_entry_price_per_base_lot: String,
    pub oneshot_settle_pnl_allowance: String,
    pub recurring_settle_pnl_allowance: String,
    pub realized_pnl_for_position_native: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub owner: Pubkey,
    pub mango_account: Pubkey,
    pub open_orders: Vec<OpenOrderSnapshot>,
    pub perp_positions: Vec<PerpPositionSnapshot>,
}

/// Minimal per-market perp position view used by the relayer's margin
/// check hot path. Contains only the fields needed by
/// `build_harness_margin_snapshot`'s single-account overlay overwrite —
/// nothing else — so it can be materialized without walking any
/// orderbook or stringifying fields the caller doesn't read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastPerpPosition {
    pub market_index: PerpMarketIndex,
    pub base_position_lots: i64,
    pub quote_position_native: String,
    pub open_bid_base_lots: i64,
    pub open_ask_base_lots: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFill {
    pub maker: Pubkey,
    pub taker: Pubkey,
    pub taker_side: Side,
    pub maker_out: bool,
    pub maker_order_id: u128,
    pub maker_client_order_id: u64,
    pub taker_client_order_id: u64,
    pub price_lots: i64,
    pub base_lots: i64,
    pub quote_lots: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOut {
    pub owner: Pubkey,
    pub side: Side,
    pub order_id: u128,
    pub base_lots: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionResult {
    pub posted_order_id: Option<u128>,
    pub cancelled_orders: usize,
    pub fills: Vec<ExecutionFill>,
    pub outs: Vec<ExecutionOut>,
}

/// Thin off-chain engine backed by the real Mango perp orderbook primitives.
///
/// This first slice intentionally limits scope to the queue payloads the TS harness
/// already uses for optimistic perp state:
/// - place order
/// - cancel order
/// - cancel by client id
/// - cancel all / cancel all by side
///
/// Health checks, funding updates, liquidity ops, and NAPI bindings are still pending.
#[derive(Clone)]
pub struct HarnessEngine {
    group_key: Pubkey,
    group: Group,
    markets: BTreeMap<PerpMarketIndex, MarketState>,
    accounts: HashMap<Pubkey, AccountState>,
}

impl Default for HarnessEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessEngine {
    pub fn new() -> Self {
        let group_key = Pubkey::new_unique();
        let mut group = Group::zeroed();
        group.creator = group_key;
        group.admin = group_key;

        Self {
            group_key,
            group,
            markets: BTreeMap::new(),
            accounts: HashMap::new(),
        }
    }

    pub fn group_key(&self) -> Pubkey {
        self.group_key
    }

    pub fn register_market(
        &mut self,
        market_index: PerpMarketIndex,
        config: MarketConfig,
    ) -> Result<()> {
        if self.markets.contains_key(&market_index) {
            return Err(HarnessError::MarketAlreadyRegistered(market_index));
        }

        let mut market = PerpMarket::default_for_tests();
        market.group = self.group_key;
        market.perp_market_index = market_index;
        apply_market_config(&mut market, &config);

        let bids = RefCell::new(BookSide::zeroed());
        let asks = RefCell::new(BookSide::zeroed());
        {
            let mut book = Orderbook {
                bids: bids.borrow_mut(),
                asks: asks.borrow_mut(),
            };
            book.init();
        }

        self.markets.insert(
            market_index,
            MarketState {
                market,
                oracle_price: I80F48::from_num(config.oracle_price),
                bids,
                asks,
                event_queue: EventQueue::zeroed(),
            },
        );
        Ok(())
    }

    pub fn configure_market(
        &mut self,
        market_index: PerpMarketIndex,
        config: MarketConfig,
    ) -> Result<()> {
        if !self.markets.contains_key(&market_index) {
            self.register_market(market_index, config)?;
            return Ok(());
        }

        let market_state = self
            .markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        apply_market_config(&mut market_state.market, &config);
        market_state.oracle_price = I80F48::from_num(config.oracle_price);
        Ok(())
    }

    pub fn ensure_account(&mut self, mango_account: Pubkey, owner: Pubkey) -> Result<()> {
        if let Some(existing) = self.accounts.get(&mango_account) {
            if existing.owner == owner {
                return Ok(());
            }
            return Err(HarnessError::AccountOwnerMismatch {
                account: mango_account,
                existing_owner: existing.owner,
                requested_owner: owner,
            });
        }

        self.accounts.insert(
            mango_account,
            AccountState {
                owner,
                account: new_account_value(owner, self.group_key)?,
            },
        );
        Ok(())
    }

    pub fn set_oracle_price(&mut self, market_index: PerpMarketIndex, price: f64) -> Result<()> {
        let market = self
            .markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        market.oracle_price = I80F48::from_num(price);
        Ok(())
    }

    pub fn has_market(&self, market_index: PerpMarketIndex) -> bool {
        self.markets.contains_key(&market_index)
    }

    pub fn market_oracle_price_lots(&self, market_index: PerpMarketIndex) -> Result<String> {
        let market = self
            .markets
            .get(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        Ok(market.oracle_price.to_string())
    }

    pub fn market_quote_lot_size(&self, market_index: PerpMarketIndex) -> Result<i64> {
        let market = self
            .markets
            .get(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        Ok(market.market.quote_lot_size)
    }

    pub fn open_orders_snapshot_for_market(
        &self,
        market_index: PerpMarketIndex,
        now_ts: u64,
    ) -> Result<Vec<OpenOrderSnapshot>> {
        self.collect_orders_for_market(market_index, now_ts)
    }

    pub fn import_open_order(
        &mut self,
        market_index: PerpMarketIndex,
        mango_account: Pubkey,
        side: Side,
        order_id: u128,
        client_order_id: u64,
        price_lots: i64,
        base_lots: i64,
        expiry_timestamp: u64,
    ) -> Result<()> {
        if price_lots <= 0 {
            return Err(HarnessError::NegativeValue {
                field: "price_lots",
                value: price_lots,
            });
        }
        if base_lots <= 0 {
            return Err(HarnessError::NegativeValue {
                field: "base_lots",
                value: base_lots,
            });
        }
        if (order_id >> 64) as i64 != price_lots {
            return Err(HarnessError::InvalidInteger {
                field: "order_id",
                value: order_id.to_string(),
            });
        }

        let (markets, accounts) = (&mut self.markets, &mut self.accounts);
        let market_state = markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        let account_state = accounts
            .get_mut(&mango_account)
            .ok_or(HarnessError::UnknownAccount(mango_account))?;

        let (timestamp, time_in_force) = absolute_expiry_to_leaf_fields(expiry_timestamp);
        let mut account = account_state.account.borrow_mut();
        account.ensure_perp_position(market_index, market_state.market.settle_token_index)?;
        let owner_slot = account.perp_next_order_slot()?;
        let leaf = LeafNode::new(
            owner_slot as u8,
            order_id,
            mango_account,
            base_lots,
            timestamp,
            PostOrderType::Limit,
            time_in_force,
            -1,
            client_order_id,
        );

        let replaced = match side {
            Side::Bid => market_state
                .bids
                .borrow_mut()
                .insert_leaf(BookSideOrderTree::Fixed, &leaf)?,
            Side::Ask => market_state
                .asks
                .borrow_mut()
                .insert_leaf(BookSideOrderTree::Fixed, &leaf)?,
        }
        .1;
        if replaced.is_some() {
            return Err(HarnessError::DuplicateOrderId {
                market_index,
                order_id,
            });
        }

        account.add_perp_order(market_index, side, BookSideOrderTree::Fixed, &leaf)?;
        market_state.market.seq_num = market_state
            .market
            .seq_num
            .max(restored_order_sequence(side, order_id));
        Ok(())
    }

    pub fn import_perp_position_state(
        &mut self,
        mango_account: Pubkey,
        owner: Pubkey,
        market_index: PerpMarketIndex,
        settle_token_index: TokenIndex,
        state: &crate::AccountPerpPositionState,
    ) -> Result<()> {
        if !self.has_market(market_index) {
            self.register_market(
                market_index,
                MarketConfig {
                    settle_token_index,
                    ..MarketConfig::default()
                },
            )?;
        }
        self.ensure_account(mango_account, owner)?;

        let market_state = self
            .markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        market_state.market.settle_token_index = settle_token_index;
        let account_state = self
            .accounts
            .get_mut(&mango_account)
            .ok_or(HarnessError::UnknownAccount(mango_account))?;

        let account = &mut account_state.account;
        let (position, _) = account.ensure_perp_position(market_index, settle_token_index)?;
        let previous_base = position.base_position_lots();

        position.settle_pnl_limit_window = state.settle_pnl_limit_window as u32;
        position.settle_pnl_limit_settled_in_current_window_native = parse_i64_field(
            "settle_pnl_limit_settled_in_current_window_native",
            &state.settle_pnl_limit_settled_in_current_window_native,
        )?;
        position.base_position_lots =
            parse_i64_field("base_position_lots", &state.base_position_lots)?;
        position.quote_position_native =
            parse_i80f48_field("quote_position_native", &state.quote_position_native)?;
        position.quote_running_native =
            parse_i64_field("quote_running_native", &state.quote_running_native)?;
        position.long_settled_funding =
            parse_i80f48_field("long_settled_funding", &state.long_settled_funding)?;
        position.short_settled_funding =
            parse_i80f48_field("short_settled_funding", &state.short_settled_funding)?;
        position.bids_base_lots = parse_i64_field("open_bid_base_lots", &state.open_bid_base_lots)?;
        position.asks_base_lots = parse_i64_field("open_ask_base_lots", &state.open_ask_base_lots)?;
        position.taker_base_lots = parse_i64_field("taker_base_lots", &state.taker_base_lots)?;
        position.taker_quote_lots = parse_i64_field("taker_quote_lots", &state.taker_quote_lots)?;
        position.cumulative_long_funding =
            parse_f64_field("cumulative_long_funding", &state.cumulative_long_funding)?;
        position.cumulative_short_funding =
            parse_f64_field("cumulative_short_funding", &state.cumulative_short_funding)?;
        position.maker_volume = parse_u64_field("maker_volume", &state.maker_volume)?;
        position.taker_volume = parse_u64_field("taker_volume", &state.taker_volume)?;
        position.perp_spot_transfers =
            parse_i64_field("perp_spot_transfers", &state.perp_spot_transfers)?;
        position.avg_entry_price_per_base_lot = parse_f64_field(
            "avg_entry_price_per_base_lot",
            &state.avg_entry_price_per_base_lot,
        )?;
        position.oneshot_settle_pnl_allowance = parse_i80f48_field(
            "oneshot_settle_pnl_allowance",
            &state.oneshot_settle_pnl_allowance,
        )?;
        position.recurring_settle_pnl_allowance = parse_i64_field(
            "recurring_settle_pnl_allowance",
            &state.recurring_settle_pnl_allowance,
        )?;
        position.realized_pnl_for_position_native = parse_i80f48_field(
            "realized_pnl_for_position_native",
            &state.realized_pnl_for_position_native,
        )?;

        market_state.market.open_interest = market_state.market.open_interest
            + position.base_position_lots.abs()
            - previous_base.abs();
        Ok(())
    }

    pub fn prune_expired_orders(&mut self, now_ts: u64) -> Result<usize> {
        let market_indices: Vec<PerpMarketIndex> = self.markets.keys().copied().collect();
        let mut removed = 0usize;

        for market_index in market_indices {
            loop {
                let expired = {
                    let market_state = self
                        .markets
                        .get_mut(&market_index)
                        .ok_or(HarnessError::UnknownMarket(market_index))?;
                    market_state
                        .bids
                        .borrow_mut()
                        .remove_one_expired(BookSideOrderTree::Fixed, now_ts)
                };
                let Some(expired) = expired else {
                    break;
                };
                let owner = expired.owner;
                let account_state = self
                    .accounts
                    .get_mut(&owner)
                    .ok_or(HarnessError::UnknownAccount(owner))?;
                account_state
                    .account
                    .borrow_mut()
                    .remove_perp_order(expired.owner_slot as usize, expired.quantity)?;
                removed += 1;
            }

            loop {
                let expired = {
                    let market_state = self
                        .markets
                        .get_mut(&market_index)
                        .ok_or(HarnessError::UnknownMarket(market_index))?;
                    market_state
                        .asks
                        .borrow_mut()
                        .remove_one_expired(BookSideOrderTree::Fixed, now_ts)
                };
                let Some(expired) = expired else {
                    break;
                };
                let owner = expired.owner;
                let account_state = self
                    .accounts
                    .get_mut(&owner)
                    .ok_or(HarnessError::UnknownAccount(owner))?;
                account_state
                    .account
                    .borrow_mut()
                    .remove_perp_order(expired.owner_slot as usize, expired.quantity)?;
                removed += 1;
            }
        }

        Ok(removed)
    }

    pub fn execute_queue_payload(
        &mut self,
        market_index: PerpMarketIndex,
        mango_account: Pubkey,
        payload: &[u8],
        now_ts: u64,
    ) -> Result<ExecutionResult> {
        let decoded = QueuePayload::decode(payload)?;
        self.execute_decoded_payload(market_index, mango_account, decoded, now_ts)
    }

    pub fn execute_decoded_payload(
        &mut self,
        market_index: PerpMarketIndex,
        mango_account: Pubkey,
        payload: QueuePayload,
        now_ts: u64,
    ) -> Result<ExecutionResult> {
        match payload {
            QueuePayload::PerpPlaceOrderV2(place) => {
                self.execute_place_order(market_index, mango_account, place, now_ts)
            }
            QueuePayload::PerpCancelOrder(cancel) => {
                let slot = self
                    .accounts
                    .get(&mango_account)
                    .ok_or(HarnessError::UnknownAccount(mango_account))?
                    .account
                    .perp_find_order_with_order_id(market_index, cancel.order_id)
                    .map(|(slot, _)| slot);

                let mut result = ExecutionResult::default();
                if let Some(slot) = slot {
                    result.cancelled_orders =
                        self.execute_cancel_by_slot(market_index, mango_account, slot)?;
                }
                Ok(result)
            }
            QueuePayload::PerpCancelOrderByClientOrderId(cancel) => {
                let slot = self
                    .accounts
                    .get(&mango_account)
                    .ok_or(HarnessError::UnknownAccount(mango_account))?
                    .account
                    .perp_find_order_with_client_order_id(market_index, cancel.client_order_id)
                    .map(|(slot, _)| slot);

                let mut result = ExecutionResult::default();
                if let Some(slot) = slot {
                    result.cancelled_orders =
                        self.execute_cancel_by_slot(market_index, mango_account, slot)?;
                }
                Ok(result)
            }
            QueuePayload::PerpCancelAllOrders(cancel) => {
                let (markets, accounts) = (&mut self.markets, &mut self.accounts);
                let market_state = markets
                    .get_mut(&market_index)
                    .ok_or(HarnessError::UnknownMarket(market_index))?;
                let account_state = accounts
                    .get_mut(&mango_account)
                    .ok_or(HarnessError::UnknownAccount(mango_account))?;
                let before = count_open_orders(&account_state.account, market_index);

                let mut book = Orderbook {
                    bids: market_state.bids.borrow_mut(),
                    asks: market_state.asks.borrow_mut(),
                };
                let mut account = account_state.account.borrow_mut();
                book.cancel_all_orders(
                    &mut account,
                    &mango_account,
                    &mut market_state.market,
                    cancel.limit,
                    None,
                )?;

                let after = count_open_orders(&account_state.account, market_index);
                Ok(ExecutionResult {
                    cancelled_orders: before.saturating_sub(after),
                    ..ExecutionResult::default()
                })
            }
            QueuePayload::PerpCancelAllOrdersBySide(cancel) => {
                let (markets, accounts) = (&mut self.markets, &mut self.accounts);
                let market_state = markets
                    .get_mut(&market_index)
                    .ok_or(HarnessError::UnknownMarket(market_index))?;
                let account_state = accounts
                    .get_mut(&mango_account)
                    .ok_or(HarnessError::UnknownAccount(mango_account))?;
                let before = count_open_orders(&account_state.account, market_index);

                let mut book = Orderbook {
                    bids: market_state.bids.borrow_mut(),
                    asks: market_state.asks.borrow_mut(),
                };
                let mut account = account_state.account.borrow_mut();
                book.cancel_all_orders(
                    &mut account,
                    &mango_account,
                    &mut market_state.market,
                    cancel.limit,
                    cancel.side,
                )?;

                let after = count_open_orders(&account_state.account, market_index);
                Ok(ExecutionResult {
                    cancelled_orders: before.saturating_sub(after),
                    ..ExecutionResult::default()
                })
            }
            QueuePayload::LiquidityDeposit(_) => {
                Err(HarnessError::UnsupportedOperation("liquidity deposit"))
            }
            QueuePayload::LiquidityWithdraw(_) => {
                Err(HarnessError::UnsupportedOperation("liquidity withdraw"))
            }
        }
    }

    pub fn orderbook_snapshot(
        &self,
        market_index: PerpMarketIndex,
        now_ts: u64,
    ) -> Result<OrderbookSnapshot> {
        let market_state = self
            .markets
            .get(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        let orders = self.collect_orders_for_market(market_index, now_ts)?;

        let mut bids = BTreeMap::<i64, i64>::new();
        let mut asks = BTreeMap::<i64, i64>::new();
        for order in orders {
            let book = match order.side {
                Side::Bid => &mut bids,
                Side::Ask => &mut asks,
            };
            *book.entry(order.price_lots).or_insert(0) += order.base_lots;
        }

        Ok(OrderbookSnapshot {
            market_index,
            bids: bids
                .into_iter()
                .rev()
                .map(|(price_lots, base_lots)| BookLevel {
                    price_lots,
                    base_lots,
                })
                .collect(),
            asks: asks
                .into_iter()
                .map(|(price_lots, base_lots)| BookLevel {
                    price_lots,
                    base_lots,
                })
                .collect(),
            open_interest: market_state.market.open_interest,
            event_queue_seq_num: market_state.event_queue.header.seq_num,
        })
    }

    /// Phase 4-lite hot-path helper: return just the per-market perp
    /// position aggregates for a single mango account. Does NOT walk any
    /// orderbook, does NOT materialize an AccountSnapshot. Used by the
    /// relayer's margin-check hot path where only these fields are read.
    ///
    /// Cost: O(user's active perp positions), typically 1–3 entries.
    /// Measured overhead: ~5–20 µs per call vs ~50–200 ms for a full
    /// `account_snapshot` + `EngineSnapshot` rebuild.
    pub fn fast_perp_positions(
        &self,
        mango_account: Pubkey,
    ) -> Result<Vec<FastPerpPosition>> {
        let account_state = self
            .accounts
            .get(&mango_account)
            .ok_or(HarnessError::UnknownAccount(mango_account))?;
        Ok(account_state
            .account
            .all_perp_positions()
            .filter(|position| position.is_active())
            .map(|position| FastPerpPosition {
                market_index: position.market_index,
                base_position_lots: position.base_position_lots(),
                quote_position_native: position.quote_position_native().to_string(),
                open_bid_base_lots: position.bids_base_lots,
                open_ask_base_lots: position.asks_base_lots,
            })
            .collect())
    }

    pub fn account_snapshot(&self, mango_account: Pubkey, now_ts: u64) -> Result<AccountSnapshot> {
        let account_state = self
            .accounts
            .get(&mango_account)
            .ok_or(HarnessError::UnknownAccount(mango_account))?;

        let mut open_orders = Vec::new();
        for market_index in self.markets.keys() {
            open_orders.extend(
                self.collect_orders_for_market(*market_index, now_ts)?
                    .into_iter()
                    .filter(|order| order.mango_account == mango_account),
            );
        }
        open_orders.sort_by_key(|order| {
            (
                order.market_index,
                order.side as u8,
                order.price_lots,
                order.order_id,
            )
        });

        let mut perp_positions: Vec<PerpPositionSnapshot> = account_state
            .account
            .all_perp_positions()
            .filter(|position| position.is_active())
            .map(|position| {
                let market_index = position.market_index;
                let settle_pnl_limit_window = position.settle_pnl_limit_window as u64;
                let settle_pnl_limit_settled_in_current_window_native =
                    position.settle_pnl_limit_settled_in_current_window_native;
                let base_position_lots = position.base_position_lots();
                let quote_position_native = position.quote_position_native().to_string();
                let quote_running_native = position.quote_running_native;
                let long_settled_funding = position.long_settled_funding;
                let short_settled_funding = position.short_settled_funding;
                let open_bid_base_lots = position.bids_base_lots;
                let open_ask_base_lots = position.asks_base_lots;
                let taker_base_lots = position.taker_base_lots;
                let taker_quote_lots = position.taker_quote_lots;
                let cumulative_long_funding = position.cumulative_long_funding;
                let cumulative_short_funding = position.cumulative_short_funding;
                let maker_volume = position.maker_volume;
                let taker_volume = position.taker_volume;
                let perp_spot_transfers = position.perp_spot_transfers;
                let avg_entry_price_per_base_lot = position.avg_entry_price_per_base_lot;
                let oneshot_settle_pnl_allowance = position.oneshot_settle_pnl_allowance;
                let recurring_settle_pnl_allowance = position.recurring_settle_pnl_allowance;
                let realized_pnl_for_position_native = position.realized_pnl_for_position_native;
                PerpPositionSnapshot {
                    market_index,
                    settle_pnl_limit_window,
                    settle_pnl_limit_settled_in_current_window_native:
                        settle_pnl_limit_settled_in_current_window_native.to_string(),
                    base_position_lots,
                    quote_position_native,
                    quote_running_native: quote_running_native.to_string(),
                    long_settled_funding: long_settled_funding.to_string(),
                    short_settled_funding: short_settled_funding.to_string(),
                    open_bid_base_lots,
                    open_ask_base_lots,
                    taker_base_lots,
                    taker_quote_lots,
                    cumulative_long_funding: cumulative_long_funding.to_string(),
                    cumulative_short_funding: cumulative_short_funding.to_string(),
                    maker_volume: maker_volume.to_string(),
                    taker_volume: taker_volume.to_string(),
                    perp_spot_transfers: perp_spot_transfers.to_string(),
                    avg_entry_price_per_base_lot: avg_entry_price_per_base_lot.to_string(),
                    oneshot_settle_pnl_allowance: oneshot_settle_pnl_allowance.to_string(),
                    recurring_settle_pnl_allowance: recurring_settle_pnl_allowance.to_string(),
                    realized_pnl_for_position_native: realized_pnl_for_position_native.to_string(),
                }
            })
            .collect();
        perp_positions.sort_by_key(|position| position.market_index);

        Ok(AccountSnapshot {
            owner: account_state.owner,
            mango_account,
            open_orders,
            perp_positions,
        })
    }

    fn execute_place_order(
        &mut self,
        market_index: PerpMarketIndex,
        mango_account: Pubkey,
        payload: crate::payload::PerpPlaceOrderPayload,
        now_ts: u64,
    ) -> Result<ExecutionResult> {
        let Some((order, limit)) = payload.to_order(now_ts)? else {
            return Ok(ExecutionResult::default());
        };

        let (markets, accounts) = (&mut self.markets, &mut self.accounts);
        let market_state = markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        let account_state = accounts
            .get_mut(&mango_account)
            .ok_or(HarnessError::UnknownAccount(mango_account))?;

        account_state
            .account
            .ensure_perp_position(market_index, market_state.market.settle_token_index)?;

        let order_id = {
            let mut book = Orderbook {
                bids: market_state.bids.borrow_mut(),
                asks: market_state.asks.borrow_mut(),
            };
            let mut account = account_state.account.borrow_mut();
            book.new_order(
                order,
                &mut market_state.market,
                &mut market_state.event_queue,
                market_state.oracle_price,
                &mut account,
                &mango_account,
                now_ts,
                limit,
            )?
        };

        let mut result = ExecutionResult {
            posted_order_id: order_id,
            ..ExecutionResult::default()
        };
        self.consume_market_events(market_index, &mut result)?;
        Ok(result)
    }

    fn execute_cancel_by_slot(
        &mut self,
        market_index: PerpMarketIndex,
        mango_account: Pubkey,
        slot: usize,
    ) -> Result<usize> {
        let (markets, accounts) = (&mut self.markets, &mut self.accounts);
        let market_state = markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        let account_state = accounts
            .get_mut(&mango_account)
            .ok_or(HarnessError::UnknownAccount(mango_account))?;

        let mut book = Orderbook {
            bids: market_state.bids.borrow_mut(),
            asks: market_state.asks.borrow_mut(),
        };
        let mut account = account_state.account.borrow_mut();
        match book.cancel_order_by_slot(&mut account, &mango_account, slot, market_index) {
            Ok(()) => Ok(1),
            Err(anchor_lang::error::Error::AnchorError(error))
                if error.error_code_number == MangoError::PerpOrderIdNotFound.error_code() =>
            {
                Ok(0)
            }
            Err(err) => Err(err.into()),
        }
    }

    fn consume_market_events(
        &mut self,
        market_index: PerpMarketIndex,
        result: &mut ExecutionResult,
    ) -> Result<()> {
        let group = &self.group;
        let (markets, accounts) = (&mut self.markets, &mut self.accounts);
        let market_state = markets
            .get_mut(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;

        while !market_state.event_queue.is_empty() {
            let event = market_state.event_queue.pop_front()?;
            let event_type = EventType::try_from(event.event_type)
                .map_err(|_| HarnessError::UnknownEventType(event.event_type))?;

            match event_type {
                EventType::Fill => {
                    let fill = FillEvent::try_from(event)?;
                    let maker = fill.maker;
                    let taker = fill.taker;

                    if maker == taker {
                        let account_state = accounts
                            .get_mut(&maker)
                            .ok_or(HarnessError::UnknownAccount(maker))?;
                        let mut account = account_state.account.borrow_mut();
                        account.execute_perp_maker(
                            market_index,
                            &mut market_state.market,
                            &fill,
                            group,
                        )?;
                        account.execute_perp_taker(
                            market_index,
                            &mut market_state.market,
                            &fill,
                        )?;
                    } else {
                        if !accounts.contains_key(&maker) {
                            return Err(HarnessError::UnknownAccount(maker));
                        }
                        if !accounts.contains_key(&taker) {
                            return Err(HarnessError::UnknownAccount(taker));
                        }

                        let mut maker_state = accounts
                            .remove(&maker)
                            .ok_or(HarnessError::UnknownAccount(maker))?;
                        let mut taker_state = accounts
                            .remove(&taker)
                            .ok_or(HarnessError::UnknownAccount(taker))?;

                        maker_state.account.borrow_mut().execute_perp_maker(
                            market_index,
                            &mut market_state.market,
                            &fill,
                            group,
                        )?;
                        taker_state.account.borrow_mut().execute_perp_taker(
                            market_index,
                            &mut market_state.market,
                            &fill,
                        )?;

                        accounts.insert(maker, maker_state);
                        accounts.insert(taker, taker_state);
                    }

                    let price_lots = fill.price;
                    let base_lots = fill.quantity;
                    let maker_order_id = fill.maker_order_id;
                    let maker_client_order_id = fill.maker_client_order_id;
                    let taker_client_order_id = fill.taker_client_order_id;
                    result.fills.push(ExecutionFill {
                        maker,
                        taker,
                        taker_side: fill.taker_side(),
                        maker_out: fill.maker_out(),
                        maker_order_id,
                        maker_client_order_id,
                        taker_client_order_id,
                        price_lots,
                        base_lots,
                        quote_lots: price_lots.saturating_mul(base_lots),
                    });
                }
                EventType::Out => {
                    let out = OutEvent::try_from(event)?;
                    let owner = out.owner;
                    let quantity = out.quantity;
                    let order_id = out.order_id;
                    let side = out.side();

                    let account_state = accounts
                        .get_mut(&owner)
                        .ok_or(HarnessError::UnknownAccount(owner))?;
                    account_state.account.borrow_mut().execute_perp_out_event(
                        market_index,
                        side,
                        out.owner_slot as usize,
                        quantity,
                        order_id,
                    )?;

                    result.outs.push(ExecutionOut {
                        owner,
                        side,
                        order_id,
                        base_lots: quantity,
                    });
                }
                EventType::Liquidate => {}
            }
        }

        Ok(())
    }

    fn collect_orders_for_market(
        &self,
        market_index: PerpMarketIndex,
        now_ts: u64,
    ) -> Result<Vec<OpenOrderSnapshot>> {
        let market_state = self
            .markets
            .get(&market_index)
            .ok_or(HarnessError::UnknownMarket(market_index))?;
        let oracle_price_lots = market_state
            .market
            .native_price_to_lot(market_state.oracle_price);
        let mut orders = Vec::new();

        {
            let bids = market_state.bids.borrow();
            for item in bids.iter_all_including_invalid(now_ts, oracle_price_lots) {
                let mango_account = item.node.owner;
                let owner = self
                    .accounts
                    .get(&mango_account)
                    .ok_or(HarnessError::UnknownAccount(mango_account))?
                    .owner;
                let price_lots = item.price_lots;
                let base_lots = item.node.quantity;
                orders.push(OpenOrderSnapshot {
                    owner,
                    mango_account,
                    market_index,
                    side: Side::Bid,
                    order_id: item.node.key,
                    client_order_id: item.node.client_order_id,
                    price_lots,
                    base_lots,
                    quote_lots: price_lots.saturating_mul(base_lots),
                    expiry_timestamp: item.node.expiry(),
                });
            }
        }

        {
            let asks = market_state.asks.borrow();
            for item in asks.iter_all_including_invalid(now_ts, oracle_price_lots) {
                let mango_account = item.node.owner;
                let owner = self
                    .accounts
                    .get(&mango_account)
                    .ok_or(HarnessError::UnknownAccount(mango_account))?
                    .owner;
                let price_lots = item.price_lots;
                let base_lots = item.node.quantity;
                orders.push(OpenOrderSnapshot {
                    owner,
                    mango_account,
                    market_index,
                    side: Side::Ask,
                    order_id: item.node.key,
                    client_order_id: item.node.client_order_id,
                    price_lots,
                    base_lots,
                    quote_lots: price_lots.saturating_mul(base_lots),
                    expiry_timestamp: item.node.expiry(),
                });
            }
        }

        orders.sort_by_key(|order| {
            (
                order.side as u8,
                order.price_lots,
                order.order_id,
                order.client_order_id,
            )
        });
        Ok(orders)
    }
}

fn new_account_value(owner: Pubkey, group_key: Pubkey) -> Result<MangoAccountValue> {
    let mut template = MangoAccount::default_for_tests();
    template.owner = owner;
    template.group = group_key;
    template.perp_open_orders = vec![Default::default(); 64];
    let bytes = template.try_to_vec()?;
    Ok(MangoAccountValue::from_bytes(&bytes)?)
}

fn apply_market_config(market: &mut PerpMarket, config: &MarketConfig) {
    market.settle_token_index = config.settle_token_index;
    market.base_lot_size = config.base_lot_size;
    market.quote_lot_size = config.quote_lot_size;
    market.maint_base_asset_weight = config.maint_base_asset_weight;
    market.init_base_asset_weight = config.init_base_asset_weight;
    market.maint_base_liab_weight = config.maint_base_liab_weight;
    market.init_base_liab_weight = config.init_base_liab_weight;
    market.maint_overall_asset_weight = config.maint_overall_asset_weight;
    market.init_overall_asset_weight = config.init_overall_asset_weight;
    market.long_funding = config.long_funding;
    market.short_funding = config.short_funding;
    market.maker_fee = config.maker_fee;
    market.taker_fee = config.taker_fee;
    market.stable_price_model.stable_price = config.stable_price;
    market.fee_penalty = 0.0;
}

fn count_open_orders(account: &MangoAccountValue, market_index: PerpMarketIndex) -> usize {
    account
        .all_perp_orders()
        .filter(|order| order.is_active_for_market(market_index))
        .count()
}

fn absolute_expiry_to_leaf_fields(expiry_timestamp: u64) -> (u64, u16) {
    if expiry_timestamp == 0 {
        return (0, 0);
    }

    let time_in_force = expiry_timestamp.min(u16::MAX as u64) as u16;
    let timestamp = expiry_timestamp.saturating_sub(time_in_force as u64);
    (timestamp, time_in_force)
}

fn restored_order_sequence(side: Side, order_id: u128) -> u64 {
    let low = order_id as u64;
    match side {
        Side::Bid => !low,
        Side::Ask => low,
    }
}

fn parse_i80f48_field(field: &'static str, value: &str) -> Result<I80F48> {
    I80F48::from_str(value).map_err(|_| HarnessError::InvalidFixedPoint {
        field,
        value: value.to_string(),
    })
}

fn parse_i64_field(field: &'static str, value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .map_err(|_| HarnessError::InvalidInteger {
            field,
            value: value.to_string(),
        })
}

fn parse_u64_field(field: &'static str, value: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| HarnessError::InvalidInteger {
            field,
            value: value.to_string(),
        })
}

fn parse_f64_field(field: &'static str, value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .map_err(|_| HarnessError::InvalidFixedPoint {
            field,
            value: value.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mango_v4::state::{PlaceOrderType, SelfTradeBehavior};

    fn run_with_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .name("rust-harness-test".to_string())
            .stack_size(32 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    fn encode_place_order(
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
    ) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + 45);
        payload.push(1);
        payload.push(0);
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.push(side as u8);
        payload.extend_from_slice(&price_lots.to_le_bytes());
        payload.extend_from_slice(&max_base_lots.to_le_bytes());
        payload.extend_from_slice(&max_quote_lots.to_le_bytes());
        payload.extend_from_slice(&client_order_id.to_le_bytes());
        payload.push(order_type as u8);
        payload.push(self_trade_behavior as u8);
        payload.push(u8::from(reduce_only));
        payload.extend_from_slice(&expiry_timestamp.to_le_bytes());
        payload.push(limit);
        payload
    }

    fn encode_cancel_by_client_order_id(client_order_id: u64) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + 8);
        payload.push(1);
        payload.push(2);
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.extend_from_slice(&client_order_id.to_le_bytes());
        payload
    }

    #[test]
    fn posts_a_resting_bid_using_real_orderbook_logic() {
        run_with_large_stack(|| {
            let mut engine = HarnessEngine::new();
            let market_index = 7;
            let owner = Pubkey::new_unique();
            let mango_account = Pubkey::new_unique();

            engine
                .register_market(
                    market_index,
                    MarketConfig {
                        oracle_price: 100.0,
                        ..MarketConfig::default()
                    },
                )
                .unwrap();
            engine.ensure_account(mango_account, owner).unwrap();

            let payload = encode_place_order(
                Side::Bid,
                100,
                5,
                500,
                42,
                PlaceOrderType::Limit,
                SelfTradeBehavior::DecrementTake,
                false,
                0,
                10,
            );

            let result = engine
                .execute_queue_payload(market_index, mango_account, &payload, 1_000)
                .unwrap();
            assert!(result.posted_order_id.is_some());
            assert!(result.fills.is_empty());

            let book = engine.orderbook_snapshot(market_index, 1_000).unwrap();
            assert_eq!(
                book.bids,
                vec![BookLevel {
                    price_lots: 100,
                    base_lots: 5,
                }]
            );
            assert!(book.asks.is_empty());

            let account = engine.account_snapshot(mango_account, 1_000).unwrap();
            assert_eq!(account.owner, owner);
            assert_eq!(account.open_orders.len(), 1);
            assert_eq!(account.open_orders[0].client_order_id, 42);
            assert_eq!(account.open_orders[0].price_lots, 100);
            assert_eq!(account.open_orders[0].base_lots, 5);
        });
    }

    #[test]
    fn matches_crossing_orders_and_updates_positions() {
        run_with_large_stack(|| {
            let mut engine = HarnessEngine::new();
            let market_index = 11;
            let maker_owner = Pubkey::new_unique();
            let maker_account = Pubkey::new_unique();
            let taker_owner = Pubkey::new_unique();
            let taker_account = Pubkey::new_unique();

            engine
                .register_market(
                    market_index,
                    MarketConfig {
                        oracle_price: 100.0,
                        ..MarketConfig::default()
                    },
                )
                .unwrap();
            engine.ensure_account(maker_account, maker_owner).unwrap();
            engine.ensure_account(taker_account, taker_owner).unwrap();

            let maker_payload = encode_place_order(
                Side::Ask,
                100,
                5,
                500,
                1,
                PlaceOrderType::Limit,
                SelfTradeBehavior::DecrementTake,
                false,
                0,
                10,
            );
            engine
                .execute_queue_payload(market_index, maker_account, &maker_payload, 1_000)
                .unwrap();

            let taker_payload = encode_place_order(
                Side::Bid,
                110,
                5,
                550,
                2,
                PlaceOrderType::Limit,
                SelfTradeBehavior::DecrementTake,
                false,
                0,
                10,
            );
            let result = engine
                .execute_queue_payload(market_index, taker_account, &taker_payload, 1_001)
                .unwrap();

            assert_eq!(result.posted_order_id, None);
            assert_eq!(result.fills.len(), 1);
            assert!(result.outs.is_empty());

            let book = engine.orderbook_snapshot(market_index, 1_001).unwrap();
            assert!(book.bids.is_empty());
            assert!(book.asks.is_empty());

            let maker_snapshot = engine.account_snapshot(maker_account, 1_001).unwrap();
            assert!(maker_snapshot.open_orders.is_empty());
            let maker_position = maker_snapshot
                .perp_positions
                .iter()
                .find(|position| position.market_index == market_index)
                .unwrap();
            assert_eq!(maker_position.base_position_lots, -5);
            assert_eq!(maker_position.quote_position_native, "500");

            let taker_snapshot = engine.account_snapshot(taker_account, 1_001).unwrap();
            assert!(taker_snapshot.open_orders.is_empty());
            let taker_position = taker_snapshot
                .perp_positions
                .iter()
                .find(|position| position.market_index == market_index)
                .unwrap();
            assert_eq!(taker_position.base_position_lots, 5);
            assert_eq!(taker_position.quote_position_native, "-500");
        });
    }

    #[test]
    fn cancels_by_client_order_id() {
        run_with_large_stack(|| {
            let mut engine = HarnessEngine::new();
            let market_index = 3;
            let owner = Pubkey::new_unique();
            let mango_account = Pubkey::new_unique();

            engine
                .register_market(
                    market_index,
                    MarketConfig {
                        oracle_price: 25.0,
                        ..MarketConfig::default()
                    },
                )
                .unwrap();
            engine.ensure_account(mango_account, owner).unwrap();

            let place_payload = encode_place_order(
                Side::Bid,
                25,
                2,
                50,
                99,
                PlaceOrderType::Limit,
                SelfTradeBehavior::DecrementTake,
                false,
                0,
                10,
            );
            engine
                .execute_queue_payload(market_index, mango_account, &place_payload, 500)
                .unwrap();

            let cancel_payload = encode_cancel_by_client_order_id(99);
            let result = engine
                .execute_queue_payload(market_index, mango_account, &cancel_payload, 501)
                .unwrap();
            assert_eq!(result.cancelled_orders, 1);

            let book = engine.orderbook_snapshot(market_index, 501).unwrap();
            assert!(book.bids.is_empty());

            let account = engine.account_snapshot(mango_account, 501).unwrap();
            assert!(account.open_orders.is_empty());
        });
    }

    #[test]
    fn prunes_expired_orders_and_frees_account_slots() {
        run_with_large_stack(|| {
            let mut engine = HarnessEngine::new();
            let market_index = 23;
            let owner = Pubkey::new_unique();
            let mango_account = Pubkey::new_unique();
            let now_ts = 10_000u64;
            let expired_ts = now_ts - 10;

            engine
                .register_market(
                    market_index,
                    MarketConfig {
                        oracle_price: 100.0,
                        ..MarketConfig::default()
                    },
                )
                .unwrap();
            engine.ensure_account(mango_account, owner).unwrap();

            for sequence in 0..64u64 {
                let order_id = ((100u128) << 64) | (!(sequence + 1) as u128);
                engine
                    .import_open_order(
                        market_index,
                        mango_account,
                        Side::Bid,
                        order_id,
                        sequence + 1,
                        100,
                        1,
                        expired_ts,
                    )
                    .unwrap();
            }

            assert_eq!(engine.prune_expired_orders(now_ts).unwrap(), 64);
            assert!(engine
                .account_snapshot(mango_account, now_ts)
                .unwrap()
                .open_orders
                .is_empty());

            let payload = encode_place_order(
                Side::Bid,
                100,
                1,
                100,
                9_999,
                PlaceOrderType::Limit,
                SelfTradeBehavior::DecrementTake,
                false,
                0,
                10,
            );
            let result = engine
                .execute_queue_payload(market_index, mango_account, &payload, now_ts)
                .unwrap();
            assert!(result.posted_order_id.is_some());
            assert_eq!(
                engine
                    .account_snapshot(mango_account, now_ts)
                    .unwrap()
                    .open_orders
                    .len(),
                1
            );
        });
    }

    #[test]
    fn snapshot_retains_lazy_expired_orders_until_pruned() {
        run_with_large_stack(|| {
            let mut engine = HarnessEngine::new();
            let market_index = 24;
            let owner = Pubkey::new_unique();
            let mango_account = Pubkey::new_unique();
            let now_ts = 10_000u64;
            let expired_ts = now_ts - 10;
            let order_id = ((100u128) << 64) | (!1u64 as u128);

            engine
                .register_market(
                    market_index,
                    MarketConfig {
                        oracle_price: 100.0,
                        ..MarketConfig::default()
                    },
                )
                .unwrap();
            engine.ensure_account(mango_account, owner).unwrap();
            engine
                .import_open_order(
                    market_index,
                    mango_account,
                    Side::Bid,
                    order_id,
                    1,
                    100,
                    1,
                    expired_ts,
                )
                .unwrap();

            let pre_prune_book = engine.orderbook_snapshot(market_index, now_ts).unwrap();
            assert_eq!(pre_prune_book.bids.len(), 1);
            let pre_prune_account = engine.account_snapshot(mango_account, now_ts).unwrap();
            assert_eq!(pre_prune_account.open_orders.len(), 1);

            assert_eq!(engine.prune_expired_orders(now_ts).unwrap(), 1);

            let post_prune_book = engine.orderbook_snapshot(market_index, now_ts).unwrap();
            assert!(post_prune_book.bids.is_empty());
            let post_prune_account = engine.account_snapshot(mango_account, now_ts).unwrap();
            assert!(post_prune_account.open_orders.is_empty());
        });
    }
}
