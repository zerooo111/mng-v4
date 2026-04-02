use anchor_lang::prelude::*;

use crate::error::*;
use crate::instructions::account_init::{create_pda_account, write_discriminator};
use crate::serum3_cpi::{load_market_state, pubkey_from_u64_array};
use crate::state::*;
use crate::util::fill_from_str;

use crate::accounts_ix::*;
use crate::logs::{emit_stack, Serum3RegisterMarketLog};

pub fn serum3_register_market(
    ctx: Context<Serum3RegisterMarket>,
    market_index: Serum3MarketIndex,
    name: String,
    oracle_price_band: f32,
) -> Result<()> {
    let is_fast_listing;
    let group = ctx.accounts.group.load()?;
    // checking the admin account (#1)
    if ctx.accounts.admin.key() == group.admin {
        is_fast_listing = false;
    } else if ctx.accounts.admin.key() == group.fast_listing_admin {
        is_fast_listing = true;
    } else {
        return Err(error_msg!(
            "admin must be the group admin or group fast listing admin"
        ));
    }

    let base_bank = ctx.accounts.base_bank.load()?;
    let quote_bank = ctx.accounts.quote_bank.load()?;
    let market_external = load_market_state(
        &ctx.accounts.serum_market_external,
        &ctx.accounts.serum_program.key(),
    )?;
    require!(
        pubkey_from_u64_array(market_external.pc_mint) == quote_bank.mint,
        MangoError::SomeError
    );
    require!(
        pubkey_from_u64_array(market_external.coin_mint) == base_bank.mint,
        MangoError::SomeError
    );

    if is_fast_listing {
        // Safety parameters have fixed values when fast listing is used.

        // C tier tokens (no borrows, no asset weight) allow wider bands if the quote token has
        // no deposit limits
        let base_c_tier =
            base_bank.are_borrows_reduce_only() && base_bank.maint_asset_weight.is_zero();
        let quote_has_no_deposit_limit = quote_bank.deposit_weight_scale_start_quote == f64::MAX
            && quote_bank.deposit_limit == 0;
        if base_c_tier && quote_has_no_deposit_limit {
            require_eq!(oracle_price_band, 19.0);
        } else {
            require_eq!(oracle_price_band, 1.0);
        }
    }

    let group_key = ctx.accounts.group.key();
    let serum_market_external_key = ctx.accounts.serum_market_external.key();
    let market_index_bytes = market_index.to_le_bytes();
    let serum_market_bump = *ctx.bumps.get("serum_market").ok_or(MangoError::SomeError)?;
    let serum_market_seeds = &[
        b"Serum3Market".as_ref(),
        group_key.as_ref(),
        serum_market_external_key.as_ref(),
        &[serum_market_bump],
    ];
    create_pda_account(
        &ctx.accounts.payer,
        &ctx.accounts.serum_market.to_account_info(),
        &ctx.accounts.system_program,
        serum_market_seeds,
        8 + std::mem::size_of::<Serum3Market>(),
        ctx.program_id,
    )?;

    let index_reservation_bump = *ctx
        .bumps
        .get("index_reservation")
        .ok_or(MangoError::SomeError)?;
    let index_reservation_seeds = &[
        b"Serum3Index".as_ref(),
        group_key.as_ref(),
        &market_index_bytes,
        &[index_reservation_bump],
    ];
    create_pda_account(
        &ctx.accounts.payer,
        &ctx.accounts.index_reservation.to_account_info(),
        &ctx.accounts.system_program,
        index_reservation_seeds,
        8 + std::mem::size_of::<Serum3MarketIndexReservation>(),
        ctx.program_id,
    )?;

    let serum_market_loader = AccountLoader::<Serum3Market>::try_from_unchecked(
        ctx.program_id,
        &ctx.accounts.serum_market,
    )?;
    let mut serum_market = serum_market_loader.load_init()?;
    *serum_market = Serum3Market {
        group: ctx.accounts.group.key(),
        base_token_index: base_bank.token_index,
        quote_token_index: quote_bank.token_index,
        reduce_only: 0,
        force_close: 0,
        padding1: Default::default(),
        name: fill_from_str(&name)?,
        serum_program: ctx.accounts.serum_program.key(),
        serum_market_external: ctx.accounts.serum_market_external.key(),
        market_index,
        bump: serum_market_bump,
        padding2: Default::default(),
        oracle_price_band,
        registration_time: Clock::get()?.unix_timestamp.try_into().unwrap(),
        reserved: [0; 128],
    };

    let serum_index_reservation_loader =
        AccountLoader::<Serum3MarketIndexReservation>::try_from_unchecked(
            ctx.program_id,
            &ctx.accounts.index_reservation,
        )?;
    let mut serum_index_reservation = serum_index_reservation_loader.load_init()?;
    *serum_index_reservation = Serum3MarketIndexReservation {
        group: ctx.accounts.group.key(),
        market_index,
        reserved: [0; 38],
    };

    emit_stack(Serum3RegisterMarketLog {
        mango_group: ctx.accounts.group.key(),
        serum_market: ctx.accounts.serum_market.key(),
        market_index,
        base_token_index: base_bank.token_index,
        quote_token_index: quote_bank.token_index,
        serum_program: ctx.accounts.serum_program.key(),
        serum_program_external: ctx.accounts.serum_market_external.key(),
    });

    drop(serum_index_reservation);
    drop(serum_market);

    write_discriminator::<Serum3Market>(&ctx.accounts.serum_market.to_account_info())?;
    write_discriminator::<Serum3MarketIndexReservation>(
        &ctx.accounts.index_reservation.to_account_info(),
    )?;

    Ok(())
}
