# Continuum State Harness Design

## Objective

Provide a single API wrapper that exposes **optimistic** and **confirmed** state for Mango queue-driven perp flows, so clients can render immediate UX without waiting for on-chain execution.

Primary outputs:
- Per-market orderbook depth and state
- Per-user margin/position summary
- Per-user open orders
- Queue progress and divergence indicators

## Scope and Assumptions

- Initial implementation targets queue payload variants:
  - `PerpPlaceOrderV2`
  - `PerpCancelAllOrders`
- Liquidity and additional cancel variants are supported as ingest records in phase 1 and expanded in projection logic iteratively.
- Sequence mapping is currently inferred from relay events; ambiguity can exist if identical sequence numbers are active across markets simultaneously. We track and surface ambiguity as divergence risk.

## Architecture

## Components

1. `ctm-sequencer-relayer` (existing)
- Accepts signed intents and submits enqueue tx.
- Publishes accepted intent events to harness sink.

2. `continuum-state-harness` (new)
- Ingests relay-accepted intent events.
- Subscribes to on-chain program logs and parses queue events.
- Maintains dual projections:
  - `confirmed_view`
  - `optimistic_view = confirmed_view + pending_intents`
- Exposes read APIs + stream API.

3. Consumers
- UI and bots query one wrapper API with `view=optimistic|confirmed`.

## Data Flow

1. Client sends intent to relay.
2. Relay signs/enqueues and emits `relay_intent_accepted` to harness.
3. Harness stores pending intent and rebuilds optimistic view immediately.
4. Harness listens to chain logs for `QueueItemProcessed`.
5. On `Executed`, harness promotes pending intent to confirmed and rebuilds both views.
6. On `Failed/Skipped`, harness removes pending intent and rebuilds optimistic view.

## Event Contracts

## Relay Intent Event

`POST /ingest/relay-intent`

```json
{
  "event_type": "relay_intent_accepted",
  "ts_ms": 0,
  "group": "<pubkey>",
  "execution_queue": "<pubkey>",
  "market": "0",
  "sequence": "123",
  "kind": 0,
  "payload_b64": "...",
  "remaining_accounts": [
    { "pubkey": "...", "is_signer": false, "is_writable": true }
  ],
  "min_execute_slot": "0",
  "expires_at_slot": "0",
  "user_owner": "<pubkey>",
  "mango_account": "<pubkey>",
  "enqueue_tx_signature": "<sig>"
}
```

## On-chain Queue Events

Parsed from program logs:
- `QueueItemEnqueued`
- `QueueItemProcessed`

Required fields:
- `group`
- `sequence`
- `kind`
- `status` (for processed)
- `slot`
- `tx_signature`

## State Model

## Canonical Intent

- Key: `(group, sequence, kind)`
- Optional discriminator: `(market, user_owner, mango_account, payload_hash)`

## Projections

`MarketProjection`
- `market`
- `bids` and `asks` aggregated by price lots
- `open_orders` indexed by order id and owner
- `trades` ring buffer
- `watermarks`: `optimistic_seq`, `confirmed_seq`, `last_slot`

`UserProjection`
- `owner`, `mango_account`
- `open_orders`
- `base_position_lots` per market
- `quote_position_native` per market
- `margin_summary` placeholder fields for extension

`QueueProjection`
- `pending_count`
- `processed_count`
- `failed_count`
- `last_processed_sequence`
- `lag_slots`

## Deterministic Replay

- Replay engine applies actions in deterministic order.
- Confirmed state is derived only from executed actions.
- Optimistic state is confirmed + pending accepted actions.
- Any mismatch between processed outcomes and optimistic assumptions triggers `divergence_event` and full market/user recompute from snapshots + event log.

## API Wrapper

## Read Endpoints

- `GET /healthz`
- `GET /state/markets/:market?view=optimistic|confirmed`
- `GET /state/users/:owner?view=optimistic|confirmed`
- `GET /state/orders/:market?owner=<pubkey>&view=optimistic|confirmed`
- `GET /state/queue/:market`
- `GET /state/full?market=<id>&view=optimistic|confirmed`

## Stream Endpoint

- `GET /state/stream`
- Server-sent events for:
  - `relay_intent_accepted`
  - `queue_item_processed`
  - `market_state_updated`
  - `user_state_updated`
  - `divergence_event`

## Runtime and Deployment

## Local Validator Mode

- Harness runs on same machine as validator and relay.
- Lowest latency for optimistic updates.
- File-backed event log and in-memory projections.

## Testnet/Mainnet Mode

- Shared event log (durable store) + periodic snapshots.
- Multiple read replicas behind load balancer.
- Replay determinism tests in CI using captured event streams.

## Phase Plan

## Phase 1: Ingestion + Event Log

- Implement harness service skeleton.
- Add relay event sink emission.
- Add append-only local event log.
- Add chain log parser for queue events.
Status: Implemented in initial form.

## Phase 2: Dual Projection Engine

- Implement deterministic replay core.
- Implement market/user/order projections for place + cancel-all.
- Implement optimistic + confirmed views with rebuild triggers.
Status: Implemented for place/cancel/cancel-all variants with deterministic replay ordering.

## Phase 3: Single API Wrapper

- Implement all read endpoints.
- Implement SSE stream.
- Implement response contracts with view watermarks.
Status: Implemented (`/state/*` + `/state/stream`).

## Phase 4: Runtime Controls and Operations

- Add mode configs (`local`, `testnet`, `mainnet`).
- Add health/liveness/metrics endpoints.
- Add divergence diagnostics and replay tooling.
Status: Implemented in initial form (`/healthz`, `/livez`, `/metrics`, `/diagnostics/divergence`, `/admin/replay`).

## Phase 5: Verification and Rollout

- Determinism tests from captured event fixtures.
- Replay consistency checks against confirmed chain state.
- Performance soak tests and rollout gates.
Status: In progress with runnable tooling:
- `continuum-state-harness-replay-check` for shuffled replay determinism checks over captured JSONL event logs.
- `continuum-state-harness-verify` for confirmed-view consistency checks against live on-chain Mango perp open orders.
- Remaining: long-running soak/perf gates and rollout SLO checks.

## Initial Success Criteria

- Relay-accepted intent appears in optimistic market/user state in under 200ms local mode.
- Confirmed state updates after `QueueItemProcessed(Executed)`.
- API supports both views for market/user/orders/queue.
- Divergence events are emitted and state recovers deterministically.
