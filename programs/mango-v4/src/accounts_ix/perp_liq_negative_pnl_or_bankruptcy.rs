use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};

#[derive(Accounts)]
pub struct PerpLiqNegativePnlOrBankruptcy<'info> {
    pub group: AccountLoader<'info, Group>,

    #[account(mut)]
    pub liqor: AccountLoader<'info, MangoAccountFixed>,
    pub liqor_owner: Signer<'info>,

    // This account MUST have a loss
    #[account(mut)]
    pub liqee: AccountLoader<'info, MangoAccountFixed>,

    #[account(mut)]
    pub perp_market: AccountLoader<'info, PerpMarket>,

    /// CHECK: Oracle can have different account types, constrained by address in perp_market
    pub oracle: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub settle_bank: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub settle_vault: UncheckedAccount<'info>,

    /// CHECK: Oracle can have different account types
    pub settle_oracle: UncheckedAccount<'info>,

    // future: this would be an insurance fund vault specific to a
    // trustless token, separate from the shared one on the group
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub insurance_vault: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub token_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct PerpLiqNegativePnlOrBankruptcyV2<'info> {
    pub group: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub liqor: UncheckedAccount<'info>,
    pub liqor_owner: UncheckedAccount<'info>,

    // This account MUST have a loss
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub liqee: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub perp_market: UncheckedAccount<'info>,

    /// CHECK: Oracle can have different account types, constrained by address in perp_market
    pub oracle: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub settle_bank: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub settle_vault: UncheckedAccount<'info>,

    // future: this would be an insurance fund vault specific to a
    // trustless token, separate from the shared one on the group
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub insurance_vault: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub insurance_bank_vault: UncheckedAccount<'info>,
}
