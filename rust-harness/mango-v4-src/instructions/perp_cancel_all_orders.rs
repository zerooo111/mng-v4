use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::MangoError;
use crate::state::*;

fn perp_cancel_all_orders_inner<'info>(
    account: &AccountLoader<'info, MangoAccountFixed>,
    owner: Pubkey,
    perp_market: &AccountLoader<'info, PerpMarket>,
    bids: &AccountLoader<'info, BookSide>,
    asks: &AccountLoader<'info, BookSide>,
    limit: u8,
) -> Result<()> {
    let account_pk = account.key();
    let mut account = account.load_full_mut()?;
    require!(
        account.fixed.is_owner_or_delegate(owner),
        MangoError::SomeError
    );

    let mut perp_market = perp_market.load_mut()?;
    let mut book = Orderbook {
        bids: bids.load_mut()?,
        asks: asks.load_mut()?,
    };

    book.cancel_all_orders(
        &mut account.borrow_mut(),
        &account_pk,
        &mut perp_market,
        limit,
        None,
    )?;

    Ok(())
}

pub(crate) fn perp_cancel_all_orders_from_account_infos<'info>(
    dispatch_accounts: &[AccountInfo<'info>],
    limit: u8,
) -> Result<()> {
    require!(
        dispatch_accounts.len() >= 6,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let group = AccountLoader::try_from(&dispatch_accounts[0])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let account = AccountLoader::try_from(&dispatch_accounts[1])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let owner = *dispatch_accounts[2].key;
    let perp_market = AccountLoader::try_from(&dispatch_accounts[3])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let bids = AccountLoader::try_from(&dispatch_accounts[4])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let asks = AccountLoader::try_from(&dispatch_accounts[5])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;

    super::perp_cancel_order::validate_perp_cancel_order_queue_accounts(
        &group,
        &account,
        &perp_market,
        &bids,
        &asks,
        IxGate::PerpCancelAllOrders,
        false,
    )?;

    perp_cancel_all_orders_inner(&account, owner, &perp_market, &bids, &asks, limit)
}

pub fn perp_cancel_all_orders(ctx: Context<PerpCancelAllOrders>, limit: u8) -> Result<()> {
    perp_cancel_all_orders_inner(
        &ctx.accounts.account,
        ctx.accounts.owner.key(),
        &ctx.accounts.perp_market,
        &ctx.accounts.bids,
        &ctx.accounts.asks,
        limit,
    )
}
