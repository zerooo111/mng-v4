# V5 Roadmap Implementation Specs

Date: 2026-04-21

This document turns the `Priority Plan` in `v5-roadmap.md` into brief implementation specs. It is intentionally opinionated. The codebase already shows the main failure mode: too many queue-era concepts leaking into relayer config, TS builders, harness probing, and runtime telemetry. The right fix is not another compatibility wrapper. The right fix is to collapse the supported surface to `v2` and `v5`, make market-scoped queue state the default everywhere, and then build the async direct path as a clean second ingress into a single ordered executor model.

## Design Rules

- Prefer deletion over compatibility. If a path is not part of the forward architecture, freeze it immediately and remove it from docs, env vars, public exports, and new tests.
- Keep queue family explicit. Use a real enum and adapter boundary, not boolean feature combinations.
- Keep market as the unit of scaling. Queue accounts, telemetry, rollout flags, and executor planning should all be market-scoped.
- Do not overload commit/reveal types to carry direct-ingress semantics. Direct ingress should have its own message format and staging state.
- Treat tranche-1 async semantics as best-effort after acceptance. Reservation semantics are a later, explicit product decision.
- Keep fee accounting relayer-local. The fee ledger is an operator service concern, not an on-chain protocol concern.

## Tranche A — Queue-Family Consolidation

### 1. Lock the queue-family target in code and docs

Define a single cross-language queue-family vocabulary and make everything else internal-only. In Rust, that should be an enum used by relayer startup, harness readers, replay, and telemetry; in TS, it should replace stringly queue-version branching in builders and scripts. `v3` and `v4` may remain parseable behind `legacy` modules during migration, but they should stop appearing in user-facing env vars, docs, defaults, and exported helpers. If a new code path needs to ask "is this v3?", the architecture is already regressing.

### 2. Define the new per-market v2 account family and instruction surface

The new `v2` should stop pretending a shared account with sub-queues is operationally acceptable. Define a per-market PDA family that mirrors the successful parts of `v5`: fixed header, market-local ring metadata, and explicit lifecycle instructions for create, resize, init, configure, execute, and admin drop. Preserve `v2`'s payload semantics and `execute_multi`, but move the storage shape to market-scoped accounts so Solana account locking stops coupling unrelated markets. Use deterministic PDA derivation from `(group, market_index, family=v2)` and keep the instruction surface parallel to `v5` where possible.

### 3. Remove paginated v3/v4 from the forward plan and make them compatibility-only until deletion lands

This is mostly governance and code hygiene, but it matters. Mark v3/v4 as frozen in docs, move legacy helpers under clearly named modules, and stop extending their tests except for migration safety or deletion coverage. The practical implementation pattern is "read-only compatibility": allow decode, replay ingestion, and operator inspection long enough to exit them cleanly, but reject any new product requests that require touching page roots, page refresh loops, or v3-specific runtime config.

### 4. Replace the relayer's split config model with a single explicit queue mode

The current `EXECUTION_QUEUE_TOPOLOGY` plus `V5_ROUTE_ALL` split in `bin/service-mango-execution-engine/src/main.rs` is a design bug because it lets startup, submit routing, and executor behavior diverge. Replace it with one authoritative `QUEUE_MODE=v2|v5` setting, parsed once at startup into a typed runtime config. Mode-specific required fields should be validated in one place, and downstream code should receive a fully constructed mode object instead of re-checking booleans. This is the kind of cleanup that pays off everywhere: fewer invalid states, simpler telemetry, and cleaner rollout logic.

### 5. Introduce a queue-family adapter boundary in the relayer

The relayer needs an explicit interface for `submit`, `inspect_head`, `build_execute`, `drop`, `telemetry_snapshot`, and optimistic-state hooks. Do not make this a giant abstract framework; keep it as a small trait or enum-dispatch object with `V2Adapter` and `V5Adapter` implementations. The point is to stop sprinkling queue-family conditionals across submit handlers, executor workers, harness bridges, and replay code. Once this boundary exists, it becomes much easier to modernize `v2` and harden `v5` without re-breaking the rest of the service.

## Tranche B — v2 Modernization

### 6. Implement v2 per-market queue PDAs with ring-buffer semantics parallel to v5

This is the real on-chain `v2` rewrite. The storage model should be a per-market queue account with ring-buffer slots, sequence tracking, per-market limits, and enough metadata to preserve `v2` payload execution rules without shared-account parsing. Reuse the patterns that already work in `execution_queue_v5.rs`: explicit create/resize/init lifecycle, market-local head/tail invariants, and clear admin pause bits. The main constraint is compatibility of payload execution semantics, not compatibility of storage layout.

### 7. Port v2 admin, drop, and configure flows to the new per-market layout

Once per-market `v2` exists, admin paths must stop assuming shared-account sub-queues. Add market-local configure and drop instructions, plus any global pause that still makes sense at the family level. Keep the admin surface narrow: operator pause, market config update, bounded head drop, and account close once empty. Resist rebuilding the old page-root machinery in disguise; the whole point of the new layout is that admin flows become small, local, and easy to reason about.

### 8. Port relayer submit and executor logic from shared-account v2 parsing to per-market v2 queue accounts

The relayer should derive or look up the per-market `v2` queue PDA and treat it as the sole queue account for that market. Submit, queue inspection, and execute planning should consume the same adapter surface used by `v5`, which means shared-account sub-queue parsing should disappear from hot paths. This is a good place to centralize account recipe resolution too: the relayer should ask one component for "the execution account set for market X in family v2" rather than rebuilding account lists ad hoc.

### 9. Keep efficient v2 lane batching by reusing execute_multi on per-market v2 queues

`execute_multi` is worth preserving, but only as a batching primitive, not as an excuse to keep the old storage topology. Retarget it so each lane references a per-market queue account and its dispatch-account hash, then keep lane selection and packing off-chain where the relayer already has queue visibility. The design pattern should be "multiple market-local queues, one multi-lane executor tx", not "one shared queue pretending to be multiple markets". That keeps batching efficiency without keeping operational coupling.

### 10. Add migration and bootstrap tooling for new v2 per-market accounts

This is an operator workflow problem as much as a code problem. Build a deterministic toolchain that can create, resize, init, and configure the full v2 market set, verify PDA derivation, and emit a machine-readable manifest of live accounts. It should be idempotent, dry-runnable, and able to compare intended config with on-chain state before making changes. Migration tooling should not attempt clever in-place translation of old queue contents; the safer posture is bootstrap new accounts, gate traffic, drain old state, and then switch.

## Tranche C — AMQ Framing and Direct-Path Architecture

### 11. Lock the AMQ instruction taxonomy: sync deposit and withdraw, async market execution

This must become product policy and code policy at the same time. Funding instructions already exist synchronously; the missing piece is to formalize that deposits and withdraws do not belong in the async market queue model, while market execution does. Encode that classification centrally in on-chain constants where needed, relayer route selection, TS builders, and docs. If the codebase keeps treating "async" as a transport detail rather than a semantic lane, it will keep drifting.

### 12. Finalize the direct-pool and unified-execute design for the async lane

The clean design is a two-stage async lane: a direct pool for authenticated user ingress, and a single per-market ordered execution stream that contains both promoted direct items and relayer commit/reveal items. The direct pool should be a separate market-scoped staging account or account family, not a mutation of `CommitItemV5`. Promotion should assign a real sequence and feed a single executor order model. Keep tranche-1 semantics best-effort: accepted direct items may still fail later due to changing health or market state, and that is fine until reservation semantics are explicitly chosen.

### 13. Add the new direct-intent signature format and direct enqueue instruction

Do not reuse the old `enqueue_direct` trick of pretending sequence zero and leaning on earlier intent hashes. Define a new canonical signed message that binds `group`, `mango_account`, `user_owner`, target market, payload hash, kind, expiry, and a user-controlled nonce or `client_order_id` for privacy. The new enqueue instruction should only validate the direct intent and stage it into the direct pool; it should not allocate final queue sequence itself. This separates "user-authenticated ingress" from "ordered execution admission", which is exactly the boundary the current implementation lacks.

### 14. Add the new unified v5 execute instruction

`execution_queue_v5_reveal_execute_market` already proves the value of a single market-scoped executor loop, but it is still built around commit/reveal materialization. Replace or supersede it with a unified execute instruction that can consume promoted direct items and committed reveal items through one ordered head path, one failure taxonomy, and one event surface. The executor should not care how an item entered the ordered stream after promotion. The on-chain contract here is simple: once an async action has a per-market sequence, it must obey the same head, expiry, and cleanup rules as everything else.

### 15. Upgrade the executor to read the direct pool and build unified execute transactions

The relayer-side executor needs a planner that understands two sources of work: the ordered queue head and the direct pool backlog eligible for promotion. The pattern should be "observe both, decide promotion, then build one execute transaction shape", not separate mini-executors. Keep promotion logic centralized in the adapter or planner layer so instrumentation, retries, and head-lock logic remain coherent. This also makes it easier to introduce admission heuristics later, such as prioritizing same-user packing or lane-aware batching.

### 16. Add per-market feature gating so direct ingress is disabled until the new executor is live

This is where rollout discipline matters. Add an on-chain per-market flag for direct-ingress enablement, but also keep a relayer-side market capability map so startup refuses to advertise unsupported markets. The default should be disabled, and the relayer should fail closed: if the executor cannot prove that market `M` supports direct promotion and unified execution, direct ingress for `M` stays off. Feature gates should be visible in telemetry and exposed to SDK/frontend so users do not probe dead features.

### 17. Make SDK, client, and relayer routing default to sync for funding and async for market execution

This should become the only default path presented to integrators. Centralize route selection in a shared policy layer so TS builders, relayer submit endpoints, and frontend UX all answer the same question the same way. Funding builders should be explicit sync actions; async builders should focus on market intents and accept relayer-local controls like `max_fee_lamports`. Avoid hidden auto-magic in the frontend: the SDK should expose the chosen lane and the frontend should surface it.

## Tranche D — Relayer and Harness Parity

### 18. Make relayer startup select exactly one queue family cleanly

Relayer startup should instantiate one queue adapter, one account recipe source, one executor planner, and one telemetry family. That means no mode-specific code should survive outside initialization and the adapter implementation. The current mixed state in `main.rs`, where queue topology and v5 routing are partly independent, is exactly what causes operational ambiguity. A clean boot model also makes it much easier to test and roll back.

### 19. Ensure both queue families use the same optimistic-state integration surface

Optimistic state should not know or care whether an accepted async action came from `v2` or `v5`. Define a family-neutral optimistic projection API that takes normalized accepted-item and processed-item events, plus enough metadata to invalidate or replay them deterministically. This is more important than it sounds: if queue-family differences leak into optimistic-state updates, frontend consistency and replay correctness will drift immediately. Normalize first, specialize only where execution details truly differ.

### 20. Add harness live queue readers for both per-market v2 and per-market v5

The harness currently still special-cases `onchain_v5` reads, which is a temporary expedient, not a stable model. Replace that with explicit live readers for `v2` and `v5`, both market-scoped and both returning the same normalized queue snapshot shape. The harness should depend on the same PDA derivation and decoding logic used elsewhere, ideally via a shared TS or Rust utility rather than duplicated account-layout knowledge. Once that exists, queue health tooling stops caring which family it is looking at.

### 21. Unify queue telemetry and watermarks so both families publish the same market-scoped health surface

Define one telemetry schema for queue health: head sequence, max seen sequence, live count, backlog, age, gap state, execution pause state, ingress pause state, and any direct-pool metrics where relevant. The relayer, harness, and dashboards should all publish or consume that normalized model. This is not a place for family-specific metric names beyond low-level debugging. Operators need comparable market-level health, not a taxonomy of implementation details.

### 22. Remove v3-specific queue root and page probing from relayer and harness

Once `v2` and `v5` are the only target families, v3 root/page probes become technical debt with real blast radius. Remove them from runtime workers, HTTP endpoints, scripts, and health checks, then keep any necessary read-only forensic tools behind an operator-only legacy namespace if needed. The practical goal is that no production code path should wake up and ask for a queue root page ever again. That entire shape should become impossible to depend on by accident.

### 23. Fill TS and script gaps so v2 and v5 are the only exposed queue families

The TS client currently still exports legacy enqueue and execute builders, which keeps old semantics alive for integrators even after docs move on. Prune those exports or move them into a clearly named legacy file, then add parity helpers for modernized `v2` and the new `v5` direct/unified path. Scripts should also stop inferring queue family from ad hoc env vars. The design pattern here is public-surface hygiene: if an API is still exported, someone will build against it.

## Tranche E — Correctness and Observability

### 24. Add v2-per-market and v5 tests for submit, replay, ordering, expiry, and batch execution

The test strategy should move from instruction-level fragments to behavior slices. For each family, add coverage for enqueue, ordering, expiry, gap handling, replay reconstruction, and batch execution under mixed conditions. Use golden event streams where possible so the same scenario can assert on on-chain events, replay output, and relayer optimistic state. This will catch architectural drift faster than unit tests that only validate byte layouts.

### 25. Extend replay and optimistic-state code so direct-path accepted items and promoted sequences are reflected correctly

Replay needs to understand that direct-path acceptance and final ordered sequence assignment are separate lifecycle events. Add explicit internal event types for direct accepted, promoted, expired before promotion, processed, and dropped, then teach optimistic-state projection how to collapse them into user-visible state. The important pattern is append-only lifecycle modeling, not inferred reconstruction from side effects. If replay has to guess whether an item was promoted, the event model is still under-specified.

### 26. Add trace, event, and metrics coverage for direct ingress and promotion

Direct ingress will be operationally invisible unless it emits first-class traces and counters. Add structured events for direct accepted, promotion attempted, promotion succeeded, promotion skipped, promotion expired, and unified execution result. Emit relayer-side spans that carry request ID, market index, mango account, queue family, and eventual sequence when assigned. This is the minimum needed to debug "accepted but never executed" complaints without reading raw account state by hand.

### 27. Add lane-aware observability so sync funding and async execution are reported separately

The system needs a lane model that operators and users can actually see. Funding and market-execution traffic should have separate counters, latencies, error rates, and queueing metrics, even if they share some surrounding infrastructure. This is also where fee observability belongs later, because billing async execution and billing sync funding are likely to diverge. If one metric aggregates both lanes, it will lie.

### 28. Add admin tooling for direct-pool inspection and cleanup

Operators will need more than a raw account dump. Build tooling that can list direct-pool contents by market, age, owner, and status; identify expired or malformed items; and perform bounded cleanup actions with clear audit output. Keep destructive actions explicit and market-local, and require that tools emit before/after state so operator interventions can be replayed or reviewed later. Direct pools are staging areas; they must be operable as such.

## Tranche F — Remaining v5 Hardening

### 29. Finish the stateful ingress precheck work

`v5_precheck` should graduate from static validation to a real ingress policy engine that understands current market state, account health prerequisites, and known unrevealable payload shapes. Keep it stateful but cheap: precheck should reject obviously doomed traffic before it enters the expensive async path, without trying to become a full execution simulator. The best pattern is layered validation: structural checks first, then market/account readiness checks, then execution-path-specific policy checks. This is one of the few places where rejecting more aggressively is a net simplification.

### 30. Add a real v5 benchmark floor, including direct-path promotion and mixed traffic

Benchmarking must stop measuring only the happy-path commit/reveal pipeline. Create repeatable workloads that include relayer commits, direct ingress, promotions, mixed markets, and varied user distributions, then publish a floor the architecture must not regress below. Benchmarks should record both throughput and fairness metrics, because same-user batching or promotion heuristics can easily improve one while harming the other. Treat the benchmark suite as release gating, not as an occasional experiment.

### 31. Expand stall telemetry with direct-pool age, eligible backlog, and promoted-but-not-yet-executed backlog

Current stall telemetry is too queue-head-centric for a dual-ingress async model. Add direct-pool age percentiles, count of items eligible for promotion, count of promoted items waiting behind head constraints, and time-since-last-successful-promotion per market. These metrics let operators distinguish "no one is submitting", "promotion is broken", and "execution is stuck after promotion", which are very different incidents. A system with direct ingress but no promotion telemetry is effectively blind.

### 32. Decide the fate of legacy async liquidity

This is a product call, but it needs a technical answer, not indefinite limbo. Either deprecate it, freeze it as compatibility-only, or deliberately rebuild it as a first-class v5 lane with its own routing and observability. My view is to deprecate unless there is strong product demand, because the system already has enough async semantics to harden. Indefinite half-support is the most expensive option.

## Tranche G — Measured Optimization Work

### 33. Same-user reveal packing and ALT automation

This optimization belongs after correctness and observability, not before. The relayer should detect same-user adjacency opportunities when building reveal or unified execute transactions and use ALTs automatically when account compression is the only blocker. Keep the planner deterministic and telemetry-rich so operators can see when packing was attempted and whether it improved batch width. This should be a strictly optional optimization layer, never a correctness dependency.

### 34. Event-queue preflight and consume-events planning

Execution success is not enough if event queues become the next bottleneck. Add a planner step that inspects downstream event-queue conditions and incorporates consume-events work into execution planning when needed, rather than letting it accumulate as invisible debt. The design pattern should be backpressure-aware execution: execution planning should know when market-side queues are near saturation and adjust accordingly. This is especially important once direct ingress increases burstiness.

### 35. Hot-path Orderbook::new_order allocation cleanup and batch funding

This is an on-chain performance hygiene task and should be treated surgically. Profile where `new_order` still allocates or copies unnecessarily under queued execution, then remove that overhead without changing visible semantics. Pair that with batch funding or account-preparation cleanup where the executor currently pays repeated fixed costs per item. This is classic hot-path work: benchmark first, patch narrowly, and keep the diff easy to review.

## Tranche H — Reserved-Semantics Optional Expansion

### 36. If desired, design on-chain reservations for queued async actions

Only do this if the product explicitly wants stronger post-acceptance guarantees. The correct model is an explicit reservation state attached to accepted async items that encumbers the relevant health or funding dimension until execution, expiry, or cancellation. Do not smuggle reservation semantics into the direct pool or optimistic state alone; if it matters, it has to be an on-chain invariant. Until this tranche is approved, keep the public contract best-effort and document it clearly.

### 37. Thread reservation state through health checks, expiry, autodrop, and future async cancel flow

Reservations are not a single flag; they are a full lifecycle. If added, they must participate in health computation, queued-item expiry, admin drop safety, and any future user-initiated cancel path so the system never leaks encumbrances. This will touch many core invariants, so the design should stay narrow: reserve only what is necessary for the supported async action set and reject over-general reservation machinery. Reservation logic is where complexity explodes if left vague.

### 38. Extend replay, metrics, and tooling for reservation lifecycle

If reservation semantics exist, replay and observability must expose them explicitly. Add reservation-created, reservation-released, reservation-expired, reservation-consumed, and reservation-force-cleared events, then reflect them in operator tooling and user-visible status where relevant. The append-only event model from earlier tranches is what makes this tractable. Do not try to infer reservation state from queue state after the fact.

## Tranche I — Still-Gated Future Work

### 39. Exact-health fast paths

This is worthwhile only after the main queue and direct-path architecture is stable. Add targeted fast paths for exact-health recomputation where profiling shows the current path is too expensive, but keep them behind tight invariants and explicit tests. Fast paths that silently disagree with canonical health are worse than no fast path at all. This work belongs late because it optimizes a stable model rather than trying to discover one.

### 40. MakerReplaceBand and MakerBandState

Treat this as a separate product-capability tranche, not an incidental extension of the queue rewrite. It likely needs its own message model, state transitions, and execution semantics, and those should be designed against the stabilized async lane rather than bolted onto it midstream. The main design rule is to avoid contaminating the base queue abstractions with maker-specific lifecycle state unless the feature is firmly committed. Otherwise the core executor gets more complex for no immediate value.

### 41. Orderbook V2 and open-order-slot refactor

This is the deepest future structural change on the list and should not be conflated with queue-family cleanup. If pursued, it deserves a separate architecture doc describing slot ownership, cancellation semantics, matching behavior, and migration from existing orderbook assumptions. It will likely simplify several current hot paths, but it also risks broad state churn. The correct posture is deliberate refactor after queue and execution invariants are already stable.

## Fee Model Implementation Epics

The fee model should be implemented as a relayer-owned subsystem with public read APIs, one authenticated internal credit API, and a deterministic balance engine. Do not put billing logic in the harness. The harness may proxy or display state, but the relayer owns fee admission because it owns intent acceptance.

### Epic 1. Fee account schema and balance engine

Build a durable per-`mango_account` fee ledger with a materialized balance row and an append-only journal, exactly as described in `v5-fees-model.md`. The materialized row exists for fast admission checks; the append-only ledger exists for audit, reconciliation, and rollback analysis. Keep accounting integer-only in lamports, split sponsored and paid credit, and model reservations explicitly even if the first release only holds and debits within a single submit lifecycle. The implementation pattern should be transactional row update plus immutable journal write, never "journal later if we remember".

Action items:
- Add `fee_accounts`, `fee_ledger_entries`, and `credited_deposit_hashes` tables plus uniqueness constraints.
- Implement a relayer-local balance service with atomic `ensure_account`, `reserve`, `release`, `debit`, and `credit_deposit` operations.
- Make all submit-time mutations idempotent by request ID so retries do not double-debit.

### Epic 2. Account creation and sponsored seed policy

Fee accounts should be created lazily on first authenticated relayer interaction for a `mango_account`, not pre-created for every chain account in existence. Credit the configured `0.1 SOL` sponsored seed exactly once per fee account, but ship production guardrails immediately because this policy is farmable. My recommendation is simple: seed only after first successful authenticated submit or enrollment, and cap automatic seeds per `user_owner` until behavior is proven safe. If product insists on unconditional seeding, at least instrument abuse from day one.

Action items:
- Add `ensure_fee_account(mango_account, user_owner)` to submit and fee-state read paths.
- Track seed provenance and expose sponsored-vs-paid balances separately in API responses.
- Add abuse telemetry: seeded accounts per owner, first-use lag, and seed burn rate.

### Epic 3. Deposit attribution and authenticated `/fees-deposited`

Use one common SOL deposit address and require structured attribution via memo or a back-office mapping workflow. A settlement worker should verify the transfer on-chain, compute the deterministic `deposit_credit_hash`, and call an internal-only `/fees-deposited` endpoint on the relayer. That endpoint should be idempotent, burn the hash, write the deposit-credit journal entry, and return the updated fee state. Do not let the frontend or SDK mint credits directly; the entire point of the endpoint is to centralize fraud and duplication checks.

Action items:
- Implement a settlement worker or indexer that watches the deposit address and validates finality.
- Add authenticated relayer endpoint `POST /fees-deposited` with service-to-service auth only.
- Support pending deposits for owner-scoped deposits that lack an unambiguous target `mango_account`.

### Epic 4. Fee quote engine and dynamic pricing model

Treat fee pricing as a pure function over queue pressure, relayer pressure, and observed on-chain landing cost. The quote engine should publish a prevailing fee and the factors behind it, not just a black-box number, because operators and users both need to understand spikes. Keep the normal cap at `0.01 SOL` per intent and require explicit warning states when emergency pricing pushes above that. Pricing code should live in one module with versioned parameters, so policy changes are reviewable and testable instead of scattered across submit handlers.

Action items:
- Implement a quote service that consumes queue backlog, backlog age, relayer inflight pressure, and recent on-chain fee samples.
- Expose a public read endpoint such as `GET /fees/quote?mango_account=...&market_index=...`.
- Emit structured quote diagnostics so dashboards can explain why pricing moved.

### Epic 5. Intent admission, reservation, and debit path

Every relayed async intent should pass through the same fee admission sequence: ensure account, compute prevailing fee, compare against user `max_fee_lamports` policy, reserve funds, attempt acceptance, then either release or finalize the debit. The reservation is short-lived and local in the first release; it is there to make retries and partial failures safe, not to promise on-chain execution. Rejections should be explicit and stable, especially `"insufficient fees, deposit more"` and `"prevailing fee exceeds max_fee_lamports"`. Do not debit before an intent is accepted into the relayer-owned async path.

Action items:
- Extend the relayer submit envelope to accept `max_fee_lamports` or `auto`.
- Add stable API errors and machine-readable reason codes for insufficient balance and max-fee rejection.
- Write tests for success, duplicate request, submit failure after reservation, and concurrent submits on the same fee account.

### Epic 6. Fee state read APIs and operator tooling

Users and operators need a first-class read surface for fee balances, pending deposits, and recent debits. Add public authenticated read endpoints for fee state and deposit context, plus operator endpoints or tools for journal inspection, manual adjustments, and reconciliation status. Keep these APIs relayer-owned because they are views over relayer-local truth. The frontend should never have to reverse-engineer fee balances from failed submits or raw deposit transactions.

Action items:
- Add `GET /fees/state/:mango_account` returning balances, recent debits, and optional quota fields.
- Add `GET /fees/deposit-context?user_owner=...&mango_account=...` returning deposit address, memo, and any pending deposits.
- Build operator commands for manual credit, manual debit adjustment, and hash-burn inspection with full audit logging.

### Epic 7. SDK integration

The SDK should make fees visible and boring. Add typed clients for fee state, deposit context, and fee quote reads, plus helper methods that decorate async submit requests with `max_fee_lamports` and interpret fee rejection errors consistently. Keep the SDK opinionated: balances in lamports plus UI helpers, explicit sponsored-vs-paid fields, and one deposit-context helper that tells integrators exactly what memo to send. Do not force frontend teams to stitch fee UX out of raw HTTP calls.

Action items:
- Add typed methods like `getFeeState`, `getFeeQuote`, and `getFeeDepositContext` to the TS client.
- Extend async submit builders to accept `maxFeeLamports: bigint | 'auto'`.
- Add SDK-level error classes for `InsufficientFees` and `FeeAboveUserMax`.

### Epic 8. Frontend integration

Frontend work should focus on clarity, not cleverness. Users need one fee panel showing available balance, sponsored balance remaining, paid balance remaining, current fee quote, and a deposit CTA that generates the exact memo and address to use. When a user sets a non-auto `max_fee_lamports`, the UI should show what happens if the quote rises above that threshold before submit. Failed submits should route directly to the fee panel with the current quote and deposit instructions, not a generic error toast.

Action items:
- Add a fee state card on account pages and in async order-entry flows.
- Add deposit UX that shows the common address, exact memo, copy actions, and pending-credit status.
- Add submit-time warnings when market pressure is high or prevailing fees are near the normal cap.

### Epic 9. Rollout, safeguards, and policy tuning

Roll this out in phases. First ship read APIs and internal ledgering in shadow mode, then credit the sponsored seed and record would-be debits without enforcing them, then turn on hard admission for a small market or account cohort, and only then make fees mandatory across async traffic. Keep emergency switches for seed policy, quote caps, and enforcement mode. Billing systems fail safest when they can degrade to "observe and warn" before they are trusted to block user flow.

Action items:
- Add an enforcement mode enum: `off`, `shadow`, `warn`, `enforce`.
- Start with one or two markets and a small internal cohort before full rollout.
- Publish operator dashboards for fee balance exhaustion, deposit-credit latency, and fee-rejection rate before enforcement goes global.

## Recommended Execution Order

If this work is split across teams, the critical path is still architectural, not implementation volume:

1. Tranche A tasks 1-5.
2. Tranche B tasks 6-10 in parallel with Tranche C task 11 and design completion for tasks 12-16.
3. Tranche C tasks 13-17.
4. Tranche D tasks 18-23 and Tranche E tasks 24-28.
5. Fee model epics 1-5 in shadow mode, then epics 6-9 for product rollout.
6. Tranche F onward only after the new queue model, direct path, and fee enforcement all have stable telemetry.

If there is one principle to keep: stop expanding the old architecture while trying to replace it. The code already contains enough evidence that mixed queue eras, mixed routing models, and hidden operational policy are the real sources of complexity.
