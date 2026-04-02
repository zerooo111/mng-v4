use anchor_lang::prelude::*;
use anchor_spl::token::Token;

use crate::error::*;
use crate::state::*;

#[derive(Accounts)]
pub struct Serum3LiqForceCancelOrders<'info> {
    /// CHECK: validated inline in the instruction body.
    pub group: UncheckedAccount<'info>,

    // Allow force cancel even if account is frozen
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub account: UncheckedAccount<'info>,

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

    // token_index and bank.vault == vault is validated inline at #3
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub quote_bank: UncheckedAccount<'info>,
    #[account(mut)]
    pub quote_vault: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub base_bank: UncheckedAccount<'info>,
    #[account(mut)]
    pub base_vault: UncheckedAccount<'info>,
}
