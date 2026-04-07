# Tentative CU Reduction Plan

## Goal

Target `50%+` median CU reduction for common hot paths, especially maker-style cancel/replace and quote-update flow.

That target is realistic only if this is treated primarily as a risk and account-state architecture problem, not just as a queue-plumbing problem.

## Core Observation

The remaining dominant costs are:

- rebuilding global cross-margin health
- loading and mutating large all-purpose account state
- repeating setup and teardown for bursts of same-account operations

Queue-level polish still matters, but it is no longer the main lever.

## Exact-Semantics Track

These keep the current trust model and execution semantics intact as much as possible:

- sequence-indexed, head-only FIFO
- accounts-hash and payload-hash checks
- on-chain Ed25519 authorization
- direct fallback paths
- liquidation remaining independent of the queue

### 1. Exact Incremental Risk Sidecar

Add a canonical per-account `RiskCache` or `HealthAccount` PDA that stores running risk totals plus per-active-token and per-active-perp contributions, with versioning against:

- bank params
- market params
- oracle slot
- funding state

Every instruction that mutates balances or perp state updates only the touched contributions. Hot place paths then perform an incremental delta check for the touched perp and settle token instead of rebuilding fresh health from the full account basket. On version mismatch or structural change, fall back to a slow full rebuild.

This is the main architecture project. It is the most credible exact-semantics path to attacking the dominant health cost directly.

Expected gain:

- place/update flow on larger active accounts: `35-50%`
- smaller accounts: less

### 2. Hot/Cold Account Split

Split hot trading state out of `MangoAccount`.

Hot sidecars should hold:

- open orders
- slot metadata
- client IDs
- reserved risk or compact risk metadata
- per-market perp hot state

Keep slower-moving identity, delegate, collateral, and admin fields in the core account.

This lets hot place/cancel/update/match paths touch a compact hot state account plus the risk sidecar, not the full dynamic account blob.

Expected gain:

- cancel/update flow: often `2-3x` cheaper versus today
- matching side: materially lower account-touch overhead

### 3. Stateful Same-Account Execution in `execute_multi`

If consecutive head items share `group + mango_account + perp_market + lane hash`, keep the user hot state and risk sidecar open across the contiguous run.

- load account, book, and risk context once
- execute ordered ops in exact head order
- perform one final risk check for the run

This preserves FIFO because execution order does not change. It only amortizes repeated setup and teardown across bursts.

Expected gain:

- bursty market-maker traffic: `10-25%`
- isolated one-off flow: smaller

### 4. Per-Market Hot Position Accounts for Matching

If consume remains expensive after the first three projects, move maker/taker fill application onto compact per-market position PDAs, with the global risk sidecar updated from those deltas.

This is the long-term consume-side answer. It is bigger migration work and should come after the first three steps.

Expected gain:

- consume/matching: material, but depends on the hot/cold split already existing

## Near-Term Bridge Work

These are still worthwhile and can land before or alongside the larger architecture.

### 1. `PerpQuoteUpdateBySlot`

Build a dedicated queue payload and execution path that performs `cancel -> place` in one ordered path.

- one payload decode
- one account validation pass
- one signature parse
- one health finalization
- preserve exact `cancel -> place` semantics

Expected gain:

- quote-update path: `25-40%`

This is still worth doing early even if the larger sidecar work is approved.

### 2. Tiered Cranker Pre-Check

Add fast reject, simulation skip-list, and stale-account invalidation to the cranker.

- does not lower CU for successful on-chain instructions
- does reduce total burned CU under load

Expected system-level waste reduction:

- `15-25%+`

### 3. Remaining Plumbing Cleanup

Continue stripping avoidable allocator work and hot-path logging where semantics are unchanged.

Expected gain:

- `2-5%`

## Opt-In Semantic Additions

These are the fastest path to `>50%` on update-heavy maker flows, but they change user-level semantics, capital efficiency, or execution guarantees.

### 1. Native Batch-Intent Payloads

Introduce a queue payload containing an ordered list of cancels and places for one account, initially likely one market.

- same payload-hash and accounts-hash security model
- FIFO preserved at queue-item granularity
- semantic change: the batch becomes atomic

Expected gain:

- market-maker update traffic: `2-4x`

### 2. Reserved Margin / Risk Envelopes

On placement, compute and store a conservative worst-case margin reservation for the resting order. Cancels release it. Replaces net old against new. Fills consume from the reservation instead of forcing a fresh full-account health walk every time.

This preserves solvency if done conservatively, but it can reduce capital efficiency relative to exact cross-margin.

Best initial scope:

- opt-in for maker accounts

### 3. Signed Risk-Budget / Quote-Lease Intents

User signs a bounded risk delta for a short window. Quote updates within that budget avoid full margin recomputation and only update bounded counters.

Likely gain:

- quote flow: `60-80%`

Tradeoff:

- changes what the signed intent means

### 4. Isolated or Bucketed Risk Mode

Offer optional isolated subaccounts or per-strategy risk buckets for active traders.

This shrinks the active health basket and is one of the cleanest paths to `>50%` average CU reduction for hot trading flows, but it changes the current all-in-one cross-margin UX.

### 5. In-Place Amend Semantics

Potentially very cheap, but changes FIFO or time-priority behavior.

Do not pursue unless revisiting that invariant is explicitly approved.

## What Not To Prioritize

- queue sharding for CU per hot-path op
  - useful for throughput scaling
  - not the main lever for per-op compute reduction
- removing generic CPI dispatch for hot perp queue variants
  - those variants already dispatch directly into internal handlers
- more hash micro-optimizations
  - already in the noise relative to health and account-state cost
- CPI self-dispatch as a CU optimization
  - helps rollback isolation
  - adds overhead

## Recommended Execution Order

If preserving current semantics is the main goal:

1. Exact incremental risk sidecar
2. Hot/cold account split
3. Stateful same-account execution in `execute_multi`
4. Per-market hot position accounts for matching

If the goal is to get `>50%` on update-heavy maker flow quickly, after step 1:

1. Exact incremental risk sidecar
2. Native batch-intent payloads
3. Reserved margin / risk envelopes

## Practical Merged Sequence

This is the sequence I would actually run from the current codebase:

1. `PerpQuoteUpdateBySlot` and cranker fast-reject for immediate wins
2. Exact incremental risk sidecar
3. Hot/cold state split
4. Stateful same-account execution in `execute_multi`
5. If needed, per-market hot position accounts for matching
6. Only then consider opt-in batch intents or reserved risk envelopes

## Rough Expected Outcome

Without semantic changes:

- quote-update hot path: `50%+` becomes plausible if incremental risk sidecar and hot/cold split land well, especially with same-account burst amortization
- place-only hot path: may still stay below `50%` for simple one-off orders
- consume-only hot path: unlikely to reach `50%` without the deeper per-market hot position redesign

With opt-in semantic additions:

- update-heavy maker flow: `>50%` is realistic and may be much higher

These estimates are tentative and should be validated with before/after simulation and CU traces.
