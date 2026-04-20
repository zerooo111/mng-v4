use anchor_lang::prelude::*;

use crate::state::*;

#[derive(Accounts)]
pub struct PerpAdminRepairStaleOrders<'info> {
    #[account(
        constraint = group.load()?.is_testing(),
        has_one = admin,
    )]
    pub group: AccountLoader<'info, Group>,
    pub admin: Signer<'info>,

    #[account(
        mut,
        has_one = group,
    )]
    pub account: AccountLoader<'info, MangoAccountFixed>,

    #[account(
        has_one = group,
        has_one = bids,
        has_one = asks,
        has_one = event_queue,
    )]
    pub perp_market: AccountLoader<'info, PerpMarket>,

    pub bids: AccountLoader<'info, BookSide>,
    pub asks: AccountLoader<'info, BookSide>,
    pub event_queue: AccountLoader<'info, EventQueue>,
}
