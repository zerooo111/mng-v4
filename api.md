# Continuum State Harness API Reference

This document is the end-user API reference for the local/testnet/mainnet **Continuum State Harness** service.

The harness provides a single HTTP/SSE surface for:
- optimistic state (relay-accepted intents + confirmed chain state),
- confirmed state (executed on-chain queue outcomes only),
- queue health, divergence diagnostics, and replay tooling.

## Version and Stability

- Current API version: `v1` (implicit, pathless).
- Compatibility: additive fields may be introduced without a breaking change.
- Breaking changes: endpoint or schema-breaking changes should be introduced behind a new versioned path.

## Base URL

Default local URL:

```text
http://127.0.0.1:9091
```

Set via:
- `CONTINUUM_HARNESS_BIND_ADDR`

### Exact Localhost Endpoints Requested

- `http://127.0.0.1:9091/state/balances/<owner>?view=optimistic|confirmed`
- `http://127.0.0.1:9091/state/trades/<market>?view=optimistic|confirmed&limit=200`
- `http://127.0.0.1:9091/state/candles/<market>?view=optimistic|confirmed&resolution_sec=60&limit=200`
- `http://127.0.0.1:9091/airdrop` (POST body with connected wallet pubkey)
- `http://127.0.0.1:9091/airdrop-deposit` (POST body with connected wallet pubkey)

## Authentication

### Relay ingest endpoint auth

`POST /ingest/relay-intent` optionally requires bearer token auth.

- Env: `CONTINUUM_HARNESS_RELAY_INGEST_TOKEN`
- Header:

```http
Authorization: Bearer <token>
```

If no token is configured, ingest is open.

### Read endpoints

Read endpoints are currently unauthenticated by default.

### Airdrop endpoint

`POST /airdrop` is intended for local/test deployments only and should not be exposed publicly.

## Content Types

- Request JSON: `application/json`
- Response JSON: `application/json`
- Metrics: `text/plain`
- SSE stream: `text/event-stream`

## Core Concepts

- `view=optimistic`: confirmed state + pending relay-accepted intents.
- `view=confirmed`: only executed queue outcomes.
- `market`: current implementation uses market index as string (for example `"0"`).
- `owner`: user owner pubkey (base58).

## Endpoints

## 1) Health and Operations

### `GET /healthz`

Returns harness status and high-level counts.

Response `200`:

```json
{
  "ok": true,
  "mode": "local",
  "intents_total": 10,
  "divergences_total": 0,
  "markets_total": 1,
  "users_total": 2,
  "queue_views_total": 1,
  "sse_clients": 0,
  "airdrop_enabled": true,
  "airdrop_deposit_enabled": true,
  "generated_ts_ms": 1772349020285
}
```

### `GET /livez`

Simple liveness probe.

Response `200`:

```json
{
  "ok": true,
  "ts_ms": 1772349020285
}
```

### `GET /metrics`

Prometheus-style gauges.

Response `200` (text):

```text
# TYPE continuum_harness_intents_total gauge
continuum_harness_intents_total 10
# TYPE continuum_harness_divergences_total gauge
continuum_harness_divergences_total 0
# TYPE continuum_harness_markets_total gauge
continuum_harness_markets_total 1
# TYPE continuum_harness_users_total gauge
continuum_harness_users_total 2
# TYPE continuum_harness_sse_clients gauge
continuum_harness_sse_clients 0
```

### `GET /diagnostics/divergence?limit=<n>`

Lists recent divergence events.

Query params:
- `limit` optional, default `200`

Response `200`:

```json
{
  "items": [
    {
      "event_type": "divergence_event",
      "ts_ms": 1772349020285,
      "reason": "processed_without_relay_intent",
      "key": "<group>:<sequence>:<kind>",
      "details": {
        "status": "3",
        "slot": "123",
        "tx_signature": "..."
      }
    }
  ]
}
```

### `POST /admin/replay`

Triggers snapshot generation/replay path and returns timestamps.

Response `200`:

```json
{
  "ok": true,
  "optimistic_generated_ts_ms": 1772349020285,
  "confirmed_generated_ts_ms": 1772349020285
}
```

### `POST /airdrop`

Mints local/test USDC from the configured faucet mint authority to a wallet ATA.

Request body:

```json
{
  "owner": "FByAc4zWBnKKKnvdXSscFsztYBLgbUtmbVobnMVYxzkC",
  "ui_amount": 250
}
```

Alternative owner sources:
- `?owner=<pubkey>` query param
- `x-wallet-pubkey: <pubkey>` header

If `ui_amount` is omitted, harness default amount is used.

Response `200`:

```json
{
  "ok": true,
  "owner": "FByAc4zWBnKKKnvdXSscFsztYBLgbUtmbVobnMVYxzkC",
  "mint": "3kWXL6KRYf3CXp1De6q2tuFZc3spBtUbU4hNoCboBTCg",
  "destination_token_account": "8j8YqW5WE8MnN4R1xC4S4Q4Zf8D3Vj4eGj1hVq5MuQeW",
  "ui_amount": 250,
  "raw_amount": "250000000",
  "tx_signature": "5Qf...abc"
}
```

Errors:
- `400` invalid input, amount limit exceeded, or endpoint disabled
- `500` mint/send runtime error

### `POST /airdrop-deposit`

Credits `1000 USDC` directly inside Mango protocol accounting for a user via the on-chain `unsafe_deposit` instruction.

This is intentionally unsafe and test-only. It bypasses real token transfer semantics and must never be enabled in production.

Request body:

```json
{
  "owner": "FByAc4zWBnKKKnvdXSscFsztYBLgbUtmbVobnMVYxzkC",
  "mango_account": "optional-mango-account-pubkey"
}
```

Notes:
- `ui_amount` is fixed by harness config (`CONTINUUM_HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT`, default `1000`).
- If `mango_account` is omitted, the first Mango account for `owner` in the configured group is used.

Response `200`:

```json
{
  "ok": true,
  "owner": "FByAc4zWBnKKKnvdXSscFsztYBLgbUtmbVobnMVYxzkC",
  "mango_account": "gddrsZnnddtquJHquhCmqq3bekkW3MN5SBCSJSij79j",
  "group": "9VYm4QaBhEPEiFfyGxXEDpN7ZTh2muajTDebKrDL4f5k",
  "mint": "DnTjy48VD6KN2mkXoaHjmgtMxjT1Ub9dc64vxPfiHomA",
  "ui_amount": 1000,
  "raw_amount": "1000000000",
  "unsafe_deposit_tx_signature": "5Qf...abc",
  "execution_path": "unsafe_deposit"
}
```

Errors:
- `400` invalid/missing owner, account mismatch, endpoint disabled, or fixed-amount mismatch
- `500` runtime failure

`execution_path` values:
- `unsafe_deposit`: new on-chain unsafe instruction path was used.
- `token_deposit_into_existing_fallback`: node is running an older program binary; harness fell back to mint+deposit-into-existing path.

## 2) Relay Ingestion

### `POST /ingest/relay-intent`

Ingests a relay-accepted intent event.

Request body:

```json
{
  "event_type": "relay_intent_accepted",
  "ts_ms": 1772349020285,
  "group": "...",
  "execution_queue": "...",
  "market": "0",
  "sequence": "123",
  "kind": 0,
  "payload_b64": "...",
  "remaining_accounts": [
    {
      "pubkey": "...",
      "is_signer": false,
      "is_writable": true
    }
  ],
  "min_execute_slot": "100",
  "expires_at_slot": "0",
  "user_owner": "...",
  "mango_account": "...",
  "enqueue_tx_signature": "..."
}
```

Response `202`:

```json
{
  "ok": true,
  "key": "<group>:<sequence>:<kind>"
}
```

Errors:
- `401` unauthorized (when token is configured and missing/invalid)
- `500` parse/validation errors

## 3) State Read API

## `GET /state/markets/:market?view=optimistic|confirmed`

Returns market-level orderbook/open-order projection.

Example response:

```json
{
  "view": "optimistic",
  "data": {
    "market": "0",
    "bids": [
      { "price_lots": "100", "base_lots": "2" }
    ],
    "asks": [],
    "open_orders": [
      {
        "order_id": "...",
        "owner": "...",
        "mango_account": "...",
        "market": "0",
        "side": "bid",
        "price_lots": "100",
        "base_lots": "2",
        "quote_lots": "1000",
        "client_order_id": "1712345678901",
        "sequence": "123",
        "status": "open"
      }
    ],
    "watermarks": {
      "optimistic_seq": "124",
      "confirmed_seq": "120",
      "last_slot": "290123456"
    }
  }
}
```

## `GET /state/users/:owner?view=optimistic|confirmed`

Returns user-level projection.

Example response:

```json
{
  "view": "confirmed",
  "data": {
    "owner": "...",
    "mango_accounts": ["..."],
    "open_orders": [],
    "per_market": [
      {
        "market": "0",
        "open_order_base_lots_bid": "0",
        "open_order_base_lots_ask": "0",
        "quote_reserved_lots": "0",
        "base_position_lots": "0",
        "quote_position_native": "0"
      }
    ],
    "margin_summary": {
      "status": "placeholder",
      "source": "queue-replay"
    }
  }
}
```

## `GET /state/balances/:owner?view=optimistic|confirmed`

Returns balance-style user projection for UI consumption.

Response `200`:

```json
{
  "view": "optimistic",
  "data": {
    "owner": "...",
    "mango_accounts": ["..."],
    "per_market": [
      {
        "market": "0",
        "open_order_base_lots_bid": "2",
        "open_order_base_lots_ask": "0",
        "quote_reserved_lots": "200",
        "base_position_lots": "0",
        "quote_position_native": "0"
      }
    ],
    "totals": {
      "total_open_order_base_lots_bid": "2",
      "total_open_order_base_lots_ask": "0",
      "total_quote_reserved_lots": "200"
    },
    "margin_summary": {
      "status": "placeholder",
      "source": "queue-replay"
    },
    "view": "optimistic"
  }
}
```

## `GET /state/orders/:market?owner=<pubkey>&view=optimistic|confirmed`

Returns open orders for a market, optionally filtered by owner.

Query params:
- `owner` optional
- `view` optional (`optimistic` default)

Response `200`:

```json
{
  "view": "optimistic",
  "market": "0",
  "owner": "...",
  "data": [
    {
      "order_id": "...",
      "owner": "...",
      "mango_account": "...",
      "market": "0",
      "side": "ask",
      "price_lots": "101",
      "base_lots": "1",
      "quote_lots": "100",
      "client_order_id": "42",
      "sequence": "125",
      "status": "open"
    }
  ]
}
```

## `GET /state/trades/:market?view=optimistic|confirmed&limit=<n>`

Returns recent trade prints inferred by deterministic replay from matched order flow.

Response `200`:

```json
{
  "view": "confirmed",
  "market": "0",
  "data": [
    {
      "trade_id": "...",
      "market": "0",
      "price_lots": "100",
      "base_lots": "1",
      "quote_lots": "100",
      "taker_side": "ask",
      "maker_owner": "...",
      "taker_owner": "...",
      "maker_order_id": "...",
      "taker_sequence": "125",
      "ts_ms": 1772349020285,
      "view": "confirmed"
    }
  ]
}
```

## `GET /state/candles/:market?view=optimistic|confirmed&resolution_sec=<seconds>&limit=<n>`

Returns OHLCV candles derived from `trades` endpoint output.

Response `200`:

```json
{
  "view": "confirmed",
  "market": "0",
  "resolution_sec": 60,
  "data": [
    {
      "market": "0",
      "bucket_start_ts_ms": 1772349000000,
      "resolution_sec": 60,
      "open_price_lots": "100",
      "high_price_lots": "101",
      "low_price_lots": "99",
      "close_price_lots": "100",
      "base_volume_lots": "5",
      "quote_volume_lots": "500",
      "trade_count": 3,
      "view": "confirmed"
    }
  ]
}
```

## `GET /state/queue/:market`

Returns queue metrics for a market.

Response `200`:

```json
{
  "market": "0",
  "data": {
    "market": "0",
    "pending_count": 1,
    "processed_count": 40,
    "failed_count": 0,
    "skipped_count": 0,
    "last_processed_sequence": "120",
    "lag_slots": "3",
    "unmatched_processed_count": 0
  }
}
```

## `GET /state/full?view=optimistic|confirmed`

Returns full snapshot.

Response `200` (shape):

```json
{
  "view": "optimistic",
  "markets": { "0": { "...": "..." } },
  "users": { "<owner>": { "...": "..." } },
  "queue": { "0": { "...": "..." } },
  "generated_ts_ms": 1772349020285
}
```

## `GET /state/full?market=<market>&view=optimistic|confirmed`

Market-scoped full view.

Response `200`:

```json
{
  "view": "optimistic",
  "generated_ts_ms": 1772349020285,
  "market": { "...": "..." },
  "queue": { "...": "..." },
  "users": {
    "<owner>": { "...": "..." }
  }
}
```

## 4) SSE Stream

### `GET /state/stream`

Server-sent event stream.

Events emitted:
- `connected`
- `relay_intent_accepted`
- `queue_item_enqueued`
- `queue_item_processed`
- `divergence_event`
- `market_state_updated` (initial synthetic update)

Heartbeat comments are sent periodically.

Example:

```text
event: connected
data: {"ts_ms":1772349020285,"mode":"local"}

event: relay_intent_accepted
data: {"event_type":"relay_intent_accepted", ...}
```

## Data Models

## `QueueView`

- `optimistic`
- `confirmed`

## `QueueItemProcessed.status`

- `0`: empty
- `1`: pending
- `2`: executed
- `3`: failed
- `4`: skipped

## `kind`

- `0`: CTM wrapped
- `1`: liquidity deposit
- `2`: liquidity withdraw

## Common Errors

Generic error payload from handler exceptions:

```json
{
  "error": "<message>"
}
```

Common statuses:
- `401`: unauthorized (`/ingest/relay-intent` token mismatch)
- `404`: path not found
- `500`: validation/parsing/runtime errors

## TypeScript Client Wrapper

A typed client wrapper is available in:
- `ts/client/src/continuumHarnessClient.ts`

Example:

```ts
import { ContinuumHarnessClient } from '@blockworks-foundation/mango-v4';

const client = new ContinuumHarnessClient('http://127.0.0.1:9091');

const health = await client.healthz();
const market = await client.getMarketState('0', 'optimistic');
const user = await client.getUserState('OWNER_PUBKEY', 'confirmed');
const balances = await client.getBalances('OWNER_PUBKEY', 'optimistic');
const trades = await client.getTrades({ market: '0', view: 'confirmed', limit: 200 });
const candles = await client.getCandles({
  market: '0',
  view: 'confirmed',
  resolutionSec: 60,
  limit: 200,
});
const faucet = await client.airdropUsdc({
  owner: 'OWNER_PUBKEY',
  ui_amount: 250,
});
const unsafeDeposit = await client.airdropDepositUsdc({
  owner: 'OWNER_PUBKEY',
});
const full = await client.getFullState('confirmed');
```

## Verification Utilities (Phase 5)

These scripts are part of the operational API workflow:

- `yarn continuum-state-harness-verify`
  - compares harness view to on-chain Mango perp open orders.
- `yarn continuum-state-harness-replay-check`
  - shuffles captured JSONL events and checks deterministic replay outputs.

## Environment Variables (Harness)

- `CONTINUUM_HARNESS_BIND_ADDR`
- `CONTINUUM_HARNESS_MODE`
- `CONTINUUM_HARNESS_PROGRAM_ID`
- `CONTINUUM_HARNESS_EVENT_LOG_PATH`
- `CONTINUUM_HARNESS_RELAY_INGEST_TOKEN`
- `CONTINUUM_HARNESS_REPLAY_LOG`
- `CONTINUUM_HARNESS_BACKFILL_SIGNATURE_LIMIT`
- `CONTINUUM_HARNESS_COMMITMENT`
- `CONTINUUM_HARNESS_REQUEST_BODY_MAX_BYTES`
- `CONTINUUM_HARNESS_ENABLE_AIRDROP`
- `CONTINUUM_HARNESS_USDC_MINT`
- `CONTINUUM_HARNESS_AIRDROP_KEYPAIR`
- `CONTINUUM_HARNESS_AIRDROP_DEFAULT_UI_AMOUNT`
- `CONTINUUM_HARNESS_AIRDROP_MAX_UI_AMOUNT`
- `CONTINUUM_HARNESS_GROUP_PK`
- `CONTINUUM_HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT`

## Environment Variables (Relayer -> Harness Sink)

- `CTM_RELAYER_EVENT_SINK_URL`
- `CTM_RELAYER_EVENT_SINK_AUTH_TOKEN`

## Curl Quick Reference

```bash
curl -s http://127.0.0.1:9091/healthz | jq
curl -s 'http://127.0.0.1:9091/state/markets/0?view=optimistic' | jq
curl -s 'http://127.0.0.1:9091/state/users/<owner>?view=confirmed' | jq
curl -s 'http://127.0.0.1:9091/state/balances/<owner>?view=optimistic' | jq
curl -s 'http://127.0.0.1:9091/state/orders/0?owner=<owner>&view=optimistic' | jq
curl -s 'http://127.0.0.1:9091/state/trades/0?view=confirmed&limit=200' | jq
curl -s 'http://127.0.0.1:9091/state/candles/0?view=confirmed&resolution_sec=60&limit=200' | jq
curl -s 'http://127.0.0.1:9091/airdrop' \
  -H 'Content-Type: application/json' \
  -d '{"owner":"<owner>","ui_amount":250}' | jq
curl -s 'http://127.0.0.1:9091/airdrop-deposit' \
  -H 'Content-Type: application/json' \
  -d '{"owner":"<owner>"}' | jq
curl -s http://127.0.0.1:9091/state/queue/0 | jq
curl -s 'http://127.0.0.1:9091/state/full?view=confirmed' | jq
curl -N http://127.0.0.1:9091/state/stream
```
