# Execution Queue Relayer + Cranker

## CTM Sequencer Relayer (gRPC :9090)

Runs a gRPC server that accepts user-signed intents, assigns a **market-specific sequence number**, applies CTM envelope signing, and submits `execution_queue_enqueue_ctm`.

### Start

```bash
CTM_RELAYER_BIND_ADDR=0.0.0.0:9090 \
CLUSTER_OVERRIDE=devnet \
CLUSTER_URL_OVERRIDE=https://api.devnet.solana.com \
CTM_RELAYER_PAYER_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_CTM_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_PROGRAM_ID=<optional-program-id-override> \
CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
CTM_RELAYER_EVENT_SINK_AUTH_TOKEN=<optional-token> \
yarn ctm-sequencer-relayer
```

Rust execution engine drop-in:

```bash
CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR=127.0.0.1:9093 \
CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
CTM_RELAYER_PAYER_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_CTM_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
cargo run -p service-mango-execution-engine
```

The startup scripts now support `CTM_RELAYER_IMPL=rust` and keep the same gRPC submit address on `:9090`. The Rust engine exposes `GET /healthz` and `GET /metrics` on `CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR`.
`EXECUTION_QUEUE_BUFFER_PK` is now optional and treated as an alias of `EXECUTION_QUEUE_PK` for older tooling.

### gRPC API

Proto: `ts/client/scripts/execution-queue/ctm_sequencer.proto`

`SubmitIntent` request fields:
- `group`, `execution_queue`, `market`
- `payload` (queue payload bytes)
- `remaining_accounts` (account metas for dispatch)
- `min_execute_slot`, `expires_at_slot`
- `user_owner`, `mango_account`
- `user_signature` (ed25519 signature over canonical user-intent message)

## Continuum State Harness (HTTP + SSE :9091)

Reads relay-ingested intents and on-chain queue events, then exposes optimistic and confirmed state from a single API.

### Start

```bash
CONTINUUM_HARNESS_BIND_ADDR=0.0.0.0:9091 \
CONTINUUM_HARNESS_MODE=local \
CONTINUUM_HARNESS_EVENT_LOG_PATH=/tmp/continuum-harness-events.jsonl \
CONTINUUM_HARNESS_PROGRAM_ID=<optional-program-id-override> \
CLUSTER_OVERRIDE=devnet \
CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
yarn continuum-state-harness
```

### Key Endpoints

- `POST /ingest/relay-intent`
- `GET /state/markets/:market?view=optimistic|confirmed`
- `GET /state/users/:owner?view=optimistic|confirmed`
- `GET /state/balances/:owner?view=optimistic|confirmed`
- `GET /state/orders/:market?owner=<pubkey>&view=optimistic|confirmed`
- `GET /state/trades/:market?view=optimistic|confirmed&limit=200`
- `GET /state/candles/:market?view=optimistic|confirmed&resolution_sec=60&limit=200`
- `GET /state/queue/:market`
- `GET /state/full?market=<id>&view=optimistic|confirmed`
- `GET /state/stream` (SSE)
- `GET /healthz`, `GET /metrics`, `GET /diagnostics/divergence`

### Verification (Phase 5)

Chain consistency check (`confirmed` harness view vs on-chain Mango perp open orders):

```bash
VERIFY_GROUP_PK=<group-pubkey> \
CLUSTER_OVERRIDE=devnet \
CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
CONTINUUM_HARNESS_BASE_URL=http://127.0.0.1:9091 \
VERIFY_VIEW=confirmed \
yarn continuum-state-harness-verify
```

Deterministic replay check (shuffle-replay harness JSONL log):

```bash
CONTINUUM_HARNESS_EVENT_LOG_PATH=/tmp/continuum-harness-events.jsonl \
REPLAY_CHECK_ITERATIONS=25 \
yarn continuum-state-harness-replay-check
```

## Execution Queue Cranker

Periodically submits `execution_queue_execute` using configured account-meta lanes.

### Start

```bash
EXECUTION_QUEUE_GROUP_PK=<group-pk> \
EXECUTION_QUEUE_PK=<execution-queue-pk> \
EXECUTION_QUEUE_CRANKER_KEYPAIR=~/.config/solana/id.json \
EXECUTION_QUEUE_PROGRAM_ID=<optional-program-id-override> \
EXECUTION_QUEUE_CRANK_MAX_ITEMS=4 \
EXECUTION_QUEUE_CRANK_INTERVAL_MS=1500 \
EXECUTION_QUEUE_CRANK_LANES_JSON='[
  {
    "name":"btc-perp",
    "remainingAccounts":[
      {"pubkey":"<group>","isWritable":false},
      {"pubkey":"<mango-account>","isWritable":true},
      {"pubkey":"<execution-queue>","isWritable":false},
      {"pubkey":"<perp-market>","isWritable":true},
      {"pubkey":"<bids>","isWritable":true},
      {"pubkey":"<asks>","isWritable":true},
      {"pubkey":"<event-queue>","isWritable":true},
      {"pubkey":"<oracle>","isWritable":false}
    ]
  }
]' \
yarn execution-queue-cranker
```

Notes:
- Lane account metas must match the `accounts_hash` for queued items.
- Multiple lanes can be configured (one per market/account-meta set).
