# v5 Roadmap

Written 2026-04-21 against branch `stage-v6` at
`/home/hetalkenaudekar/stagin4/experimental/cleanups/mng-v4`.

This file replaces `v4-roadmap-execution.md`.

The active execution path in this tree is already v5:

- per-market queue PDAs
- fixed-size ring buffer per market
- commit/reveal as the primary production path
- no pagination
- relayer-side per-market routing, reveal batching, WAL replay, drift resync,
  and autodrop

The next roadmap needs to reflect two facts at the same time:

1. The current v5 commit/reveal path is the real center of gravity.
2. We now want the program to also accept direct payload ingress cleanly,
   without undoing the reasons v5 moved away from storing full payloads in the
   main sequenced ring.
3. We now want AMQ-style semantics to be the default framing:
   asynchronous market execution by default, with synchronous balance-moving
   instructions as a parallel track.
4. We now want to consolidate the queue families down to **v2 and v5 only**:
   remove paginated v3/v4, and upgrade v2 to use per-market queue PDAs in the
   same spirit as v5 instead of a single shared account with sub-queues.

---

## Direct Answer

No: the previous roadmap did **not** include "allow both payload and
commit/reveal transaction types seamlessly" as a first-class target.

It also did **not** frame the system explicitly as:

- an async market-execution lane by default
- plus a synchronous deposit / withdraw lane running in parallel
- a long-term codebase that keeps only queue families `v2` and `v5`

It explicitly deprioritized:

- queue unification as a top-line goal
- reviving payload-mode abstractions as primary architecture
- expanding the v5 sequenced ring into a rich payload-carrying queue

That was reasonable for the prior state of the branch, but it is no longer
enough for the requirement now on the table.

---

## What Exists Today

### Current v5 path

- `ExecutionQueueV5` is a per-market ring buffer of small sequenced items.
- The on-chain v5 item shape stores commit hash and timing data, not payload.
- `execution_queue_v5_commit_market` writes sequenced commit records.
- `execution_queue_v5_reveal_execute_market` verifies reveal args against the
  stored commit hash and dispatches the payload.

This is already the natural async lane.

### Existing synchronous balance path

Synchronous deposit / withdraw is already implemented directly onchain:

- `token_deposit`
- `token_deposit_into_existing`
- `token_withdraw`

Those execute immediately and already form the right "sync lane" for an
AMQ-style split.

### Existing direct path

There is already a direct enqueue path in the legacy queue code:

- `execution_queue_enqueue_direct`
- `execution_queue_v3_enqueue_market_direct`

That path:

- verifies only the user signature, not the CTM signature
- allocates sequence immediately at enqueue time
- writes a normal pending queue item immediately
- enforces a delayed `min_execute_slot`

It does **not** do what we want now.

### Existing async liquidity path

There is also an older async liquidity path in legacy queue code:

- `execution_queue_enqueue_liquidity`
- `execution_queue_v3_execute_liquidity`

That path queues deposit / withdraw-style payloads with a liquidity delay.

Important: this is **not** the active v5 architecture. It is legacy
compatibility, not a v5-complete AMQ implementation.

### Current relayer / harness split

The offchain stack is not yet cleanly modeled as "v2 or v5".

Today:

- relayer submit / execute legacy path is selected by
  `EXECUTION_QUEUE_TOPOLOGY=v2|v3`
- v5 is enabled separately via `V5_ROUTE_ALL=true` plus per-market config
- executor batching uses `execute_multi` for v2 / v3
- v5 uses its own commit + reveal-batch pipeline, not the same executor path
- the optimistic harness replay model is mostly generic across both paths
- live onchain queue probing is only special-cased for `onchain_v5`

### New requirement

For the v5 path we want:

- a payload-type direct ingress transaction
- no CTM signature on direct ingress
- no sequence number on direct ingress
- a 4-slot speedbump
- then, on execute, assignment of `current highest sequence + 1`
- and coexistence with the current commit/reveal path without destabilizing it
- while making AMQ-style sync vs async routing explicit

---

## Queue Family Target

The desired steady state is:

- `v2`: full-payload queue with efficient `execute_multi`
- `v5`: commit/reveal queue with batched reveal/execute

And explicitly **not**:

- paginated v3 market roots / pages
- paginated v4-style queue migration scaffolding
- a relayer that thinks in terms of `v2|v3` plus an unrelated `v5` side path

### Best implementation direction

The cleanest way to get there is:

- keep `v5` as-is conceptually: one queue PDA per market, ring-buffered,
  small sequenced items
- rebuild `v2` around one queue PDA per market as well, instead of a single
  shared queue account with 16 sub-queues
- keep the semantic distinction:
  `v2 = payload queue`, `v5 = commit/reveal queue`
- remove paginated v3/v4 rather than trying to keep them as transitional
  first-class modes in the relayer and harness

Do **not** try to mutate the current shared-account v2 layout in place into a
per-market layout. The safer path is:

- define the per-market v2 account family explicitly
- migrate relayer / harness to that family behind a clean mode switch
- then delete v3/v4 and the old shared-account v2 plumbing

That avoids baking more special cases into the code that is supposed to become
simpler.

---

## Completion Review

### Already substantially done

- v5 onchain queue already uses per-market queue PDAs with fixed ring buffers
- v5 relayer submit path already supports per-market routing by `target_index`
- v5 reveal batching already exists and is the effective v5 "batch execution"
  mechanism
- v2 relayer executor path already supports efficient lane batching through
  `execute_multi`
- optimistic harness replay is mostly event-driven and already consumes the
  same accepted / enqueued / processed event model for both v2 and v5

### Partially done but structurally awkward

- v2 relayer path works, but still assumes a single shared queue account
- v2 executor head inspection / drop / admin logic reads raw bytes from the
  shared-account sub-queue layout
- startup config is split awkwardly:
  `EXECUTION_QUEUE_TOPOLOGY=v2|v3` for legacy and `V5_ROUTE_ALL` for v5
- harness queue probing is generic for replayed state, but live onchain queue
  reads only have a first-class `onchain_v5` source
- the relayer has one batching abstraction for v2 / v3 `execute_multi` and a
  separate batching abstraction for v5 reveal execution

### Not done

- v2 per-market queue PDA redesign
- deletion of paginated v3/v4 onchain state / instructions / admin flows
- a single relayer startup mode that cleanly means "run v2" or "run v5"
- a modular queue adapter boundary that lets submit / execute / probe / drop /
  optimistic integration vary by queue family without v3 baggage
- harness live onchain support for per-market v2 queues
- cleanup of the surrounding TS / scripts surface so it also reflects only
  v2 and v5

---

## AMQ Default Framing

The right way to frame the target architecture is:

- async lane: market-execution instructions that should be delayed, ordered,
  and executed through the per-market v5 queue
- sync lane: balance-moving instructions that should execute immediately

In this codebase today, that means:

- the current v5 FIFO market queue is the async lane
- direct payload ingress into that queue remains async, not sync
- `token_deposit` / `token_withdraw` are the sync lane

Strictly speaking, the sync path is not a queue at all. It is an immediate
execution lane. That is still the correct AMQ split.

### Recommended default classification

Default instruction classification should be:

- `PerpPlaceOrder`, cancel / replace, and similar market instructions:
  `Async`
- v5 direct payload enqueue for those market instructions: `Async`
- `token_deposit`, `token_withdraw`, and account-funding flows: `Sync`

### What is already done

Already present:

- async market lane in v5
- sync deposit / withdraw instructions
- legacy async liquidity queue for older queue versions

Not yet first-class in v5:

- explicit AMQ instruction taxonomy in docs / SDK / infra
- default routing logic that chooses sync vs async by instruction family
- observability that treats sync and async lanes as parallel products
- a v5 answer for legacy async liquidity

### The important unresolved design choice

There are two possible semantics for async queued actions relative to
synchronous withdraw:

1. Best-effort async semantics

- async actions are admitted and sequenced
- no funds are reserved onchain at async ingress
- a later synchronous withdraw can still change account state
- the queued async action may fail at execute-time if health / funds no longer
  permit it

2. Reserved async semantics

- async ingress creates an onchain reservation / encumbrance
- synchronous withdraw must respect that reservation
- queued actions have stronger execution guarantees

The best **non-breaking** path is to keep best-effort async semantics for the
first v5 expansion.

Reason:

- current v5 commit/reveal does not reserve assets onchain at commit
- adding reservations touches account state, health, release-on-expiry, replay,
  and cancel semantics
- that is a much larger project than direct staging + unified execute

If stronger "funds locked at enqueue" semantics are wanted later, that should
be a separate tranche, not bundled into the first direct-ingress rollout.

### Recommendation for deposit / withdraw

For v5 AMQ-by-default:

- keep deposit / withdraw synchronous by default
- do **not** treat legacy async liquidity as the default user path
- do **not** port async liquidity into v5 in tranche 1 unless there is a
  concrete product requirement that needs delayed funding operations

This gives the cleanest product story:

- funding is synchronous
- market execution is asynchronous
- the queue remains focused on market actions

If we later decide there is value in async liquidity, it should be rebuilt as a
deliberate v5 feature instead of inheriting the legacy path by accident.

### What remains to make AMQ real in this codebase

Remaining work, even if deposit / withdraw stay synchronous:

- publish a canonical sync-vs-async instruction matrix
- route SDK / relayer / UI builders accordingly
- make executor and replay tooling lane-aware
- expose metrics separately for sync balance operations and async queued
  execution
- define failure semantics clearly for async items that lose solvency before
  execution
- decide whether legacy async liquidity is deprecated, frozen for compatibility,
  or reintroduced in v5 later

If the product later wants reserved async semantics, add a separate design
tranche for:

- onchain reservation state at async ingress
- reservation-aware withdraw checks
- reservation release on execute / expire / drop / cancel
- replay and telemetry for reservation lifecycle

---

## The Core Design Decision

The best non-breaking path is:

- keep the current v5 sequenced commit/reveal ring intact
- do **not** turn the main v5 ring back into a payload-carrying queue
- add a separate direct-ingress staging area for payload items
- promote staged direct items into the main sequenced space only during execute
- use a new unified v5 execute path for markets that enable direct ingress

This keeps the current production path stable while adding the new capability
without forcing a risky rewrite of the existing ring design.

---

## Options Considered

### Option A — assign sequence immediately on direct enqueue

This is the legacy approach.

Why not:

- it does not satisfy the new sequencing requirement
- it causes direct items to consume sequence space before they are ready
- it creates avoidable contention with the commit/reveal path
- it reintroduces the exact kind of queue-shape coupling we were trying to
  reduce in v5

### Option B — execute direct items straight from a staging pool with no
sequence promotion

Why not:

- it breaks the "single ordered market sequence" mental model
- replay, telemetry, and operator tooling become harder to reason about
- direct items can appear to bypass sequenced commit/reveal work

### Option C — stage direct items unsequenced, then promote them into the main
sequence during execute

This is the recommended design.

Why:

- it satisfies the new requirement literally
- it preserves the v5 ring as the single ordered sequenced surface
- it avoids bloating the main ring with payload bytes
- it gives a clean compatibility story: old commit/reveal still works, new
  unified execute is used only where direct ingress is enabled

---

## Recommended Design

### 1. Keep two ingress classes, one ordered sequencer

We should support two ingress types:

- `CommitReveal`: current v5 path, offchain commit then reveal/execute
- `DirectPayload`: new direct enqueue path, user-signed payload with no CTM
  signature and no sequence

But there should still be only **one** ordered per-market sequence space.

That means direct items should not bypass the sequence. They should enter it
only at execute-time promotion.

### 2. Add a separate direct staging store

Do **not** put full direct payload bytes into the current `CommitItemV5`.

Instead add a sidecar direct staging store per market.

Recommended shape:

- a new PDA, one per `(group, market_index)`, for staged direct payload items
- fixed-capacity ring/FIFO, similar in spirit to the main queue but storing:
  payload bytes, canonical dispatch account metas, hashes, timing, and identity

This is safer than inflating the existing queue PDA layout because:

- existing v5 queue accounts and raw parsers stay valid
- migration can be incremental
- the current commit/reveal ABI and telemetry stay usable while the new lane is
  built

### 3. Add a new v5 direct enqueue instruction

Add a new instruction rather than mutating the current commit/reveal ingress.

Recommended instruction:

- `execution_queue_v5_enqueue_direct_market`

Semantics:

- verify payload decode and deterministic prechecks
- verify market binding
- verify canonicalized dispatch account metas
- verify user signature only
- store the item in the direct staging pool
- record `eligible_at_slot = current_slot + 4`
- store no sequence yet

### 4. Add a new unified v5 execute instruction

Do **not** mutate `execution_queue_v5_reveal_execute_market` in-place as the
primary migration target.

Recommended instruction:

- `execution_queue_v5_execute_market`

This instruction should handle both classes:

- existing sequenced commit/reveal heads
- eligible direct staged items

The old `execution_queue_v5_reveal_execute_market` remains as a compatibility
path for commit-only markets during migration.

### 5. Promotion semantics for direct items

On execute:

- scan the direct staging head in FIFO order
- only consider items with `eligible_at_slot <= current_slot`
- for each promotable item, assign `sequence = current max_seen_sequence + 1`
- create a lightweight sequenced head/tail record in the main v5 ring that
  references the staged direct item
- emit `QueueItemEnqueued` with the newly assigned sequence

Important:

- promotion should happen during execute, not ingress
- promotion should not require the main ring to be empty
- promoted direct items should sit behind whatever older sequenced items
  already exist

That gives direct items a real place in total order without allowing them to
cut ahead of already-sequenced commit/reveal traffic.

### 6. Execution semantics for promoted direct items

When the main queue head is a promoted direct item:

- load payload and account metadata from the direct staging store
- verify the passed execution accounts match what was staged
- decode and dispatch exactly as with current payload execution
- clear the main-ring head and the referenced direct staged item together

This keeps all sequencing in the main ring while keeping large payload storage
out of it.

### 7. Signature format for direct ingress

Do not overload the current v5 commit/reveal user-intent hash.

The direct path should have its own canonical message that does **not** bind a
sequence number.

Recommended message binds:

- `group`
- `mango_account`
- `user_owner`
- `kind`
- `target_kind`
- `market_index`
- `payload_hash`
- canonical direct-dispatch `accounts_hash`
- `expires_at_slot`
- a replay-protection nonce

This should be a new versioned direct-intent message, not a fake "sequence 0"
reuse of the commit/reveal message.

### 8. Rollout gating

Direct ingress should be gated per market.

Do not enable direct ingress on a market until:

- the new unified v5 execute path is deployed
- infra is reading the direct staging pool
- replay / trace / metrics are updated for the direct lane

That is the cleanest non-breaking rollout.

---

## Why A Sidecar Direct Pool Is Better Than Reworking The Main v5 Ring

Using a separate direct pool is the better tradeoff because it avoids breaking
the strongest properties of the current v5 design:

- small sequenced items
- simple raw parsing
- isolated commit/reveal semantics
- low coupling between ingress storage and execute-time account fanout

If we instead expand the main ring to store direct payload bytes and direct
dispatch account metadata, we pay for that complexity in:

- queue account layout migration
- parser churn
- larger write surfaces on the main sequence structure
- higher operational risk for the path that is already working

The sidecar direct pool localizes the new complexity to the new feature.

---

## Priority Plan

### Tranche A — queue-family consolidation

This tranche should happen before further feature expansion.

1. Lock the queue-family target in code and docs:
   only `v2` and `v5` remain supported.
Scope: `Dual`

2. Define the new per-market v2 account family and instruction surface.
Scope: `Primarily Onchain`

3. Remove paginated v3/v4 from the forward plan and make them compatibility
only until deletion lands.
Scope: `Dual`

4. Replace the relayer's split config model
   (`EXECUTION_QUEUE_TOPOLOGY=v2|v3` plus `V5_ROUTE_ALL`)
   with a single explicit queue-mode model:
   `QUEUE_MODE=v2|v5` or equivalent.
Scope: `Primarily Infra`

5. Introduce a queue-family adapter boundary in the relayer:
   enqueue, head-inspect, batch-execute, drop, telemetry, optimistic hooks.
Scope: `Primarily Infra`

### Tranche B — v2 modernization

6. Implement v2 per-market queue PDAs with ring-buffer semantics parallel to
v5, while preserving payload-queue and `execute_multi` behavior.
Scope: `Primarily Onchain`

7. Port v2 admin / drop / configure flows to the new per-market layout.
Scope: `Primarily Onchain`

8. Port relayer submit and executor logic from shared-account v2 parsing to
per-market v2 queue accounts.
Scope: `Primarily Infra`

9. Keep efficient v2 lane batching by reusing `execute_multi`, but retarget it
to per-market v2 queues instead of shared-account sub-queues.
Scope: `Dual`

10. Add migration / bootstrap tooling for new v2 per-market accounts.
Scope: `Dual`

### Tranche C — AMQ framing and direct-path architecture

This tranche is the new critical path.

11. Lock the AMQ instruction taxonomy:
   sync deposit / withdraw, async market execution.
Scope: `Dual`

12. Finalize the direct-pool and unified-execute design for the async lane.
Scope: `Dual`

13. Add the new direct-intent signature format and direct enqueue instruction.
Scope: `Primarily Onchain`

14. Add the new unified v5 execute instruction that can process both promoted
direct items and commit/reveal items.
Scope: `Primarily Onchain`

15. Upgrade the executor to read the direct pool and build unified execute
transactions.
Scope: `Primarily Infra`

16. Add per-market feature gating so direct ingress is disabled until the new
executor is live for that market.
Scope: `Dual`

17. Make SDK / client / relayer routing default to:
   sync for funding, async for market execution.
Scope: `Primarily Infra`

### Tranche D — relayer / harness parity

18. Make relayer startup select exactly one queue family cleanly:
    `v2` or `v5`.
Scope: `Primarily Infra`

19. Ensure both queue families use the same optimistic-state integration
surface in the relayer.
Scope: `Primarily Infra`

20. Add harness live queue readers for both per-market `v2` and per-market
`v5`, instead of only special-casing `onchain_v5`.
Scope: `Dual`

21. Unify queue telemetry and watermarks so both families publish the same
market-scoped health surface.
Scope: `Dual`

22. Remove v3-specific queue root / page probing from relayer and harness.
Scope: `Dual`

23. Fill TS / script gaps so v2 and v5 are the only exposed queue families.
Scope: `Primarily Infra`

### Tranche E — correctness and observability

24. Add v2-per-market and v5 tests for submit, replay, ordering, expiry, and
batch execution.
Scope: `Dual`

25. Extend replay / optimistic-state code so direct-path accepted items and
promoted sequences are reflected correctly.
Scope: `Primarily Infra`

26. Add trace / event / metrics coverage for direct ingress and promotion.
Scope: `Dual`

27. Add lane-aware observability so sync funding and async execution are
reported separately.
Scope: `Dual`

28. Add admin tooling for direct-pool inspection and cleanup.
Scope: `Primarily Infra`

### Tranche F — remaining v5 hardening

29. Finish the stateful ingress precheck work.
Scope: `Dual`

30. Add a real v5 benchmark floor, including direct-path promotion and mixed
traffic with commit/reveal.
Scope: `Dual`

31. Expand stall telemetry with direct-pool age, eligible backlog, and
promoted-but-not-yet-executed backlog.
Scope: `Dual`

32. Decide the fate of legacy async liquidity:
    deprecate, freeze as compatibility only, or rebuild as a deliberate v5
    lane.
Scope: `Dual`

### Tranche G — measured optimization work

33. Same-user reveal packing and ALT automation.
Scope: `Primarily Infra`

34. Event-queue preflight and consume-events planning.
Scope: `Dual`

35. Hot-path `Orderbook::new_order` allocation cleanup and batch funding.
Scope: `Primarily Onchain`

### Tranche H — reserved-semantics optional expansion

36. If desired, design onchain reservations for queued async actions so sync
withdraw cannot invalidate queued intent after ingress.
Scope: `Dual`

37. Thread reservation state through health checks, expiry, autodrop, and any
future async cancel flow.
Scope: `Primarily Onchain`

38. Extend replay / metrics / tooling for reservation lifecycle.
Scope: `Primarily Infra`

### Tranche I — still gated future work

39. Exact-health fast paths.
Scope: `Primarily Onchain`

40. MakerReplaceBand / MakerBandState.
Scope: `Primarily Onchain`

41. Orderbook V2 / open-order-slot refactor.
Scope: `Primarily Onchain`

---

## Scoped Workstreams

### Primarily Onchain

- per-market `v2` queue account family mirroring v5's per-market allocation
  model
- v2 payload-queue state and raw parsing without shared-account sub-queues
- v2 `execute_multi` on the new per-market account family
- deletion of paginated v3/v4 roots, pages, and execute paths
- `ExecutionQueueV5DirectPool` state and layout
- `execution_queue_v5_enqueue_direct_market`
- `execution_queue_v5_execute_market`
- AMQ classification constants / config where onchain exposure is needed
- direct-intent canonical message verification
- promotion from direct pool into sequenced v5 ring
- direct-head dispatch from staged payload storage
- per-market direct-ingress config flags
- on-chain events for direct accepted / promoted / expired / processed
- tests for promotion ordering, expiry, queue head behavior, and cleanup
- optional future reservation state for queued async items

### Primarily Infra

- single queue-family mode selection at startup (`v2` or `v5`)
- queue-family adapter boundary in relayer submit / execute / admin code
- relayer support for per-market `v2` queue PDAs
- executor support for direct-pool inspection and unified execute tx building
- default routing: funding sync, market execution async
- queue / direct-pool telemetry probes
- admin tooling for pool inspection and recovery
- SDK / TS builders for v5 direct enqueue and unified execute
- SDK / TS builders for modernized per-market `v2`, including missing v2
  `execute_multi` helper parity
- explicit sync-lane builders / routing docs for deposit and withdraw
- direct-path trace ingestion so users get the same observability as relayed
  commit/reveal flow
- replay ingestion of direct enqueue, promotion, and processing lifecycle
- compatibility posture for legacy async liquidity in scripts / tooling
- removal of v3-specific runtime config, page refresh, and v3 queue probes

### Dual

- formal deletion plan for paginated v3/v4
- migration / bootstrap flow for per-market `v2`
- harness support for onchain live queue reads for both `v2` and `v5`
- unified queue telemetry model across both families
- canonical sync-vs-async instruction matrix and product policy
- exact event model for direct enqueue accepted / promoted / expired paths
- rollout gating and market-by-market enablement
- end-to-end and benchmark coverage
- stateful precheck completion where onchain and infra mirrors must stay in
  sync
- compatibility story between old `reveal_execute_market` and the new unified
  execute path
- decision on best-effort versus reserved async semantics
- lane-aware observability and operational runbooks

---

## Immediate Next Step

Lock the architecture first.

The next concrete deliverable should be a short implementation spec covering:

- queue-family consolidation (`v2` + `v5` only)
- per-market `v2` account / PDA shape
- relayer queue-mode startup model
- relayer / harness adapter boundary
- AMQ instruction taxonomy
- direct-pool PDA shape
- direct-intent message format
- unified execute instruction behavior
- promotion rules
- event model
- per-market rollout gates
- the explicit decision that tranche-1 async semantics are best-effort unless
  the reservation tranche is separately approved

Only after that should the work split in parallel across onchain and infra.

---

## Critical Files By Scope

### Primarily Onchain

- new per-market `v2` state file(s) under `programs/mango-v4/src/state/`
- `programs/mango-v4/src/instructions/execution_queue.rs`
- `programs/mango-v4/src/state/execution_queue_v5.rs`
- new direct-pool state file under `programs/mango-v4/src/state/`
- `programs/mango-v4/src/instructions/execution_queue_v5.rs`
- `programs/mango-v4/src/lib.rs`
- `programs/mango-v4/src/accounts_ix/execution_queue_v5.rs`
- `programs/mango-v4/src/error.rs`
- `programs/mango-v4/src/instructions/token_deposit.rs`
- `programs/mango-v4/src/instructions/token_withdraw.rs`
- new v5 direct-path tests under `programs/mango-v4/tests/cases/`

### Primarily Infra

- `bin/service-mango-execution-engine/src/main.rs`
- `bin/service-mango-execution-engine/src/v5_pipeline.rs`
- `ts/client/scripts/execution-queue/continuum-state-harness.ts`
- `rust-harness/src/replay.rs`
- `rust-harness/src/engine.rs`
- `rust-harness/src/node.rs`
- `ts/client/src/client.ts`
- `ts/client/src/executionQueue.ts`
- `ts/client/src/executionQueue.spec.ts`

### Dual

- event / trace plumbing between the relayer, direct-pool observer, and
  harness replay path
- benchmark and E2E coverage
- operational docs and rollout docs

---

## What Should Not Happen

- Do not bloat `CommitItemV5` into a full payload-carrying item type.
- Do not repurpose the current commit/reveal hash format for direct ingress by
  pretending the sequence is zero.
- Do not enable direct ingress on a market before the new unified executor is
  live for that market.
- Do not make direct payload items bypass the single ordered per-market
  sequence.
- Do not keep v3/v4-specific runtime structures around inside the relayer once
  the only supported queue families are `v2` and `v5`.
- Do not "upgrade v2" by preserving the shared-account sub-queue model and
  just renaming flags around it; that leaves the hardest operational coupling
  in place.
- Do not leave the relayer in a split-brain config where legacy uses one
  startup model and v5 uses another.
- Do not describe the sync deposit / withdraw path as "missing" when it is
  already implemented; the missing work is classification, routing, and policy.
- Do not couple tranche-1 direct ingress to a reservation system unless the
  larger health / encumbrance work is explicitly chosen.

Those are the easiest ways to make the system harder to reason about than it
needs to be.
