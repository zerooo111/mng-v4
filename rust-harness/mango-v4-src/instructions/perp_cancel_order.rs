use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;

pub(crate) fn validate_perp_cancel_order_queue_accounts<'info>(
    group: &AccountLoader<'info, Group>,
    account: &AccountLoader<'info, MangoAccountFixed>,
    perp_market: &AccountLoader<'info, PerpMarket>,
    bids: &AccountLoader<'info, BookSide>,
    asks: &AccountLoader<'info, BookSide>,
    ix_gate: IxGate,
    require_operational: bool,
) -> Result<()> {
    let group_data = group.load()?;
    require!(group_data.is_ix_enabled(ix_gate), MangoError::IxIsDisabled);

    let group_key = group.key();
    let account_data = account.load()?;
    require!(
        account_data.group == group_key,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    if require_operational {
        require!(account_data.is_operational(), MangoError::AccountIsFrozen);
    }

    let perp_market_data = perp_market.load()?;
    require!(
        perp_market_data.group == group_key
            && perp_market_data.bids == bids.key()
            && perp_market_data.asks == asks.key(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    Ok(())
}

fn perp_cancel_order_inner<'info>(
    account: &AccountLoader<'info, MangoAccountFixed>,
    owner: Pubkey,
    perp_market: &AccountLoader<'info, PerpMarket>,
    bids: &AccountLoader<'info, BookSide>,
    asks: &AccountLoader<'info, BookSide>,
    order_id: u128,
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
        .perp_find_order_with_order_id(perp_market.perp_market_index, order_id)
        .ok_or_else(|| {
            error_msg_typed!(
                MangoError::PerpOrderIdNotFound,
                "could not find perp order with id {order_id} in user account"
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

pub(crate) fn perp_cancel_order_from_account_infos<'info>(
    dispatch_accounts: &[AccountInfo<'info>],
    order_id: u128,
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

    validate_perp_cancel_order_queue_accounts(
        &group,
        &account,
        &perp_market,
        &bids,
        &asks,
        IxGate::PerpCancelOrder,
        true,
    )?;

    perp_cancel_order_inner(&account, owner, &perp_market, &bids, &asks, order_id)
}

pub fn perp_cancel_order(ctx: Context<PerpCancelOrder>, order_id: u128) -> Result<()> {
    perp_cancel_order_inner(
        &ctx.accounts.account,
        ctx.accounts.owner.key(),
        &ctx.accounts.perp_market,
        &ctx.accounts.bids,
        &ctx.accounts.asks,
        order_id,
    )
}
