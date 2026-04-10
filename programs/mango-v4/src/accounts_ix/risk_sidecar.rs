use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;

#[derive(Accounts)]
#[instruction(snapshot_capacity: u32)]
pub struct RiskSidecarCreate<'info> {
    #[account(
        constraint = group.load()?.is_ix_enabled(IxGate::HealthCheck) @ MangoError::IxIsDisabled,
    )]
    pub group: AccountLoader<'info, Group>,

    #[account(
        mut,
        has_one = group,
        constraint = account.load()?.is_operational() @ MangoError::AccountIsFrozen
    )]
    pub account: AccountLoader<'info, MangoAccountFixed>,
    pub owner: Signer<'info>,

    #[account(
        init,
        seeds = [b"RiskSidecar".as_ref(), group.key().as_ref(), account.key().as_ref()],
        bump,
        payer = payer,
        space = RiskSidecar::space(snapshot_capacity as usize),
    )]
    pub risk_sidecar: Account<'info, RiskSidecar>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RiskSidecarRefresh<'info> {
    #[account(
        constraint = group.load()?.is_ix_enabled(IxGate::HealthCheck) @ MangoError::IxIsDisabled,
    )]
    pub group: AccountLoader<'info, Group>,

    #[account(
        mut,
        has_one = group,
        constraint = account.load()?.is_operational() @ MangoError::AccountIsFrozen
    )]
    pub account: AccountLoader<'info, MangoAccountFixed>,
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"RiskSidecar".as_ref(), group.key().as_ref(), account.key().as_ref()],
        bump,
    )]
    pub risk_sidecar: Account<'info, RiskSidecar>,
}
