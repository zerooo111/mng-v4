use crate::accounts_ix::*;
use crate::error::*;
use crate::instructions::account_init::{
    create_pda_account, create_token_account_pda, write_discriminator,
};
use crate::state::*;
use anchor_lang::prelude::*;

#[allow(clippy::too_many_arguments)]
#[allow(unused_variables)]
pub fn token_add_bank(
    ctx: Context<TokenAddBank>,
    token_index: TokenIndex,
    bank_num: u32,
) -> Result<()> {
    let existing_bank = ctx.accounts.existing_bank.load()?;
    let bank_bump = *ctx.bumps.get("bank").ok_or(MangoError::SomeError)?;
    let group_key = ctx.accounts.group.key();
    let bank_num_bytes = bank_num.to_le_bytes();
    let token_index_bytes = token_index.to_le_bytes();
    let bank_seeds = &[
        b"Bank".as_ref(),
        group_key.as_ref(),
        &token_index_bytes,
        &bank_num_bytes,
        &[bank_bump],
    ];
    create_pda_account(
        &ctx.accounts.payer,
        &ctx.accounts.bank.to_account_info(),
        &ctx.accounts.system_program,
        bank_seeds,
        8 + std::mem::size_of::<Bank>(),
        ctx.program_id,
    )?;

    let vault_bump = *ctx.bumps.get("vault").ok_or(MangoError::SomeError)?;
    let vault_seeds = &[
        b"Vault".as_ref(),
        group_key.as_ref(),
        &token_index_bytes,
        &bank_num_bytes,
        &[vault_bump],
    ];
    create_token_account_pda(
        &ctx.accounts.payer,
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.mint,
        &ctx.accounts.group.to_account_info(),
        &ctx.accounts.token_program,
        &ctx.accounts.system_program,
        vault_seeds,
    )?;

    let bank_loader =
        AccountLoader::<Bank>::try_from_unchecked(ctx.program_id, &ctx.accounts.bank)?;
    let mut bank = bank_loader.load_init()?;
    let bump = bank_bump;
    *bank = Bank::from_existing_bank(&existing_bank, ctx.accounts.vault.key(), bank_num, bump);

    let mut mint_info = ctx.accounts.mint_info.load_mut()?;
    let free_slot = mint_info
        .banks
        .iter()
        .position(|bank| bank == &Pubkey::default())
        .unwrap();
    require_eq!(bank_num as usize, free_slot);
    mint_info.banks[free_slot] = ctx.accounts.bank.key();
    mint_info.vaults[free_slot] = ctx.accounts.vault.key();

    drop(mint_info);
    drop(bank);

    write_discriminator::<Bank>(&ctx.accounts.bank.to_account_info())?;

    Ok(())
}
