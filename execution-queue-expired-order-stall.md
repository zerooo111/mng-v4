# Execution Queue Stall: Expired Health-Gated Orders

## Incident Summary

512 expired `PerpPlaceOrderV2` orders accumulated in the on-chain execution queue,
blocking all new order processing. The queue could not advance because:

1. **On-chain**: No built-in drain for expired health-gated items
2. **Off-chain**: The relayer's auto-drop recovery failed due to RPC rate-limiting

---

## Root Cause Analysis

### Where the stall occurs

**Both on-chain AND off-chain**, in a compounding failure:

| Layer | Component | Failure |
|-------|-----------|---------|
| On-chain | `execution_queue_execute_multi` | Expired health-gated items roll back the entire tx; retry counter never increments |
| Off-chain | Executor `reconcile_pending_dispatches` | Signature status poll fails (RPC rate-limited), so `maybe_auto_drop_expired_head` never fires |

### On-chain failure chain (primary deficiency)

**File**: `programs/mango-v4/src/instructions/execution_queue.rs`

1. **Enqueue** (line ~1020): The CTM envelope expiry check passes — `envelope.expires_at_slot`
   is a *slot-based* envelope TTL, not the order's Unix `expiry_timestamp`. An order's
   timestamp can expire while the envelope is still valid.

2. **Execute dispatch** (line ~1456): `dispatch_queue_payload()` calls `perp_place_order_v2`,
   which calls `Order::tif_from_expiry(expiry_timestamp)`. This returns `None` because
   `expiry_timestamp < now_ts`. The function logs `"Order is already expired"` and returns
   `Err(SomeError)`.

3. **Post-dispatch failure** (line ~1491): Since `PerpPlaceOrderV2` is health-gated
   (`item_health_region.is_some()`), the code at line 1511 does `return dispatch_result` —
   **rolling back the entire transaction**. The retry counter increment at line 1515 is
   unreachable for health-gated items that fail during dispatch.

4. **Pre-dispatch retry check** (line ~1417): Checks `candidate.retries >= EXECUTION_QUEUE_MAX_RETRIES`.
   But `retries` is always 0 because the rollback undoes any increment. This check never triggers.

5. **Gap-skip logic** (line ~1321): Only triggers when `queue.current_ctm_head().is_none()` —
   i.e., the slot is *empty*. Expired items have `status == Pending`, so `current_ctm_head()`
   returns `Some(item)`. Gap-skip never fires.

6. **Result**: The queue head is permanently stuck. Every execute tx sends, fails, rolls back.
   The on-chain `next_sequence_to_execute` never advances.

### Off-chain failure chain (secondary, masking the workaround)

**File**: `bin/service-mango-execution-engine/src/main.rs`

The relayer has a working auto-drop mechanism (`maybe_auto_drop_expired_head`, line 1154)
that builds an admin tx: pause-execute -> drop-ctm -> unpause. However:

1. **Tx spam** (line ~1937): `execute_once()` runs in a tight loop at `CRANK_INTERVAL_MS=25ms`.
   With `CRANK_BUSY_INTERVAL_MS=1ms`, the relayer sends ~40 txs/second to devnet for the
   same stuck sequence.

2. **RPC rate-limit** (observed in logs): Helius devnet returns `ENHANCE_YOUR_CALM` /
   `too_many_internal_resets`. The `getSignatureStatuses` call fails.

3. **No status = no recovery**: Without signature confirmation, the pending dispatch is
   never reconciled. `maybe_auto_drop_expired_head` requires a confirmed-failed tx to inspect
   its logs for the `"Order is already expired"` string.

4. **Lane failure backoff is per-lane, not per-sequence**: `apply_executor_lane_failure`
   backs off the *lane* for 10s, but the executor just tries a different lane. No per-sequence
   failure count exists, so the same stuck sequence is retried indefinitely.

---

## On-Chain Deficiency: No Drain for Expired Health-Gated Items

### The gap

The execution queue has these mechanisms for stuck items:

| Mechanism | Works for expired health-gated items? | Why not? |
|-----------|---------------------------------------|----------|
| Gap-skip (`gap_wait_slots`) | No | Only skips *empty* slots (`current_ctm_head().is_none()`) |
| Pre-dispatch retry clear | No | Retries=0 always (tx rollback undoes increment) |
| Post-dispatch non-health-gated clear | No | Health-gated items take the `return dispatch_result` path |
| `execution_queue_drop_ctm` admin ix | Partial | Requires `paused_execute != 0` — needs admin tx |
| Health-region-begin failure retry | No | Expiry fails *during dispatch*, not during health-region begin |

**There is no mechanism that drains expired items without an admin transaction.**

### Proposed on-chain fix

Add a **pre-dispatch expiry check** for health-gated CTM items, before attempting dispatch.
This clears expired orders without entering the health region, so no rollback occurs.

**Location**: `execution_queue.rs`, after line 1411 (after `let item_health_region = ...`),
before the existing pre-dispatch retry check.

```rust
// --- FIX: Pre-dispatch expiry check for health-gated CTM items ---
// Expired PerpPlaceOrderV2 orders fail during dispatch inside a health region,
// causing a full tx rollback. The retry counter is never incremented because
// the rollback undoes it. This check clears expired items BEFORE dispatch,
// avoiding the health-region rollback trap.
if item_health_region.is_some() && candidate.is_ctm {
    if let QueuePayloadBody::PerpPlaceOrderV2(ref place) = decoded_payload.body {
        let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
        if place.expiry_timestamp != 0 && place.expiry_timestamp <= now_ts {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            queue.clear_ctm_item_at(candidate.sequence);
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            msg!("execution_queue: expired health-gated item cleared pre-dispatch seq={}", candidate.sequence);
            continue;
        }
    }
    // Same check for PerpPlaceOrderV2Pegged
    if let QueuePayloadBody::PerpPlaceOrderV2Pegged(ref place) = decoded_payload.body {
        let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
        if place.expiry_timestamp != 0 && place.expiry_timestamp <= now_ts {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            queue.clear_ctm_item_at(candidate.sequence);
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            msg!("execution_queue: expired pegged order cleared pre-dispatch seq={}", candidate.sequence);
            continue;
        }
    }
}
// --- END FIX ---
```

Apply the same fix in `execution_queue_execute_multi` (after the equivalent line ~1717).

### Why this is safe

- The check runs *before* entering any health region — no state mutation to roll back
- `clear_ctm_item_at` is the same function used by the existing retry-exhaustion path
- The `QueueItemProcessed` event with `Failed` status matches existing semantics
- Only affects items that would unconditionally fail dispatch anyway

---

## Off-Chain Fix: Relayer Executor Improvements

### Problem 1: No per-sequence failure tracking

The executor tracks failures per-lane but not per-sequence. A permanently-failing
sequence bounces between lanes forever.

**Fix**: Add a `HashMap<u64, u32>` tracking consecutive failures per `next_sequence`.
After `EXECUTOR_MAX_SEQUENCE_FAILURES` (default: 5), trigger `drop_ctm_head_with_admin_tx`
proactively without waiting for log inspection.

### Problem 2: Tx send rate overwhelms RPC

At `CRANK_INTERVAL_MS=25, CRANK_BUSY_INTERVAL_MS=1`, the executor sends 40+ txs/second
for a stuck sequence, rate-limiting itself out of signature status polling.

**Fix**: After a dispatch is pending (sent but not confirmed), the executor should
**not send another tx for the same sequence**. The existing `pending_dispatches` check
should gate new sends, but the `MATCH_HEAD_ONLY=true` + `SKIP_PREFLIGHT=true` config
allows blind re-sends. Add an explicit check: if a pending dispatch exists for the
current `next_sequence`, wait for confirmation before re-sending.

### Problem 3: Status poll failure silently drops the recovery path

When `getSignatureStatuses` fails, the pending dispatch stays in limbo. No retry
of the status check occurs until the next execute loop, which sends yet another tx.

**Fix**: On status poll failure, retry the status check 3 times with exponential
backoff before giving up. If all retries fail, simulate the tx locally to check
for the expired-order error pattern, and trigger auto-drop without needing the
on-chain tx logs.

---

## Permanent Remedy (Both Layers)

### Layer 1: On-chain (program upgrade required)

Deploy the pre-dispatch expiry check described above. This is the **definitive fix** —
expired orders are cleared in O(1) per execute call without rollback. The queue
self-heals regardless of off-chain behavior.

### Layer 2: Off-chain (relayer config + code, no program upgrade)

1. Add per-sequence failure counter to `ExecutorState`
2. After 5 consecutive failures for the same `next_sequence`, fire admin-drop
3. Rate-limit execute sends: max 1 pending tx per sequence at a time
4. Add simulation-based expired-order detection as fallback when log fetch fails
5. Set `EXECUTION_QUEUE_CRANK_INTERVAL_MS=500` (from 25) for devnet to stay within
   RPC rate limits

### Layer 3: Bot configuration (immediate)

- Set `QUOTER_ORDER_EXPIRY_SECS=0` (no expiry) or a much larger value (300+)
  to prevent orders from expiring while queued
- This eliminates the root trigger for the stall

---

## Immediate Unblock (No Code Changes)

To unblock the current stalled queue without deploying code:

```bash
# 1. Stop relayer and bots
screen -S relayer -X quit
screen -S quoter -X quit

# 2. Reset the sequence cursor past the stalled region
echo '{"9n527U6Q5fXAecZ4WCrcgKJCR37nrLNYb8vRbXg6MGTH:0": 580}' > \
  ~/stagin4/mng-v4/.devnet/run/ctm-sequences-9125.json

# 3. Increase crank interval to avoid RPC rate-limiting
export EXECUTION_QUEUE_CRANK_INTERVAL_MS=500

# 4. Restart relayer — gap-skip will eventually advance past stale items
# 5. Restart bots with no-expiry orders
export QUOTER_ORDER_EXPIRY_SECS=0
```

---

## TODO

- [ ] On-chain: **Reject already-expired orders at enqueue time** — `execution_queue_enqueue_ctm`
      (line ~1020) only checks `envelope.expires_at_slot` (slot-based envelope TTL), not the
      order's `expiry_timestamp` (Unix time). Decode the payload at enqueue and reject if
      `expiry_timestamp != 0 && expiry_timestamp <= clock.unix_timestamp`. This prevents
      expired orders from ever entering the queue. Location: `execution_queue.rs` after
      line 1021, before the item is written to the ring buffer.
- [ ] On-chain: Implement pre-dispatch expiry check in `execution_queue_execute` and `execution_queue_execute_multi`
- [ ] On-chain: Add unit test for expired-health-gated-item drain
- [x] Off-chain: Add per-sequence failure counter to `ExecutorState` *(done 2026-03-24)*
- [ ] Off-chain: Gate execute sends on pending dispatch confirmation
- [ ] Off-chain: Add simulation-based expired-order detection fallback
- [x] Off-chain: Tune `CRANK_INTERVAL_MS` / `PENDING_TIMEOUT_MS` / `STATUS_POLL_MS` for devnet *(done 2026-03-24)*
- [x] Bots: Default `QUOTER_ORDER_EXPIRY_SECS=0` to prevent queue stalls *(done 2026-03-24)*
- [x] Bots: Startup account check + SOL transfer from payer + harness airdrop-deposit *(done 2026-03-24)*

---

## Additional Stall Scenarios Discovered

### Stall: Lane mismatch after event log clear

**Trigger:** Clearing the harness event log (`continuum-harness-*.jsonl`) removes the
dynamic lane history. The executor builds lanes from this log. With no history,
the executor has no matching lane for items already in the on-chain queue,
resulting in `reason=no_lane_match` indefinitely.

**Fix needed:** The executor should fall back to reading the queue item's `accounts_hash`
directly from on-chain and building a matching lane from the account layout, rather than
requiring pre-existing lane definitions from the event log.

### Stall: ProgramFailedToComplete (compute exceeded) on large queues

**Trigger:** When the on-chain queue has 400+ items (`ctm_count >= ~400`), the
`execution_queue_execute_multi` instruction exceeds the 900,000 CU compute budget.
The queue ring buffer scan to find and dispatch the head item becomes O(n) on queue
depth, consuming all available compute.

**Symptoms:** Relayer sends execute txs, all fail with `ProgramFailedToComplete`.
Queue cannot advance. New items keep being enqueued, making the problem worse.

**Root cause:** The execute instruction scans the ring buffer starting from the head
position. With a full 1024-slot ring buffer, the scan must skip many items.
Additionally, the health-region check, CPI to perp_place_order, and account
deserialization all consume significant CU on top of the scan.

**Fix needed (on-chain):** Optimize the ring buffer scan — use direct indexing
(`sequence % capacity`) instead of linear scan. The head item is always at
`ctm_items[next_sequence_to_execute % CTM_CAPACITY]`, so no scan should be needed.

**Workaround (off-chain):** Rate-limit bot order submission to match the executor's
crank rate (`submit_rate <= crank_rate`) so the queue never fills up. For devnet with
5 sendTx/sec limit: 1 crank/sec + 1 order/sec = safe.
