use anchor_lang::prelude::*;
use anchor_spl::token::accessor;

use crate::accounts_ix::*;
use crate::error::*;
use crate::health::*;
use crate::instructions::serum3_place_order::apply_settle_changes;
use crate::instructions::serum3_settle_funds::charge_loan_origination_fees;
use crate::logs::{emit_stack, Serum3OpenOrdersBalanceLogV2};
use crate::serum3_cpi::{load_open_orders_ref, OpenOrdersAmounts, OpenOrdersSlim};
use crate::state::*;
use crate::util::clock_now;

pub fn serum3_liq_force_cancel_orders<'info>(
    ctx: Context<'_, '_, '_, 'info, Serum3LiqForceCancelOrders<'info>>,
    limit: u8,
) -> Result<()> {
    const EXTRA_CPI_ACCOUNTS: usize = 7;
    require_gte!(ctx.remaining_accounts.len(), EXTRA_CPI_ACCOUNTS);
    let (health_remaining, cpi_remaining) = ctx
        .remaining_accounts
        .split_at(ctx.remaining_accounts.len() - EXTRA_CPI_ACCOUNTS);
    let market_bids_ai = cpi_remaining[0].clone();
    let market_asks_ai = cpi_remaining[1].clone();
    let market_event_queue_ai = cpi_remaining[2].clone();
    let market_base_vault_ai = cpi_remaining[3].clone();
    let market_quote_vault_ai = cpi_remaining[4].clone();
    let market_vault_signer_ai = cpi_remaining[5].clone();
    let token_program_ai = cpi_remaining[6].clone();

    let group_loader = AccountLoader::<Group>::try_from(&ctx.accounts.group.to_account_info())?;
    let group = group_loader.load()?;
    require!(
        group.is_ix_enabled(IxGate::Serum3LiqForceCancelOrders),
        MangoError::IxIsDisabled
    );
    require_keys_eq!(*token_program_ai.key, anchor_spl::token::ID);

    let account_loader =
        AccountLoader::<MangoAccountFixed>::try_from(&ctx.accounts.account.to_account_info())?;
    //
    // Validation
    //
    let serum_market_loader =
        AccountLoader::<Serum3Market>::try_from(&ctx.accounts.serum_market.to_account_info())?;
    let quote_bank_loader =
        AccountLoader::<Bank>::try_from(&ctx.accounts.quote_bank.to_account_info())?;
    let base_bank_loader =
        AccountLoader::<Bank>::try_from(&ctx.accounts.base_bank.to_account_info())?;
    let serum_market = serum_market_loader.load()?;
    require_keys_eq!(serum_market.group, ctx.accounts.group.key());
    require_keys_eq!(serum_market.serum_program, ctx.accounts.serum_program.key());
    require_keys_eq!(
        serum_market.serum_market_external,
        ctx.accounts.serum_market_external.key()
    );
    {
        let account = account_loader.load_full()?;
        require_keys_eq!(account.fixed.group, ctx.accounts.group.key());

        // Validate open_orders #2
        require!(
            account
                .serum3_orders(serum_market.market_index)?
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
            quote_bank.token_index == serum_market.quote_token_index,
            MangoError::SomeError
        );
        let base_bank = base_bank_loader.load()?;
        require_keys_eq!(base_bank.group, ctx.accounts.group.key());
        require!(
            base_bank.vault == ctx.accounts.base_vault.key(),
            MangoError::SomeError
        );
        require!(
            base_bank.token_index == serum_market.base_token_index,
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
            || serum_market.is_force_close();
        if !can_force_cancel {
            return Ok(());
        }

        health_cache
    };

    //
    // Charge any open loan origination fees
    //
    let before_oo = {
        let open_orders = load_open_orders_ref(ctx.accounts.open_orders.as_ref())?;
        let before_oo = OpenOrdersSlim::from_oo(&open_orders);
        let mut account = account_loader.load_full_mut()?;
        let mut base_bank = base_bank_loader.load_mut()?;
        let mut quote_bank = quote_bank_loader.load_mut()?;
        charge_loan_origination_fees(
            &ctx.accounts.group.key(),
            &ctx.accounts.account.key(),
            serum_market.market_index,
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
    cpi_cancel_all_orders(
        ctx.accounts,
        limit,
        &market_bids_ai,
        &market_asks_ai,
        &market_event_queue_ai,
    )?;
    cpi_settle_funds(
        ctx.accounts,
        &market_base_vault_ai,
        &market_quote_vault_ai,
        &market_vault_signer_ai,
        &token_program_ai,
    )?;

    //
    // After-settle tracking
    //
    let after_oo;
    {
        let oo_ai = &ctx.accounts.open_orders.as_ref();
        let open_orders = load_open_orders_ref(oo_ai)?;
        after_oo = OpenOrdersSlim::from_oo(&open_orders);

        emit_stack(Serum3OpenOrdersBalanceLogV2 {
            mango_group: ctx.accounts.group.key(),
            mango_account: ctx.accounts.account.key(),
            market_index: serum_market.market_index,
            base_token_index: serum_market.base_token_index,
            quote_token_index: serum_market.quote_token_index,
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
    apply_settle_changes(
        &group,
        ctx.accounts.account.key(),
        &mut account.borrow_mut(),
        &mut base_bank,
        &mut quote_bank,
        &serum_market,
        before_base_vault,
        before_quote_vault,
        &before_oo,
        after_base_vault,
        after_quote_vault,
        &after_oo,
        Some(&mut health_cache),
        true,
        None,
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
    ctx: &Serum3LiqForceCancelOrders<'info>,
    limit: u8,
    market_bids_ai: &AccountInfo<'info>,
    market_asks_ai: &AccountInfo<'info>,
    market_event_queue_ai: &AccountInfo<'info>,
) -> Result<()> {
    use crate::serum3_cpi;
    let group_loader = AccountLoader::<Group>::try_from(&ctx.group.to_account_info())?;
    let group = group_loader.load()?;
    serum3_cpi::CancelOrder {
        program: ctx.serum_program.to_account_info(),
        market: ctx.serum_market_external.to_account_info(),
        bids: market_bids_ai.clone(),
        asks: market_asks_ai.clone(),
        event_queue: market_event_queue_ai.clone(),

        open_orders: ctx.open_orders.to_account_info(),
        open_orders_authority: ctx.group.to_account_info(),
    }
    .cancel_all(&group, limit)
}

fn cpi_settle_funds<'info>(
    ctx: &Serum3LiqForceCancelOrders<'info>,
    market_base_vault_ai: &AccountInfo<'info>,
    market_quote_vault_ai: &AccountInfo<'info>,
    market_vault_signer_ai: &AccountInfo<'info>,
    token_program_ai: &AccountInfo<'info>,
) -> Result<()> {
    use crate::serum3_cpi;
    let group_loader = AccountLoader::<Group>::try_from(&ctx.group.to_account_info())?;
    let group = group_loader.load()?;
    serum3_cpi::SettleFunds {
        program: ctx.serum_program.to_account_info(),
        market: ctx.serum_market_external.to_account_info(),
        open_orders: ctx.open_orders.to_account_info(),
        open_orders_authority: ctx.group.to_account_info(),
        base_vault: market_base_vault_ai.clone(),
        quote_vault: market_quote_vault_ai.clone(),
        user_base_wallet: ctx.base_vault.to_account_info(),
        user_quote_wallet: ctx.quote_vault.to_account_info(),
        vault_signer: market_vault_signer_ai.clone(),
        token_program: token_program_ai.clone(),
        rebates_quote_wallet: ctx.quote_vault.to_account_info(),
    }
    .call(&group)
}
