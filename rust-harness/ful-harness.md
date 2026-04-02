## Full Harness Plan

Goal: make the Rust replay engine mirror on-chain Mango state for `perp + token-collateral`
accounts, with periodic on-chain rebases. Spot/Serum/OpenBook exposures remain out of scope
for now and must be detected as unsupported/partial.

### Scope contract

- Exact mirror target:
  - perp positions and perp open orders
  - token collateral balances and token health inputs
  - perp funding state
  - periodic absorption of external on-chain mutations
- Explicit exceptions:
  - liquidity deposit / withdraw do not need optimistic replay and may arrive through on-chain
    sync every 2-5 seconds
  - spot / Serum / OpenBook state may be skipped for now
  - fees may be stubbed temporarily until exact values are wired

### Required design changes

- Replace the current owner-aggregate baseline with full per-`mango_account` baseline state.
- Extend the snapshot contract so it carries:
  - account-level token state
  - account-level perp state
  - token bank health inputs
  - perp market health inputs
  - unsupported exposure flags
- Make `bootstrapFromOnchainSnapshot(...)` rebase-capable instead of one-shot.
- Rebuild optimistic/confirmed state as:
  - authoritative on-chain baseline
  - plus queue-intent overlay after the baseline confirmed watermark

### Rust work

1. Extend public types:
   - add account snapshot types for tokens / perps / collateral
   - add market sync and token bank sync types
   - add optional `accounts`, `market_sync`, and `token_banks` sections to `EngineSnapshot`
2. Extend `HarnessEngine`:
   - import full perp position state into `MangoAccountValue`
   - import token position state
   - configure perp market risk/funding fields from sync snapshots
   - expose richer account snapshots
3. Rework replay baseline state:
   - store full baseline account snapshots keyed by mango account
   - store market sync state and bank sync state
   - replace one-time bootstrap with repeatable rebase
4. Rework projection:
   - initialize engine from baseline account + market state
   - overlay executed and optimistic intents after the baseline confirmed sequence
   - keep pending intents across rebases
5. Health:
   - build Mango Rust `HealthCache` directly from synced token/perp state
   - compute owner/account health from token + perp inputs
   - use actual market funding accumulators and per-position settled funding checkpoints
6. Unsupported exposures:
   - detect spot / Serum / OpenBook activity from sync snapshot
   - mark them in projected account state and emit divergences where useful

### TS / Node work

1. Extend TS snapshot builders with:
   - token bank sync fields
   - perp market sync fields
   - full account token/perp snapshots
2. During reconciliation:
   - build confirmed on-chain snapshot
   - pass it into `engine.bootstrapFromOnchainSnapshot(...)` on rust-backend so it rebases
3. Keep the backend interface stable where possible.

### Verification

- Rust unit tests:
  - rebase keeps pending intents and drops absorbed confirmed intents
  - funding changes affect projected health correctly
  - token collateral changes are absorbed by rebase
  - owner/account health matches the snapshot health inputs for token+perp accounts
- TS verification:
  - compile passes
  - rust backend smoke path still loads
  - harness reconciliation loop rebases cleanly
