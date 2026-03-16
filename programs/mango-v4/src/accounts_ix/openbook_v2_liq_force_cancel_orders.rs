use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use openbook_v2::state::OpenOrdersAccount;

use crate::error::*;
use crate::state::*;
use openbook_v2::{program::OpenbookV2, state::Market};

#[derive(Accounts)]
pub struct OpenbookV2LiqForceCancelOrders<'info> {
    /// CHECK: validated inline in the instruction body.
    pub group: UncheckedAccount<'info>,

    // Allow force cancel even if account is frozen
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub account: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub payer: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: Validated inline by checking against the pubkey stored in the account at #2
    pub open_orders: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub openbook_v2_market: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub openbook_v2_program: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub openbook_v2_market_external: UncheckedAccount<'info>,

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
