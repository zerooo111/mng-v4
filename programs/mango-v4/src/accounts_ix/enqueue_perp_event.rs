use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct EnqueuePerpEventArgs {
    pub seq_no: u64,
    pub user: Pubkey,
    pub event_type: QueueEventType,
    pub params: CompactOrderParamsArgs,
    pub user_signature: [u8; 64],
    pub continuum_signature: [u8; 64],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CompactOrderParamsArgs {
    pub market_index: u16,
    pub side: u8,
    pub order_type: u8,
    pub self_trade_behavior: u8,
    pub reduce_only: u8,
    pub limit: u8,
    pub tif_offset: u16,
    pub max_base_lots: i64,
    pub max_quote_lots: i64,
    pub price_lots: i64,
    pub client_order_id: u64,
}

impl From<CompactOrderParamsArgs> for CompactOrderParams {
    fn from(value: CompactOrderParamsArgs) -> Self {
        Self {
            market_index: value.market_index,
            side: value.side,
            order_type: value.order_type,
            self_trade_behavior: value.self_trade_behavior,
            reduce_only: value.reduce_only,
            limit: value.limit,
            tif_offset: value.tif_offset,
            max_base_lots: value.max_base_lots,
            max_quote_lots: value.max_quote_lots,
            price_lots: value.price_lots,
            client_order_id: value.client_order_id,
            padding: [0u8; 7],
        }
    }
}

#[derive(Accounts)]
pub struct EnqueuePerpEvent<'info> {
    #[account(
        mut,
        constraint = group.load()?.is_ix_enabled(IxGate::PerpEnqueueEvent) @ MangoError::IxIsDisabled,
    )]
    pub group: AccountLoader<'info, Group>,

    #[account(mut)]
    pub queue: AccountLoader<'info, QueueFifo>,

    /// CHECK: instruction sysvar is read to validate ed25519 signatures
    #[account(address = sysvar::instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}
