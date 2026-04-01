# Testing Notes

## Queue `test-bpf` notes

- Use `SolanaCookie::get_account_boxed::<ExecutionQueue>()` for queue reads in tests. `ExecutionQueue` is large enough to blow the stack if loaded by value.
- For queue-focused `test-bpf` cases, prefer a minimal `TestContextBuilder` setup over the default `TestContext::new()`. The default path pulls in placeholder Serum/OpenBook programs and creates unrelated runtime failures when the test only needs Mango + token + oracle setup.
- `ExecutionQueueResize` runs many times before the queue reaches full account space. This is expected in program-test and makes logs noisy.
- Liquidity retry tests must advance to the queued item's actual `min_execute_slot`, not assume a fixed `+2 slots` step. `rotate_liquidity_head_with_retry()` rewrites `min_execute_slot` from the current execution slot, and program-test slot progression does not always line up with naive fixed-step assumptions.
- The reliable pattern for retry/execute tests is:
  - read the current queue head
  - compare `clock.slot` to `head.min_execute_slot`
  - advance exactly `head.min_execute_slot - clock.slot` slots if needed
  - only then send `ExecutionQueueExecute`
- For CTM enqueue tests, do not duplicate a fixed mutable account like `group` inside `remaining_accounts` unless you intentionally account for merged runtime flags. The onchain `accounts_hash` is derived from `AccountInfo` flags as seen at runtime, and duplicate keys can become writable because of the fixed account set, causing surprising `ExecutionQueueAccountsHashMismatch` failures.
- If a CTM success-path test reuses the real `group` pubkey in `remaining_accounts`, hash it with the effective runtime mutability, not the nominal client-side readonly meta. `ExecutionQueueEnqueueCtm` and `ExecutionQueueExecute` both take `group` as a fixed mutable account, so a duplicate `group` in the dispatch lane must be treated as writable for the queue `accounts_hash`.

## Current queue execution observations

- Queue lifecycle, pause flags, and liquidity delay gating are now covered in `test-bpf`.
- Liquidity dispatch failures with bad remaining accounts do retry through the queue path rather than immediately aborting the instruction.
- The retry/drop behavior is sensitive to execute eligibility timing; when a test appears to "miss" the max-retry drop, check `min_execute_slot` first before assuming an onchain bug.
- Signed CTM enqueue is now covered in `test-bpf` with real ed25519 preinstructions for both the CTM signer and user-intent signer.
- Wrong-lane CTM execute currently leaves the queued head untouched, which is the desired safety behavior for lane/account-hash mismatch.
- A matched-lane CTM execute that fails dispatch clears the item and now normalizes `next_sequence_to_execute` past the resolved CTM range when the ring becomes empty. Empty queues should not retain a stale skipped-gap head.
- A matched-lane CTM execute can now be exercised successfully against a real perp market via queued `PerpCancelAllOrders`. That closes the gap between queue mechanics and a real onchain dispatch path.
- A matched-lane queued `PerpPlaceOrderV2` that fails health still does not get auto-cleared by the queue. The queue now scopes health regions per item, but `HealthMustBePositiveOrIncrease` is checked only after the order mutates the book. For safety, `ExecutionQueueExecute` must still bubble the error so the transaction rolls back, the book stays unchanged, and the CTM head remains pending.
- `execution_queue_execute_multi` has the same effective behavior for an undercollateralized queued place: it bubbles `HealthMustBePositiveOrIncrease` and leaves the CTM head pending. Multi-lane execution is not currently a self-healing path for bad health-gated heads.
- `pause_execute` is currently the only explicit onchain stop-the-bleed control for a sticky bad CTM head. It prevents further execute attempts, but it does not clear or mutate the head.
- There is now an explicit admin recovery path for sticky CTM heads: `execution_queue_drop_ctm(sequence)`. It is intentionally pause-gated; operators must set `pause_execute=true` first, then drop the exact pending sequence they want to clear.
- CTM signer rotation is deferred and slot-gated. `execution_queue_set_ctm_pending` leaves the old signer valid until a later queue touch at or after `activate_at_slot`, then swaps in the pending signer atomically.

## Current production-relevant queue caveats

- Bad queued place-order heads are currently retryable/sticky, not self-pruning. This is a deliberate safety tradeoff: pruning after a post-order health failure would be incorrect because the order mutation has already happened by the time the health check runs. A cranker that keeps retrying the same undercollateralized head can burn cycles indefinitely unless there is upstream admission control, operator intervention, or an explicit queue-drop path.
- The intended operator sequence for a sticky CTM head is now: pause execute, inspect/confirm the bad pending sequence, call `execution_queue_drop_ctm(sequence)`, then resume execute. Do not drop while execute is live; the program rejects that path on purpose.
- This sticky-head caveat applies to both single-lane execute and `execute_multi`. Do not assume the multi-lane cranker path will clear a bad head automatically.
- Because signer rotation activates lazily on the next queue interaction, tests should always perform a real queue touch after advancing slots before asserting that the new signer is live.
- The queue failure behavior is instruction-dependent:
  - queued `PerpCancelAllOrders` dispatch failure clears the head
  - queued undercollateralized `PerpPlaceOrderV2` bubbles the health error and keeps the head
  Future tests and cranker logic should not assume one uniform failure policy across CTM variants.
- The per-item health-region change is still useful. It avoids deferring health-region closure across unrelated queue items, but it does not make post-order health failures safely prunable.

## Replay / Harness notes

- The harness optimistic view correctly removes queued place orders once it ingests a `QueueItemProcessed` event with `status=Failed`. Until such an event exists, a sticky failed head will still look optimistic if the relay accepted it.
- There is no explicit harness event for `pause_execute`. Operational pause state has to come from external admin/config telemetry, not queue replay alone.

## Tooling footguns

- Keep `--manifest-path` before the test binary separator `--` in `cargo test` commands. If it appears after `--`, it is forwarded to the test binary and fails with `Unrecognized option: 'manifest-path'`.
- When targeting one `test_all` case, use the substring filter in the cargo command rather than `--exact` unless the full rust test path is known. The exact filter can silently run `0 tests`.
- `local-perp-e2e-bootstrap.ts` needs an explicit `process.exit(0)` on success. Without it, open RPC handles can keep the Node process alive after the config has already been written, which makes `startup_all_local.sh restart` look hung even though bootstrap succeeded.
- `inspect-queue-head.ts` was previously a footgun on devnet: it ignored the config path argument and fell back to a hardcoded queue PDA. Always verify it is reading the intended `executionQueue` from the runtime config, especially when switching groups.
- Keep the root launcher and the repo-local launcher in sync for funding behavior. `startup_all_local.sh` had drifted back to `HARNESS_ENABLE_AIRDROP=true` on devnet even after the repo-local launcher was fixed. That can silently re-enable test funding paths if only one launcher is patched.

## Devnet validation notes

- Fresh devnet bootstrap on Helius is slow in the repeated `idsSource: 'get-program-accounts'` phase, but it does complete. Treat that as a slow path, not an immediate stuck-state, unless the config file never gets written.
- On the upgraded devnet program `7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37`, a fresh `9124` group passed the clean relayer E2E after fixing the event-consumption race:
  - maker queue sequence `0`
  - taker queue sequence `1`
  - cancel sequence `2`
  - final positions `makerBaseLots=10000`, `takerBaseLots=-10000`
- The relayer and onchain execute path were not the cause of the earlier devnet E2E failure. The failure was in the test script: queued orders had executed and one side of the match was resting correctly, but positions were asserted before the fill had been consumed into account state.
- For devnet E2E, consume perp events with a funded admin client and wait until positions become visible before asserting. Using the thinly funded maker wallet as the consumer makes the test more brittle and obscures whether the actual failure is in matching or only in post-trade observation.
- On the upgraded devnet program, a fresh `9125` group passed the dedicated PnL path end to end:
  - relayer mode was used for both trade submissions
  - after trade: maker `+5.0950000045 USDC` unsettled PnL, taker `-5.1900000090 USDC`
  - `perpSettlePnl` succeeded with signature `5tGqGwxif7UJNNntjJScveQnNzuhSYN5tjHkXWxPspWMMcJREzVh7apVqonfF6WUhgL6nvft3fy3B8qpyr5moqRd`
  - after settle: maker unsettled PnL `0`, maker USDC `10005.095000004512`
- Fresh devnet group creation currently costs about `5.1-5.2 SOL` from the deployer/admin wallet. Plan clean-group validation passes deliberately; each new group is not cheap.
- `provision-quoter-bots.ts` had the same payer bug as the earlier bootstrap flow: creating a Mango account through the bot client fails on devnet because the bot wallet is intentionally thin. The safe pattern is owner-client for reads and post-create use, funded admin client for `accountCreate`.
- After that fix, a bounded low-rate devnet quoter smoke on fresh group `9125` completed cleanly:
  - `2` bots
  - `6` ticks
  - `5s` interval
  - `tickErrorCount=0`
  - final `queueCount=0`
  - final queue header `next=25`, `max=25`, `gapSpan=0`
- Even under that light load, the executor still showed many `head_missing` observations on devnet while ending cleanly. Treat low-pressure success and high-throughput readiness as separate claims; the former is now validated, the latter is not.

## Recommended commands

```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf test_execution_queue --manifest-path /home/ec2-user/stagin4/mng-v4/Cargo.toml
```

```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf test_execution_queue_execute_retries_and_drops_failed_liquidity --manifest-path /home/ec2-user/stagin4/mng-v4/Cargo.toml -- --nocapture --test-threads=1
```
