use crate::error::MangoError;
use anchor_lang::prelude::*;
use static_assertions::const_assert_eq;
use std::mem::size_of;

pub const EXECUTION_QUEUE_CAPACITY: usize = 1000;
pub const EXECUTION_QUEUE_PAYLOAD_MAX: usize = 256;
pub const EXECUTION_QUEUE_DISCRIMINATOR_BYTES: usize = 8;
pub const EXECUTION_QUEUE_ACCOUNT_SPACE: usize = 8 + size_of::<ExecutionQueue>();
pub const EXECUTION_QUEUE_CREATE_SPACE: usize = 8;

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemKind {
    CtmWrapped = 0,
    LiquidityDeposit = 1,
    LiquidityWithdraw = 2,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemStatus {
    Empty = 0,
    Pending = 1,
    Executed = 2,
    Failed = 3,
    Skipped = 4,
}

#[zero_copy]
#[derive(Debug)]
pub struct QueueItem {
    pub sequence: u64,
    pub min_execute_slot: u64,
    pub ingress_slot: u64,
    pub first_failure_slot: u64,
    pub kind: u8,
    pub status: u8,
    pub retries: u8,
    pub _padding: [u8; 5],
    pub payload_len: u16,
    pub _padding2: [u8; 6],
    pub payload_hash: [u8; 32],
    pub accounts_hash: [u8; 32],
    pub payload: [u8; EXECUTION_QUEUE_PAYLOAD_MAX],
}

impl Default for QueueItem {
    fn default() -> Self {
        Self {
            sequence: 0,
            min_execute_slot: 0,
            ingress_slot: 0,
            first_failure_slot: 0,
            kind: QueueItemKind::CtmWrapped as u8,
            status: QueueItemStatus::Empty as u8,
            retries: 0,
            _padding: [0; 5],
            payload_len: 0,
            _padding2: [0; 6],
            payload_hash: [0; 32],
            accounts_hash: [0; 32],
            payload: [0; EXECUTION_QUEUE_PAYLOAD_MAX],
        }
    }
}
const_assert_eq!(size_of::<QueueItem>(), 368);
const_assert_eq!(size_of::<QueueItem>() % 8, 0);

#[zero_copy]
#[derive(Debug)]
pub struct ExecutionQueueHeader {
    pub head: u32,
    pub count: u32,
    pub next_sequence_to_execute: u64,
    pub max_seen_sequence: u64,
    pub gap_observed_slot: u64,
    pub gap_wait_slots: u64,
    pub liquidity_delay_slots: u64,
    pub reserved: [u8; 16],
}
const_assert_eq!(size_of::<ExecutionQueueHeader>(), 64);
const_assert_eq!(size_of::<ExecutionQueueHeader>() % 8, 0);

#[account(zero_copy)]
#[derive(Debug)]
pub struct ExecutionQueue {
    pub group: Pubkey,
    pub admin: Pubkey,
    pub ctm_signer: Pubkey,
    pub pending_ctm_signer: Pubkey,
    pub pending_ctm_activate_slot: u64,
    pub bump: u8,
    pub paused_ingress: u8,
    pub paused_execute: u8,
    pub _padding: [u8; 5],
    pub header: ExecutionQueueHeader,
    pub reserved: [u8; 64],
    pub items: [QueueItem; EXECUTION_QUEUE_CAPACITY],
}
const_assert_eq!(size_of::<ExecutionQueue>(), 368272);
const_assert_eq!(size_of::<ExecutionQueue>() % 8, 0);
const_assert_eq!(EXECUTION_QUEUE_ACCOUNT_SPACE, 368280);

pub const EXECUTION_QUEUE_HEADER_OFFSET: usize = 152;
pub const EXECUTION_QUEUE_ITEMS_OFFSET: usize = 280;
pub const EXECUTION_QUEUE_ITEM_SIZE: usize = size_of::<QueueItem>();
pub const EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET: usize = 0;
pub const EXECUTION_QUEUE_ITEM_MIN_EXECUTE_SLOT_OFFSET: usize = 8;
pub const EXECUTION_QUEUE_ITEM_KIND_OFFSET: usize = 32;
pub const EXECUTION_QUEUE_ITEM_STATUS_OFFSET: usize = 33;
pub const EXECUTION_QUEUE_ITEM_RETRIES_OFFSET: usize = 34;
pub const EXECUTION_QUEUE_ITEM_PAYLOAD_LEN_OFFSET: usize = 40;
pub const EXECUTION_QUEUE_ITEM_PAYLOAD_HASH_OFFSET: usize = 48;
pub const EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET: usize = 80;
pub const EXECUTION_QUEUE_ITEM_PAYLOAD_OFFSET: usize = 112;
pub const EXECUTION_QUEUE_HEAD_OFFSET: usize = 152;
pub const EXECUTION_QUEUE_COUNT_OFFSET: usize = 156;
pub const EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET: usize = 160;
pub const EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET: usize = 168;
const_assert_eq!(EXECUTION_QUEUE_HEADER_OFFSET, 152);
const_assert_eq!(EXECUTION_QUEUE_HEAD_OFFSET, 152);
const_assert_eq!(EXECUTION_QUEUE_COUNT_OFFSET, 156);
const_assert_eq!(EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 160);
const_assert_eq!(EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 168);
const_assert_eq!(EXECUTION_QUEUE_ITEMS_OFFSET, 280);
const_assert_eq!(EXECUTION_QUEUE_ITEM_SIZE, 368);
const_assert_eq!(EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET, 0);
const_assert_eq!(EXECUTION_QUEUE_ITEM_MIN_EXECUTE_SLOT_OFFSET, 8);
const_assert_eq!(EXECUTION_QUEUE_ITEM_KIND_OFFSET, 32);
const_assert_eq!(EXECUTION_QUEUE_ITEM_STATUS_OFFSET, 33);
const_assert_eq!(EXECUTION_QUEUE_ITEM_RETRIES_OFFSET, 34);
const_assert_eq!(EXECUTION_QUEUE_ITEM_PAYLOAD_LEN_OFFSET, 40);
const_assert_eq!(EXECUTION_QUEUE_ITEM_PAYLOAD_HASH_OFFSET, 48);
const_assert_eq!(EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET, 80);
const_assert_eq!(EXECUTION_QUEUE_ITEM_PAYLOAD_OFFSET, 112);

impl ExecutionQueue {
    pub fn init(&mut self, group: Pubkey, admin: Pubkey, ctm_signer: Pubkey, bump: u8) {
        self.group = group;
        self.admin = admin;
        self.ctm_signer = ctm_signer;
        self.pending_ctm_signer = Pubkey::default();
        self.pending_ctm_activate_slot = 0;
        self.bump = bump;
        self.paused_ingress = 0;
        self.paused_execute = 0;
        self._padding = [0; 5];
        self.header = ExecutionQueueHeader {
            head: 0,
            count: 0,
            next_sequence_to_execute: 0,
            max_seen_sequence: 0,
            gap_observed_slot: 0,
            gap_wait_slots: 50,
            liquidity_delay_slots: 25,
            reserved: [0; 16],
        };
        self.reserved = [0; 64];
        self.items = [QueueItem::default(); EXECUTION_QUEUE_CAPACITY];
    }

    pub fn maybe_activate_pending_ctm(&mut self, current_slot: u64) {
        if self.pending_ctm_signer != Pubkey::default()
            && current_slot >= self.pending_ctm_activate_slot
        {
            self.ctm_signer = self.pending_ctm_signer;
            self.pending_ctm_signer = Pubkey::default();
            self.pending_ctm_activate_slot = 0;
        }
    }

    pub fn capacity(&self) -> usize {
        EXECUTION_QUEUE_CAPACITY
    }

    pub fn len(&self) -> usize {
        self.header.count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }

    pub fn logical_to_physical(&self, logical_index: usize) -> usize {
        (self.header.head as usize + logical_index) % self.capacity()
    }

    pub fn item(&self, logical_index: usize) -> &QueueItem {
        &self.items[self.logical_to_physical(logical_index)]
    }

    pub fn item_mut(&mut self, logical_index: usize) -> &mut QueueItem {
        let physical = self.logical_to_physical(logical_index);
        &mut self.items[physical]
    }

    pub fn push_back(&mut self, item: QueueItem) -> Result<()> {
        require!(!self.is_full(), MangoError::ExecutionQueueFull);
        let tail = self.logical_to_physical(self.len());
        self.items[tail] = item;
        self.header.count = self.header.count.saturating_add(1);
        Ok(())
    }

    pub fn remove_logical_index(&mut self, logical_index: usize) -> Result<QueueItem> {
        require!(logical_index < self.len(), MangoError::SomeError);
        let len = self.len();
        let removed = *self.item(logical_index);

        if logical_index == 0 {
            let head = self.header.head as usize;
            self.items[head] = QueueItem::default();
            self.header.head = ((head + 1) % self.capacity()) as u32;
            self.header.count = self.header.count.saturating_sub(1);
            return Ok(removed);
        }

        let mut logical = logical_index;
        while logical + 1 < len {
            let src = self.logical_to_physical(logical + 1);
            let dst = self.logical_to_physical(logical);
            self.items[dst] = self.items[src];
            logical += 1;
        }

        let tail = self.logical_to_physical(len - 1);
        self.items[tail] = QueueItem::default();
        self.header.count = self.header.count.saturating_sub(1);
        Ok(removed)
    }
}
