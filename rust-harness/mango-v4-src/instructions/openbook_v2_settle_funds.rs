use anchor_lang::prelude::*;
use fixed::types::I80F48;

use crate::error::*;
use crate::serum3_cpi::{OpenOrdersAmounts, OpenOrdersSlim};
use crate::state::*;
use openbook_v2::cpi::accounts::SettleFunds;

use crate::accounts_ix::*;
use crate::instructions::openbook_v2_place_order::apply_settle_changes;
use crate::logs::{
    emit_stack, LoanOriginationFeeInstruction, OpenbookV2OpenOrdersBalanceLog, WithdrawLoanLog,
};

use crate::accounts_zerocopy::AccountInfoRef;
use anchor_spl::token::accessor;

/// Settling means moving free funds from the open orders account
/// back into the mango account wallet.
///
/// There will be free funds on open_orders when an order was triggered.
///
pub fn openbook_v2_settle_funds<'info>(
    ctx: Context<'_, '_, '_, 'info, OpenbookV2SettleFunds<'info>>,
    fees_to_dao: bool,
) -> Result<()> {
    const EXTRA_CPI_ACCOUNTS: usize = 7;
    require_gte!(ctx.remaining_accounts.len(), EXTRA_CPI_ACCOUNTS);
    let (_, cpi_remaining) = ctx
        .remaining_accounts
        .split_at(ctx.remaining_accounts.len() - EXTRA_CPI_ACCOUNTS);
    let market_base_vault_ai = cpi_remaining[0].clone();
    let market_quote_vault_ai = cpi_remaining[1].clone();
    let market_vault_signer_ai = cpi_remaining[2].clone();
    let quote_oracle_ai = cpi_remaining[3].clone();
    let base_oracle_ai = cpi_remaining[4].clone();
    let token_program_ai = cpi_remaining[5].clone();
    let system_program_ai = cpi_remaining[6].clone();

    let group_loader = AccountLoader::<Group>::try_from(&ctx.accounts.group.to_account_info())?;
    let group = group_loader.load()?;
    require!(
        group.is_ix_enabled(IxGate::OpenbookV2SettleFunds),
        MangoError::IxIsDisabled
    );
    require_keys_eq!(*token_program_ai.key, anchor_spl::token::ID);
    require_keys_eq!(*system_program_ai.key, anchor_lang::system_program::ID);
    require!(ctx.accounts.authority.is_signer, MangoError::SomeError);

    let account_loader =
        AccountLoader::<MangoAccountFixed>::try_from(&ctx.accounts.account.to_account_info())?;
    let openbook_market_loader = AccountLoader::<OpenbookV2Market>::try_from(
        &ctx.accounts.openbook_v2_market.to_account_info(),
    )?;
    let openbook_market = openbook_market_loader.load()?;
    let open_orders_loader = AccountLoader::<openbook_v2::state::OpenOrdersAccount>::try_from(
        &ctx.accounts.open_orders.to_account_info(),
    )?;
    let openbook_market_external_loader = AccountLoader::<openbook_v2::state::Market>::try_from(
        &ctx.accounts.openbook_v2_market_external.to_account_info(),
    )?;
    let quote_bank_loader =
        AccountLoader::<Bank>::try_from(&ctx.accounts.quote_bank.to_account_info())?;
    let base_bank_loader =
        AccountLoader::<Bank>::try_from(&ctx.accounts.base_bank.to_account_info())?;
    require_keys_eq!(openbook_market.group, ctx.accounts.group.key());
    require_keys_eq!(
        openbook_market.openbook_v2_market_external,
        ctx.accounts.openbook_v2_market_external.key()
    );
    require_keys_eq!(
        openbook_market.openbook_v2_program,
        ctx.accounts.openbook_v2_program.key()
    );

    //
    // Validation
    //
    {
        let account = account_loader.load_full()?;
        require_keys_eq!(account.fixed.group, ctx.accounts.group.key());
        require!(account.fixed.is_operational(), MangoError::AccountIsFrozen);
        // account constraint #1
        require!(
            account
                .fixed
                .is_owner_or_delegate(ctx.accounts.authority.key()),
            MangoError::SomeError
        );

        // Validate open_orders #2
        require!(
            account
                .openbook_v2_orders(openbook_market.market_index)?
                .open_orders
                == ctx.accounts.open_orders.key(),
            MangoError::SomeError
        );

        // Validate banks and vaults #3
        let quote_bank = quote_bank_loader.load()?;
        require_keys_eq!(quote_bank.group, ctx.accounts.group.key());
        require!(
            quote_bank.vault == ctx.accounts.quote_vault.key(),
            MangoError::SomeError
        );
        require!(
            quote_bank.token_index == openbook_market.quote_token_index,
            MangoError::SomeError
        );
        let base_bank = base_bank_loader.load()?;
        require_keys_eq!(base_bank.group, ctx.accounts.group.key());
        require!(
            base_bank.vault == ctx.accounts.base_vault.key(),
            MangoError::SomeError
        );
        require!(
            base_bank.token_index == openbook_market.base_token_index,
            MangoError::SomeError
        );

        // Validate oracles #4
        require_keys_eq!(base_bank.oracle, *base_oracle_ai.key, MangoError::SomeError);
        require_keys_eq!(
            quote_bank.oracle,
            *quote_oracle_ai.key,
            MangoError::SomeError
        );
    }

    //
    // Charge any open loan origination fees
    //
    let base_lot_size: u64;
    let quote_lot_size: u64;
    let before_oo;
    {
        let openbook_market_external = openbook_market_external_loader.load()?;
        require_keys_eq!(
            openbook_market_external.market_base_vault,
            *market_base_vault_ai.key
        );
        require_keys_eq!(
            openbook_market_external.market_quote_vault,
            *market_quote_vault_ai.key
        );
        base_lot_size = openbook_market_external.base_lot_size.try_into().unwrap();
        quote_lot_size = openbook_market_external.quote_lot_size.try_into().unwrap();

        let open_orders = open_orders_loader.load()?;
        before_oo = OpenOrdersSlim::from_oo_v2(&open_orders, base_lot_size, quote_lot_size);
        let mut account = account_loader.load_full_mut()?;
        let mut base_bank = base_bank_loader.load_mut()?;
        let mut quote_bank = quote_bank_loader.load_mut()?;
        charge_loan_origination_fees(
            &ctx.accounts.group.key(),
            &ctx.accounts.account.key(),
            openbook_market.market_index,
            &mut base_bank,
            &mut quote_bank,
            &mut account.borrow_mut(),
            &before_oo,
            Some(&base_oracle_ai),
            Some(&quote_oracle_ai),
        )?;
    }

    //
    // Settle
    //
    let before_base_vault = accessor::amount(&ctx.accounts.base_vault.to_account_info())?;
    let before_quote_vault = accessor::amount(&ctx.accounts.quote_vault.to_account_info())?;
    let mango_account_seeds_data = account_loader.load()?.pda_seeds();
    let seeds = &mango_account_seeds_data.signer_seeds();
    cpi_settle_funds(
        ctx.accounts,
        &[seeds],
        &market_base_vault_ai,
        &market_quote_vault_ai,
        &market_vault_signer_ai,
        &token_program_ai,
        &system_program_ai,
    )?;

    //
    // After-settle tracking
    //
    let after_oo = {
        let open_orders = open_orders_loader.load()?;
        OpenOrdersSlim::from_oo_v2(&open_orders, base_lot_size, quote_lot_size)
    };

    let after_base_vault = accessor::amount(&ctx.accounts.base_vault.to_account_info())?;
    let after_quote_vault = accessor::amount(&ctx.accounts.quote_vault.to_account_info())?;

    let mut account = account_loader.load_full_mut()?;
    let mut base_bank = base_bank_loader.load_mut()?;
    let mut quote_bank = quote_bank_loader.load_mut()?;
    let group = group_loader.load()?;
    let open_orders = open_orders_loader.load()?;
    apply_settle_changes(
        &group,
        ctx.accounts.account.key(),
        &mut account.borrow_mut(),
        &mut base_bank,
        &mut quote_bank,
        &openbook_market,
        before_base_vault,
        before_quote_vault,
        &before_oo,
        after_base_vault,
        after_quote_vault,
        &after_oo,
        None,
        fees_to_dao,
        Some(&quote_oracle_ai),
        &open_orders,
    )?;

    emit_stack(OpenbookV2OpenOrdersBalanceLog {
        mango_group: ctx.accounts.group.key(),
        mango_account: ctx.accounts.account.key(),
        market_index: openbook_market.market_index,
        base_token_index: openbook_market.base_token_index,
        quote_token_index: openbook_market.quote_token_index,
        base_total: after_oo.native_base_total(),
        base_free: after_oo.native_base_free(),
        quote_total: after_oo.native_quote_total(),
        quote_free: after_oo.native_quote_free(),
        referrer_rebates_accrued: after_oo.native_rebates(),
    });

    Ok(())
}

// Charge fees if the potential borrows are bigger than the funds on the open orders account
pub fn charge_loan_origination_fees(
    group_pubkey: &Pubkey,
    account_pubkey: &Pubkey,
    market_index: OpenbookV2MarketIndex,
    base_bank: &mut Bank,
    quote_bank: &mut Bank,
    account: &mut MangoAccountRefMut,
    before_oo: &OpenOrdersSlim,
    base_oracle: Option<&AccountInfo>,
    quote_oracle: Option<&AccountInfo>,
) -> Result<()> {
    let openbook_v2_orders = account.openbook_v2_orders_mut(market_index).unwrap();

    let now_ts = Clock::get()?.unix_timestamp.try_into().unwrap();

    let oo_base_total = before_oo.native_base_total();
    let actualized_base_loan = I80F48::from_num(
        openbook_v2_orders
            .base_borrows_without_fee
            .saturating_sub(oo_base_total),
    );
    if actualized_base_loan > 0 {
        openbook_v2_orders.base_borrows_without_fee = oo_base_total;

        // now that the loan is actually materialized, charge the loan origination fee
        // note: the withdraw has already happened while placing the order
        let base_token_account = account.token_position_mut(base_bank.token_index)?.0;
        let withdraw_result = base_bank.withdraw_loan_origination_fee(
            base_token_account,
            actualized_base_loan,
            now_ts,
        )?;

        let base_oracle_price = base_oracle
            .map(|ai| {
                let ai_ref = &AccountInfoRef::borrow(ai)?;
                base_bank.oracle_price(
                    &OracleAccountInfos::from_reader(ai_ref),
                    Some(Clock::get()?.slot),
                )
            })
            .transpose()?;

        emit_stack(WithdrawLoanLog {
            mango_group: *group_pubkey,
            mango_account: *account_pubkey,
            token_index: base_bank.token_index,
            loan_amount: withdraw_result.loan_amount.to_bits(),
            loan_origination_fee: withdraw_result.loan_origination_fee.to_bits(),
            instruction: LoanOriginationFeeInstruction::OpenbookV2SettleFunds,
            price: base_oracle_price.map(|p| p.to_bits()),
        });
    }

    let openbook_v2_account = account.openbook_v2_orders_mut(market_index).unwrap();
    let oo_quote_total = before_oo.native_quote_total();
    let actualized_quote_loan = I80F48::from_num::<u64>(
        openbook_v2_account
            .quote_borrows_without_fee
            .saturating_sub(oo_quote_total),
    );
    if actualized_quote_loan > 0 {
        openbook_v2_account.quote_borrows_without_fee = oo_quote_total;

        // now that the loan is actually materialized, charge the loan origination fee
        // note: the withdraw has already happened while placing the order
        let quote_token_account = account.token_position_mut(quote_bank.token_index)?.0;
        let withdraw_result = quote_bank.withdraw_loan_origination_fee(
            quote_token_account,
            actualized_quote_loan,
            now_ts,
        )?;

        let quote_oracle_price = quote_oracle
            .map(|ai| {
                let ai_ref = &AccountInfoRef::borrow(ai)?;
                quote_bank.oracle_price(
                    &OracleAccountInfos::from_reader(ai_ref),
                    Some(Clock::get()?.slot),
                )
            })
            .transpose()?;

        emit_stack(WithdrawLoanLog {
            mango_group: *group_pubkey,
            mango_account: *account_pubkey,
            token_index: quote_bank.token_index,
            loan_amount: withdraw_result.loan_amount.to_bits(),
            loan_origination_fee: withdraw_result.loan_origination_fee.to_bits(),
            instruction: LoanOriginationFeeInstruction::OpenbookV2SettleFunds,
            price: quote_oracle_price.map(|p| p.to_bits()),
        });
    }

    Ok(())
}

fn cpi_settle_funds<'info>(
    ctx: &OpenbookV2SettleFunds<'info>,
    seeds: &[&[&[u8]]],
    market_base_vault_ai: &AccountInfo<'info>,
    market_quote_vault_ai: &AccountInfo<'info>,
    market_vault_signer_ai: &AccountInfo<'info>,
    token_program_ai: &AccountInfo<'info>,
    system_program_ai: &AccountInfo<'info>,
) -> Result<()> {
    let cpi_accounts = SettleFunds {
        penalty_payer: ctx.authority.to_account_info(),
        market: ctx.openbook_v2_market_external.to_account_info(),
        market_authority: market_vault_signer_ai.clone(),
        market_base_vault: market_base_vault_ai.clone(),
        market_quote_vault: market_quote_vault_ai.clone(),
        user_base_account: ctx.base_vault.to_account_info(),
        user_quote_account: ctx.quote_vault.to_account_info(),
        referrer_account: Some(ctx.quote_vault.to_account_info()),
        token_program: token_program_ai.clone(),
        owner: ctx.account.to_account_info(),
        open_orders_account: ctx.open_orders.to_account_info(),
        system_program: system_program_ai.clone(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.openbook_v2_program.to_account_info(),
        cpi_accounts,
        seeds,
    );

    openbook_v2::cpi::settle_funds(cpi_ctx)
}
