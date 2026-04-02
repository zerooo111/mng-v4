use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token};

const FIRST_BANK_NUM: u32 = 0;

#[derive(Accounts)]
#[instruction(token_index: TokenIndex)]
pub struct TokenRegisterTrustless<'info> {
    #[account(mut)]
    pub group: AccountLoader<'info, Group>,
    pub admin: Signer<'info>,

    pub mint: Box<Account<'info, Mint>>,

    /// CHECK: Fresh PDA account created and initialized in the instruction body.
    #[account(
        mut,
        seeds = [b"Bank".as_ref(), group.key().as_ref(), &token_index.to_le_bytes(), &FIRST_BANK_NUM.to_le_bytes()],
        bump,
    )]
    pub bank: UncheckedAccount<'info>,

    /// CHECK: Fresh PDA SPL token account created and initialized in the instruction body.
    #[account(
        mut,
        seeds = [b"Vault".as_ref(), group.key().as_ref(), &token_index.to_le_bytes(), &FIRST_BANK_NUM.to_le_bytes()],
        bump,
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: Fresh PDA account created and initialized in the instruction body.
    #[account(
        mut,
        seeds = [b"MintInfo".as_ref(), group.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub mint_info: UncheckedAccount<'info>,

    /// CHECK: The oracle can be one of several different account types
    pub oracle: UncheckedAccount<'info>,

    /// CHECK: The oracle can be one of several different account types
    pub fallback_oracle: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}
