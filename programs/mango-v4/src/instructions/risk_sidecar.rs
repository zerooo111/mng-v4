use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::*;
use crate::health::*;
use crate::state::*;
use crate::util::clock_now;

pub fn risk_sidecar_create(
    ctx: Context<RiskSidecarCreate>,
    snapshot_capacity: u32,
) -> Result<()> {
    let account = ctx.accounts.account.load_full()?;
    require!(
        account.fixed.is_owner_or_delegate(ctx.accounts.owner.key()),
        MangoError::SomeError
    );
    let required_capacity = required_risk_sidecar_snapshot_capacity(&account.borrow());
    require!(
        snapshot_capacity as usize >= required_capacity,
        MangoError::RiskSidecarCapacityTooSmall
    );
    drop(account);

    let (now_ts, now_slot) = clock_now();
    let group_key = ctx.accounts.group.key();
    let account_pk = ctx.accounts.account.key();

    let account = ctx.accounts.account.load_full()?;
    let account_ref = account.borrow();
    let account_state_hash = risk_sidecar_account_state_hash(&account_ref)?;
    let health_accounts_state_hash =
        risk_sidecar_health_accounts_state_hash(&account_ref, ctx.remaining_accounts, now_slot, now_ts)?;
    let snapshot = ExactRiskSidecarSnapshot::from_fixed_accounts(
        account_pk,
        &account_ref,
        ctx.remaining_accounts,
        now_slot,
        now_ts,
    )
    .context("risk sidecar create snapshot")?;
    drop(account);

    refresh_risk_sidecar_account(
        &mut ctx.accounts.risk_sidecar,
        &snapshot,
        account_state_hash,
        health_accounts_state_hash,
        now_slot,
        now_ts,
        true,
    )?;
    ctx.accounts.risk_sidecar.bump = RiskSidecar::pda(&group_key, &account_pk).1;
    Ok(())
}

pub fn risk_sidecar_refresh(ctx: Context<RiskSidecarRefresh>) -> Result<()> {
    let account = ctx.accounts.account.load_full()?;
    require!(
        account.fixed.is_owner_or_delegate(ctx.accounts.owner.key()),
        MangoError::SomeError
    );
    drop(account);

    let (now_ts, now_slot) = clock_now();
    let group_key = ctx.accounts.group.key();
    let account_pk = ctx.accounts.account.key();

    let account = ctx.accounts.account.load_full()?;
    let account_ref = account.borrow();
    let account_state_hash = risk_sidecar_account_state_hash(&account_ref)?;
    let health_accounts_state_hash =
        risk_sidecar_health_accounts_state_hash(&account_ref, ctx.remaining_accounts, now_slot, now_ts)?;
    let snapshot_capacity_required = required_risk_sidecar_snapshot_capacity(&account_ref);
    let snapshot_capacity_available =
        RiskSidecar::snapshot_capacity(ctx.accounts.risk_sidecar.to_account_info().data_len());
    require!(
        snapshot_capacity_available >= snapshot_capacity_required,
        MangoError::RiskSidecarCapacityTooSmall
    );
    let snapshot = ExactRiskSidecarSnapshot::from_fixed_accounts(
        account_pk,
        &account_ref,
        ctx.remaining_accounts,
        now_slot,
        now_ts,
    )
    .context("risk sidecar refresh snapshot")?;
    drop(account);

    refresh_risk_sidecar_account(
        &mut ctx.accounts.risk_sidecar,
        &snapshot,
        account_state_hash,
        health_accounts_state_hash,
        now_slot,
        now_ts,
        true,
    )?;
    ctx.accounts.risk_sidecar.bump = RiskSidecar::pda(&group_key, &account_pk).1;
    Ok(())
}
