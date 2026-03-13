use crate::error::MangoError;
use anchor_lang::prelude::*;
use static_assertions::const_assert_eq;
use std::mem::size_of;

pub const EXECUTION_QUEUE_CTM_CAPACITY: usize = 1024;
pub const EXECUTION_QUEUE_LIQUIDITY_CAPACITY: usize = 128;
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
    pub total_count: u32,
    pub liquidity_head: u32,
    pub next_sequence_to_execute: u64,
    pub max_seen_sequence: u64,
    pub gap_observed_slot: u64,
    pub gap_wait_slots: u64,
    pub liquidity_delay_slots: u64,
    pub ctm_count: u32,
    pub liquidity_count: u32,
    pub reserved: [u8; 16],
}
const_assert_eq!(size_of::<ExecutionQueueHeader>(), 72);
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
    pub reserved: [u8; 56],
    pub ctm_items: [QueueItem; EXECUTION_QUEUE_CTM_CAPACITY],
    pub liquidity_items: [QueueItem; EXECUTION_QUEUE_LIQUIDITY_CAPACITY],
}
const_assert_eq!(size_of::<ExecutionQueue>(), 424208);
const_assert_eq!(size_of::<ExecutionQueue>() % 8, 0);
const_assert_eq!(EXECUTION_QUEUE_ACCOUNT_SPACE, 424216);

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
pub const EXECUTION_QUEUE_HEAD_OFFSET: usize = 156;
pub const EXECUTION_QUEUE_COUNT_OFFSET: usize = 152;
pub const EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET: usize = 160;
pub const EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET: usize = 168;
pub const EXECUTION_QUEUE_CTM_ITEMS_OFFSET: usize = 280;
pub const EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET: usize =
    EXECUTION_QUEUE_CTM_ITEMS_OFFSET + EXECUTION_QUEUE_CTM_CAPACITY * EXECUTION_QUEUE_ITEM_SIZE;
const_assert_eq!(EXECUTION_QUEUE_HEADER_OFFSET, 152);
const_assert_eq!(EXECUTION_QUEUE_COUNT_OFFSET, 152);
const_assert_eq!(EXECUTION_QUEUE_HEAD_OFFSET, 156);
const_assert_eq!(EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 160);
const_assert_eq!(EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 168);
const_assert_eq!(EXECUTION_QUEUE_CTM_ITEMS_OFFSET, 280);
const_assert_eq!(EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET, 377112);

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
            total_count: 0,
            liquidity_head: 0,
            next_sequence_to_execute: 0,
            max_seen_sequence: 0,
            gap_observed_slot: 0,
            gap_wait_slots: 50,
            liquidity_delay_slots: 25,
            ctm_count: 0,
            liquidity_count: 0,
            reserved: [0; 16],
        };
        self.reserved = [0; 56];
        self.ctm_items = [QueueItem::default(); EXECUTION_QUEUE_CTM_CAPACITY];
        self.liquidity_items = [QueueItem::default(); EXECUTION_QUEUE_LIQUIDITY_CAPACITY];
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

    pub fn len(&self) -> usize {
        self.header.total_count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.header.total_count == 0
    }

    pub fn is_full(&self) -> bool {
        self.header.ctm_count >= EXECUTION_QUEUE_CTM_CAPACITY as u32
            && self.header.liquidity_count >= EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32
    }

    pub fn ctm_slot_index(sequence: u64) -> usize {
        (sequence as usize) % EXECUTION_QUEUE_CTM_CAPACITY
    }

    pub fn ctm_item(&self, sequence: u64) -> &QueueItem {
        &self.ctm_items[Self::ctm_slot_index(sequence)]
    }

    pub fn ctm_item_mut(&mut self, sequence: u64) -> &mut QueueItem {
        &mut self.ctm_items[Self::ctm_slot_index(sequence)]
    }

    pub fn can_enqueue_ctm_sequence(&self, sequence: u64) -> bool {
        let base = self.header.next_sequence_to_execute;
        sequence >= base && sequence < base.saturating_add(EXECUTION_QUEUE_CTM_CAPACITY as u64)
    }

    pub fn push_ctm(&mut self, item: QueueItem) -> Result<()> {
        require!(
            self.can_enqueue_ctm_sequence(item.sequence),
            MangoError::ExecutionQueueFull
        );
        let slot = self.ctm_item_mut(item.sequence);
        require!(
            slot.status != QueueItemStatus::Pending as u8 || slot.sequence != item.sequence,
            MangoError::ExecutionQueueDuplicateSequence
        );
        require!(
            slot.status != QueueItemStatus::Pending as u8,
            MangoError::ExecutionQueueFull
        );
        *slot = item;
        self.header.ctm_count = self.header.ctm_count.saturating_add(1);
        self.header.total_count = self.header.total_count.saturating_add(1);
        Ok(())
    }

    pub fn current_ctm_head(&self) -> Option<&QueueItem> {
        if self.header.ctm_count == 0 {
            return None;
        }
        let next = self.header.next_sequence_to_execute;
        let item = self.ctm_item(next);
        if item.status == QueueItemStatus::Pending as u8 && item.sequence == next {
            Some(item)
        } else {
            None
        }
    }

    pub fn clear_current_ctm_head_and_advance(&mut self) {
        let next = self.header.next_sequence_to_execute;
        let item = self.ctm_item_mut(next);
        *item = QueueItem::default();
        self.header.ctm_count = self.header.ctm_count.saturating_sub(1);
        self.header.total_count = self.header.total_count.saturating_sub(1);
        self.header.next_sequence_to_execute = self.header.next_sequence_to_execute.saturating_add(1);
        self.header.gap_observed_slot = 0;
    }

    pub fn liquidity_tail_index(&self) -> usize {
        ((self.header.liquidity_head + self.header.liquidity_count) as usize)
            % EXECUTION_QUEUE_LIQUIDITY_CAPACITY
    }

    pub fn liquidity_head_item(&self) -> Option<&QueueItem> {
        if self.header.liquidity_count == 0 {
            return None;
        }
        Some(&self.liquidity_items[self.header.liquidity_head as usize])
    }

    pub fn liquidity_head_item_mut(&mut self) -> Option<&mut QueueItem> {
        if self.header.liquidity_count == 0 {
            return None;
        }
        Some(&mut self.liquidity_items[self.header.liquidity_head as usize])
    }

    pub fn push_liquidity(&mut self, item: QueueItem) -> Result<()> {
        require!(
            self.header.liquidity_count < EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32,
            MangoError::ExecutionQueueFull
        );
        let tail = self.liquidity_tail_index();
        self.liquidity_items[tail] = item;
        self.header.liquidity_count = self.header.liquidity_count.saturating_add(1);
        self.header.total_count = self.header.total_count.saturating_add(1);
        Ok(())
    }

    pub fn pop_liquidity_head(&mut self) -> Option<QueueItem> {
        if self.header.liquidity_count == 0 {
            return None;
        }
        let head = self.header.liquidity_head as usize;
        let item = self.liquidity_items[head];
        self.liquidity_items[head] = QueueItem::default();
        self.header.liquidity_head =
            ((head + 1) % EXECUTION_QUEUE_LIQUIDITY_CAPACITY) as u32;
        self.header.liquidity_count = self.header.liquidity_count.saturating_sub(1);
        self.header.total_count = self.header.total_count.saturating_sub(1);
        Some(item)
    }

    pub fn rotate_liquidity_head_with_retry(
        &mut self,
        current_slot: u64,
        next_retry: u8,
        backoff_slots: u64,
    ) -> Result<()> {
        let mut item = self
            .pop_liquidity_head()
            .ok_or_else(|| error!(MangoError::SomeError))?;
        if item.first_failure_slot == 0 {
            item.first_failure_slot = current_slot;
        }
        item.retries = next_retry;
        item.min_execute_slot = current_slot.saturating_add(backoff_slots);
        self.push_liquidity(item)
    }
}
