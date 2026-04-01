# Execution Queue Relayer + Cranker

## Working E2E Pipeline

This is the current maintained path for:
- local validator testing
- devnet testing against a deployed Mango program
- Rust relayer for submit
- embedded Rust executor/cranker by default
- one intent per place tx

Safety defaults now in effect:
- no auto-generated maker/taker keypairs
- no auto-generated quoter bot keypairs
- no bootstrap-time SOL auto-funding
- persistent script keypairs live under `mng-v4/keypairs`

### 1. Start the stack

```bash
PRELOAD_PROGRAM_IN_VALIDATOR=0 \
RESET_VALIDATOR=1 \
EXECUTION_QUEUE_ENGINE_ENABLED=false \
./startup_local.sh restart
```

Devnet:

```bash
STACK_CLUSTER=devnet \
PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
./startup_local.sh restart
```

Expected healthy ports:
- localnet validator RPC: `http://127.0.0.1:8899`
- devnet RPC: external, usually `https://api.devnet.solana.com`
- relayer gRPC: `127.0.0.1:9090`
- relayer HTTP health/metrics: `http://127.0.0.1:9093`
- harness HTTP: `http://127.0.0.1:9091`

### 2. Start the external cranker only if Rust execute is disabled

```bash
cd /home/ec2-user/stagin4/mng-v4

env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  EXECUTION_QUEUE_GROUP_PK=$(node -p "require('./.localnet/run/execution-queue-e2e-9120.json').group") \
  EXECUTION_QUEUE_PK=$(node -p "require('./.localnet/run/execution-queue-e2e-9120.json').executionQueue") \
  EXECUTION_QUEUE_BUFFER_PK=$(node -p "require('./.localnet/run/execution-queue-e2e-9120.json').executionQueueBuffer") \
  EXECUTION_QUEUE_PROGRAM_ID=$(node -p "require('./.localnet/run/execution-queue-e2e-9120.json').programId") \
  EXECUTION_QUEUE_CRANKER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  EXECUTION_QUEUE_CRANK_LANES_JSON_PATH=.localnet/run/execution-queue-lanes-9120.json \
  EXECUTION_QUEUE_CRANK_RELAY_EVENT_LOG_PATH=.localnet/run/continuum-harness-9120.jsonl \
  EXECUTION_QUEUE_CRANK_MAX_ITEMS=8 \
  EXECUTION_QUEUE_CRANK_INTERVAL_MS=1000 \
  node -r ts-node/register/transpile-only \
  EXECUTION_QUEUE_ENGINE_ENABLED=false \
  ts/client/scripts/execution-queue/execution-queue-cranker.ts
```

Important:
- `EXECUTION_QUEUE_PROGRAM_ID` must be set on localnet. If omitted, the cranker falls back to `MANGO_V4_ID[CLUSTER]` and can target the wrong program id.
- `EXECUTION_QUEUE_ENGINE_ENABLED=true` is now the default local mode and enables the embedded Rust executor.
- Set `EXECUTION_QUEUE_ENGINE_ENABLED=false` only when you intentionally want the external TS cranker.
- `CTM_RELAYER_IMPL=rust` is now the default startup mode. Set `CTM_RELAYER_IMPL=ts` only if you need the legacy TS relayer path.
- In `STACK_CLUSTER=devnet`, runtime config defaults move from `.localnet/run` to `.devnet/run`.
- Maker/taker now come from `keypairs/execution-queue-maker.json` and `keypairs/execution-queue-taker.json`.

### 3. Run the E2E test

```bash
CLUSTER_OVERRIDE=devnet \
CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
CTM_RELAYER_ADDR=127.0.0.1:9090 \
E2E_OUTPUT_CONFIG_PATH=.localnet/run/execution-queue-e2e-9120.json \
E2E_MAKER_MAX_QUOTE_QTY=1000 \
E2E_TAKER_MAX_QUOTE_QTY=1000 \
npm run -s execution-queue-local-perp-e2e-run
```

Devnet launcher equivalent:

```bash
STACK_CLUSTER=devnet ./startup_local.sh run-e2e
```

Expected result:
- JSON output with `"status": "ok"`
- relayer submissions for maker place, taker place, and cancel
- queue drains back to `count=0`

Last validated result on `2026-03-13`:
- `status: ok`
- maker sequence `0`
- taker sequence `1`
- cancel sequence `2`
- final positions:
  - maker `10000` base lots
  - taker `-10000` base lots

### 4. Quick checks

```bash
./startup_local.sh status
curl -s http://127.0.0.1:9093/metrics | rg 'execution_engine_(requests|execute)'
npx ts-node --transpile-only ts/client/scripts/execution-queue/inspect-queue-head.ts \
  .localnet/run/execution-queue-e2e-9120.json
```

Healthy expectations:
- `status` shows validator / harness / relayer running
- queue inspector reports `count=0`
- relayer request counters advance

### Current recommendation

For local integration testing, prefer:
- Rust relayer for submit
- embedded Rust executor/cranker for execute by default

This is now the default local startup path. The embedded Rust executor is still under separate debugging and should not be treated as the default clean-path execute runner yet.

## Rust Relayer (Default gRPC :9090)

Runs a gRPC server that accepts user-signed intents, assigns a **market-specific sequence number**, applies CTM envelope signing, and submits `execution_queue_enqueue_ctm`.

### Start

```bash
CTM_RELAYER_BIND_ADDR=0.0.0.0:9090 \
CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR=127.0.0.1:9093 \
CLUSTER_OVERRIDE=devnet \
CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
CTM_RELAYER_PAYER_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_CTM_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
CTM_RELAYER_HARNESS_BASE_URL=http://127.0.0.1:9091 \
EXECUTION_QUEUE_GROUP_PK=<group-pubkey> \
EXECUTION_QUEUE_PK=<queue-pubkey> \
EXECUTION_QUEUE_ENGINE_ENABLED=false \
EXECUTION_QUEUE_CRANK_LANES_JSON_PATH=.localnet/run/execution-queue-lanes-9120.json \
CTM_RELAYER_SEQUENCE_STATE_PATH=.localnet/run/ctm-sequences-9120.json \
cargo run -p service-mango-execution-engine
```

The startup scripts now default to `CTM_RELAYER_IMPL=rust` and
`EXECUTION_QUEUE_ENGINE_ENABLED=true`, while keeping the same gRPC submit address on `:9090`. The
Rust engine exposes `GET /healthz` and `GET /metrics` on
`CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR`.
`EXECUTION_QUEUE_BUFFER_PK` is now optional and treated as an alias of `EXECUTION_QUEUE_PK` for older tooling.
When `CTM_RELAYER_HARNESS_BASE_URL` is set, submit now performs a cached pre-enqueue harness gate:
`/healthz` must be fresh, on-chain reconciliation must be fresh when enabled, and any market with
active harness reconciliation drift is rejected before the relayer sends the enqueue transaction.

Quick checks:

```bash
curl -s http://127.0.0.1:9093/healthz
curl -s http://127.0.0.1:9093/metrics | rg 'execution_engine_(requests|execute|sequence)'
```

Additional executor metrics now exposed by the Rust engine:
- `execution_engine_submit_parse_avg_ms`
- `execution_engine_submit_prepare_avg_ms`
- `execution_engine_submit_send_avg_ms`
- `execution_engine_harness_submit_rejects_total`
- `execution_engine_execute_head_missing_total`
- `execution_engine_execute_head_blocked_total`
- `execution_engine_execute_confirmed_no_advance_total`
- `execution_engine_execute_targeted_total`
- `execution_engine_execute_speculative_total`

The executor now defaults to reason-coded head inspection and only uses speculative execute sends
for queue-gap recovery (`EXECUTION_QUEUE_CRANK_SAFE_SPECULATIVE=true`).

### Devnet RPC Provider Selection

The persistent devnet stack can switch write-path RPC providers for the relayer and quoter via:
- `default`: public Solana devnet RPC
- `helius`: Helius devnet RPC
- `triton`: Triton / RPCPool devnet RPC

Render the systemd env with one of these:

```bash
./scripts/render_devnet_runtime.sh --rpc-provider default
./scripts/render_devnet_runtime.sh --rpc-provider helius --helius-api-key <key>
./scripts/render_devnet_runtime.sh --rpc-provider triton --triton-rpc-url https://<endpoint>.devnet.rpcpool.com/<token>
```

For Triton websocket-compatible subscriptions, the runtime renderer derives
`CLUSTER_WS_URL_OVERRIDE` from the HTTPS endpoint and appends `/whirligig`
unless you pass `--triton-ws-url` explicitly.

### Legacy TS Relayer Fallback

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

## Local Benchmark Reports

`random-sol-usdc-quoter-bot.ts` can now emit a run summary JSON artifact:

```bash
QUOTER_REPORT_PATH=.localnet/run/quoter-report.json \
QUOTER_MAX_RUNTIME_MS=20000 \
QUOTER_COINGECKO_REFRESH_MS=60000 \
QUOTER_LOG_EACH_ORDER=false \
./node_modules/.bin/ts-node ts/client/scripts/execution-queue/random-sol-usdc-quoter-bot.ts
```

Useful knobs:
- `QUOTER_REPORT_PATH`: write a JSON summary with TPS, queue count, relayer metrics, and errors
- `QUOTER_MAX_RUNTIME_MS` or `QUOTER_MAX_TICKS`: end the run automatically
- `QUOTER_RELAYER_METRICS_URL`: override the metrics endpoint for the report snapshot

The report includes:
- average and peak place TPS / total intent TPS
- max inflight submits and tick error count
- end-of-run queue count
- relayer submit/execute metrics snapshot
