use anchor_lang::prelude::*;
use anchor_lang::Discriminator;
use fixed::types::I80F48;

use crate::accounts_ix::*;
use crate::accounts_zerocopy::AccountInfoRef;
use crate::error::*;
use crate::instructions::INDEX_START;
use crate::logs::{emit_stack, TokenMetaDataLogV2};
use crate::state::*;
use crate::util::fill_from_str;

pub fn token_register_bootstrap(
    ctx: Context<TokenRegisterBootstrap>,
    token_index: TokenIndex,
    name: String,
    group_insurance_fund: bool,
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

    require_keys_eq!(ctx.accounts.vault.owner, ctx.accounts.group.key());
    require_keys_eq!(ctx.accounts.vault.mint, ctx.accounts.mint.key());

    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();

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
    bank.adjustment_factor = I80F48::from_num(0.004);
    bank.util0 = I80F48::from_num(0.5);
    bank.rate0 = I80F48::from_num(0.018);
    bank.util1 = I80F48::from_num(0.75);
    bank.rate1 = I80F48::from_num(0.05);
    bank.max_rate = I80F48::from_num(0.5);
    bank.loan_origination_fee_rate = I80F48::from_num(0.0020);
    bank.loan_fee_rate = I80F48::from_num(0.005);
    bank.maint_asset_weight = I80F48::ONE;
    bank.init_asset_weight = I80F48::ONE;
    bank.maint_liab_weight = I80F48::from_num(1.4);
    bank.init_liab_weight = I80F48::from_num(1.8);
    bank.liquidation_fee = I80F48::from_num(0.05);
    bank.platform_liquidation_fee = I80F48::from_num(0.05);
    bank.flash_loan_token_account_initial = u64::MAX;
    bank.token_index = token_index;
    bank.mint_decimals = ctx.accounts.mint.decimals;
    bank.oracle_config = OracleConfig {
        conf_filter: I80F48::from_num(0.1),
        max_staleness_slots: -1,
        reserved: [0; 72],
    };
    bank.stable_price_model = StablePriceModel::default();
    bank.bump = 0;
    bank.min_vault_to_deposits_ratio = 0.2;
    bank.net_borrow_limit_window_size_ts = 86_400;
    bank.last_net_borrows_window_start_ts = now_ts / 86_400 * 86_400;
    bank.net_borrow_limit_per_window_quote = 5_000_000_000;
    bank.borrow_weight_scale_start_quote = 5_000_000_000.0;
    bank.deposit_weight_scale_start_quote = 5_000_000_000.0;
    bank.reduce_only = 2;
    bank.disable_asset_liquidation = 0;
    bank.interest_target_utilization = 0.5;
    bank.interest_curve_scaling = 4.0;
    bank.fallback_oracle = ctx.accounts.fallback_oracle.key();

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

    {
        let mut bank_data = ctx.accounts.bank.try_borrow_mut_data()?;
        bank_data[0..8].copy_from_slice(&Bank::discriminator());
    }
    {
        let mut mint_info_data = ctx.accounts.mint_info.try_borrow_mut_data()?;
        mint_info_data[0..8].copy_from_slice(&MintInfo::discriminator());
    }

    Ok(())
}
