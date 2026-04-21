# V5 Fees Model

Date: 2026-04-21

Status: proposed design for the relayer-side fee ledger and dynamic fee policy

## Summary

V5 introduces a **relayer-local fee ledger** keyed by trading account, with all balances denominated in **lamports**.

Core policy:

- every new `mango_account` gets a **relayer-sponsored seed balance** of `0.1 SOL`
- users can top up fee balance by sending SOL to a **common deposit address**
- an authenticated internal call to `/fees-deposited` credits the user's internal balance after deposit observation
- the relayer burns a unique **deposit credit hash** for every credited deposit to prevent double-crediting
- the relayer debits the user's internal balance on every accepted relayer intent
- intents are rejected with **"insufficient fees, deposit more"** once the account runs out of available fee balance
- intent pricing is dynamic and responds to:
  - market queue backpressure
  - relayer off-chain contention
  - actual on-chain landing economics
- users may pass a relayer-local `max_fee_lamports` limit
  - if omitted or set to `auto`, the relayer charges the prevailing fee

This is intentionally an **internal ledger model**, not an on-chain gas PDA model.

## Goals

- make relayer usage economically bounded per account
- preserve fast off-chain pricing changes without on-chain governance churn
- cover relayer operating cost when on-chain priority fees rise
- deter spam during both off-chain and on-chain contention
- keep fee UX predictable most of the time
- avoid surprise fees above `0.01 SOL` per intent except during nearly-full queue conditions, with warnings

## Non-Goals

- this document does not change on-chain Mango fee accounting
- this document does not make fee credit withdrawable from the protocol
- this document does not replace direct on-chain user fallback paths
- this document does not define KYC or sanctions policy

## Terms

- `user_owner`: the user's EOA / wallet pubkey used in relayer submits
- `mango_account`: the Mango trading account targeted by the relayer intent
- `fee account`: the internal relayer ledger record for a specific `mango_account`
- `sponsored balance`: the one-time relayer-funded starting balance
- `paid balance`: user-funded fee credit from actual SOL deposits
- `available balance`: spendable internal fee balance
- `deposit credit hash`: a unique hash burned after a deposit is credited so the same deposit cannot be credited twice

## Accounting Unit

The canonical fee ledger is **per `mango_account`**.

Why:

- relayer writes are scoped to `mango_account`
- users can have multiple Mango accounts with different activity profiles
- the relayer already receives both `user_owner` and `mango_account` on submit

The ownership relationship is:

- `user_owner` owns one or more `mango_account`s
- each `mango_account` has exactly one fee account row
- a deposit can be credited to a specific `mango_account`
- if a deposit is owner-scoped but no target `mango_account` is provided:
  - if the owner has exactly one active Mango account, credit that one
  - if the owner has multiple active Mango accounts, reject the credit as ambiguous unless the authenticated caller specifies the target account

## Sponsored Seed Balance

Every new `mango_account` is seeded with:

- `SPONSORED_SEED_LAMPORTS = 100_000_000`
- equivalent to `0.1 SOL`

Rules:

- the seed is granted once, when the fee account row is first created
- the seed is **internal credit only**
- it is **not withdrawable**
- it is spendable only on relayer fees
- it is tracked separately from user-paid deposits

Recommended display fields:

- `sponsored_seed_total_lamports`
- `sponsored_seed_remaining_lamports`
- `paid_credit_total_lamports`
- `paid_credit_remaining_lamports`
- `available_balance_lamports`

### Important Risk

A strict "0.1 SOL for every new Mango account" policy can be farmed by creating many accounts.

This README preserves the stated policy, but production rollout should consider one or more guards:

- only grant the seed after the account first passes authenticated relayer submission
- limit the number of seeded accounts per `user_owner`
- apply seed only to accounts created after login / API-key enrollment
- require deposit or signed enrollment before granting seed on secondary accounts

## Fee Account Record

Each `mango_account` gets one durable fee account record.

Suggested schema:

```text
fee_accounts
-----------
mango_account                     pubkey primary key
user_owner                        pubkey not null
created_at_ms                     bigint not null
updated_at_ms                     bigint not null

sponsored_seed_total_lamports     bigint not null
sponsored_seed_remaining_lamports bigint not null

paid_credit_total_lamports        bigint not null
paid_credit_remaining_lamports    bigint not null

reserved_lamports                 bigint not null
debited_lamports_total            bigint not null

available_balance_lamports        bigint not null

daily_quota_limit                 bigint null
daily_quota_used                  bigint not null
daily_quota_window_start_ms       bigint not null

last_debit_at_ms                  bigint null
last_credit_at_ms                 bigint null
status                            enum(active, suspended, closed)
```

Notes:

- `available_balance_lamports = sponsored_seed_remaining_lamports + paid_credit_remaining_lamports - reserved_lamports`
- `daily_quota_*` fields exist so the system can expose "available quotas/fee balances" from the same record even if the first rollout relies primarily on balance
- launch default may set `daily_quota_limit = null` and use balance as the main gate

## Append-Only Ledger

Every credit and debit must also be written to an append-only ledger.

Suggested schema:

```text
fee_ledger_entries
------------------
id                                uuid primary key
mango_account                     pubkey not null
user_owner                        pubkey not null
entry_type                        enum(
                                    seed_credit,
                                    deposit_credit,
                                    reservation_hold,
                                    reservation_release,
                                    intent_debit,
                                    admin_adjustment
                                  )
amount_lamports                   bigint not null
balance_after_lamports            bigint not null
reference_type                    enum(
                                    seed,
                                    deposit,
                                    intent,
                                    admin
                                  )
reference_id                      text not null
metadata_json                     jsonb not null
created_at_ms                     bigint not null
```

This ledger is the source of truth for audit and reconciliation.

## Burned Deposit Hashes

The anti-double-credit table is separate and immutable.

Suggested schema:

```text
credited_deposit_hashes
-----------------------
deposit_credit_hash                 text primary key
source_chain                        text not null
source_tx_signature                 text not null
instruction_index                   integer not null
user_owner                          pubkey not null
mango_account                       pubkey not null
amount_lamports                     bigint not null
credited_at_ms                      bigint not null
```

Behavior:

- if a credit hash is already present, the deposit is already consumed
- the hash is never deleted
- repeated `/fees-deposited` calls for the same hash must be idempotent

Recommended response on duplicate:

- `200 OK` with `{ "ok": true, "duplicate": true }`

This is safer than returning a hard error because the caller is usually an automated indexer or settlement worker.

## Common Deposit Address

Users top up by sending SOL to one **common deposit address** controlled by the operator.

### Recommended Deposit Transaction Format

To make a common address safely attributable, deposits should include a memo or structured reference.

Recommended memo payload:

```text
fee_credit:v1:<user_owner>[:<mango_account>]
```

Examples:

```text
fee_credit:v1:9abc...owner
fee_credit:v1:9abc...owner:7def...mango
```

If no `mango_account` is included:

- the settlement service may resolve it automatically only when the owner has exactly one active fee account
- otherwise the credit should remain pending and require explicit disambiguation

### Deposit Confirmation Rule

The settlement worker should only credit deposits after:

- the transfer to the common deposit address is observed
- the amount matches the recorded transfer
- the memo / attribution data is valid
- the transaction reaches the configured finality threshold

Recommended threshold:

- Solana `finalized`, or
- a conservative internal confirmation policy if faster crediting is desired

## `/fees-deposited`

`/fees-deposited` is an **authenticated internal endpoint**.

It is **not** a public user endpoint.

It should be called only by:

- a deposit indexer
- a settlement worker
- an operator-only back office

### Request Shape

Suggested request body:

```json
{
  "source_chain": "solana-mainnet",
  "source_tx_signature": "5abc...sig",
  "instruction_index": 0,
  "user_owner": "9abc...owner",
  "mango_account": "7def...mango",
  "amount_lamports": "250000000",
  "deposit_address": "CommonFeeDeposit11111111111111111111111111111",
  "memo": "fee_credit:v1:9abc...owner:7def...mango",
  "observed_at_ms": 1775000000000
}
```

### Credit Hash

The server computes:

```text
deposit_credit_hash =
  sha256(
    source_chain ||
    source_tx_signature ||
    instruction_index ||
    deposit_address ||
    user_owner ||
    mango_account ||
    amount_lamports
  )
```

The server then:

1. verifies the deposit really exists and matches the request
2. checks whether `deposit_credit_hash` is already burned
3. if not burned:
   - credits the fee account
   - writes a `deposit_credit` ledger entry
   - inserts the burned hash row
4. returns the updated fee account state

### Authentication

Recommended auth:

- service-to-service JWT, or
- HMAC-signed internal request header, or
- mutual TLS behind the gateway

At minimum:

- do not expose this endpoint on the public edge
- do not trust caller-provided amount without chain verification

## Intent Debit Lifecycle

The relayer debits fees for **accepted relayer intents**.

The debit model should be:

1. compute the prevailing fee quote
2. compare against `max_fee_lamports` policy
3. atomically check available balance
4. create a reservation hold
5. if the intent is accepted into the relayer pipeline:
   - convert the reservation into a finalized debit
6. if the relayer rejects before acceptance:
   - release the reservation

This preserves the user-visible rule:

- every accepted relayer submission is charged

while still avoiding accidental balance loss on internal pre-accept failures.

### Recommended Debit Point

Finalize the debit after:

- request validation succeeds
- account ownership/group checks succeed
- signature verification succeeds
- fee quote check succeeds
- balance reservation succeeds
- the relayer has accepted the intent into the submission pipeline and allocated any local sequencing state it needs

Do **not** charge for:

- malformed requests
- invalid signatures
- unauthorized API/auth failures
- requests rejected because `max_fee_lamports` is too low
- requests rejected because balance is insufficient

## Direct On-Chain Fallback

This fee model applies to the **relayer write path** only.

If a user bypasses the relayer and uses the direct on-chain path:

- the relayer fee ledger is not consulted
- the user pays actual chain fees directly
- no internal debit occurs

This preserves a liveness fallback even if the relayer fee service is degraded.

## Submit Request Fee Controls

`max_fee_lamports` is a **relayer-local request field**.

It is **not** part of the signed core user intent hash.

That means:

- users can adjust fee tolerance without re-signing the trading payload shape
- the field belongs in the submit wrapper, not the on-chain intent body

### Semantics

- omitted: treated as `auto`
- `"auto"` in JSON/REST clients: charge the prevailing fee
- typed clients: use `fee_mode = AUTO`
- numeric `max_fee_lamports`: reject if prevailing fee exceeds that value

### Suggested API Shape

For JSON/REST:

```json
{
  "intent_version": 2,
  "target_kind": 1,
  "target_index": 2,
  "user_owner": "9abc...owner",
  "mango_account": "7def...mango",
  "payload_b64": "...",
  "user_signature": "...",
  "max_fee_lamports": "auto"
}
```

Or:

```json
{
  "max_fee_lamports": "250000"
}
```

For gRPC/protobuf:

```text
enum FeeMode {
  AUTO = 0;
  CAP = 1;
}

message SubmitIntentRequest {
  ...
  FeeMode fee_mode = ...;
  uint64 max_fee_lamports = ...; // used only when fee_mode == CAP
}
```

### Failure Behavior

If `max_fee_lamports < prevailing_fee_lamports`, reject with:

- code: `FAILED_PRECONDITION`
- message: `fee too high for request; increase max_fee_lamports or use auto`

If the account lacks balance, reject with:

- code: `FAILED_PRECONDITION`
- message: `insufficient fees, deposit more`

## Fee Quote Endpoint

Clients should have a way to preflight current fees before submit.

Recommended endpoint:

```text
GET /fees/quote?mango_account=<pk>&market_index=<n>&intent_kind=<kind>
```

Suggested response:

```json
{
  "ok": true,
  "mango_account": "7def...mango",
  "market_index": 2,
  "intent_kind": "perp_place_order",
  "prevailing_fee_lamports": "85000",
  "components": {
    "service_base_lamports": "25000",
    "estimated_chain_cost_lamports": "30000",
    "queue_pressure_multiplier": 1.5,
    "relayer_pressure_multiplier": 1.2
  },
  "warnings": []
}
```

This endpoint is the recommended source for UI display and `max_fee_lamports` selection.

## Fee Formula

The fee must react to both:

- **off-chain contention**
  - market queue backlog
  - relayer inflight saturation
  - relayer queued backlog
- **on-chain economics**
  - current signature fees
  - current CU price / prioritization fee required for landing
  - relayer's recent actual landed cost

### Inputs

Suggested inputs:

- `service_base_lamports`
- `estimated_chain_cost_lamports`
- `market_queue_fill_ratio`
- `relayer_inflight_ratio`
- `relayer_wait_ratio`

Definitions:

```text
market_queue_fill_ratio = pending_items / soft_capacity
relayer_inflight_ratio  = inflight_submits / max_inflight
relayer_wait_ratio      = queued_waiters / max_queued
```

`estimated_chain_cost_lamports` should come from actual relayer economics:

```text
estimated_chain_cost_lamports =
  signature_fee_lamports +
  ceil(cu_used_estimate * cu_price_p75_micro_lamports / 1_000_000)
```

Recommended data source:

- rolling p75 or p90 of recently landed enqueue transactions
- provider priority fee APIs may assist, but landed relayer cost is the authoritative signal

### Recommended Constants

Start with:

```text
SERVICE_BASE_LAMPORTS = 25_000
CHAIN_COST_COVERAGE_MULTIPLIER = 1.20
NORMAL_HIGH_FEE_THRESHOLD = 2_000_000      // 0.002 SOL
NORMAL_MAX_FEE_LAMPORTS   = 10_000_000     // 0.01 SOL
EMERGENCY_MAX_FEE_LAMPORTS = 25_000_000    // 0.025 SOL
```

### Pressure Multipliers

Use separate multipliers for market pressure and relayer pressure.

#### Market Queue Multiplier

| Market queue fill ratio | Multiplier | Meaning |
|---|---:|---|
| `< 0.50` | `1.00` | healthy |
| `0.50 - 0.75` | `1.20` | moderate |
| `0.75 - 0.90` | `1.60` | busy |
| `0.90 - 0.97` | `2.50` | severe |
| `>= 0.97` | `5.00` | nearly full, warn |

#### Relayer Pressure Multiplier

Compute:

```text
relayer_pressure_ratio = max(relayer_inflight_ratio, relayer_wait_ratio)
```

Then:

| Relayer pressure ratio | Multiplier |
|---|---:|
| `< 0.50` | `1.00` |
| `0.50 - 0.75` | `1.10` |
| `0.75 - 0.90` | `1.25` |
| `>= 0.90` | `1.50` |

### Final Fee Formula

```text
chain_component =
  ceil(CHAIN_COST_COVERAGE_MULTIPLIER * estimated_chain_cost_lamports)

raw_fee =
  (SERVICE_BASE_LAMPORTS + chain_component) *
  market_queue_multiplier *
  relayer_pressure_multiplier

prevailing_fee_lamports =
  round_up_to_1_000(raw_fee)
```

### Fee Caps

Normal behavior:

- if `prevailing_fee_lamports <= NORMAL_MAX_FEE_LAMPORTS`, accept normally

If the computed fee exceeds `NORMAL_MAX_FEE_LAMPORTS`:

- if `market_queue_fill_ratio < 0.97`, cap at `NORMAL_MAX_FEE_LAMPORTS`
- if `market_queue_fill_ratio >= 0.97`, allow the computed fee to exceed `0.01 SOL` but:
  - attach warnings
  - clamp to `EMERGENCY_MAX_FEE_LAMPORTS`
- if the queue is fully unavailable or the computed fee exceeds `EMERGENCY_MAX_FEE_LAMPORTS`, reject instead of charging

### Warning Conditions

Emit warnings when:

- `market_queue_fill_ratio >= 0.90`
- `prevailing_fee_lamports >= NORMAL_HIGH_FEE_THRESHOLD`
- `prevailing_fee_lamports > NORMAL_MAX_FEE_LAMPORTS`

Suggested warning codes:

- `HIGH_FEE`
- `HIGH_ONCHAIN_COST`
- `QUEUE_NEARLY_FULL`
- `FEE_ABOVE_NORMAL_CAP`

## Example Fees

These are illustrative only.

### Healthy Market

```text
estimated_chain_cost = 8_000
chain_component      = 9_600
market_mult          = 1.00
relayer_mult         = 1.00

fee = round_up_to_1_000((25_000 + 9_600) * 1.0 * 1.0)
    = 35_000 lamports
```

### Busy Market, Elevated Priority Fees

```text
estimated_chain_cost = 60_000
chain_component      = 72_000
market_mult          = 1.60
relayer_mult         = 1.25

fee = round_up_to_1_000((25_000 + 72_000) * 1.60 * 1.25)
    = 194_000 lamports
```

### Nearly Full Queue

```text
estimated_chain_cost = 250_000
chain_component      = 300_000
market_mult          = 5.00
relayer_mult         = 1.50

fee = round_up_to_1_000((25_000 + 300_000) * 5.00 * 1.50)
    = 2_438_000 lamports
```

Still below `0.01 SOL`, but already high enough to warrant warnings.

## Debit Ordering and Concurrency

The fee account must be updated transactionally.

Recommended flow:

1. `SELECT ... FOR UPDATE` the fee account row
2. lazily create and seed it if missing
3. compute prevailing fee
4. enforce `max_fee_lamports`
5. enforce `available_balance_lamports >= prevailing_fee_lamports`
6. insert `reservation_hold`
7. update `reserved_lamports`
8. once intent is accepted:
   - insert `intent_debit`
   - move amount from reserved to debited
9. if pre-accept failure occurs:
   - insert `reservation_release`
   - release reserved amount

This avoids race conditions when one owner submits multiple intents concurrently.

## API Surface

### Public / User-Facing

- `GET /fees/state?mango_account=...`
- `GET /fees/quote?...`
- relayer submit API with `max_fee_lamports`

### Internal / Authenticated

- `POST /fees-deposited`
- `POST /fees/admin-adjust`

## `GET /fees/state`

Suggested response:

```json
{
  "ok": true,
  "mango_account": "7def...mango",
  "user_owner": "9abc...owner",
  "sponsored_seed_total_lamports": "100000000",
  "sponsored_seed_remaining_lamports": "87650000",
  "paid_credit_total_lamports": "250000000",
  "paid_credit_remaining_lamports": "247500000",
  "reserved_lamports": "85000",
  "available_balance_lamports": "335065000",
  "daily_quota_limit": null,
  "daily_quota_used": "0",
  "warnings": []
}
```

## Rejection Semantics

### Insufficient Balance

Reject with:

- code: `FAILED_PRECONDITION`
- message: `insufficient fees, deposit more`

Optional structured payload:

```json
{
  "error": "insufficient_fees",
  "message": "insufficient fees, deposit more",
  "available_balance_lamports": "12000",
  "required_fee_lamports": "85000",
  "deposit_address": "CommonFeeDeposit11111111111111111111111111111"
}
```

### Fee Above User Cap

Reject with:

- code: `FAILED_PRECONDITION`
- message: `fee too high for request; increase max_fee_lamports or use auto`

### Queue Unavailable

If the queue is effectively unavailable, prefer rejection over charging an absurd fee.

Reject with:

- code: `RESOURCE_EXHAUSTED`
- message: `queue congested; try again later or use direct fallback`

## Logging and Metrics

Every accepted or rejected fee decision should emit structured metadata:

- `correlation_id`
- `intent_id`
- `user_owner`
- `mango_account`
- `market_index`
- `prevailing_fee_lamports`
- `estimated_chain_cost_lamports`
- `market_queue_fill_ratio`
- `relayer_pressure_ratio`
- `fee_mode`
- `max_fee_lamports`
- `decision`

Recommended metrics:

- `fees_quote_requests_total`
- `fees_debits_total`
- `fees_debited_lamports_total`
- `fees_credit_total`
- `fees_credited_lamports_total`
- `fees_insufficient_balance_rejections_total`
- `fees_above_user_cap_rejections_total`
- `fees_high_fee_warnings_total`
- `fees_balance_available_lamports`

## Rollout Plan

### Phase 1

- add fee account tables
- lazily seed `0.1 SOL` per new `mango_account`
- add `/fees/state`
- add `/fees/quote`
- add `max_fee_lamports`
- add `/fees-deposited`
- debit accepted relayer intents

### Phase 2

- add UI around fee state and top-up
- add warning surfacing for high-fee conditions
- add operator reconciliation tooling
- add abuse controls for sponsor seed farming

### Phase 3

- optionally add daily quota overlays
- optionally add fee differentiation by intent class
- optionally move toward on-chain escrow if needed later

## Open Questions

These do not block the core model, but they should be settled before launch:

1. Should all intent classes cost the same, or should cancels / reduce-only actions be discounted?
2. Should the sponsored `0.1 SOL` seed be per Mango account, per EOA, or capped per EOA?
3. What is the required memo/reference format for the common deposit address?
4. How many confirmations are required before deposit crediting?
5. Should emergency risk-reducing actions be allowed with a small overdraft if balance is exhausted?

## Bottom Line

This model gives V5 a practical fee system without requiring new on-chain fee accounts:

- every `mango_account` gets an immediately usable sponsored balance
- deposits are credited to a common internal ledger through a verified, authenticated settlement path
- every accepted relayer intent is charged against that ledger
- dynamic pricing responds to both queue pressure and actual chain cost
- users can cap fees with `max_fee_lamports`
- fees above `0.01 SOL` are avoided by default and only allowed with warnings under nearly-full queue conditions

It is simple enough to ship quickly, but structured enough to evolve into tighter quota and escrow models later.
