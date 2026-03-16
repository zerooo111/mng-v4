use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use openbook_v2::state::OpenOrdersAccount;

use crate::error::*;
use crate::state::*;
use openbook_v2::{program::OpenbookV2, state::Market};

#[derive(Accounts)]
pub struct OpenbookV2SettleFunds<'info> {
    /// CHECK: validated inline in the instruction body.
    pub group: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub account: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub open_orders: UncheckedAccount<'info>,

    pub openbook_v2_market: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub openbook_v2_program: UncheckedAccount<'info>,

    #[account(mut)]
    pub openbook_v2_market_external: UncheckedAccount<'info>,

    // token_index and bank.vault == vault is validated inline at #3
    #[account(mut)]
    pub quote_bank: UncheckedAccount<'info>,

    #[account(mut)]
    pub quote_vault: UncheckedAccount<'info>,

    #[account(mut)]
    pub base_bank: UncheckedAccount<'info>,

    #[account(mut)]
    pub base_vault: UncheckedAccount<'info>,
}
