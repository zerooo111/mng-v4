# mango-v4

This repository contains a Mango v4-based Solana trading stack. In addition to the on-chain program, it includes the execution-queue extensions used by the current Fermi workflows, Rust and TypeScript clients, relayer and executor services, a continuum state harness, and local/devnet operational scripts.

If you want the architecture first, start with [docs/Overview.md](docs/Overview.md). If you want the maintained execution-queue flow, start with [ts/client/scripts/execution-queue/README.md](ts/client/scripts/execution-queue/README.md).

## What Is Here

- `programs/mango-v4`: the main Anchor program and on-chain execution-queue logic
- `programs/margin-trade`: auxiliary on-chain program kept in the repo, but excluded from the Anchor workspace
- `lib/`: Rust libraries, including the Rust client and shared service code
- `bin/`: operational binaries such as the CLI, keeper, liquidator, settler, crank, and execution-engine services
- `ts/client`: the published TypeScript client package plus scripts for bootstrap, relaying, cranking, and diagnostics
- `rust-harness`: native N-API backend used by the continuum harness
- `anchor-tests`: Anchor/TypeScript integration tests
- `docs/`: high-level architecture and component documentation
- `formal_verification/`: Lean models, specs, and proofs for protocol invariants

## Toolchain

The repo mixes Rust, Solana/Anchor, and TypeScript tooling. The baseline setup is:

- Rust `1.70` from [rust-toolchain.toml](rust-toolchain.toml)
- Solana CLI and SBF toolchain `1.16.x`
- Anchor CLI `0.28.x`
- Node.js `16.x`
- Yarn `1.x`

For the local startup scripts, you will also want `curl`, `rg`, `ss`, `timeout`, and `solana-test-validator` available on `PATH`.

## Getting Started

Install dependencies:

```bash
yarn install --frozen-lockfile
cargo build --workspace
```

Build the TypeScript client package:

```bash
yarn build
```

Build a fresh program artifact when you need `target/deploy/mango_v4.so`:

```bash
cargo +solana build-sbf --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl
```

Important: this repository currently has SBF build caveats. Do not trust a fresh `mango_v4.so` just because the build ends with `Finished`; check [FAQ-DEV.md](FAQ-DEV.md) for the current failure mode and validation steps.

## Local Stack Quick Start

The maintained local path uses:

- the Rust relayer by default
- the embedded Rust execution engine by default
- the Rust harness backend by default

Start the local stack:

```bash
./startup_local.sh restart
./startup_local.sh status
```

Run the relayer-driven end-to-end flow:

```bash
./startup_local.sh run-e2e
```

Stop everything:

```bash
./startup_local.sh stop
```

By default the local launcher uses:

- Solana RPC: `http://127.0.0.1:8899`
- relayer gRPC: `127.0.0.1:9090`
- harness HTTP: `127.0.0.1:9091`
- relayer health and metrics: `http://127.0.0.1:9093`
- runtime artifacts: `.localnet/`

Useful overrides:

```bash
RESET_VALIDATOR=1 ./startup_local.sh restart
BUILD_SBF=1 ./startup_local.sh restart
CTM_RELAYER_IMPL=ts ./startup_local.sh restart
EXECUTION_QUEUE_ENGINE_ENABLED=false ./startup_local.sh restart
# Fallback/debug only: forces the legacy TS replay backend.
HARNESS_BACKEND=ts-backend ./startup_local.sh restart
```

## Devnet Quick Start

Reuse the same launcher against devnet:

```bash
STACK_CLUSTER=devnet ./startup_local.sh restart
STACK_CLUSTER=devnet ./startup_local.sh run-e2e
```

Common overrides:

```bash
STACK_CLUSTER=devnet SOLANA_URL=https://api.devnet.solana.com ./startup_local.sh restart
STACK_CLUSTER=devnet PROGRAM_ID=<program-id> ./startup_local.sh restart
```

Devnet runtime state is written under `.devnet/`.

Persistent devnet harness deploys must keep the low-latency wrapper profile in
`.devnet/systemd/devnet-stack.env`:

```bash
CONTINUUM_HARNESS_BACKEND=rust-backend
CONTINUUM_HARNESS_SANITY_INTERVAL_MS=0
CONTINUUM_HARNESS_MARKET_STATS_INTERVAL_MS=0
CONTINUUM_HARNESS_RECONCILE_INTERVAL_MS=30000
CONTINUUM_HARNESS_ONCHAIN_CACHE_TTL_MS=5000
CONTINUUM_HARNESS_FRONTEND_REFRESH_INTERVAL_MS=10000
CONTINUUM_HARNESS_READ_FAILURE_WINDOW_MS=30000
CONTINUUM_HARNESS_READ_FAILURE_MIN_SAMPLES=2
CONTINUUM_HARNESS_READ_FAILURE_RATE_THRESHOLD=0.15
CONTINUUM_HARNESS_READ_SLOW_THRESHOLD_MS=200
CONTINUUM_HARNESS_READ_SLOW_MIN_SAMPLES=3
CONTINUUM_HARNESS_READ_SLOW_RATE_THRESHOLD=0.20
CONTINUUM_HARNESS_READ_IMMEDIATE_FAILOVER_ON_ERROR=true
CONTINUUM_HARNESS_READ_IMMEDIATE_SLOW_THRESHOLD_MS=750
CTM_RELAYER_LOCAL_STATE=true
CTM_RELAYER_HARNESS_REJECT_MARKET_DRIFT=false
```

Important: `rust-backend` still means the `:9091` HTTP/SSE server is the
TypeScript harness wrapper around the Rust native engine. If these wrapper
intervals drift back to aggressive settings, SSE and `/state/*` latency can
regress even while the Rust executor stays fast.

The relayer should also default to the embedded Rust local-state path. With
`CTM_RELAYER_HARNESS_REJECT_MARKET_DRIFT=false`, submit-path margin checks stay
in Rust, do not fall back to `GET /state/users/:owner`, and the relayer should
not poll `:9091 /healthz` on every ingress.

## Common Commands

Rust:

```bash
cargo fmt --all
cargo clippy --workspace --features enable-gpl -- --no-deps
cargo +solana test-sbf --features enable-gpl
```

TypeScript:

```bash
yarn format
yarn lint
yarn test
yarn typecheck
```

Rust harness native module:

```bash
node scripts/build-rust-harness-native.js --build --profile release
```

Execution-queue scripts exposed through `package.json` include:

- `execution-queue-local-perp-e2e-bootstrap`
- `execution-queue-local-perp-e2e-run`
- `execution-queue-local-perp-pnl-test`
- `execution-queue-cranker`
- `ctm-sequencer-relayer`
- `ctm-relayer-http-bridge`
- `continuum-state-harness`
- `continuum-state-harness-verify`

## Documentation Map

- [docs/Overview.md](docs/Overview.md): high-level system overview
- [docs/Onchain_Programs.md](docs/Onchain_Programs.md): on-chain program design and execution queue
- [docs/Offchain_Components.md](docs/Offchain_Components.md): relayer, harness, and HTTP bridge
- [ts/client/scripts/execution-queue/README.md](ts/client/scripts/execution-queue/README.md): maintained local/devnet execution-queue workflow
- [usage_guide.md](usage_guide.md): additional operational notes and examples
- [DEVELOPING.md](DEVELOPING.md): development notes
- [FAQ-DEV.md](FAQ-DEV.md): current build and debugging issues
- [RELEASING.md](RELEASING.md): program release and governance flow
- [setup.md](setup.md): off-chain devnet deployment guide
- [formal_verification/SPEC.md](formal_verification/SPEC.md): formal specification entry point

## Licensing

This repository is not single-license.

- The root [LICENSE](LICENSE) applies MIT to most files.
- Files under `programs/mango-v4/src/instructions/` are GPLv3 as described in the root license.
- `third-party/` content carries the licenses shipped in those subdirectories.
- Some individual packages declare their own licenses; for example, `bin/service-mango-execution-engine` is `AGPL-3.0-or-later`.

If you are redistributing code or binaries from this workspace, check the root license and the relevant crate or package manifest rather than assuming the entire repository is MIT-only.
