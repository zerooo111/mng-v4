use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token};

use crate::error::*;
use crate::state::*;

#[derive(Accounts)]
#[instruction(token_index: TokenIndex, bank_num: u32)]
pub struct TokenAddBank<'info> {
    #[account(
        has_one = admin,
        constraint = group.load()?.is_ix_enabled(IxGate::TokenAddBank) @ MangoError::IxIsDisabled,
        constraint = group.load()?.multiple_banks_supported(),
        // Concerns are:
        // - general reaudit
        // - client support
        // - potential_serum_tokens
        constraint = group.load()?.is_testing(),
    )]
    pub group: AccountLoader<'info, Group>,
    pub admin: Signer<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        constraint = existing_bank.load()?.token_index == token_index,
        has_one = group,
        has_one = mint,
    )]
    pub existing_bank: AccountLoader<'info, Bank>,

    /// CHECK: Fresh PDA account created and initialized in the instruction body.
    #[account(
        mut,
        seeds = [b"Bank".as_ref(), group.key().as_ref(), &token_index.to_le_bytes(), &bank_num.to_le_bytes()],
        bump,
    )]
    pub bank: UncheckedAccount<'info>,

    /// CHECK: Fresh PDA SPL token account created and initialized in the instruction body.
    #[account(
        mut,
        seeds = [b"Vault".as_ref(), group.key().as_ref(), &token_index.to_le_bytes(), &bank_num.to_le_bytes()],
        bump,
    )]
    pub vault: UncheckedAccount<'info>,

    #[account(
        mut,
        constraint = mint_info.load()?.token_index == token_index,
        has_one = group,
        has_one = mint,
    )]
    pub mint_info: AccountLoader<'info, MintInfo>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}
