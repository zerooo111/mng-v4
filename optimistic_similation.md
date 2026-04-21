# Optimistic Harness Assessment

Date: 2026-04-21

## Target

Desired behavior:

- Pre-play transactions and show users the updated state in under 10 ms.
- Account for all earlier transactions in the queue before showing the result.
- Match on-chain behavior closely enough that if commit and reveal both land with correct data, the optimistic state almost always matches the eventual on-chain state.

## Short Answer

We are not close to that target in the deployed v5 path today.

The core replay engine is much better than the old TypeScript-only simulator, and the architecture is pointed in the right direction: both the relayer and the harness use the Rust replay engine, and the relayer has an in-process local state engine enabled. But the active v5 commit/reveal path still does not feed accepted intents into the optimistic replay path that users read from, and the replay engine still carries ordering assumptions that match older global-queue behavior better than the current v5 per-market queues.

So today we have:

- a stronger replay kernel,
- better commit/reveal hygiene,
- better status tracing,
- but not a true sub-10 ms optimistic user-facing state path for v5.

## What Is Good Today

### 1. Rust replay engine is real and already deployed

The optimistic/confirmed state machinery is no longer just the old TS simulator. The relayer and harness are now built around the Rust replay engine.

Relevant code and config:

- `bin/service-mango-execution-engine/src/main.rs`
- `rust-harness/src/replay.rs`
- `.devnet/systemd/devnet-stack.env`

Notable current settings:

- `CTM_RELAYER_LOCAL_STATE=true`
- `CTM_RELAYER_LOCAL_STATE_BOOTSTRAP_URL=http://127.0.0.1:9091/state/full?view=confirmed`

That means the relayer already has an embedded in-process `ContinuumStateEngine`, rather than depending entirely on the HTTP harness in the hot path.

### 2. The replay engine has the right shape conceptually

The current design is:

- confirmed baseline from on-chain snapshot,
- plus replayed accepted intents for optimistic view,
- plus divergence tracking and periodic rebases.

That is the correct high-level structure for the target system.

### 3. The v5 ingress path already blocks many obviously bad intents

The v5 precheck path rejects several classes of requests that would otherwise fail at reveal time:

- malformed payloads,
- terminal/expired payloads,
- dispatch-shape mismatches,
- stale expiry windows,
- phantom cancels,
- admission-saturated queue states.

This is valuable because it reduces the gap between "optimistic accepted" and "eventual on-chain outcome."

### 4. Commit/reveal operational handling is much better

The v5 path now does several important things correctly:

- emits accepted/submitted/rejected status events,
- uses preflight on commit tx send,
- persists reveal material only after commit send succeeds,
- guards against sequence collisions,
- makes failures visible instead of silently dropping them.

That is good infrastructure for eventual correctness and observability.

## Where We Fall Short Today

### 1. The v5 path is not actually applying optimistic state on accept

This is the biggest gap.

In the legacy path, accepted intents are:

- applied to the relayer's local optimistic engine, and
- emitted as `relay_intent_accepted` into the harness ingest path.

That happens around:

- `bin/service-mango-execution-engine/src/main.rs` lines near `7714`
- `bin/service-mango-execution-engine/src/main.rs` lines near `7833`

But the active v5 path returns earlier after commit handling and status emission, around:

- `bin/service-mango-execution-engine/src/main.rs` lines near `7605-7619`

It emits `relay_intent_status` records, but it does not run the same optimistic accepted-intent flow used by the legacy path before returning.

Effect:

- v5 orders are not entering the optimistic replay state that users read.
- Status tracing exists, but optimistic state projection is missing.
- The system is closer to "commit/reveal tracker" than "true optimistic harness" for v5.

### 2. The harness confirms this gap in live behavior

The live harness behavior is consistent with missing accepted-intent ingest:

- `intents_total` in `/healthz` is very low.
- divergence history is saturated with `processed_without_relay_intent`.
- `/state/queue/0?view=optimistic` showed:
  - `pending_count=0`
  - `unmatched_processed_count=9`

That means the harness is observing processed on-chain queue activity, but it is not seeing the matching accepted optimistic events for those items.

That is the opposite of the desired optimistic flow.

### 3. The replay engine still orders pending CTM intents like a global queue

This is the second major correctness issue.

The active deployment is explicitly v5 per-market queues:

- `devnet-metadata-2026-04-21.md` states every market is a v5 per-market queue.

But the optimistic pending replay logic still groups by `group` only:

- `rust-harness/src/replay.rs` around `1889`

And its start sequence is computed from the max baseline confirmed sequence across markets:

- `rust-harness/src/replay.rs` around `3051-3067`

That is not the right queue model for v5.

Even after accepted intents start flowing into the optimistic engine, this ordering logic must change to be per market, not just per group.

### 4. Current read-path latency is nowhere near 10 ms

Live measurements from the relayer metrics show that the current hot path is still too slow:

- submit latency p50 about 139 ms
- margin-check stage p50 about 73 ms
- processed reconciliation latency p50 about 29.8 s

Even ignoring the long processed latency, the optimistic path is not presently near a sub-10 ms user-facing SLO.

The main reason is simple: there is still too much work and too much external dependency on the critical path.

### 5. The harness is still reconcile/poll driven on the read path

Current intervals:

- `CONTINUUM_HARNESS_RECONCILE_INTERVAL_MS=10000`
- queue refresh fallback defaults to 2000 ms

That is fine for correctness recovery and drift absorption, but it is not a primary sub-10 ms optimistic delivery path.

### 6. Drift is real in the live stack

Live reconciliation currently reports at least one market with book drift.

Observed example:

- market 0 replay best bid/ask did not match on-chain,
- nonzero bid and ask base-lot diffs,
- `markets_with_drift=1`

And the relayer is configured with:

- `CTM_RELAYER_HARNESS_REJECT_MARKET_DRIFT=false`

So the system is currently willing to keep accepting intents even while the read-path harness shows drift.

That is reasonable for experimentation, but not for a "optimistic state should almost always match eventual on-chain state" promise.

### 7. The relayer local state is bootstrapped once, not continuously rebased

The relayer local engine is currently described as:

- one GET at startup,
- then never again,
- future work replaces this with direct Rust on-chain reading.

That is not sufficient for parity with:

- oracle changes,
- funding changes,
- token balance changes,
- external on-chain account mutations,
- queue activity produced outside the relayer.

For a high-confidence optimistic engine, rebasing cannot be one-shot.

### 8. Some reveal-time parity checks are still deferred

`v5_precheck.rs` explicitly says that several stateful stages are deferred:

- mango account state checks,
- oracle freshness checks,
- health simulation,
- payer balance checks.

So the ingress path is better than before, but it is not yet full reveal-path parity.

### 9. Unsupported or partial exposures still exist

The replay engine handles perp and token-collateral state much better now, but it still tracks `unsupported_exposures` and partial health cases.

That means any claim of "nearly always matches on-chain" must be qualified:

- it can only be true inside the supported exposure surface,
- not for every possible Mango account shape.

### 10. Reveal throughput settings are still conservative

Current deployment notes show:

- no reveal ALT configured,
- `V5_REVEAL_BATCH_SIZE=1`

That does not directly block optimistic preplay, but it does widen the delay between optimistic acceptance and eventual on-chain confirmation/reconciliation.

The larger that gap gets under load, the more valuable correctness and rollback discipline become.

## Current Distance From Target

### Latency target: under 10 ms

Current state: far away.

Why:

- hot path still includes nontrivial margin-check work,
- some cache misses still fall through to HTTP or RPC,
- the user-facing optimistic path for v5 is not actually wired,
- current harness read-side refresh/reconcile cadence is much slower than the target.

Conclusion:

- we do not currently have a deployed sub-10 ms optimistic state path for v5.

### Fidelity target: should almost always match eventual on-chain state

Current state: partially but not enough.

What is good:

- same Rust semantics for replay,
- strong v5 prechecks,
- durable commit/reveal bookkeeping,
- explicit divergence tracking.

What breaks the claim:

- accepted intents are not entering the optimistic overlay on the v5 path,
- per-market queue ordering is not fully reflected in replay ordering,
- drift still appears in live reconciliation,
- local state is not continuously rebased,
- some stateful reveal checks are still deferred,
- unsupported exposures still exist.

Conclusion:

- we are not yet at the point where we should claim "if commit and reveal land, optimistic state almost always matches on-chain."

## What Must Be Done To Reach The Target

### 1. Wire v5 accepted intents into optimistic replay immediately

This is the first must-have.

On every accepted v5 intent, before returning to the caller:

- apply the intent to the relayer's in-process engine,
- expose that updated state to reads immediately,
- and optionally forward a corresponding accepted event to the external harness for visibility.

Without this, there is no real optimistic harness for v5.

### 2. Add rollback semantics for the user-visible preconfirm path

If we optimistically show state before durable commit success, we need a rollback path.

The relayer already has internal rollback through `UndoToken`, but the external read path does not yet have equivalent status-aware rollback behavior for accepted-then-rejected flows.

Needed behavior:

- accepted intent updates optimistic state,
- if commit send fails, remove or reverse it,
- if commit lands but later reveal fails terminally, transition it explicitly out of pending optimistic state.

### 3. Make queue semantics fully per-market in the replay engine

The optimistic replay engine must order pending CTM intents by:

- group,
- market,
- sequence

not by:

- group,
- sequence only

This change is required for correctness under the deployed v5 topology.

### 4. Move user-visible optimistic reads onto the relayer-local engine

If the target is under 10 ms, the cleanest architecture is:

- relayer accepts,
- relayer applies locally,
- user reads from the same in-process state or from a very thin same-process read API.

The separate harness should then become:

- replication,
- reconciliation,
- streaming,
- diagnostics,
- fallback,

not the primary low-latency truth source.

### 5. Remove remote I/O from the critical path

Sub-10 ms is not compatible with hot-path:

- harness HTTP fetches,
- RPC account reads,
- slow mutex or serialization paths,
- cross-process rebuilds.

This implies:

- local metadata caches,
- local account state caches,
- local health inputs,
- no rebuild-on-read behavior,
- no per-request remote dependency in the optimistic path.

### 6. Finish stateful parity checks in the v5 precheck path

To make optimistic accepted state line up with eventual reveal:

- health simulation has to be local and deterministic,
- oracle freshness assumptions have to match on-chain,
- account-state checks need to mirror reveal-time logic,
- payer balance or submission viability checks need to be included where relevant.

The goal is:

- if an intent passes precheck and commit lands, reveal failure should be rare and operational, not semantic.

### 7. Rebase continuously from on-chain state

The local engine needs periodic or triggered rebases for:

- confirmed queue progress,
- oracle/funding updates,
- token-bank changes,
- account collateral changes,
- external on-chain mutations.

One-shot bootstrap is not enough.

### 8. Tighten drift policy

Once the optimistic path is trusted, drift should become an active control signal.

That likely means:

- detect drift quickly,
- suppress optimistic replay or reject new optimistic admits for drifted markets,
- trigger fast direct rebase,
- only resume normal optimistic admission once the market is back in sync.

### 9. Define a supported-surface contract

If the fidelity guarantee is only valid for:

- perp-only accounts,
- or perp + token-collateral accounts without unsupported exposures,

then the system should say that explicitly and downgrade behavior outside that surface.

Otherwise operators and users will over-trust the optimistic output.

### 10. Improve reveal throughput to reduce optimistic-confirmed gap

This is secondary to correctness but still important.

Needed eventually:

- reveal ALT,
- better batching,
- tuned reveal pipeline depth,
- measured behavior under sustained load.

The shorter the optimistic-to-confirmed gap, the smaller the window for disagreement.

## Recommended Sequence Of Work

If the goal is to get to a credible production-grade optimistic harness, the work should happen in this order:

1. Wire v5 accepted intents into the relayer-local optimistic engine.
2. Add rollback for accepted-then-failed v5 paths.
3. Fix replay ordering to per-market queue semantics.
4. Serve the user-facing optimistic read path from the same in-process engine.
5. Remove hot-path remote I/O from margin and health checks.
6. Add continuous rebase from on-chain state.
7. Finish the missing stateful precheck parity work.
8. Tighten drift gating and policy.
9. Expand or constrain the supported exposure surface explicitly.
10. Optimize reveal throughput and batch settings.

## Bottom Line

Today we have the beginnings of the right system, but not the finished one.

The repo already contains:

- a serious Rust replay engine,
- a relayer-local state engine,
- v5 precheck infrastructure,
- rollback machinery for local apply,
- divergence diagnostics,
- periodic confirmed rebasing.

But the active v5 path still misses the key ingredient that would make the harness genuinely optimistic for users:

- accepted v5 intents are not being fed into the optimistic replay state that users read from.

And even after that is fixed, the replay engine still needs per-market queue semantics and continuous rebasing before we should trust it as a near-exact predictor of eventual on-chain state.

So the current answer is:

- architecturally promising,
- operationally much better than before,
- but still materially short of both the sub-10 ms target and the "almost always matches eventual on-chain state" target.

## Reveal / Executor Throughput Design Space

The optimistic harness is not the only gating factor. The executor/reveal path
is its own bottleneck, and if reveal lags too far behind commit then even a
perfect optimistic engine has to live with a larger rollback/reconciliation
window.

Target for this discussion:

- sustained `10-20` executed intents per second per market,
- with ordering close enough to on-chain reality that optimistic state remains
  trustworthy.

### Short Answer

We are not close to that throughput target on the current deployed v5
configuration.

Today the relayer is intentionally in the conservative regime:

- `V5_REVEAL_BATCH_SIZE=1`
- `V5_REVEAL_ALT_ADDRESS=` is empty
- reveal txs fall back to legacy/non-versioned encoding
- each signed reveal still carries its own ed25519 pre-instruction
- the worker is still fundamentally a head-serialized send loop

That setup is defensible for correctness, but it is not a design that should
be expected to robustly deliver `10-20` market-local executions per second.

### What Actually Limits Throughput Today

#### 1. Per-market execution is still head-serialized on chain

`execution_queue_v5_reveal_execute_market` repeatedly looks at the current
sub-queue head and only advances from there. So within one market, extra client
parallelism only helps in two cases:

- multiple head items are executed inside a single tx, or
- multiple txs land in the correct order.

Blind pipelining does not create true parallel execution inside a market.

#### 2. Tx size is the first hard ceiling

The current batch builder in `v5_pipeline.rs` is explicit about the byte
pressure:

- mixed-user reveals are estimated around `~281 B` marginal each,
- same-user reveals around `~217 B`,
- every user-signed reveal still gets its own ed25519 pre-ix,
- current comments in `.devnet/systemd/devnet-stack.env` say batch `2`
  overflows until a fresh reveal ALT exists.

That means we are byte-bound before we are scheduler-bound.

#### 3. Ordering sensitivity is inherent in the current model

When separate reveal txs are in flight for the same market, out-of-order landing
is not just a latency problem. It is a correctness problem because each tx is
validated against the then-current head.

So for this workload, "send more txs faster" is often the wrong optimization.
Ordered landing matters more than raw send rate.

#### 4. Repeated account metas are expensive

The reveal path still sends:

- fixed market accounts,
- then per-reveal dispatch accounts,
- then verifies layout on chain.

Many of those accounts are structurally repeated across reveals in the same
market. We are paying for that repetition on every batch.

#### 5. User-signature verification is a structural tax

For variants that require a user signature, the reveal handler verifies an
ed25519 pre-instruction against the expected commit hash. That is good for
correctness, but it is one of the main reasons batching hits size limits so
quickly.

#### 6. After bytes, CU and heap will become the next ceiling

The v0 batch builder already requests:

- `1_400_000` compute units
- `128 KB` heap

That is a signal that once byte pressure is reduced, compute and heap reuse
inside the batch handler become the next constraint.

### Interpreting The Old v4 Pipelining Result

The older v4 note showing `~40.5 reveals/s` on devnet at `50 ms` spacing is
useful, but only as proof that validator-side queue serialization can be
exploited when order happens to hold.

It should not be read as evidence that the current v5 path is already good
enough. The current deploy explicitly backed away from that regime:

- v5 batching is set to `1`,
- ALT is disabled after the fresh redeploy,
- comments already document out-of-order and tx-size failure modes.

So the old result is best treated as "there is upside if we can preserve
ordering and fit more work per land," not as the current production ceiling.

### Design Space

#### A. Runtime / Relayer Changes That Do Not Require A Program Change

These should be treated as table stakes, not as the full answer.

1. Fresh reveal ALT lifecycle, with automatic rotation

The relayer needs a reliable way to:

- bootstrap a fresh ALT after every program/market redeploy,
- detect stale ALT contents,
- roll forward without manual intervention.

This is necessary to move off the current `batch_size=1` regime, but ALT alone
is not enough.

2. Dynamic batch packing instead of fixed batch size

`V5_REVEAL_BATCH_SIZE` is too blunt. We should pack by:

- actual serialized tx size,
- expected ed25519 count,
- expected CU,
- account-key pressure.

That lets the relayer opportunistically send `2-4` item batches when the shape
fits, rather than pinning the whole market to the worst case.

3. Ordered submission, not generic RPC pipelining

The current worker uses normal RPC send paths with failover. That is good for
availability, but it does not solve ordered landing.

Higher-value options here are:

- leader-aware send paths,
- direct TPU/QUIC submission,
- bundle/private-relay submission where available,
- market-local ordered send queues with backpressure.

For this workload, preserving order is more important than fanning the same
head-sequential txs across multiple unordered RPC paths.

4. Intentional microbatch windows per market

Instead of firing reveal immediately on every new head item, use a very short
per-market accumulation window, for example `25-75 ms`, then submit one packed
batch or one ordered bundle.

This is especially attractive once the optimistic harness exists, because:

- the user can still see the optimistic result immediately,
- while the on-chain path trades a small extra delay for much higher throughput.

5. Dynamic priority-fee policy for reveal, not just commit

Reveal is the head blocker. Fee logic should react to:

- market backlog,
- aged head items,
- expected next-leader congestion.

Commit and reveal should not necessarily share the same fee policy.

6. Prebuilt reveal skeletons and faster local send path

This is not the main ceiling, but it helps tail latency:

- cache static instruction/account prefixes,
- precompute versioned-message skeletons,
- patch only blockhash and the small dynamic portion at send time.

7. Ingress-side coalescing where semantics allow it

If a single trader emits a burst of superseding intents, the cheapest reveal is
the one we never have to do.

This only works where semantics permit it, but for some market-maker style
flows it is worth considering:

- drop dominated intermediate replaces,
- collapse cancel-plus-new into one effective intent,
- avoid committing transient states that no longer matter.

That reduces reveal load rather than making each reveal cheaper.

#### B. Moderate Program Changes With Good ROI

These are the most interesting options if we want better throughput without
fully changing the trust model.

1. Merge same-user signatures into one ed25519 pre-instruction

The current code already calls this out as deferred work. It is not enough by
itself, but it is a cheap, concrete win because same-user bursts are common in
practice.

2. Split batch-common accounts from per-item accounts

This is one of the highest-leverage changes.

Today the reveal batch repeats too much account metadata per item. A better
shape is:

- one common fixed account set for the market/batch,
- one small per-item account tail for user-unique accounts only.

That attacks one of the main current sources of tx bloat without weakening
correctness.

3. Add specialized fast-path reveal instructions

Common flows should not pay for the fully generic dispatcher. Examples:

- place-only
- cancel-by-client-order-id
- cancel-all-on-market
- reduce-only amend/replace

Smaller payloads and fixed account layouts mean more items per tx.

4. Reuse decoded state and health machinery across a batch

Right now each reveal pays repeated decode/validation/health overhead. Once we
start fitting larger batches, the handler should reuse:

- market-level decoded state,
- repeated account lookups,
- reusable health-cache components,
- per-user state when multiple batch entries touch the same account.

That is how we prevent CU from becoming the next wall after bytes.

5. Session key or delegated trading authorization

This is a strong middle ground between:

- strict on-chain per-order user ed25519, and
- fully trusted-relayer mode.

A user could authorize a short-lived session key or on-chain delegate with
bounded permissions, and reveal would validate that capability instead of
paying a full per-order ed25519 pre-ix every time.

If we want to stay close to current trust assumptions while materially reducing
reveal bytes, this is one of the best options.

#### C. Structural Changes That Likely Matter Most For `10-20 TPS / Market`

These are the options most likely to change the ceiling in a meaningful way.

1. Trusted-relayer reveal mode

This is the clearest direct throughput win in the existing model.

The older throughput model already shows why:

- per-item user-sig verification dominates batch size,
- removing that check radically increases items per tx.

If off-chain proof, auditability, and operational trust are good enough, this
is the fastest path to materially higher reveal throughput.

2. Reveal-by-reference instead of reveal-by-inline-payload

Rather than carrying every payload and all dynamic metadata in the execute tx,
store a batch blob in a program-owned buffer/PDA and let the execute tx refer
to it.

That shifts the hot path from:

- "tx bytes contain all reveal material"

to:

- "tx bytes mostly identify which committed head range to execute"

This is more complex, but it directly attacks the current tx-size bottleneck.

3. Account-layout templates or registries

Instead of sending the full generic `remaining_accounts` shape repeatedly,
allow reveal to reference:

- a known template id for a payload kind, or
- a market-local registry entry for common account layouts.

That would compress both tx size and on-chain layout parsing.

4. Market-state sharding only if the target grows beyond this

If the real long-term goal becomes much larger than `10-20 TPS / market`, then
the true book/event-queue account locks will eventually matter and the protocol
will need deeper restructuring.

That is a much larger redesign:

- sharded books,
- multiple event queues,
- lane-partitioned matching state,
- or a different matching/execution architecture.

It is probably overkill for the immediate target, but it is the long-term upper
bound discussion.

### What Is Likely Enough, And What Probably Is Not

Likely not enough on its own:

- ALT only
- more aggressive raw RPC pipelining
- minor fee tuning
- higher fixed pipeline depth

Useful, but still probably insufficient alone:

- ALT plus dynamic packing
- ALT plus same-user sig merging
- better leader-aware ordered send
- fast-path instructions without auth-model changes

Most plausible paths to the `10-20 TPS / market` target:

- session/delegate auth plus ALT plus microbatching,
- trusted-relayer reveal mode plus ALT plus microbatching,
- reveal-by-reference plus ordered batch execution,
- batch-common account compression combined with one of the auth-model changes
  above.

### Practical Recommendation

If the goal is to reach `10-20 TPS / market` without wasting time on dead-end
optimizations, I would do the work in this order:

1. Rebuild reveal ALT management so v5 can leave `batch_size=1`.
2. Replace fixed-size batching with a real packer based on actual tx bytes and
   CU.
3. Add an ordered submission path for reveal, ideally leader-aware or bundled,
   rather than relying on generic RPC pipelining.
4. Implement same-user ed25519 merge and measure the real incremental gain.
5. Decide the authorization model explicitly:
   strict per-order user sig,
   session/delegate auth,
   or trusted-relayer reveal.
6. If strict per-order user sig remains mandatory, add batch-common account
   compression and fast-path instructions.
7. If `10-20 TPS / market` is still the hard target, expect either
   session/delegate auth, trusted-relayer reveal, or reveal-by-reference to be
   required.

### Bottom Line For Reveal Throughput

The current reveal executor is not failing because we have not "tuned it hard
enough." It is hitting real structural limits:

- per-market head serialization,
- per-item user-signature bytes,
- repeated account-metadata bytes,
- and unordered landing when we try to pipeline around those limits.

So the right conclusion is:

- do the ALT, packer, and ordered-send work immediately,
- but do not expect those alone to comfortably deliver `10-20 TPS / market`.

If that throughput target is real, we should plan now for at least one
structural change to the reveal model, most likely:

- a different auth model,
- a different reveal-data transport model,
- or both.
