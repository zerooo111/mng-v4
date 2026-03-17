# Production Readiness Test Plan

## Purpose
This plan defines the test surface required before treating the Mango v4 + execution-queue pipeline as production-ready. It is intentionally broader than a single "all tests passed" gate: the system includes onchain state transitions, relayer/executor correctness, replayable state projection, localnet performance, and devnet operational behavior.

## Current Baseline
- Strong existing coverage already exists for the legacy Mango protocol paths under [`programs/mango-v4/tests`](/home/ec2-user/stagin4/mng-v4/programs/mango-v4/tests).
- Newer execution-queue, Rust relayer/executor, and TS continuum-harness paths are materially less covered and must be treated as the highest-risk surface.
- Devnet smoke and PnL tests exist, but those are not sufficient for a production-readiness claim on their own.

## Certification Standard
Passing tests is necessary but not sufficient. A credible production-readiness claim requires:
- clean automated results for all required suites
- stable localnet and devnet end-to-end runs
- no known correctness-critical open bugs
- explicit sign-off on residual operational risks
- external review or audit for critical onchain changes

## Test Matrix

### 1. Pure onchain unit tests
Focus: deterministic state-machine behavior without RPC or validator noise.

Required coverage:
- execution queue state transitions
- queue header invariants
- gap handling and head advancement
- queue slot reuse and duplicate-sequence rejection
- liquidity queue rotation and retry backoff
- payload hashing and account-hash stability helpers
- health-region selection rules for queued instructions
- margin, health, funding, and settle math edge cases

Required cases:
- enqueue pending CTM item into empty queue
- reject duplicate pending sequence in the same slot
- allow dense sequence reuse after head clears or failure recovery
- clear out-of-order CTM item and advance head across front gaps correctly
- find matching CTM lane within scan limits and reject out-of-window matches
- rotate liquidity head with retry and preserve FIFO order
- reject malformed queue payload variant
- reject invalid dispatch account layouts
- verify queue header totals always satisfy `total_count == ctm_count + liquidity_count`

Primary files:
- [programs/mango-v4/src/state/execution_queue.rs](/home/ec2-user/stagin4/mng-v4/programs/mango-v4/src/state/execution_queue.rs)
- [programs/mango-v4/src/instructions/execution_queue.rs](/home/ec2-user/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs)
- [programs/mango-v4/src/health](/home/ec2-user/stagin4/mng-v4/programs/mango-v4/src/health)

### 2. Onchain program-test integration
Focus: full instruction execution in `solana-program-test`.

Required coverage:
- group/token/perp bootstrap
- margin-backed place/cancel via direct shared-core dispatch
- consume events, update funding, settle pnl, settle fees
- liquidation, bankruptcy, force cancel, reduce-only restrictions
- instruction-gate and admin authority enforcement
- replay resistance and stale oracle rejection
- execution queue enqueue and execute flows

Required new program-test scenarios:
- enqueue place/cancel through queue and execute them successfully
- queue gap at head, then bounded skip progression after wait slots
- lane mismatch should not mutate queue state
- failed dispatch increments retry metadata and preserves item until terminal handling
- queued place order must respect health-region requirements and fail when undercollateralized
- queue pause flags must block ingress/execute independently

Primary files:
- [programs/mango-v4/tests/cases](/home/ec2-user/stagin4/mng-v4/programs/mango-v4/tests/cases)
- [programs/mango-v4/tests/program_test](/home/ec2-user/stagin4/mng-v4/programs/mango-v4/tests/program_test)

### 3. Rust relayer/executor unit tests
Focus: correctness of queue inspection, sequence allocation, retry policy, dedupe, and metrics-facing logic.

Required coverage:
- queue account layout decoding
- head detection reasons
- sequence cursor dense reuse
- submitted-depth accounting
- duplicate execute suppression
- signature-status accounting
- queue backpressure thresholds

Required cases:
- decode correct queue header offsets
- detect CTM pending head vs liquidity fallback
- detect sequence mismatch, empty-slot gap, and invalid status reasons
- reserve recyclable sequence before allocating a new one
- count only submitted pending sequences for backpressure
- keep submitted head pending until queue floor proves absence
- avoid double-counting confirmed-no-advance for the same signature

Primary files:
- [bin/service-mango-execution-engine/src/main.rs](/home/ec2-user/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs)

### 4. TS client and harness unit tests
Focus: client-side decoding, replay determinism, state projection, and observability correctness.

Required coverage:
- execution queue layout decode
- queue head decode for CTM and liquidity items
- header invariant reporting
- continuum harness replay determinism
- duplicate processed event suppression
- divergence reporting when status changes or events arrive without relay intent
- trade and candle projection from confirmed executions

Required cases:
- decode queue header offsets and invariant flag correctly
- decode CTM head item payload and hash
- fall back to liquidity head when CTM head is unavailable
- return null on malformed head rather than a false positive
- dedupe identical processed events by signature and key
- emit divergence when a processed status changes for the same item
- keep skipped and failed queue counters accurate

Primary files:
- [ts/client/src/executionQueueLayout.ts](/home/ec2-user/stagin4/mng-v4/ts/client/src/executionQueueLayout.ts)
- [ts/client/src/continuumHarness.ts](/home/ec2-user/stagin4/mng-v4/ts/client/src/continuumHarness.ts)

### 5. Localnet end-to-end correctness
Focus: the whole pipeline under deterministic local validator conditions.

Required scenarios:
- bootstrap from empty state
- restart from persisted state
- relayer submit + embedded rust cranker execute
- queue drains to zero after run
- maker/taker place, match, cancel, settle pnl
- harness confirmed view matches onchain state

Required negative scenarios:
- duplicate sequence reconciliation
- queue pause flags
- malformed relay payload
- insufficient margin on queued place
- stale oracle or invalid market config

Primary scripts:
- [ts/client/scripts/execution-queue/local-perp-e2e-bootstrap.ts](/home/ec2-user/stagin4/mng-v4/ts/client/scripts/execution-queue/local-perp-e2e-bootstrap.ts)
- [ts/client/scripts/execution-queue/local-perp-e2e-run.ts](/home/ec2-user/stagin4/mng-v4/ts/client/scripts/execution-queue/local-perp-e2e-run.ts)
- [ts/client/scripts/execution-queue/local-perp-pnl-test.ts](/home/ec2-user/stagin4/mng-v4/ts/client/scripts/execution-queue/local-perp-pnl-test.ts)

### 6. Devnet pre-production tests
Focus: real RPC behavior, deploy-time config, persisted keypairs, and external rate limits.

Required scenarios:
- reuse existing group and queue state safely
- no hot-generated keypairs
- no auto-funding behavior
- clean place/match/cancel/settle flow against deployed program
- harness and relayer using current program id and current state model
- conservative TPS run under provider rate limits

Required assertions:
- post-run balances stay within expected fee budget
- queue either drains or stops safely without fee-spam loops
- relayer/harness configs persist across restart
- PnL and settlement remain correct on deployed code

### 7. Adversarial and robustness tests
Focus: malformed input, permission abuse, replay, and fault tolerance.

Required coverage:
- bad signatures
- wrong remaining-account layouts
- stale or missing accounts in queue payload
- conflicting processed statuses
- duplicate event ingestion
- replayed relay intents
- corrupted persisted sequence state
- partial restart with missing runtime files

### 8. Performance and regression tests
Focus: throughput and cost stability.

Required benchmarks:
- single-lane hot path
- few-lane mixed traffic
- many-lane mixed traffic
- queue drain after burst
- relayer submit-stage latency
- execute items-per-transaction

Primary acceptance targets:
- localnet single-lane `100+ avg place TPS`
- explicit mixed-lane verdict: meets target or demonstrates architecture-bound limit
- no unbounded queue growth under target profile
- no sustained duplicate execute spam

### 9. Operational tests
Focus: startup, restart, persistence, and observability.

Required coverage:
- startup scripts on localnet and devnet
- persisted keypairs and config reuse
- metrics and health endpoints
- safe restart behavior without deleting devnet runtime state
- explicit failure when required funding or config is absent

## Execution Order
1. Pure unit tests for queue state, relayer, and TS decoders.
2. Program-test integration for queue instruction flows.
3. Localnet end-to-end correctness.
4. Localnet TPS/perf regression.
5. Devnet conservative end-to-end and PnL tests.
6. Pre-release restart and operational drills.

## Automation Tiers

### Required on every PR
- Rust unit tests for `mango-v4`
- Rust unit tests for `service-mango-execution-engine`
- TS unit/spec tests for client and harness

### Required on merge to main
- targeted `solana-program-test` cases including execution queue
- localnet E2E bootstrap + run + PnL

### Required before release
- repeated localnet TPS benchmark
- devnet end-to-end and PnL confirmation
- restart/persistence drill
- manual review of metrics and logs for queue stalls, retries, and divergence events

## Immediate Gaps To Close
- add direct tests for execution queue state transitions
- add direct tests for TS queue layout decoding
- add direct tests for relayer queue-head and sequence-cursor edge cases
- add program-test queue execute scenarios rather than relying only on scripts

## Current Outcome Standard
After the tests added in this pass, the codebase will be better protected against the recent queue/regression bugs. It will still not be honest to certify the codebase as production-ready until the full matrix above is automated or intentionally signed off as out of scope.
