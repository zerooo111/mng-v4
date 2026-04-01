# Execution Queue Stall Catalog — Production Reference

Every known failure mode that can stall the execution queue, how to detect it,
how to debug it, and the fix (applied or pending). **On mainnet any of these
can cause loss of FIFO fidelity, position divergence, or effective fund lock.**

---

## Quick Diagnostic Checklist

When the queue appears stuck, check in this order:

```bash
# 1. Is the relayer running?
screen -r relayer   # or: systemctl status stagin4-devnet-relayer

# 2. What sequence is stuck?
tail -5 .devnet/logs/ctm-relayer.log | grep "sequence="

# 3. What error?
grep WARN .devnet/logs/ctm-relayer.log | tail -10

# 4. On-chain queue state
# (replace Pubkeys with your deployment)
node -e "... read queue header at offset 152 ..."
# → next_sequence, max_seen_sequence, ctm_count, gap_wait_slots

# 5. Harness watermarks
curl -s http://127.0.0.1:9091/state/full | jq '.markets["0"].watermarks'
# → optimistic_seq vs confirmed_seq gap

# 6. RPC health
grep "ENHANCE_YOUR_CALM\|too_many_internal_resets\|429\|rate" .devnet/logs/ctm-relayer.log | tail -5
```

---

## STALL-1: Expired Health-Gated Orders (Critical)

**Severity:** Critical — permanently blocks queue until admin intervention
**Discovered:** 2026-03-24 on devnet (512 items blocked)

### Trigger

Orders with `expiry_timestamp` (Unix seconds) expire while sitting in the queue.
When the executor tries to crank them, `PerpPlaceOrderV2` is health-gated:
the dispatch enters a health region, fails ("Order is already expired"),
and the **entire transaction rolls back**. The retry counter is never incremented
(rolled back with the tx), so the item remains permanently at the head.

### Detection

```
WARN executor tx failed sequence=N err=Some(InstructionError(2, Custom(6000)))
```
Error 6000 = `SomeError` triggered by `tif_from_expiry` returning `None`.
The sequence number never advances. Relayer log shows the same `sequence=N` repeating.

### Root Cause Chain

1. Enqueue checks `envelope.expires_at_slot` (slot-based) but NOT `payload.expiry_timestamp` (Unix)
2. Order can have valid envelope but expired Unix timestamp
3. `tif_from_expiry()` at `state/orderbook/order.rs:50` returns `None` when `expiry_timestamp <= now_ts`
4. Health-gated dispatch failure at `execution_queue.rs:1511` does `return dispatch_result` → full rollback
5. Retry counter at `execution_queue.rs:1515` is unreachable (rolled back)
6. Gap-skip at `execution_queue.rs:1321` only fires for empty slots (`current_ctm_head().is_none()`);
   expired items have `status=Pending` so `current_ctm_head()` returns `Some(item)`

### Why Existing Mechanisms Fail

| Mechanism | Why it doesn't help |
|-----------|-------------------|
| Gap-skip (`gap_wait_slots`) | Only skips empty slots, not populated-but-expired |
| Pre-dispatch retry clear | `retries` always 0 (tx rollback undoes increment) |
| `execution_queue_drop_ctm` | Requires `paused_execute != 0` — needs admin tx |

### Fix Status

| Fix | Layer | Status |
|-----|-------|--------|
| Pre-dispatch expiry check before health region | On-chain | **TODO** — proposed code in this doc |
| Reject expired orders at enqueue time | On-chain | **TODO** |
| Per-sequence failure counter → proactive admin-drop | Off-chain | **Done** 2026-03-24 |
| Default `ORDER_EXPIRY_SECS=0` (no expiry) | Bots | **Done** 2026-03-24 |

### Emergency Unblock

```bash
# Stop relayer + bots
screen -S relayer -X quit; screen -S quoter -X quit

# The relayer's auto-drop mechanism works IF:
# a) PENDING_TIMEOUT_MS is high enough for devnet (≥10000)
# b) STATUS_POLL_MS gives time between polls (≥2000)
# c) RPC isn't rate-limited
# Set these in devnet-stack.env, restart relayer, and wait for auto-drop to fire.

# If auto-drop can't fire (RPC too slow), reset sequence cursor:
echo '{"<GROUP_PK>:0": <MAX_SEEN_SEQ + 1>}' > .devnet/run/ctm-sequences-*.json
# Then restart relayer + bots. Stale items become gaps, eventually skipped.
```

### Production Prevention

- **Always set `ORDER_EXPIRY_SECS=0`** or ensure `expiry_timestamp > now + max_queue_latency`
- On-chain fix (when deployed): pre-dispatch expiry check clears expired items in O(1)
- On-chain fix (when deployed): enqueue-time rejection prevents expired orders from entering

### Recommended Long-Term Fix

The correct durable fix is **not** to auto-skip every reverted head. The queue must
distinguish between:

- **terminal / immutable failures**: deterministically invalid from signed payload
  bytes, account metas, and current time, before any state mutation;
- **stateful / retryable failures**: depend on live health, oracle freshness,
  account balances, market state, or matching outcomes.

Recommended implementation:

1. Add a shared on-chain `prevalidate_terminal_ctm_payload()` helper.
2. Call it at **enqueue time** to reject items that are already terminally invalid.
3. Call it again at **execute time**, before `queue_health_region_begin()` and
   before any order-book mutation.
4. If the helper returns a terminal failure, clear the head item on-chain and emit
   `QueueItemProcessed { status: Failed }` without bubbling a rollback.
5. If the failure is not terminal, keep the current rollback behavior so FIFO and
   state correctness are preserved.

Initial terminal class for `PerpPlaceOrderV2`:

- `expiry_timestamp` already expired at enqueue
- `expiry_timestamp` expires while waiting in queue
- payload invariants that can be checked pre-dispatch and do not depend on live
  market/account state, such as invalid numeric inputs

Failures that must **not** be auto-dropped:

- health-post failures
- oracle freshness failures
- account-balance / margin failures
- any dispatch failure that occurs after book mutation begins
- RPC submission / polling failures

Reason: these are not immutable-invalidity proofs. Treating them as terminal can
break FIFO guarantees or discard orders that would have become executable later.

---

## STALL-2: RPC Rate-Limit Masks Auto-Drop Recovery

**Severity:** High — prevents the off-chain workaround for STALL-1
**Discovered:** 2026-03-24 on devnet (Helius 5 sendTx/sec limit)

### Trigger

The executor sends execute txs in a tight loop (`CRANK_INTERVAL_MS=25`,
`BUSY_INTERVAL_MS=1` = 40+ txs/sec). This exceeds the RPC `sendTransaction` rate limit.
The RPC returns `ENHANCE_YOUR_CALM` / HTTP2 `too_many_internal_resets`.

### Detection

```
WARN h2::proto::streams::streams: locally-reset streams reached limit (1024)
DEBUG executor signature status poll failed: ...http2 error...
```
No `WARN executor tx failed` lines (status poll never succeeds).

### Impact

The auto-drop mechanism (`maybe_auto_drop_expired_head`) requires a **confirmed-failed**
tx signature to inspect its logs. If `getSignatureStatuses` fails, the pending dispatch
is never reconciled, and the recovery path never fires. The stall continues.

### Fix Status

| Fix | Status |
|-----|--------|
| Per-sequence failure counter (fires admin-drop after N poll failures) | **Done** 2026-03-24 |
| Tune `CRANK_INTERVAL_MS=1000`, `PENDING_TIMEOUT_MS=10000`, `STATUS_POLL_MS=2000` | **Done** 2026-03-24 |
| Gate execute sends: max 1 pending tx per sequence | **TODO** |

### Production Prevention

- **Know your RPC limits** and configure crank interval accordingly
- `CRANK_INTERVAL_MS` should be `≥ 1000 / sendTx_per_sec_limit`
- `PENDING_TIMEOUT_MS` should be `≥ 3 * expected_slot_time` (devnet: ≥10s, mainnet: ≥5s)
- `STATUS_POLL_MS` should be `≥ 1000` to avoid wasting read budget on status checks
- For Helius devnet (5 write/sec, 50 read/sec): `CRANK_INTERVAL_MS=1000`, `STATUS_POLL_MS=2000`
- Use **at most one pending execute tx per head sequence**. Multiple in-flight txs
  for the same head amplify RPC write pressure without improving recovery.
- Treat RPC/status-poll failure as an **operational signal**, not semantic evidence
  that the queue head is invalid. Admin-drop based only on poll failure should
  remain an emergency fallback, not the primary correctness path.

---

## STALL-3: Lane Mismatch After Event Log Clear / Restart

**Severity:** High — executor cannot crank any items
**Discovered:** 2026-03-24 on devnet

### Trigger

The executor builds "lanes" (sets of accounts needed to execute specific order types)
from the harness event log (`continuum-harness-*.jsonl`). If the event log is cleared
or missing (e.g., after a restart or disk cleanup), the executor has no lane definitions.
Items in the on-chain queue have `accounts_hash` values that don't match any known lane.

### Detection

```
DEBUG executor skipped: queue_count=N next_sequence=M reason=no_lane_match head_hash=<hex>
```
This repeats indefinitely. The queue grows but nothing executes.

### Root Cause

The executor's `execute_once()` function at `main.rs:1985` requires matching the
queue head's `accounts_hash` against a pre-built lane. Lanes come from:
1. Static lane config file (`EXECUTION_QUEUE_CRANK_LANES_JSON_PATH`)
2. Dynamic lanes built from the event log via `refresh_dynamic_lanes_from_event_log()`

With both sources empty, no lane matches any queue item.

### Fix Status

| Fix | Status |
|-----|--------|
| Ensure event log is preserved across restarts | **Operational** |
| Executor should read queue item accounts directly from on-chain | **TODO** |
| Executor should derive lanes from on-chain queue state at startup | **TODO** |

### Emergency Unblock

The only current option is to re-bootstrap a new execution queue (new group number)
since there's no way to build a matching lane without the event history:

```bash
export EXECUTION_QUEUE_GROUP_NUM=<new_number>
ts-node ts/client/scripts/execution-queue/local-perp-e2e-bootstrap.ts
# Then update E2E_CONFIG_PATH, re-render env, re-provision bots, restart all
```

### Production Prevention

- **Never clear the event log** — it's the executor's lane history
- Back up the event log file as part of deployment/restart procedures
- Ensure the lane config file (`execution-queue-lanes-*.json`) is generated and preserved
- **TODO:** Implement on-chain lane derivation so the executor is self-sufficient

---

## STALL-4: Compute Budget Exceeded on Large Queues

**Severity:** Critical — queue becomes permanently stuck when it fills up
**Discovered:** 2026-03-24 on devnet (515+ items → ProgramFailedToComplete)

### Trigger

When the on-chain queue has 400+ items (`ctm_count ≥ ~400`), the
`execution_queue_execute_multi` instruction exceeds the 900,000 CU compute budget.
The execute tx fails with `ProgramFailedToComplete`. Since the queue can't advance,
new items keep being enqueued, making the problem worse.

### Detection

```
WARN executor tx failed sequence=N sig=... err=Some(InstructionError(2, ProgramFailedToComplete))
```
The sequence never advances. `ctm_count` keeps growing in on-chain reads.

### Root Cause

The execute instruction's dispatch loop scans the ring buffer and performs:
- Ring buffer item lookup and validation
- Payload decoding
- Health region begin CPI (for health-gated items)
- `perp_place_order_v2` dispatch CPI
- Health region end CPI

With a full 1024-slot ring buffer, even finding the correct item at
`ctm_items[next_sequence % 1024]` involves non-trivial account deserialization.
The combined cost exceeds 900K CU when the queue has many items.

### Fix Status

| Fix | Status |
|-----|--------|
| Optimize ring buffer access (direct index, not scan) | **TODO** on-chain |
| Increase CU limit to 1.2M or 1.4M | **TODO** off-chain — simple config change |
| Rate-limit bot submission to match crank rate | **Done** operationally |

### Emergency Unblock

Cannot drain via executor (every tx fails). Options:
1. **Admin-drop in batches** — pause execute → drop_ctm × 8 → unpause, repeat.
   The relayer's auto-drop does this for expired items, but not for compute failures.
2. **Re-bootstrap** — create a new execution queue on a new group number.

### Production Prevention

- **Enforce `submit_rate ≤ crank_rate`** so the queue never fills up
- Monitor `ctm_count` — alert at 200, critical at 400

---

## Recommended Failure-Handling Policy

To avoid queue stalls without introducing new edge cases, the queue should follow
this failure policy for CTM-wrapped perp flow:

| Failure class | Examples | Queue action |
|---------------|----------|--------------|
| Terminal / immutable | expired `expiry_timestamp`, malformed/invalid payload that can be proven before dispatch | reject at enqueue or clear at execute |
| Stateful / retryable | health failure, oracle staleness, temporary margin shortfall, mutable market-state failure | preserve current rollback semantics; do not auto-drop on-chain |
| Operational / transport | RPC rate limit, status poll failure, relayer restart, missing lane cache | handle off-chain with backoff, alerting, and operator recovery |

### Why this split is the safest option

- It removes the known expired-order stall in O(1) on-chain.
- It preserves strict FIFO for orders that are merely temporarily unexecutable.
- It avoids state divergence from swallowing errors after partial book mutation.
- It keeps off-chain executor logic focused on transport recovery rather than
  semantic classification of order validity.

### Future Refactor Boundary

If broader self-healing is desired beyond expired orders, the long-term safe path
is to refactor perp placement into a **validate-then-apply** shape, where all
terminal checks run before any book mutation. Until then, only pre-dispatch,
immutable-invalidity checks should be allowed to clear queue heads automatically.
- Set `queue_soft_limit` to 256 (not 1024) to reject new enqueues before compute becomes an issue
- **TODO:** Increase CU limit to 1.4M in the executor
- **TODO:** On-chain optimization to reduce per-item compute cost

---

## STALL-5: Pending Timeout Too Short for Network Latency

**Severity:** Medium — prevents auto-drop recovery
**Discovered:** 2026-03-24 on devnet

### Trigger

`EXECUTION_QUEUE_CRANK_PENDING_TIMEOUT_MS=100` (original default) is far too short
for devnet (~400ms slot time). The executor sends a tx, then 100ms later considers
it "timed out" and discards it from `pending_dispatches`. The signature status is
never checked, so even if the tx confirmed (with an error), the auto-drop path
doesn't fire.

### Detection

The relayer shows `executor sent` lines but no corresponding `executor tx failed`
or `executor tx confirmed` lines. The pending dispatch expires silently.

### Fix Status

| Fix | Status |
|-----|--------|
| `PENDING_TIMEOUT_MS=10000` for devnet | **Done** 2026-03-24 |
| `STATUS_POLL_MS=2000` for devnet | **Done** 2026-03-24 |

### Production Values

| Parameter | Devnet | Mainnet (recommended) |
|-----------|--------|----------------------|
| `CRANK_PENDING_TIMEOUT_MS` | 10000 | 5000 |
| `CRANK_STATUS_POLL_MS` | 2000 | 500 |
| `CRANK_INTERVAL_MS` | 1000 | 200 |
| `CRANK_BUSY_INTERVAL_MS` | 500 | 100 |
| `CRANK_MAX_PENDING_TXS` | 2 | 5 |

---

## STALL-6: Dual Expiry Systems (Slot vs Unix Timestamp)

**Severity:** Design issue — causes STALL-1
**Discovered:** 2026-03-24

### Description

The execution queue has **two independent expiry mechanisms** that don't cross-validate:

| Field | Checked at | Units | Purpose |
|-------|-----------|-------|---------|
| `envelope.expires_at_slot` | Enqueue (`execution_queue.rs:1021`) | Solana slot | CTM envelope validity window |
| `payload.expiry_timestamp` | Execute dispatch (`order.rs:50`) | Unix seconds (UTC) | Order trading expiry |

An order can pass the enqueue check (`expires_at_slot=0` or future slot) but fail at
execute time because `expiry_timestamp` has passed. This is the root cause of STALL-1.

### Fix Status

| Fix | Status |
|-----|--------|
| Validate `expiry_timestamp` at enqueue time | **TODO** on-chain |
| Require `expiry_timestamp == 0 OR expiry_timestamp > now + MIN_BUFFER` | **TODO** on-chain |
| Default `ORDER_EXPIRY_SECS=0` in bots | **Done** 2026-03-24 |

### Production Prevention

- Until on-chain fix: **never set order expiry** (`expiry_timestamp=0`)
- If expiry is needed: set `expiry_timestamp > now + 300` (5 min minimum)
  to account for worst-case queue processing delay

---

## STALL-7: Harness State Divergence After Restart

**Severity:** Medium — harness shows wrong positions, users see stale data
**Discovered:** 2026-03-24

### Trigger

The harness builds state exclusively from the intent event stream. On restart,
it replays from the event log file. If the event log is missing, corrupted, or
doesn't cover the full history, the harness starts from zero — showing no users,
no positions, and no orderbook while on-chain state has accumulated data.

### Detection

The divergence monitor shows large position differences:
```
base_position_lots: quoter-bot-0 harness=0 onchain=715940 delta=-715940
```
Harness `/healthz` shows `intents_total` near zero after a restart.

### Fix Status

| Fix | Status |
|-----|--------|
| On-chain bootstrap at harness startup (`bootstrapFromOnchainSnapshot`) | **Done** 2026-03-24 |
| Baseline positions injected into `ContinuumStateEngine` | **Done** 2026-03-24 |

### What the Bootstrap Does

At startup, before event log replay:
1. Reads all mango accounts from on-chain via `buildOnchainConfirmedSnapshot()`
2. Calls `engine.bootstrapFromOnchainSnapshot(snapshot)` to seed baseline positions
3. Subsequent intents are applied as deltas on top of the on-chain baseline

### Limitation

The bootstrap reads **confirmed** on-chain state. Any optimistic intents that were
accepted but not yet confirmed are lost on restart. This means:
- Open orders from the optimistic window disappear from the harness
- Positions from unconfirmed matches are not reflected
- The harness will re-converge as the executor confirms pending sequences

---

## Production Monitoring Requirements

### Alerts to implement before mainnet

| Metric | Threshold | Action |
|--------|-----------|--------|
| `ctm_count` | > 200 | Warn — queue filling up |
| `ctm_count` | > 400 | Critical — compute budget at risk |
| `optimistic_seq - confirmed_seq` | > 50 | Warn — executor falling behind |
| `optimistic_seq - confirmed_seq` | > 200 | Critical — stall likely |
| Same `next_sequence` for > 30s | — | Critical — queue head stuck |
| `ProgramFailedToComplete` count | > 3 in 1min | Critical — compute exceeded |
| `Custom(6000)` error count | > 0 | Warn — expired orders in queue |
| RPC `429` or `ENHANCE_YOUR_CALM` | > 0 | Warn — rate-limited |

### p(inclusion) monitoring

The `gap_wait_slots` parameter controls FIFO fidelity. Current setting: **2 slots (~0.8s)**.

For p98-p99 FIFO guarantee on mainnet (SWQoS), measure:
- Time from `submit_intent` to `queue_item_processed` per sequence
- Track the distribution of slot gaps between enqueue and execute
- `gap_wait_slots` should be set to the p99 inclusion latency in slots

---

## TODO Summary

### On-chain (program upgrade required)

- [ ] Pre-dispatch expiry check in `execution_queue_execute` / `execute_multi`
- [ ] Reject expired orders at `execution_queue_enqueue_ctm`
- [ ] Validate `expiry_timestamp > clock.unix_timestamp + MIN_BUFFER` at enqueue
- [ ] Optimize ring buffer access for lower CU cost
- [ ] Add `execution_queue_clear` admin instruction for emergency drain
- [ ] Unit tests for all stall scenarios

### Off-chain (relayer)

- [x] Per-sequence failure counter → proactive admin-drop *(2026-03-24)*
- [x] Tune crank timing for devnet RPC limits *(2026-03-24)*
- [ ] Gate execute sends: max 1 pending tx per sequence
- [ ] Simulation-based expired-order detection (fallback when log fetch fails)
- [ ] Handle `ProgramFailedToComplete` — trigger admin-drop or increase CU
- [ ] Derive lanes from on-chain queue state (eliminate event log dependency)
- [ ] Increase CU limit to 1.4M

### Off-chain (harness)

- [x] On-chain bootstrap at startup *(2026-03-24)*
- [ ] Periodic re-sync from on-chain (not just at startup)
- [ ] Alert on divergence between optimistic and confirmed state

### Off-chain (bots)

- [x] Startup SOL/account/deposit checks *(2026-03-24)*
- [x] Default `ORDER_EXPIRY_SECS=0` *(2026-03-24)*
- [ ] Enforce `submit_rate <= crank_rate` as a hard limit
- [ ] Backpressure: pause submission when `ctm_count > threshold`

---

## On-Chain Recovery Architecture — Design Proposal

### The Fundamental Problem

All `PerpPlaceOrderV2` dispatches are health-gated. The execution pattern is:

```
health_region_begin (CPI)  →  perp_place_order (CPI, mutates book)  →  health_region_end (CPI)
```

If `health_region_end` fails (bad health), the book has already been mutated by
`perp_place_order`. The only way to undo the book mutation is to roll back the
entire transaction (`return Err`). But rolling back the tx also reverts any
retry counter increment or item clearing we attempted.

**This is the root cause of ALL health-gated stalls.** It's a design constraint
of the CPI + health region pattern, not a bug in any specific check.

### Why Simple Fixes Don't Work

**"Just increment the retry counter before dispatch"** — Doesn't work. Solana tx
rollback reverts ALL account mutations in the tx, not just the CPI's. The retry
increment is reverted too.

**"Use a separate account for retry tracking"** — Doesn't work. ALL accounts
mutated in the tx are reverted on rollback, regardless of whether they were
passed to the CPI.

**"Set gap_observed_slot before dispatch"** — Doesn't work. Same reason. The
slot observation is reverted on rollback.

**"Head dwell timeout"** — The timeout state itself is reverted on rollback,
so the timer never advances. The only way to persist timeout state across
failed txs is to not return an error — which means the book stays mutated.

### Constraint

We cannot modify audited mango-v4 internal code (`perp_place_order`,
`health_region_begin/end`, orderbook logic). We can only add new code in
`execution_queue.rs` (our code) and call existing mango-v4 CPI endpoints.

### Proposed Solution: Defense-in-Depth (4 Layers)

#### Layer 1: Pre-Check (catches ~95% of failures)

Read-only checks BEFORE entering the health region. No CPI, no mutations,
no rollback risk. If any check fails, clear the item and continue.

```rust
// New function in execution_queue.rs (our code)
fn pre_check_would_dispatch_fail(
    accounts: &[AccountInfo],
    decoded_payload: &DecodedQueuePayload,
    clock: &Clock,
) -> Result<bool> {
    // 1. EXPIRY CHECK (exact — no race condition)
    //    expiry_timestamp != 0 && expiry_timestamp <= clock.unix_timestamp
    //    Covers: STALL-1, H-5

    // 2. ACCOUNT STATUS (read-only)
    //    - mango account frozen? (covers H-3, H-18)
    //    - mango account closed/invalid? (covers H-18)
    //    - account already in health region? (covers H-16)

    // 3. MARKET STATUS (read-only)
    //    - perp market paused? (covers H-2)
    //    - perp market settled?

    // 4. ORACLE FRESHNESS (read-only)
    //    - oracle last_update_slot too old? (covers H-4)

    // 5. CONSERVATIVE HEALTH ESTIMATE (read-only)
    //    - Load current positions + oracle prices
    //    - Compute approximate post-order health
    //    - If clearly negative → would fail
    //    Covers: H-1 (most cases)
    //
    //    This is CONSERVATIVE: may reject orders that would pass (false positive).
    //    Never approves orders that would fail (no false negative).
    //    A wrongly-rejected order can be resubmitted by the user.

    Ok(would_fail)
}

// In the execute loop, before health_region_begin:
if item_health_region.is_some() && candidate.is_ctm {
    if pre_check_would_dispatch_fail(dispatch_accounts, &decoded_payload, &clock)? {
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.clear_ctm_item_at(candidate.sequence);
        emit!(QueueItemProcessed { status: Failed });
        continue;  // No CPI entered, no rollback, item cleared
    }
}
```

**CU cost:** ~100-150K for read-only account deserialization + health math.
**Coverage:** ~95% of health-gated failures.
**False positives:** Conservative estimate may over-reject by ~1-2%. Acceptable —
user resubmits, no funds lost.

#### Layer 2: Normal CPI Dispatch (the happy path)

The existing health region pattern, unchanged:

```
health_region_begin → perp_place_order → health_region_end
```

Pre-check passed, so this should succeed. If it does, item is executed normally.

#### Layer 3: Compensating Cancel (catches ~4.9% — the pre-check misses)

If pre-check passed but `health_region_end` still fails (state changed between
pre-check and dispatch, rounding differences, etc.), the book has been mutated.
Instead of rolling back the entire tx, **cancel the just-placed order** using
the existing `perp_cancel_order` CPI to undo the book mutation, then clear the
queue item.

```rust
// After dispatch, if health_region_end failed:
if dispatch_result.is_err() && item_health_region.is_some() && candidate.is_ctm {
    // The book was mutated by perp_place_order. Undo it by cancelling
    // the order we just placed. This uses the EXISTING audited
    // perp_cancel_order CPI — no internal mango-v4 modifications.
    let cancel_result = compensating_cancel(
        dispatch_accounts,
        invoke_accounts,
        &decoded_payload,  // contains client_order_id for the cancel
    );

    if cancel_result.is_ok() {
        // Book is clean. Item can be safely cleared without rollback.
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.clear_ctm_item_at(candidate.sequence);
        emit!(QueueItemProcessed {
            group: ctx.accounts.group.key(),
            sequence: candidate.sequence,
            kind: candidate.kind,
            status: QueueItemStatus::Failed as u8,
        });
        continue;  // ← RETURN OK, item cleared, no rollback
    } else {
        // Cancel also failed — fall through to Layer 4
        return dispatch_result;  // tx rolls back (existing behavior)
    }
}

fn compensating_cancel(
    dispatch_accounts: &[AccountInfo],
    invoke_accounts: &[AccountInfo],
    decoded_payload: &DecodedQueuePayload,
) -> Result<()> {
    // Extract client_order_id from the decoded PerpPlaceOrderV2 payload.
    // Call perp_cancel_order_by_client_order_id via CPI to remove the
    // order from the book. This is an existing audited mango-v4 endpoint.
    //
    // The cancel CPI uses the same accounts as the place CPI
    // (mango account, perp market, orderbook, etc.) which are already
    // available in dispatch_accounts.
    ...
}
```

**Why this works:**
- `perp_place_order` succeeded → order is on the book with a `client_order_id`
- `health_region_end` failed → we know health is bad
- `perp_cancel_order_by_client_order_id` removes the order from the book
- Book is now clean (order removed), so we can return `Ok` without rollback
- Returning `Ok` means our `clear_ctm_item_at` mutation is persisted

**Why the cancel should succeed:**
- We just placed the order, so it exists on the book
- The cancel uses the same accounts (already validated)
- Cancel is NOT health-gated — it always succeeds if the order exists

**CU cost:** ~50-100K for the cancel CPI. Only runs on failed health checks
(rare after Layer 1 pre-check).

**What if cancel fails?** The order we just placed doesn't exist on the book
(shouldn't happen), or the accounts are wrong (shouldn't happen — same accounts
used for place). In this extremely unlikely case, we fall through to the existing
rollback behavior, and the off-chain auto-drop handles it (Layer 4).

#### Layer 4: Off-Chain Auto-Drop (0.1% — last resort)

The existing off-chain mechanism: relayer detects repeated failures for the same
sequence, sends an admin tx (pause → drop_ctm → unpause). This is the current
behavior, already implemented.

Only fires if Layers 1-3 all fail, which requires:
1. Pre-check passed (didn't catch the problem)
2. Health region CPI failed (dispatch or health_region_end)
3. Compensating cancel also failed (order not on book or account error)

This should be near-zero in practice.

### Coverage Analysis

| Layer | Catches | CU Cost | Modifies mango-v4? | On-chain? |
|-------|---------|---------|---------------------|-----------|
| 1. Pre-check | ~95% | ~100-150K | No (read-only) | Yes |
| 2. Normal dispatch | Happy path | ~300-400K | No (existing CPI) | Yes |
| 3. Compensating cancel | ~4.9% | ~50-100K | No (existing CPI) | Yes |
| 4. Off-chain auto-drop | ~0.1% | 0 (separate tx) | No | No |

**Total on-chain CU for failure path:** ~250K (pre-check + cancel).
**Total on-chain CU for success path:** ~400-550K (pre-check + dispatch).

### What Each Layer Covers

| Scenario | Layer 1 | Layer 2 | Layer 3 | Layer 4 |
|----------|---------|---------|---------|---------|
| Expired order (STALL-1) | Caught | — | — | — |
| Bad health (H-1) | Caught (most) | — | Caught (rest) | — |
| Market paused (H-2) | Caught | — | — | — |
| Account frozen (H-3) | Caught | — | — | — |
| Oracle stale (H-4) | Caught | — | — | — |
| Bad price (H-5) | Caught | — | — | — |
| Self-trade (H-6) | Not caught | Fails | Caught | — |
| Position limit (H-7) | Could catch | Fails | Caught | — |
| Reduce-only (H-8) | Not caught | Fails | Caught | — |
| Bank frozen (H-17) | Caught (begin) | — | — | — |
| Account closed (H-18) | Caught | — | — | — |
| Cancel fails too | — | — | Not caught | Caught |

### Open Questions

1. **Can `perp_cancel_order_by_client_order_id` be called after a failed
   `health_region_end`?** The account may still be "in health region" state.
   Need to verify the cancel doesn't require health region or check the flag.
   If it does, we may need to call `health_region_end` with a flag that forces
   success (or add a `health_region_abort` instruction).

2. **What `client_order_id` was used?** The execution queue payload contains
   the order parameters including `client_order_id`. We need to pass this
   through to the cancel CPI.

3. **Does the cancel CPI need different accounts than the place CPI?**
   Both need: mango_account, perp_market, bids, asks, group. Should be the
   same account set already in `dispatch_accounts`.

4. **CU budget:** With pre-check (~150K) + dispatch (~400K) + cancel (~100K),
   total is ~650K. Current limit is 900K. Sufficient, but tight if the queue
   is large (scan overhead). May need to increase to 1.2M.

### Implementation Priority

1. **Layer 1 (pre-check):** Implement first. Standalone, no interaction with
   CPI pattern. Eliminates 95% of stalls immediately.
2. **Layer 3 (compensating cancel):** Implement second. Requires careful testing
   of the cancel-after-failed-health-region flow. Eliminates remaining 4.9%.
3. **Layer 4 (auto-drop):** Already implemented off-chain.
4. **Layer 2 (normal dispatch):** Already exists, no changes.

---

## Hypothetical Stall Scenarios — Speculative Analysis

**Key design fact:** ALL `PerpPlaceOrderV2` and `PerpPlaceOrderV2Pegged` orders are
health-gated. Any dispatch failure for these types causes a full tx rollback,
meaning the retry counter is never incremented. The item stays at the queue head
until the off-chain admin-drop fires (if it can).

Recovery depends on the off-chain auto-drop mechanism:
- **With auto-drop working:** Stall lasts ~5-15s (N failures × crank interval)
- **Without auto-drop (RPC issues, config bugs):** Permanent stall until manual intervention

Each scenario is rated:
- **Recoverable (auto)**: Off-chain auto-drop handles it
- **Recoverable (manual)**: Requires operator intervention
- **Permanent stall**: No recovery without code change or re-bootstrap
- **Not a stall**: Queue handles it natively

---

### H-1: User health drops below maintenance margin before execution

**Scenario:** User places a perp order. Between enqueue and execute, their
health drops (e.g., oracle price moved against them). `health_region_end`
fails because `check_health_post` detects worsened health.

**On-chain path:** Dispatch succeeds → health_region_end fails → tx rolls back (line 1511)

**Recoverable?** Recoverable (auto) — auto-drop fires after N failures.
But during the stall window, ALL other orders behind this one are blocked.
**On mainnet this could block hundreds of orders for 5-15s.**

**Risk:** HIGH for mainnet. A single undercollateralized user can block the
entire queue. This is the most likely production stall scenario.

**Prevention:** Pre-dispatch health check (dry-run) before entering health region.
Reject orders that would fail the post-health check without entering the region.

---

### H-2: Perp market is paused by admin during execution

**Scenario:** Admin pauses a perp market (e.g., during an oracle incident).
Orders already in the queue for that market fail at dispatch.

**On-chain path:** `perp_place_order` checks market status → dispatch fails → rollback

**Recoverable?** Recoverable (auto) — auto-drop clears them. But new orders
for the same market keep being enqueued and also fail, creating a continuous
stream of stall-then-drop cycles.

**Risk:** MEDIUM. The queue processes slowly while the market is paused because
every order for that market must fail → auto-drop → next order → fail → drop.

**Prevention:** Off-chain: relayer should check market status before accepting
intents and reject orders for paused markets at the gRPC layer.

---

### H-3: Mango account frozen (e.g., liquidation in progress)

**Scenario:** A user's mango account is frozen (e.g., concurrent liquidation).
Their pending orders in the queue fail at dispatch.

**On-chain path:** Account frozen check fails → dispatch error → rollback

**Recoverable?** Recoverable (auto). Same as H-1.

**Risk:** LOW-MEDIUM. Liquidations are brief. But if multiple accounts are
liquidated simultaneously, queue throughput degrades.

---

### H-4: Oracle goes stale or is unavailable

**Scenario:** The Pyth oracle for SOL/USD hasn't been updated in N slots.
Health computation in `health_region_end` fails because stale oracle data
is rejected.

**On-chain path:** Health cache creation fails → health_region_end fails → rollback

**Recoverable?** Recoverable (auto) — but the stall persists until the oracle
refreshes. If the oracle stays stale for minutes, the queue is blocked for
minutes. Every item in the queue is affected (all perp orders need the oracle).

**Risk:** CRITICAL for mainnet. A single stale oracle blocks ALL perp order
execution. This could cascade: bots keep submitting → queue fills → STALL-4.

**Prevention:**
- Off-chain: relayer should check oracle freshness before accepting intents
- On-chain: pre-dispatch oracle freshness check, clear item if oracle is stale
- Monitoring: alert when oracle age > threshold

---

### H-5: Order price is zero or negative

**Scenario:** Bug in bot code submits an order with `price_lots = 0`.

**On-chain path:** `require_gte!(price_lots, 0)` at dispatch (line 1261) —
if price is 0, the order may still pass this check but fail later in the
orderbook matching logic. If it fails inside the health region → rollback.

**Recoverable?** Recoverable (auto) — malformed orders are auto-dropped.

**Risk:** LOW. Bots should validate prices before submission.

---

### H-6: Self-trade with AbortTransaction behavior

**Scenario:** Two orders from the same owner match against each other.
`SelfTradeBehavior::AbortTransaction` causes the dispatch to fail.

**On-chain path:** Perp matching detects self-trade → returns error → if
health-gated, rollback.

**Recoverable?** Recoverable (auto). Auto-drop clears it.

**Risk:** LOW. Self-trade is usually caught off-chain.

---

### H-7: Position limit exceeded

**Scenario:** User's position would exceed the perp market's max position limit.

**On-chain path:** Dispatch fails at position limit check → rollback for
health-gated orders.

**Recoverable?** Recoverable (auto). Auto-drop clears it.

**Risk:** LOW-MEDIUM. Can happen with high-frequency bots that don't track
their own position size. Each failed order stalls the queue for ~5-15s.

---

### H-8: Reduce-only order when no position to reduce

**Scenario:** User submits a reduce-only order but their position was
already closed by a previous execution.

**On-chain path:** `reduce_only` check fails → dispatch error → rollback.

**Recoverable?** Recoverable (auto).

**Risk:** LOW. Common edge case with concurrent trading.

---

### H-9: CTM signer key rotation while items are in-flight

**Scenario:** Admin rotates the CTM signer via `execution_queue_set_ctm_pending`.
After `pending_ctm_activate_slot`, the new signer is active. Items signed with
the old key are still in the queue.

**On-chain path:** The execute function calls `maybe_activate_pending_ctm()`
before processing. Items already in the queue were verified at enqueue time
against the old signer. The execute dispatch does NOT re-verify the CTM signature.

**Recoverable?** Not a stall — items execute normally. The signature was checked
at enqueue, not at execute. **But this means old-signer items are still valid**
after rotation, which may be a security consideration.

**Risk:** LOW for stalls, MEDIUM for security (old signer items still executable).

---

### H-10: Ring buffer wraparound collision

**Scenario:** Queue processes slowly. After 1024 sequences, a new enqueue
at sequence N+1024 maps to the same ring buffer slot as sequence N, which
hasn't been cleared yet.

**On-chain path:** Enqueue checks `header.ctm_count >= CTM_CAPACITY` (line 1038).
If the ring buffer is full, enqueue is rejected with `ExecutionQueueFull`.

**Recoverable?** Not a stall — enqueue is rejected. The bot gets an error
and can retry later. **But from the user's perspective, their order is lost.**

**Risk:** MEDIUM. If the queue is full, no new orders can be placed.
Combined with STALL-4 (compute exceeded on full queue), this becomes a
total lockout.

**Prevention:** `queue_soft_limit` should be set well below `CTM_CAPACITY`
(e.g., 256 instead of 1024) to reject orders before the queue becomes
too full to crank.

---

### H-11: Relayer crash after submitting enqueue tx but before recording sequence

**Scenario:** The relayer sends an enqueue tx to Solana, then crashes before
writing the sequence to its local cursor file. On restart, it may assign
a duplicate or stale sequence number.

**On-chain path:** Duplicate sequence is rejected at enqueue with
`ExecutionQueueDuplicateSequence`. Stale sequence (lower than `max_seen`)
is also rejected.

**Recoverable?** Not a stall — the on-chain program protects against
duplicate/stale sequences. The relayer will advance its cursor on next
successful enqueue.

**Risk:** LOW. User's order may need to be re-submitted.

---

### H-12: Multiple relayers cranking the same queue

**Scenario:** Two relayer instances are running against the same execution
queue (e.g., during a deployment overlap).

**On-chain path:** Both send execute txs for the same head sequence.
One succeeds, the other fails (item already processed). The failing tx
is a wasted tx fee but doesn't stall.

**Recoverable?** Not a stall. The queue advances normally. But tx fees
are wasted and both relayers may get confused about pending dispatches.

**Risk:** LOW for stalls, MEDIUM for operational waste.

---

### H-13: Solana fork / rollback after confirmed execute tx

**Scenario:** An execute tx is confirmed at `confirmed` commitment level,
but a fork causes the slot to be rolled back. The on-chain state reverts
to before the execution.

**On-chain path:** The execution is undone. The item returns to `Pending`
status in the queue. The relayer's `confirmed_seq` watermark is now wrong
(it thinks the sequence was confirmed but it wasn't).

**Recoverable?** Partially — the executor will re-crank the item since
`next_sequence_to_execute` reverted. But the harness has optimistic state
that diverges from reality. The harness's on-chain reconciliation (every 10s)
will eventually detect the drift, but positions may be temporarily wrong.

**Risk:** LOW on mainnet (forks are rare). HIGH impact if it happens —
user sees confirmed trade that later disappears. **This is a fund-safety issue.**

**Prevention:** Use `finalized` commitment for critical state transitions.
The harness should track both `confirmed` and `finalized` watermarks.

---

### H-14: Priority fee too low on mainnet (tx not included)

**Scenario:** On mainnet, the relayer's `EXECUTOR_PRIORITIZATION_FEE` is
too low. Execute txs are submitted but never included in a block.

**On-chain path:** No stall on-chain — the item stays `Pending`. But the
queue doesn't advance because no execute tx lands.

**Recoverable?** Recoverable (auto) — the relayer will keep retrying.
The pending dispatch will timeout (PENDING_TIMEOUT_MS), and a new tx
with the same sequence will be sent. But if ALL txs fail to land, the
queue is effectively stalled.

**Risk:** HIGH on mainnet. Priority fee markets can spike. If the relayer
can't land txs for minutes, the queue backs up → STALL-4.

**Prevention:**
- Dynamic priority fee based on recent block fee levels
- Fee escalation: increase fee on each retry
- Alert when no execute tx has landed in >30s

---

### H-15: Blockhash expires before tx inclusion

**Scenario:** The relayer uses a blockhash that expires (~90s on Solana)
before the tx is included.

**On-chain path:** Tx is rejected by the validator. No on-chain effect.

**Recoverable?** Recoverable (auto) — relayer refreshes blockhash and
retries. The `blockhash_refresh_ms` config controls refresh frequency.

**Risk:** LOW if blockhash refresh is configured properly. If the
blockhash cache is broken (e.g., RPC connection lost), ALL txs fail.

---

### H-16: Concurrent health region from another instruction

**Scenario:** Another program or instruction puts the same mango account
into a health region concurrently. The execution queue's
`health_region_begin` fails because the account is already in a region.

**On-chain path:** `health_region_begin` fails (line 801) → retry
counter incremented → cleared after MAX_RETRIES (1).

**Recoverable?** Recoverable (native) — retried once, then cleared.
**This is one of the few health-gated failures that IS recoverable**
because it fails at region_begin (before dispatch), not at dispatch.

**Risk:** LOW. Concurrent health regions are rare in practice.

---

### H-17: Token bank frozen for the quote/base token

**Scenario:** Admin freezes the USDC bank (e.g., during a security incident).
Health computation fails because token positions can't be evaluated.

**On-chain path:** Health cache creation fails → health_region_end fails →
or health_region_begin fails. If begin fails: retried then cleared. If
end fails: rollback → permanent stall until auto-drop.

**Recoverable?** Depends on which health region step fails.
- health_region_begin: Retried then cleared (recoverable)
- health_region_end: Rollback → auto-drop needed

**Risk:** MEDIUM. Bank freezes are admin actions, should be coordinated
with queue draining.

**Prevention:** Before freezing a bank, pause queue ingress and drain
existing items.

---

### H-18: Mango account closed while order is in queue

**Scenario:** User closes their mango account (via `account_close`) while
they still have orders in the execution queue.

**On-chain path:** Dispatch tries to access the account → account not found
or invalid → dispatch error → rollback for health-gated orders.

**Recoverable?** Recoverable (auto) — auto-drop clears it.

**Risk:** LOW. `account_close` should check for pending queue items.
**TODO:** On-chain check at `account_close` to reject if queue items exist.

---

### H-19: Event log file grows unbounded → disk full

**Scenario:** The harness event log (`continuum-harness-*.jsonl`) grows
without rotation. Disk fills up. Harness can't write new events.

**On-chain path:** No on-chain impact. But the harness loses event history.
On next restart, the event log replay is incomplete → STALL-3 (lane mismatch).

**Recoverable?** Not a stall itself, but causes STALL-3 on restart.

**Risk:** MEDIUM over time.

**Prevention:** Log rotation with size cap. Keep last N MB of events.

---

### H-20: gap_wait_slots set to 0

**Scenario:** Admin configures `gap_wait_slots = 0`. The gap-skip mechanism
fires immediately when a sequence slot is empty.

**On-chain path:** For missing sequences (gaps), items are skipped instantly.
**But for health-gated failures (non-empty slots that fail), gap_wait_slots=0
has no effect** — gap-skip only works on empty slots.

**Recoverable?** Gap-skip fires faster for actual gaps, but doesn't help
with health-gated stalls. Could cause FIFO violations if a legitimate order
arrives slightly late (1-2 slots) and is skipped before inclusion.

**Risk:** HIGH for FIFO fidelity. Orders could be skipped before they
arrive on-chain, breaking the fairness guarantee.

**Prevention:** `gap_wait_slots` should be set to p99 inclusion latency.
For SWQoS: measure actual inclusion distribution, set to p99 value.

---

### H-21: gap_wait_slots set too high (e.g., 100)

**Scenario:** `gap_wait_slots = 100` (~40s). A missing sequence (legitimate
gap from a failed enqueue) blocks the queue for 40s before being skipped.

**On-chain path:** The gap-skip timer doesn't fire until 100 slots pass.
During this time, ALL orders behind the gap are blocked.

**Recoverable?** Yes, after the timeout. But 40s is a long time for a DEX.

**Risk:** MEDIUM. Trade-off between FIFO fidelity and recovery latency.

---

### H-22: Bot submits cancel order for non-existent order

**Scenario:** Bot sends a `PerpCancelOrder` for a `client_order_id` that
doesn't exist (already cancelled or never placed).

**On-chain path:** Cancel is NOT health-gated. Dispatch fails → retry
counter incremented → cleared after MAX_RETRIES.

**Recoverable?** Recoverable (native) — retried then cleared.

**Risk:** NONE. Cancel orders are non-health-gated and handle gracefully.

---

### H-23: Two matching orders from different users arrive simultaneously

**Scenario:** A bid and an ask at crossing prices arrive in consecutive
sequences. The first one places on the book, the second matches against it.

**On-chain path:** Both are executed in sequence order. The match happens
during the second order's dispatch. Health regions are per-order.

**Recoverable?** Not a stall — this is normal operation.

**Risk:** NONE. But if the first order stalls (for any reason above),
the matching second order is also blocked.

---

### H-24: Relayer submits wrong accounts for a queue item

**Scenario:** The relayer constructs an execute tx with incorrect
remaining_accounts that don't match the item's `accounts_hash`.

**On-chain path:** The execute function checks `accounts_hash` against
the provided accounts (line ~1300). Mismatch → the item is not dispatched,
the loop continues to the next item. No stall.

**Recoverable?** Not a stall — the on-chain program rejects the wrong
accounts and tries the next item. The relayer should refresh its lane.

**Risk:** LOW. The C-1 hash integrity fix prevents malicious account substitution.

---

### H-25: On-chain program upgrade while queue has pending items

**Scenario:** The Solana program is upgraded (via BPF upgrade authority).
Queue items were serialized with the old program's payload format.

**On-chain path:** If the new program changes payload deserialization,
`decode_queue_payload` fails → item is cleared immediately (line 1375-1391).

**Recoverable?** Recoverable (native) — decode failures are cleared
immediately. **But all pending orders are lost.** Users whose orders
were in the queue at upgrade time lose their orders without execution.

**Risk:** HIGH for users. **All pending queue items become invalid on upgrade.**

**Prevention:** Before upgrading, drain the queue completely:
1. Pause ingress
2. Wait for all items to execute
3. Upgrade program
4. Unpause ingress

---

### H-26: Admin pauses execute but forgets to unpause

**Scenario:** Admin pauses execute for maintenance but doesn't unpause.
New orders keep being enqueued but nothing executes.

**On-chain path:** `execution_queue_execute` checks `paused_execute != 0`
and returns early. Queue fills up.

**Recoverable?** Recoverable (manual) — admin unpauses.
But if the queue fills up during the pause → STALL-4 on unpause.

**Risk:** MEDIUM. Operational error.

**Prevention:** Monitoring: alert if `paused_execute` is set for >60s.

---

### H-27: Network partition between relayer and Solana validators

**Scenario:** The relayer can reach the RPC but the RPC can't propagate
txs to validators (e.g., RPC is on a stale fork).

**On-chain path:** Txs are accepted by RPC but never included.
Same as H-14 but harder to detect.

**Recoverable?** Recoverable once partition heals. But during partition,
queue backs up. If partition lasts long enough → STALL-4.

**Risk:** MEDIUM. Hard to detect from the relayer's perspective.

**Prevention:** Monitor tx inclusion rate. If 0 txs included in 30s,
alert and potentially switch to backup RPC.

---

### H-28: Integer overflow in position calculation

**Scenario:** A series of large orders causes the perp position to
overflow i64 (base_position_lots) or the quote position to overflow.

**On-chain path:** Checked arithmetic would fail → dispatch error → rollback.
Unchecked would produce wrong values → health check might fail or pass
incorrectly.

**Recoverable?** If it causes a dispatch error: auto-drop clears it.
If it causes a silent wrong value: **fund safety issue** — positions
are wrong, health checks pass when they shouldn't.

**Risk:** LOW (positions are i64, overflow requires ~9.2 quintillion lots).
But worth auditing the arithmetic paths.

---

### H-29: Execution queue account runs out of rent-exempt balance

**Scenario:** Unlikely for a fixed-size account, but if Solana changes
rent rules or the account is partially defunded.

**On-chain path:** Account becomes invalid. All operations fail.

**Recoverable?** Recoverable (manual) — re-fund the account.

**Risk:** VERY LOW. Fixed-size accounts have rent calculated at creation.

---

### H-30: Harness and relayer disagree on sequence numbering

**Scenario:** The harness's `optimistic_seq` watermark diverges from the
relayer's sequence cursor (e.g., relayer restarts with stale cursor).

**On-chain path:** No direct stall. But the harness shows incorrect
watermarks, and users may see orders in wrong positions.

**Recoverable?** Not a stall. Harness self-corrects via reconciliation.

**Risk:** LOW for queue, MEDIUM for UX (users see stale data).

---

### Summary: Permanently Unrecoverable Scenarios

| # | Scenario | Why unrecoverable | Fix needed |
|---|----------|-------------------|------------|
| H-4 | Oracle stale for extended period | Every order fails health check; auto-drop clears them one by one but new orders keep failing too | Pre-dispatch oracle check; reject orders when oracle stale |
| H-10+STALL-4 | Queue full + compute exceeded | Can't enqueue (full) AND can't execute (compute) | `queue_soft_limit` well below capacity; CU optimization |
| H-13 | Solana fork rollback | Confirmed state reverted; harness/user sees phantom trades | Use `finalized` commitment; reconciliation |
| H-25 | Program upgrade with items in queue | All pending items become undeserializable → cleared as failed | Drain before upgrade; migration path |

### Scenarios Requiring Auto-Drop (5-15s stall window)

All health-gated dispatch failures rely on the off-chain auto-drop mechanism.
If auto-drop is working, recovery takes 5-15s. If auto-drop is broken
(STALL-2 or STALL-5), the stall is indefinite.

| H-1 | H-2 | H-3 | H-4 | H-5 | H-6 | H-7 | H-8 | H-17 | H-18 |
|-----|-----|-----|-----|-----|-----|-----|-----|------|------|
| Bad health | Market paused | Account frozen | Oracle stale | Bad price | Self-trade | Position limit | Reduce-only | Bank frozen | Account closed |

**All 10 of these share the same root cause:** health-gated dispatch failure →
tx rollback → retry counter not incremented → auto-drop needed.

**Mainnet impact:** A 5-15s stall per bad order. If bad orders arrive at 2/sec
and auto-drop takes 5s each, the queue falls behind by 10 orders per bad order.
This can cascade.

**Critical TODO:** The on-chain program should clear health-gated items that
fail dispatch WITHOUT rolling back. The proposed pre-dispatch checks (expiry,
oracle freshness, account status) would eliminate most of these at near-zero cost.
