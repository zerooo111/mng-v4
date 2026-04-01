/// Standalone test binary for execution queue unit tests.
///
/// The main test_all binary links against BPF objects which set GNU_STACK to 16 bytes,
/// causing immediate segfaults. This binary runs the queue state tests in a thread
/// with an explicit 8MB stack to avoid the issue.
///
/// Run with:
///   cargo +1.70.0 test -p mango-v4 --test test_execution_queue_unit --features enable-gpl
use mango_v4::state::*;

fn test_queue() -> Box<ExecutionQueue> {
    let layout = std::alloc::Layout::new::<ExecutionQueue>();
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) as *mut ExecutionQueue };
    assert!(!ptr.is_null());
    let mut queue = unsafe { Box::from_raw(ptr) };
    queue.group = anchor_lang::prelude::Pubkey::new_unique();
    queue.admin = anchor_lang::prelude::Pubkey::new_unique();
    queue.ctm_signer = anchor_lang::prelude::Pubkey::new_unique();
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

/// Run a closure on a thread with 8MB stack.
fn with_large_stack<F: FnOnce() + Send + 'static>(f: F) {
    let builder = std::thread::Builder::new().stack_size(8 * 1024 * 1024);
    let handle = builder.spawn(f).expect("failed to spawn thread");
    handle.join().expect("test thread panicked");
}

// ── Phase 1A: CTM Ring Buffer Boundaries (P0) ──

#[test]
fn push_ctm_wraps_around_at_capacity_boundary() {
    with_large_stack(|| {
        let mut queue = test_queue();
        // Advance head to 1 so seq 1024 is within window [1, 1025)
        queue.header.next_sequence_to_execute = 1;
        queue.push_ctm(pending_ctm_item(1024, 1)).unwrap();
        assert_eq!(ExecutionQueue::ctm_slot_index(1024), 0);
        assert_eq!(queue.ctm_item(1024).sequence, 1024);
        assert_eq!(queue.ctm_item(1024).status, QueueItemStatus::Pending as u8);
        assert_eq!(queue.header.ctm_count, 1);
    });
}

#[test]
fn push_ctm_rejects_sequence_at_exact_window_edge() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.header.next_sequence_to_execute = 0;
        let result = queue.push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 1));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 0);
    });
}

#[test]
fn push_ctm_accepts_max_valid_sequence() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.header.next_sequence_to_execute = 0;
        queue
            .push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64 - 1, 1))
            .unwrap();
        assert_eq!(queue.header.ctm_count, 1);
    });
}

#[test]
fn push_ctm_collision_with_cleared_slot_succeeds() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.header.max_seen_sequence = 0;
        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.next_sequence_to_execute, 1);

        queue
            .push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 2))
            .unwrap();
        assert_eq!(queue.header.ctm_count, 1);
        assert_eq!(
            queue.ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64).sequence,
            EXECUTION_QUEUE_CTM_CAPACITY as u64
        );
    });
}

#[test]
fn push_ctm_collision_with_pending_different_sequence_errors() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        let result = queue.push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 2));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 1);
    });
}

#[test]
fn ctm_slot_index_u64_max_does_not_panic() {
    let index = ExecutionQueue::ctm_slot_index(u64::MAX);
    assert!(index < EXECUTION_QUEUE_CTM_CAPACITY);
}

#[test]
fn full_ctm_ring_then_drain_all() {
    with_large_stack(|| {
        let mut queue = test_queue();
        for i in 0..EXECUTION_QUEUE_CTM_CAPACITY as u64 {
            queue
                .push_ctm(pending_ctm_item(i, (i % 256) as u8))
                .unwrap();
        }
        assert_eq!(queue.header.ctm_count, EXECUTION_QUEUE_CTM_CAPACITY as u32);
        assert_eq!(
            queue.header.total_count,
            EXECUTION_QUEUE_CTM_CAPACITY as u32
        );

        let result = queue.push_ctm(pending_ctm_item(EXECUTION_QUEUE_CTM_CAPACITY as u64, 1));
        assert!(result.is_err());

        queue.header.max_seen_sequence = EXECUTION_QUEUE_CTM_CAPACITY as u64 - 1;
        for _ in 0..EXECUTION_QUEUE_CTM_CAPACITY {
            queue.clear_current_ctm_head_and_advance();
        }
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.total_count, 0);
        assert!(queue.current_ctm_head().is_none());
    });
}

// ── Phase 1B: Liquidity Ring Buffer Boundaries (P0) ──

#[test]
fn push_liquidity_fills_to_128_then_rejects() {
    with_large_stack(|| {
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
        let result = queue.push_liquidity(pending_liquidity_item(
            EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u64,
            QueueItemKind::LiquidityDeposit,
        ));
        assert!(result.is_err());
    });
}

#[test]
fn liquidity_wraparound_head_and_tail() {
    with_large_stack(|| {
        let mut queue = test_queue();
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

        for i in 3..EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u64 + 1 {
            queue
                .push_liquidity(pending_liquidity_item(i, QueueItemKind::LiquidityDeposit))
                .unwrap();
        }
        let head = queue.liquidity_head_item().unwrap();
        assert_eq!(head.sequence, 2);

        let mut prev_seq = 1u64;
        while queue.header.liquidity_count > 0 {
            let item = queue.pop_liquidity_head().unwrap();
            assert!(item.sequence > prev_seq);
            prev_seq = item.sequence;
        }
    });
}

#[test]
fn pop_liquidity_head_from_empty_returns_none() {
    with_large_stack(|| {
        let mut queue = test_queue();
        assert!(queue.pop_liquidity_head().is_none());
        assert_eq!(queue.header.liquidity_count, 0);
    });
}

#[test]
fn liquidity_tail_index_wraps() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.header.liquidity_head = 120;
        queue.header.liquidity_count = 10;
        assert_eq!(queue.liquidity_tail_index(), 2);
    });
}

// ── Phase 1C: Gap/Head Advancement (P0) ──

#[test]
fn clear_ctm_item_at_advances_past_contiguous_empty_front() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(2, 3)).unwrap();
        queue.push_ctm(pending_ctm_item(5, 5)).unwrap();
        queue.header.max_seen_sequence = 5;

        queue.clear_ctm_item_at(0);
        assert_eq!(queue.header.next_sequence_to_execute, 1);

        queue.clear_ctm_item_at(1);
        assert_eq!(queue.header.next_sequence_to_execute, 2);

        queue.clear_ctm_item_at(2);
        assert_eq!(queue.header.next_sequence_to_execute, 5);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 5);
    });
}

#[test]
fn clear_ctm_item_at_does_not_advance_past_pending() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(3, 3)).unwrap();
        queue.header.max_seen_sequence = 3;

        queue.clear_ctm_item_at(0);
        assert_eq!(queue.header.next_sequence_to_execute, 1);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 1);
    });
}

#[test]
fn clear_ctm_item_at_middle_item_no_head_advance() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.push_ctm(pending_ctm_item(2, 3)).unwrap();
        queue.header.max_seen_sequence = 2;

        queue.clear_ctm_item_at(1);
        assert_eq!(queue.header.next_sequence_to_execute, 0);
        assert_eq!(queue.current_ctm_head().unwrap().sequence, 0);
        assert_eq!(queue.header.ctm_count, 2);
    });
}

#[test]
fn find_matching_ctm_item_at_max_scan_boundary() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(5, 5)).unwrap();
        queue.header.max_seen_sequence = 5;

        assert!(queue.find_matching_ctm_item(&[5; 32], 5).is_none());

        let (seq, _) = queue.find_matching_ctm_item(&[5; 32], 6).unwrap();
        assert_eq!(seq, 5);
    });
}

// ── Phase 1D: Signer Rotation (P1) ──

#[test]
fn maybe_activate_pending_ctm_at_exact_slot() {
    with_large_stack(|| {
        use anchor_lang::prelude::Pubkey;
        let mut queue = test_queue();
        let new_signer = Pubkey::new_unique();
        queue.pending_ctm_signer = new_signer;
        queue.pending_ctm_activate_slot = 100;

        queue.maybe_activate_pending_ctm(100);
        assert_eq!(queue.ctm_signer, new_signer);
        assert_eq!(queue.pending_ctm_signer, Pubkey::default());
        assert_eq!(queue.pending_ctm_activate_slot, 0);
    });
}

#[test]
fn maybe_activate_pending_ctm_before_slot() {
    with_large_stack(|| {
        use anchor_lang::prelude::Pubkey;
        let mut queue = test_queue();
        let old_signer = queue.ctm_signer;
        let new_signer = Pubkey::new_unique();
        queue.pending_ctm_signer = new_signer;
        queue.pending_ctm_activate_slot = 100;

        queue.maybe_activate_pending_ctm(99);
        assert_eq!(queue.ctm_signer, old_signer);
        assert_eq!(queue.pending_ctm_signer, new_signer);
        assert_eq!(queue.pending_ctm_activate_slot, 100);
    });
}

#[test]
fn maybe_activate_pending_ctm_noop_when_no_pending() {
    with_large_stack(|| {
        use anchor_lang::prelude::Pubkey;
        let mut queue = test_queue();
        let original_signer = queue.ctm_signer;
        assert_eq!(queue.pending_ctm_signer, Pubkey::default());

        queue.maybe_activate_pending_ctm(u64::MAX);
        assert_eq!(queue.ctm_signer, original_signer);
    });
}

// ── Phase 1E: Overflow/Edge (P1) ──

#[test]
fn counts_never_underflow_below_zero() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.total_count, 0);

        assert!(queue.pop_liquidity_head().is_none());
        assert_eq!(queue.header.liquidity_count, 0);
        assert_eq!(queue.header.total_count, 0);
    });
}

#[test]
fn sequence_overflow_u64_max() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.header.next_sequence_to_execute = u64::MAX - 5;
        // can_enqueue checks: seq >= base && seq < base.saturating_add(1024)
        // base = u64::MAX - 5, saturating_add(1024) = u64::MAX
        // So u64::MAX - 5 is accepted (>= base && < u64::MAX) ✓
        // But u64::MAX itself fails (u64::MAX < u64::MAX is false)
        assert!(queue.can_enqueue_ctm_sequence(u64::MAX - 5));
        assert!(queue.can_enqueue_ctm_sequence(u64::MAX - 1));
        assert!(!queue.can_enqueue_ctm_sequence(u64::MAX)); // saturating edge
                                                            // Push at u64::MAX - 5 should not panic
        queue.push_ctm(pending_ctm_item(u64::MAX - 5, 1)).unwrap();
        assert_eq!(queue.header.ctm_count, 1);
    });
}

#[test]
fn is_full_requires_both_queues_full() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.header.ctm_count = EXECUTION_QUEUE_CTM_CAPACITY as u32;
        queue.header.liquidity_count = 0;
        assert!(!queue.is_full());

        queue.header.ctm_count = 0;
        queue.header.liquidity_count = EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32;
        assert!(!queue.is_full());

        queue.header.ctm_count = EXECUTION_QUEUE_CTM_CAPACITY as u32;
        queue.header.liquidity_count = EXECUTION_QUEUE_LIQUIDITY_CAPACITY as u32;
        assert!(queue.is_full());
    });
}

// ── Existing tests ported ──

#[test]
fn push_ctm_rejects_duplicate_pending_sequence() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();

        let result = queue.push_ctm(pending_ctm_item(0, 2));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 1);
        assert_eq!(queue.header.total_count, 1);
    });
}

#[test]
fn push_ctm_rejects_sequence_outside_enqueue_window() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.header.next_sequence_to_execute = 11;
        queue.header.max_seen_sequence = 42;

        let result = queue.push_ctm(pending_ctm_item(10, 1));
        assert!(result.is_err());
        assert_eq!(queue.header.ctm_count, 0);
    });
}

#[test]
fn mixed_ctm_and_liquidity_operations_preserve_header_totals() {
    with_large_stack(|| {
        let mut queue = test_queue();
        queue.push_ctm(pending_ctm_item(0, 1)).unwrap();
        queue.push_ctm(pending_ctm_item(1, 2)).unwrap();
        queue.header.max_seen_sequence = 1;
        queue
            .push_liquidity(pending_liquidity_item(7, QueueItemKind::LiquidityDeposit))
            .unwrap();

        assert_eq!(
            queue.header.total_count,
            queue.header.ctm_count + queue.header.liquidity_count
        );

        queue.clear_current_ctm_head_and_advance();
        assert_eq!(
            queue.header.total_count,
            queue.header.ctm_count + queue.header.liquidity_count
        );

        queue.pop_liquidity_head().unwrap();
        assert_eq!(
            queue.header.total_count,
            queue.header.ctm_count + queue.header.liquidity_count
        );

        queue.clear_current_ctm_head_and_advance();
        assert_eq!(queue.header.total_count, 0);
        assert_eq!(queue.header.ctm_count, 0);
        assert_eq!(queue.header.liquidity_count, 0);
    });
}
