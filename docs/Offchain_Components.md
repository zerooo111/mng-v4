# Fermi DEX v1 — Off-Chain Components

The on-chain program handles settlement and trust; the off-chain stack handles speed and usability. This document covers the three services that compose the Fermi off-chain infrastructure: the **relayer**, the **simulation harness**, and the **HTTP bridge** — plus how they connect to form a unified read/write interface to the chain.

---

## Architecture Overview

```
 ┌────────────────────┐
 │  Trader / Bot      │
 │  (wallet + SDK)    │
 └────┬──────────┬────┘
      │          │
      │ write    │ read
      │ (gRPC    │ (REST/SSE
      │  or HTTP)│  from harness)
      │          │
      ▼          ▼
 ┌─────────┐  ┌──────────────┐
 │ Relayer │  │   Harness    │◄──── on-chain subscription
 │ :9090   │  │   :9091      │      (confirmed state)
 │ (gRPC)  │  │   (HTTP)     │
 └────┬────┘  └──────┬───────┘
      │               │
      │               │ relay-intent event
      │               │ (optimistic update)
      │          ┌────┘
      │          │
      ▼          ▼
 ┌──────────────────┐     ┌──────────────┐
 │ Solana           │     │ HTTP Bridge  │
 │ (execution queue │     │ :9092        │
 │  + orderbook)    │     │ REST→gRPC    │
 └──────────────────┘     └──────────────┘
```

All three services run in a single process space when using the unified Rust execution engine (`service-mango-execution-engine`), or as separate Node.js processes in the TypeScript legacy stack. The Rust implementation is the default and recommended path.

---

## 1. The Relayer

### What it does

The relayer is the **write path** into Fermi. It receives signed user intents, assigns sequence numbers, applies the CTM co-signature, and submits enqueue transactions to Solana. It is the single point of sequencing — this is what gives every order a deterministic position in the queue.

### Protocol

**gRPC** on port 9090 (default), defined in `ctm_sequencer.proto`:

```protobuf
service CtmSequencerRelayer {
  rpc SubmitIntent (SubmitIntentRequest) returns (SubmitIntentResponse);
}

message SubmitIntentRequest {
  string group = 1;              // Mango group pubkey
  string execution_queue = 2;    // Queue account pubkey
  string market = 3;             // Perp market index
  bytes  payload = 4;            // Serialized queue payload
  repeated AccountMeta remaining_accounts = 5;
  string min_execute_slot = 6;
  string expires_at_slot = 7;
  string user_owner = 8;         // User's public key
  string mango_account = 9;      // User's MangoAccount
  bytes  user_signature = 10;    // Ed25519 over user-intent message
}

message SubmitIntentResponse {
  string sequence = 1;            // Assigned sequence number
  string tx_signature = 2;        // Solana transaction signature
  bytes  user_intent_message = 3; // Canonical intent bytes
  bytes  ctm_envelope_message = 4;// Canonical envelope bytes
}
```

### Sequencing model

The relayer maintains a per-market monotonic counter. Each incoming intent receives `sequence = last_sequence + 1`. Sequence state is persisted to disk every 250ms (configurable via `CTM_RELAYER_SEQUENCE_STATE_PATH`).

The sequence number determines the item's position in the on-chain queue. Since the queue enforces head-only execution, the relayer's sequencing directly determines execution order. This is the one trust assumption that the direct-enqueue fallback relaxes (at the cost of a 10-slot delay).

### Submission pipeline

```
Intent arrives (gRPC)
    │
    ▼
Parse + validate payload
    │ (~1-5ms)
    ▼
Assign sequence number
    │
    ▼
Build CTM envelope + sign
    │
    ▼
Construct Solana transaction
    │ (Ed25519 preinstruction + enqueue_ctm ix)
    │ (~5-10ms)
    ▼
Submit to Solana RPC
    │ (skip-preflight for speed, or strict mode)
    │ (~20-100ms depending on RPC)
    ▼
Return sequence + tx_signature to caller
    │
    ▼
Emit relay_intent_accepted event → harness
```

### Backpressure and capacity

The relayer enforces backpressure to prevent overwhelming the chain:

| Parameter | Default | Purpose |
|---|---|---|
| `CTM_RELAYER_MAX_INFLIGHT` | 24 | Max concurrent unconfirmed Solana transactions |
| `CTM_RELAYER_MAX_QUEUED` | 96 | Max intents waiting for an inflight slot |
| `CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS` | 500 | How long an intent waits for a slot before rejection |
| `CTM_RELAYER_BLOCKHASH_CACHE_MS` | 500 | Blockhash reuse window (avoids RPC calls per tx) |
| `CTM_RELAYER_PRIORITIZATION_FEE` | 0 | Priority fee in micro-lamports (increase for mainnet) |

When `max_inflight` is reached, new intents queue up. When `max_queued` is also full, the relayer returns an error immediately — the client should retry with backoff.

### Health and metrics

HTTP server on port 9093 (default):

- `GET /healthz` — `{"ok": true}` if the relayer is accepting intents
- `GET /metrics` — Prometheus-format counters:
  - `execution_engine_requests_total` — total intents received
  - `execution_engine_requests_ok` — successfully submitted
  - `execution_engine_requests_error` — failed submissions
  - `execution_engine_inflight` — current in-flight count
  - `execution_engine_submit_send_avg_ms` — average RPC send latency
  - `execution_engine_sequence_flushes` — sequence persistence writes

### Embedded executor

The Rust relayer binary includes an **embedded cranker** (executor) that polls the on-chain queue and submits `execution_queue_execute` transactions automatically. This runs in a separate thread and is configurable:

| Parameter | Default | Purpose |
|---|---|---|
| `EXECUTION_QUEUE_ENGINE_ENABLED` | true | Enable/disable embedded executor |
| `EXECUTION_QUEUE_CRANK_INTERVAL_MS` | 25 | Poll frequency |
| `EXECUTION_QUEUE_CRANK_MAX_ITEMS` | 8 | Max items per execute transaction |

The executor is convenience, not a requirement — the execute instruction is permissionless and anyone can run a standalone cranker.

---

## 2. The Continuum State Harness

### What it does

The harness is the **read path** for Fermi. It maintains two independent views of market state:

- **Optimistic view** — updated immediately when the relayer accepts an intent, before the on-chain transaction even lands. This is what traders see in the UI.
- **Confirmed view** — updated only when transactions are finalized on-chain. This is ground truth.

The harness serves both views via a single REST API, with a `?view=optimistic|confirmed` query parameter.

### How optimistic state works

```
Relayer accepts intent (seq 317: buy 2 SOL @ $148)
    │
    ├──── submits enqueue_ctm tx to Solana
    │
    └──── emits relay_intent_accepted to harness
              │
              ▼
         Harness ContinuumStateEngine
              │
              ├── simulates the order against in-memory book
              │   (applies matching rules, updates positions)
              │
              └── optimistic view now shows the order
                  (resting on book or filled, depending on match)
```

The optimistic view is speculative — it assumes the enqueue transaction will land, the queue will execute in order, and the order will pass health checks. In practice, this is correct >95% of the time. The confirmed view is authoritative.

### Reconciliation

Every `CONTINUUM_HARNESS_RECONCILE_INTERVAL_MS` (default 10s), the harness compares optimistic vs confirmed state:

- Open order counts per market
- Bid/ask depth differences (L2 divergence)
- Best bid/ask price discrepancies
- Timestamp of last divergence

Reconciliation snapshots are available via the diagnostics API for monitoring.

### REST API endpoints

**Market state:**

| Endpoint | Description |
|---|---|
| `GET /state/markets/{marketId}` | Orderbook, metadata, oracle price, funding rate |
| `GET /state/full` | All markets + complete state snapshot |
| `GET /state/full?market={id}` | Single market full snapshot |

**User state:**

| Endpoint | Description |
|---|---|
| `GET /state/users/{owner}` | MangoAccounts, open orders, positions |
| `GET /state/balances/{owner}` | Token balances (available + reserved) |
| `GET /state/orders/{marketId}?owner={pubkey}` | User's orders in a specific market |

**Trade history:**

| Endpoint | Description |
|---|---|
| `GET /state/trades/{marketId}?limit=200` | Recent fills |
| `GET /state/candles/{marketId}?tf=1h&limit=500` | OHLCV candles (requires TimescaleDB) |

**Streaming:**

| Endpoint | Description |
|---|---|
| `GET /state/stream` | Server-Sent Events (SSE) — real-time updates |

**Diagnostics:**

| Endpoint | Description |
|---|---|
| `GET /healthz` | Readiness probe |
| `GET /livez` | Liveness probe |
| `GET /metrics` | Prometheus metrics |
| `GET /diagnostics/divergence` | Optimistic vs confirmed mismatch |
| `GET /diagnostics/reconciliation` | Latest reconciliation snapshot |

**All state endpoints** accept `?view=optimistic` (default) or `?view=confirmed`.

### Event ingestion

The harness receives events from two sources:

1. **Relayer events** — via `POST /ingest/relay-intent` (internal, not user-facing). Provides instant optimistic updates.
2. **On-chain subscription** — WebSocket connection to Solana RPC, watching the execution queue account and perp market accounts for confirmed state changes.

---

## 3. The HTTP Bridge

### What it does

The HTTP bridge translates REST API calls into gRPC calls to the relayer. It exists for clients that don't have a gRPC library (browsers, simple scripts, curl-based workflows).

### Endpoints

| Endpoint | Maps to |
|---|---|
| `GET /healthz` | Relayer connectivity + config status |
| `GET /relay/config?owner={pubkey}` | Default group, queue, market, account mapping |
| `POST /relay/submit-intent` | gRPC `SubmitIntent` (base64-encoded payload + signature) |

### Candle storage

The bridge also manages OHLCV candle data via TimescaleDB:

| Endpoint | Description |
|---|---|
| `POST /candles/ingest` | Insert price/size tick |
| `GET /candles/{market}?tf=1h&from={ms}&to={ms}&limit=500` | Fetch OHLCV |

Supported timeframes: 1m, 5m, 15m, 1h, 4h, 1d.

---

## How the Components Compose

### Normal operation (relayer up)

```
1. Trader signs intent → sends to relayer (gRPC :9090 or HTTP bridge :9092)
2. Relayer assigns sequence, CTM-signs, submits to Solana
3. Relayer emits event to harness → optimistic view updated immediately
4. Embedded executor cranks queue → order dispatched into orderbook
5. Harness sees on-chain confirmation → confirmed view updated
6. Trader reads state from harness (REST :9091) — sees optimistic fill
7. After 6-10 slots, confirmed view matches optimistic view
```

### Degraded operation (relayer down, harness up)

```
1. Trader detects relayer is unreachable (health check fails)
2. Trader submits enqueue_direct transaction directly to Solana
   (10-slot delay, no CTM co-signature)
3. Standalone cranker or another executor picks up the item
4. Harness sees on-chain state change → confirmed view updated
5. Optimistic view may be stale until reconciliation runs
```

### Emergency operation (all off-chain services down)

```
1. Trader submits enqueue_direct to Solana (or any direct instruction)
2. Any party can crank the queue by submitting execute transactions
3. State is read directly from on-chain accounts (Solana RPC or explorer)
4. Liquidation works normally (never depends on off-chain services)
```

The key design principle: **off-chain components are performance accelerators, not safety dependencies**.

---

## Deployment Topology

### Single-machine (development / testing)

All components run on one host, orchestrated by `startup_all_local.sh`:

```
Port 8899  — solana-test-validator (localnet)
Port 9090  — Rust relayer (gRPC)
Port 9091  — Continuum harness (HTTP)
Port 9092  — HTTP bridge (REST)
Port 9093  — Relayer health/metrics (HTTP)
Port 8070  — Frontend dev server (Vite)
Port 443   — nginx HTTPS gateway (proxies to frontend + harness)
```

### Production (recommended)

```
┌─────────────────────────────────────────────────┐
│  Relayer cluster (2+ instances, active-passive)  │
│  - gRPC :9090                                    │
│  - Health :9093                                  │
│  - Sequence state persisted to durable storage   │
│  - Embedded executor in each instance            │
└──────────────────────┬──────────────────────────┘
                       │ SWQoS RPCs
                       ▼
               Solana mainnet
                       │
                       ▼
┌─────────────────────────────────────────────────┐
│  Harness cluster (N instances, stateless reads)  │
│  - HTTP :9091                                    │
│  - Each instance subscribes to on-chain state    │
│  - Load-balanced behind reverse proxy            │
└──────────────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────┐
│  CDN / edge                                      │
│  - Frontend static assets                        │
│  - WebSocket passthrough for SSE stream          │
└─────────────────────────────────────────────────┘
```

For the relayer, only one instance should be active at a time (it owns the sequence counter). A passive standby can take over if the primary fails, loading sequence state from the persisted file.

The harness is stateless from a durability perspective — each instance builds its view from on-chain state and relayer events. Multiple instances can serve reads behind a load balancer.

---

## Configuration Reference

### Relayer (Rust execution engine)

| Variable | Default | Description |
|---|---|---|
| `CTM_RELAYER_BIND_ADDR` | `127.0.0.1:9090` | gRPC listen address |
| `CTM_RELAYER_HTTP_BIND_ADDR` | `127.0.0.1:9093` | Health/metrics HTTP address |
| `CTM_RELAYER_KEYPAIR_PATH` | — | Path to CTM signer keypair (Ed25519) |
| `CTM_RELAYER_PAYER_KEYPAIR_PATH` | — | Path to fee-payer keypair |
| `CTM_RELAYER_SEQUENCE_STATE_PATH` | `/tmp/ctm-seq.json` | Sequence persistence file |
| `CTM_RELAYER_MAX_INFLIGHT` | 24 | Max concurrent Solana transactions |
| `CTM_RELAYER_MAX_QUEUED` | 96 | Max queued intents |
| `CTM_RELAYER_PRIORITIZATION_FEE` | 0 | Priority fee (micro-lamports) |
| `CTM_RELAYER_BLOCKHASH_CACHE_MS` | 500 | Blockhash reuse window |
| `CLUSTER_URL_OVERRIDE` | — | Solana RPC URL |
| `EXECUTION_QUEUE_GROUP_PK` | — | Mango group pubkey |
| `EXECUTION_QUEUE_PK` | — | Queue account pubkey |

### Executor (embedded in relayer)

| Variable | Default | Description |
|---|---|---|
| `EXECUTION_QUEUE_ENGINE_ENABLED` | true | Enable embedded cranker |
| `EXECUTION_QUEUE_CRANK_INTERVAL_MS` | 25 | Poll frequency (ms) |
| `EXECUTION_QUEUE_CRANK_MAX_ITEMS` | 8 | Items per execute tx |
| `EXECUTION_QUEUE_CRANKER_KEYPAIR` | — | Fee-payer for execute txs |
| `EXECUTION_QUEUE_CRANK_LANES_JSON_PATH` | — | Pre-computed lane templates |

### Harness

| Variable | Default | Description |
|---|---|---|
| `CONTINUUM_HARNESS_BIND_ADDR` | `127.0.0.1:9091` | HTTP listen address |
| `CONTINUUM_HARNESS_RECONCILE_INTERVAL_MS` | 10000 | Reconciliation frequency |
| `CONTINUUM_HARNESS_EVENT_LOG_PATH` | — | Event log for replay/debug |
| `HARNESS_ENABLE_AIRDROP` | false | Enable test airdrop endpoint |

### HTTP Bridge

| Variable | Default | Description |
|---|---|---|
| `CTM_RELAYER_HTTP_BIND_ADDR` | `127.0.0.1:9092` | Bridge listen address |
| `CTM_RELAYER_ADDR` | `127.0.0.1:9090` | Relayer gRPC endpoint |

---

## Monitoring and Observability

### Health checks

```bash
# Relayer alive?
curl -sS http://127.0.0.1:9093/healthz
# {"ok":true}

# Harness alive?
curl -sS http://127.0.0.1:9091/healthz

# Bridge alive?
curl -sS http://127.0.0.1:9092/healthz

# Full stack (via nginx)
curl -k -sS https://127.0.0.1/harness/healthz
curl -k -sS https://127.0.0.1/bridge/healthz
```

### Key metrics to watch

| Metric | Alert threshold | Meaning |
|---|---|---|
| `execution_engine_inflight` | >20 sustained | Relayer approaching backpressure limit |
| `execution_engine_requests_error` | >5% of total | Submission failures (RPC issues, queue full) |
| `execution_engine_submit_send_avg_ms` | >200ms | RPC latency degradation |
| Harness divergence duration | >30s | Optimistic and confirmed views diverging |
| Queue gap count | >0 sustained | Missing sequences, possible relayer issue |

### Logs

All components write structured logs:

```
mng-v4/.localnet/logs/ctm-relayer.log           # Relayer
mng-v4/.localnet/logs/continuum-harness.log      # Harness
mng-v4/.localnet/logs/ctm-relayer-http-bridge.log # Bridge
```

For devnet deployments, substitute `.localnet` with `.devnet`.
