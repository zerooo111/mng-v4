use anchor_lang::prelude::*;

use crate::error::*;
use crate::instructions::account_init::{create_pda_account, write_discriminator};
use crate::state::*;
use crate::util::fill_from_str;

use crate::accounts_ix::*;
use crate::logs::{emit_stack, OpenbookV2RegisterMarketLog};

pub fn openbook_v2_register_market(
    ctx: Context<OpenbookV2RegisterMarket>,
    market_index: OpenbookV2MarketIndex,
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
    let market_external = ctx.accounts.openbook_v2_market_external.load()?;
    require_keys_eq!(
        market_external.quote_mint,
        quote_bank.mint,
        MangoError::SomeError
    );
    require_keys_eq!(
        market_external.base_mint,
        base_bank.mint,
        MangoError::SomeError
    );

    if is_fast_listing {
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
    let market_external_key = ctx.accounts.openbook_v2_market_external.key();
    let market_index_bytes = market_index.to_le_bytes();

    let openbook_market_bump = *ctx
        .bumps
        .get("openbook_v2_market")
        .ok_or(MangoError::SomeError)?;
    let openbook_market_seeds = &[
        b"OpenbookV2Market".as_ref(),
        group_key.as_ref(),
        market_external_key.as_ref(),
        &[openbook_market_bump],
    ];
    create_pda_account(
        &ctx.accounts.payer,
        &ctx.accounts.openbook_v2_market.to_account_info(),
        &ctx.accounts.system_program,
        openbook_market_seeds,
        8 + std::mem::size_of::<OpenbookV2Market>(),
        ctx.program_id,
    )?;

    let index_reservation_bump = *ctx
        .bumps
        .get("index_reservation")
        .ok_or(MangoError::SomeError)?;
    let index_reservation_seeds = &[
        b"OpenbookV2Index".as_ref(),
        group_key.as_ref(),
        &market_index_bytes,
        &[index_reservation_bump],
    ];
    create_pda_account(
        &ctx.accounts.payer,
        &ctx.accounts.index_reservation.to_account_info(),
        &ctx.accounts.system_program,
        index_reservation_seeds,
        8 + std::mem::size_of::<OpenbookV2MarketIndexReservation>(),
        ctx.program_id,
    )?;

    let openbook_market_loader = AccountLoader::<OpenbookV2Market>::try_from_unchecked(
        ctx.program_id,
        &ctx.accounts.openbook_v2_market,
    )?;
    let mut openbook_market = openbook_market_loader.load_init()?;
    *openbook_market = OpenbookV2Market {
        group: ctx.accounts.group.key(),
        base_token_index: base_bank.token_index,
        quote_token_index: quote_bank.token_index,
        reduce_only: 0,
        force_close: 0,
        name: fill_from_str(&name)?,
        openbook_v2_program: ctx.accounts.openbook_v2_program.key(),
        openbook_v2_market_external: ctx.accounts.openbook_v2_market_external.key(),
        market_index,
        bump: openbook_market_bump,
        oracle_price_band,
        registration_time: Clock::get()?.unix_timestamp.try_into().unwrap(),
        reserved: [0; 1027],
    };

    let openbook_index_reservation_loader =
        AccountLoader::<OpenbookV2MarketIndexReservation>::try_from_unchecked(
            ctx.program_id,
            &ctx.accounts.index_reservation,
        )?;
    let mut openbook_index_reservation = openbook_index_reservation_loader.load_init()?;
    *openbook_index_reservation = OpenbookV2MarketIndexReservation {
        group: ctx.accounts.group.key(),
        market_index,
        reserved: [0; 38],
    };

    emit_stack(OpenbookV2RegisterMarketLog {
        mango_group: ctx.accounts.group.key(),
        openbook_market: ctx.accounts.openbook_v2_market.key(),
        market_index,
        base_token_index: base_bank.token_index,
        quote_token_index: quote_bank.token_index,
        openbook_program: ctx.accounts.openbook_v2_program.key(),
        openbook_market_external: ctx.accounts.openbook_v2_market_external.key(),
    });

    drop(openbook_index_reservation);
    drop(openbook_market);

    write_discriminator::<OpenbookV2Market>(&ctx.accounts.openbook_v2_market.to_account_info())?;
    write_discriminator::<OpenbookV2MarketIndexReservation>(
        &ctx.accounts.index_reservation.to_account_info(),
    )?;

    Ok(())
}
