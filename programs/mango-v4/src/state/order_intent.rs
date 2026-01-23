use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hashv;

use super::{CompactOrderParams, QueueEventType};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct CompactOrderParamsPayload {
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

impl From<CompactOrderParamsPayload> for CompactOrderParams {
    fn from(value: CompactOrderParamsPayload) -> Self {
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

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct OrderIntent {
    pub version: u8,
    pub group: Pubkey,
    pub account: Pubkey,
    pub owner: Pubkey,
    pub perp_market: Pubkey,
    pub event_type: QueueEventType,
    pub params: CompactOrderParamsPayload,
    pub expiry_ts: u64,
}

impl OrderIntent {
    pub fn message_bytes(&self) -> Result<Vec<u8>> {
        self.try_to_vec()
            .map_err(|_| error!(crate::error::MangoError::SomeError))
    }

    pub fn intent_hash(&self) -> Result<[u8; 32]> {
        let bytes = self.message_bytes()?;
        Ok(hashv(&[&bytes]).to_bytes())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct QueuedOrderIntent {
    pub version: u8,
    pub order_intent_hash: [u8; 32],
    pub seq_no: u64,
    pub expiry_ts: u64,
}

impl QueuedOrderIntent {
    pub fn message_bytes(&self) -> Result<Vec<u8>> {
        self.try_to_vec()
            .map_err(|_| error!(crate::error::MangoError::SomeError))
    }
}
