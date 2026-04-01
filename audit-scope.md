# Fermi DEX v1 — External Audit Scope (Differential)

**Date:** 2026-03-24
**Program:** Mango v4 (Fermi DEX fork)
**Program ID:** `4MangoMjqJ2firMokCjjGgoK8d4MXcrgL7XJaL3w6fVg`
**Branch:** `v5b`
**Scope type:** Differential — new on-chain components only

---

## Rationale for Differential Scope

This program is a fork of **Mango Markets v4** by Blockworks Foundation. The original Mango v4 on-chain program has been audited **9 times** by OtterSec across versions v0.7.0 through v0.24.0:

| Audit | File |
|-------|------|
| OtterSec Mango v0.7.0 | `audits/Audit_OtterSec_Mango_v0.7.0.pdf` |
| OtterSec Mango v0.17.0 | `audits/Audit_OtterSec_Mango_v0.17.0.pdf` |
| OtterSec Mango v0.18.0 | `audits/Audit_OtterSec_Mango_v0.18.0.pdf` |
| OtterSec Mango v0.19.0 | `audits/Audit_OtterSec_Mango_v0.19.0.pdf` |
| OtterSec Mango v0.20.0 | `audits/Audit_OtterSec_Mango_v0.20.0.pdf` |
| OtterSec Mango v0.21.0 | `audits/Audit_OtterSec_Mango_v0.21.0.pdf` |
| OtterSec Mango v0.22.0 | `audits/Audit_OtterSec_Mango_v0.22.0.pdf` |
| OtterSec Mango v0.23.0 | `audits/Audit_OtterSec_Mango_v0.23.0.pdf` |
| OtterSec Mango v0.24.0 | `audits/Audit_OtterSec_Mango_v0.24.0.pdf` |

An internal security review (`audit_mar20.md`, 2026-03-20) covers the full codebase including the new execution queue. Its findings (C-1 through C-5, H-1 through H-12, M-1 through M-16, L-1 through L-13) have been addressed in the current `v5b` branch.

**The Fermi DEX fork does not modify any previously-audited Mango v4 logic.** All changes are additive — a single new module (Execution Queue) plus thin dispatch wrappers on 5 existing instructions. The core Mango v4 systems (banks, token operations, health engine, liquidation, perp markets, flash loans, oracles, serum/openbook integration, margin trading, governance) remain unchanged from their last audited state.

Therefore, an external audit of the 3 new files listed below, **combined with the existing OtterSec audit reports**, provides complete coverage of all on-chain program logic.

---

## In-Scope Files (3 files, ~2,500 LOC)

### File 1: `programs/mango-v4/src/state/execution_queue.rs` (~375 LOC)

**Purpose:** Defines all on-chain state for the execution queue system.

**Structures to verify:**
- `ExecutionQueue` (424,208 bytes, zero-copy) — top-level account with group, admin, CTM signer, pending signer rotation, pause flags, header, and two ring buffers
- `ExecutionQueueHeader` (72 bytes) — counters, sequence pointers, gap tracking, configuration
- `QueueItem` (368 bytes, 8-byte aligned) — individual queue entry: sequence, min_execute_slot, kind, status, retries, payload hash, accounts hash, payload bytes
- `QueueItemKind` enum — CtmWrapped (0), LiquidityDeposit (1), LiquidityWithdraw (2)
- `QueueItemStatus` enum — Empty (0), Pending (1), Executed (2), Failed (3), Skipped (4)

**Assumptions to verify:**

1. **Ring buffer integrity (CTM):** The CTM queue uses sequence-indexed slots (`sequence % 1024`). Verify:
   - `push_ctm` correctly rejects sequences outside the window `[next_sequence_to_execute, next_sequence_to_execute + 1024)`
   - `push_ctm` correctly detects duplicate pending sequences at the same physical slot
   - `push_ctm` allows reuse of a physical slot after the previous item was cleared (different logical sequence)
   - No integer overflow in `ctm_slot_index` for values up to `u64::MAX`
   - `can_enqueue_ctm_sequence` uses `saturating_add` to prevent overflow at the upper boundary

2. **Ring buffer integrity (Liquidity):** The liquidity queue uses a circular FIFO buffer with explicit head pointer. Verify:
   - `push_liquidity` writes to `(head + count) % 128` and rejects when count reaches 128
   - `pop_liquidity_head` advances head with modular wrap and decrements count
   - `liquidity_tail_index` correctly computes the wrap-around tail position
   - FIFO ordering is preserved across head/tail wrap boundaries

3. **Counter consistency:** Verify that `total_count`, `ctm_count`, and `liquidity_count` remain consistent across all mutation paths:
   - `push_ctm`, `push_liquidity` increment both specific and total counts
   - `clear_current_ctm_head_and_advance`, `clear_ctm_item_at`, `pop_liquidity_head` decrement both
   - `saturating_sub` prevents underflow when clearing on empty
   - Invariant: `total_count == ctm_count + liquidity_count` always holds

4. **Head advancement (`clear_ctm_item_at`):** When an out-of-order item is cleared, the function advances `next_sequence_to_execute` past contiguous empty/stale slots at the front. Verify:
   - Advancement stops at the first slot containing a pending item with matching sequence
   - Advancement does not skip past pending items
   - Advancement correctly handles a mix of Empty status and stale (wrong sequence) slots
   - `gap_observed_slot` is reset to 0 during advancement

5. **Scan-ahead (`find_matching_ctm_item`):** Used by multi-lane execute to find items matching a given accounts hash. Verify:
   - Scan respects `scan_limit` boundary (does not read beyond)
   - Scan respects `max_seen_sequence` upper bound
   - Only returns items with `status == Pending` and `sequence == seq` (not stale)

6. **CTM signer rotation:** Verify:
   - `maybe_activate_pending_ctm` activates at exactly `pending_ctm_activate_slot` (not before)
   - After activation, `pending_ctm_signer` is reset to `Pubkey::default()` and `pending_ctm_activate_slot` to 0
   - No activation occurs when `pending_ctm_signer == Pubkey::default()`

7. **Retry rotation (liquidity):** `rotate_liquidity_head_with_retry` pops the head, updates retry count and min_execute_slot with backoff, then re-pushes to tail. Verify FIFO is preserved and the retried item does not jump ahead of other pending items.

---

### File 2: `programs/mango-v4/src/instructions/execution_queue.rs` (~1,970 LOC)

**Purpose:** All instruction handlers for the execution queue: lifecycle, enqueue, execute, and dispatch.

**Instructions to verify:**

#### Lifecycle (admin-only)

8. **`execution_queue_create` / `execution_queue_resize` / `execution_queue_init`:** Three-phase account creation (allocate with small space, incrementally realloc up to 424,216 bytes, then zero-copy init). Verify:
   - `resize` respects `MAX_PERMITTED_DATA_INCREASE` per call
   - `resize` transfers correct rent-exempt lamports from payer
   - `init` uses `load_init()` on the zero-copy account
   - Only group admin can call these (enforced via account constraints)

9. **`execution_queue_configure`:** Sets `gap_wait_slots`, `liquidity_delay_slots`, pause flags. Verify admin-only access. No validation on parameter ranges — auditor should assess whether unbounded values (e.g., `gap_wait_slots = u64::MAX`) create a denial-of-service vector.

10. **`execution_queue_set_ctm_pending`:** Schedules a CTM signer rotation. Verify admin-only. Note: no minimum delay is enforced on `activate_at_slot` — instant rotation is possible. Auditor should assess whether this is acceptable.

11. **`execution_queue_drop_ctm`:** Admin-only removal of a stuck pending CTM item. Verify:
    - Requires `paused_execute != 0` (queue must be paused)
    - The target item must have `status == Pending` and `sequence` must match
    - Uses `clear_ctm_item_at` which may advance the head

#### Enqueue — CTM path (`execution_queue_enqueue_ctm`)

12. **Payload hash verification:** `hashv(&[&payload])` must equal `envelope.payload_hash`. Verify the hash is computed over the raw payload bytes and compared before any deserialization.

13. **Accounts hash verification:** `hash_accounts(dispatch_accounts)` must equal `envelope.accounts_hash`. The hash includes pubkey (32 bytes) + is_signer (1 byte) + is_writable (1 byte) per account. Verify:
    - Hash is computed from the actual `remaining_accounts` passed to the instruction (after `split_dispatch_accounts` processing)
    - `split_dispatch_accounts` correctly strips the trailing execution queue alias and/or program ID sentinel

14. **Ed25519 CTM signature verification:** A pre-instruction Ed25519 verify must exist with:
    - Public key = `queue.ctm_signer`
    - Message = `SHA256("mango-v4-ctm-envelope-v1" || group || sequence || min_execute_slot || kind || payload_hash || accounts_hash || expires_at_slot)`
    - Verify the canonical message construction is domain-separated and binds all envelope fields
    - Verify `has_ed25519_preinstruction` iterates all instructions before the current one (not after)
    - Verify signature offset parsing correctly handles multi-signature Ed25519 instructions

15. **Ed25519 user intent signature verification:** For perp order variants, a second Ed25519 pre-instruction must exist with:
    - Public key = `account.owner` (loaded from the MangoAccount at `dispatch_accounts[1]`)
    - Message = `SHA256("mango-v4-user-intent-v1" || group || mango_account || user_owner || kind || payload_hash || accounts_hash)`
    - Supports both raw 32-byte hash and hex-encoded UTF-8 (64 bytes) message formats for wallet compatibility
    - Verify `extract_user_owner_for_ctm_payload` loads the MangoAccount, checks `account.group == group_key`, and returns the actual owner
    - Verify the user intent message binds the mango account key and owner, preventing replay across accounts

16. **Envelope expiry:** `expires_at_slot == 0 || clock.slot <= expires_at_slot`. Verify 0 means no expiry. Note: expiry is only checked at enqueue time, not at execute time — auditor should assess staleness risk.

17. **Sequence validation:** `envelope.sequence >= queue.header.next_sequence_to_execute`. Verify this prevents re-enqueue of already-executed sequences.

18. **Payload kind validation:** The decoded payload variant must map to `QueueItemKind::CtmWrapped`. Verify the `queue_item_kind_for_payload_variant` mapping is correct and complete.

#### Enqueue — Direct fallback (`execution_queue_enqueue_direct`)

19. **No CTM signature required:** This is the liveness fallback (C-4 fix). Verify:
    - CTM signer verification is NOT performed
    - User Ed25519 intent signature IS required (for variants that use user signatures)
    - Sequence is auto-assigned as `max_seen_sequence + 1` (not user-provided)
    - `min_execute_slot` is forced to at least `clock.slot + 10` (`DIRECT_SUBMIT_DELAY_SLOTS`)
    - The forced delay cannot be bypassed by the caller's `envelope.min_execute_slot`

20. **Sequence collision:** Since direct-submit assigns `max_seen_sequence + 1`, verify it cannot collide with a pending relayer-submitted sequence. The `can_enqueue_ctm_sequence` check and the `push_ctm` duplicate detection should prevent this.

#### Enqueue — Liquidity (`execution_queue_enqueue_liquidity`)

21. **No signer required:** This instruction has no signer constraint. Verify that the absence of a signer cannot be exploited to fill the 128-slot liquidity queue with garbage (griefing). Note: this was identified as H-1 in the internal audit.

22. **Delay enforcement:** `min_execute_slot = clock.slot + queue.header.liquidity_delay_slots` (default 25 slots). Verify the delay is non-bypassable.

23. **Kind validation:** Only `LiquidityDeposit` and `LiquidityWithdraw` kinds are accepted. Verify CTM-wrapped payloads are rejected.

#### Execute — Single lane (`execution_queue_execute`)

24. **Head-only strict FIFO (H-8 fix):** The execute path processes only the queue head (CTM first, then liquidity). Verify:
    - No scan-ahead or out-of-order execution
    - If the head item's `accounts_hash` doesn't match the provided accounts, execution stops (not skips)
    - A zero accounts hash (`[0; 32]`) is treated as a wildcard match

25. **Gap handling:** When `ctm_count > 0` but the head slot is empty (gap):
    - `gap_observed_slot` is set on first observation
    - After `gap_wait_slots` have elapsed, sequences are skipped (up to 32 per call)
    - Skipped items emit `QueueItemProcessed` events with `Skipped` status
    - `gap_observed_slot` is reset after skipping
    - Verify the skip loop terminates correctly (bounded by `EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE = 32`)

26. **min_execute_slot enforcement:** Items cannot execute before their `min_execute_slot`. Verify this is checked for both CTM and liquidity items.

27. **Payload hash re-verification:** At enqueue, payload hash is verified against the envelope. At execute, the stored payload is decoded directly (no re-hash). Verify this is safe because the payload bytes are stored on-chain and immutable after enqueue.

28. **Accounts hash verification at execute:** The `provided_accounts_hash` is computed from the actual `remaining_accounts` (C-1 fix) and compared to the item's stored `accounts_hash`. Verify this prevents account substitution attacks.

29. **Health-region wrapping for PerpPlaceOrderV2:** Verify:
    - `queue_health_region_begin` loads the MangoAccount, checks it's not already in a health region, computes pre-init health, and sets the health region flag
    - `queue_health_region_end` re-computes health and calls `check_health_post` against the stored pre-init value
    - If health region begin fails, the item's retry counter is incremented
    - If dispatch succeeds but health region end fails, the entire transaction rolls back (health-gated items cannot be swallowed)

30. **Retry logic:** Verify:
    - `EXECUTION_QUEUE_MAX_RETRIES = 1` — items get one retry before being cleared as Failed
    - Health-gated items that exhaust retries are cleared WITHOUT dispatching (pre-dispatch check)
    - Health-gated dispatch failures roll back the entire transaction (`return dispatch_result`)
    - Non-health-gated CTM failures increment retry in-place and break
    - Liquidity failures use `rotate_liquidity_head_with_retry` to re-queue at tail with backoff

31. **Dispatch routing (`dispatch_queue_payload`):** Verify:
    - Perp variants (PlaceOrderV2, CancelOrder, CancelOrderByClientOrderId, CancelAllOrders, CancelAllOrdersBySide) dispatch via direct function calls to `*_from_account_infos`
    - Liquidity variants (Deposit, Withdraw) dispatch via CPI (`invoke` or `invoke_signed`) to the program itself
    - For CPI dispatch, the program ID sentinel at the end of `invoke_accounts` is verified
    - PDA signing uses correct seeds: `["ExecutionQueue", group_key, bump]`

#### Execute — Multi-lane (`execution_queue_execute_multi`)

32. **Lane hash computation (C-1 fix):** Verify:
    - Lane hashes are computed from actual `remaining_accounts` slices (not from the `lane_hashes` instruction parameter, which is accepted but ignored)
    - The comment at line 1590-1591 confirms `lane_hashes` parameter is deprecated
    - Each lane's hash is computed via `hash_accounts(&account_metas_from_infos(lane))`

33. **Lane matching:** The head item's `accounts_hash` is matched against all lane hashes. Verify:
    - Only the head is checked (no scan-ahead, strict FIFO)
    - If no lane matches, execution stops
    - Zero accounts hash is treated as matching lane 0

34. **Multi-lane does not process liquidity items:** Verify `execute_multi` only processes CTM items (there is no liquidity fallback path). Liquidity items must be drained via single-lane `execute`.

#### Signature verification helpers

35. **`ed25519_ix_matches`:** Parses Ed25519 instruction data to find a signature matching a given public key and message. Verify:
    - Iterates all signatures in the instruction (supports multi-sig Ed25519 instructions)
    - Correctly parses the 14-byte signature offset structure
    - Handles `ED25519_CURRENT_INSTRUCTION_INDEX` (u16::MAX) as self-reference
    - Requires the signature blob to exist (not just the offsets)

36. **`merge_effective_runtime_flags_for_hash`:** Merges `is_signer` and `is_writable` flags across remaining and fixed account sets using OR semantics. Verify this correctly handles Solana's runtime flag promotion (accounts that appear in both sets get the union of their flags).

---

### File 3: `programs/mango-v4/src/accounts_ix/execution_queue.rs` (~150 LOC)

**Purpose:** Anchor account constraint definitions for all execution queue instructions.

**Constraints to verify:**

37. **`ExecutionQueueCreate`:** Verify:
    - `group.admin == admin.key()` (admin signer required)
    - PDA seeds: `["ExecutionQueue", group.key()]` with bump
    - Space: `EXECUTION_QUEUE_CREATE_SPACE` (8 bytes — just discriminator for initial alloc)
    - Payer is mutable signer

38. **`ExecutionQueueResize`:** Verify:
    - Same admin constraint as Create
    - Same PDA seeds (without `init`, since account exists)
    - `execution_queue` is `UncheckedAccount` (not yet initialized as typed)

39. **`ExecutionQueueInit`:** Verify:
    - Same admin constraint
    - `execution_queue` uses `zero` constraint (Anchor zero-copy init)
    - Same PDA seeds

40. **`ExecutionQueueAdmin`:** Verify:
    - `group.admin == admin.key()`
    - `execution_queue` has `has_one = group` constraint
    - Used for configure, set_ctm_pending, drop_ctm

41. **`ExecutionQueueEnqueueCtm`:** Verify:
    - `group` is mutable (for event emission CPI lamport accounting)
    - `execution_queue` has `has_one = group` and is mutable
    - `instructions` sysvar is verified via `address = tx_instructions::ID`
    - **No signer constraint on the enqueue caller** — authorization is via Ed25519 pre-instructions, not transaction signers

42. **`ExecutionQueueEnqueueLiquidity`:** Verify:
    - `group` is NOT mutable (read-only)
    - `execution_queue` has `has_one = group` and is mutable
    - **No signer required** — auditor should assess griefing risk (see item 21)
    - No instructions sysvar (no signature verification for liquidity)

43. **`ExecutionQueueExecute`:** Verify:
    - `group` is mutable
    - `execution_queue` has `has_one = group` and is mutable
    - **No signer or authority constraint** — permissionless cranking (anyone can execute)
    - All authorization is implicit via accounts hash matching and payload integrity

---

## Expected Functional Behavior (Summary)

The execution queue implements an **on-chain FIFO order sequencing system** for a perpetual futures exchange. The expected flow is:

1. **Relayer path:** An off-chain CTM (Continuum Transaction Manager) relayer assigns monotonically increasing sequence numbers to user intents, co-signs an envelope, and submits `enqueue_ctm`. The on-chain program verifies both the relayer's and user's Ed25519 signatures, validates payload/accounts hashes, and inserts the item into the CTM ring buffer at its sequence slot.

2. **Direct fallback:** If the relayer is unavailable, users can submit `enqueue_direct` with only their own signature. The program assigns `max_seen_sequence + 1` and enforces a 10-slot execution delay to prevent race conditions with relayer-submitted items.

3. **Liquidity path:** Deposit/withdraw operations are enqueued separately with a 25-slot delay and no signature requirement.

4. **Execution:** A permissionless cranker calls `execute` or `execute_multi`. The program processes items in strict FIFO order (head only), verifies accounts hashes match, decodes the payload, and dispatches to the appropriate internal handler. Health-gated operations (PerpPlaceOrderV2) are wrapped in pre/post health checks. Failed items are retried once, then cleared.

5. **Gap handling:** If a sequence gap is detected (relayer transaction dropped), the queue waits `gap_wait_slots` (default 4) before skipping the missing sequences and resuming.

---

## Explicitly Out of Scope

The following components are **unchanged from audited Mango v4** and are NOT in scope:

- Token operations (deposit, withdraw, force_withdraw, charge_collateral_fees, conditional_swap)
- Bank state, interest rate model, index update, dust handling
- Health engine (health_cache, health_region, account health computation)
- Liquidation (token_liq_with_token, token_liq_bankruptcy, perp_liq_*, force_cancel)
- Perp markets (perp_create_market, perp_edit_market, perp_consume_events, perp_settle_pnl, perp_settle_fees, funding)
- Perp orderbook (bookside, order matching, event queue) — the `perp_place_order`, `perp_cancel_order`, etc. **internal logic** is unchanged; only thin `from_account_infos` wrapper functions were added
- Oracle system (Pyth, Switchboard, CLMM, StubOracle, stable_price)
- Flash loans
- Serum/OpenBook integration
- Group and account management (group_create, group_edit, account_create, account_close, etc.)
- Governance, ix_gate, and all admin instructions except the new execution_queue admin functions
- All off-chain services (liquidator, keeper, settler, crank, fills, health, orderbook, pnl, execution-engine)
- All TypeScript SDK and client code
- All test scripts and tooling
- Third-party patched dependencies (openbook-v2-patched, solana-program-1.16.14-local, ahash patches, protobuf-src-noop) — these are build compatibility patches with no logic changes

---

## Path to "Fully Audited" Certification

| Coverage Area | Audit Source | Status |
|---------------|-------------|--------|
| Core Mango v4 (banks, tokens, health, liquidation, perps, oracles, flash loans, serum) | OtterSec v0.7.0 through v0.24.0 (9 reports) | Previously audited, unchanged |
| Execution Queue module (state, instructions, account constraints) | **This differential audit** | Pending |
| Internal security review of full codebase including execution queue | `audit_mar20.md` (46 findings, all addressed) | Complete |

Upon completion of this differential audit with no unresolved critical/high findings, the combined coverage provides:

1. **OtterSec audits** cover all original Mango v4 on-chain logic (~210 Rust source files)
2. **This audit** covers all new on-chain logic (3 files, ~2,500 LOC)
3. **The internal review** provides cross-cutting coverage of the interaction surface between old and new components

No on-chain instruction path exists that is not covered by at least one of these three audit sources. The program can be certified as fully audited for the `v5b` branch.
