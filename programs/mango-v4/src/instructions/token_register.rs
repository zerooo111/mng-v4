use anchor_lang::prelude::*;
use fixed::types::I80F48;

use crate::accounts_zerocopy::AccountInfoRef;
use crate::error::*;
use crate::instructions::account_init::{
    create_pda_account, create_token_account_pda, write_discriminator,
};
use crate::state::*;
use crate::util::fill_from_str;

use crate::logs::{emit_stack, TokenMetaDataLogV2};

pub const INDEX_START: I80F48 = I80F48::from_bits(1_000_000 * I80F48::ONE.to_bits());

use crate::accounts_ix::*;

#[allow(clippy::too_many_arguments)]
pub fn token_register(
    ctx: Context<TokenRegister>,
    token_index: TokenIndex,
    name: String,
    oracle_config: OracleConfigParams,
    interest_rate_params: InterestRateParams,
    loan_fee_rate: f32,
    loan_origination_fee_rate: f32,
    maint_asset_weight: f32,
    init_asset_weight: f32,
    maint_liab_weight: f32,
    init_liab_weight: f32,
    liquidation_fee: f32,
    stable_price_delay_interval_seconds: u32,
    stable_price_delay_growth_limit: f32,
    stable_price_growth_limit: f32,
    min_vault_to_deposits_ratio: f64,
    net_borrow_limit_window_size_ts: u64,
    net_borrow_limit_per_window_quote: i64,
    borrow_weight_scale_start_quote: f64,
    deposit_weight_scale_start_quote: f64,
    reduce_only: u8,
    token_conditional_swap_taker_fee_rate: f32,
    token_conditional_swap_maker_fee_rate: f32,
    flash_loan_swap_fee_rate: f32,
    interest_curve_scaling: f32,
    interest_target_utilization: f32,
    group_insurance_fund: bool,
    deposit_limit: u64,
    zero_util_rate: f32,
    platform_liquidation_fee: f32,
    disable_asset_liquidation: bool,
    collateral_fee_per_day: f32,
) -> Result<()> {
    require_neq!(token_index, TokenIndex::MAX);
    {
        let group = ctx.accounts.group.load()?;
        require_keys_eq!(group.admin, ctx.accounts.admin.key());
        require!(
            group.is_ix_enabled(IxGate::TokenRegister),
            MangoError::IxIsDisabled
        );
    }

    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();

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
    bank.deposit_index = INDEX_START;
    bank.borrow_index = INDEX_START;
    bank.index_last_updated = now_ts;
    bank.bank_rate_last_updated = now_ts;
    bank.adjustment_factor = I80F48::from_num(interest_rate_params.adjustment_factor);
    bank.util0 = I80F48::from_num(interest_rate_params.util0);
    bank.rate0 = I80F48::from_num(interest_rate_params.rate0);
    bank.util1 = I80F48::from_num(interest_rate_params.util1);
    bank.rate1 = I80F48::from_num(interest_rate_params.rate1);
    bank.max_rate = I80F48::from_num(interest_rate_params.max_rate);
    bank.loan_origination_fee_rate = I80F48::from_num(loan_origination_fee_rate);
    bank.loan_fee_rate = I80F48::from_num(loan_fee_rate);
    bank.maint_asset_weight = I80F48::from_num(maint_asset_weight);
    bank.init_asset_weight = I80F48::from_num(init_asset_weight);
    bank.maint_liab_weight = I80F48::from_num(maint_liab_weight);
    bank.init_liab_weight = I80F48::from_num(init_liab_weight);
    bank.liquidation_fee = I80F48::from_num(liquidation_fee);
    bank.flash_loan_token_account_initial = u64::MAX;
    bank.token_index = token_index;
    bank.bump = bank_bump;
    bank.mint_decimals = ctx.accounts.mint.decimals;
    bank.oracle_config = oracle_config.to_oracle_config();
    bank.stable_price_model = StablePriceModel {
        delay_interval_seconds: stable_price_delay_interval_seconds,
        delay_growth_limit: stable_price_delay_growth_limit,
        stable_growth_limit: stable_price_growth_limit,
        ..StablePriceModel::default()
    };
    bank.min_vault_to_deposits_ratio = min_vault_to_deposits_ratio;
    bank.net_borrow_limit_window_size_ts = net_borrow_limit_window_size_ts;
    bank.last_net_borrows_window_start_ts =
        now_ts / net_borrow_limit_window_size_ts * net_borrow_limit_window_size_ts;
    bank.net_borrow_limit_per_window_quote = net_borrow_limit_per_window_quote;
    bank.borrow_weight_scale_start_quote = borrow_weight_scale_start_quote;
    bank.deposit_weight_scale_start_quote = deposit_weight_scale_start_quote;
    bank.reduce_only = reduce_only;
    bank.disable_asset_liquidation = u8::from(disable_asset_liquidation);
    bank.token_conditional_swap_taker_fee_rate = token_conditional_swap_taker_fee_rate;
    bank.token_conditional_swap_maker_fee_rate = token_conditional_swap_maker_fee_rate;
    bank.flash_loan_swap_fee_rate = flash_loan_swap_fee_rate;
    bank.interest_target_utilization = interest_target_utilization;
    bank.interest_curve_scaling = interest_curve_scaling.into();
    bank.fallback_oracle = ctx.accounts.fallback_oracle.key();
    bank.deposit_limit = deposit_limit;
    bank.zero_util_rate = I80F48::from_num(zero_util_rate);
    bank.platform_liquidation_fee = I80F48::from_num(platform_liquidation_fee);
    bank.collateral_fee_per_day = collateral_fee_per_day;

    let oracle_ref = &AccountInfoRef::borrow(ctx.accounts.oracle.as_ref())?;
    if let Ok(oracle_price) = bank.oracle_price(&OracleAccountInfos::from_reader(oracle_ref), None)
    {
        bank.stable_price_model
            .reset_to_price(oracle_price.to_num(), now_ts);
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
    mint_info.group_insurance_fund = if group_insurance_fund { 1 } else { 0 };
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
