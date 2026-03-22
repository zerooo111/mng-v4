# Fermi DEX v1 — On-Chain Programs

This document covers the Solana program that powers Fermi: a fork of Mango v4 extended with a cooperative-transaction-model (CTM) execution queue. We focus on what changed, why, and the trust assumptions at each layer.

---

## Relationship to Mango v4

Fermi's on-chain program is a superset of Mango v4. The core Mango primitives — groups, banks, perp markets, the orderbook, health accounting, liquidation, and PnL settlement — are preserved without modification. What Fermi adds is a **gated execution path**: instead of users submitting order instructions directly, orders flow through an on-chain FIFO queue that enforces sequencing, signature verification, and hash-locked dispatch.

### What's preserved from Mango v4

- **Token banks** — deposit/borrow accounting, interest rate curves, oracle-weighted collateral
- **Perp markets** — on-chain orderbook (bids/asks trees), event queue, funding rate computation
- **Health engine** — init/maint/liquidation-end health types, weighted asset/liability calculation
- **Liquidation** — permissionless liquidation of undercollateralized accounts
- **PnL settlement** — socialized loss, insurance fund, settlement limits
- **Group administration** — market creation, parameter tuning, oracle configuration

### What Fermi adds

- **ExecutionQueue account** — a 424KB on-chain account holding a circular buffer of 1,024 CTM items and 128 liquidity items
- **CTM envelope protocol** — dual-signature scheme (relayer + user) with sequence numbering and hash commitment
- **Queue dispatch** — instructions to enqueue, execute, and manage queue items
- **Direct-enqueue fallback** — user-only submission path (no relayer co-signature) with enforced delay
- **Health-region wrapping** — queue-dispatched place orders are wrapped in pre/post health snapshots to catch unhealthy orders before they hit the book

---

## The Execution Queue

### Account layout

```
ExecutionQueue (424,216 bytes)
├── header
│   ├── group: Pubkey
│   ├── admin: Pubkey
│   ├── ctm_signer: Pubkey              // active relayer's signing key
│   ├── pending_ctm_signer: Pubkey      // staged key rotation
│   ├── pending_ctm_activation_slot: u64
│   ├── next_sequence_to_execute: u64   // HEAD pointer
│   ├── max_seen_sequence: u64          // highest sequence received
│   ├── gap_observed_slot: u64          // when a gap was first detected
│   ├── gap_wait_slots: u8             // slots to wait before skipping (default 4-6)
│   ├── liquidity_delay_slots: u8      // delay before liquidity execution (default 25)
│   ├── execute_paused: bool
│   ├── enqueue_paused: bool
│   └── bump: u8
├── ctm_items: [QueueItem; 1024]        // sequence-indexed circular buffer
└── liquidity_items: [QueueItem; 128]   // standard FIFO circular buffer
```

### QueueItem structure

Each queue item is 368 bytes and carries everything needed for on-chain dispatch:

```
QueueItem
├── sequence: u64              // relayer-assigned monotonic number
├── min_execute_slot: u64      // earliest slot this can execute
├── ingress_slot: u64          // slot when enqueued
├── first_failure_slot: u64    // for retry tracking
├── kind: u8                   // CtmWrapped(0), LiquidityDeposit(1), LiquidityWithdraw(2)
├── status: u8                 // Empty(0), Pending(1), Executed(2), Failed(3), Skipped(4)
├── retries: u8                // max 1 retry
├── payload_len: u16           // up to 256 bytes
├── payload_hash: [u8; 32]     // SHA256 of serialized instruction payload
├── accounts_hash: [u8; 32]    // SHA256 of canonical account list
└── payload: [u8; 256]         // embedded instruction data
```

### Sequence-indexed addressing

CTM items use **sequence-indexed** slot assignment: item with sequence N is stored at `ctm_items[N % 1024]`. This means:

- The queue can hold at most 1,024 unexecuted items
- New sequences must fall in the window `[next_sequence_to_execute, next_sequence_to_execute + 1024)`
- Duplicate sequences at the same slot index are rejected if the slot holds a `Pending` item with matching sequence

The HEAD pointer (`next_sequence_to_execute`) advances only when the head item is dispatched, skipped, or drained. This enforces strict FIFO: you cannot execute item N+1 until item N is resolved.

---

## Order Lifecycle

### Phase 1: Intent signing

The user constructs a payload (e.g., `PerpPlaceOrderV2Payload`) and signs a canonical user-intent message:

```
user_intent_message = SHA256(
    "fermi:user-intent-v1",
    group,
    mango_account,
    user_owner,
    kind,               // 0 = CtmWrapped
    payload_hash,       // SHA256 of serialized payload
    accounts_hash       // SHA256 of canonical account list
)
```

This signature proves the user authorized this specific action with these specific accounts. The user never signs a Solana transaction — only the intent.

### Phase 2: Relayer sequencing (enqueue_ctm)

The relayer wraps the user's intent in a CTM envelope:

```
envelope_message = SHA256(
    "fermi:ctm-envelope-v1",
    group,
    sequence,            // monotonically increasing per market
    min_execute_slot,
    kind,
    payload_hash,
    accounts_hash,
    expires_at_slot      // 0 = no expiry
)
```

The relayer signs this envelope with its CTM key and submits an `execution_queue_enqueue_ctm` transaction containing:
- The serialized payload
- Both Ed25519 signatures (user + relayer) as pre-instructions
- The canonical account list

The on-chain program:
1. Scans the transaction's Ed25519 pre-instructions to find matching signatures
2. Verifies the relayer's signature over the envelope message
3. Verifies the user's signature over the intent message
4. Checks the sequence is in the valid window and the slot is available
5. Stores the item with `status = Pending`

### Phase 3: Execution (execute / execute_multi)

A permissionless cranker calls `execution_queue_execute` with the accounts needed for the head item. The program:

1. **Reads the head item** — `ctm_items[next_sequence_to_execute % 1024]`
2. **Checks slot-gating** — rejects if `clock.slot < item.min_execute_slot`
3. **Handles gaps** — if the head slot is empty or doesn't match the expected sequence:
   - Records `gap_observed_slot` if not already set
   - If `current_slot - gap_observed_slot >= gap_wait_slots`, skips forward (up to 32 items per call)
   - Otherwise, returns without executing (waits for the missing item)
4. **Verifies accounts hash** — recomputes `SHA256(accounts)` from the provided remaining accounts and compares to the stored `accounts_hash`. **Mismatch = halt.** This is the core anti-tampering check.
5. **Decodes the payload** — extracts the variant and parameters from the embedded bytes
6. **Dispatches** — routes to the underlying handler:

| Payload variant | Handler | Health-gated? |
|---|---|---|
| `PerpPlaceOrderV2` | `perp_place_order_inner()` | Yes |
| `PerpCancelOrder` | `perp_cancel_order()` | No |
| `PerpCancelOrderByClientOrderId` | `perp_cancel_order_by_client_order_id()` | No |
| `PerpCancelAllOrders` | `perp_cancel_all_orders()` | No |
| `PerpCancelAllOrdersBySide` | `perp_cancel_all_orders_by_side()` | No |
| `LiquidityDeposit` | `token_deposit()` via CPI | No |
| `LiquidityWithdraw` | `token_withdraw()` via CPI | No |

7. **Updates status** — marks the item as `Executed` or `Failed`, advances the head pointer

### Phase 4: Settlement

Matched perp orders produce fill events on the perp market's event queue. These are consumed by a separate `perp_consume_events` instruction (also permissionless). Funding payments accrue continuously and are settled via `perp_settle_fees` and `perp_settle_pnl`.

---

## Health-Region Wrapping

Place-order variants are **health-gated**: the program wraps them in a pre/post health check to ensure the order doesn't push the account below the init-health threshold.

```
┌─ health_region_begin ──────────────────────────────┐
│  1. Load MangoAccount                              │
│  2. Build ScanningAccountRetriever from accounts   │
│  3. Compute health cache                           │
│  4. Record pre_init_health                         │
│  5. Set is_in_health_region = true                 │
└────────────────────────────────────────────────────┘
                        │
                        ▼
             dispatch_perp_place_order_v2()
              (order enters the orderbook)
                        │
                        ▼
┌─ health_region_end ────────────────────────────────┐
│  1. Recompute health cache                         │
│  2. Verify health >= check_health_post threshold   │
│  3. Clear is_in_health_region flag                 │
└────────────────────────────────────────────────────┘
```

If post-health fails, the entire transaction rolls back — the order never enters the book. This is the primary mechanism preventing overleveraged positions.

### Pre-check optimization

Health-gated dispatch can revert (~26% of the time under load when accounts change between enqueue and execute). Each revert wastes CU and stalls the queue. The recommended mitigation is a **tiered pre-check** system in the cranker:

1. **Flag check** (~1K CU) — is the account frozen, bankrupt, or in liquidation?
2. **Fast health reject** (~5-15K CU) — approximate health calculation, skip if clearly unhealthy
3. **Full dispatch** — only attempted if pre-checks pass

This reduces the revert rate to ~3-5% and improves effective throughput by ~20%.

---

## Direct-Enqueue Fallback

If the relayer is offline or censoring, any user can bypass it entirely:

```
execution_queue_enqueue_direct
├── requires: user Ed25519 signature (no CTM co-signature)
├── assigns: sequence = max_seen_sequence + 1
├── enforces: min_execute_slot = current_slot + DIRECT_SUBMIT_DELAY_SLOTS (10)
└── otherwise: identical to CTM path (same hash checks, same dispatch)
```

The 10-slot delay exists to prevent race conditions: if the relayer has in-flight transactions with sequences near `max_seen_sequence`, the direct-enqueue item needs to land after them. The delay is long enough for any honest relayer transaction to finalize.

**What users can do via direct-enqueue without any off-chain cooperation:**
- Cancel individual orders (`PerpCancelOrder`, `PerpCancelOrderByClientOrderId`)
- Cancel all orders (`PerpCancelAllOrders`, `PerpCancelAllOrdersBySide`)
- Place reduce-only orders to close positions
- Deposit or withdraw collateral (via liquidity queue items)

---

## Gap Handling

Network jitter can cause sequence N+1 to land on-chain before sequence N. The queue handles this gracefully:

```
Queue state:  [315: executed] [316: executed] [317: EMPTY] [318: pending] [319: pending]
                                                ▲
                                    HEAD (next_sequence_to_execute = 317)

Slot 100: gap detected, gap_observed_slot = 100
Slot 104: gap_wait_slots = 4 elapsed
          → skip seq 317, mark as Skipped
          → advance HEAD to 318
          → execute 318, then 319
```

- Default `gap_wait_slots`: 4-6 (configurable by admin)
- Maximum items skipped per execute call: 32
- Skipped items emit on-chain events for auditability
- With SWQoS RPCs and proper infrastructure, gap-skip rate is <0.5%

---

## Liquidation

Liquidation in Fermi uses the same mechanics as Mango v4 and is **completely independent of the execution queue**. Liquidation instructions are direct on-chain calls — they never pass through the queue and are always available regardless of queue or relayer state.

### Flow

```
                    ┌─────────────────────────────────┐
                    │   Liquidator bot (permissionless)│
                    └───────────────┬─────────────────┘
                                    │
                    scans accounts for health < maint threshold
                                    │
                                    ▼
                    ┌─────────────────────────────────┐
                    │  perp_liq_base_or_positive_pnl  │
                    │                                 │
                    │  1. Verify liqee health < maint │
                    │  2. Compute base transfer       │
                    │  3. Transfer position + quote   │
                    │  4. Apply liquidation fee       │
                    │     (keeper + platform split)   │
                    │  5. Verify liqee post-health    │
                    │  6. Update PnL settlement limits│
                    └─────────────────────────────────┘
```

### Key properties

- **Permissionless** — any account with sufficient health can liquidate any undercollateralized account
- **Partial** — liquidations can be incremental; the liquidator doesn't have to absorb the entire position
- **Fee-incentivized** — liquidators earn a fee split between the keeper (who submits the tx) and the platform
- **Health-gated** — the liquidator's own account must remain healthy after absorbing the position
- **Insurance fund backstop** — if a liquidation results in socialized loss and the insurance fund has balance, it absorbs the loss first

### Liquidation types

| Instruction | What it does |
|---|---|
| `perp_liq_base_or_positive_pnl` | Reduces liqee base position, takes over positive PnL |
| `perp_liq_negative_pnl_or_bankruptcy` | Handles negative PnL / bankruptcy cases |
| `perp_liq_force_cancel_orders` | Force-cancels all resting orders for an undercollateralized account |
| `token_liq_with_token` | Liquidates token (spot) positions |
| `token_liq_bankruptcy` | Handles token bankruptcy |

---

## Risk Engine

### Health calculation

Every account's solvency is tracked through a **health cache** — a weighted sum of all positions:

```
health = Σ (token_balance × weight × oracle_price)
       + Σ (perp_base_value × weight + perp_quote + unsettled_pnl)
       - Σ (liabilities × liab_weight × oracle_price)
```

Three health types with progressively stricter weights:

| Health type | Used for | Weight strictness |
|---|---|---|
| **Init** | Opening new positions, health-region checks | Most conservative |
| **Maint** | Liquidation threshold | Moderate |
| **LiquidationEnd** | Post-liquidation health verification | Least conservative |

An account is liquidatable when `maint_health < 0`. New positions are rejected when they would cause `init_health < 0`.

### Oracle model

Perp markets reference an oracle account for price data. The program supports:

- **Pyth** — standard push oracle
- **Switchboard** — pull oracle
- **CLMM** — concentrated liquidity market maker oracles (with important caveats; see below)

**CLMM oracle risk**: Same-transaction price manipulation is viable — an attacker can sandwich a CLMM observation within a single Solana transaction. The recommended mitigation is a **dual-oracle band check**: compare the CLMM price against a secondary oracle (e.g., Pyth) and reject if they diverge beyond a configured threshold. This guard should be applied across all oracle-consuming instruction paths.

### Collateral and margin

- Each token has per-bank `init_asset_weight`, `init_liab_weight`, `maint_asset_weight`, `maint_liab_weight`
- Perp markets have separate base-position and PnL weights
- Cross-collateralization: all token and perp positions in a MangoAccount contribute to a single health value
- No isolated margin (all positions share one health pool)

---

## Trust Assumptions Summary

### What the program guarantees (trustless)

| Property | Mechanism |
|---|---|
| FIFO execution order | Sequence-indexed buffer, head-only dispatch |
| No account substitution | SHA256 hash of accounts stored at enqueue, re-derived and verified at execute |
| No payload tampering | SHA256 hash of payload stored at enqueue, payload embedded in queue item |
| User authorized the action | Ed25519 signature over canonical intent message verified on-chain |
| Health constraints respected | Pre/post health snapshot wrapping all place-order dispatches |
| Liquidation always available | Direct on-chain instructions, no queue dependency |
| Fallback always available | `enqueue_direct` requires only user signature + 10-slot delay |

### What requires trust (with mitigations)

| Trust assumption | Risk | Mitigation |
|---|---|---|
| Relayer sequences honestly | Reordering, censorship | Auditable sequence gaps; direct-enqueue fallback |
| Relayer is available | Delayed trading | Direct-enqueue (10-slot delay); cranker embedded in relayer binary is independent |
| Admin doesn't abuse privileges | Parameter manipulation | Timelock recommended (30-min minimum for critical changes); admin key can be multisig or governance |
| Oracle is accurate | Mispriced liquidations | Dual-oracle band check; confidence interval validation |
| No program bugs | Loss of funds | 165/165 tests passing; security audit completed; independent review recommended |

### Admin capabilities

The group admin can:

- Create/configure markets and banks
- Set oracle sources and parameters
- Pause/unpause the execution queue
- Rotate the CTM signer key (with staged activation)
- Configure gap_wait_slots, liquidity_delay_slots
- Force-close accounts (testing mode only — guarded by `is_testing()`)

The admin **cannot**:
- Execute queue items out of order
- Modify enqueued payloads or accounts hashes
- Bypass signature verification
- Access user funds directly (token accounts are PDA-controlled)

---

## Compute Budget

Understanding CU costs is important for anyone running a cranker or estimating throughput:

| Operation | CU cost | Items per 1.4M budget |
|---|---|---|
| Place order (direct dispatch, 1 lane) | ~140K | 10 |
| Place order (direct dispatch, 5 lanes) | ~200K | 7 |
| Place order (CPI dispatch) | ~315K | 4 |
| Cancel order | ~30-50K | 28-46 |
| Lane hash computation | ~2-3K per lane | — |
| Health region (begin + end) | ~40-60K | — |
| Accounts hash verification | ~5-10K | — |

The dominant cost driver for place orders is the health-region check, which must load and evaluate all of a user's positions and collateral. Users with more positions incur higher CU costs per order.

### execute_multi

The `execute_multi` variant supports up to 20 parallel account sets (lanes), allowing a single transaction to execute orders for multiple users in one call. The cranker pre-computes lane hashes and packs compatible items together:

```
execute_multi(max_items=8, lanes=[
    [group, account_A, market, bids, asks, ...],  // lane 0
    [group, account_B, market, bids, asks, ...],  // lane 1
    [group, account_C, market, bids, asks, ...],  // lane 2
])

→ dispatches items 317 (account_A), 318 (account_B), 319 (account_C) in one tx
```

Lane matching is strict: the program computes the hash from the actual provided accounts and compares to the stored `accounts_hash`. If the head item doesn't match any lane, execution stops (FIFO preserved).
