use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;

#[derive(Clone)]
pub struct PerpCancelOrderByClientOrderIdAccounts<'a, 'info> {
    pub account: &'a AccountLoader<'info, MangoAccountFixed>,
    pub perp_market: &'a AccountLoader<'info, PerpMarket>,
    pub bids: &'a AccountLoader<'info, BookSide>,
    pub asks: &'a AccountLoader<'info, BookSide>,
    pub owner_key: Pubkey,
}

pub fn perp_cancel_order_by_client_order_id(
    ctx: Context<PerpCancelOrderByClientOrderId>,
    client_order_id: u64,
) -> Result<()> {
    perp_cancel_order_by_client_order_id_logic(
        PerpCancelOrderByClientOrderIdAccounts {
            account: &ctx.accounts.account,
            perp_market: &ctx.accounts.perp_market,
            bids: &ctx.accounts.bids,
            asks: &ctx.accounts.asks,
            owner_key: ctx.accounts.owner.key(),
        },
        client_order_id,
    )
}

pub fn perp_cancel_order_by_client_order_id_prevalidated(
    accounts: PerpCancelOrderByClientOrderIdAccounts<'_, '_>,
    client_order_id: u64,
) -> Result<()> {
    perp_cancel_order_by_client_order_id_logic(accounts, client_order_id)
}

fn perp_cancel_order_by_client_order_id_logic(
    accounts: PerpCancelOrderByClientOrderIdAccounts<'_, '_>,
    client_order_id: u64,
) -> Result<()> {
    let mut account = accounts.account.load_full_mut()?;
    // account constraint #1
    require!(
        account.fixed.is_owner_or_delegate(accounts.owner_key),
        MangoError::SomeError
    );

    let perp_market = accounts.perp_market.load_mut()?;
    let mut book = Orderbook {
        bids: accounts.bids.load_mut()?,
        asks: accounts.asks.load_mut()?,
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
        accounts.account.as_ref().key,
        slot,
        perp_market.perp_market_index,
    )?;

    Ok(())
}
