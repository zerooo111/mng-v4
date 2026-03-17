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

## Current queue execution observations

- Queue lifecycle, pause flags, and liquidity delay gating are now covered in `test-bpf`.
- Liquidity dispatch failures with bad remaining accounts do retry through the queue path rather than immediately aborting the instruction.
- The retry/drop behavior is sensitive to execute eligibility timing; when a test appears to "miss" the max-retry drop, check `min_execute_slot` first before assuming an onchain bug.

## Tooling footguns

- Keep `--manifest-path` before the test binary separator `--` in `cargo test` commands. If it appears after `--`, it is forwarded to the test binary and fails with `Unrecognized option: 'manifest-path'`.
- When targeting one `test_all` case, use the substring filter in the cargo command rather than `--exact` unless the full rust test path is known. The exact filter can silently run `0 tests`.

## Recommended commands

```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf test_execution_queue --manifest-path /home/ec2-user/stagin4/mng-v4/Cargo.toml
```

```bash
cargo +1.70.0 test -p mango-v4 --test test_all --features enable-gpl,test-bpf test_execution_queue_execute_retries_and_drops_failed_liquidity --manifest-path /home/ec2-user/stagin4/mng-v4/Cargo.toml -- --nocapture --test-threads=1
```
