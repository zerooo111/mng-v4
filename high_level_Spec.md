# High-Level Protocol Spec

## Purpose

This note is a short bird's-eye overview of the protocol for agents researching
performance. It focuses on:

- the end-to-end transaction lifecycle
- the main onchain entrypoints
- the key onchain and offchain components

Primary code roots:

- onchain: `programs/mango-v4/src`
- program tests: `programs/mango-v4/tests`
- TS client: `ts/client/src`
- offchain services: `bin/`

## Core Onchain Components

- `Group`: protocol-wide configuration, admins, market/token registry, global gates
- `Bank`: token vault/risk/interest/oracle state for a token bank
- `MangoAccount`: user state, token positions, perp positions, open-orders metadata
- `PerpMarket`: perp book, event queue, funding, fees, settle limits
- `Serum3Market` / `OpenbookV2Market`: adapters for external spot markets
- `ExecutionQueue`: optional queued execution layer for CTM/direct-submit/liquidity flows

Main state code:

- `programs/mango-v4/src/state/bank.rs`
- `programs/mango-v4/src/state/mango_account.rs`
- `programs/mango-v4/src/state/perp_market.rs`
- `programs/mango-v4/src/state/execution_queue.rs`

## Critical Onchain Entrypoints

The highest-value entrypoint groups for performance work are:

- token liquidity:
  - `token_deposit`
  - `token_withdraw`
  - `token_update_index_and_rate`
- perp trading:
  - `perp_place_order`
  - `perp_cancel_*`
  - `perp_consume_events`
  - `perp_update_funding`
  - `perp_settle_pnl`
  - `perp_settle_fees`
- external spot integrations:
  - `serum3_place_order`, `serum3_cancel_*`, `serum3_settle_funds`
  - `openbook_v2_place_order`, `openbook_v2_cancel_*`, `openbook_v2_settle_funds`
- liquidation / recovery:
  - `token_liq_with_token`
  - `token_liq_bankruptcy`
  - `perp_liq_base_or_positive_pnl`
  - `perp_liq_negative_pnl_or_bankruptcy`
  - `*_liq_force_cancel_orders`
- flash liquidity:
  - `flash_loan_begin`
  - `flash_loan_end`
- queued execution:
  - `execution_queue_enqueue_ctm`
  - `execution_queue_enqueue_direct`
  - `execution_queue_enqueue_liquidity`
  - `execution_queue_execute`
  - `execution_queue_execute_multi`

Most instruction implementations live in `programs/mango-v4/src/instructions/`.

## End-to-End Transaction Lifecycle

### 1. Direct user flow

1. User builds an instruction through the TS/Rust client.
2. The client supplies the user `MangoAccount`, relevant banks/markets, oracle accounts, and external market accounts.
3. The onchain instruction runs health/oracle checks, mutates state, and may CPI into SPL Token, Serum3, or OpenBook.
4. Resulting state is reflected in `MangoAccount`, `Bank`, market state, and event queues.
5. Offchain indexers/health/orderbook services observe the new state.

This is the lowest-latency path and is the baseline for compute profiling.

### 2. Queued execution flow

1. User or relayer encodes a queue payload in the TS client.
2. Payload is submitted through:
   - `execution_queue_enqueue_ctm` for relayer/CTM-signed flow, or
   - `execution_queue_enqueue_direct` for delayed direct submit.
3. The payload sits in `ExecutionQueue` until eligible.
4. Executor/cranker calls `execution_queue_execute` or `execution_queue_execute_multi`.
5. The queue instruction validates the head item, dispatches into the underlying Mango instruction, and then clears/retries/skips the item.
6. Downstream services update optimistic/confirmed views and operational telemetry.

This is the most important lifecycle for throughput and queue-liveness research.

### 3. Liquidation lifecycle

1. Offchain health/liquidator services identify unhealthy accounts.
2. Liquidator closes blocking orders first.
3. Liquidator executes token/perp liquidation entrypoints.
4. If losses remain, bankruptcy / insurance / socialization paths are used.
5. Settler and related services continue reducing residual debt/PnL over time.

This lifecycle matters for worst-case latency, state explosion, and liveness under stress.

## Main Offchain Components

- `ts/client`: user-facing builders, payload encoders, queue helpers
- `bin/service-mango-execution-engine`: relayer + executor for queued execution
- `bin/keeper`: recurring bank/funding/event maintenance
- `bin/liquidator`: unhealthy-account detection and liquidation
- `bin/settler`: perp PnL and related settlement flows
- `bin/service-mango-health`: offchain health computation / persistence
- `bin/service-mango-orderbook`: orderbook projection
- `bin/service-mango-fills`: fill ingestion
- `bin/service-mango-crank`: crank sender for onchain work loops

## Performance Hot Paths

For performance research, start with:

- health-cache construction and oracle reads
- `perp_place_order` and orderbook mutation
- `perp_consume_events`
- `token_withdraw` when borrow/health checks are triggered
- `execution_queue_execute` / `execution_queue_execute_multi`
- liquidation loops across repeated account fetches and retries
- external-market settle/cancel flows with large account lists

Typical bottlenecks:

- account-list size
- compute spent in health/oracle validation
- repeated bank/oracle lookups
- event consumption fanout
- queue head blocking and retry behavior
- offchain re-fetch / polling cadence

## Suggested Starting Points for Agents

- Read `programs/mango-v4/src/lib.rs` to see the full instruction surface.
- Read `programs/mango-v4/src/instructions/execution_queue.rs` for queued execution.
- Read `programs/mango-v4/src/instructions/perp_place_order.rs` and `perp_consume_events.rs` for the main trading hot path.
- Read `programs/mango-v4/src/instructions/token_withdraw.rs` and `token_update_index_and_rate.rs` for token-path costs.
- Read `bin/service-mango-execution-engine/src/main.rs` to understand relayer/executor batching and recovery behavior.

This document is intentionally short. It should orient performance-focused agents
before they descend into per-instruction or per-service profiling.
