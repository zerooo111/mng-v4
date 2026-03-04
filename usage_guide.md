# Usage Guide

## Quickstart (Single Clean Run)

Run from repo root in one shell (starts relayer + cranker in background, then runs E2E):

```bash
export PATH="/tmp/solana-1.16.14/solana-release/bin:$HOME/.cargo/bin:/home/ec2-user/.local/share/solana/install/active_release/bin:$PATH"
export CLUSTER_OVERRIDE=devnet
export CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899
export CTM_RELAYER_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF
export MB_PAYER_KEYPAIR=/home/ec2-user/.config/solana/id.json
export GROUP_NUM=9110

# Build + deploy
cargo build-sbf --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl
solana program deploy \
  --url http://127.0.0.1:8899 \
  target/deploy/mango_v4.so \
  --program-id target/deploy/mango_v4-keypair.json

# Bootstrap local state
export EXECUTION_QUEUE_GROUP_NUM=$GROUP_NUM
export PERP_MARKET_INDEX=0
npm run -s execution-queue-local-perp-e2e-bootstrap

CFG="/tmp/execution-queue-e2e-${GROUP_NUM}.json"
LANES="/tmp/execution-queue-lanes-${GROUP_NUM}.json"
BUFFER_PK="$(node -p "require('${CFG}').executionQueueBuffer")"
GROUP_PK="$(node -p "require('${CFG}').group")"
QUEUE_PK="$(node -p "require('${CFG}').executionQueue")"

# Start relayer
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  CTM_RELAYER_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
  CTM_RELAYER_PAYER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_CTM_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
  EXECUTION_QUEUE_BUFFER_PK="$BUFFER_PK" \
  CTM_RELAYER_SEQUENCE_STATE_PATH="/tmp/ctm-sequences-${GROUP_NUM}.json" \
  CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
  ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts \
  >/tmp/ctm-relayer-${GROUP_NUM}.log 2>&1 &
RELAYER_PID=$!

# Start continuum state harness
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  CONTINUUM_HARNESS_BIND_ADDR=127.0.0.1:9091 \
  CONTINUUM_HARNESS_MODE=local \
  CONTINUUM_HARNESS_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  CONTINUUM_HARNESS_EVENT_LOG_PATH="/tmp/continuum-harness-${GROUP_NUM}.jsonl" \
  ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/continuum-state-harness.ts \
  >/tmp/continuum-harness-${GROUP_NUM}.log 2>&1 &
HARNESS_PID=$!

# Start cranker
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  EXECUTION_QUEUE_GROUP_PK="$GROUP_PK" \
  EXECUTION_QUEUE_PK="$QUEUE_PK" \
  EXECUTION_QUEUE_BUFFER_PK="$BUFFER_PK" \
  EXECUTION_QUEUE_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  EXECUTION_QUEUE_CRANKER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  EXECUTION_QUEUE_CRANK_LANES_JSON_PATH="$LANES" \
  EXECUTION_QUEUE_CRANK_MAX_ITEMS=8 \
  EXECUTION_QUEUE_CRANK_INTERVAL_MS=1000 \
  ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/execution-queue-cranker.ts \
  >/tmp/execution-queue-cranker-${GROUP_NUM}.log 2>&1 &
CRANKER_PID=$!

# Run E2E
export CTM_RELAYER_ADDR=127.0.0.1:9090
export E2E_OUTPUT_CONFIG_PATH="$CFG"
export E2E_MAKER_MAX_QUOTE_QTY=1000
export E2E_TAKER_MAX_QUOTE_QTY=1000
npm run -s execution-queue-local-perp-e2e-run

# Optional cleanup
kill $RELAYER_PID $CRANKER_PID $HARNESS_PID
```

## What Changed

This branch now includes an end-to-end execution-queue path for CTM-sequenced perp actions, plus local tooling to run and validate it.

### Program-side execution queue changes
- Execution queue uses a zero-copy external buffer account (`ExecutionQueueBuffer`) with capacity for 1000 items.
- CTM enqueue path verifies:
  - CTM ed25519 pre-instruction signature over canonical envelope.
  - User ed25519 pre-instruction signature over canonical intent message.
- Queue execute dispatches Mango instructions from queued payloads.
- Multi-lane cranking behavior improved:
  - If an execute call uses the wrong lane account set (`accounts_hash` mismatch), the queue head is now left untouched so the correct lane can process it.
  - This avoids dropping valid queued items when multiple lanes are used.

Modified file:
- `programs/mango-v4/src/instructions/execution_queue.rs`

### Client and scripts
- Added local full bootstrap script:
  - `ts/client/scripts/execution-queue/local-perp-e2e-bootstrap.ts`
- Added local full relayer-driven runner:
  - `ts/client/scripts/execution-queue/local-perp-e2e-run.ts`
- Added random SOL/USDC quoting bot (relayer path):
  - `ts/client/scripts/execution-queue/random-sol-usdc-quoter-bot.ts`
- Added package scripts:
  - `execution-queue-local-perp-e2e-bootstrap`
  - `execution-queue-local-perp-e2e-run`
  - `execution-queue-random-sol-usdc-quoter`

Modified file:
- `package.json`

## Random Quoter Bot

The quoter submits limit orders through the CTM relayer every 5s.

- Reference price source: CoinGecko Simple Price API (`solana/usd`) primary, on-chain SOL/perp oracle fallback.
- Price bands: bid in `[ref*(1-2%), ref]`, ask in `[ref, ref*(1+2%)]`.
- Size: random `1-3` SOL per quote.
- Flow per bot each tick:
  - optional `cancel_all` enqueue
  - `perp_place_order_v2` enqueue

Example:

```bash
export QUOTER_CONFIG_PATH=/home/ec2-user/stagin4/mng-v4/.localnet/run/execution-queue-e2e-9120.json
export CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899
export CTM_RELAYER_ADDR=127.0.0.1:9090
export QUOTER_INTERVAL_MS=5000
export QUOTER_PRICE_RANGE_BPS=200
export QUOTER_SIZE_MIN_SOL=1
export QUOTER_SIZE_MAX_SOL=3
export QUOTER_COINGECKO_ASSET_ID=solana
export QUOTER_COINGECKO_VS_CURRENCY=usd
npm run -s execution-queue-random-sol-usdc-quoter
```

## Prerequisites

- Local validator running at `http://127.0.0.1:8899`
- Solana CLI + cargo tools available
- Node dependencies installed (`npm install`)

Recommended PATH export (same pattern used in this workspace):

```bash
export PATH="$HOME/.cargo/bin:/home/ec2-user/.local/share/solana/install/active_release/bin:$PATH"
```

## Build and Deploy Program

```bash
export PATH="/tmp/solana-1.16.14/solana-release/bin:$HOME/.cargo/bin:/home/ec2-user/.local/share/solana/install/active_release/bin:$PATH"
cargo build-sbf --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl

solana program deploy \
  --url http://127.0.0.1:8899 \
  target/deploy/mango_v4.so \
  --program-id target/deploy/mango_v4-keypair.json
```

## SBF Build Notes (Important)

This repo now uses a local patched OpenBook dependency for reliable SBF builds:

- `openbook-v2` is sourced from:
  - `third-party/openbook-v2-patched/programs/openbook-v2`
- workspace dependency is pinned in:
  - `Cargo.toml` (`[workspace.dependencies].openbook-v2`)

Why this patch exists:

- Upstream OpenBook + current toolchain produced SBF failures from:
  - duplicate `Pod/Zeroable` impls on zero-copy structs,
  - BPF stack-frame overflows in heavy external oracle account parsers.

Local patch behavior:

- keeps OpenBook support needed by Mango,
- removes problematic duplicate derives in OpenBook orderbook structs,
- disables Switchboard/Raydium oracle parsing in patched OpenBook oracle module.

Mango-side build stability change:

- direct `switchboard-*` dependencies were removed from `programs/mango-v4/Cargo.toml`,
- Mango oracle parsing now targets Pyth/Stub/CLMM paths used in local flow.

If future upstream versions are restored:

1. revert `Cargo.toml` `openbook-v2` path override back to a git rev,
2. reintroduce required external oracle deps only after `cargo build-sbf` is clean,
3. run:
   - `cargo check --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl`
   - `cargo build-sbf --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl`

## Local Bootstrap (Group + Queue + Users + Market)

Use a fresh group number for each clean test run.

```bash
export CLUSTER_OVERRIDE=devnet
export CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899
export CTM_RELAYER_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF
export MB_PAYER_KEYPAIR=/home/ec2-user/.config/solana/id.json
export EXECUTION_QUEUE_GROUP_NUM=9107
export PERP_MARKET_INDEX=0

npm run -s execution-queue-local-perp-e2e-bootstrap
```

This writes config artifacts like:
- `/tmp/execution-queue-e2e-<GROUP_NUM>.json`
- `/tmp/execution-queue-lanes-<GROUP_NUM>.json`

## Start CTM Relayer (gRPC :9090)

```bash
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  CTM_RELAYER_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
  CTM_RELAYER_PAYER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_CTM_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
  EXECUTION_QUEUE_BUFFER_PK=<from bootstrap output> \
  CTM_RELAYER_SEQUENCE_STATE_PATH=/tmp/ctm-sequences-<GROUP_NUM>.json \
  CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
  ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts
```

## Start Continuum State Harness (Optimistic + Confirmed API)

```bash
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  CONTINUUM_HARNESS_BIND_ADDR=127.0.0.1:9091 \
  CONTINUUM_HARNESS_MODE=local \
  CONTINUUM_HARNESS_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  CONTINUUM_HARNESS_EVENT_LOG_PATH=/tmp/continuum-harness-<GROUP_NUM>.jsonl \
  ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/continuum-state-harness.ts
```

Quick checks:

```bash
curl -s http://127.0.0.1:9091/healthz | jq
curl -s 'http://127.0.0.1:9091/state/markets/0?view=optimistic' | jq
curl -s 'http://127.0.0.1:9091/state/balances/<owner-pubkey>?view=optimistic' | jq
curl -s 'http://127.0.0.1:9091/state/trades/0?view=confirmed&limit=200' | jq
curl -s 'http://127.0.0.1:9091/state/candles/0?view=confirmed&resolution_sec=60&limit=200' | jq
curl -N http://127.0.0.1:9091/state/stream
```

Full endpoint and schema reference:
- `api.md`

## Harness Verification (Phase 5)

Consistency check between harness `confirmed` view and live on-chain Mango perp open orders:

```bash
export VERIFY_GROUP_PK=<group-pubkey>
export CLUSTER_OVERRIDE=devnet
export CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899
export CONTINUUM_HARNESS_BASE_URL=http://127.0.0.1:9091
export VERIFY_VIEW=confirmed

npm run -s continuum-state-harness-verify
```

Deterministic replay check against captured harness event log:

```bash
export CONTINUUM_HARNESS_EVENT_LOG_PATH=/tmp/continuum-harness-<GROUP_NUM>.jsonl
export REPLAY_CHECK_ITERATIONS=25

npm run -s continuum-state-harness-replay-check
```

## Start Cranker

```bash
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  EXECUTION_QUEUE_GROUP_PK=<from bootstrap output> \
  EXECUTION_QUEUE_PK=<from bootstrap output> \
  EXECUTION_QUEUE_BUFFER_PK=<from bootstrap output> \
  EXECUTION_QUEUE_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  EXECUTION_QUEUE_CRANKER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  EXECUTION_QUEUE_CRANK_LANES_JSON_PATH=/tmp/execution-queue-lanes-<GROUP_NUM>.json \
  EXECUTION_QUEUE_CRANK_MAX_ITEMS=8 \
  EXECUTION_QUEUE_CRANK_INTERVAL_MS=1000 \
  ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/execution-queue-cranker.ts
```

## Run End-to-End Test

```bash
export CLUSTER_OVERRIDE=devnet
export CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899
export CTM_RELAYER_ADDR=127.0.0.1:9090
export E2E_OUTPUT_CONFIG_PATH=/tmp/execution-queue-e2e-<GROUP_NUM>.json
export E2E_MAKER_MAX_QUOTE_QTY=1000
export E2E_TAKER_MAX_QUOTE_QTY=1000

npm run -s execution-queue-local-perp-e2e-run
```

Expected output includes:
- `status: "ok"`
- relayer submissions for maker/taker/cancel (sequence + tx signatures)
- non-zero maker/taker perp base lots with opposite signs
- positive USDC balances

## Optional State Verification

You can verify queue/account state after a run:

- Queue account count should be zero.
- Maker/taker open orders should be zero after cancel-all.
- Maker and taker perp positions should be opposite signs.

## Notes

- Re-running against the same group accumulates position history; use a fresh `EXECUTION_QUEUE_GROUP_NUM` for clean one-shot validation.
- `Error while loading price impact: TypeError: fetch failed` logs are external price-impact fetch warnings and do not block local validator E2E queue flow.
