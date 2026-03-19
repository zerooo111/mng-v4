use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct UnsafeDeposit<'info> {
    #[account(
        constraint = group.load()?.is_testing() @ MangoError::SomeError,
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
        constraint = group.load()?.is_ix_enabled(IxGate::TokenDeposit) @ MangoError::IxIsDisabled,
    )]
    pub group: AccountLoader<'info, Group>,

    #[account(
        mut,
        has_one = group,
        constraint = account.load()?.is_operational() @ MangoError::AccountIsFrozen
    )]
    pub account: AccountLoader<'info, MangoAccountFixed>,

    pub admin: Signer<'info>,

    #[account(
        mut,
        has_one = group,
        has_one = oracle,
    )]
    pub bank: AccountLoader<'info, Bank>,

    /// CHECK: The oracle can be one of several different account types
    pub oracle: UncheckedAccount<'info>,
}
