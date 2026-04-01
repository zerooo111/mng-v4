# Internal Audit Report: Execution Queue Module — 43-Item Verification

**Date:** 2026-03-25
**Auditor:** Internal (automated + manual code review)
**Scope:** All 43 verification items from `audit-scope.md`
**Branch:** `v5b`
**Files reviewed:**
- `programs/mango-v4/src/state/execution_queue.rs` (895 lines incl. tests)
- `programs/mango-v4/src/instructions/execution_queue.rs` (1,970 lines incl. tests)
- `programs/mango-v4/src/accounts_ix/execution_queue.rs` (112 lines)
- 5 modified perp instruction files (`perp_place_order.rs`, `perp_cancel_order.rs`, `perp_cancel_order_by_client_order_id.rs`, `perp_cancel_all_orders.rs`, `perp_cancel_all_orders_by_side.rs`)
- Corresponding `accounts_ix/` files for baseline comparison

---

## Severity Definitions

| Severity | Definition |
|----------|-----------|
| PASS | Verified correct. No issues found. |
| INFO | Correct but noteworthy design decision; no action required. |
| LOW | Minor issue with limited practical impact. |
| MEDIUM | Issue with moderate impact or exploitability. |
| HIGH | Significant security concern. |
| CRITICAL | Exploitable vulnerability with direct fund risk. |

---

## File 1: `state/execution_queue.rs`

### Item 1 — Ring buffer integrity (CTM)

**Verdict: PASS**

- `can_enqueue_ctm_sequence` (line 199-202): Checks `sequence >= base && sequence < base.saturating_add(1024)`. The `saturating_add` prevents overflow when `base` is near `u64::MAX`; at that point the window shrinks (sequences near `u64::MAX` are rejected) rather than wrapping. Correct.
- `push_ctm` (line 204-222): Two-phase rejection: (1) exact duplicate detection — if physical slot has `status == Pending` AND `sequence == item.sequence`, reject as `DuplicateSequence`; (2) slot occupied — if physical slot has `status == Pending` (any sequence), reject as `Full`. This correctly handles both duplicate enqueue and physical slot collision from wraparound.
- `ctm_slot_index` (line 187-189): `(sequence as usize) % 1024`. For `u64::MAX`, this yields `u64::MAX as usize % 1024`. On 64-bit, `usize == u64`, so no truncation. The modulo is always < 1024. Unit test `ctm_slot_index_u64_max_does_not_panic` confirms this.
- Slot reuse after clear: After `clear_current_ctm_head_and_advance` resets a slot to `QueueItem::default()` (status = Empty), a new item at the same physical slot (different logical sequence) passes both push_ctm checks. Unit test `push_ctm_collision_with_cleared_slot_succeeds` confirms.

### Item 2 — Ring buffer integrity (Liquidity)

**Verdict: PASS**

- `push_liquidity` (line 321-331): Writes to `liquidity_tail_index()` which computes `(head + count) % 128`. Rejects when `count >= 128`. Correct.
- `pop_liquidity_head` (line 333-344): Reads from `head`, replaces with default, advances head as `(head + 1) % 128`, decrements count. Correct.
- `liquidity_tail_index` (line 302-305): `(head + count) % 128`. Unit test `liquidity_tail_index_wraps` confirms wraparound (head=120, count=10 → tail=2).
- FIFO ordering: Unit test `liquidity_wraparound_head_and_tail` pushes, pops, wraps, and verifies monotonic sequence order. Confirmed.

### Item 3 — Counter consistency

**Verdict: PASS**

All mutation paths maintain the invariant `total_count == ctm_count + liquidity_count`:
- `push_ctm`: increments both `ctm_count` and `total_count` (line 219-220).
- `push_liquidity`: increments both `liquidity_count` and `total_count` (line 328-329).
- `clear_current_ctm_head_and_advance`: decrements both `ctm_count` and `total_count` (line 241-242).
- `clear_ctm_item_at`: decrements both `ctm_count` and `total_count` (line 278-279).
- `pop_liquidity_head`: decrements both `liquidity_count` and `total_count` (line 341-342).
- All use `saturating_sub` to prevent underflow. Unit test `counts_never_underflow_below_zero` confirms.
- Unit test `mixed_ctm_and_liquidity_operations_preserve_header_totals` explicitly asserts the invariant after each operation.

### Item 4 — Head advancement (`clear_ctm_item_at`)

**Verdict: PASS**

The advancement loop (line 281-299) after clearing a non-head item:
- Iterates while `ctm_count > 0` AND `max_seen_sequence >= next_sequence_to_execute`.
- At each position: if the slot is Pending with matching sequence → `break` (real head found).
- If the slot is Empty or has a non-matching sequence (stale) → advance `next_sequence_to_execute` and reset `gap_observed_slot`.
- Otherwise → `break`.
- Unit tests confirm: `clear_ctm_item_at_advances_across_front_gaps`, `clear_ctm_item_at_stops_advancing_once_real_pending_head_found`, `clear_ctm_item_at_middle_item_no_head_advance`, `clear_ctm_item_at_advances_past_contiguous_empty_front`.

### Item 5 — Scan-ahead (`find_matching_ctm_item`)

**Verdict: PASS (INFO — function is defined but unused in instruction code)**

- The function (line 251-271) scans from `next_sequence_to_execute` up to `min(scan_limit, 1024)` positions, bounded by `max_seen_sequence`.
- Only returns items with `status == Pending` AND `sequence == seq` AND matching `accounts_hash`.
- Unit test `find_matching_ctm_item_respects_hash_and_scan_limit` confirms boundary behavior.
- **Importantly: `find_matching_ctm_item` is never called from `instructions/execution_queue.rs`.** Both `execute` and `execute_multi` use only `current_ctm_head()` (head-only). The function exists for potential future use but has zero security impact in the current code.

### Item 6 — CTM signer rotation

**Verdict: PASS**

- `maybe_activate_pending_ctm` (line 164-172): Activates when `pending_ctm_signer != default` AND `current_slot >= pending_ctm_activate_slot`. At exactly the activation slot, rotation occurs. Before it, nothing happens.
- After activation: `ctm_signer` is set to the pending value, `pending_ctm_signer` reset to default, `pending_ctm_activate_slot` reset to 0.
- When no pending signer (default pubkey): the first condition fails, no-op. Unit test `maybe_activate_pending_ctm_noop_when_no_pending` confirms.
- Unit tests `maybe_activate_pending_ctm_at_exact_slot` and `maybe_activate_pending_ctm_before_slot` confirm exact timing.

### Item 7 — Retry rotation (liquidity)

**Verdict: PASS**

- `rotate_liquidity_head_with_retry` (line 358-373): Pops head, updates retry/failure fields, sets `min_execute_slot = current_slot + backoff_slots`, pushes to tail.
- FIFO is preserved: the retried item goes to the back. Other pending items ahead of it (now at head) execute first.
- Unit test `rotate_liquidity_head_with_retry_preserves_fifo_order`: pushes seq 7 and 8, rotates 7 to tail, verifies 8 is now head and 7 is behind it with correct retry/failure metadata.

---

## File 2: `instructions/execution_queue.rs`

### Item 8 — Queue lifecycle (create/resize/init)

**Verdict: PASS**

- `execution_queue_create` (line 850-852): No-op body; Anchor's `init` constraint allocates the account with `EXECUTION_QUEUE_CREATE_SPACE` (8 bytes, discriminator only). Admin constraint enforced in accounts_ix.
- `execution_queue_resize` (line 854-886): Grows by `MAX_PERMITTED_DATA_INCREASE` (10,240 bytes on Solana) per call, capped at `EXECUTION_QUEUE_ACCOUNT_SPACE` (424,216 bytes). Requires ~42 resize calls. Rent lamports are transferred from payer via `system_instruction::transfer` before `realloc`. The `realloc(next_len, true)` zero-fills the new bytes. Admin constraint in accounts_ix.
- `execution_queue_init` (line 888-899): Uses `load_init()` for zero-copy initialization. Sets group, admin, ctm_signer, bump. Header defaults: `gap_wait_slots = 4`, `liquidity_delay_slots = 25`. Admin constraint in accounts_ix.

### Item 9 — `execution_queue_configure`

**Verdict: INFO**

- Admin-only (via `ExecutionQueueAdmin` constraint). Sets all four configuration fields directly from params (line 901-911).
- **No range validation on `gap_wait_slots` or `liquidity_delay_slots`.** Setting `gap_wait_slots = u64::MAX` would make gap skipping effectively impossible (queue stalls forever at any gap). Setting `liquidity_delay_slots = u64::MAX` would make liquidity items unexecutable (min_execute_slot overflows via saturating_add to `u64::MAX`, which is unreachable).
- **Impact:** Admin-only, so this is a trust assumption on the admin, not an external attack vector. Consistent with the Mango v4 trust model where admin has broad configuration authority.
- **Recommendation for external audit:** Confirm this is acceptable within the protocol's admin trust model. Consider minimum/maximum bounds if admin key compromise is a threat model concern.

### Item 10 — `execution_queue_set_ctm_pending`

**Verdict: INFO**

- Admin-only. Sets `pending_ctm_signer` and `pending_ctm_activate_slot` directly (line 913-922).
- No minimum delay enforced — `activate_at_slot = 0` or `activate_at_slot = current_slot` allows instant rotation. This was noted as M-2 in `audit_mar20.md`.
- **Impact:** Same as Item 9 — admin trust assumption.

### Item 11 — `execution_queue_drop_ctm`

**Verdict: PASS**

- Admin-only, requires `paused_execute != 0` (line 928-931). Without pause, the instruction fails.
- Validates target item: `status == Pending` AND `sequence` matches (line 934-937). Cannot drop already-executed or empty slots.
- Uses `clear_ctm_item_at` which may advance the head past empty front slots (Item 4). Correct.
- Emits `QueueItemProcessed` with `Failed` status.

### Item 12 — Payload hash verification (enqueue_ctm)

**Verdict: PASS**

- Line 979-983: `hashv(&[&payload]).to_bytes()` is compared to `envelope.payload_hash` **before** `decode_queue_payload` is called (line 984). Hash-then-decode ordering is correct — rejects tampered payloads before any parsing.

### Item 13 — Accounts hash verification (enqueue_ctm)

**Verdict: PASS**

- Line 1005-1018: Hash is computed from `dispatch_accounts` (the result of `split_dispatch_accounts` on `ctx.remaining_accounts`). The hash includes pubkey + is_signer + is_writable per account (via `AccountMeta` construction from `AccountInfo`).
- `split_dispatch_accounts` (line 168-194): Correctly strips the trailing execution queue alias (if present) and separates the program ID sentinel. The hash is computed over the dispatch accounts (excluding the program ID sentinel), matching what will be used at execute time.

### Item 14 — Ed25519 CTM signature verification

**Verdict: PASS**

- `canonical_envelope_message` (line 237-249): Domain-separated with `"mango-v4-ctm-envelope-v1"`. Binds all 7 envelope fields: group, sequence, min_execute_slot, kind, payload_hash, accounts_hash, expires_at_slot. All fixed-width fields, no ambiguity in concatenation.
- `verify_ed25519_preinstruction` (line 767-771): Calls `has_ed25519_preinstruction` which iterates `0..current_index` (line 757) — only pre-instructions, never the current or subsequent instructions. Correct.
- `ed25519_ix_matches` (line 677-749): Iterates all signatures in the Ed25519 instruction (up to 255, bounded by `u8`). Correctly parses the 14-byte offset structure. Validates signature blob existence, public key match, message size match, and message content match. Uses `data.get()` for bounds-safe access. The Solana runtime's Ed25519 precompile validates the actual cryptographic signature; this code only locates the matching entry.
- `extract_bytes` (line 666-675): Only returns data when the instruction index is either `u16::MAX` (self-referential) or matches `expected_ix_index`. This prevents an attacker from referencing data in a different instruction.

### Item 15 — Ed25519 user intent signature verification

**Verdict: PASS (with INFO on L-7)**

- `canonical_user_intent_message` (line 251-267): Domain-separated with `"mango-v4-user-intent-v1"`. Binds: group, mango_account, user_owner, kind, payload_hash, accounts_hash.
- **Does NOT bind sequence, min_execute_slot, or expires_at_slot.** This is confirmed finding L-7 from `audit_mar20.md`. A single user signature can be reused for multiple envelopes with the same kind/payload_hash/accounts_hash. This is by design — the relayer controls sequencing, and the user authorizes the *intent* (what to do), not the *scheduling* (when/in what order).
- `extract_user_owner_for_ctm_payload` (line 612-631): Loads `MangoAccountFixed` from `dispatch_accounts[1]` via `AccountLoader::try_from` (validates discriminator + program ownership). Checks `account.group == group_key`. Returns the on-chain `account.owner`, not a caller-provided value. This correctly prevents cross-group and cross-account replay.
- `verify_user_ed25519_preinstruction` (line 773-787): Tries raw 32-byte hash first, falls back to hex-UTF8 64-byte encoding. No confusion between formats (different lengths). Correct.

### Item 16 — Envelope expiry

**Verdict: PASS (with INFO)**

- Line 1020-1023: `expires_at_slot == 0 || clock.slot <= expires_at_slot`. Zero means no expiry. Checked at enqueue time only.
- **Not checked at execute time.** A queued item can execute arbitrarily late. This was noted as M-1 in `audit_mar20.md`. For perp orders, the `expiry_timestamp` field in `PerpPlaceOrderV2Payload` provides order-level expiry enforced by the perp engine at dispatch. For cancel orders, late execution is harmless (canceling a non-existent order is a no-op).
- **Impact:** Low. The envelope expiry is an ingress-time freshness check; order-level expiry handles the execution-time case.

### Item 17 — Sequence validation (enqueue_ctm)

**Verdict: PASS**

- Line 1024-1027: `envelope.sequence >= queue.header.next_sequence_to_execute`. Sequences below the current execution head are rejected. Combined with `push_ctm`'s duplicate detection (Item 1), this prevents re-enqueue of executed or pending sequences.

### Item 18 — Payload kind validation

**Verdict: PASS**

- `queue_item_kind_for_payload_variant` (line 295-305): Maps PerpPlaceOrderV2/CancelOrder/CancelOrderByClientOrderId/CancelAllOrders/CancelAllOrdersBySide → CtmWrapped. Maps LiquidityDeposit → LiquidityDeposit, LiquidityWithdraw → LiquidityWithdraw. All 7 variants covered. Correct.
- In `enqueue_ctm` (line 989-993): Requires the decoded variant maps to `CtmWrapped`. Liquidity payloads in CTM enqueue are rejected.

### Item 19 — Direct-submit fallback (enqueue_direct)

**Verdict: PASS**

- **No CTM signature:** The function (line 1080-1199) never calls `verify_ed25519_preinstruction` with the CTM signer. Only `verify_user_ed25519_preinstruction` is called (line 1157-1161), gated by `variant_uses_user_signature`. Correct.
- **Sequence assignment:** `max_seen_sequence.saturating_add(1)` (line 1165). Not user-provided. The `can_enqueue_ctm_sequence` check (line 1166-1169) ensures the assigned sequence is within the active window.
- **Forced delay:** `forced_min_execute_slot = clock.slot.saturating_add(DIRECT_SUBMIT_DELAY_SLOTS)` where `DIRECT_SUBMIT_DELAY_SLOTS = 10` (line 1172). `min_execute_slot = envelope.min_execute_slot.max(forced_min_execute_slot)` (line 1173). The `.max()` ensures the caller cannot reduce the delay below 10 slots. Correct.
- **Payload and accounts hashes:** Both are verified against the envelope (lines 1110-1114, 1126-1139), then the computed values are stored in the item (lines 1182-1183). The envelope's hashes serve as a commitment; the stored values match actuals.

### Item 20 — Direct-submit sequence collision

**Verdict: PASS**

- Direct-submit assigns `max_seen_sequence + 1`. If a relayer has already enqueued that sequence, `push_ctm` rejects with `DuplicateSequence` (the physical slot has `status == Pending` and matching `sequence`).
- If the relayer subsequently tries to enqueue the same sequence after a direct-submit has taken it, the same duplicate detection rejects the relayer's attempt.
- Solana's runtime serializes writes to the same account, so concurrent submissions are resolved by slot ordering — only one lands per slot. No on-chain race condition is possible.

### Item 21 — Liquidity enqueue: no signer

**Verdict: LOW (known, matches H-1 from audit_mar20.md)**

- `ExecutionQueueEnqueueLiquidity` (accounts_ix line 92-100) has no `Signer` constraint. Anyone can submit liquidity enqueue transactions.
- The 128-slot liquidity queue can be filled with items that will fail at execution (no actual token authority), costing only transaction fees (~0.000005 SOL each). At 128 items, the griefing cost is ~0.00064 SOL.
- Failed items are cleared after 1 retry (Items 30, 7), so the queue drains within a few execute calls.
- **Impact:** Low. Temporary queue congestion; legitimate liquidity items are delayed by at most a few seconds. The attack is economically unattractive and self-resolving.

### Item 22 — Liquidity delay enforcement

**Verdict: PASS**

- Line 1241: `min_execute_slot = clock.slot + queue.header.liquidity_delay_slots`. No user-controllable override. The `+` could theoretically overflow, but `clock.slot` is a Solana slot (~2B at current rates, far from `u64::MAX`), and `liquidity_delay_slots` is admin-controlled. Not a practical concern.

### Item 23 — Liquidity kind validation

**Verdict: PASS**

- Lines 1217-1221: Requires `kind == LiquidityDeposit || kind == LiquidityWithdraw`. CtmWrapped (0) is rejected.
- Lines 1227-1230: Cross-checks `queue_item_kind_for_payload_variant(decoded_payload.variant) == kind`, ensuring the payload variant matches the declared kind. A LiquidityDeposit kind with a PerpPlaceOrderV2 payload is rejected.

### Item 24 — Head-only strict FIFO (execute)

**Verdict: PASS**

- CTM: `current_ctm_head()` (line 1294) returns `None` unless the item at `next_sequence_to_execute` is Pending with matching sequence. Only the head is accessible.
- Accounts hash mismatch: line 1370-1372: `if provided_accounts_hash != candidate.accounts_hash { break; }` — stops execution entirely (doesn't skip to next item).
- Zero accounts hash (`[0; 32]`): treated as wildcard match (line 1370: `candidate.accounts_hash != [0; 32]` gates the comparison).
- Liquidity: `liquidity_head_item()` (line 1340) returns only the head. Head-only.
- `find_matching_ctm_item` is **never called** from the instructions file. Confirmed via grep.

### Item 25 — Gap handling

**Verdict: PASS**

- Gap detection (line 1308-1310): `ctm_count > 0 && max_seen_sequence >= next_sequence_to_execute` with `current_ctm_head() == None`. Correct — items exist but the head slot is empty.
- First observation (line 1311-1313): Sets `gap_observed_slot = clock.slot`.
- Wait check (line 1314-1318): `clock.slot >= gap_observed_slot + gap_wait_slots`. Uses `saturating_add`.
- Skip loop (line 1320-1335): Bounded by `EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE = 32`. Loop conditions: `ctm_count > 0`, `max_seen_sequence >= next_sequence_to_execute`, `current_ctm_head().is_none()`. The loop only increments `next_sequence_to_execute` (not decrementing `ctm_count`, since gap slots have no items). Terminates when: (a) 32 skips reached, (b) a real pending head is found, (c) `next_sequence_to_execute` exceeds `max_seen_sequence`.
- After skipping: `gap_observed_slot = 0`, `continue` to re-enter the main loop.
- The skip loop does NOT decrement `ctm_count` — this is correct because no actual items occupy the skipped slots.

### Item 26 — min_execute_slot enforcement

**Verdict: PASS**

- CTM: line 1295: `if clock.slot < item.min_execute_slot { blocked = true; }`. Breaks the loop.
- Liquidity: line 1345: `if clock.slot < item.min_execute_slot { blocked = true; }`. Same.
- In `execute_multi`: line 1610-1611: same check for CTM. (No liquidity processing in multi.)

### Item 27 — Payload hash at execute (no re-hash)

**Verdict: PASS**

- The payload bytes are stored on-chain at enqueue time (line 1061: `item.payload[..payload.len()].copy_from_slice(&payload)`). At execute time, the stored bytes are decoded directly (line 1375: `decode_queue_payload(&candidate.payload)`). No re-hash.
- This is safe because the on-chain account data is immutable between enqueue and execute (only the program can modify it via `load_mut()`, and no instruction modifies payload bytes after enqueue).

### Item 28 — Accounts hash verification at execute

**Verdict: PASS**

- Single-lane (line 1287): `provided_accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts))`. Computed from actual `remaining_accounts` at runtime.
- Compared to `candidate.accounts_hash` (line 1370), which was stored at enqueue time from the hash of actual accounts at that time.
- This means the execute-time accounts must exactly match the enqueue-time accounts (same keys, same signer/writable flags). An attacker cannot substitute accounts.
- Multi-lane (C-1 fix, line 1671-1674): Same — hashes computed from actual lane slices.

### Item 29 — Health-region wrapping

**Verdict: PASS**

- `queue_health_region_begin` (line 789-817): Loads MangoAccount from `dispatch_accounts[spec.account_index]` (index 1 for PerpPlaceOrderV2). Checks `!is_in_health_region()`. Computes pre-init health. Sets `is_in_health_region(true)` and stores `health_region_begin_init_health`.
- `queue_health_region_end` (line 819-848): Loads same account. Checks `is_in_health_region()`. Recomputes health. Calls `check_health_post` against stored pre-init health. Clears the flag.
- **If begin fails:** Error caught at line 1433. For CTM: retry incremented, item potentially cleared. The `is_in_health_region` flag was NOT set (error prevented it). No dangling flag.
- **If dispatch succeeds but end fails:** `dispatch_result` becomes `Err`. For health-gated CTM: code hits `return dispatch_result` (line 1511), rolling back the entire transaction. The `is_in_health_region(true)` set in begin is rolled back. No dangling flag.
- **If dispatch fails:** End is not called (line 1467: `Err(err) => Err(err)`). Same rollback path. Flag reverted.
- **All paths are safe.** The health region flag cannot be left in an inconsistent state.

### Item 30 — Retry logic

**Verdict: PASS**

- `EXECUTION_QUEUE_MAX_RETRIES = 1` (line 60).
- **Pre-dispatch check** (line 1417-1430): Health-gated items with `retries >= 1` are cleared WITHOUT dispatching. This prevents permanently-failing health checks from blocking the queue.
- **Health-region begin failure** (line 1433-1452): Retry incremented. If `retries >= 1`, item cleared immediately. Otherwise, `break`.
- **Health-gated dispatch/end failure** (line 1491-1511): `return dispatch_result` — tx rolls back. Retry counter is NOT persisted (rolled back). The cranker's offchain skip-list + `execution_queue_drop_ctm` handle this case.
- **Non-health-gated CTM failure** (line 1513-1527): Retry incremented in-place (persists, no rollback). If `retries >= 1`, cleared. Otherwise, break.
- **Liquidity failure** (line 1530-1545): If `next_retry >= 1`, popped and discarded. Otherwise, `rotate_liquidity_head_with_retry` with 1-slot backoff.
- All paths correctly handle the 1-retry budget.

### Item 31 — Dispatch routing

**Verdict: PASS**

- **Perp variants** (line 552-567): Direct function calls to `dispatch_perp_place_order_v2`, `dispatch_perp_cancel_order`, etc. These call `super::perp_*.perp_*_from_account_infos()`. No CPI overhead. Correct.
- **Liquidity variants** (line 569-609): Fall through the perp match to the CPI path. Program ID sentinel required at end of `invoke_accounts` (line 571-577). CPI instruction built with `program_id: crate::id()` (line 593). `invoke` or `invoke_signed` used depending on `variant_uses_queue_owner_signer`.
- **`variant_uses_queue_owner_signer`** (line 329-331): Returns `false` for ALL variants. This means PDA signing is never used. The `invoke_signed` path is dead code. All CPI goes through `invoke`. This is correct — liquidity deposits/withdraws don't need the execution queue PDA as a signer.

### Item 32 — Lane hash computation (C-1 fix, execute_multi)

**Verdict: PASS**

- Line 1586-1591: `lane_hashes` parameter is validated for length (`lane_hashes.len() == lane_count`) but then ignored. The comment explicitly states it is deprecated.
- Line 1671-1674: Hashes are computed on-the-fly from actual `AccountInfo` slices: `hash_accounts(&account_metas_from_infos(lane))`. This is the C-1 fix — prevents account substitution by computing hashes from runtime accounts, not caller-provided data.

### Item 33 — Lane matching (execute_multi)

**Verdict: PASS**

- Line 1668-1675: Head item's `accounts_hash` is compared against all lane hashes. Only the head is checked (no scan-ahead — `current_ctm_head()` is used, not `find_matching_ctm_item`).
- Zero accounts hash: line 1668: `if candidate.accounts_hash == [0; 32] { Some(0) }` — matches lane 0 unconditionally. This is the wildcard behavior.
- No match: line 1679: `None => break` — execution stops entirely. Strict FIFO.

### Item 34 — execute_multi does not process liquidity

**Verdict: PASS (INFO)**

- The candidate selection block in `execute_multi` (line 1607-1656) only checks `current_ctm_head()` and the gap logic. There is no `liquidity_head_item()` fallback branch (compare with `execute` line 1340).
- If the CTM queue is empty: `current_ctm_head()` returns `None`, `ctm_count == 0` prevents gap logic entry, `candidate` stays `None`, loop breaks.
- **Liquidity items must be drained via single-lane `execute`.** This is by design (documented in `audit_mar20.md` finding H-2) — liquidity items require a different account set than perp orders.

### Item 35 — `ed25519_ix_matches`

**Verdict: PASS**

- Program ID check (line 683): Rejects non-Ed25519 instructions.
- Signature count (line 687-688): `u8`, max 255. Bounded loop.
- Min length check (line 692-695): `2 + signature_count * 14`. Prevents OOB in offset parsing.
- Signature blob existence (line 703-713): Verified via `extract_bytes`. Returns `None` on OOB (`.get()`).
- Public key match (line 725): Byte comparison against `ctm_signer`.
- Message size and content match (line 729-745).
- `extract_bytes`/`extract_ix_data` (line 654-675): Only returns data for self-referential (`u16::MAX`) or matching instruction index. Cannot be tricked into reading cross-instruction data.
- Solana runtime validates the actual Ed25519 signatures. This code merely locates the matching entry. An attacker cannot include a forged Ed25519 instruction — the runtime would reject the transaction.

### Item 36 — `merge_effective_runtime_flags_for_hash`

**Verdict: PASS**

- HashMap keyed by pubkey, with `is_signer` and `is_writable` ORed across all occurrences (line 208-209).
- Output preserves `remaining_accounts` order with merged flags (line 211-224).
- OR semantics correctly match Solana's runtime flag promotion: if any reference marks an account as signer/writable, all references see it as signer/writable.
- The `or_insert` initializer (line 207) handles the first occurrence; subsequent occurrences OR via `|=`.

---

## File 3: `accounts_ix/execution_queue.rs`

### Item 37 — `ExecutionQueueCreate`

**Verdict: PASS**

- Line 8-10: `group.load()?.admin == admin.key()` — admin signer required.
- Line 13-19: PDA seeds `[b"ExecutionQueue", group.key()]` with bump. `init` constraint. Space = `EXECUTION_QUEUE_CREATE_SPACE` (8).
- Line 22-23: `payer` is `#[account(mut)]` and `Signer`.
- Line 24: `admin` is `Signer`.

### Item 38 — `ExecutionQueueResize`

**Verdict: PASS**

- Line 30-32: Same admin constraint.
- Line 35-38: Same PDA seeds, `#[account(mut)]`, no `init`. `UncheckedAccount` since the account is not yet zero-copy initialized.
- Line 41-43: Payer and admin as signers.

### Item 39 — `ExecutionQueueInit`

**Verdict: PASS**

- Line 50-52: Same admin constraint.
- Line 55-58: `zero` constraint (Anchor zero-copy init validation). Same PDA seeds.
- Line 60: `AccountLoader<'info, ExecutionQueue>` — typed zero-copy loader.
- Line 61: Admin is signer.

### Item 40 — `ExecutionQueueAdmin`

**Verdict: PASS**

- Line 66-67: `group.admin == admin.key()`.
- Line 70-73: `execution_queue` is `#[account(mut, has_one = group)]`. The `has_one = group` ensures the queue belongs to the group.
- Line 75: Admin is signer.
- Used for configure, set_ctm_pending, drop_ctm.

### Item 41 — `ExecutionQueueEnqueueCtm`

**Verdict: PASS**

- Line 80: `group` is `#[account(mut)]`.
- Line 82-86: `execution_queue` is `#[account(mut, has_one = group)]`.
- Line 87-89: `instructions` verified via `address = tx_instructions::ID`. This is the Solana instructions sysvar, required for Ed25519 pre-instruction introspection.
- **No `Signer` on any account** besides what Anchor requires for payer. Authorization is via Ed25519 pre-instructions verified in the instruction handler. This is the correct design — the CTM relayer submits on behalf of users.

### Item 42 — `ExecutionQueueEnqueueLiquidity`

**Verdict: LOW (known, matches H-1)**

- Line 94: `group` is NOT mutable (no `#[account(mut)]`). Read-only.
- Line 95-99: `execution_queue` is `#[account(mut, has_one = group)]`.
- **No `Signer` required.** No `instructions` sysvar (no signature verification).
- This matches Item 21 analysis. Griefing vector exists but is low-impact and self-resolving.

### Item 43 — `ExecutionQueueExecute`

**Verdict: PASS**

- Line 104: `group` is `#[account(mut)]`.
- Line 106-110: `execution_queue` is `#[account(mut, has_one = group)]`.
- **No signer or authority constraint.** Permissionless cranking — anyone can call execute. This is by design.
- All authorization is implicit: the accounts hash (stored at enqueue, verified at execute) ensures only the correct accounts are used. The payload was validated and signed at enqueue time. The cranker merely triggers execution of already-authorized items.

---

## Cross-Cutting Concerns

### Dispatch wrapper functions (`from_account_infos`)

**Verdict: PASS (with INFO)**

All 5 modified perp instruction files add `from_account_infos` functions that bypass Anchor's typed deserialization. Audit of all 5 functions confirms:

**Correctly replicated:**
- Account discriminator + program ownership checks via `AccountLoader::try_from`
- `IxGate` enablement check
- `has_one = group` on MangoAccount and PerpMarket
- `has_one = bids`, `has_one = asks` on PerpMarket
- `has_one = event_queue`, `has_one = oracle` (place order only)
- Owner/delegate authorization in the inner function
- `is_operational()` check parity: all 5 functions correctly mirror their Anchor counterpart's behavior

**Intentionally omitted:**
- **Signer check on the owner account.** In the Anchor path, `owner: Signer<'info>` requires the transaction signer. In the dispatch path, the owner pubkey is read but `is_signer` is not checked. This is by design: at execute time, the original user is not a transaction signer. Authorization was established at enqueue time via the Ed25519 user intent signature (Item 15), and the accounts hash (Item 28) ensures the same accounts are used at execute time.

**Security dependency:** The safety of this omission rests entirely on the accounts hash chain: (1) user signs an intent binding `accounts_hash`, (2) `enqueue_ctm`/`enqueue_direct` stores the hash, (3) `execute` computes the hash from actual runtime accounts and compares. If any link breaks, account impersonation is possible. All three links have been verified correct (Items 13, 15, 28, 32).

### `variant_uses_queue_owner_signer` always returns `false`

**Verdict: INFO**

This function (line 329-331) returns `false` for all variants, making the `invoke_signed` PDA-signing path in `dispatch_queue_payload` (line 598-605) dead code. The CPI path for liquidity always uses plain `invoke` (line 607-608). This is correct — TokenDeposit/TokenWithdraw don't need the execution queue PDA as a signer; they need the token account owner, which is validated by the underlying instruction.

---

## Summary

| Item | Component | Verdict | Notes |
|------|-----------|---------|-------|
| 1 | CTM ring buffer | PASS | Window check, duplicate detection, overflow safety all correct |
| 2 | Liquidity ring buffer | PASS | FIFO, wrap, bounds all correct |
| 3 | Counter consistency | PASS | Invariant holds across all paths; saturating arithmetic |
| 4 | Head advancement | PASS | Stops at pending items; skips gaps correctly |
| 5 | Scan-ahead | PASS | Correct but unused in instruction code |
| 6 | CTM signer rotation | PASS | Exact-slot activation, clean reset |
| 7 | Retry rotation (liquidity) | PASS | FIFO preserved, backoff correct |
| 8 | Queue lifecycle | PASS | Three-phase creation, admin-only |
| 9 | Configure | INFO | No range validation on params (admin trust) |
| 10 | Set CTM pending | INFO | No minimum rotation delay (admin trust) |
| 11 | Drop CTM | PASS | Pause required, status validated |
| 12 | Payload hash (enqueue) | PASS | Hash-before-decode ordering correct |
| 13 | Accounts hash (enqueue) | PASS | Computed from actual remaining_accounts |
| 14 | Ed25519 CTM signature | PASS | Domain-separated, all fields bound, pre-instruction only |
| 15 | Ed25519 user intent | PASS | Binds owner/account/group; L-7 (no sequence binding) is by design |
| 16 | Envelope expiry | PASS | Enqueue-time only; order-level expiry handles execution |
| 17 | Sequence validation | PASS | Rejects below execution head |
| 18 | Payload kind validation | PASS | Complete variant mapping, cross-checked |
| 19 | Direct-submit fallback | PASS | No CTM sig, user sig required, forced 10-slot delay |
| 20 | Sequence collision | PASS | push_ctm duplicate detection prevents collision |
| 21 | Liquidity no signer | LOW | Griefing vector, low impact, self-resolving (H-1) |
| 22 | Liquidity delay | PASS | Non-bypassable |
| 23 | Liquidity kind validation | PASS | CTM payloads rejected |
| 24 | FIFO (execute) | PASS | Head-only, hash mismatch stops (not skips) |
| 25 | Gap handling | PASS | Bounded skip loop (32), correct termination |
| 26 | min_execute_slot | PASS | Enforced for both CTM and liquidity |
| 27 | Payload hash (execute) | PASS | On-chain bytes are immutable; no re-hash needed |
| 28 | Accounts hash (execute) | PASS | Computed from actual runtime accounts (C-1 fix) |
| 29 | Health-region wrapping | PASS | All failure paths roll back; no dangling flag |
| 30 | Retry logic | PASS | 1-retry budget, pre-dispatch check, rollback for health-gated |
| 31 | Dispatch routing | PASS | Perp=direct call, liquidity=CPI with program ID check |
| 32 | Lane hash (C-1 fix) | PASS | Computed from actual accounts; lane_hashes param ignored |
| 33 | Lane matching | PASS | Head-only, no scan-ahead, zero=wildcard |
| 34 | Multi no liquidity | PASS | By design; liquidity via single-lane execute |
| 35 | ed25519_ix_matches | PASS | Bounded, OOB-safe, runtime validates crypto |
| 36 | Flag merging | PASS | OR semantics match Solana runtime promotion |
| 37 | Create constraints | PASS | Admin, PDA, payer all correct |
| 38 | Resize constraints | PASS | Admin, PDA, UncheckedAccount pre-init |
| 39 | Init constraints | PASS | Admin, PDA, zero constraint |
| 40 | Admin constraints | PASS | Admin + has_one group |
| 41 | EnqueueCtm constraints | PASS | Mut group, has_one, instructions sysvar, no signer (by design) |
| 42 | EnqueueLiquidity constraints | LOW | No signer (H-1 griefing vector) |
| 43 | Execute constraints | PASS | Permissionless cranking by design |

---

## Findings Summary

| Severity | Count | Items |
|----------|-------|-------|
| CRITICAL | 0 | — |
| HIGH | 0 | — |
| MEDIUM | 0 | — |
| LOW | 2 | Items 21, 42 (same issue: liquidity enqueue griefing, known H-1) |
| INFO | 4 | Items 5, 9, 10, 34 (unused code, admin trust assumptions, design decisions) |
| PASS | 39 | All remaining items |

---

## Conclusion

All 43 verification items from the audit scope have been reviewed against the source code. No new critical, high, or medium severity issues were found. The two LOW findings (Items 21/42) are the same known issue (H-1 from `audit_mar20.md`) — the liquidity enqueue instruction lacks a signer constraint, allowing low-cost queue griefing that self-resolves within seconds.

The execution queue module is well-engineered with defense-in-depth:
- **Enqueue-time authorization** via dual Ed25519 signatures (CTM + user intent)
- **Enqueue-time integrity** via payload hash and accounts hash commitment
- **Execute-time integrity** via accounts hash recomputation from runtime accounts (C-1 fix)
- **Strict FIFO enforcement** via head-only execution (H-8 fix)
- **Bounded failure handling** via 1-retry budget with pre-dispatch clearing
- **Liveness fallback** via direct-submit with forced 10-slot delay (C-4 fix)

The module's security depends on the correctness of the accounts hash chain (enqueue → store → execute verification). All three links in this chain have been verified correct.
