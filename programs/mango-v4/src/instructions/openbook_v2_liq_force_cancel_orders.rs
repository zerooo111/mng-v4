use anchor_lang::prelude::*;
use anchor_spl::token::accessor;
use openbook_v2::cpi::accounts::{CancelOrder, SettleFunds};

use crate::accounts_ix::*;
use crate::error::*;
use crate::health::*;
use crate::instructions::openbook_v2_place_order::apply_settle_changes;
use crate::instructions::openbook_v2_settle_funds::charge_loan_origination_fees;
use crate::logs::{emit_stack, OpenbookV2OpenOrdersBalanceLog};
use crate::serum3_cpi::OpenOrdersAmounts;
use crate::serum3_cpi::OpenOrdersSlim;
use crate::state::*;
use crate::util::clock_now;

pub fn openbook_v2_liq_force_cancel_orders<'info>(
    ctx: Context<'_, '_, '_, 'info, OpenbookV2LiqForceCancelOrders<'info>>,
    limit: u8,
) -> Result<()> {
    const EXTRA_CPI_ACCOUNTS: usize = 8;
    require_gte!(ctx.remaining_accounts.len(), EXTRA_CPI_ACCOUNTS);
    let (health_remaining, cpi_remaining) = ctx
        .remaining_accounts
        .split_at(ctx.remaining_accounts.len() - EXTRA_CPI_ACCOUNTS);
    let bids_ai = cpi_remaining[0].clone();
    let asks_ai = cpi_remaining[1].clone();
    let event_heap_ai = cpi_remaining[2].clone();
    let market_base_vault_ai = cpi_remaining[3].clone();
    let market_quote_vault_ai = cpi_remaining[4].clone();
    let market_vault_signer_ai = cpi_remaining[5].clone();
    let token_program_ai = cpi_remaining[6].clone();
    let system_program_ai = cpi_remaining[7].clone();

    let group_loader = AccountLoader::<Group>::try_from(&ctx.accounts.group.to_account_info())?;
    let group = group_loader.load()?;
    require!(
        group.is_ix_enabled(IxGate::OpenbookV2LiqForceCancelOrders),
        MangoError::IxIsDisabled
    );
    require!(ctx.accounts.payer.is_signer, MangoError::SomeError);
    require_keys_eq!(*token_program_ai.key, anchor_spl::token::ID);
    require_keys_eq!(*system_program_ai.key, anchor_lang::system_program::ID);

    let account_loader =
        AccountLoader::<MangoAccountFixed>::try_from(&ctx.accounts.account.to_account_info())?;
    //
    // Validation
    //
    let openbook_market_loader = AccountLoader::<OpenbookV2Market>::try_from(
        &ctx.accounts.openbook_v2_market.to_account_info(),
    )?;
    let openbook_market = openbook_market_loader.load()?;
    let open_orders_loader = AccountLoader::<openbook_v2::state::OpenOrdersAccount>::try_from(
        &ctx.accounts.open_orders.to_account_info(),
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
    {
        let account = account_loader.load_full()?;
        require_keys_eq!(account.fixed.group, ctx.accounts.group.key());

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
    }

    let (now_ts, now_slot) = clock_now();

    //
    // Early return if if liquidation is not allowed or if market is not in force close
    //
    let mut health_cache = {
        let mut account = account_loader.load_full_mut()?;
        let retriever =
            new_fixed_order_account_retriever(health_remaining, &account.borrow(), now_slot)?;
        let health_cache = new_health_cache(&account.borrow(), &retriever, now_ts)
            .context("create health cache")?;

        let liquidatable = account.check_liquidatable(&health_cache)?;
        let can_force_cancel = !account.fixed.is_operational()
            || liquidatable == CheckLiquidatable::Liquidatable
            || openbook_market.is_force_close();
        if !can_force_cancel {
            return Ok(());
        }

        health_cache
    };

    //
    // Charge any open loan origination fees
    //
    let openbook_market_external_loader = AccountLoader::<openbook_v2::state::Market>::try_from(
        &ctx.accounts.openbook_v2_market_external.to_account_info(),
    )?;
    let openbook_market_external = openbook_market_external_loader.load()?;
    require_keys_eq!(openbook_market_external.bids, *bids_ai.key);
    require_keys_eq!(openbook_market_external.asks, *asks_ai.key);
    require_keys_eq!(openbook_market_external.event_heap, *event_heap_ai.key);
    require_keys_eq!(
        openbook_market_external.market_base_vault,
        *market_base_vault_ai.key
    );
    require_keys_eq!(
        openbook_market_external.market_quote_vault,
        *market_quote_vault_ai.key
    );
    let base_lot_size: u64 = openbook_market_external.base_lot_size.try_into().unwrap();
    let quote_lot_size: u64 = openbook_market_external.quote_lot_size.try_into().unwrap();
    let before_oo = {
        let open_orders = open_orders_loader.load()?;
        let before_oo = OpenOrdersSlim::from_oo_v2(&open_orders, base_lot_size, quote_lot_size);
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
            None,
            None,
        )?;

        before_oo
    };

    //
    // Before-settle tracking
    //
    let before_base_vault = accessor::amount(&ctx.accounts.base_vault.to_account_info())?;
    let before_quote_vault = accessor::amount(&ctx.accounts.quote_vault.to_account_info())?;

    //
    // Cancel all and settle
    //
    let mango_account_seeds_data = account_loader.load()?.pda_seeds();
    let seeds = &mango_account_seeds_data.signer_seeds();
    cpi_cancel_all_orders(ctx.accounts, &[seeds], limit, &bids_ai, &asks_ai)?;
    // this requires a mut ctx.accounts.account for no reason
    drop(openbook_market_external);
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
    let after_oo;
    {
        let open_orders = open_orders_loader.load()?;
        after_oo = OpenOrdersSlim::from_oo_v2(&open_orders, base_lot_size, quote_lot_size);

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
        Some(&mut health_cache),
        true,
        None,
        &open_orders,
    )?;

    //
    // Health check at the end
    //
    let liq_end_health = health_cache.health(HealthType::LiquidationEnd);
    account
        .fixed
        .maybe_recover_from_being_liquidated(liq_end_health);

    Ok(())
}

fn cpi_cancel_all_orders<'info>(
    ctx: &OpenbookV2LiqForceCancelOrders<'info>,
    seeds: &[&[&[u8]]],
    limit: u8,
    bids_ai: &AccountInfo<'info>,
    asks_ai: &AccountInfo<'info>,
) -> Result<()> {
    let cpi_accounts = CancelOrder {
        market: ctx.openbook_v2_market_external.to_account_info(),
        open_orders_account: ctx.open_orders.to_account_info(),
        signer: ctx.account.to_account_info(),
        bids: bids_ai.clone(),
        asks: asks_ai.clone(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.openbook_v2_program.to_account_info(),
        cpi_accounts,
        seeds,
    );

    // todo-pan: maybe allow passing side for cu opt
    openbook_v2::cpi::cancel_all_orders(cpi_ctx, None, limit)
}

fn cpi_settle_funds<'info>(
    ctx: &OpenbookV2LiqForceCancelOrders<'info>,
    seeds: &[&[&[u8]]],
    market_base_vault_ai: &AccountInfo<'info>,
    market_quote_vault_ai: &AccountInfo<'info>,
    market_vault_signer_ai: &AccountInfo<'info>,
    token_program_ai: &AccountInfo<'info>,
    system_program_ai: &AccountInfo<'info>,
) -> Result<()> {
    let cpi_accounts = SettleFunds {
        penalty_payer: ctx.payer.to_account_info(),
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
