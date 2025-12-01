use crate::error::Contextable;
use crate::error::MangoError;
use crate::error_msg;
use crate::error_msg_typed;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hashv;
use bytemuck::bytes_of;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use static_assertions::const_assert_eq;
use std::mem::size_of;

use super::{PlaceOrderType, SelfTradeBehavior, Side};

pub const FIFO_QUEUE_CAPACITY: usize = 64;

#[account(zero_copy)]
#[repr(C)]
pub struct QueueFifo {
    pub header: QueueFifoHeader,
    pub entries: [QueueFifoEvent; FIFO_QUEUE_CAPACITY],
}

const_assert_eq!(size_of::<QueueFifo>() % 8, 0);

impl QueueFifo {
    pub fn is_full(&self) -> bool {
        self.header.count as usize >= FIFO_QUEUE_CAPACITY
    }

    pub fn tail_index(&self) -> Option<usize> {
        if self.header.count == 0 {
            None
        } else {
            Some(((self.header.head + self.header.count - 1) % FIFO_QUEUE_CAPACITY as u32) as usize)
        }
    }

    pub fn tail_seq(&self) -> u64 {
        self.tail_index()
            .map(|index| self.entries[index].seq_no)
            .unwrap_or(self.header.last_executed_seq)
    }

    pub fn next_index(&self) -> usize {
        ((self.header.head + self.header.count) % FIFO_QUEUE_CAPACITY as u32) as usize
    }
}

#[zero_copy]
#[repr(C)]
#[derive(Debug, Default)]
pub struct QueueFifoHeader {
    pub head: u32,
    pub count: u32,
    pub last_executed_seq: u64,
    pub max_lag_slots: u32,
    pub reserved: u32,
}

const_assert_eq!(size_of::<QueueFifoHeader>() % 8, 0);

#[zero_copy]
#[repr(C)]
#[derive(Debug, Default)]
pub struct QueueFifoEvent {
    pub seq_no: u64,
    pub submitted_slot: u64,
    pub user: Pubkey,
    pub continuum: Pubkey,
    pub event_type: u8,
    pub padding: [u8; 7],
    pub params: CompactOrderParams,
    pub user_signature: SignatureBlob,
    pub continuum_signature: SignatureBlob,
}

impl QueueFifoEvent {
    pub fn queue_event_type(&self) -> Result<QueueEventType> {
        QueueEventType::try_from(self.event_type).map_err(|_| {
            error_msg_typed!(
                MangoError::UnsupportedQueueEventType,
                "queue_fifo: invalid queue event type for entry"
            )
        })
    }

    fn signature_flags(&self) -> [u8; 2] {
        [
            self.user_signature.is_present,
            self.continuum_signature.is_present,
        ]
    }

    /// Hash of the user intent. This intentionally omits `seq_no`
    /// so the same payload can be signed before being added to the queue.
    pub fn intent_hash(&self) -> [u8; 32] {
        hashv(&[
            self.user.as_ref(),
            self.continuum.as_ref(),
            &[self.event_type],
            &self.signature_flags(),
            bytes_of(&self.params),
        ])
        .to_bytes()
    }

    /// Hash of the actual payload that gets executed from the queue.
    /// Includes `seq_no` so signatures can cover ordering within the queue.
    pub fn payload_hash(&self) -> [u8; 32] {
        hashv(&[
            &self.seq_no.to_le_bytes(),
            self.user.as_ref(),
            self.continuum.as_ref(),
            &[self.event_type],
            &self.signature_flags(),
            bytes_of(&self.params),
        ])
        .to_bytes()
    }
}

const_assert_eq!(size_of::<QueueFifoEvent>() % 8, 0);

#[zero_copy]
#[repr(C)]
#[derive(Debug, Default)]
pub struct CompactOrderParams {
    pub max_base_lots: i64,
    pub max_quote_lots: i64,
    pub price_lots: i64,
    pub client_order_id: u64,
    pub market_index: u16,
    pub tif_offset: u16,
    pub side: u8,
    pub order_type: u8,
    pub self_trade_behavior: u8,
    pub reduce_only: u8,
    pub limit: u8,
    pub padding: [u8; 7],
}

impl CompactOrderParams {
    pub fn side(&self) -> Result<Side> {
        Side::try_from(self.side).map_err(|_| {
            error_msg_typed!(
                MangoError::InvalidQueueParams,
                "queue_fifo: invalid side in compact order params"
            )
        })
    }

    pub fn place_order_type(&self) -> Result<PlaceOrderType> {
        PlaceOrderType::try_from(self.order_type).map_err(|_| {
            error_msg_typed!(
                MangoError::InvalidQueueParams,
                "queue_fifo: invalid order type in compact order params"
            )
        })
    }

    pub fn self_trade_behavior(&self) -> Result<SelfTradeBehavior> {
        SelfTradeBehavior::try_from(self.self_trade_behavior).map_err(|_| {
            error_msg_typed!(
                MangoError::InvalidQueueParams,
                "queue_fifo: invalid self trade behavior in compact order params"
            )
        })
    }

    pub fn is_reduce_only(&self) -> bool {
        self.reduce_only != 0
    }
}

const_assert_eq!(size_of::<CompactOrderParams>() % 8, 0);

#[zero_copy]
#[repr(C)]
#[derive(Debug)]
pub struct SignatureBlob {
    pub bytes: [u8; 64],
    pub is_present: u8,
    pub padding: [u8; 7],
}

impl Default for SignatureBlob {
    fn default() -> Self {
        Self {
            bytes: [0u8; 64],
            is_present: 0,
            padding: [0u8; 7],
        }
    }
}

const_assert_eq!(size_of::<SignatureBlob>() % 8, 0);

#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    IntoPrimitive,
    TryFromPrimitive,
    AnchorSerialize,
    AnchorDeserialize,
)]
#[repr(u8)]
pub enum QueueEventType {
    PerpPlaceOrder = 0,
    PerpCancelOrder = 1,
    OpenbookV2PlaceOrder = 2,
    OpenbookV2CancelOrder = 3,
    OpenbookV2PlaceTakeOrder = 4,
}

impl QueueEventType {
    pub fn to_perp_action(self) -> Option<PerpQueueAction> {
        match self {
            QueueEventType::PerpPlaceOrder => Some(PerpQueueAction::PlaceOrder),
            QueueEventType::PerpCancelOrder => Some(PerpQueueAction::CancelOrder),
            _ => None,
        }
    }

    pub fn to_openbook_action(self) -> Option<OpenbookQueueAction> {
        match self {
            QueueEventType::OpenbookV2PlaceOrder => Some(OpenbookQueueAction::PlaceOrder),
            QueueEventType::OpenbookV2CancelOrder => Some(OpenbookQueueAction::CancelOrder),
            QueueEventType::OpenbookV2PlaceTakeOrder => Some(OpenbookQueueAction::PlaceTakeOrder),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum PerpQueueAction {
    PlaceOrder = 0,
    CancelOrder = 1,
}

impl TryFrom<QueueEventType> for PerpQueueAction {
    type Error = Error;
    fn try_from(value: QueueEventType) -> Result<Self> {
        value
            .to_perp_action()
            .ok_or_else(|| error_msg!("queue_fifo: queue event is not a perp action"))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum OpenbookQueueAction {
    PlaceOrder = 0,
    CancelOrder = 1,
    PlaceTakeOrder = 2,
}

impl TryFrom<QueueEventType> for OpenbookQueueAction {
    type Error = Error;
    fn try_from(value: QueueEventType) -> Result<Self> {
        value
            .to_openbook_action()
            .ok_or_else(|| error_msg!("queue_fifo: queue event is not an openbook action"))
    }
}

const_assert_eq!(size_of::<QueueFifoEvent>(), 280);
const_assert_eq!(size_of::<QueueFifoHeader>(), 24);
