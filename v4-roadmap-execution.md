# V4 Roadmap — Execution Plan (grounded in current code state)

Written 2026-04-20 against branch `vc7` at
`/home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4`, after the
"v4 commit/reveal pipeline correctness fixes" commit (431bd8247 /
subsequently folded into main) and the "pipelined reveal submission"
commit (08e40986a).

This plan maps the user's 24-item roadmap (P0.0 … P5.2) onto what is
already in the tree and gives a concrete tranche-based order to proceed.

---

## Status at a glance

### Done (or effectively done)
- **P0.1** pipelined reveal at 50 ms — env `V4_REVEAL_PIPELINE_DEPTH`
  (default 6), `v4_pipeline.rs::spawn_reveal_worker`.
- **P0.3** durable WAL before commit signing — `v4_reveal_wal.rs`,
  appended before `build_commit_market_tx`, replayed on startup
  filtered by on-chain `next_sequence_to_execute`, compacted in the
  reveal-worker GC block.
- **P0.2** v0 / `VersionedTransaction` compilation used across the
  relayer (`MessageV0::try_compile`, `VersionedTransaction` at main.rs
  5348, 5490, 5665, 6854, 7667, 9071, 9147, 9218) — *ALT population
  still open*.
- **P4.2** fixed + oracle-pegged books already split (`BookSideOrderTree::{Fixed,OraclePegged}` in
  `state/orderbook/order.rs:84-91`) and merged at iteration time.
- **P5.2 (on-chain)** kill switches
  `paused_ingress` / `paused_execute` in `PerpMarketCommitRootV4`,
  enforced at the commit and reveal admission points.

### Partial / finish-the-job
- **P0.0** invariants informally held in code; no golden-trace + CU
  harness in CI.
- **P1.0** v2 / v3 / v4 still coexist, `V4_ROUTE_ALL` switches the
  cohort.
- **P1.1** `CommitPageV4` has `first_pending_offset` + `live_count`
  (O(1) head advance) but **no bitmap**; expiry/skip bounded by
  `EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE`.
- **P1.2 / P1.3** payload variants exist (`QueuePayloadBody`,
  `UserIntentTargetKind`) but no explicit `op_class` / `recipe_kind` /
  `failure_code` on items, no canonical recipe registry.
- **P1.4** expiry + decode failures terminalize as no-ops; richer
  prechecks + `failure_code` population missing.
- **P1.5** health-region wrapping present; behavior on health failure
  ambiguous — may rollback and pin head.
- **P2.2** `Book::new_order` still `Vec`-allocates change/delete lists
  on the match loop.
- **P2.3** `update_funding_and_stable_price` per-instruction, not
  per-batch (`perp_market.rs:311-384`, called from
  `perp_place_order.rs:48`).
- **P3.0** only health-region bypass; no reduce-only / isolated-single-perp
  shortcuts.
- **P5.0** rate limiting + attribution on `submit_intent` only;
  receipts are `sequence` only.
- **P5.1** `/metrics` exists with ~40 counters; missing per-market
  queue depth, head age, unrevealed-commit age, divergence rate,
  no-op reason breakdown.

### Stubs / enums only
- **P2.4** event-queue preflight (`soft_limit` defined; no preflight /
  backpressure).
- **P3.1** `QueueItemOpClassV3::MakerReplaceBand = 6` in
  `state/execution_queue_v3.rs:46`; no handler.

### Not started
- **P0.0** golden harness, **P0.4** standby, **P2.0** op-class
  `execute_*_run` instructions, **P2.1** compact cancel lanes,
  **P2.5** offchain consume_events planner, **P3.2** MakerBandState,
  **P4.0** Orderbook V2, **P4.1** open-order slot refactor (current
  `PerpOpenOrder` at `mango_account_components.rs:928-950` has no
  generation / node_index / price_level).

---

## Execution order — tranches

### Tranche A — finish the P0 floor (1–2 weeks)

1. **P0.0 golden harness.** `programs/mango-v4/tests/golden/` replays
   fixed commit → reveal → execute traces against a frozen ledger;
   asserts on `QueueItemEnqueued` / `QueueItemProcessed` sequences,
   final head, `accounts_hash` at every reveal. Wire CU measurement
   from `tx.meta.compute_units_consumed` into a CSV artifact per test.
   Safety net for every subsequent change — build it first.
2. **P0.2 ALT populator.** Offchain ALT manager owns one ALT per
   market pre-seeded with
   `{group, authority_state, queue_root, perp_market, bids, asks,
   event_queue, oracle, usdc_bank, usdc_oracle}` plus a rolling ALT
   for hot mango accounts. Pass `Vec<AddressLookupTableAccount>` into
   `MessageV0::try_compile`. Gate with `V4_USE_ALT=1`, keep legacy
   fallback. Tx byte budget roughly doubles for the reveal packer.
3. **P1.5 health failure → terminal no-op.** Trace
   `dispatch_queue_payload` + health-region error paths in
   `execution_queue_v4.rs:780-846`. On health-check failure emit
   `QueueItemProcessed { status: Failed, failure_code: NoOpInsufficientMargin }`
   and `note_commit_executed(seq)` instead of returning Err. Add a
   `failure_code: u8` field and corresponding `MangoError` variants.
   This single change unblocks the hard constraint for Tranches E/F.
4. **P0.4 active/cold standby.** WAL is now the system of record for
   reveal material. Standby tails the WAL over rsync/scp or a tiny
   TCP push channel, mirrors the in-memory store, holds a heartbeat
   lease file, promotes after lease lapse. Primary stays sole commit
   signer; standby refuses to submit commits unless it has held the
   lease for N seconds. Single-writer model — no consensus needed.

### Tranche B — queue unification + self-describing items (2–3 weeks). Requires Tranche A.

5. **P1.0 unified queue abstraction.** `ExecutionQueueUnified` with
   `mode: Payload | CommitReveal`. v4 `PerpMarketCommitRootV4` becomes
   the `CommitReveal` variant; v3's payload path becomes the `Payload`
   variant. On-chain PDAs stay separate during migration; offchain
   codepath merges. Deprecate v2 cohort after v4 runs clean for 2 weeks.
6. **P1.1 per-page bitmap + O(1) expiry.** `[u8; 32]` pending bitmap
   in `CommitPageV4` header (256 bits for 256-slot page). Expiry/skip
   scans replaced with bit-clear + find-next-set; removes the 64-slot
   cap. Migration: initialize the bitmap from existing items on first
   touch.
7. **P1.2 self-describing payload items.** Add `op_class: u8`,
   `recipe_kind: u8`, `failure_code: u8`, `accounts_hash: [u8;32]`,
   `mango_account_hint: Pubkey`, `expiry_slot: u64` to the
   payload-mode queue item. CommitReveal already has `accounts_hash`
   via the committed hash; add the same fields to the reveal args so
   indexers see them.

### Tranche C — recipes + deterministic admission + hot-path cleanup (3–4 weeks). Requires Tranche B.

8. **P1.3 canonical account-recipe system.** Move hardcoded
   `build_dispatch_accounts` (`v4_pipeline.rs:325-344`) into a
   versioned recipe registry (`recipe_v1::place_order`,
   `recipe_v1::cancel`, `recipe_v1::replace_band`,
   `recipe_v1::direct_fallback`). Program-side verifier computes
   `accounts_hash` from `(recipe_kind, inputs)` and matches against
   committed hash — becomes the security gate for the payload mode too.
9. **P1.4 deterministic prechecks + failure codes.** `prevalidate_deterministic(item)`
   runs decode + bounds + side + market-active + ownership +
   recipe-shape checks before any side effects. Convert all expected
   failures into `QueueItemStatus::Failed` + populated `failure_code`.
   No early-exit Err.
10. **P2.1 compact cancel-only lane.** Once recipes exist, cancel-only
    reveals ship with a 6-account lane
    (`group, mango_account, owner, perp_market, bids, asks`). Pack-reveal
    test with 8+ cancels per tx becomes the regression.
11. **P2.2 hot-path alloc cleanup.** Replace `Vec` in `Book::new_order`
    (`state/orderbook/book.rs:82-83`) match/delete lists with
    `ArrayVec` or `[T; N]` fixed caps tied to a configured maker-depth
    limit. Remove remaining `payload.to_vec()` on the place-order
    path. Measure CU via the P0.0 harness before/after.
12. **P2.3 batch funding.** Cache `update_funding_and_stable_price`
    output at the `execute_run` boundary; pass through as a batch
    context; items within one execute tx reuse it.

### Tranche D — event-queue health + selective fast health (2–3 weeks). Parallel to Tranche C late.

13. **P2.4 event-queue preflight + watermark.** Before matching,
    require `event_queue.free_slots >= worst_case_events(order)`.
    Above high-watermark, reject crossing orders with
    `NoOpEventQueueFull` and promote `consume_events` priority.
14. **P2.5 offchain packed consume_events planner.** New worker reads
    the event prefix, greedily groups by overlapping maker/taker
    account set, emits the largest valid prefix per tx.
15. **P3.0 exact-health fast paths.** Three narrow paths:
    (a) no-health cancels; (b) reduce-only when
    `perp_position.base_lots * side <= 0`; (c) isolated single-perp
    accounts with no token exposure. Each runs the full health calc
    in shadow mode under a feature flag for 1 week before enablement.

### Tranche E — replace-band (gated)

16. **P3.1 MakerReplaceBandV1** + **P3.2 MakerBandState.** Only once
    Tranche A + C (specifically P1.4, P1.5) are live and stable.
    Budget/reservation bookkeeping on `MakerBandState`; one-sequence
    atomic cancel-band + new-quote-set. Canary on a single low-volume
    market under `ENABLE_REPLACE_BAND=1`.

### Tranche F — Orderbook V2 (gated)

17. **P4.0 slab + price-level + generation** and **P4.1 open-order
    slot refactor** (node_index + generation + price_level + side).
    Migration: dual-write for N days, readers consult both, then
    switch. Public `order_id` preserved by keeping the old ID as an
    alias into the new structure. Canary on one market.

### Tranche G — continuous: observability, ingress fairness, rollout gates

- **P5.0 unified ingress.** Promote the execution-engine's
  rate-limit + attribution middleware into a shared crate used by
  SDK/Continuum/direct relayer.
- **P5.1 fill observability gaps.** Per-market queue depth, head age,
  unrevealed-commit age, reveal/execute latency histograms,
  divergence rate. Alerting thresholds in Grafana, not in the binary.
- **P5.2 per-op kill switches.** Env flags `V4_REVEAL_ENABLED`,
  `ENABLE_REPLACE_BAND`, `ENABLE_FAST_HEALTH`, `ENABLE_ORDERBOOK_V2`;
  per-market canary list for the latter three.

---

## Hard gates before Tranches E + F

1. P0.0 golden harness green for 2 weeks.
2. P0.4 standby demonstrated in a kill-primary drill.
3. P1.4 + P1.5 deterministic no-ops observed on live traffic for
   ≥ 100k committed sequences with zero head-pin incidents.
4. P1.1 bitmap-backed expiry in production.

If any gate slips, Tranches E/F wait.

---

## Immediate next step

Start Tranche A item 1 (P0.0 golden harness) in parallel with item 2
(P0.2 ALT populator) — they're independent and both unblock
confidence for everything after. Item 3 (P1.5) is the
smallest-diff/biggest-operational-win of the four and is a natural
third lane.

---

## Critical files to touch (by tranche item)

- **A.1** `programs/mango-v4/tests/golden/*` (new);
  `anchor-tests/*`; CU measurement hook in relayer bench scripts.
- **A.2** `bin/service-mango-execution-engine/src/v4_alt.rs` (new);
  plumb `Vec<AddressLookupTableAccount>` into every
  `MessageV0::try_compile` site in `main.rs` and `v4_pipeline.rs`.
- **A.3** `programs/mango-v4/src/instructions/execution_queue_v4.rs`
  (dispatch error conversion);
  `programs/mango-v4/src/error.rs` (new `NoOp*` variants + `failure_code`
  field on events).
- **A.4** `bin/service-mango-execution-engine/src/v4_reveal_wal.rs`
  (extend to streaming tail);
  `bin/service-mango-execution-engine/src/v4_standby.rs` (new —
  lease + promotion).
- **B.5** `programs/mango-v4/src/state/execution_queue_unified.rs`
  (new); migrate imports from `execution_queue_v3` / `_v4`.
- **B.6** `programs/mango-v4/src/state/execution_queue_v4.rs` (page
  header layout change — bump layout_version).
- **B.7** `programs/mango-v4/src/instructions/mod.rs` (reveal args
  surface change).
- **C.8** `programs/mango-v4/src/state/account_recipes/*` (new).
- **C.9** `programs/mango-v4/src/instructions/execution_queue_v4.rs`
  (prevalidation loop).
- **C.10** `bin/service-mango-execution-engine/src/v4_reveal_packer.rs`
  (recipe-aware packing — already dedupe-free per fix 1).
- **C.11** `programs/mango-v4/src/state/orderbook/book.rs:82-83`
  (`ArrayVec`).
- **C.12** `programs/mango-v4/src/state/perp_market.rs:311-384`
  (batch context lifecycle).
- **D.13** `programs/mango-v4/src/instructions/perp_place_order*.rs`
  (event-queue preflight).
- **D.14** `bin/service-mango-execution-engine/src/consume_events_planner.rs`
  (new).
- **D.15** `programs/mango-v4/src/state/health/*` (feature-flagged
  fast paths).
- **E.16** `programs/mango-v4/src/state/maker_band.rs` (new);
  `programs/mango-v4/src/instructions/maker_replace_band.rs` (new).
- **F.17** `programs/mango-v4/src/state/orderbook/v2/*` (new);
  `programs/mango-v4/src/state/mango_account_components.rs:928-950`
  (slot layout change).
- **G** continuous updates to
  `bin/service-mango-execution-engine/src/main.rs` (metrics),
  shared crate under `lib/ingress_common/*` (new).
