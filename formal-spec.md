# Mainnet Readiness Formal Specification

## Scope

This document defines:

- the current safety and readiness assessment of the repository
- the minimum evidence required to certify the system as mainnet ready
- the end-to-end testing strategy required for sign-off

Scope includes:

- onchain programs in `programs/`
- client libraries in `lib/` and `ts/client/`
- offchain operators and services in `bin/`

Primary certification concerns:

- flawed accounting
- loss or misappropriation of funds
- unauthorized state transitions
- unrecoverable or unexpected protocol stall
- liveness failures caused by offchain dependencies

## Current Assessment

Current status: **not mainnet ready for certification**.

Reason:

- the onchain test surface is broad, but several mainnet-critical paths are still skipped, ignored, placeholder-only, or operationally dependent on offchain heuristics
- at least one likely onchain defect exists in multi-bank fallback-oracle inheritance
- the current TypeScript test gate is not trustworthy because it is narrowed by an accidental `describe.only`
- the documented standalone execution-queue unit binary is not reliably runnable in the current environment

## Evidence Summary

Observed test inventory:

- `176` async program tests in `programs/mango-v4/tests/cases`
- `134` Rust unit tests in `programs/mango-v4/src`
- `57` TypeScript spec cases in `ts/client/src`
- `42` skipped or ignored tests across critical Rust/TS surfaces

Interpretation:

- onchain protocol mechanics have meaningful coverage
- queue logic has especially deep coverage
- offchain and real-fixture safety gates are incomplete
- current green test results would not justify mainnet certification

## Formal Findings

### F1. Multi-bank fallback-oracle inheritance bug candidate

Severity: **High**

Evidence:

- `programs/mango-v4/src/state/bank.rs:390`
- `programs/mango-v4/src/instructions/token_add_bank.rs:59`

Observation:

- `Bank::from_existing_bank()` copies `fallback_oracle` from `existing_bank.oracle`
- `token_add_bank()` uses this constructor when creating additional banks for a token

Required property:

- all cloned banks for the same token must preserve the configured fallback oracle

Failure mode:

- a secondary bank may lose failover oracle protection and become less available or less safe under primary-oracle degradation

Certification requirement:

- fix constructor behavior
- add a dedicated regression test proving fallback-oracle preservation across `token_add_bank`
- include a failing-primary / working-fallback scenario on the added bank

### F2. TypeScript test gate integrity failure

Severity: **High**

Evidence:

- `ts/client/src/accounts/oracle.spec.ts:26`
- `ts/client/src/accounts/oracle.spec.ts:27`

Observation:

- `oracle.spec.ts` is marked `describe.only`
- the same spec uses live mainnet RPC directly

Observed runtime effect:

- a targeted Mocha run for queue/harness specs was hijacked by this suite
- the run failed on live RPC account fetches instead of exercising the requested queue specs

Required property:

- unit and integration tests must be selectable, isolated, and deterministic

Certification requirement:

- remove `describe.only`
- separate live-network tests from unit/default CI tests
- ensure queue/client specs can be run directly without accidental suite narrowing

### F3. Mainnet-critical tests remain skipped or ignored

Severity: **High**

Evidence:

- `programs/mango-v4/tests/cases/test_flash_loan_security.rs:6`
- `programs/mango-v4/tests/cases/test_execution_queue_security.rs:1328`
- `programs/mango-v4/tests/cases/test_execution_queue_security.rs:1499`
- `ts/client/src/e2e/lifecycle.spec.ts:21`
- `ts/client/src/e2e/perf.spec.ts:23`
- `programs/mango-v4/tests/cases/test_perp_settle.rs:205`
- `programs/mango-v4/tests/cases/test_perp_settle_fees.rs:205`
- `programs/mango-v4/tests/cases/test_perp_settle_fees.rs:254`

Missing runnable coverage includes:

- flash-loan duplicate-bank, begin/end mismatch, underpayment security cases
- execution-queue health behavior under real stale/confidence oracle failure
- funding settlement behavior for perp PnL and perp fee settlement
- fee-settlement health gating under realistic conditions
- full relayer / crank / queue / executor end-to-end lifecycle
- compute-budget and throughput certification tests

Required property:

- all custody-critical and liveness-critical flows must be covered by runnable tests

Certification requirement:

- convert all critical ignored/skipped coverage into runnable CI or release-gate suites

### F4. Known rounding shortfall on withdraw-all path

Severity: **Medium**

Evidence:

- `programs/mango-v4/src/instructions/token_withdraw.rs:51`
- `programs/mango-v4/tests/cases/test_position_lifetime.rs:238`

Observation:

- `u64::MAX` withdraw-all path explicitly rounds down
- existing test accepts a `-1` workaround

Required property:

- no silent user-visible token loss beyond explicitly specified and bounded rounding rules

Certification requirement:

- either eliminate the shortfall or formally bound and document it
- add invariant tests that quantify the exact rounding envelope

### F5. Execution-queue liveness still depends on offchain recovery heuristics

Severity: **Medium**

Evidence:

- `programs/mango-v4/src/instructions/execution_queue.rs:1554`
- `bin/service-mango-execution-engine/src/main.rs:128`
- `bin/service-mango-execution-engine/src/main.rs:2660`

Observation:

- health-gated queue failures rely on rollback semantics plus offchain skip-list/admin-drop behavior
- executor contains explicit proactive admin-drop heuristics for repeated failures and no-lane-match cases

Required property:

- queue progress must remain bounded and recoverable under invalid, stale, or permanently unexecutable heads

Certification requirement:

- prove liveness under relayer failure, executor failure, stale heads, no-lane-match heads, expired heads, and repeated simulation failures
- operational drills must show bounded recovery without manual state surgery

### F6. Liquidator still has acknowledged unresolved operational branches

Severity: **Medium**

Evidence:

- `bin/liquidator/src/liquidate.rs:651`
- `bin/liquidator/src/liquidate.rs:663`
- `bin/liquidator/src/liquidate.rs:671`
- `bin/liquidator/src/liquidate.rs:718`

Observation:

- liquidator explicitly acknowledges unhandled or deferred cases, including positive PnL handling and residual liquidatable states

Required property:

- every liquidatable state reachable onchain must have either:
  - a deterministic liquidation path, or
  - a well-defined bounded-failure operator procedure that preserves funds and liveness

Certification requirement:

- enumerate every reachable liquidation phase outcome
- prove an execution path exists for each
- failure-inject operator outages and verify that backlog does not create unrecoverable states

## Certification Gates

The system may be certified mainnet ready only if all gates below pass.

### Gate 0. Test Harness Integrity

Requirements:

- no `describe.only` / `it.only` / accidental suite narrowing
- no skipped or ignored critical tests
- deterministic separation between unit, localnet integration, devnet shadow, and live-network probes
- documented and reproducible test commands
- queue unit tests must run reliably on the prescribed toolchain

Exit criteria:

- CI and release scripts run the intended suites without hidden exclusions

### Gate 1. Onchain Accounting Invariants

Required invariants:

- token conservation across vaults, accounts, insurance, fee buckets, and dust buckets
- monotonic correctness of deposit and borrow indices
- no unauthorized fee, insurance, or socialized-loss transfer
- failed instructions leave state unchanged
- queue counters and sequence invariants remain valid after success, failure, skip, retry, and wraparound

Test method:

- property-based and randomized state-machine testing
- post-step invariant checks after every instruction sequence

Exit criteria:

- zero invariant violations over large randomized campaigns

### Gate 2. Adversarial Onchain Matrix

Each of the following must have runnable positive and negative tests:

- deposits, withdrawals, borrows, repay, withdraw-all
- deposit/borrow limits, reduce-only, force-close, force-withdraw
- token liquidation, perp liquidation, bankruptcy, insurance depletion, socialization
- Serum3 and OpenBook reserve tracking, settlement, and force-cancel flows
- flash-loan ordering, duplicate bank, under-repayment, wrong account, and nested misuse
- TCS create/start/trigger/cancel/expiry/incentive paths
- execution queue CTM, direct submit, replay, duplicate sequence, wrong signer, pause, gap skip, lane mismatch, retry, admin drop
- stale oracle, bad confidence, missing fallback, wrong fallback, fallback success

Exit criteria:

- all cases runnable and passing

### Gate 3. Real Oracle and Real Fixture Integration

Stub-only coverage is insufficient where semantics differ from production.

Must use:

- real Pyth-like staleness and confidence behavior
- real fallback-oracle quote dependency behavior
- real DEX fixture coverage for flash-loan and settle paths where required

Exit criteria:

- production-relevant oracle and CPI behavior is exercised directly

### Gate 4. Full Localnet End-to-End

Required components:

- validator
- onchain program
- TS/Rust client flows
- relayer
- execution engine/cranker
- keeper
- liquidator
- settler
- health/orderbook/fills supporting services

Required flows:

- deposit -> borrow -> trade -> settle
- queue enqueue -> execute -> cancel -> retry -> drain
- liquidation and bankruptcy
- signer rotation
- pause ingress / pause execute / resume
- stale item expiry and recovery
- operator restart during active flow

Exit criteria:

- end-to-end state converges correctly with no manual intervention

### Gate 5. Failure Injection

Inject and validate recovery for:

- RPC timeouts
- websocket lag or disconnect
- relayer crash/restart
- executor crash/restart
- liquidator outage
- stale oracle
- confidence failure
- no-lane-match queue heads
- expired queue heads
- duplicate intent submission
- partial downstream CPI failure
- insurance exhaustion

Exit criteria:

- no unrecoverable queue stall
- no unexplained accounting drift
- bounded recovery time with documented operator runbooks

### Gate 6. Soak and Reconciliation

Run long-duration randomized workloads:

- 24-hour soak minimum
- 72-hour preferred pre-certification run

Reconcile periodically:

- vault balances
- bank totals
- user balances and liabilities
- perp base and quote exposure
- fee buckets
- insurance funds
- queue occupancy, head, max seen, and sequence floor

Exit criteria:

- zero unexplained drift
- only explicitly documented rounding behavior remains

### Gate 7. Shadow Release

Run in a prod-like environment with:

- real oracles
- realistic service deployment
- operator drills

Required drills:

- CTM signer rotation
- ingress pause / execute pause
- relayer restart preserving sequencing
- executor backlog and admin-drop recovery
- fallback-oracle activation
- liquidation backlog under load

Exit criteria:

- all drills complete without fund risk or persistent stall

## Mandatory New Test Work

The following new tests are mandatory before certification:

1. `token_add_bank` preserves fallback oracle and behaves correctly under primary-oracle failure.
2. Flash-loan security suite runs with real required fixtures and covers duplicate bank, missing end, under-repayment, wrong trailing accounts, and wrong token-account mint.
3. Perp funding-settlement tests cover:
   - positive and negative funding
   - fee settlement after funding accrual
   - health gating under small and large fee settlement
4. Execution-queue direct-submit tests cover:
   - forced delay
   - sequence assignment and collision avoidance
   - replay resistance
   - coexistence with relayer-signed CTM flow
5. Execution-queue real-oracle tests cover stale/confidence failure with production-like fixtures.
6. Offchain crash/restart tests cover relayer, executor, keeper, liquidator, and settler continuity.
7. Reconciliation tests independently recompute balances from logs and account state.
8. Throughput and compute-budget tests become real release gates, not stubs.

## Required Sign-Off Criteria

Certification is allowed only if all statements below are true:

- all High and Medium findings in this document are resolved or explicitly accepted with documented bounded risk
- no critical tests are skipped or ignored
- all certification gates pass on two consecutive release candidates
- soak and shadow runs show zero unexplained accounting drift
- queue liveness is demonstrated under component failure and restart
- liquidation and bankruptcy paths are shown to terminate safely
- operator runbooks exist for pause, signer rotation, queue recovery, oracle failover, and liquidation backlog

If any statement above is false, certification must be denied.

## Notes on Current Verification State

Additional observed validation results during review:

- targeted TS execution against queue-related specs was intercepted by `ts/client/src/accounts/oracle.spec.ts` because of `describe.only`
- targeted TS execution then failed on live mainnet RPC fetches rather than exercising the intended queue/harness specs
- `cargo test -p mango-v4 --test test_execution_queue_unit --features enable-gpl` compiled successfully but the test binary exited with `SIGSEGV`
- retrying the same queue binary under `cargo +1.70.0` also exited with `SIGSEGV`

Interpretation:

- current test commands are not yet reliable certification evidence by themselves

## Final Determination

Until the findings are fixed and all certification gates pass, the program must be treated as **not certified for mainnet readiness**.
