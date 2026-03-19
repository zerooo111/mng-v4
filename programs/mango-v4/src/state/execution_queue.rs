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
            gap_wait_slots: 4, // M-3 fix: 4 slots covers p80-p85 SWQoS inclusion
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
        self.header.next_sequence_to_execute =
            self.header.next_sequence_to_execute.saturating_add(1);
        self.header.gap_observed_slot = 0;
    }

    /// Scan forward from next_sequence_to_execute for the first pending CTM item
    /// whose accounts_hash matches the provided hash, within a bounded window.
    /// Returns (sequence, copied item) if found.
    pub fn find_matching_ctm_item(
        &self,
        accounts_hash: &[u8; 32],
        scan_limit: u16,
    ) -> Option<(u64, QueueItem)> {
        let base = self.header.next_sequence_to_execute;
        let max = self.header.max_seen_sequence;
        let limit = (scan_limit as u64).min(EXECUTION_QUEUE_CTM_CAPACITY as u64);
        let mut seq = base;
        while seq <= max && seq < base.saturating_add(limit) {
            let item = self.ctm_item(seq);
            if item.status == QueueItemStatus::Pending as u8
                && item.sequence == seq
                && &item.accounts_hash == accounts_hash
            {
                return Some((seq, *item));
            }
            seq = seq.saturating_add(1);
        }
        None
    }

    /// Clear a specific CTM item by sequence without advancing next_sequence_to_execute.
    /// Used for out-of-order lane execution.
    pub fn clear_ctm_item_at(&mut self, sequence: u64) {
        let item = self.ctm_item_mut(sequence);
        *item = QueueItem::default();
        self.header.ctm_count = self.header.ctm_count.saturating_sub(1);
        self.header.total_count = self.header.total_count.saturating_sub(1);
        // Advance head past any now-cleared slots at the front
        while self.header.ctm_count > 0
            && self.header.max_seen_sequence >= self.header.next_sequence_to_execute
        {
            let head = self.ctm_item(self.header.next_sequence_to_execute);
            if head.status == QueueItemStatus::Pending as u8
                && head.sequence == self.header.next_sequence_to_execute
            {
                break; // real pending head found, stop
            }
            if head.status == QueueItemStatus::Empty as u8
                || head.sequence != self.header.next_sequence_to_execute
            {
                self.header.next_sequence_to_execute =
                    self.header.next_sequence_to_execute.saturating_add(1);
                self.header.gap_observed_slot = 0;
            } else {
                break;
            }
        }
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
        self.header.liquidity_head = ((head + 1) % EXECUTION_QUEUE_LIQUIDITY_CAPACITY) as u32;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_queue() -> Box<ExecutionQueue> {
        let layout = std::alloc::Layout::new::<ExecutionQueue>();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) as *mut ExecutionQueue };
        assert!(!ptr.is_null());
        let mut queue = unsafe { Box::from_raw(ptr) };
        // Avoid calling init() which creates large stack arrays.
        // The zeroed memory already has all items as Empty/zero.
        queue.group = Pubkey::new_unique();
        queue.admin = Pubkey::new_unique();
        queue.ctm_signer = Pubkey::new_unique();
        queue.bump = 1;
        queue.header.gap_wait_slots = 4;
        queue.header.liquidity_delay_slots = 25;
        queue
    }

    fn pending_ctm_item(sequence: u64, accounts_hash_seed: u8) -> QueueItem {
        let mut item = QueueItem::default();
        item.sequence = sequence;
        item.kind = QueueItemKind::CtmWrapped as u8;
        item.status = QueueItemStatus::Pending as u8;
        item.accounts_hash = [accounts_hash_seed; 32];
        item
    }

    fn pending_liquidity_item(sequence: u64, kind: QueueItemKind) -> QueueItem {
        let mut item = QueueItem::default();
        item.sequence = sequence;
        item.kind = kind as u8;
        item.status = QueueItemStatus::Pending as u8;
        item
    }

    #[test]
    fn push_ctm_rejects_duplicate_pending_sequence() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();

        let result = queue.push_ctm(pending_ctm_item(0, 2));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 1);
        assert_eq!(queue.header.total_count, 1);
    }

    #[test]
    fn clear_ctm_item_at_advances_across_front_gaps() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(2, 3)).unwrap();
        queue.header.max_seen_sequence = 2;

        queue.clear_ctm_item_at(1);
        assert_eq!(queue.header.next_sequence_to_execute, 0);
        assert_eq!(queue.header.ctm_count, 2);
        assert_eq!(queue.header.total_count, 2);

        queue.clear_ctm_item_at(0);
        assert_eq!(queue.header.next_sequence_to_execute, 2);
        assert_eq!(queue.header.ctm_count, 1);
        assert_eq!(queue.header.total_count, 1);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 2);
    }

    #[test]
    fn find_matching_ctm_item_respects_hash_and_scan_limit() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(3, 9)).unwrap();
        queue.header.max_seen_sequence = 3;

        assert!(queue.find_matching_ctm_item(&[9; 32], 2).is_none());

        let (sequence, item) = queue.find_matching_ctm_item(&[9; 32], 4).unwrap();
        assert_eq!(sequence, 3);
        assert_eq!(item.accounts_hash, [9; 32]);
    }

    #[test]
    fn rotate_liquidity_head_with_retry_preserves_fifo_order() {
        let mut queue = test_queue();
        queue
            .push_liquidity(pending_liquidity_item(7, QueueItemKind::LiquidityDeposit))
            .unwrap();
        queue
            .push_liquidity(pending_liquidity_item(8, QueueItemKind::LiquidityWithdraw))
            .unwrap();

        queue.rotate_liquidity_head_with_retry(50, 3, 7).unwrap();

        let head = queue.liquidity_head_item().unwrap();
        assert_eq!(head.sequence, 8);
        assert_eq!(head.kind, QueueItemKind::LiquidityWithdraw as u8);

        let first = queue.pop_liquidity_head().unwrap();
        let second = queue.pop_liquidity_head().unwrap();
        assert_eq!(first.sequence, 8);
        assert_eq!(second.sequence, 7);
        assert_eq!(second.retries, 3);
        assert_eq!(second.first_failure_slot, 50);
        assert_eq!(second.min_execute_slot, 57);
    }

    #[test]
    fn push_ctm_rejects_sequence_outside_enqueue_window() {
        let mut queue = test_queue();
        queue.header.next_sequence_to_execute = 11;
        queue.header.max_seen_sequence = 42;

        let result = queue.push_ctm(pending_ctm_item(10, 1));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.total_count, 0);
    }

    #[test]
    fn clear_current_ctm_head_and_advance_updates_counts_and_resets_gap_slot() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.header.max_seen_sequence = 1;
        queue.header.gap_observed_slot = 99;

        queue.clear_current_ctm_head_and_advance();

        assert_eq!(queue.header.next_sequence_to_execute, 1);
        assert_eq!(queue.header.ctm_count, 1);
        assert_eq!(queue.header.total_count, 1);
        assert_eq!(queue.header.gap_observed_slot, 0);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 1);
    }

    #[test]
    fn clear_ctm_item_at_stops_advancing_once_real_pending_head_found() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(2, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(3, 3)).unwrap();
        queue.header.max_seen_sequence = 3;

        queue.clear_ctm_item_at(0);

        assert_eq!(queue.header.next_sequence_to_execute, 2);
        assert_eq!(queue.header.ctm_count, 2);
        assert_eq!(queue.header.total_count, 2);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 2);
    }

    #[test]
    fn mixed_ctm_and_liquidity_operations_preserve_header_totals() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.header.max_seen_sequence = 1;
        queue
            .push_liquidity(pending_liquidity_item(7, QueueItemKind::LiquidityDeposit))
            .unwrap();

        assert_eq!(queue.header.total_count, queue.header.ctm_count + queue.header.liquidity_count);

        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.total_count, queue.header.ctm_count + queue.header.liquidity_count);

        queue.pop_liquidity_head().unwrap();
        assert_eq!(queue.header.total_count, queue.header.ctm_count + queue.header.liquidity_count);

        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.total_count, 0);
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.liquidity_count, 0);
    }

    // ── Phase 1A: CTM Ring Buffer Boundaries (P0) ──

    #[test]
    fn push_ctm_wraps_around_at_capacity_boundary() {
        let mut queue = test_queue();
        // Place an item at sequence 1024 which should map to physical index 0
        queue.push_ctm(pending_ctm_item(1024, 1)).unwrap();
        assert_eq!(ExecutionQueue::ctm_slot_index(1024), 0);
        assert_eq!(queue.ctm_item(1024).sequence, 1024);
        assert_eq!(queue.ctm_item(1024).status, QueueItemStatus::Pending as u8);
        assert_eq!(queue.header.ctm_count, 1);
    }

    #[test]
    fn push_ctm_rejects_sequence_at_exact_window_edge() {
        let mut queue = test_queue();
        // Window is [next, next+1024). Sequence at next+1024 should be rejected.
        queue.header.next_sequence_to_execute = 0;
        let result = queue.push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 1));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 0);
    }

    #[test]
    fn push_ctm_accepts_max_valid_sequence() {
        let mut queue = test_queue();
        // Window is [0, 1024). Sequence 1023 should be accepted.
        queue.header.next_sequence_to_execute = 0;
        queue
            .push_ctm(pending_ctm_item(
                EXECUTION_QUEUE_CTM_CAPACITY as u64 - 1,
                1,
            ))
            .unwrap();
        assert_eq!(queue.header.ctm_count, 1);
    }

    #[test]
    fn push_ctm_collision_with_cleared_slot_succeeds() {
        let mut queue = test_queue();
        // Push at seq 0, clear it, advance head, then push at seq 1024 (same physical slot)
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.header.max_seen_sequence = 0;
        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.next_sequence_to_execute, 1);

        // seq 1024 maps to physical index 0, same as seq 0 did
        queue
            .push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 2))
            .unwrap();
        assert_eq!(queue.header.ctm_count, 1);
        assert_eq!(
            queue
                .ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64)
                .sequence,
            EXECUTION_QUEUE_CTM_CAPACITY as u64
        );
    }

    #[test]
    fn push_ctm_collision_with_pending_different_sequence_errors() {
        let mut queue = test_queue();
        // Push seq 0 (physical index 0), then try seq 1024 (also physical index 0)
        // but seq 0 is still pending, so the slot is occupied
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        let result = queue.push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 2));
        // seq 1024 is outside the window [0, 1024) so it's rejected as full
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 1);
    }

    #[test]
    fn ctm_slot_index_u64_max_does_not_panic() {
        // Ensure no overflow at u64::MAX
        let index = ExecutionQueue::ctm_slot_index(u64::MAX);
        assert!(index < EXECUTION_QUEUE_CTM_CAPACITY);
    }

    #[test]
    fn full_ctm_ring_then_drain_all() {
        let mut queue = test_queue();
        // Push 1024 items
        for i in 0..EXECUTION_QUEUE_CTM_CAPACITY as u64 {
            queue.push_ctm(pending_ctm_item(i, (i % 256) as u8)).unwrap();
        }
        assert_eq!(queue.header.ctm_count, EXECUTION_QUEUE_CTM_CAPACITY as u32);
        assert_eq!(
            queue.header.total_count,
            EXECUTION_QUEUE_CTM_CAPACITY as u32
        );

        // Verify the queue rejects one more
        let result = queue.push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 1));
        assert!(result.is_err());

        // Drain all by clearing head and advancing
        queue.header.max_seen_sequence = EXECUTION_QUEUE_CTM_CAPACITY as u64 - 1;
        for _ in 0..EXECUTION_QUEUE_CTM_CAPACITY {
            queue.clear_current_ctm_head_and_advance();
        }
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.total_count, 0);
        assert!(queue.current_ctm_head().is_none());
    }

    // ── Phase 1B: Liquidity Ring Buffer Boundaries (P0) ──

    #[test]
    fn push_liquidity_fills_to_128_then_rejects() {
        let mut queue = test_queue();
        for i in 0..EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u64 {
            queue
                .push_liquidity(pending_liquidity_item(i, QueueItemKind::LiquidityDeposit))
                .unwrap();
        }
        assert_eq!(
            queue.header.liquidity_count,
            EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32
        );

        // 129th push should fail
        let result = queue.push_liquidity(pending_liquidity_item(
            EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u64,
            QueueItemKind::LiquidityDeposit,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn liquidity_wraparound_head_and_tail() {
        let mut queue = test_queue();
        // Push 3 items, pop 2 (head advances to 2), push 2 more -> test FIFO across wrap
        for i in 0..3u64 {
            queue
                .push_liquidity(pending_liquidity_item(i, QueueItemKind::LiquidityDeposit))
                .unwrap();
        }
        let first = queue.pop_liquidity_head().unwrap();
        let second = queue.pop_liquidity_head().unwrap();
        assert_eq!(first.sequence, 0);
        assert_eq!(second.sequence, 1);
        assert_eq!(queue.header.liquidity_head, 2);

        // Now push enough to wrap around
        for i in 3..EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u64 + 1 {
            queue
                .push_liquidity(pending_liquidity_item(i, QueueItemKind::LiquidityDeposit))
                .unwrap();
        }
        // Verify FIFO: head should be seq 2
        let head = queue.liquidity_head_item().unwrap();
        assert_eq!(head.sequence, 2);

        // Pop all and verify order
        let mut prev_seq = 1u64;
        while queue.header.liquidity_count > 0 {
            let item = queue.pop_liquidity_head().unwrap();
            assert!(item.sequence > prev_seq);
            prev_seq = item.sequence;
        }
    }

    #[test]
    fn pop_liquidity_head_from_empty_returns_none() {
        let mut queue = test_queue();
        assert!(queue.pop_liquidity_head().is_none());
        assert_eq!(queue.header.liquidity_count, 0);
    }

    #[test]
    fn liquidity_tail_index_wraps() {
        let mut queue = test_queue();
        queue.header.liquidity_head = 120;
        queue.header.liquidity_count = 10;
        // tail = (120 + 10) % 128 = 2
        assert_eq!(queue.liquidity_tail_index(), 2);
    }

    // ── Phase 1C: Gap/Head Advancement (P0) ──

    #[test]
    fn clear_ctm_item_at_advances_past_contiguous_empty_front() {
        let mut queue = test_queue();
        // Push seq 0, 1, 2, 5 (gap at 3, 4)
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(2, 3)).unwrap();
        queue.push_ctm(pending_ctm_item(5, 5)).unwrap();
        queue.header.max_seen_sequence = 5;

        // Clear 0, 1, 2 in order — head should advance past the gap at 3, 4 to 5
        queue.clear_ctm_item_at(0);
        // After clearing 0: head tries to advance, but 1 is pending → stops at 1
        assert_eq!(queue.header.next_sequence_to_execute, 1);

        queue.clear_ctm_item_at(1);
        // After clearing 1: head tries to advance, 2 is pending → stops at 2
        assert_eq!(queue.header.next_sequence_to_execute, 2);

        queue.clear_ctm_item_at(2);
        // After clearing 2: slot 3 is empty (gap), slot 4 is empty (gap), slot 5 is pending → stops at 5
        assert_eq!(queue.header.next_sequence_to_execute, 5);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 5);
    }

    #[test]
    fn clear_ctm_item_at_does_not_advance_past_pending() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(3, 3)).unwrap();
        queue.header.max_seen_sequence = 3;

        // Clear seq 0 — head should advance to seq 1 (pending), not further
        queue.clear_ctm_item_at(0);
        assert_eq!(queue.header.next_sequence_to_execute, 1);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 1);
    }

    #[test]
    fn clear_ctm_item_at_middle_item_no_head_advance() {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(2, 3)).unwrap();
        queue.header.max_seen_sequence = 2;

        // Clear seq 1 (middle) — head should NOT advance since seq 0 is still pending
        queue.clear_ctm_item_at(1);
        assert_eq!(queue.header.next_sequence_to_execute, 0);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 0);
        assert_eq!(queue.header.ctm_count, 2);
    }

    #[test]
    fn find_matching_ctm_item_at_max_scan_boundary() {
        let mut queue = test_queue();
        // Place items at seq 0 and seq 5
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(5, 5)).unwrap();
        queue.header.max_seen_sequence = 5;

        // Scan limit of 5 covers [0..5), so seq 5 is NOT reached
        assert!(queue.find_matching_ctm_item(&[5; 32], 5).is_none());

        // Scan limit of 6 covers [0..6), so seq 5 IS reached
        let (seq, _) = queue.find_matching_ctm_item(&[5; 32], 6).unwrap();
        assert_eq!(seq, 5);
    }

    // ── Phase 1D: Signer Rotation (P1) ──

    #[test]
    fn maybe_activate_pending_ctm_at_exact_slot() {
        let mut queue = test_queue();
        let new_signer = Pubkey::new_unique();
        queue.pending_ctm_signer = new_signer;
        queue.pending_ctm_activate_slot = 100;

        // At exact slot = 100, activation should occur
        queue.maybe_activate_pending_ctm(100);
        assert_eq!(queue.ctm_signer, new_signer);
        assert_eq!(queue.pending_ctm_signer, Pubkey::default());
        assert_eq!(queue.pending_ctm_activate_slot, 0);
    }

    #[test]
    fn maybe_activate_pending_ctm_before_slot() {
        let mut queue = test_queue();
        let old_signer = queue.ctm_signer;
        let new_signer = Pubkey::new_unique();
        queue.pending_ctm_signer = new_signer;
        queue.pending_ctm_activate_slot = 100;

        // At slot 99, activation should NOT occur
        queue.maybe_activate_pending_ctm(99);
        assert_eq!(queue.ctm_signer, old_signer);
        assert_eq!(queue.pending_ctm_signer, new_signer);
        assert_eq!(queue.pending_ctm_activate_slot, 100);
    }

    #[test]
    fn maybe_activate_pending_ctm_noop_when_no_pending() {
        let mut queue = test_queue();
        let original_signer = queue.ctm_signer;
        // No pending signer (default)
        assert_eq!(queue.pending_ctm_signer, Pubkey::default());

        queue.maybe_activate_pending_ctm(u64::MAX);
        // Signer should remain unchanged
        assert_eq!(queue.ctm_signer, original_signer);
    }

    // ── Phase 1E: Overflow/Edge (P1) ──

    #[test]
    fn counts_never_underflow_below_zero() {
        let mut queue = test_queue();
        // Counts start at 0; clearing/popping on empty should not underflow
        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.total_count, 0);

        assert!(queue.pop_liquidity_head().is_none());
        assert_eq!(queue.header.liquidity_count, 0);
        assert_eq!(queue.header.total_count, 0);
    }

    #[test]
    fn sequence_overflow_u64_max() {
        let mut queue = test_queue();
        queue.header.next_sequence_to_execute = u64::MAX - 5;
        // can_enqueue_ctm_sequence uses saturating_add, so no panic
        assert!(queue.can_enqueue_ctm_sequence(u64::MAX - 5));
        assert!(queue.can_enqueue_ctm_sequence(u64::MAX));
        // Push at u64::MAX should not panic
        queue
            .push_ctm(pending_ctm_item(u64::MAX - 5, 1))
            .unwrap();
        assert_eq!(queue.header.ctm_count, 1);
    }

    #[test]
    fn is_full_requires_both_queues_full() {
        let mut queue = test_queue();
        // Fill only CTM
        queue.header.ctm_count = EXECUTION_QUEUE_CTM_CAPACITY as u32;
        queue.header.liquidity_count = 0;
        assert!(!queue.is_full());

        // Fill only liquidity
        queue.header.ctm_count = 0;
        queue.header.liquidity_count = EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32;
        assert!(!queue.is_full());

        // Fill both
        queue.header.ctm_count = EXECUTION_QUEUE_CTM_CAPACITY as u32;
        queue.header.liquidity_count = EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32;
        assert!(queue.is_full());
    }
}
