# TODO

## Program-side: self-heal gap-tail wedge (next follow-up to 2026-04-21 incident)

**Status:** deferred — requires on-chain program redeploy.
**Tracked incident:** 2026-04-21 wedge where markets 2, 3, 4 all had
`live_count=0 && max_seen_sequence > next_sequence_to_execute` and no
on-chain path could advance the head. Only the relayer-side mitigation
(drift-resync formula fix + ingress admission guard, landed same day) is
live today. The on-chain program still has the underlying dead-end.

### Root cause (on-chain)

`programs/mango-v4/src/state/execution_queue_v5.rs`:
- `clear_head` requires `live_count > 0` (line 355). `drop_head_market`
  (admin autodrop) can therefore not clear gap-tail slots.
- `skip_sub_queue_head_gaps_v5` in
  `programs/mango-v4/src/instructions/execution_queue_v5.rs:425` requires
  BOTH `live_count > 0` AND `max_seen >= next_seq`. When `live=0` the
  skip loop never fires, so the reveal-execute crank can't advance the
  head either.

Net: once a sub-queue reaches `live=0 && max_seen > next_seq` (happens
when all live commits have been revealed or terminal-failed but max_seen
was pushed ahead by later commits that subsequently failed to land
their reveal), the head is structurally unreachable without admin
intervention. Ingress keeps allocating into the admission window but the
window never moves.

### Fix

**Option A (preferred — self-healing).** Relax the gate on
`skip_sub_queue_head_gaps_v5` so it can advance over gaps when `live=0 &&
max_seen >= next_seq`:

```rust
fn skip_sub_queue_head_gaps_v5(...) {
    ...
    loop {
        let sqh = &queue.sub_queue_headers[idx];
        if skipped >= EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE { break; }
        // OLD: if sqh.live_count == 0 || sqh.max_seen_sequence < sqh.next_sequence_to_execute
        // NEW:
        if sqh.max_seen_sequence < sqh.next_sequence_to_execute { break; }
        let next_seq = sqh.next_sequence_to_execute;
        if queue.get_item(idx, next_seq).is_some() { break; }
        emit!(QueueItemProcessed {
            group, market_index, sequence: next_seq,
            kind: QueueItemKind::CtmWrapped as u8,
            status: CommitStatusV5::Failed as u8,
            failure_code: QueueFailureCode::GapSkipped as u8,
        });
        queue.sub_queue_headers[idx].next_sequence_to_execute =
            next_seq.saturating_add(1);
        queue.sub_queue_headers[idx].gap_observed_slot = 0;
        skipped = skipped.saturating_add(1);
    }
}
```

Combined with a no-op `reveal_execute_market` call periodically (the
crank already hits this ix), the chain self-heals within
`EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE` seqs per call.

**Option B (admin-only).** New admin ix
`execution_queue_v5_advance_head_to_max_seen(market_index)`:
- `require!(live_count == 0)`
- `require!(max_seen_seq >= next_seq_to_execute)`
- For each ring slot in `[next_seq_to_execute, max_seen]`, assert
  `status != Committed` (sanity check — slot must have been Revealed,
  Failed, or cleared Empty).
- Bulk advance `next_seq_to_execute = max_seen + 1`, zero affected slots
  to `Empty`, emit `HeadGapAdvanced { from, to }`.
- Wire into the relayer autodrop worker as a second trigger alongside
  `drop_head_market` for the `is_gap_stall` case.

Option A is less code and leaves no admin-only code path, which is
better for production self-healing. Option B is easier to reason about
under attack (admin-gated, explicit log line per advance) but leaves
stalls visible until someone wakes up.

### Autodrop trigger extension (relayer-side, ships with on-chain fix)

`bin/service-mango-execution-engine/src/v5_pipeline.rs:1447` — replace
the `live_count == 0 → continue` early-exit with the combined trigger:

```rust
let is_live_stall = live_count > 0 && age.as_secs() >= cfg.stall_threshold_secs;
let is_gap_stall  = live_count == 0 && max_seen > next_seq;
if !(is_live_stall || is_gap_stall) { continue; }
```

For `is_gap_stall`, send the Option-B admin ix (or a throwaway
reveal-execute call when Option A is deployed).

### Acceptance tests

1. Reproduce the wedge on devnet: commit N items, revert all their
   reveals (force-exit relayer between commit and reveal), restart.
   Observe `live=0 && max_seen > next_seq`. With the fix, crank drains
   head without admin intervention within ≤ `GAP_SKIP_LIMIT` seqs.
2. Trace coverage: every gap-skip emits a `QueueItemProcessed` event
   with `status=Failed, failure_code=GapSkipped`. Trace endpoint
   returns "skipped at slot X" for any seq in the advanced range (per
   CLAUDE.md "no silent drops" rule).
