use crate::error::MangoError;
use crate::state::{QueueItemKind, EXECUTION_QUEUE_PAYLOAD_MAX};
use anchor_lang::prelude::*;
use static_assertions::const_assert_eq;
use std::mem::size_of;

pub const EXECUTION_QUEUE_V3_MAX_PAGE_SIZE: usize = 128;
pub const EXECUTION_QUEUE_V3_MAX_NUM_PAGES: u16 = 64;
pub const EXECUTION_QUEUE_PAGE_V3_CREATE_SPACE: usize = 8;

pub const EXECUTION_QUEUE_V3_DEFAULT_GAP_WAIT_SLOTS: u16 = 4;
pub const EXECUTION_QUEUE_V3_DEFAULT_LIQUIDITY_DELAY_SLOTS: u16 = 25;
pub const EXECUTION_QUEUE_V3_DEFAULT_MAX_COMPACTION_DISTANCE: u8 = 16;
pub const EXECUTION_QUEUE_V3_DEFAULT_MIN_EXPIRY_BUFFER_SLOTS: u8 = 4;
pub const EXECUTION_QUEUE_V3_ACCOUNT_RECIPE_LEN: usize = 24;
pub const EXECUTION_QUEUE_V3_CANONICAL_PERP_ACCOUNT_RECIPE_V1_LEN: u8 = 8;

pub const EXECUTION_QUEUE_AUTHORITY_STATE_SPACE: usize =
    8 + size_of::<ExecutionQueueAuthorityState>();
pub const EXECUTION_QUEUE_PERP_MARKET_ROOT_V3_SPACE: usize =
    8 + size_of::<PerpMarketQueueRootV3>();
pub const EXECUTION_QUEUE_LIQUIDITY_ROOT_V3_SPACE: usize =
    8 + size_of::<LiquidityQueueRootV3>();
pub const EXECUTION_QUEUE_PAGE_V3_SPACE: usize = 8 + size_of::<ExecutionQueuePageV3>();

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemStatusV3 {
    Empty = 0,
    Pending = 1,
    Executed = 2,
    Failed = 3,
    Skipped = 4,
    Compacted = 5,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemOpClassV3 {
    GenericPerpPlace = 0,
    GenericCancel = 1,
    GenericCancelByClientOrderId = 2,
    GenericCancelAll = 3,
    LiquidityDeposit = 4,
    LiquidityWithdraw = 5,
    MakerReplaceBand = 6,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemMatchKindV3 {
    None = 0,
    Market = 1,
    Ioc = 2,
    Limit = 3,
    PostOnly = 4,
    Unknown = 5,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueItemMatchSideV3 {
    Bid = 0,
    Ask = 1,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuePageStateV3 {
    Unassigned = 0,
    Active = 1,
    Retired = 2,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueAccountRecipeKindV3 {
    None = 0,
    CanonicalPerpV1 = 1,
}

pub mod queue_account_recipe_v3 {
    pub const LANE_CLASS_PERP_CANONICAL: u8 = 0;
    pub const ACCOUNT_LOCATOR_MANGO_ACCOUNT_HINT: u8 = 0;
    pub const BANK_SELECTOR_CANONICAL: u8 = 0;
    pub const ORACLE_SELECTOR_CANONICAL: u8 = 0;
    pub const FLAG_USE_MANGO_ACCOUNT_OWNER: u8 = 1 << 0;
}

pub mod queue_item_flags_v3 {
    pub const HEALTH_GATED: u8 = 1 << 0;
    pub const PRECHECK_REQUIRED: u8 = 1 << 1;
    pub const AGGRESSIVE_CANDIDATE: u8 = 1 << 2;
    pub const SUPERSEDABLE: u8 = 1 << 3;
    pub const FIXED_BAND_BUDGET: u8 = 1 << 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalPerpAccountRecipeV3 {
    pub market_index: u16,
    pub op_class: u8,
    pub lane_class: u8,
    pub account_locator: u8,
    pub bank_selector: u8,
    pub oracle_selector: u8,
    pub flags: u8,
}

impl CanonicalPerpAccountRecipeV3 {
    pub fn new(market_index: u16, op_class: u8) -> Self {
        Self {
            market_index,
            op_class,
            lane_class: queue_account_recipe_v3::LANE_CLASS_PERP_CANONICAL,
            account_locator: queue_account_recipe_v3::ACCOUNT_LOCATOR_MANGO_ACCOUNT_HINT,
            bank_selector: queue_account_recipe_v3::BANK_SELECTOR_CANONICAL,
            oracle_selector: queue_account_recipe_v3::ORACLE_SELECTOR_CANONICAL,
            flags: queue_account_recipe_v3::FLAG_USE_MANGO_ACCOUNT_OWNER,
        }
    }

    pub fn encode(self) -> [u8; EXECUTION_QUEUE_V3_ACCOUNT_RECIPE_LEN] {
        let mut bytes = [0u8; EXECUTION_QUEUE_V3_ACCOUNT_RECIPE_LEN];
        bytes[0..2].copy_from_slice(&self.market_index.to_le_bytes());
        bytes[2] = self.op_class;
        bytes[3] = self.lane_class;
        bytes[4] = self.account_locator;
        bytes[5] = self.bank_selector;
        bytes[6] = self.oracle_selector;
        bytes[7] = self.flags;
        bytes
    }

    pub fn decode(
        recipe_kind: u8,
        recipe_len: u8,
        account_recipe: &[u8; EXECUTION_QUEUE_V3_ACCOUNT_RECIPE_LEN],
    ) -> Option<Self> {
        if recipe_kind != QueueAccountRecipeKindV3::CanonicalPerpV1 as u8
            || recipe_len != EXECUTION_QUEUE_V3_CANONICAL_PERP_ACCOUNT_RECIPE_V1_LEN
        {
            return None;
        }
        Some(Self {
            market_index: u16::from_le_bytes([account_recipe[0], account_recipe[1]]),
            op_class: account_recipe[2],
            lane_class: account_recipe[3],
            account_locator: account_recipe[4],
            bank_selector: account_recipe[5],
            oracle_selector: account_recipe[6],
            flags: account_recipe[7],
        })
    }
}

#[account]
#[derive(Debug)]
pub struct ExecutionQueueAuthorityState {
    pub group: Pubkey,
    pub admin: Pubkey,
    pub ctm_signer: Pubkey,
    pub pending_ctm_signer: Pubkey,
    pub pending_ctm_activation_slot: u64,
    pub bump: u8,
    pub _padding: [u8; 7],
    pub reserved: [u8; 16],
}
const_assert_eq!(size_of::<ExecutionQueueAuthorityState>() % 8, 0);

impl ExecutionQueueAuthorityState {
    pub fn init(&mut self, group: Pubkey, admin: Pubkey, ctm_signer: Pubkey, bump: u8) {
        self.group = group;
        self.admin = admin;
        self.ctm_signer = ctm_signer;
        self.pending_ctm_signer = Pubkey::default();
        self.pending_ctm_activation_slot = 0;
        self.bump = bump;
        self._padding = [0; 7];
        self.reserved = [0; 16];
    }

    pub fn set_pending_ctm_signer(&mut self, pending_ctm_signer: Pubkey, activate_at_slot: u64) {
        self.pending_ctm_signer = pending_ctm_signer;
        self.pending_ctm_activation_slot = activate_at_slot;
    }

    pub fn maybe_activate_pending_ctm(&mut self, current_slot: u64) -> bool {
        if self.pending_ctm_signer == Pubkey::default()
            || current_slot < self.pending_ctm_activation_slot
        {
            return false;
        }
        self.ctm_signer = self.pending_ctm_signer;
        self.pending_ctm_signer = Pubkey::default();
        self.pending_ctm_activation_slot = 0;
        true
    }
}

#[account]
#[derive(Debug)]
pub struct PerpMarketQueueRootV3 {
    pub group: Pubkey,
    pub authority_state: Pubkey,
    pub market_index: u16,
    pub shard_id: u8,
    pub paused_ingress: u8,
    pub paused_execute: u8,
    pub bump: u8,
    pub max_compaction_distance: u8,
    pub min_expiry_buffer_slots: u8,
    pub _padding0: [u8; 4],
    pub next_sequence_to_execute: u64,
    pub max_seen_sequence: u64,
    pub gap_observed_slot: u64,
    pub first_failure_slot: u64,
    pub live_count: u32,
    pub gap_wait_slots: u16,
    pub page_size: u16,
    pub num_pages: u16,
    pub soft_limit: u16,
    pub recipe_version: u16,
    pub _padding1: [u8; 6],
    pub reserved: [u8; 32],
}
const_assert_eq!(size_of::<PerpMarketQueueRootV3>() % 8, 0);

impl PerpMarketQueueRootV3 {
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
        recipe_version: u16,
        gap_wait_slots: u16,
        max_compaction_distance: u8,
        min_expiry_buffer_slots: u8,
    ) {
        self.group = group;
        self.authority_state = authority_state;
        self.market_index = market_index;
        self.shard_id = shard_id;
        self.paused_ingress = 0;
        self.paused_execute = 0;
        self.bump = bump;
        self.max_compaction_distance = max_compaction_distance;
        self.min_expiry_buffer_slots = min_expiry_buffer_slots;
        self._padding0 = [0; 4];
        self.next_sequence_to_execute = 0;
        self.max_seen_sequence = 0;
        self.gap_observed_slot = 0;
        self.first_failure_slot = 0;
        self.live_count = 0;
        self.gap_wait_slots = gap_wait_slots;
        self.page_size = page_size;
        self.num_pages = num_pages;
        self.soft_limit = soft_limit;
        self.recipe_version = recipe_version;
        self._padding1 = [0; 6];
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

    pub fn validate_enqueue_sequence(&self, sequence: u64) -> Result<()> {
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

    pub fn note_enqueue(&mut self, sequence: u64) {
        self.live_count = self.live_count.saturating_add(1);
        if self.live_count == 1 || sequence > self.max_seen_sequence {
            self.max_seen_sequence = sequence;
        }
    }
}

#[account]
#[derive(Debug)]
pub struct LiquidityQueueRootV3 {
    pub group: Pubkey,
    pub authority_state: Pubkey,
    pub paused_ingress: u8,
    pub paused_execute: u8,
    pub bump: u8,
    pub _padding0: [u8; 5],
    pub next_sequence_to_execute: u64,
    pub max_seen_sequence: u64,
    pub gap_observed_slot: u64,
    pub first_failure_slot: u64,
    pub live_count: u32,
    pub liquidity_delay_slots: u16,
    pub page_size: u16,
    pub num_pages: u16,
    pub _padding1: [u8; 14],
    pub reserved: [u8; 32],
}
const_assert_eq!(size_of::<LiquidityQueueRootV3>() % 8, 0);

impl LiquidityQueueRootV3 {
    pub fn init(
        &mut self,
        group: Pubkey,
        authority_state: Pubkey,
        bump: u8,
        page_size: u16,
        num_pages: u16,
        liquidity_delay_slots: u16,
    ) {
        self.group = group;
        self.authority_state = authority_state;
        self.paused_ingress = 0;
        self.paused_execute = 0;
        self.bump = bump;
        self._padding0 = [0; 5];
        self.next_sequence_to_execute = 0;
        self.max_seen_sequence = 0;
        self.gap_observed_slot = 0;
        self.first_failure_slot = 0;
        self.live_count = 0;
        self.liquidity_delay_slots = liquidity_delay_slots;
        self.page_size = page_size;
        self.num_pages = num_pages;
        self._padding1 = [0; 14];
        self.reserved = [0; 32];
    }

    pub fn capacity(&self) -> u32 {
        self.page_size as u32 * self.num_pages as u32
    }

    pub fn next_enqueue_sequence(&self) -> u64 {
        if self.live_count == 0 {
            self.next_sequence_to_execute
        } else {
            self.max_seen_sequence.saturating_add(1)
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

    pub fn head_abs_page_no(&self) -> u64 {
        self.abs_page_no_for_sequence(self.next_sequence_to_execute)
    }

    pub fn head_page_slot(&self) -> u16 {
        self.page_slot_for_sequence(self.next_sequence_to_execute)
    }

    pub fn head_page_offset(&self) -> u16 {
        self.page_offset_for_sequence(self.next_sequence_to_execute)
    }

    pub fn validate_enqueue_sequence(&self, sequence: u64) -> Result<()> {
        require!(
            sequence >= self.next_sequence_to_execute,
            MangoError::InvalidSequenceNumber
        );
        require!(
            sequence
                < self
                    .next_sequence_to_execute
                    .saturating_add(self.capacity() as u64),
            MangoError::ExecutionQueueFull
        );
        Ok(())
    }

    pub fn note_enqueue(&mut self, sequence: u64) {
        self.live_count = self.live_count.saturating_add(1);
        if self.live_count == 1 || sequence > self.max_seen_sequence {
            self.max_seen_sequence = sequence;
        }
    }
}

#[zero_copy]
#[derive(Debug)]
pub struct QueueItemV3 {
    pub sequence: u64,
    pub min_execute_slot: u64,
    pub ingress_slot: u64,
    pub first_failure_slot: u64,
    pub expires_at_slot: u64,
    pub superseded_by: u64,
    pub match_limit_price_lots: i64,
    pub mango_account_hint: Pubkey,
    pub payload_hash: [u8; 32],
    pub accounts_hash: [u8; 32],
    pub compact_key: [u8; 16],
    pub account_recipe: [u8; EXECUTION_QUEUE_V3_ACCOUNT_RECIPE_LEN],
    pub market_index: u16,
    pub failure_code: u16,
    pub payload_len: u16,
    pub kind: u8,
    pub status: u8,
    pub retries: u8,
    pub flags: u8,
    pub op_class: u8,
    pub recipe_kind: u8,
    pub recipe_len: u8,
    pub match_kind: u8,
    pub match_side: u8,
    pub _padding0: [u8; 1],
    pub payload: [u8; EXECUTION_QUEUE_PAYLOAD_MAX],
}
const_assert_eq!(size_of::<QueueItemV3>(), 464);
const_assert_eq!(size_of::<QueueItemV3>() % 8, 0);

impl Default for QueueItemV3 {
    fn default() -> Self {
        Self {
            sequence: 0,
            min_execute_slot: 0,
            ingress_slot: 0,
            first_failure_slot: 0,
            expires_at_slot: 0,
            superseded_by: 0,
            match_limit_price_lots: 0,
            mango_account_hint: Pubkey::default(),
            payload_hash: [0; 32],
            accounts_hash: [0; 32],
            compact_key: [0; 16],
            account_recipe: [0; EXECUTION_QUEUE_V3_ACCOUNT_RECIPE_LEN],
            market_index: 0,
            failure_code: 0,
            payload_len: 0,
            kind: QueueItemKind::CtmWrapped as u8,
            status: QueueItemStatusV3::Empty as u8,
            retries: 0,
            flags: 0,
            op_class: QueueItemOpClassV3::GenericPerpPlace as u8,
            recipe_kind: 0,
            recipe_len: 0,
            match_kind: QueueItemMatchKindV3::None as u8,
            match_side: QueueItemMatchSideV3::Bid as u8,
            _padding0: [0; 1],
            payload: [0; EXECUTION_QUEUE_PAYLOAD_MAX],
        }
    }
}

#[account(zero_copy)]
#[derive(Debug)]
pub struct ExecutionQueuePageV3 {
    pub queue_root: Pubkey,
    pub assigned_abs_page_no: u64,
    pub live_count: u16,
    pub first_pending_offset: u16,
    pub page_slot: u16,
    pub page_state: u8,
    pub bump: u8,
    pub _padding0: [u8; 16],
    pub items: [QueueItemV3; EXECUTION_QUEUE_V3_MAX_PAGE_SIZE],
}
const_assert_eq!(size_of::<ExecutionQueuePageV3>(), 59456);
const_assert_eq!(size_of::<ExecutionQueuePageV3>() % 8, 0);

impl ExecutionQueuePageV3 {
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
        self.page_state = QueuePageStateV3::Active as u8;
        self.bump = bump;
        self._padding0 = [0; 16];
        for item in self.items.iter_mut() {
            *item = QueueItemV3::default();
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
            self.page_state == QueuePageStateV3::Active as u8,
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
            self.page_state == QueuePageStateV3::Active as u8,
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
            *item = QueueItemV3::default();
        }
        Ok(())
    }

    pub fn write_pending_item(
        &mut self,
        page_size: u16,
        offset: u16,
        item: QueueItemV3,
    ) -> Result<()> {
        require!(
            offset < page_size && (offset as usize) < self.items.len(),
            MangoError::ExecutionQueueV3PageOffsetOutOfRange
        );
        let existing = &self.items[offset as usize];
        require!(
            existing.status != QueueItemStatusV3::Pending as u8 || existing.sequence != item.sequence,
            MangoError::ExecutionQueueDuplicateSequence
        );
        require!(
            existing.status != QueueItemStatusV3::Pending as u8,
            MangoError::ExecutionQueueFull
        );
        self.items[offset as usize] = item;
        self.live_count = self.live_count.saturating_add(1);
        if self.live_count == 1 || offset < self.first_pending_offset {
            self.first_pending_offset = offset;
        }
        Ok(())
    }

    fn validate_offset(&self, page_size: u16, offset: u16) -> Result<()> {
        require!(
            offset < page_size && (offset as usize) < self.items.len(),
            MangoError::ExecutionQueueV3PageOffsetOutOfRange
        );
        Ok(())
    }

    pub fn clear_item(&mut self, page_size: u16, offset: u16) -> Result<()> {
        self.validate_offset(page_size, offset)?;
        self.items[offset as usize] = QueueItemV3::default();
        self.live_count = self.live_count.saturating_sub(1);
        if self.live_count == 0 {
            self.first_pending_offset = page_size;
            return Ok(());
        }

        if offset == self.first_pending_offset {
            let mut next = page_size;
            for idx in offset as usize + 1..page_size as usize {
                let item = &self.items[idx];
                if item.status == QueueItemStatusV3::Pending as u8 {
                    next = idx as u16;
                    break;
                }
            }
            self.first_pending_offset = next;
        }
        Ok(())
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

    pub fn set_retry_backoff(
        &mut self,
        page_size: u16,
        offset: u16,
        current_slot: u64,
        next_retry: u8,
        backoff_slots: u64,
    ) -> Result<()> {
        self.validate_offset(page_size, offset)?;
        let item = &mut self.items[offset as usize];
        if item.first_failure_slot == 0 {
            item.first_failure_slot = current_slot;
        }
        item.retries = next_retry;
        item.min_execute_slot = current_slot.saturating_add(backoff_slots);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_state_init_sets_expected_defaults() {
        let mut state: ExecutionQueueAuthorityState = unsafe { std::mem::zeroed() };
        let group = Pubkey::new_unique();
        let admin = Pubkey::new_unique();
        let signer = Pubkey::new_unique();

        state.init(group, admin, signer, 7);

        assert_eq!(state.group, group);
        assert_eq!(state.admin, admin);
        assert_eq!(state.ctm_signer, signer);
        assert_eq!(state.pending_ctm_signer, Pubkey::default());
        assert_eq!(state.pending_ctm_activation_slot, 0);
        assert_eq!(state.bump, 7);
    }

    #[test]
    fn authority_state_activates_pending_signer() {
        let mut state: ExecutionQueueAuthorityState = unsafe { std::mem::zeroed() };
        let signer = Pubkey::new_unique();
        let pending = Pubkey::new_unique();
        state.init(Pubkey::new_unique(), Pubkey::new_unique(), signer, 1);
        state.set_pending_ctm_signer(pending, 99);

        assert!(!state.maybe_activate_pending_ctm(98));
        assert_eq!(state.ctm_signer, signer);
        assert!(state.maybe_activate_pending_ctm(99));
        assert_eq!(state.ctm_signer, pending);
        assert_eq!(state.pending_ctm_signer, Pubkey::default());
    }

    #[test]
    fn market_root_init_sets_capacity() {
        let mut root: PerpMarketQueueRootV3 = unsafe { std::mem::zeroed() };
        root.init(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            7,
            0,
            3,
            128,
            16,
            256,
            1,
            EXECUTION_QUEUE_V3_DEFAULT_GAP_WAIT_SLOTS,
            EXECUTION_QUEUE_V3_DEFAULT_MAX_COMPACTION_DISTANCE,
            EXECUTION_QUEUE_V3_DEFAULT_MIN_EXPIRY_BUFFER_SLOTS,
        );

        assert_eq!(root.market_index, 7);
        assert_eq!(root.shard_id, 0);
        assert_eq!(root.capacity(), 2048);
        assert_eq!(root.soft_limit, 256);
        assert_eq!(root.page_slot_for_sequence(255), 1);
        assert_eq!(root.page_offset_for_sequence(255), 127);
    }

    #[test]
    fn market_root_next_enqueue_sequence_uses_head_when_empty() {
        let mut root: PerpMarketQueueRootV3 = unsafe { std::mem::zeroed() };
        root.init(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            7,
            0,
            3,
            128,
            16,
            256,
            1,
            EXECUTION_QUEUE_V3_DEFAULT_GAP_WAIT_SLOTS,
            EXECUTION_QUEUE_V3_DEFAULT_MAX_COMPACTION_DISTANCE,
            EXECUTION_QUEUE_V3_DEFAULT_MIN_EXPIRY_BUFFER_SLOTS,
        );
        root.next_sequence_to_execute = 19;
        assert_eq!(root.next_enqueue_sequence(), 19);
        root.live_count = 2;
        root.max_seen_sequence = 24;
        assert_eq!(root.next_enqueue_sequence(), 25);
    }

    #[test]
    fn market_root_soft_limit_bounds_enqueue_window() {
        let mut root: PerpMarketQueueRootV3 = unsafe { std::mem::zeroed() };
        root.init(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            7,
            0,
            3,
            128,
            16,
            256,
            1,
            EXECUTION_QUEUE_V3_DEFAULT_GAP_WAIT_SLOTS,
            EXECUTION_QUEUE_V3_DEFAULT_MAX_COMPACTION_DISTANCE,
            EXECUTION_QUEUE_V3_DEFAULT_MIN_EXPIRY_BUFFER_SLOTS,
        );
        root.next_sequence_to_execute = 10;

        assert!(root.validate_enqueue_sequence(265).is_ok());
        assert!(root.validate_enqueue_sequence(266).is_err());
    }

    #[test]
    fn liquidity_root_init_sets_capacity() {
        let mut root: LiquidityQueueRootV3 = unsafe { std::mem::zeroed() };
        root.init(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            9,
            64,
            4,
            EXECUTION_QUEUE_V3_DEFAULT_LIQUIDITY_DELAY_SLOTS,
        );

        assert_eq!(root.capacity(), 256);
        assert_eq!(
            root.liquidity_delay_slots,
            EXECUTION_QUEUE_V3_DEFAULT_LIQUIDITY_DELAY_SLOTS
        );
        assert_eq!(root.next_enqueue_sequence(), 0);
    }

    #[test]
    fn page_init_zeros_items_and_uses_page_size_as_empty_sentinel() {
        let mut page: ExecutionQueuePageV3 = unsafe { std::mem::zeroed() };
        let queue_root = Pubkey::new_unique();

        page.init(queue_root, 3, 19, 64, 2);

        assert_eq!(page.queue_root, queue_root);
        assert_eq!(page.page_slot, 3);
        assert_eq!(page.assigned_abs_page_no, 19);
        assert_eq!(page.first_pending_offset, 64);
        assert_eq!(page.page_state, QueuePageStateV3::Active as u8);
        assert!(page
            .items
            .iter()
            .all(|item| item.status == QueueItemStatusV3::Empty as u8));
    }

    #[test]
    fn page_write_pending_updates_counts_and_rejects_duplicates() {
        let mut page: ExecutionQueuePageV3 = unsafe { std::mem::zeroed() };
        let queue_root = Pubkey::new_unique();
        page.init(queue_root, 0, 0, 64, 1);

        let mut item = QueueItemV3::default();
        item.sequence = 5;
        item.status = QueueItemStatusV3::Pending as u8;

        page.write_pending_item(64, 5, item.clone()).unwrap();
        assert_eq!(page.live_count, 1);
        assert_eq!(page.first_pending_offset, 5);
        assert!(page.write_pending_item(64, 5, item).is_err());
    }

    #[test]
    fn page_prepare_for_write_target_reassigns_when_drained() {
        let mut page: ExecutionQueuePageV3 = unsafe { std::mem::zeroed() };
        let queue_root = Pubkey::new_unique();
        page.init(queue_root, 0, 0, 64, 1);

        page.prepare_for_write_target(queue_root, 0, 16, 64).unwrap();

        assert_eq!(page.assigned_abs_page_no, 16);
        assert_eq!(page.first_pending_offset, 64);
        assert_eq!(page.live_count, 0);
    }

    #[test]
    fn canonical_perp_account_recipe_round_trips() {
        let recipe = CanonicalPerpAccountRecipeV3::new(
            42,
            QueueItemOpClassV3::GenericCancelByClientOrderId as u8,
        );
        let encoded = recipe.encode();
        let decoded = CanonicalPerpAccountRecipeV3::decode(
            QueueAccountRecipeKindV3::CanonicalPerpV1 as u8,
            EXECUTION_QUEUE_V3_CANONICAL_PERP_ACCOUNT_RECIPE_V1_LEN,
            &encoded,
        )
        .unwrap();

        assert_eq!(decoded, recipe);
        assert_eq!(
            decoded.flags,
            queue_account_recipe_v3::FLAG_USE_MANGO_ACCOUNT_OWNER
        );
    }
}
