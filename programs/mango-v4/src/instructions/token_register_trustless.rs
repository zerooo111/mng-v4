use anchor_lang::prelude::*;
use fixed::types::I80F48;

use crate::accounts_zerocopy::AccountInfoRef;
use crate::error::*;
use crate::instructions::account_init::{
    create_pda_account, create_token_account_pda, write_discriminator,
};
use crate::instructions::INDEX_START;
use crate::state::*;
use crate::util::fill_from_str;

use crate::logs::{emit_stack, TokenMetaDataLogV2};

use crate::accounts_ix::*;

#[allow(clippy::too_many_arguments)]
pub fn token_register_trustless(
    ctx: Context<TokenRegisterTrustless>,
    token_index: TokenIndex,
    name: String,
) -> Result<()> {
    require_neq!(token_index, QUOTE_TOKEN_INDEX);
    require_neq!(token_index, TokenIndex::MAX);
    {
        let group = ctx.accounts.group.load()?;
        require!(
            group.admin == ctx.accounts.admin.key()
                || group.fast_listing_admin == ctx.accounts.admin.key(),
            MangoError::SomeError
        );
        require!(
            group.is_ix_enabled(IxGate::TokenRegisterTrustless),
            MangoError::IxIsDisabled
        );
    }

    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
    {
        let mut group = ctx.accounts.group.load_mut()?;
        let week = 7 * 24 * 60 * 60;
        if now_ts >= group.fast_listing_interval_start + week {
            group.fast_listing_interval_start = now_ts / week * week;
            group.fast_listings_in_interval = 0;
        }
        group.fast_listings_in_interval += 1;
        require_gte!(
            group.allowed_fast_listings_per_interval,
            group.fast_listings_in_interval
        );
    }

    let net_borrow_limit_window_size_ts = 24 * 60 * 60u64;

    let group_key = ctx.accounts.group.key();
    let mint_key = ctx.accounts.mint.key();
    let token_index_bytes = token_index.to_le_bytes();
    let first_bank_num_bytes = 0u32.to_le_bytes();

    let bank_bump = *ctx.bumps.get("bank").ok_or(MangoError::SomeError)?;
    let bank_seeds = &[
        b"Bank".as_ref(),
        group_key.as_ref(),
        &token_index_bytes,
        &first_bank_num_bytes,
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
        &first_bank_num_bytes,
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

    let mint_info_bump = *ctx.bumps.get("mint_info").ok_or(MangoError::SomeError)?;
    let mint_info_seeds = &[
        b"MintInfo".as_ref(),
        group_key.as_ref(),
        mint_key.as_ref(),
        &[mint_info_bump],
    ];
    create_pda_account(
        &ctx.accounts.payer,
        &ctx.accounts.mint_info.to_account_info(),
        &ctx.accounts.system_program,
        mint_info_seeds,
        8 + std::mem::size_of::<MintInfo>(),
        ctx.program_id,
    )?;

    let bank_loader =
        AccountLoader::<Bank>::try_from_unchecked(ctx.program_id, &ctx.accounts.bank)?;
    let mut bank = bank_loader.load_init()?;
    bank.group = ctx.accounts.group.key();
    bank.name = fill_from_str(&name)?;
    bank.mint = ctx.accounts.mint.key();
    bank.vault = ctx.accounts.vault.key();
    bank.oracle = ctx.accounts.oracle.key();
    bank.oracle_config = OracleConfig {
        conf_filter: I80F48::from_num(1000.0), // effectively disabled
        max_staleness_slots: -1,
        reserved: [0; 72],
    };
    bank.deposit_index = INDEX_START;
    bank.borrow_index = INDEX_START;
    bank.index_last_updated = now_ts;
    bank.bank_rate_last_updated = now_ts;
    bank.adjustment_factor = I80F48::from_num(0.004);
    bank.util0 = I80F48::from_num(0.5);
    bank.rate0 = I80F48::from_num(0.018);
    bank.util1 = I80F48::from_num(0.75);
    bank.rate1 = I80F48::from_num(0.05);
    bank.max_rate = I80F48::from_num(0.5);
    bank.loan_origination_fee_rate = I80F48::from_num(0.0020);
    bank.loan_fee_rate = I80F48::from_num(0.005);
    bank.maint_liab_weight = I80F48::from_num(1.4);
    bank.init_liab_weight = I80F48::from_num(1.8);
    bank.liquidation_fee = I80F48::from_num(0.05);
    bank.platform_liquidation_fee = I80F48::from_num(0.05);
    bank.flash_loan_token_account_initial = u64::MAX;
    bank.token_index = token_index;
    bank.bump = bank_bump;
    bank.mint_decimals = ctx.accounts.mint.decimals;
    bank.min_vault_to_deposits_ratio = 0.2;
    bank.net_borrow_limit_window_size_ts = net_borrow_limit_window_size_ts;
    bank.last_net_borrows_window_start_ts =
        now_ts / net_borrow_limit_window_size_ts * net_borrow_limit_window_size_ts;
    bank.net_borrow_limit_per_window_quote = 5_000_000_000;
    bank.borrow_weight_scale_start_quote = 5_000_000_000.0;
    bank.deposit_weight_scale_start_quote = 5_000_000_000.0;
    bank.reduce_only = 2;
    bank.disable_asset_liquidation = 1;
    bank.interest_target_utilization = 0.5;
    bank.interest_curve_scaling = 4.0;
    bank.fallback_oracle = ctx.accounts.fallback_oracle.key();
    let oracle_ref = &AccountInfoRef::borrow(ctx.accounts.oracle.as_ref())?;
    if let Ok(oracle_price) = bank.oracle_price(&OracleAccountInfos::from_reader(oracle_ref), None)
    {
        let mut spm = { bank.stable_price_model };
        spm.reset_to_price(oracle_price.to_num(), now_ts);
        bank.stable_price_model = spm;
    } else {
        bank.stable_price_model.reset_on_nonzero_price = 1;
    }

    bank.verify()?;
    check_is_valid_fallback_oracle(&AccountInfoRef::borrow(
        ctx.accounts.fallback_oracle.as_ref(),
    )?)?;

    let mint_info_loader =
        AccountLoader::<MintInfo>::try_from_unchecked(ctx.program_id, &ctx.accounts.mint_info)?;
    let mut mint_info = mint_info_loader.load_init()?;
    mint_info.group = ctx.accounts.group.key();
    mint_info.token_index = token_index;
    mint_info.mint = ctx.accounts.mint.key();
    mint_info.oracle = ctx.accounts.oracle.key();
    mint_info.fallback_oracle = ctx.accounts.fallback_oracle.key();
    mint_info.registration_time = now_ts;
    mint_info.banks[0] = ctx.accounts.bank.key();
    mint_info.vaults[0] = ctx.accounts.vault.key();

    emit_stack(TokenMetaDataLogV2 {
        mango_group: ctx.accounts.group.key(),
        mint: ctx.accounts.mint.key(),
        token_index,
        mint_decimals: ctx.accounts.mint.decimals,
        oracle: ctx.accounts.oracle.key(),
        fallback_oracle: ctx.accounts.fallback_oracle.key(),
        mint_info: ctx.accounts.mint_info.key(),
    });

    drop(mint_info);
    drop(bank);

    write_discriminator::<Bank>(&ctx.accounts.bank.to_account_info())?;
    write_discriminator::<MintInfo>(&ctx.accounts.mint_info.to_account_info())?;

    Ok(())
}
