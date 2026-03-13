use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;

fn perp_cancel_order_by_client_order_id_inner<'info>(
    account: &AccountLoader<'info, MangoAccountFixed>,
    owner: Pubkey,
    perp_market: &AccountLoader<'info, PerpMarket>,
    bids: &AccountLoader<'info, BookSide>,
    asks: &AccountLoader<'info, BookSide>,
    client_order_id: u64,
) -> Result<()> {
    let account_pk = account.key();
    let mut account = account.load_full_mut()?;
    require!(
        account.fixed.is_owner_or_delegate(owner),
        MangoError::SomeError
    );

    let perp_market = perp_market.load_mut()?;
    let mut book = Orderbook {
        bids: bids.load_mut()?,
        asks: asks.load_mut()?,
    };

    let (slot, _) = account
        .perp_find_order_with_client_order_id(perp_market.perp_market_index, client_order_id)
        .ok_or_else(|| {
            error_msg_typed!(
                MangoError::PerpOrderIdNotFound,
                "could not find perp order with client order id {client_order_id} in user account"
            )
        })?;

    book.cancel_order_by_slot(
        &mut account.borrow_mut(),
        &account_pk,
        slot,
        perp_market.perp_market_index,
    )?;

    Ok(())
}

pub(crate) fn perp_cancel_order_by_client_order_id_from_account_infos<'info>(
    dispatch_accounts: &[AccountInfo<'info>],
    client_order_id: u64,
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
        IxGate::PerpCancelOrderByClientOrderId,
        false,
    )?;

    perp_cancel_order_by_client_order_id_inner(
        &account,
        owner,
        &perp_market,
        &bids,
        &asks,
        client_order_id,
    )
}

pub fn perp_cancel_order_by_client_order_id(
    ctx: Context<PerpCancelOrderByClientOrderId>,
    client_order_id: u64,
) -> Result<()> {
    perp_cancel_order_by_client_order_id_inner(
        &ctx.accounts.account,
        ctx.accounts.owner.key(),
        &ctx.accounts.perp_market,
        &ctx.accounts.bids,
        &ctx.accounts.asks,
        client_order_id,
    )
}
