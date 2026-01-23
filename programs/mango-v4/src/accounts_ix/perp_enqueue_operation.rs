use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpEnqueueOperationArgs {
    pub order_intent: OrderIntent,
    pub queued_order_intent: QueuedOrderIntent,
}

#[derive(Accounts)]
pub struct PerpEnqueueOperation<'info> {
    #[account(
        mut,
        constraint = group.load()?.is_ix_enabled(IxGate::PerpEnqueueEvent) @ MangoError::IxIsDisabled,
    )]
    pub group: AccountLoader<'info, Group>,

    #[account(mut)]
    pub queue: AccountLoader<'info, QueueFifo>,

    #[account(mut, has_one = group)]
    pub account: AccountLoader<'info, MangoAccount>,

    #[account(has_one = group)]
    pub perp_market: AccountLoader<'info, PerpMarket>,

    /// CHECK: instruction sysvar is read to validate ed25519 signatures
    #[account(address = sysvar::instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}
