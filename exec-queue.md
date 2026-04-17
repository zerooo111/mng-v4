# Execution Queue Status

This file tracks implementation progress against [exec-queue-amq.md](./exec-queue-amq.md).

## Phase 1 Status

### Landed

- Added the v3 paged queue state model on-chain:
  - `ExecutionQueueAuthorityState`
  - `PerpMarketQueueRootV3`
  - `LiquidityQueueRootV3`
  - `ExecutionQueuePageV3`
  - enriched `QueueItemV3`
- Added v3 on-chain instruction/account plumbing for:
  - authority/root/page init
  - market and liquidity execute paths
  - market direct submit
  - market multi-lane execute
  - admin market drop
  - page close
- Preserved the current perp intent-v2 ingress contract:
  - `intent_version = 2`
  - `target_kind = PerpMarket`
  - `target_index = request.market`
  - relayer canonical remaining-account derivation
  - current user-intent-v2 signature semantics
- Added relayer feature gating via `EXECUTION_QUEUE_TOPOLOGY=v2|v3|v3-market|v3_perp_market`.
- Wired the relayer v3 market path for:
  - enqueue with auto page initialization
  - direct submit
  - single-lane execute
  - multi-lane execute
  - gap-skip execute
  - v3 queue-head inspection from root/page accounts
  - v3 admin pause/drop recovery
- Added TS client support for:
  - v3 PDA derivation helpers
  - v3 root decoding
  - v3 enqueue/direct/execute builders
  - `MangoClient` v3 market helpers
- Tightened page lifecycle behavior:
  - page slot reuse only after the previous absolute page is drained
  - page existence is re-probed by the relayer instead of treated as permanently cached
  - empty head pages cannot be closed while still serving as the current head for a live queue

### Verified

- `cargo check -p mango-v4 --features enable-gpl` passes.
- `cargo check -p service-mango-execution-engine` passes.
- Rust tests are still blocked by pre-existing unrelated packed-struct `E0793` failures elsewhere in the repo.
- TS typecheck was not completed in this environment because repo JS dependencies are not installed locally.

### Remaining Phase 1 Follow-Ups

- Add relayer/client surfacing for the v3 liquidity queue if liquidity is part of the immediate rollout.
- Add focused integration tests around:
  - page initialization and reuse
  - v3 direct submit
  - v3 multi-lane execute
  - v3 admin drop / gap recovery
- Add rollout/bootstrap tooling for:
  - queue root creation
  - page precreation or lazy creation policy
  - queue migration from v2 to v3
- Add any queue-inspection/client decoders still needed for ops dashboards or harness tooling.

## Phase 2 Next

Phase 2 should be delivered in three increments.

### Phase 2A: Maker Band Execution Without Compaction

Land the new maker-band operation first, but execute every item normally.

- Add `MakerReplaceBandV1` payload and variant wiring across:
  - on-chain Rust
  - relayer/executor Rust
  - TS client builders
- Add `MakerBandState` PDA and registration/configuration flow.
- Implement full-state band execution semantics:
  - cancel live slots not present in desired state
  - cancel slots whose tuple changed
  - place desired slots not already live with the exact tuple
  - deterministic `client_order_id`
  - `quote_epoch`-based forced repost even when economics are unchanged
- Keep compaction disabled in this increment.

### Phase 2B: Supersession Metadata And Hints

Once `MakerReplaceBandV1` executes correctly, add the metadata required for future compaction.

- Set `op_class = MakerReplaceBand`.
- Set `SUPERSEDABLE` and `FIXED_BAND_BUDGET` flags.
- Derive and persist:
  - `compact_key`
  - maker-band `account_recipe`
  - maker/band identity hints
- On enqueue, scan backward up to `max_compaction_distance` and set `superseded_by` for the nearest earlier pending compatible maker-band item.
- Treat `superseded_by` as a hint only; do not compact yet.

### Phase 2C: Head-Time Safe Compaction

Add the actual compaction rule only after the above is stable.

- Extend market execute to accept the additional lookahead page accounts needed to scan across the bounded compaction window.
- Implement `try_compact_head(A)` for head item `A`:
  - verify `B = superseded_by` exists and is still pending
  - verify `B` is the same compactable class and key
  - verify `B.sequence - A.sequence <= max_compaction_distance`
  - reconstruct `Delta(A)` from current `MakerBandState` and `A`
  - scan all intervening items conservatively
- Block compaction on:
  - any same-maker non-band op
  - any cancel / liquidation / admin op touching the same band
  - any potentially matching taker inferred from stored metadata
  - any uncertainty
- If safe:
  - mark `A` as `Compacted`
  - emit an event
  - advance head
  - leave `B` pending for execution at its own sequence

## Phase 2 Test Matrix

Minimum tests needed before Phase 2 should be considered done:

- `MakerReplaceBandV1` full-state replacement preserves desired live slots exactly.
- Deterministic `client_order_id` derivation is stable across relayer restart.
- Supersession link is created only for compatible maker-band items within distance.
- Compaction succeeds for clean supersession with no intervening blockers.
- Compaction is blocked by:
  - matching taker
  - same-maker intervening op
  - cancel/liquidation/admin band touch
  - unknown or insufficient metadata
- Cross-page compaction scans work at the page boundary.
- Compacted head does not mutate the book and does not execute the later item early.
