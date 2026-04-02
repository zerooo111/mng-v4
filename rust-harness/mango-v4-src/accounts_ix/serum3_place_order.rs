use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use num_enum::IntoPrimitive;
use num_enum::TryFromPrimitive;

/// Copy paste a bunch of enums so that we could AnchorSerialize & AnchorDeserialize them

#[derive(Clone, Copy, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum Serum3SelfTradeBehavior {
    DecrementTake = 0,
    CancelProvide = 1,
    AbortTransaction = 2,
}

#[derive(Clone, Copy, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]

pub enum Serum3OrderType {
    Limit = 0,
    ImmediateOrCancel = 1,
    PostOnly = 2,
}
#[derive(Clone, Copy, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]

pub enum Serum3Side {
    Bid = 0,
    Ask = 1,
}

// Used for Serum3PlaceOrder v1 and v2
#[derive(Accounts)]
pub struct Serum3PlaceOrder<'info> {
    /// CHECK: validated inline in the instruction body.
    pub group: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub account: UncheckedAccount<'info>,
    /// CHECK: validated inline in the instruction body.
    pub owner: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: Validated inline by checking against the pubkey stored in the account at #2
    pub open_orders: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub serum_market: UncheckedAccount<'info>,
    /// CHECK: The pubkey is checked and then it's passed to the serum cpi
    pub serum_program: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: The pubkey is checked and then it's passed to the serum cpi
    pub serum_market_external: UncheckedAccount<'info>,

    /// The bank that pays for the order, if necessary
    // token_index and payer_bank.vault == payer_vault is validated inline at #3
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub payer_bank: UncheckedAccount<'info>,
    /// The bank vault that pays for the order, if necessary
    #[account(mut)]
    pub payer_vault: UncheckedAccount<'info>,
}
