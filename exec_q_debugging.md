# Execution Queue Debugging Rolodex

Quick reference for diagnosing stuck / slow execution queues on Fermi devnet.

---

## How to read the queue state

```bash
# On-chain head + count
EXECUTION_QUEUE_PK=8J7vAomtCVabazRNs8XH4BF3w4BVP852QoASg9yrXUaa \
EXECUTION_QUEUE_GROUP_PK=Cj8vUC2nWbREhofnD3iWk4j8CD9Fo6j9c33M5ZFKLVPB \
PROGRAM_ID=Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt \
CLUSTER_URL_OVERRIDE='https://devnet.helius-rpc.com/?api-key=...' \
npx ts-node ts/client/scripts/execution-queue/inspect-queue-head.ts

# Live engine log tail
tail -f .devnet/logs/ctm-relayer.log | grep -E "queue_count|tx failed|auto-dropped|admin recovery"
```

Key fields in `inspect-queue-head` output:
- `count` / `ctmCount` — total items queued
- `next` / `max` — head and tail sequence numbers
- `accountsHash` — same hash across many items = single account flooding
- `decoded.order_type` — 4 = PostOnly, 3 = ImmediateOrCancel

---

## Failure Mode 1: `Custom(6007)` — HealthMustBePositiveOrIncrease

**Symptom**
```
executor tx failed sequence=N err=Some(InstructionError(2, Custom(6007)))
executor auto-dropped terminal head sequence=N reason=health_check_failed
```
Queue drains 1 item at a time. Queue count grows faster than it drains (100s–1000s of items accumulate).

**Cause**
A user account's `init_health` is negative or would go negative after placing the order. Common triggers:
- Account funded just enough to submit but not enough to place (init_health < margin requirement)
- `depositWeightScaleStartQuote` set too low → USDC collateral haircut reduces effective health
- Large perp position moving against the account between submission and execution

**Check**
```bash
# Check account health via TS client
npx ts-node -e "
  // ... connect ...
  const acc = await client.getMangoAccount(new PublicKey('ACCOUNT_PK'));
  console.log({
    init_health: acc.getHealth(group, 'Init').toNumber() / 1e6,
    maint_health: acc.getHealth(group, 'Maint').toNumber() / 1e6,
    equity: acc.getEquity(group).toNumber() / 1e6,
  });
"
```

**Fix**
1. **Immediate (if queue is large):** Batch-drop all items from the offending account using `admin-drop-head.ts` in a loop (10 drops/tx, ~90 txs for 900 items).
2. **If account is underfunded:** Airdrop + deposit USDC to the account via `/airdrop` endpoint then direct `client.tokenDeposit()`.
3. **If `depositWeightScaleStartQuote` is too low:** Run `tokenEdit` on the USDC bank to set `depositWeightScaleStartQuote = 1e18`.
4. **Long-term:** Add pre-enqueue health gate in the relayer — reject orders from accounts with insufficient init_health before they enter the queue.

**Auto-recovery**
Engine detects `Custom(6006)` or `Custom(6007)` → classifies as `TerminalHeadFailureReason::HealthCheckFailed` → drops head automatically. But only 1/drop, so large backlogs must be batch-dropped manually.

---

## Failure Mode 2: `Custom(6000)` — no free perp order index

**Symptom**
```
executor tx failed sequence=N err=Some(InstructionError(2, Custom(6000)))
```
Queue fully blocked. The head account's mango account has `perpOoCount=0` or all OO slots are filled.

**Cause**
Mango account was created with `perp_oo_count=0` (no open order slots). Common when bot accounts are provisioned without calling `expandMangoAccount`. Note: TS client may show 64 slots (from parsing the fixed-size array) but raw bytes show 0 actual slots — check with on-chain raw decode.

**Check**
```bash
# Check perpOoCount via raw account bytes (offset 0x... in the mango account layout)
# Or check if perpOrdersActive() returns items despite perpActive() being empty
```

**Fix**
Call `expandMangoAccount` on the offending account to add 32 perp OO slots:
```typescript
await client.expandMangoAccount(group, mangoAccount, 0, 32, 0, 0);
```
Then batch-drop or let the engine retry (it will add the account to `blocked_mango_accounts` cache for 60s if `perp_order_slots_full` is detected).

**Auto-recovery**
Engine detects full OO slots → adds account to `blocked_mango_accounts` cache (60s block) → skips execute and batch-drops up to 8 items directly.

---

## Failure Mode 3: `ProgramFailedToComplete` + `sequence_failure_threshold`

**Symptom**
```
executor tx failed sequence=N err=Some(InstructionError(2, ProgramFailedToComplete))
executor admin recovery ... reason=sequence_failure_threshold
```
Queue drains at ~1 item/2s (hits `EXECUTION_QUEUE_CRANK_MAX_SEQUENCE_FAILURES=2` retry limit then drops).

**Cause**
**Confirmed root cause (2026-04-07):** BPF heap overflow in `ExecutionQueueExecuteMulti`. The engine batches up to `EXECUTION_QUEUE_CRANK_MAX_ITEMS=6` items per tx. When all 6 items are `perpPlaceOrder` for the same account+market, each order allocates heap for orderbook tree ops and health recomputation. Six in a row exhaust the 32KB BPF heap:
```
Program log: ask on book order_id=... quantity=290 price=81759
Program Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt failed:
  Access violation in heap section at address 0x300008000 of size 1
```
`0x300008000` = one byte past the 32KB heap region at `0x300000000`.

Typically seen as a *second phase* after Failure Mode 1: once health is restored (e.g. via airdrop) and 6007s stop, orders start executing, the multi-execute path kicks in for a backed-up single-account queue, and heap overflows immediately.

Other possible triggers (less common):
- Compute budget exhausted on a deep orderbook
- Oracle account stale or missing from remaining accounts

**Check**
```bash
# Check open order slot usage on the account
npx ts-node -e "
  const acc = await client.getMangoAccount(...);
  console.log({ open_orders: acc.perpOrdersActive().length, perp_oo_count: acc.perpOpenOrders?.length });
"
```

**Fix**
1. Queue drains automatically via `sequence_failure_threshold` (2 retries → drop). ~1 item/2s.
2. To clear faster: batch-drop all items from the offending account.
3. **Root fix for heap overflow:** reduce `EXECUTION_QUEUE_CRANK_MAX_ITEMS` (e.g. to 3) to halve heap pressure per tx. Alternatively, fix the program to use less heap per order or increase heap with `sol_alloc_free` extension.

To diagnose heap vs other causes, fetch the failed tx and look for `Access violation in heap section at address 0x300008000`.

**Auto-recovery**
Yes — `MAX_SEQUENCE_FAILURES=2` threshold fires after 2 failures on same sequence, drops head. No manual intervention needed unless queue is very large (>200 items, ~6+ min to drain).

---

## Failure Mode 4: Queue paused (`paused_ingress` or `paused_execute`)

**Symptom**
New orders return `RESOURCE_EXHAUSTED: execution queue backpressure` even when queue appears empty. No new items can enter or execute.

**Cause**
- `paused_ingress=255` set by relayer when soft limit hit (`CTM_RELAYER_QUEUE_SOFT_LIMIT=1020`)
- `paused_execute` set manually or by an admin instruction

**Check**
Look for `execution_queue_configure` events in logs, or inspect raw queue account bytes:
- Byte 16 = `paused_ingress`
- Byte 17 = `paused_execute`

**Fix**
Send `execution_queue_configure` instruction with `pause_ingress=0, pause_execute=0`. This is done via the admin keypair.

---

## Failure Mode 5: Queue soft-limit backpressure (`RESOURCE_EXHAUSTED count=1020`)

**Symptom**
```
queue backpressure, backing off 3s  error: 8 RESOURCE_EXHAUSTED: execution queue backpressure count=1020 soft_limit=1020
```
Bots get rate-limited. New orders rejected at the relayer gate.

**Cause**
Queue count hit `CTM_RELAYER_QUEUE_SOFT_LIMIT=1020`. The relayer stops accepting new items via gRPC. This is a symptom of one of the above failure modes filling the queue.

**Fix**
Drain the queue (see Failure Modes 1–3). Once count drops below soft limit, backpressure releases automatically.

---

## Batch-drop procedure (when queue is large)

Use when the queue has 100+ stuck items from a single account:

```bash
cd /home/hetalkenaudekar/stagin4/mng-v4

# Check current queue size first
EXECUTION_QUEUE_PK=... npx ts-node ts/client/scripts/execution-queue/inspect-queue-head.ts

# Drop head in a loop (10 items/tx)
# Edit admin-drop-head.ts DROP_COUNT as needed, then run repeatedly:
for i in $(seq 1 120); do
  npx ts-node ts/client/scripts/execution-queue/admin-drop-head.ts && sleep 1
done
```

Note: `admin-drop-head.ts` requires `pause_execute=true` at the queue level. Set that first via `execution_queue_configure`, run drops, then unpause. The engine has a fast-path that atomically does `pause → drop(N) → unpause` in a single tx.

---

## Key accounts and env

| Name | Value |
|------|-------|
| Group | `Cj8vUC2nWbREhofnD3iWk4j8CD9Fo6j9c33M5ZFKLVPB` |
| Execution Queue | `8J7vAomtCVabazRNs8XH4BF3w4BVP852QoASg9yrXUaa` |
| Program ID | `Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt` |
| USDC Mint | `BBf5TvMhDG3rA8WuNV8xFxZoi2qZ9d5QTouDkeR1ha66` |
| Relayer log | `.devnet/logs/ctm-relayer.log` |
| Harness log | `.devnet/logs/continuum-harness.log` |
| Engine HTTP | `http://127.0.0.1:9093` |
| Harness HTTP | `http://127.0.0.1:9091` |

## Known problematic accounts (as of 2026-04)

| Account | Owner | Issue |
|---------|-------|-------|
| `5jDcx19MKSgp3M23JQmaqjRfjKLnSerT8CTyGELyZdaW` | `3bRyWQ5pDQU7cXowskTWZpvp39UuZ6gjYDfq24xPMztJ` | Repeatedly floods queue — thin health → 6007, then OO slots full → ProgramFailedToComplete. Airdropped 10k USDC on 2026-04-07 to stabilize. |
