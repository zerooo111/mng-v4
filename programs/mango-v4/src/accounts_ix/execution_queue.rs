use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;

#[derive(Accounts)]
pub struct ExecutionQueueInit<'info> {
    #[account(
        mut,
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        init,
        payer = payer,
        space = 8 + std::mem::size_of::<ExecutionQueue>(),
        seeds = [b"ExecutionQueue".as_ref(), group.key().as_ref()],
        bump,
    )]
    pub execution_queue: AccountLoader<'info, ExecutionQueue>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecutionQueueAdmin<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
    )]
    pub execution_queue: AccountLoader<'info, ExecutionQueue>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueEnqueueCtm<'info> {
    #[account(mut)]
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
    )]
    pub execution_queue: AccountLoader<'info, ExecutionQueue>,
    /// CHECK: fixed instructions sysvar account
    #[account(address = tx_instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueEnqueueLiquidity<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
    )]
    pub execution_queue: AccountLoader<'info, ExecutionQueue>,
}

#[derive(Accounts)]
pub struct ExecutionQueueExecute<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
    )]
    pub execution_queue: AccountLoader<'info, ExecutionQueue>,
}
