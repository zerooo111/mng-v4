# Usage Guide

## Quickstart (Single Clean Run)

Run from `mng-v4` in one shell. This uses the local launcher, which now defaults to the Rust relayer on `127.0.0.1:9090` and exposes health/metrics on `127.0.0.1:9093`.

```bash
cd /home/ec2-user/stagin4/mng-v4

DEPLOY_TIMEOUT_SECS=45 \
DEPLOY_RETRIES=0 \
EXECUTION_QUEUE_ENGINE_ENABLED=false \
./startup_local.sh restart
```

## Quickstart (Devnet)

Use the already-deployed devnet program with the same launcher:

```bash
cd /home/ec2-user/stagin4/mng-v4

STACK_CLUSTER=devnet \
PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
./startup_local.sh restart
```

Devnet mode changes:
- runtime artifacts are written to `.devnet/run`
- the launcher uses `https://api.devnet.solana.com` unless `SOLANA_URL` is overridden
- maker/taker are read from `keypairs/`
- startup/bootstrap do not auto-fund or auto-generate users

Required persistent keypairs:
- [execution-queue-maker.json](/home/ec2-user/stagin4/mng-v4/keypairs/execution-queue-maker.json)
- [execution-queue-taker.json](/home/ec2-user/stagin4/mng-v4/keypairs/execution-queue-taker.json)

The default local path now uses the embedded Rust executor/cranker. Start the external TS cranker
in a second shell only if you explicitly disable the Rust executor:

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

Run the E2E flow from the first shell:

```bash
cd /home/ec2-user/stagin4/mng-v4
export CLUSTER_OVERRIDE=devnet
export CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899
export CTM_RELAYER_ADDR=127.0.0.1:9090
export E2E_OUTPUT_CONFIG_PATH=.localnet/run/execution-queue-e2e-9120.json
export E2E_MAKER_MAX_QUOTE_QTY=1000
export E2E_TAKER_MAX_QUOTE_QTY=1000
npm run -s execution-queue-local-perp-e2e-run
```

Optional cleanup:

```bash
cd /home/ec2-user/stagin4/mng-v4
./startup_local.sh stop
```

Legacy fallback:

```bash
cd /home/ec2-user/stagin4/mng-v4
CTM_RELAYER_IMPL=ts ./startup_local.sh restart
```

## Devnet E2E Run

After the devnet stack is up:

```bash
cd /home/ec2-user/stagin4/mng-v4
STACK_CLUSTER=devnet ./startup_local.sh run-e2e
```

If the saved keypairs need SOL on devnet, fund them manually before bootstrap/E2E.

## What Changed

This branch now includes an end-to-end execution-queue path for CTM-sequenced perp actions, plus local tooling to run and validate it.

### Program-side execution queue changes
- Execution queue uses a single zero-copy account with ring-buffer semantics and capacity for 1024 CTM items plus 128 liquidity items.
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

Single-process multi-bot pacing is also supported:

- `QUOTER_BOT_DISPATCH_MODE=all`: existing behavior, every loaded bot submits each tick
- `QUOTER_BOT_DISPATCH_MODE=round-robin`: one loaded bot submits per tick, rotating across bots
- `QUOTER_BOT_DISPATCH_MODE=random-one`: one random loaded bot submits per tick

For the current devnet stack, a helper launcher provisions a small bot set if needed and starts a round-robin SOL/USDC quoter at roughly one order transaction every 5 seconds:

```bash
cd /home/ec2-user/stagin4
./start_devnet_quoter_bots.sh restart
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

As of 2026-03-13, treat the build as failed unless the log contains no `^Error:` lines. In this repo, `cargo build-sbf` can still print `Finished release profile [optimized]` while emitting hard SBF stack-frame errors for Anchor-generated `try_accounts`.

Recommended check:

```bash
cargo build-sbf --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl 2>&1 | tee /tmp/mango-build-sbf.log
rg -n "^Error:" /tmp/mango-build-sbf.log
```

If any `Error:` lines are present, do not trust `target/deploy/mango_v4.so` as a fresh artifact.

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

## Current SBF Artifact Blocker (2026-03-13)

The old OpenBook and Switchboard cleanup is not the active blocker anymore. The current blocker is compile-time SBF stack-frame overflow in Anchor-generated `Accounts::try_accounts`, which prevents us from trusting a rebuilt `mango_v4.so`.

Currently failing `Accounts` contexts:

- `OpenbookV2LiqForceCancelOrders` in `programs/mango-v4/src/accounts_ix/openbook_v2_liq_force_cancel_orders.rs`
- `OpenbookV2PlaceOrder` in `programs/mango-v4/src/accounts_ix/openbook_v2_place_order.rs`
- `Serum3RegisterMarket` in `programs/mango-v4/src/accounts_ix/serum3_register_market.rs`
- `TokenAddBank` in `programs/mango-v4/src/accounts_ix/token_add_bank.rs`
- `TokenRegister` in `programs/mango-v4/src/accounts_ix/token_register.rs`
- `TokenRegisterTrustless` in `programs/mango-v4/src/accounts_ix/token_register_trustless.rs`

Representative current stack overflows:

- `OpenbookV2LiqForceCancelOrders::try_accounts`: `5056 > 4096`
- `OpenbookV2PlaceOrder::try_accounts`: `4400 > 4096`
- `Serum3RegisterMarket::try_accounts`: `4104 > 4096`
- `TokenAddBank::try_accounts`: `5424 > 4096`
- `TokenRegister::try_accounts`: `5048 > 4096`
- `TokenRegisterTrustless::try_accounts`: `5048 > 4096`

Why this matters for local testing:

- startup scripts deploy `target/deploy/mango_v4.so`
- if `cargo build-sbf` emits these stack errors, the local validator restart may still be running an older artifact
- source changes can then diverge from measured runtime behavior, which is exactly what happened during recent execution-queue optimization work

Recommended fix direction:

1. shrink or split the listed `#[derive(Accounts)]` contexts before expecting a fresh deploy
2. move setup-heavy `init` flows out of already-large account validation paths
3. rerun `cargo build-sbf` and verify `rg -n "^Error:" /tmp/mango-build-sbf.log` is empty

Anchor and Solana references:

- Anchor 0.31 release notes: `try_accounts` is the main stack hotspot, and multiple `init` constraints are a known contributor
  - <https://www.anchor-lang.com/docs/updates/release-notes/0-31-0>
- Solana program FAQ: stack frame limits are strict and warnings must be resolved for used code paths
  - <https://solana.com/docs/programs/faq>

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

## Start Rust Relayer (Default gRPC :9090)

```bash
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
  CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR=127.0.0.1:9093 \
  CTM_RELAYER_PAYER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_CTM_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
  EXECUTION_QUEUE_GROUP_PK=<group-pubkey> \
  EXECUTION_QUEUE_PK=<queue-pubkey> \
  EXECUTION_QUEUE_ENGINE_ENABLED=false \
  EXECUTION_QUEUE_CRANK_LANES_JSON_PATH=/tmp/execution-queue-lanes-<GROUP_NUM>.json \
  CTM_RELAYER_SEQUENCE_STATE_PATH=/tmp/ctm-sequences-<GROUP_NUM>.json \
  CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
  cargo run -p service-mango-execution-engine
```

Quick checks:

```bash
curl -s http://127.0.0.1:9093/healthz | jq
curl -s http://127.0.0.1:9093/metrics | rg 'execution_engine_(requests|execute|sequence)'
```

The local stack scripts now default to `CTM_RELAYER_IMPL=rust` and
`EXECUTION_QUEUE_ENGINE_ENABLED=true`, so the Rust relayer also drains the execution queue by
default. Set `CTM_RELAYER_IMPL=ts` only if you need the legacy TS relayer path, or set
`EXECUTION_QUEUE_ENGINE_ENABLED=false` if you intentionally want the external TS cranker. The gRPC
submit endpoint stays on `127.0.0.1:9090`; engine health and metrics are available on
`127.0.0.1:9093`.
`EXECUTION_QUEUE_BUFFER_PK` is optional and only kept as a compatibility alias to `EXECUTION_QUEUE_PK`.

### Legacy TS Relayer Fallback

```bash
env \
  CLUSTER_OVERRIDE=devnet \
  CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
  CTM_RELAYER_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF \
  CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
  CTM_RELAYER_PAYER_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_CTM_KEYPAIR=/home/ec2-user/.config/solana/id.json \
  CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
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
