// Account contexts for execution_queue_v5 instructions. Single queue
// account per group (no per-market or per-page PDAs — see
// state/execution_queue_v5.rs for design rationale).

use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;

// ── Lifecycle (create → resize loop → init) ────────────────────────────

#[derive(Accounts)]
pub struct ExecutionQueueV5Create<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        init,
        payer = payer,
        space = 8,
        seeds = [b"execution-queue-v5".as_ref(), group.key().as_ref()],
        bump,
    )]
    /// CHECK: chunked grow target; zero-copy init deferred.
    pub queue: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV5Resize<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        seeds = [b"execution-queue-v5".as_ref(), group.key().as_ref()],
        bump,
    )]
    /// CHECK: chunked grow target.
    pub queue: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV5Init<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        zero,
        seeds = [b"execution-queue-v5".as_ref(), group.key().as_ref()],
        bump,
    )]
    pub queue: AccountLoader<'info, ExecutionQueueV5>,
    pub admin: Signer<'info>,
}

// ── Admin: configure / pause / close ──────────────────────────────────

#[derive(Accounts)]
pub struct ExecutionQueueV5Admin<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        constraint = queue.load()?.header.group == group.key() @ MangoError::SomeError,
        constraint = queue.load()?.header.authority_state == authority_state.key() @ MangoError::SomeError,
    )]
    pub queue: AccountLoader<'info, ExecutionQueueV5>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV5Close<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        constraint = queue.load()?.header.group == group.key() @ MangoError::SomeError,
        close = receiver,
    )]
    pub queue: AccountLoader<'info, ExecutionQueueV5>,
    #[account(mut)]
    pub receiver: Signer<'info>,
    pub admin: Signer<'info>,
}

// ── Commit / reveal ───────────────────────────────────────────────────

#[derive(Accounts)]
pub struct ExecutionQueueV5CommitMarket<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        constraint = queue.load()?.header.group == group.key() @ MangoError::SomeError,
        constraint = queue.load()?.header.authority_state == authority_state.key() @ MangoError::SomeError,
    )]
    pub queue: AccountLoader<'info, ExecutionQueueV5>,
    /// CHECK: fixed instructions sysvar account; used to verify the
    /// relayer's ed25519 signature on the batch.
    #[account(address = tx_instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV5RevealExecuteMarket<'info> {
    #[account(mut)]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        constraint = queue.load()?.header.group == group.key() @ MangoError::SomeError,
        constraint = queue.load()?.header.authority_state == authority_state.key() @ MangoError::SomeError,
    )]
    pub queue: AccountLoader<'info, ExecutionQueueV5>,
    /// CHECK: fixed instructions sysvar account; only needed when user
    /// signatures are verified on the reveal batch.
    #[account(address = tx_instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}
