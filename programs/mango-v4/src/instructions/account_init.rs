use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, CreateAccount};
use anchor_spl::token::{self, InitializeAccount3, Mint, Token};

pub fn create_pda_account<'info>(
    payer: &Signer<'info>,
    account: &AccountInfo<'info>,
    system_program: &Program<'info, System>,
    signer_seeds: &[&[u8]],
    space: usize,
    owner: &Pubkey,
) -> Result<()> {
    let lamports = Rent::get()?.minimum_balance(space);
    let signer = [signer_seeds];
    let cpi_accounts = CreateAccount {
        from: payer.to_account_info(),
        to: account.clone(),
    };
    let cpi_ctx =
        CpiContext::new(system_program.to_account_info(), cpi_accounts).with_signer(&signer);
    system_program::create_account(cpi_ctx, lamports, space as u64, owner)
}

pub fn create_token_account_pda<'info>(
    payer: &Signer<'info>,
    account: &AccountInfo<'info>,
    mint: &Account<'info, Mint>,
    authority: &AccountInfo<'info>,
    token_program: &Program<'info, Token>,
    system_program: &Program<'info, System>,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    create_pda_account(
        payer,
        account,
        system_program,
        signer_seeds,
        anchor_spl::token::TokenAccount::LEN,
        &token_program.key(),
    )?;

    let cpi_accounts = InitializeAccount3 {
        account: account.clone(),
        mint: mint.to_account_info(),
        authority: authority.clone(),
    };
    token::initialize_account3(CpiContext::new(
        token_program.to_account_info(),
        cpi_accounts,
    ))
}

pub fn write_discriminator<T: anchor_lang::Discriminator>(account: &AccountInfo) -> Result<()> {
    let mut data = account.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&T::discriminator());
    Ok(())
}
