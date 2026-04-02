use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, TokenAccount};

use crate::state::*;

#[derive(Accounts)]
#[instruction(token_index: TokenIndex)]
pub struct TokenRegisterBootstrap<'info> {
    pub group: AccountLoader<'info, Group>,
    pub admin: Signer<'info>,

    pub mint: Box<Account<'info, Mint>>,
    /// CHECK: Freshly created program-owned zero-copy account, initialized in instruction body.
    #[account(mut)]
    pub bank: UncheckedAccount<'info>,
    pub vault: Box<Account<'info, TokenAccount>>,
    /// CHECK: Freshly created program-owned zero-copy account, initialized in instruction body.
    #[account(mut)]
    pub mint_info: UncheckedAccount<'info>,

    /// CHECK: The oracle can be one of several different account types
    pub oracle: UncheckedAccount<'info>,

    /// CHECK: The oracle can be one of several different account types
    pub fallback_oracle: UncheckedAccount<'info>,
}
