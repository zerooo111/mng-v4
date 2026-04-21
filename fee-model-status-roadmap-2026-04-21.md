# Fee Model Status And Rollout Roadmap

Date: 2026-04-21

## Verdict

The protocol's **economic fee accounting** is already implemented on-chain in a meaningful way, but the planned **anti-spam / admission fee model** is still mostly unimplemented.

In practical terms:

- **Implemented today**
  - on-chain fee configuration and accounting for banks, perps, collateral fees, fee buyback, and fee withdrawal
  - relayer-side **rate limiting** and queue backpressure
  - fanout-side **auth** and connection caps
  - harness-side **partial fee observability**
  - test/dev deposit helpers

- **Not implemented yet**
  - paid admission tiers backed by durable billing/quota state
  - on-chain gas escrow / refundable enqueue deposits
  - refund / forfeit accounting tied to queue lifecycle
  - a clean frontend-ready read model for fee state
  - a production deposit UX abstraction on top of the current SDK

So the current system is best described as:

- **protocol fees: real**
- **spam fee model: roadmap**
- **frontend/sdk fee ergonomics: partial**

## What Exists Now

### 1. On-chain fee/accounting primitives are already real

The on-chain program already tracks the main protocol fee surfaces:

- `Bank` tracks `collected_fees_native`, `fees_withdrawn`, `collected_liquidation_fees`, `collected_collateral_fees`, and `collateral_fee_per_day`
  - see `programs/mango-v4/src/state/bank.rs`
- `PerpMarket` tracks `maker_fee`, `taker_fee`, `fees_accrued`, `fees_settled`, `fees_withdrawn`, `platform_liquidation_fee`, and `accrued_liquidation_fees`
  - see `programs/mango-v4/src/state/perp_market.rs`
- `Group` and `MangoAccount` already have buyback-fee settings and user accrual state
  - see `programs/mango-v4/src/state/group.rs`
  - see `programs/mango-v4/src/state/mango_account.rs`

The TS SDK already exposes the core protocol operations around this:

- `perpSettleFees`
- `adminTokenWithdrawFees`
- `adminPerpWithdrawFees`
- `accountBuybackFeesWithMngo`
- standard `tokenDeposit` / `tokenDepositNative`

Conclusion: the protocol already has a fee system. What it does **not** yet have is the newer **admission / anti-spam fee system** discussed in the roadmap docs.

### 2. Relayer has rate limits, not an economic fee model

The relayer currently implements:

- per-user token-bucket rate limiting
- queue watermark fast-fail behavior
- inflight / queued backpressure
- relayer-funded transaction submission

This is visible in:

- `bin/service-mango-execution-engine/src/main.rs`
  - `ingress_rate_limit_enabled`
  - `ingress_rate_limit_burst`
  - `ingress_rate_limit_per_sec`
  - `check_ingress_rate_limit(...)`

The TypeScript relayer and Rust execution engine both load a relayer payer keypair and use it as the transaction payer.

Conclusion: the relayer is **operationally protected**, but users are still not paying for admission. The cost is currently borne by the relayer wallet.

### 3. Fanout auth exists, but tiered fee enforcement does not

`bin/service-fanout` already has:

- API key auth
- JWT auth
- per-user connection caps
- global connection caps
- a `Tier` enum (`Free`, `Pro`, `Enterprise`)

But the tier is not yet used for differentiated request-bucket enforcement in the handler code. The claims are present, the enforcement model is not.

Conclusion: the read plane has **identity**, but not the full billing / tiering layer proposed in the roadmap.

### 4. Harness exposes some fee state, but awkwardly

The harness already pulls useful fee state from on-chain:

- `maker_fee` and `taker_fee` are included in `perp_markets` inside the full snapshot
- `fees_accrued_native` and `fees_settled_native` are collected into per-market monitoring state
- open interest and funding history are collected alongside those values

However the exposure is fragmented:

- `/state/markets/:market` is orderbook-focused and does **not** expose accrued/settled fee totals
- `/state/full` includes richer raw snapshot state, but it is not the normal ergonomic path for frontend fee widgets
- `/monitoring` contains the most useful fee aggregates, but it was not previously wrapped in the TS harness client

Conclusion: fee data is **partially surfaced**, but not yet shaped as a stable frontend/SDK fee-state product.

### 5. Deposit ergonomics exist for dev/test, not production

Current deposit-related surfaces:

- `POST /airdrop-deposit`
  - test-only
  - uses `unsafe_deposit` or a fallback deposit path
- `GET /state/deposit-context/:owner`
  - enough context to build a real quote-token deposit flow
- `MangoClient.createMangoAccount(...)`
- `MangoClient.tokenDepositNative(...)`

Important nuance:

- `unsafe_deposit` now appears correctly gated by `group.is_testing()` in `accounts_ix/unsafe_deposit.rs`
- this makes the harness helper safer as a devnet/testnet-only mechanism
- but it is still not a production deposit path

Conclusion: deposit building blocks exist, but production frontend UX still requires an orchestration layer on top of the SDK.

## What Is Still Missing

### 1. No economic anti-spam admission model

The roadmap docs are explicit about the current gap:

- `mainnet_roadmap_16apr.md`
- `mainnet_roadmap_v2.md`
- `mainnet_plan_17apr.md`

Missing pieces:

- durable quota / billing store
- owner-bound paid tiers
- gas account escrow
- refundable enqueue deposit or equivalent
- refund / forfeit settlement on execute / cancel / expire / skip

### 2. Liquidity enqueue is still not hardened

One roadmap item is still open in code:

- `ExecutionQueueEnqueueLiquidity` and `ExecutionQueueV3EnqueueLiquidity` still do not require an owner signer in the account structs
- `variant_uses_queue_owner_signer(...)` still returns `false`
- dispatch-account normalization still clears signer status for the user owner when present

That means the roadmap's H-1 class concern is still materially relevant.

### 3. Fee state is not yet shaped for product consumption

Today there is no canonical answer to:

- "what is my current paid tier?"
- "how much free quota remains today?"
- "how much gas escrow balance do I have?"
- "what refundable deposit will this submit reserve?"
- "how much was refunded vs forfeited this session?"

Without those answers, the frontend cannot present the fee model clearly even after it lands.

## Recommended Architecture Decision

Prefer **per-owner gas escrow with an internal reservation ledger** over per-item lamport vaults.

Why:

- simpler UX: one "gas balance" concept instead of one PDA per queued intent
- lower account churn and rent overhead
- better fit for relayer pre-admission checks
- easier to expose in SDK/frontend
- easier to sweep forfeits and reconcile refunds

Suggested model:

1. User tops up a gas escrow PDA.
2. Relayer reserves a configurable amount per accepted intent.
3. Queue lifecycle emits terminal outcome.
4. Reservation is:
   - refunded on successful execute
   - refunded on explicit cancel where policy allows
   - partially or fully forfeited on expire / skip / malicious spam cases
5. Harness/fanout expose the resulting balance, reserved amount, refunds, forfeits, and daily quota.

## Rollout Roadmap

### Phase 0: Read model and SDK cleanup

Goal: make fee/deposit state readable before charging users.

- Promote fee state to a documented, stable API surface.
- Keep `/monitoring` for operator use, but add a product-safe fee view.
- Add owner-scoped fee/quota state endpoint.
- Expose deposit context as a first-class SDK method.
- Expose monitoring / fee aggregates as first-class SDK methods.
- Remove dev/test assumptions from product-facing deposit flows.

Exit criteria:

- frontend can render market fee state without parsing raw snapshot internals
- frontend can build a deposit flow without handwritten RPC account discovery

### Phase 1: Durable off-chain tiers and quota ledger

Goal: ship the immediate roadmap version of fee-based admission without changing on-chain queue semantics yet.

- bind API keys to signed owner identity
- persist counters in Postgres or Redis
- enforce daily quota / burst limits by tier
- record billing / admission decisions as durable events
- expose current tier and remaining quota through an API

Exit criteria:

- free tier, market-maker tier, protocol tier are real and inspectable
- relayer rejects over-quota requests deterministically after restart

### Phase 2: On-chain gas escrow

Goal: convert tiers from a trust-based operational system into a user-funded system.

- add gas escrow PDA per owner or per key-owner binding
- add top-up and withdraw instructions
- add relayer-side escrow balance check before accept
- expose escrow balance, reserved amount, and spend history

Exit criteria:

- users can self-serve paid admission
- relayer no longer relies on a manual allowlist for upgraded tiers

### Phase 3: Refundable enqueue reservation

Goal: make spam economically expensive and make queue abuse visible.

- reserve per-intent submission cost from escrow
- emit lifecycle events for reserve / refund / forfeit
- define exact policy for:
  - executed
  - explicit cancel
  - expired
  - skipped
  - relayer submit failure
  - invalid signature / malformed intent
- surface those lifecycle events to harness/fanout

Exit criteria:

- queue-gap spam becomes meaningfully costly
- frontend can show "reserved", "refunded", and "forfeited" amounts

### Phase 4: Restricted rollout

Use the staged approach from `mainnet_plan_17apr.md`:

- restricted launch on a small market set
- capped account sizes
- 2-week soak
- public rollout only after real load / failure data

## SDK Integration Plan

### Immediate additions

`ContinuumHarnessClient`

- `getDepositContext(owner, { mangoAccount?, accountNum? })`
- `getMonitoring({ fullHistory? })`
- keep `airdropDepositUsdc()` dev-only and clearly marked as such

`MangoClient`

- add a high-level helper that:
  - resolves or creates the Mango account
  - builds health remaining accounts
  - builds a quote-token deposit transaction
- later add gas-escrow helpers:
  - `getGasAccount(...)`
  - `topUpGasAccount(...)`
  - `withdrawGasAccount(...)`
  - `estimateIntentReservation(...)`

### Strongly recommended typed views

Add stable SDK models for:

- `MarketFeeState`
  - maker fee
  - taker fee
  - accrued fees
  - settled fees
  - funding snapshot
  - open interest
- `OwnerFeeState`
  - tier
  - daily quota used / remaining
  - gas escrow balance
  - reserved balance
  - refunded today
  - forfeited today

This should become the primary frontend contract rather than exposing raw snapshot internals directly.

## Frontend Integration Plan

### Read path

Build three simple product surfaces first:

1. **Market fee panel**
   - maker fee
   - taker fee
   - current accrued / settled fees
   - funding snapshot

2. **Account fee panel**
   - current tier
   - remaining daily quota
   - gas balance
   - reserved balance
   - refundable deposit per next submit

3. **Ops-aware status**
   - degraded if fee state is stale
   - degrade gracefully to on-chain raw reads if harness/fanout is unavailable

### Deposit path

Frontend production deposit flow should be:

1. Call `getDepositContext(owner)`.
2. If no Mango account exists:
   - either create one inline
   - or present explicit create-account then deposit flow
3. Build deposit transaction from `MangoClient`.
4. Show health impact preview.
5. Submit wallet-signed tx.
6. Refresh owner fee/account state after confirmation.

Dev/test-only shortcut:

- expose `airdropDepositUsdc()` behind an explicit devnet-only feature flag

### UX requirement once paid admission lands

Every submit action should show:

- estimated reservation
- whether it is refundable
- current gas balance after reservation
- remaining free quota

Without this, users will experience the fee model as random rejection rather than as a transparent system.

## Bottom Line

The repo already contains enough on-chain fee machinery to support a real fee product, and enough harness/SDK primitives to begin a usable deposit flow.

What is still missing is the productization layer:

- durable billing/quota state
- gas escrow
- refundable reservation logic
- a stable fee-state API
- frontend-ready abstractions

That means the correct next step is **not** more fee math. It is:

1. stabilize fee/deposit read APIs,
2. ship owner-bound quota state,
3. add gas escrow,
4. then add refundable enqueue reservation and staged rollout.
