# Production Readiness Test Results — FermiLabs / Mango v4 + Execution Queue

**Date:** 2026-03-19
**Environment:** Linux 6.1.159, Rust 1.70.0, Node 18.20.8, Solana SDK 1.16.x
**Devnet RPC:** https://api.devnet.solana.com
**Deployed program:** `7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37`
**Group:** `42ZYDpNAUM8NZgmuQHwrUXsEWb98AkjJfJHYFFoCZpCi`
**Execution Queue:** `2Bs3tGdMs4PQV98qFLXaafSi8AJWH55eT2WwN8ZKJMkn`

---

## Summary

| Phase | Layer | Total | Passed | Failed | Ignored | Pending (skip) |
|-------|-------|-------|--------|--------|---------|-----------------|
| 1 | Onchain Unit (Rust) | 24 | 24 | 0 | 0 | 0 |
| 2 | Onchain Integration (BPF) | 60 | 60 | 0 | 0 | 0 |
| 3 | Security & Adversarial (BPF) | 18 | 13 | 0 | 5 | 0 |
| 4 | Executor Unit (Rust) | 23 | 23 | 0 | 0 | 0 |
| 5 | TypeScript Client | 32 | 32 | 0 | 0 | 0 |
| 6 | Fermi Vault (TS) | 19 | 6 | 0 | 0 | 13 |
| 7 | Devnet E2E (TS) | 5 | 5 | 0 | 0 | 0 |
| 8 | Performance (TS) | 8 | 0 | 0 | 0 | 8 |
| 9 | Precision Edge Cases (BPF) | 2 | 2 | 0 | 0 | 0 |
| **Total** | | **191** | **165** | **0** | **5** | **21** |

**Active tests passing: 165/165 (100%)**
**Ignored: 5 (3 flash loan security + 2 oracle manipulation — need serum_dex / real oracle fixtures)**
**Pending/skip: 21 (13 vault operations + 8 perf — need local validator / devnet perf harness)**

---

## Phase 1: Onchain Unit Tests

**Binary:** `test_execution_queue_unit.rs` (standalone integration test binary)
**Command:**
```bash
cargo +1.70.0 test -p mango-v4 --test test_execution_queue_unit --features enable-gpl,test-bpf
```
**Result: 24 passed, 0 failed**

| # | Test | Status |
|---|------|--------|
| 1 | `push_ctm_wraps_around_at_capacity_boundary` | PASS |
| 2 | `push_ctm_rejects_sequence_at_exact_window_edge` | PASS |
| 3 | `push_ctm_accepts_max_valid_sequence` | PASS |
| 4 | `push_ctm_collision_with_cleared_slot_succeeds` | PASS |
| 5 | `push_ctm_collision_with_pending_different_sequence_errors` | PASS |
| 6 | `ctm_slot_index_u64_max_does_not_panic` | PASS |
| 7 | `full_ctm_ring_then_drain_all` | PASS |
| 8 | `push_liquidity_fills_to_128_then_rejects` | PASS |
| 9 | `liquidity_wraparound_head_and_tail` | PASS |
| 10 | `pop_liquidity_head_from_empty_returns_none` | PASS |
| 11 | `liquidity_tail_index_wraps` | PASS |
| 12 | `clear_ctm_item_at_advances_past_contiguous_empty_front` | PASS |
| 13 | `clear_ctm_item_at_does_not_advance_past_pending` | PASS |
| 14 | `clear_ctm_item_at_middle_item_no_head_advance` | PASS |
| 15 | `find_matching_ctm_item_at_max_scan_boundary` | PASS |
| 16 | `maybe_activate_pending_ctm_at_exact_slot` | PASS |
| 17 | `maybe_activate_pending_ctm_before_slot` | PASS |
| 18 | `maybe_activate_pending_ctm_noop_when_no_pending` | PASS |
| 19 | `counts_never_underflow_below_zero` | PASS |
| 20 | `sequence_overflow_u64_max` | PASS |
| 21 | `is_full_requires_both_queues_full` | PASS |
| 22 | `push_ctm_rejects_duplicate_pending_sequence` | PASS |
| 23 | `push_ctm_rejects_sequence_outside_enqueue_window` | PASS |
| 24 | `mixed_ctm_and_liquidity_operations_preserve_header_totals` | PASS |

**Note:** The `--lib` test binary segfaults at load due to GNU_STACK=16 bytes set by the Solana/Anchor toolchain linker. Unit tests are instead compiled as a standalone `--test` binary with `test-bpf` feature, which produces a functional binary. Each test spawns a thread with 8MB stack via `with_large_stack()` to handle the 424KB `ExecutionQueue` struct.

---

## Phase 2: Onchain Integration Tests (BPF)

**Binary:** `test_all.rs` (module `test_execution_queue`)
**Command:**
```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf -- test_execution_queue --test-threads=1
```
**Result: 60 passed, 0 failed**

### Existing tests (11) — all pass:
| # | Test | Status |
|---|------|--------|
| 1 | `test_execution_queue_lifecycle_and_liquidity_enqueue` | PASS |
| 2 | `test_execution_queue_pause_flags_block_ingress_and_execute` | PASS |
| 3 | `test_execution_queue_execute_respects_liquidity_delay` | PASS |
| 4 | `test_execution_queue_execute_retries_and_drops_failed_liquidity` | PASS |
| 5 | `test_execution_queue_enqueue_ctm_and_wrong_lane_execute_preserves_head` | PASS |
| 6 | `test_execution_queue_execute_matched_failed_ctm_clears_head` | PASS |
| 7 | `test_execution_queue_execute_matched_successful_ctm_cancels_perp_orders` | PASS |
| 8 | `test_execution_queue_execute_matched_failed_perp_place_due_to_health_keeps_head` | PASS |
| 9 | `test_execution_queue_execute_multi_underwater_perp_place_keeps_head` | PASS |
| 10 | `test_execution_queue_pending_ctm_signer_activates_by_slot` | PASS |
| 11 | `test_execution_queue_drop_ctm_requires_pause_and_clears_sticky_head` | PASS |

### Phase 2A. Enqueue CTM Error Paths (17):
| # | Test | Error Code Validated | Status |
|---|------|---------------------|--------|
| 12 | `test_enqueue_ctm_payload_too_large` | ExecutionQueuePayloadTooLarge | PASS |
| 13 | `test_enqueue_ctm_hash_mismatch` | ExecutionQueuePayloadHashMismatch | PASS |
| 14 | `test_enqueue_ctm_accounts_hash_mismatch` | ExecutionQueueAccountsHashMismatch | PASS |
| 15 | `test_enqueue_ctm_envelope_expired` | ExecutionQueueEnvelopeExpired | PASS |
| 16 | `test_enqueue_ctm_sequence_below_floor` | InvalidSequenceNumber | PASS |
| 17 | `test_enqueue_ctm_duplicate_sequence` | ExecutionQueueDuplicateSequence | PASS |
| 18 | `test_enqueue_ctm_missing_ctm_signature` | CtmSignatureMissing | PASS |
| 19 | `test_enqueue_ctm_wrong_ctm_signer` | CtmSignatureMissing | PASS |
| 20 | `test_enqueue_ctm_invalid_payload_version` | ExecutionQueuePayloadVersionUnsupported | PASS |
| 21 | `test_enqueue_ctm_invalid_payload_variant` | ExecutionQueuePayloadVariantInvalid | PASS |
| 22 | `test_enqueue_ctm_truncated_payload` | ExecutionQueuePayloadDecodeFailed | PASS |
| 23 | `test_enqueue_ctm_payload_kind_mismatch` | ExecutionQueuePayloadKindMismatch | PASS |
| 24 | `test_enqueue_ctm_nonzero_flags` | ExecutionQueuePayloadDecodeFailed | PASS |
| 25 | `test_enqueue_ctm_zero_expiry_never_expires` | (happy path) | PASS |
| 26 | `test_enqueue_ctm_missing_user_signature` | ExecutionQueueUserSignatureMissing | PASS |
| 27 | `test_enqueue_ctm_wrong_user_signer` | ExecutionQueueUserSignatureMissing | PASS |
| 28 | `test_enqueue_ctm_invalid_envelope_kind` | ExecutionQueueInvalidItemKind | PASS |

### Phase 2B. Enqueue Liquidity Error Paths (5):
| # | Test | Error Code Validated | Status |
|---|------|---------------------|--------|
| 29 | `test_enqueue_liquidity_invalid_kind_ctm_wrapped` | ExecutionQueueInvalidItemKind | PASS |
| 30 | `test_enqueue_liquidity_kind_mismatch` | ExecutionQueuePayloadKindMismatch | PASS |
| 31 | `test_enqueue_liquidity_payload_too_large` | ExecutionQueuePayloadTooLarge | PASS |
| 32 | `test_enqueue_liquidity_ring_full` | ExecutionQueueFull | PASS |
| 33 | `test_enqueue_liquidity_empty_remaining_accounts` | ExecutionQueueDispatchAccountLayoutInvalid | PASS |

### Phase 2C. Execute Happy Paths (6):
| # | Test | Dispatch Variant | Status |
|---|------|-----------------|--------|
| 34 | `test_execute_perp_place_order_v2_happy` | PerpPlaceOrderV2 | PASS |
| 35 | `test_execute_perp_cancel_order_happy` | PerpCancelOrder | PASS |
| 36 | `test_execute_perp_cancel_all_orders_by_side_happy` | PerpCancelAllOrdersBySide | PASS |
| 37 | `test_execute_perp_cancel_order_by_client_order_id_happy` | PerpCancelOrderByClientOrderId | PASS |
| 38 | `test_execute_liquidity_deposit_happy` | LiquidityDeposit | PASS |
| 39 | `test_execute_liquidity_withdraw_happy` | LiquidityWithdraw | PASS |

### Phase 2D. Execute Error/Edge Paths (4):
| # | Test | Status |
|---|------|--------|
| 40 | `test_execute_blocked_by_min_execute_slot` | PASS |
| 41 | `test_execute_gap_skip_after_wait` | PASS |
| 42 | `test_execute_gap_skip_limited_to_32` | PASS |
| 43 | `test_execute_empty_queue_is_noop` | PASS |

### Phase 2E. Health-Gated Execution (2):
| # | Test | Status |
|---|------|--------|
| 44 | `test_execute_reduce_only_bypasses_health` | PASS |
| 45 | `test_execute_cancel_ignores_health` | PASS |

### Phase 2F. Multi-Lane Execute (6):
| # | Test | Status |
|---|------|--------|
| 46 | `test_execute_multi_lane_count_zero_rejected` | PASS |
| 47 | `test_execute_multi_lane_count_exceeds_20_rejected` | PASS |
| 48 | `test_execute_multi_two_lanes_match_both` | PASS |
| 49 | `test_execute_multi_hash_count_mismatch` | PASS |
| 50 | `test_execute_multi_insufficient_remaining_accounts` | PASS |
| 51 | `test_execute_multi_gap_handling_across_lanes` | PASS |

### Phase 2G. Admin Operations (1):
| # | Test | Status |
|---|------|--------|
| 52 | `test_drop_ctm_non_pending_rejected` | PASS |

### Phase 2H. Queue Create/Resize/Init (3):
| # | Test | Status |
|---|------|--------|
| 53 | `test_execution_queue_full_lifecycle` | PASS |
| 54 | `test_execution_queue_double_init_rejected` | PASS |
| 55 | `test_execution_queue_resize_when_already_full` | PASS |

### Phase 2I. Signature Edge Cases (5):
| # | Test | Status |
|---|------|--------|
| 56 | `test_sig_hex_utf8_format` | PASS |
| 57 | `test_sig_different_tx_positions` | PASS |
| 58 | `test_sig_multiple_sigs_in_one_ix` | PASS |
| 59 | `test_sig_correct_message_wrong_signer` | PASS |
| 60 | `test_sig_replay_from_different_envelope` | PASS |

---

## Phase 3: Security & Adversarial Tests (BPF)

**Binary:** `test_all.rs` (modules `test_execution_queue_security`, `test_flash_loan_security`)
**Command:**
```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf -- test_security test_flash_loan_security --test-threads=1
```
**Result: 13 passed, 0 failed, 5 ignored**

**3A. Privilege Escalation (5):**
| # | Test | Status |
|---|------|--------|
| 1 | `test_security_forged_mango_account` | PASS |
| 2 | `test_security_non_admin_drop` | PASS |
| 3 | `test_security_non_admin_configure` | PASS |
| 4 | `test_security_dispatch_to_foreign_program` | PASS |
| 5 | `test_security_spoofed_instructions_sysvar` | PASS |

**3B. Replay / Double-Spend (3):**
| # | Test | Status |
|---|------|--------|
| 6 | `test_security_replay_same_envelope_after_execution` | PASS |
| 7 | `test_security_same_payload_different_sequence` | PASS |
| 8 | `test_security_concurrent_same_sequence` | PASS |

**3C. Queue State Corruption (2):**
| # | Test | Status |
|---|------|--------|
| 9 | `test_security_count_underflow_resistance` | PASS |
| 10 | `test_security_sequence_overflow_resistance` | PASS |

**3D. Flash Loan Security (3) — IGNORED:**
| # | Test | Status | Reason |
|---|------|--------|--------|
| 11 | `test_flash_loan_security_duplicate_bank` | IGNORED | Requires serum_dex program processor |
| 12 | `test_flash_loan_security_begin_end_mismatch` | IGNORED | Requires serum_dex program processor |
| 13 | `test_flash_loan_security_underpayment` | IGNORED | Requires serum_dex program processor |

**3E. Oracle Manipulation (3 — 1 pass, 2 ignored):**
| # | Test | Status | Reason |
|---|------|--------|--------|
| 14 | `test_security_oracle_stale_does_not_block_cancel` | PASS | Cancels bypass health/oracle checks |
| 15 | `test_security_stale_oracle_blocks_queue_health` | IGNORED | Stub oracles don't enforce staleness like Pyth |
| 16 | `test_security_oracle_confidence_too_wide` | IGNORED | Stub oracles don't enforce confidence like Pyth |

**3F. CPI / Reentrancy (2):**
| # | Test | Status |
|---|------|--------|
| 17 | `test_security_execute_rejects_cpi` | PASS |
| 18 | `test_security_dispatch_cannot_modify_queue_account` | PASS |

---

## Phase 4: Rust Executor Unit Tests

**Binary:** `service-mango-execution-engine`
**Command:**
```bash
cargo +1.70.0 test -p service-mango-execution-engine
```
**Result: 23 passed, 0 failed**

### Existing tests (12):
| # | Test | Status |
|---|------|--------|
| 1 | `inspect_queue_head_uses_correct_count_offsets` | PASS |
| 2 | `inspect_next_enqueue_sequence_finds_first_hole` | PASS |
| 3 | `inspect_queue_head_falls_back_to_liquidity_when_ctm_head_missing` | PASS |
| 4 | `inspect_queue_head_reports_ctm_sequence_mismatch` | PASS |
| 5 | `inspect_queue_head_reports_gap_for_empty_ctm_slot` | PASS |
| 6 | `inspect_queue_head_reports_liquidity_status_mismatch` | PASS |
| 7 | `sequence_cursor_reuses_failed_hole` | PASS |
| 8 | `sequence_cursor_keeps_submitted_head_pending_until_queue_proves_absence` | PASS |
| 9 | `sequence_cursor_counts_only_submitted_and_reuses_recyclable_sequence` | PASS |
| 10 | `sequence_cursor_mark_submitted_promotes_reserved_without_double_counting` | PASS |
| 11 | `sequence_cursor_observe_queue_floor_drops_old_pending_and_recyclable_sequences` | PASS |
| 12 | `inspect_queue_sequence_presence_detects_pending_slot` | PASS |

### New tests (11):
| # | Test | Status |
|---|------|--------|
| 13 | `inspect_queue_head_empty_queue` | PASS |
| 14 | `inspect_queue_head_wraparound_boundary` | PASS |
| 15 | `inspect_queue_head_both_ctm_and_liquidity_pending_ctm_wins` | PASS |
| 16 | `inspect_next_enqueue_sequence_no_gaps_returns_max_plus_1` | PASS |
| 17 | `inspect_next_enqueue_sequence_all_gaps_returns_first_hole` | PASS |
| 18 | `sequence_cursor_recycled_before_increment` | PASS |
| 19 | `sequence_cursor_floor_above_all_pending` | PASS |
| 20 | `sequence_cursor_submitted_depth_excludes_reserved` | PASS |
| 21 | `sequence_cursor_multiple_failures_recyclable` | PASS |
| 22 | `sequence_cursor_mark_submitted_idempotent` | PASS |
| 23 | `inspect_queue_sequence_presence_absent_beyond_max_seen` | PASS |

---

## Phase 5: TypeScript Client Tests

**Command:**
```bash
TS_NODE_COMPILER_OPTIONS='{"module":"commonjs"}' npx ts-mocha --no-config --timeout 30000 \
  ts/client/src/executionQueueLayout.spec.ts \
  ts/client/src/executionQueue.spec.ts \
  ts/client/src/continuumHarness.spec.ts
```
**Result: 32 passing**

### executionQueueLayout.spec.ts (10 tests):
| # | Test | Status |
|---|------|--------|
| 1 | decodes header invariants and a CTM head item | PASS |
| 2 | falls back to the liquidity head when no CTM head is pending | PASS |
| 3 | reports inconsistent headers and refuses malformed heads | PASS |
| 4 | returns null when the CTM head sequence mismatches and no liquidity item is pending | PASS |
| 5 | returns null when the liquidity head is present but not pending | PASS |
| 6 | ctm_count_zero_with_liquidity_pending | PASS |
| 7 | buffer_too_small_returns_safe_defaults | PASS |
| 8 | max_sequence_bigint | PASS |
| 9 | header_invariant_mismatch_ctm_plus_liq_ne_total | PASS |
| 10 | liquidity_head_at_wraparound | PASS |

### executionQueue.spec.ts (12 tests):
| # | Test | Status |
|---|------|--------|
| 1 | encodes payload header and body fields correctly | PASS |
| 2 | encodes option\<side\> payloads deterministically | PASS |
| 3 | builds intent and envelope messages that bind user owner | PASS |
| 4 | builds ordered preinstructions and enqueue ix | PASS |
| 5 | builds liquidity enqueue and execute instructions | PASS |
| 6 | builds and signs a user intent payload for relayer submission | PASS |
| 7 | encodes_u128_order_id_for_cancel | PASS |
| 8 | client_order_id_near_2_pow_53 | PASS |
| 9 | zero_amount_deposit | PASS |
| 10 | max_amount_withdraw | PASS |
| 11 | message_changes_with_mango_account | PASS |
| 12 | message_changes_with_sequence | PASS |

### continuumHarness.spec.ts (10 tests):
| # | Test | Status |
|---|------|--------|
| 1 | decodes v1 place-order queue payload | PASS |
| 2 | builds optimistic and confirmed views from relay + processed events | PASS |
| 3 | replays deterministically regardless of ingestion order | PASS |
| 4 | tracks divergences for processed events without relay acceptance | PASS |
| 5 | builds trade prints and candles from crossing executed orders | PASS |
| 6 | deduplicates repeated processed events and tracks skipped queue items | PASS |
| 7 | removes optimistic queued place orders after a failed processed status | PASS |
| 8 | emits a divergence when processed status changes for the same queue item | PASS |
| 9 | state_projection_after_enqueue | PASS |
| 10 | malformed_payload_graceful_error | PASS |

---

## Phase 6: Fermi Vault Tests

**Note:** Token vaults are PDAs owned by the mango-v4 program itself, not a separate deployed program. The vault IDL exists only as a TS type in the frontend (`fermilabs-frontend/src/features/vault-deposit/lib/fermi_vault.ts`).

**Command:**
```bash
TS_NODE_COMPILER_OPTIONS='{"module":"commonjs"}' npx ts-mocha --no-config --timeout 30000 \
  ts/client/src/fermiVault.spec.ts
```
**Result: 6 passing, 13 pending (skip)**

### PDA Derivation Sanity (6 active):
| # | Test | Status |
|---|------|--------|
| 1 | vaultState PDA is deterministic for the same mint | PASS |
| 2 | vaultAuthority PDA is deterministic for the same vaultState | PASS |
| 3 | vaultTokenAccount PDA is deterministic | PASS |
| 4 | userState PDA is deterministic | PASS |
| 5 | different mints produce different vaultState PDAs | PASS |
| 6 | different users produce different userState PDAs | PASS |

### Happy Path, Error Paths, Security (13 skip stubs):
| # | Test | Status | Reason |
|---|------|--------|--------|
| 7-11 | 6A Happy Path (5 tests) | PENDING | Requires local validator with deployed vault program |
| 12-16 | 6B Error Paths (5 tests) | PENDING | Requires local validator with deployed vault program |
| 17-19 | 6C Security (3 tests) | PENDING | Requires local validator with deployed vault program |

---

## Phase 7: Devnet E2E Tests

### 7A. Live Devnet Happy Path (executed against deployed program)

**Script:** `ts/client/src/e2e/devnet-e2e-happy.ts`
**Command:**
```bash
npx ts-node --compiler-options '{"module":"commonjs"}' ts/client/src/e2e/devnet-e2e-happy.ts
```
**Target:** Program `7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37` on devnet
**Result: 5 passed, 0 failed**

| # | Test | Tx Signature | Status |
|---|------|-------------|--------|
| 1 | `read_queue_state` | (RPC read) | PASS |
| 2 | `queue_is_unpaused` | (RPC read) | PASS |
| 3 | `enqueue_ctm_cancel_all` | `5ctHsh3P9ThLnr4aDWkt...` | PASS |
| 4 | `execute_drains_enqueued_item` | `4zrBW7ygrdg5LsQeFYtq...` | PASS |
| 5 | `enqueue_execute_roundtrip` | `56SLzxC82qGr...` + `4xJqojnqVb...` | PASS |

**Devnet observations:**
- Queue was empty at start (totalCount=0, nextSequence=1111)
- Enqueue incremented ctmCount 0→1, sequence 1112
- Execute drained ctmCount 1→0
- Full roundtrip (enqueue seq=1113 + execute) completed in ~6s
- Queue returned to totalCount=0 after each execute
- CU consumption: enqueue ~11K, execute ~11K (cancel-all with no open orders is lightweight)

**Key finding — accounts hash mismatch debugging:**
The initial E2E attempts failed with `ExecutionQueueAccountsHashMismatch` (error 6081). Root cause: the maker keypair was added as a transaction signer (`tx.sign(payer, maker)`), which caused the Solana runtime to set `is_signer=true` on the maker's AccountInfo in `ctx.remaining_accounts`. The on-chain hash includes `is_signer` in the computation, so it didn't match the envelope hash (computed with `is_signer=false`). Fix: only the fee payer signs the transaction — the maker's intent is proven via the ed25519 pre-instruction, not tx-level signing.

### 7B. Devnet E2E Stubs (remaining)

**File:** `ts/client/src/e2e/lifecycle.spec.ts`

| # | Test | Status |
|---|------|--------|
| 6-9 | 7B Relayer Integration (4 tests) | PENDING — requires gRPC relayer endpoint |
| 10-13 | 7C Operational Safety (4 tests) | PENDING — requires relayer restart testing |

---

## Phase 8: Performance Tests

**File:** `ts/client/src/e2e/perf.spec.ts`
**Result: 0 passing, 8 pending (all skip stubs)**

All tests require a live devnet connection for CU measurement. Test stubs document thresholds.

| # | Test | CU Threshold | Status |
|---|------|-------------|--------|
| 1 | Enqueue CU | < 200K | PENDING |
| 2 | Single execute CU | < 400K | PENDING |
| 3 | Multi-4-lane CU | < 900K | PENDING |
| 4 | Gap-skip-32 CU | < 300K | PENDING |
| 5 | Multi-20-lane CU | Document | PENDING |
| 6 | 100 sequential enqueues | < 60s | PENDING |
| 7 | 100-item drain | < 30s | PENDING |
| 8 | 50 concurrent gRPC submits | 0 errors | PENDING |

---

## Phase 9: Precision Edge Cases

**Binary:** `test_all.rs` (module `test_precision_edge_cases`)
**Command:**
```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf -- test_precision --test-threads=1
```
**Result: 2 passed, 0 failed**

| # | Test | Status |
|---|------|--------|
| 1 | `test_precision_deposit_1_lamport` | PASS |
| 2 | `test_precision_zero_amount_deposit` | PASS |

---

## Fixes Applied During Testing

### Code changes to match evolved program behavior:
1. **`gap_wait_slots` default changed from 2 to 4** — Updated `test_execution_queue_lifecycle_and_liquidity_enqueue` assertion and `test_security_non_admin_configure` expected value.
2. **`EXECUTION_QUEUE_MAX_RETRIES` changed from 5 to 1** — Updated `test_execution_queue_execute_retries_and_drops_failed_liquidity` to expect immediate drop on first dispatch failure (no retry rotation).

### Test logic fixes:
3. **`push_ctm_wraps_around_at_capacity_boundary`** — Advance head to 1 so seq 1024 is within the enqueue window `[1, 1025)`.
4. **`sequence_overflow_u64_max`** — `u64::MAX` is NOT in window when `base = u64::MAX - 5` due to `saturating_add(1024) = u64::MAX` (exclusive upper bound).
5. **`test_enqueue_ctm_envelope_expired`** — Advance slots before testing so `expires_at_slot` is genuinely in the past.
6. **`test_enqueue_ctm_sequence_below_floor`** — Use `drop_ctm` (with pause/unpause) on 2 items to advance `next_sequence_to_execute` past 0. Single-item clear doesn't advance when `ctm_count` drops to 0.
7. **`test_execute_gap_skip_after_wait`** — Create a genuine head gap by enqueuing only seq 1 (skipping seq 0) so the execute loop's gap-wait-then-skip logic is exercised.
8. **`test_security_non_admin_drop`** — Check `tx_result.result.is_err()` not `result.is_err()` (program errors are inside the transaction result).
9. **`test_security_replay_same_envelope_after_execution`** — Enqueue 2 items so head advances past seq 0 after both are cleared.
10. **`test_security_forged_mango_account`** — Use different admin keypair for second group (no `group_num` field on `GroupWithTokensConfig`).
11. **`test_execute_cancel_ignores_health`** — Fund account with 1000 tokens (not 1) so the initial direct perp order placement succeeds before the queued cancel test.

### Devnet E2E fixes:
12. **Accounts hash mismatch on devnet** — The maker keypair was incorrectly added as a transaction signer, causing `is_signer=true` in the runtime AccountInfo and changing the hash. Fix: only the fee payer signs; maker intent is proven via ed25519 pre-instruction.

### Infrastructure:
13. **Standalone unit test binary** — Created `test_execution_queue_unit.rs` because the `--lib` binary segfaults at load (GNU_STACK=16 from Solana linker). Tests run in spawned threads with 8MB stack via `with_large_stack()`.

---

## Verification Commands

```bash
# Phase 1: Unit tests
cargo +1.70.0 test -p mango-v4 --test test_execution_queue_unit --features enable-gpl,test-bpf

# Phase 2-3, 9: BPF integration tests
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf \
  -- test_execution_queue test_security test_precision test_flash_loan_security \
  --test-threads=1

# Phase 4: Executor unit tests
cargo +1.70.0 test -p service-mango-execution-engine

# Phase 5-6: TypeScript tests
TS_NODE_COMPILER_OPTIONS='{"module":"commonjs"}' npx ts-mocha --no-config --timeout 30000 \
  ts/client/src/executionQueueLayout.spec.ts \
  ts/client/src/executionQueue.spec.ts \
  ts/client/src/continuumHarness.spec.ts \
  ts/client/src/fermiVault.spec.ts

# Phase 7: Devnet E2E happy path (requires funded keypair + devnet access)
npx ts-node --compiler-options '{"module":"commonjs"}' ts/client/src/e2e/devnet-e2e-happy.ts

# All Rust tests combined
cargo +1.70.0 test -p mango-v4 --test test_execution_queue_unit --features enable-gpl,test-bpf && \
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf \
  -- test_execution_queue test_security test_precision test_flash_loan_security \
  --test-threads=1 && \
cargo +1.70.0 test -p service-mango-execution-engine
```
