use anchor_lang::prelude::*;
use fixed::types::I80F48;

use crate::accounts_ix::*;
use crate::accounts_zerocopy::*;
use crate::error::*;
use crate::health::*;
use crate::state::*;
use crate::util::clock_now;

pub(crate) fn perp_place_order_with_loaded_context<'a>(
    account: &mut MangoAccountRefMut,
    account_pk: &Pubkey,
    perp_market: &mut PerpMarket,
    book: &mut Orderbook<'a>,
    event_queue: &mut EventQueue,
    oracle_price: I80F48,
    mut order: Order,
    now_ts: u64,
    buyback_fees_expiry_interval: u64,
    limit: u8,
) -> Result<Option<u128>> {
    account
        .fixed
        .expire_buyback_fees(now_ts, buyback_fees_expiry_interval);

    let pp = account.perp_position(perp_market.perp_market_index)?;
    let max_base_lots = if order.reduce_only || perp_market.is_reduce_only() {
        reduce_only_max_base_lots(pp, &order, perp_market.is_reduce_only())
    } else {
        order.max_base_lots
    };
    if perp_market.is_reduce_only() {
        require!(
            order.reduce_only || max_base_lots == order.max_base_lots,
            MangoError::MarketInReduceOnlyMode
        )
    };
    order.max_base_lots = max_base_lots;

    book.new_order(
        order,
        perp_market,
        event_queue,
        oracle_price,
        account,
        account_pk,
        now_ts,
        limit,
    )
}

fn perp_place_order_inner<'info>(
    group: AccountLoader<'info, Group>,
    account: AccountLoader<'info, MangoAccountFixed>,
    account_ai: &AccountInfo<'info>,
    owner: Pubkey,
    perp_market: AccountLoader<'info, PerpMarket>,
    bids: AccountLoader<'info, BookSide>,
    asks: AccountLoader<'info, BookSide>,
    event_queue: AccountLoader<'info, EventQueue>,
    oracle: AccountInfo<'info>,
    remaining_accounts: &[AccountInfo<'info>],
    order: Order,
    limit: u8,
) -> Result<Option<u128>> {
    require_gte!(order.max_base_lots, 0);
    require_gte!(order.max_quote_lots, 0);
    let group_key = group.key();
    let account_pk = account.key();
    let (sidecar_ai_opt, health_accounts) =
        split_optional_risk_sidecar_account(group_key, account_pk, remaining_accounts);

    let (now_ts, now_slot) = clock_now();
    let oracle_price;

    // Update funding if possible.
    //
    // Doing this automatically here makes it impossible for attackers to add orders to the orderbook
    // before triggering the funding computation.
    {
        let mut perp_market = perp_market.load_mut()?;
        let book = Orderbook {
            bids: bids.load_mut()?,
            asks: asks.load_mut()?,
        };

        let oracle_ref = &AccountInfoRef::borrow(&oracle)?;
        let oracle_state = perp_market.oracle_state(
            &OracleAccountInfos::from_reader(oracle_ref),
            None, // staleness checked in health
        )?;
        oracle_price = oracle_state.price;

        perp_market.update_funding_and_stable_price(&book, &oracle_state, now_ts)?;
    }

    let (perp_market_index, settle_token_index) = {
        let perp_market = perp_market.load()?;
        (
            perp_market.perp_market_index,
            perp_market.settle_token_index,
        )
    };

    {
        let mut account = account.load_full_mut()?;
        require!(
            account.fixed.is_owner_or_delegate(owner),
            MangoError::SomeError
        );
        account.ensure_perp_position(perp_market_index, settle_token_index)?;
    }

    let account_for_hash = account.load_full()?;
    let account_hash_ref = account_for_hash.borrow();
    let pre_account_state_hash = risk_sidecar_account_state_hash(&account_hash_ref)?;
    let pre_health_accounts_state_hash =
        risk_sidecar_health_accounts_state_hash(&account_hash_ref, health_accounts, now_slot, now_ts)?;
    drop(account_hash_ref);
    drop(account_for_hash);
    let account_loader = AccountLoader::try_from(account_ai)
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let mut sidecar_account_opt =
        load_risk_sidecar_account(group_key, account_pk, sidecar_ai_opt)?;
    let mut account = account.load_full_mut()?;

    let mut pre_health_opt = if !account.fixed.is_in_health_region() {
        let mut account_ref = account.borrow_mut();
        let session = if let Some(snapshot) = sidecar_account_opt
            .as_ref()
            .map(|sidecar| {
                matching_risk_sidecar_snapshot(
                    sidecar,
                    group_key,
                    account_pk,
                    pre_account_state_hash,
                    pre_health_accounts_state_hash,
                )
            })
            .transpose()?
            .flatten()
        {
            ExactRiskSidecarSession::prepare_from_snapshot(account_pk, &mut account_ref, &snapshot)
                .context("pre init health from risk sidecar")?
        } else {
            ExactRiskSidecarSession::prepare_from_fixed_accounts(
                account_pk,
                &mut account_ref,
                health_accounts,
                now_slot,
                now_ts,
            )
            .context("pre init health")?
        };
        session.require_token_info(settle_token_index)?;
        Some(session)
    } else {
        None
    };

    let order_id_opt;
    {
        let mut perp_market = perp_market.load_mut()?;
        let mut book = Orderbook {
            bids: bids.load_mut()?,
            asks: asks.load_mut()?,
        };
        let mut event_queue = event_queue.load_mut()?;
        let group = group.load()?;
        let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
        order_id_opt = perp_place_order_with_loaded_context(
            &mut account.borrow_mut(),
            &account_pk,
            &mut perp_market,
            &mut book,
            &mut event_queue,
            oracle_price,
            order,
            now_ts,
            group.buyback_fees_expiry_interval,
            limit,
        )?;

        if let Some(risk_session) = pre_health_opt.as_mut() {
            let mut account_ref = account.borrow_mut();
            risk_session.recompute_perp_and_check_post(&mut account_ref, &perp_market)?;
        }
    }

    let snapshot_opt = if let Some(risk_session) = pre_health_opt.take() {
        Some(risk_session.into_snapshot(&account.borrow()))
    } else {
        None
    };
    drop(account);

    if let (Some(snapshot), Some(sidecar_account)) = (snapshot_opt.as_ref(), sidecar_account_opt.as_mut()) {
        if sidecar_account.to_account_info().is_writable {
            let clock = Clock::get()?;
            let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
            let account_for_hash = account_loader.load_full()?;
            let account_hash_ref = account_for_hash.borrow();
            let post_account_state_hash = risk_sidecar_account_state_hash(&account_hash_ref)?;
            let post_health_accounts_state_hash = risk_sidecar_health_accounts_state_hash(
                &account_hash_ref,
                health_accounts,
                clock.slot,
                now_ts,
            )?;
            refresh_risk_sidecar_account(
                sidecar_account,
                snapshot,
                post_account_state_hash,
                post_health_accounts_state_hash,
                clock.slot,
                now_ts,
                false,
            )?;
        }
    }

    Ok(order_id_opt)
}

pub(crate) fn validate_perp_place_order_queue_accounts<'info>(
    group: &AccountLoader<'info, Group>,
    account: &AccountLoader<'info, MangoAccountFixed>,
    perp_market: &AccountLoader<'info, PerpMarket>,
    bids: &AccountLoader<'info, BookSide>,
    asks: &AccountLoader<'info, BookSide>,
    event_queue: &AccountLoader<'info, EventQueue>,
    oracle: &AccountInfo<'info>,
) -> Result<()> {
    let group_data = group.load()?;
    require!(
        group_data.is_ix_enabled(IxGate::PerpPlaceOrder),
        MangoError::IxIsDisabled
    );

    let group_key = group.key();
    let account_data = account.load()?;
    require!(
        account_data.group == group_key,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    require!(account_data.is_operational(), MangoError::AccountIsFrozen);

    let perp_market_data = perp_market.load()?;
    require!(
        perp_market_data.group == group_key
            && perp_market_data.bids == bids.key()
            && perp_market_data.asks == asks.key()
            && perp_market_data.event_queue == event_queue.key()
            && perp_market_data.oracle == oracle.key(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    Ok(())
}

pub(crate) fn perp_place_order_from_account_infos<'info>(
    dispatch_accounts: &[AccountInfo<'info>],
    order: Order,
    limit: u8,
) -> Result<Option<u128>> {
    require!(
        dispatch_accounts.len() >= 8,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let group = AccountLoader::try_from(&dispatch_accounts[0])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let account = AccountLoader::try_from(&dispatch_accounts[1])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let owner = *dispatch_accounts[2].key;
    // NOTE: owner is validated against account.fixed.is_owner_or_delegate(owner)
    // inside perp_place_order_inner. Combined with the accounts_hash integrity check
    // at enqueue time (and the C-1 fix computing hashes from actual accounts in
    // execute_multi), this prevents account impersonation.
    let perp_market = AccountLoader::try_from(&dispatch_accounts[3])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let bids = AccountLoader::try_from(&dispatch_accounts[4])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let asks = AccountLoader::try_from(&dispatch_accounts[5])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let event_queue = AccountLoader::try_from(&dispatch_accounts[6])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let oracle = &dispatch_accounts[7];

    validate_perp_place_order_queue_accounts(
        &group,
        &account,
        &perp_market,
        &bids,
        &asks,
        &event_queue,
        oracle,
    )?;

    perp_place_order_inner(
        group,
        account,
        &dispatch_accounts[1],
        owner,
        perp_market,
        bids,
        asks,
        event_queue,
        oracle.clone(),
        &dispatch_accounts[8..],
        order,
        limit,
    )
}

// TODO
#[allow(clippy::too_many_arguments)]
pub fn perp_place_order(
    ctx: Context<PerpPlaceOrder>,
    mut order: Order,
    limit: u8,
) -> Result<Option<u128>> {
    require_gte!(order.max_base_lots, 0);
    require_gte!(order.max_quote_lots, 0);

    let (now_ts, now_slot) = clock_now();
    let group_key = ctx.accounts.group.key();
    let account_pk = ctx.accounts.account.key();
    let (sidecar_ai_opt, health_accounts) =
        split_optional_risk_sidecar_account(group_key, account_pk, ctx.remaining_accounts);
    let oracle_price;

    {
        let mut perp_market = ctx.accounts.perp_market.load_mut()?;
        let book = Orderbook {
            bids: ctx.accounts.bids.load_mut()?,
            asks: ctx.accounts.asks.load_mut()?,
        };

        let oracle_ref = &AccountInfoRef::borrow(ctx.accounts.oracle.as_ref())?;
        let oracle_state =
            perp_market.oracle_state(&OracleAccountInfos::from_reader(oracle_ref), None)?;
        oracle_price = oracle_state.price;

        perp_market.update_funding_and_stable_price(&book, &oracle_state, now_ts)?;
    }

    let (perp_market_index, settle_token_index) = {
        let perp_market = ctx.accounts.perp_market.load()?;
        (
            perp_market.perp_market_index,
            perp_market.settle_token_index,
        )
    };

    {
        let mut account = ctx.accounts.account.load_full_mut()?;
        require!(
            account.fixed.is_owner_or_delegate(ctx.accounts.owner.key()),
            MangoError::SomeError
        );
        account.ensure_perp_position(perp_market_index, settle_token_index)?;
    }

    let account_for_hash = ctx.accounts.account.load_full()?;
    let account_hash_ref = account_for_hash.borrow();
    let pre_account_state_hash = risk_sidecar_account_state_hash(&account_hash_ref)?;
    let pre_health_accounts_state_hash =
        risk_sidecar_health_accounts_state_hash(&account_hash_ref, health_accounts, now_slot, now_ts)?;
    drop(account_hash_ref);
    drop(account_for_hash);
    let mut sidecar_account_opt =
        load_risk_sidecar_account(group_key, account_pk, sidecar_ai_opt)?;
    let mut account = ctx.accounts.account.load_full_mut()?;

    let mut pre_health_opt = if !account.fixed.is_in_health_region() {
        let mut account_ref = account.borrow_mut();
        let session = if let Some(snapshot) = sidecar_account_opt
            .as_ref()
            .map(|sidecar| {
                matching_risk_sidecar_snapshot(
                    sidecar,
                    group_key,
                    account_pk,
                    pre_account_state_hash,
                    pre_health_accounts_state_hash,
                )
            })
            .transpose()?
            .flatten()
        {
            ExactRiskSidecarSession::prepare_from_snapshot(account_pk, &mut account_ref, &snapshot)
                .context("pre init health from risk sidecar")?
        } else {
            ExactRiskSidecarSession::prepare_from_fixed_accounts(
                account_pk,
                &mut account_ref,
                health_accounts,
                now_slot,
                now_ts,
            )
            .context("pre init health")?
        };
        session.require_token_info(settle_token_index)?;
        Some(session)
    } else {
        None
    };

    let order_id_opt;
    {
        let mut perp_market = ctx.accounts.perp_market.load_mut()?;
        let mut book = Orderbook {
            bids: ctx.accounts.bids.load_mut()?,
            asks: ctx.accounts.asks.load_mut()?,
        };
        let mut event_queue = ctx.accounts.event_queue.load_mut()?;
        let group = ctx.accounts.group.load()?;

        let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
        account
            .fixed
            .expire_buyback_fees(now_ts, group.buyback_fees_expiry_interval);

        let pp = account.perp_position(perp_market_index)?;
        let max_base_lots = if order.reduce_only || perp_market.is_reduce_only() {
            reduce_only_max_base_lots(pp, &order, perp_market.is_reduce_only())
        } else {
            order.max_base_lots
        };
        if perp_market.is_reduce_only() {
            require!(
                order.reduce_only || max_base_lots == order.max_base_lots,
                MangoError::MarketInReduceOnlyMode
            )
        };
        order.max_base_lots = max_base_lots;

        order_id_opt = book.new_order(
            order,
            &mut perp_market,
            &mut event_queue,
            oracle_price,
            &mut account.borrow_mut(),
            &account_pk,
            now_ts,
            limit,
        )?;

        if let Some(risk_session) = pre_health_opt.as_mut() {
            let mut account_ref = account.borrow_mut();
            risk_session.recompute_perp_and_check_post(&mut account_ref, &perp_market)?;
        }
    }

    let snapshot_opt = if let Some(risk_session) = pre_health_opt.take() {
        Some(risk_session.into_snapshot(&account.borrow()))
    } else {
        None
    };
    drop(account);
    if let (Some(snapshot), Some(sidecar_account)) = (snapshot_opt.as_ref(), sidecar_account_opt.as_mut()) {
        if sidecar_account.to_account_info().is_writable {
            let clock = Clock::get()?;
            let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
            let account_for_hash = ctx.accounts.account.load_full()?;
            let account_hash_ref = account_for_hash.borrow();
            let post_account_state_hash = risk_sidecar_account_state_hash(&account_hash_ref)?;
            let post_health_accounts_state_hash = risk_sidecar_health_accounts_state_hash(
                &account_hash_ref,
                health_accounts,
                clock.slot,
                now_ts,
            )?;
            refresh_risk_sidecar_account(
                sidecar_account,
                snapshot,
                post_account_state_hash,
                post_health_accounts_state_hash,
                clock.slot,
                now_ts,
                false,
            )?;
        }
    }

    Ok(order_id_opt)
}

fn reduce_only_max_base_lots(pp: &PerpPosition, order: &Order, market_reduce_only: bool) -> i64 {
    let effective_pos = pp.effective_base_position_lots();
    msg!(
        "reduce only: current effective position: {} lots",
        effective_pos
    );
    let allowed_base_lots = if (order.side == Side::Bid && effective_pos >= 0)
        || (order.side == Side::Ask && effective_pos <= 0)
    {
        msg!("reduce only: cannot increase magnitude of effective position");
        0
    } else if market_reduce_only {
        // If the market is in reduce-only mode, we are stricter and pretend
        // all open orders that go into the same direction as the new order
        // execute.
        if order.side == Side::Bid {
            msg!(
                "reduce only: effective base position incl open bids is {} lots",
                effective_pos + pp.bids_base_lots
            );
            (effective_pos + pp.bids_base_lots).min(0).abs()
        } else {
            msg!(
                "reduce only: effective base position incl open asks is {} lots",
                effective_pos - pp.asks_base_lots
            );
            (effective_pos - pp.asks_base_lots).max(0)
        }
    } else {
        effective_pos.abs()
    };
    msg!(
        "reduce only: max allowed {:?}: {} base lots",
        order.side,
        allowed_base_lots
    );
    allowed_base_lots.min(order.max_base_lots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perp_reduce_only() {
        let test_cases = vec![
            ("null", true, 0, (0, 0), (Side::Bid, 0), 0),
            ("ok bid", true, -5, (0, 0), (Side::Bid, 1), 1),
            ("limited bid", true, -5, (0, 0), (Side::Bid, 10), 5),
            ("limited bid2", true, -5, (1, 10), (Side::Bid, 10), 4),
            ("limited bid3", false, -5, (1, 10), (Side::Bid, 10), 5),
            ("no bid", true, 5, (0, 0), (Side::Bid, 1), 0),
            ("ok ask", true, 5, (0, 0), (Side::Ask, 1), 1),
            ("limited ask", true, 5, (0, 0), (Side::Ask, 10), 5),
            ("limited ask2", true, 5, (10, 1), (Side::Ask, 10), 4),
            ("limited ask3", false, 5, (10, 1), (Side::Ask, 10), 5),
            ("no ask", true, -5, (0, 0), (Side::Ask, 1), 0),
        ];

        for (
            name,
            market_reduce_only,
            base_lots,
            (open_bids, open_asks),
            (side, amount),
            expected,
        ) in test_cases
        {
            println!("test: {name}");

            let pp = PerpPosition {
                base_position_lots: base_lots,
                bids_base_lots: open_bids,
                asks_base_lots: open_asks,
                ..PerpPosition::default()
            };
            let order = Order {
                side,
                max_base_lots: amount,
                max_quote_lots: 0,
                client_order_id: 0,
                reduce_only: true,
                time_in_force: 0,
                self_trade_behavior: SelfTradeBehavior::DecrementTake,
                params: OrderParams::Market {},
            };

            let result = reduce_only_max_base_lots(&pp, &order, market_reduce_only);
            assert_eq!(result, expected);
        }
    }
}
