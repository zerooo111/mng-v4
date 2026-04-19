use crate::error::MangoError;
use anchor_lang::prelude::*;
use static_assertions::const_assert_eq;
use std::mem::size_of;

// Pages carry only commits (no payloads), so we can pack more items per page
// than v3. 256 × 96 = 24,576 bytes — still well under the single realloc
// ceiling and the 10 MiB per-account cap.
pub const EXECUTION_QUEUE_V4_MAX_PAGE_SIZE: usize = 256;
pub const EXECUTION_QUEUE_V4_MAX_NUM_PAGES: u16 = 64;
pub const EXECUTION_QUEUE_PAGE_V4_CREATE_SPACE: usize = 8;

pub const EXECUTION_QUEUE_V4_DEFAULT_GAP_WAIT_SLOTS: u16 = 4;

// Upper bound on commits per commit_market_batch call. The relayer will size
// each batch dynamically; this bounds the inner loop and CU usage.
pub const EXECUTION_QUEUE_V4_MAX_COMMIT_BATCH: usize = 64;
// Upper bound on reveals per reveal_execute_market call. Real limit is tx
// size; this just caps iteration.
pub const EXECUTION_QUEUE_V4_MAX_REVEAL_BATCH: usize = 32;

pub const EXECUTION_QUEUE_PERP_MARKET_COMMIT_ROOT_V4_SPACE: usize =
    8 + size_of::<PerpMarketCommitRootV4>();
pub const EXECUTION_QUEUE_COMMIT_PAGE_V4_SPACE: usize = 8 + size_of::<CommitPageV4>();

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitStatusV4 {
    Empty = 0,
    Committed = 1,
    Executed = 2,
    Failed = 3,
    Skipped = 4,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitPageStateV4 {
    Unassigned = 0,
    Active = 1,
    Retired = 2,
}

#[account]
#[derive(Debug)]
pub struct PerpMarketCommitRootV4 {
    pub group: Pubkey,
    pub authority_state: Pubkey,
    pub market_index: u16,
    pub shard_id: u8,
    pub paused_ingress: u8,
    pub paused_execute: u8,
    pub bump: u8,
    pub _padding0: [u8; 2],
    pub next_sequence_to_execute: u64,
    pub max_seen_sequence: u64,
    pub gap_observed_slot: u64,
    pub first_failure_slot: u64,
    pub live_count: u32,
    pub gap_wait_slots: u16,
    pub page_size: u16,
    pub num_pages: u16,
    pub soft_limit: u16,
    pub _padding1: [u8; 4],
    pub reserved: [u8; 32],
}
const_assert_eq!(size_of::<PerpMarketCommitRootV4>() % 8, 0);

impl PerpMarketCommitRootV4 {
    #[allow(clippy::too_many_arguments)]
    pub fn init(
        &mut self,
        group: Pubkey,
        authority_state: Pubkey,
        market_index: u16,
        shard_id: u8,
        bump: u8,
        page_size: u16,
        num_pages: u16,
        soft_limit: u16,
        gap_wait_slots: u16,
    ) {
        self.group = group;
        self.authority_state = authority_state;
        self.market_index = market_index;
        self.shard_id = shard_id;
        self.paused_ingress = 0;
        self.paused_execute = 0;
        self.bump = bump;
        self._padding0 = [0; 2];
        self.next_sequence_to_execute = 0;
        self.max_seen_sequence = 0;
        self.gap_observed_slot = 0;
        self.first_failure_slot = 0;
        self.live_count = 0;
        self.gap_wait_slots = gap_wait_slots;
        self.page_size = page_size;
        self.num_pages = num_pages;
        self.soft_limit = soft_limit;
        self._padding1 = [0; 4];
        self.reserved = [0; 32];
    }

    pub fn capacity(&self) -> u32 {
        self.page_size as u32 * self.num_pages as u32
    }

    pub fn admission_limit(&self) -> u64 {
        let capacity = self.capacity() as u64;
        if self.soft_limit == 0 {
            capacity
        } else {
            capacity.min(self.soft_limit as u64)
        }
    }

    pub fn abs_page_no_for_sequence(&self, sequence: u64) -> u64 {
        sequence / self.page_size as u64
    }

    pub fn page_slot_for_sequence(&self, sequence: u64) -> u16 {
        (self.abs_page_no_for_sequence(sequence) % self.num_pages as u64) as u16
    }

    pub fn page_offset_for_sequence(&self, sequence: u64) -> u16 {
        (sequence % self.page_size as u64) as u16
    }

    pub fn next_enqueue_sequence(&self) -> u64 {
        if self.live_count == 0 {
            self.next_sequence_to_execute
        } else {
            self.max_seen_sequence.saturating_add(1)
        }
    }

    pub fn head_abs_page_no(&self) -> u64 {
        self.abs_page_no_for_sequence(self.next_sequence_to_execute)
    }

    pub fn head_page_slot(&self) -> u16 {
        self.page_slot_for_sequence(self.next_sequence_to_execute)
    }

    pub fn head_page_offset(&self) -> u16 {
        self.page_offset_for_sequence(self.next_sequence_to_execute)
    }

    pub fn validate_commit_sequence(&self, sequence: u64) -> Result<()> {
        require!(
            sequence >= self.next_sequence_to_execute,
            MangoError::InvalidSequenceNumber
        );
        require!(
            sequence < self.next_sequence_to_execute.saturating_add(self.admission_limit()),
            MangoError::ExecutionQueueFull
        );
        Ok(())
    }

    pub fn note_commit(&mut self, sequence: u64) {
        self.live_count = self.live_count.saturating_add(1);
        if self.live_count == 1 || sequence > self.max_seen_sequence {
            self.max_seen_sequence = sequence;
        }
    }
}

#[zero_copy]
#[derive(Debug)]
pub struct CommitItemV4 {
    pub sequence: u64,
    pub min_execute_slot: u64,
    pub expires_at_slot: u64,
    pub ingress_slot: u64,
    pub first_failure_slot: u64,
    pub commit_hash: [u8; 32],
    pub status: u8,
    pub retries: u8,
    pub flags: u8,
    pub _padding0: [u8; 5],
    pub reserved: [u8; 16],
}
const_assert_eq!(size_of::<CommitItemV4>(), 96);
const_assert_eq!(size_of::<CommitItemV4>() % 8, 0);

impl Default for CommitItemV4 {
    fn default() -> Self {
        Self {
            sequence: 0,
            min_execute_slot: 0,
            expires_at_slot: 0,
            ingress_slot: 0,
            first_failure_slot: 0,
            commit_hash: [0; 32],
            status: CommitStatusV4::Empty as u8,
            retries: 0,
            flags: 0,
            _padding0: [0; 5],
            reserved: [0; 16],
        }
    }
}

#[account(zero_copy)]
#[derive(Debug)]
pub struct CommitPageV4 {
    pub queue_root: Pubkey,
    pub assigned_abs_page_no: u64,
    pub live_count: u16,
    pub first_pending_offset: u16,
    pub page_slot: u16,
    pub page_state: u8,
    pub bump: u8,
    pub _padding0: [u8; 16],
    pub items: [CommitItemV4; EXECUTION_QUEUE_V4_MAX_PAGE_SIZE],
}
const_assert_eq!(size_of::<CommitPageV4>() % 8, 0);

impl CommitPageV4 {
    pub fn init(
        &mut self,
        queue_root: Pubkey,
        page_slot: u16,
        assigned_abs_page_no: u64,
        page_size: u16,
        bump: u8,
    ) {
        self.queue_root = queue_root;
        self.assigned_abs_page_no = assigned_abs_page_no;
        self.live_count = 0;
        self.first_pending_offset = page_size;
        self.page_slot = page_slot;
        self.page_state = CommitPageStateV4::Active as u8;
        self.bump = bump;
        self._padding0 = [0; 16];
        for item in self.items.iter_mut() {
            *item = CommitItemV4::default();
        }
    }

    pub fn validate_target(
        &self,
        queue_root: Pubkey,
        page_slot: u16,
        assigned_abs_page_no: u64,
    ) -> Result<()> {
        require!(
            self.queue_root == queue_root,
            MangoError::ExecutionQueueV3PageQueueRootMismatch
        );
        require!(
            self.page_state == CommitPageStateV4::Active as u8,
            MangoError::ExecutionQueueV3PageInactive
        );
        require!(
            self.page_slot == page_slot && self.assigned_abs_page_no == assigned_abs_page_no,
            MangoError::ExecutionQueueV3AssignedPageMismatch
        );
        Ok(())
    }

    pub fn prepare_for_write_target(
        &mut self,
        queue_root: Pubkey,
        page_slot: u16,
        assigned_abs_page_no: u64,
        page_size: u16,
    ) -> Result<()> {
        require!(
            self.queue_root == queue_root,
            MangoError::ExecutionQueueV3PageQueueRootMismatch
        );
        require!(
            self.page_state == CommitPageStateV4::Active as u8,
            MangoError::ExecutionQueueV3PageInactive
        );
        require!(
            self.page_slot == page_slot,
            MangoError::ExecutionQueueV3AssignedPageMismatch
        );

        if self.assigned_abs_page_no == assigned_abs_page_no {
            return Ok(());
        }

        require!(self.live_count == 0, MangoError::ExecutionQueueFull);
        self.assigned_abs_page_no = assigned_abs_page_no;
        self.first_pending_offset = page_size;
        for item in self.items.iter_mut() {
            *item = CommitItemV4::default();
        }
        Ok(())
    }

    pub fn write_pending_item(
        &mut self,
        page_size: u16,
        offset: u16,
        item: CommitItemV4,
    ) -> Result<()> {
        require!(
            offset < page_size && (offset as usize) < self.items.len(),
            MangoError::ExecutionQueueV3PageOffsetOutOfRange
        );
        let existing = &self.items[offset as usize];
        require!(
            existing.status != CommitStatusV4::Committed as u8,
            MangoError::ExecutionQueueFull
        );
        self.items[offset as usize] = item;
        self.live_count = self.live_count.saturating_add(1);
        if self.live_count == 1 || offset < self.first_pending_offset {
            self.first_pending_offset = offset;
        }
        Ok(())
    }

    pub fn validate_offset(&self, page_size: u16, offset: u16) -> Result<()> {
        require!(
            offset < page_size && (offset as usize) < self.items.len(),
            MangoError::ExecutionQueueV3PageOffsetOutOfRange
        );
        Ok(())
    }

    pub fn clear_item(&mut self, page_size: u16, offset: u16) -> Result<()> {
        self.validate_offset(page_size, offset)?;
        self.items[offset as usize] = CommitItemV4::default();
        self.live_count = self.live_count.saturating_sub(1);
        if self.live_count == 0 {
            self.first_pending_offset = page_size;
            return Ok(());
        }

        if offset == self.first_pending_offset {
            let mut next = page_size;
            for idx in offset as usize + 1..page_size as usize {
                let item = &self.items[idx];
                if item.status == CommitStatusV4::Committed as u8 {
                    next = idx as u16;
                    break;
                }
            }
            self.first_pending_offset = next;
        }
        Ok(())
    }

    /// Admin repair path for an orphaned page when queue_root.live_count has
    /// already dropped to zero. Recomputes page-local counters from the actual
    /// committed entries instead of trusting the stored live_count.
    pub fn admin_repair_orphaned_items(
        &mut self,
        page_size: u16,
        max_to_clear: u16,
    ) -> Result<u16> {
        require!(
            page_size > 0 && (page_size as usize) <= self.items.len(),
            MangoError::ExecutionQueueV3PageOffsetOutOfRange
        );

        let mut cleared = 0u16;
        let mut remaining = 0u16;
        let mut first_pending = page_size;

        for idx in 0..page_size as usize {
            let item = &mut self.items[idx];
            if item.status != CommitStatusV4::Committed as u8 {
                continue;
            }

            if cleared < max_to_clear {
                *item = CommitItemV4::default();
                cleared = cleared.saturating_add(1);
                continue;
            }

            remaining = remaining.saturating_add(1);
            if first_pending == page_size {
                first_pending = idx as u16;
            }
        }

        self.live_count = remaining;
        self.first_pending_offset = if remaining == 0 {
            page_size
        } else {
            first_pending
        };
        Ok(cleared)
    }

    pub fn increment_retry(
        &mut self,
        page_size: u16,
        offset: u16,
        current_slot: u64,
    ) -> Result<u8> {
        self.validate_offset(page_size, offset)?;
        let item = &mut self.items[offset as usize];
        if item.first_failure_slot == 0 {
            item.first_failure_slot = current_slot;
        }
        item.retries = item.retries.saturating_add(1);
        Ok(item.retries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_item_size_is_96_bytes() {
        assert_eq!(size_of::<CommitItemV4>(), 96);
    }

    #[test]
    fn commit_root_sequence_math() {
        let mut root: PerpMarketCommitRootV4 = unsafe { std::mem::zeroed() };
        root.init(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            7,
            0,
            3,
            256,
            16,
            1024,
            EXECUTION_QUEUE_V4_DEFAULT_GAP_WAIT_SLOTS,
        );
        assert_eq!(root.capacity(), 4096);
        assert_eq!(root.page_slot_for_sequence(511), 1);
        assert_eq!(root.page_offset_for_sequence(511), 255);
        assert_eq!(root.next_enqueue_sequence(), 0);
        root.live_count = 1;
        root.max_seen_sequence = 10;
        assert_eq!(root.next_enqueue_sequence(), 11);
    }

    #[test]
    fn page_write_rejects_duplicate_committed() {
        let mut page: CommitPageV4 = unsafe { std::mem::zeroed() };
        page.init(Pubkey::new_unique(), 0, 0, 256, 1);
        let mut item = CommitItemV4::default();
        item.sequence = 5;
        item.status = CommitStatusV4::Committed as u8;
        page.write_pending_item(256, 5, item.clone()).unwrap();
        assert_eq!(page.live_count, 1);
        assert_eq!(page.first_pending_offset, 5);
        assert!(page.write_pending_item(256, 5, item).is_err());
    }

    #[test]
    fn page_clear_advances_first_pending_offset() {
        let mut page: CommitPageV4 = unsafe { std::mem::zeroed() };
        page.init(Pubkey::new_unique(), 0, 0, 256, 1);
        for offset in [5u16, 7, 9] {
            let mut item = CommitItemV4::default();
            item.sequence = offset as u64;
            item.status = CommitStatusV4::Committed as u8;
            page.write_pending_item(256, offset, item).unwrap();
        }
        assert_eq!(page.first_pending_offset, 5);
        page.clear_item(256, 5).unwrap();
        assert_eq!(page.first_pending_offset, 7);
        page.clear_item(256, 7).unwrap();
        assert_eq!(page.first_pending_offset, 9);
        page.clear_item(256, 9).unwrap();
        assert_eq!(page.live_count, 0);
        assert_eq!(page.first_pending_offset, 256);
    }

    #[test]
    fn orphaned_page_repair_recomputes_counters() {
        let mut page: CommitPageV4 = unsafe { std::mem::zeroed() };
        page.init(Pubkey::new_unique(), 1, 97, 256, 1);
        for offset in [12u16, 24, 48] {
            let mut item = CommitItemV4::default();
            item.sequence = offset as u64;
            item.status = CommitStatusV4::Committed as u8;
            page.write_pending_item(256, offset, item).unwrap();
        }

        // Simulate a stale header count that no longer matches the actual page.
        page.live_count = 9;
        let cleared = page.admin_repair_orphaned_items(256, 2).unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(page.live_count, 1);
        assert_eq!(page.first_pending_offset, 48);
        assert_eq!(page.items[12].status, CommitStatusV4::Empty as u8);
        assert_eq!(page.items[24].status, CommitStatusV4::Empty as u8);
        assert_eq!(page.items[48].status, CommitStatusV4::Committed as u8);

        let cleared_rest = page.admin_repair_orphaned_items(256, 256).unwrap();
        assert_eq!(cleared_rest, 1);
        assert_eq!(page.live_count, 0);
        assert_eq!(page.first_pending_offset, 256);
        assert_eq!(page.items[48].status, CommitStatusV4::Empty as u8);
    }
}
