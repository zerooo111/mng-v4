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

## Failure Mode 6: Empty orderbook despite reveals returning `status=Revealed`

**TL;DR**: If the orderbook is empty, `trade_count_24h = 0`, and head is still advancing, the reveals aren't actually placing orders — they're either (1) terminalizing with a `failure_code` that still emits `status=Revealed` tag in older builds, (2) the intents never reached the queue (bot is using a disabled legacy path), or (3) the harness is optimistically reporting fill/book state that the chain never confirmed.

**Real case (2026-04-20, devnet group `BrbJtc8ja8…`)**: 12 h of empty M2-PERP book with ~3 commits/s and quoter running. Reveals reported `err:None` at the tx level. The failure chain was three-deep:

### Layer 1 — `OracleStale` on `health_region_begin`

The `err:None` at tx level is misleading: the v5 reveal handler catches `OracleStale` inside `queue_health_region_begin`, classifies it as a terminal dispatch failure, increments retry counter, and (on retries-exhausted) calls `clear_head` — the tx returns `Ok` because the terminalize path is a deliberate operation. Look at the program logs, not the tx-level err:

```
v5 reveal seq=836 market_index=2 health_region_begin failed (terminal=true):
AnchorError { error_name: "OracleStale",
  error_msg: "name: USDC, price: 0.9998…, last_update_slot: 456901842,
              now_slot: 456902531, conf_filter: 0.1" }
v5 reveal seq=836 terminalizing health-region failure
  (retries=1/3, terminal=true, code=OracleStale) — clearing head
```

689 slots lag on USDC oracle vs `max_staleness_slots = 600` → stale by 89 slots. Devnet sponsored Pyth feeds update on a highly variable cadence (observed publish_time gaps of 100–800 slots), so `600` was too tight.

**Fix**: bump `max_staleness_slots` on both USDC bank and perp markets via `tokenEdit` / `perpEditMarket`. Devnet script: `ts/client/scripts/execution-queue/bump-staleness-all.ts` with `ORACLE_MAX_STALENESS_SLOTS=1800`. Verify post-update by reading the raw account bytes (the TS SDK's `group.reloadAll` cache can lag):

```bash
python3 -c "
import json, base64, base58, struct, subprocess, os
rpc = os.environ['CTM_RELAYER_SECONDARY_RPC_URL']
for pk in ['68MXF7CmVyYUhe1d994BJV1qnLRw9yHgkAFbcz3Rpjgy',  # USDC bank
           'FuDTET146QJzSpdrUFvSQNxhdk52a9CifctBNNTyvPna']: # SOL perp
    # ... scan for oracle pubkey, read OracleConfig.max_staleness_slots at offset +32+16 ..."
```

### Layer 2 — Quoter targeting disabled legacy path

After fixing staleness, reveals stopped terminalizing but the book was still empty. Queue head didn't advance. Relayer logs showed the real issue — quoter txs were failing at **RPC preflight** (so they never land on chain, never show up in `getSignaturesForAddress` for the queue PDA):

```
bg submitter hard failure sequence=0 sig=jUwNRo…
  err=RpcError … custom program error: 0x17ee
  logs: ["Instruction: ExecutionQueueEnqueueCtm",
         "AnchorError: LegacyQueuesDisabled. Error Number: 6126.
          legacy execution queue (v3/v4) instructions are disabled
          in this build."]
```

The `random-sol-usdc-quoter-bot.ts` was reading `perpMarketIndex: 0` from `.devnet/run/execution-queue-e2e-<group>.json` — market_index 0 is the abandoned v4 SOL-PERP (`E7mxfLL…`), which the relayer routes through the v3/v4 `ExecutionQueueEnqueueCtm` path. Post v5 migration that instruction errors with `LegacyQueuesDisabled (6126)`.

**Fix**: update the config file — `perpMarketIndex: 0` → the actual v5 market index (2 for SOL, 3 for ETH, 4 for BTC). The relayer then routes through `submit_intent_v5` and the quoter gets real sequence numbers (`sequence: "839"` instead of `"0"`).

### Layer 3 — Harness optimistic lie

Even while quoter enqueues were preflight-failing with `LegacyQueuesDisabled`, `.devnet/logs/ctm-relayer.log` contained lines like:

```
ask on book order_id=1598539501195448614388196 quantity=2632 price=86657
```

and `GET /state/markets/2?view=optimistic` reported `bids` / `asks` arrays populated. The harness was marking the order as "on book" as soon as the relayer returned `status=submitted` (tx signature assigned) — it did **not** wait for the enqueue tx to actually confirm. Preflight failures at the RPC never fed back into the optimistic view.

This is a **trap for debugging**: if you believe the harness, you'll conclude orders are landing and look for reasons they're not matching. The harness's `view=confirmed` is the source of truth; the `optimistic` view can lie when the ingress path fails before the tx lands.

**Debug checklist — "orderbook empty despite busy reveals"**

1. `getSignaturesForAddress <queue_pda> limit=50` — are there ANY recent txs for the queue? If no, bot isn't reaching the queue. Check relayer log for `bg submitter hard failure`.
2. For a recent `err:None` tx, read the full `logMessages` — search for `"AnchorError"`, `"terminalizing"`, `"failure_code"`. Don't trust `meta.err == None` alone.
3. Decode the `Program data:` event after each reveal — `status=3 Failed, failure_code=9 OracleStale` looks like `err:None` at tx level but means the reveal terminalized.
4. Compare harness `view=confirmed` vs `view=optimistic`. Divergence means ingress is lying.
5. Verify bot config files match the active v5 market indices. `.devnet/run/execution-queue-e2e-<group>.json: perpMarketIndex` is a common staleness source after a market migration.

## Failure Mode 7: Ingress pre-check rejects every intent with `order price outside oracle band (stable)` and a single frozen `stable_price`

**Observed 2026-04-20.** All SOL-PERP (market 2) intents reject with:

```
reason: order price outside oracle band (stable):
  side=Bid price_lots=90168 native_price=901.68 stable_price=852.4843151622147
```

Key tell: **`stable_price` is byte-for-byte identical across rejections spanning 75+ minutes** (e.g. `852.4843151622147` on every single rejection since relayer startup). The on-chain `PerpMarket.stable_price_model.stable_price` for the same market is fresh (e.g. `859.269` at `last_update_timestamp` a few seconds ago) and the live Pyth feed has clearly moved. So the on-chain band *would* accept the bid — only the relayer's cached copy rejects.

**Root cause:** `ClientService::load_group_static_account_mirror` seeds every `PerpMarket` account into `static_account_cache` once at startup. `fetch_margin_check_account_map` always prefers static-cache hits over the TTL'd `margin_account_cache` (see `static_account_cache` lookup before the TTL check). Because static cache never refreshes, `target_market.stable_price_model.stable_price` used by `inside_price_limit` is frozen at relayer start. After a ~5 % spot move, every bid/ask crosses the stale ±`maint_base_*_weight` band.

**Diagnose in under a minute:**
```bash
# 1. Confirm frozen stable_price across time:
tail -c 20M .devnet/logs/ctm-relayer.log \
  | grep '"market":"2"' | grep 'stable_price=' \
  | grep -oE 'stable_price=[0-9.]+' | sort -u
# Exactly one unique value over a long window → frozen.

# 2. Compare to on-chain stable_price:
#    Read PerpMarket account, find oracle pubkey offset, then StablePriceModel begins at
#    oracle + 32 (oracle pk) + 16 (I80F48 conf_filter) + 8 (u64 max_staleness) + 72 (reserved).
#    First f64 there is stable_price; next u64 is last_update_timestamp.
```

**Fix (landed):** In `load_group_static_account_mirror`, do NOT push `PerpMarket` into `static_accounts`. Keep the `perps_by_market_index` mirror and `market_metadata` map — those only need pubkeys, not live account bytes. Let `PerpMarket` flow through the TTL'd `margin_account_cache` (`CTM_RELAYER_MARGIN_CACHE_TTL_MS`, default 500 ms, devnet 10000 ms) so `stable_price` refreshes at TTL cadence.

**Mainnet note:** 10 s TTL is devnet-generous. On mainnet with sub-second Pyth cadence and tight `maint_base_*_weight`, this cache is the difference between 5 % of a fast move being rejectable and a full minute. Set `CTM_RELAYER_MARGIN_CACHE_TTL_MS ≤ 500` for mainnet.

**Related:** The same static-cache pattern caches fallback/quote oracles and Banks. Banks have a `stable_price_model` too but aren't read by the current pre-check path, so they don't trip this exact bug. If any future pre-check reads a freshness-sensitive field from Bank, revisit the same carve-out.

---

## ⚠️ WARN — Oracle staleness config (DEVNET HACK, MUST FIX BEFORE MAINNET)

**Context (2026-04-20, investigating empty orderbook with all reveals returning `status=Revealed`):**
SOL/ETH/BTC perp markets on the devnet group (`BrbJtc8ja8CH75CxbtRMzYqchtEq4XvsQZ6nKvCkQfGG`, program `9rpAcg1jNmUydb4QoeCeJBGf8JfRuxLciRbS7AHGnXEq`) are configured with `oracle_config.max_staleness_slots = 600` (~4 min) to tolerate the devnet sponsored Pyth PriceUpdateV2 feed's real update cadence (observed publish_time lags of 120–180 s are normal there, occasionally up to 240 s).

**Why this is dangerous:**
- At 400 ms/slot, 600 slots = 240 s of tolerable staleness. On mainnet Pyth feeds update sub-second; 600 slots is an eternity of price drift a liquidator or attacker can exploit.
- A 240 s stale price lets a trader place orders against a stale mark, exit before the fresh price catches up, and socialize the loss to the insurance fund.
- `oracle_config.conf_filter = 0.1` (10 %) on these same markets is also lax — a 10 % confidence interval is fine for a vestigial testnet but unacceptable for real collateral.

**Before flipping any of these markets to mainnet:**
1. Set `max_staleness_slots` to **≤ 25 slots (~10 s)** for every perp and bank oracle (`PerpEditMarket::oracle_config_opt` / `TokenEdit::oracle_config_opt`).
2. Set `conf_filter` to **≤ 0.01 (1 %)** default; tighter (0.5 %) for BTC/SOL/ETH.
3. A zero `max_staleness_slots` triggers the program's strict check (`last_update_slot + 0 < now_slot`) which is effectively "always stale" — never ship zero. A sentinel value that means "disabled" should be explicitly negative per mango-v4 semantics, not zero (see `programs/mango-v4/src/state/oracle.rs::check_staleness`).
4. Drop `oracle_state_unchecked` from any public code path. See `mainnet_plan_17apr.md §A3` for the structural fix.

**Sanity query before every deploy:**
```
ts-node ts/client/scripts/execution-queue/bump-oracle-staleness.ts   # dumps current values
```
Any market reporting `max_staleness_slots > 25` (or `= 0`) in mainnet deploy logs is a **pre-launch blocker**.

**Related code review items:**
- `programs/mango-v4/src/state/oracle.rs::check_staleness` at line 190 — semantics of `max_staleness_slots >= 0` mean negative = disabled, zero = strictest. Ergonomically inverted; an operator assuming "0 = disabled" will deploy with strict mode.
- The `Option<u32>` wrapper on `OracleConfigParams.max_staleness_slots` lets admin-edit pass `None` (no-change) but `Some(0)` = strict. Client-side builders should refuse `Some(0)` unless the operator explicitly typed `--max-stale 0` with a confirmation prompt.
