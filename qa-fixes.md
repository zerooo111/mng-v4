# QA Fixes Log

## 2026-03-04

### 1) Local startup false-negative on harness readiness
- Symptom:
  - `./startup_local.sh start` failed with `Port 9091 did not start listening` while the harness process became healthy shortly afterward.
- Root cause:
  - Startup readiness waits were hardcoded to 60 tries/seconds, which is too short for cold local boots in this environment.
- Fix:
  - Added configurable `STARTUP_WAIT_TRIES` (default `180`) and applied it to RPC/HTTP/port/harness readiness checks in `startup_local.sh`.
- Validation:
  - Re-tested with longer startup windows; the harness no longer trips the prior 60s false-negative path.

### 2) Cranker could not drain queue head items due accounts-hash lane mismatch
- Symptom:
  - `execution-queue-local-perp-e2e-run` failed with `timed out waiting for execution queue to drain`.
  - Cranker submitted `execution_queue_execute` transactions, but queue `count` stayed non-zero.
- Root cause:
  - Lane account metas were used without runtime flag normalization.
  - CTM enqueue hashing merges runtime flags for duplicated fixed accounts (`group`, `execution_queue`, `sysvar instructions`), but cranker lanes were hashed/evaluated with raw flags from config/events.
  - Result: lane pubkeys looked correct but `accounts_hash` never matched queue head items, so execute calls returned success without processing.
- Fix:
  - Updated `ts/client/scripts/execution-queue/execution-queue-cranker.ts` to:
    - normalize lane runtime flags (same effective behavior as enqueue hashing),
    - auto-discover dynamic lanes from relay-intent event logs,
    - dedupe lanes by normalized hash,
    - add stall detection and lane refresh logging.
- Validation:
  - Reproduced stuck queue state (`count: 2`, E2E timeout).
  - After patch, cranker refreshed lanes and drained queue (`count: 0`, `next_seq: 6`).
  - `execution-queue-local-perp-e2e-run` then completed successfully (`status: ok`).

### 3) Stale relay/harness runtime artifacts after validator reset
- Symptom:
  - After validator resets, harness replay and relay sequence state could carry stale data into fresh runs, creating misleading pending-state/diff noise and lane pollution.
- Root cause:
  - `startup_local.sh` reset path removed ledger but retained runtime artifacts (`ctm sequence state`, `harness event log`).
- Fix:
  - Added `reset_runtime_artifacts_if_requested` in `startup_local.sh`.
  - On `RESET_VALIDATOR=1`, it now removes stale relay sequence and harness event-log files before bootstrap/startup.
  - Also wired startup cranker env to pass harness event log path for dynamic lane recovery.
- Validation:
  - Post-fix runs produce clean on-chain queue behavior with no stale queue carryover from prior reset cycles.

### 4) Quoter throughput guard prevented requested 5-bot ~10 TPS run
- Symptom:
  - 5-bot stress run failed immediately with:
    - `Error: QUOTER_INTERVAL_MS must be >= 1000`
  - With one place intent per bot per tick, this hard floor caps throughput below the requested target when using 5 bots.
- Root cause:
  - `random-sol-usdc-quoter-bot.ts` enforced a fixed minimum interval of `1000ms` instead of a configurable lower bound for stress scenarios.
- Fix:
  - Added `QUOTER_MIN_INTERVAL_MS` (default `100`) and changed validation to:
    - reject invalid/non-positive min interval values,
    - require `QUOTER_INTERVAL_MS >= QUOTER_MIN_INTERVAL_MS`.
- Validation:
  - Script now accepts `QUOTER_INTERVAL_MS=500`, enabling the intended 5-bot load profile toward ~10 place TPS.

### 5) Quoter startup/tick overhead throttled load generation
- Symptom:
  - 5-bot quoter spent significant time in startup hydration and per-tick reloads, with very low effective place throughput.
- Root cause:
  - Repeated expensive calls (`group.reloadAll`, `getMangoAccount`) on hot path.
  - Per-order logging overhead in stress mode.
  - Coingecko queried every tick, causing frequent `429` fallback churn.
- Fix:
  - Updated `random-sol-usdc-quoter-bot.ts` to:
    - cache shared group and per-bot mango accounts,
    - add optional periodic reload controls (`QUOTER_GROUP_RELOAD_EVERY_TICKS`, `QUOTER_ACCOUNT_RELOAD_EVERY_TICKS`),
    - add optional per-order log control (`QUOTER_LOG_EACH_ORDER`),
    - add Coingecko refresh cache (`QUOTER_COINGECKO_REFRESH_MS`) and cached-price fallback.

### 6) Quoter pipelined dispatch mode for stress testing
- Symptom:
  - Tick loop awaited all bot submissions before scheduling next tick, limiting dispatch rate under relay latency.
- Fix:
  - Added non-blocking dispatch mode in `random-sol-usdc-quoter-bot.ts`:
    - `QUOTER_NONBLOCKING_SUBMIT` (default false),
    - bounded concurrency via `QUOTER_MAX_INFLIGHT`,
    - in-flight visibility in `quoter-stats`.

### 7) Relayer instability under stress from websocket/confirmation path
- Symptom:
  - Relayer repeatedly crashed during stress windows with:
    - `Unexpected server response: 405`
    - gRPC `UNAVAILABLE` / connection drops seen by quoter.
- Root cause:
  - Background confirmation path could surface websocket handshake errors and terminate process.
  - Local validator WS endpoint mismatch handling under load.
- Fix:
  - `ctm-sequencer-relayer.ts`:
    - derived/explicit WS endpoint support (`CLUSTER_WS_URL_OVERRIDE` or derived `ws://127.0.0.1:8900` for localnet),
    - `CTM_RELAYER_CONFIRM_IN_BACKGROUND` support,
    - guarded known websocket 405 uncaught/unhandled paths to prevent relayer process death during stress.
  - `ts/client/src/utils/rpc.ts`:
    - background confirmation now catches/logs failures instead of allowing unhandled rejection flow.

### 8) Relayer synchronous bottlenecks reduced for stress mode
- Symptom:
  - `submitIntent` latency was inflated by synchronous sink emission and per-intent blockhash fetches.
- Fix:
  - `ctm-sequencer-relayer.ts`:
    - made relay-intent sink emission async fire-and-forget,
    - added blockhash cache (`CTM_RELAYER_BLOCKHASH_CACHE_MS`) and reuse in send path.

### 9) Current observed ceiling after fixes (documented)
- Observation:
  - With 5 bots, ±2% pricing, and optimized offchain settings, sustained accepted throughput improved materially but remained below 10 TPS in this environment.
  - Best measured `relay_intent_accepted` window in this round: ~`5.31 TPS` (pipelined mode, `500ms`, `max_inflight=100`).
- Additional findings:
  - At higher aggression, failures included `ExecutionQueueFull` (`Custom 6076`) and cranker execution failures for one dynamic lane (`Custom 6000`), indicating remaining onchain/offchain throughput bottlenecks beyond the above fixes.

### 10) Cranker lane execution efficiency improvements
- Symptom:
  - Cranker loop executed all lanes sequentially; repeated failing lanes consumed cycles and reduced effective queue drain under load.
- Fix:
  - Updated `execution-queue-cranker.ts` to:
    - support parallel lane execution (`EXECUTION_QUEUE_CRANK_PARALLEL_LANES`, default true),
    - track repeated lane failures and apply temporary backoff for recurring `Custom 6000` failures
      (`EXECUTION_QUEUE_CRANK_LANE_FAILURE_THRESHOLD`, `EXECUTION_QUEUE_CRANK_LANE_FAILURE_BACKOFF_MS`).
- Validation:
  - Cranker remained stable under sustained load and no longer blocked on a single repeatedly failing lane path.
  - End-to-end throughput remained limited by queue/fullness behavior (`Custom 6076`) rather than process liveness.

### 11) Quoter TypeScript nullability compile break in Coingecko cache fast-path
- Symptom:
  - `npm run -s execution-queue-random-sol-usdc-quoter` failed at startup with:
    - `TS2322: Type 'number | null' is not assignable to type 'number'`
  - Failure occurred on the non-refresh path assigning `cachedCoinGeckoPriceUi` to `referencePrice`.
- Root cause:
  - Control flow did not guarantee non-null `cachedCoinGeckoPriceUi` in the `else` branch from TypeScript’s perspective.
- Fix:
  - Updated `random-sol-usdc-quoter-bot.ts` to explicitly handle `cachedCoinGeckoPriceUi === null` by falling back to on-chain reference pricing and marking source as `onchain-fallback`.
- Validation:
  - Quoter now compiles and starts under `ts-node` with the same stress env configuration.

### 12) Cranker head-item mismatch due fixed-account flag folding in lane hash normalization
- Symptom:
  - `execution_queue_execute` transactions were landing `success`, but queue head (`next_sequence_to_execute`) did not move.
  - Pending head `accounts_hash` values did not match static lane hashes.
- Root cause:
  - Cranker lane normalization was OR-merging flags using fixed instruction accounts (`group`, `execution_queue`, `sysvar`), but on-chain `provided_accounts_hash` is computed from dispatch `remaining_accounts` only.
  - This produced lane hashes that could never match head items in many cases.
- Fix:
  - Updated `ts/client/scripts/execution-queue/execution-queue-cranker.ts`:
    - keep duplicate-flag OR-merge only within lane `remainingAccounts`,
    - do not inject fixed account flags into lane hash normalization.
- Validation:
  - After restart, lane hashes aligned with queued items when dynamic lanes were enabled, and queue head began advancing.

### 13) Cranker not draining stale backlog with static lanes only
- Symptom:
  - Queue remained blocked on historical hashes not present in static lane config.
- Root cause:
  - Static lane JSON covered only limited maker/taker paths; backlog contained additional lane hashes from prior multi-bot relayer submissions.
- Fix:
  - Enabled dynamic lane ingestion in cranker runtime using:
    - `EXECUTION_QUEUE_CRANK_RELAY_EVENT_LOG_PATH=.localnet/run/continuum-harness-9120.jsonl`
    - periodic refresh (`EXECUTION_QUEUE_CRANK_DYNAMIC_LANES_REFRESH_MS`).
- Validation:
  - Cranker logged lane-set expansion and continuously emitted execute txs on dynamic lanes.
  - Queue state moved from `next=65,count=137` to `next=76,count=126` during verification.

### 14) Relayer false-positive accept semantics under background confirmation
- Symptom:
  - Harness `relay_intent_accepted` count increased while sampled `enqueue_tx_signature` values were absent from chain history.
- Root cause:
  - Relayer emitted accept events immediately after raw send signature without guaranteeing chain visibility in the current runtime mode.
- Fix:
  - Updated `ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts`:
    - submit via fast send path,
    - then poll `getSignatureStatuses` until processed (or explicit timeout) before returning success/emit.
    - added env controls:
      - `CTM_RELAYER_POST_SEND_STATUS_TIMEOUT_MS`
      - `CTM_RELAYER_POST_SEND_STATUS_POLL_MS`
- Validation:
  - Relayer now surfaces explicit gRPC errors when submitted signatures are not observed on-chain within timeout, preventing silent false-positive accepts.

### 15) Relayer localnet program-id mismatch can silently stall submit path
- Symptom:
  - Relayer gRPC calls timed out and/or produced missing signatures.
  - With preflight enabled, simulation showed:
    - `Attempt to load a program that does not exist`.
- Root cause:
  - Relayer runtime without `CTM_RELAYER_PROGRAM_ID` invoked cluster-default Mango program id, which was not deployed in this localnet run.
- Fix:
  - Enforced relayer startup with explicit:
    - `CTM_RELAYER_PROGRAM_ID=9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF`
  - Verified via successful single-intent relayer acceptance when paired with the correct user key.

### 16) Hidden simulation errors masked by global skip-preflight send path
- Symptom:
  - Raw signatures were produced but not observable in chain status during submit debugging.
- Root cause:
  - `sendRawTransaction(..., skipPreflight: true)` masked deterministic simulation failures in relayer path.
- Fix:
  - Updated `ts/client/src/utils/rpc.ts`:
    - `skipPreflight: false`
    - `maxRetries: 20`
  - This surfaced actionable errors directly in relayer gRPC responses (e.g., missing user signature preinstruction, missing program).

### 17) Probe tooling correctness: owner keypair must match mango account owner
- Symptom:
  - Direct relayer probe initially failed with:
    - `ExecutionQueueUserSignatureMissing (6093)`.
- Root cause:
  - Probe script used default payer keypair while targeting a bot-owned mango account.
- Fix:
  - For probe validations, set:
    - `USER_KEYPAIR_OVERRIDE=.localnet/run/quoter-bot-0.json`
    - matching `MANGO_ACCOUNT_PK` for the same owner.

### 18) Quoter nonblocking backpressure allowed in-flight overshoot and process instability
- Symptom:
  - Under deadline-heavy relayer conditions, quoter `inFlight` exceeded configured cap and bot process was repeatedly killed during stress windows.
- Root cause:
  - Nonblocking path checked `MAX_INFLIGHT` once per tick, then launched all bots, allowing overshoot by up to `bots.length` every cycle.
- Fix:
  - Updated `random-sol-usdc-quoter-bot.ts` to enforce backpressure before each nonblocking submission:
    - `while (inFlight.size >= MAX_INFLIGHT) await Promise.race(inFlight);`
- Validation:
  - Cap enforcement is now strict per submitted bot tick, preventing runaway in-flight growth.

### 19) Queue head unblocking required owner-specific slot recovery; `perpCancelAllOrders(limit=10)` was insufficient under slot saturation
- Symptom:
  - Cranker stalled on head seq `162` (`accounts_hash=d5bc...`) with repeated `Custom 6000` (`no free perp order index`) for owner `7cqW...`.
  - Queue stayed pinned (`count=281`, `next=162`) even while cranker sent execute txs.
- Root cause:
  - Saturated perp-order slots on the owner account were not being reclaimed by low-limit cancel intents.
  - With low cancel limits, cancellation passes may not effectively reclaim enough slots in pathological saturated states.
- Fix / mitigation:
  - Added direct owner-targeted cancel utility controls:
    - `QUOTER_CANCEL_OWNER`
    - `QUOTER_CANCEL_BOT_NAME`
    - `QUOTER_CANCEL_CONSUME_EVENTS_EACH_ROUND`
  - Executed owner-targeted cancel with high limit:
    - `QUOTER_CANCEL_LIMIT_PER_TX=255`
  - Confirmed owner `7cqW...` recovered to `remaining=0` after high-limit cancel.
- Validation:
  - After owner slot recovery and cranker restart, queue advanced from:
    - `count=281,next=162` -> `count=279,next=164` -> `count=275,next=168`.

### 20) Added cranker head-hash diagnostics for deterministic lane coverage debugging
- Symptom:
  - Needed deterministic proof whether the current head `accounts_hash` existed in active lane set.
- Fix:
  - Updated `execution-queue-cranker.ts` with:
    - `EXECUTION_QUEUE_CRANK_DEBUG_HEAD_HASH`
  - Logs whether that hash is present in merged lane set on refresh and stall.
- Validation:
  - For stuck hash `d5bc...`, cranker logged:
    - `present (dynamic-0-589-7cqWWdjH-legacy)`
  - This allowed direct linkage from on-chain head hash to owner lane and accelerated targeted remediation.

### 21) Stress test bottleneck identified: relayer RPC deadline dominates before bot intent generation target
- Run setup:
  - 5 bots, `QUOTER_INTERVAL_MS=500`, `QUOTER_PRICE_RANGE_BPS=200` (±2%), Coingecko source, nonblocking enabled.
- Result:
  - `START_INTENTS=573 END_INTENTS=624 DELTA=51 ELAPSED=70s -> ~0.729 TPS`.
- Observations:
  - Quoter logs showed sustained gRPC `DEADLINE_EXCEEDED` at ~5s to `127.0.0.1:9090`.
  - In this mode, relayer response latency, not bot scheduling cadence, is the primary throughput limiter.

### 22) Relayer request serialization was default-enabled and created a hard throughput ceiling
- Symptom:
  - Under 5-bot stress, relayer gRPC calls accumulated and timed out (`DEADLINE_EXCEEDED`), with sustained accepted TPS << target.
- Root cause:
  - `CTM_RELAYER_SERIALIZE_SUBMITS` defaulted to `true`, forcing full submit path serialization (including send + status polling) across requests.
- Fix:
  - Updated `ctm-sequencer-relayer.ts` default:
    - `CTM_RELAYER_SERIALIZE_SUBMITS=false` unless explicitly enabled.
- Validation:
  - Relayer startup log now shows `serializeSubmits=false`.
  - Remaining bottleneck still present (deadline-heavy under current strict submit semantics), but serialization is no longer forced by default.

### 23) Stress run reality check after queue-unblock fixes
- Queue stability:
  - Onchain queue advanced after owner-targeted slot recovery (`next: 162 -> 164 -> 168 -> 173`).
- Throughput under strict relayer submit semantics:
  - 5 bots, ±2% price range, `500ms` cadence:
    - run A (`cancel every 4 ticks`): `51 intents / 70s` (~`0.729 TPS`)
    - run B (`place only`, 20s RPC deadline): `48 intents / 70s` (~`0.686 TPS`)
  - After disabling default relayer serialization, sustained deadline pressure remained dominant in this run profile.

### 24) Relayer backpressure + fast-fail admission control implemented
- Change:
  - Added bounded in-flight gate in `ctm-sequencer-relayer.ts`:
    - `CTM_RELAYER_MAX_INFLIGHT` (default `24`)
    - `CTM_RELAYER_MAX_QUEUED` (default `96`)
    - `CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS` (default `500`)
  - Requests now fail fast with `RESOURCE_EXHAUSTED` when saturated or queue wait times out.
- Change:
  - Added submit-mode split in relayer:
    - `CTM_RELAYER_SUBMIT_MODE=strict|fast` (default `strict`)
    - `strict`: preflight on + post-send signature status wait
    - `fast`: preflight off + background confirmation (no post-send wait)
- Validation:
  - Relayer startup now logs submit mode and backpressure params.
  - Under stress, clients receive explicit `RESOURCE_EXHAUSTED` queue-timeout errors instead of long silent stalls.

### 25) Fast-mode stress exposed enqueue failures (`Custom 6083`) and zero accepted throughput in saturated state
- Run profile:
  - 5 bots, 500ms tick, ±2% pricing, relayer `submitMode=fast`, bounded gate enabled.
- Result:
  - `DELTA_INTENTS=0` over both 70s and 35s windows.
- Observations:
  - Quoter saw mixed:
    - `DEADLINE_EXCEEDED` (rpc timeout)
    - `RESOURCE_EXHAUSTED` (`relayer queue timeout ... active=24`)
  - Relayer background confirmations repeatedly logged:
    - `InstructionError[3, Custom 6083]`
    - `6083 = ExecutionQueueInvalidItemKind`
- Implication:
  - Current fast submit path is not sufficient for reliable accept throughput in this saturated queue state; malformed/invalid enqueue items are being submitted (or interpreted) under this load path and must be root-caused before 10 TPS is achievable.

### 26) Additional relayer speed-up under saturation: cached queue watermark rejection
- Change:
  - Added cached execution-queue count precheck in relayer:
    - `CTM_RELAYER_QUEUE_FULL_WATERMARK`
    - `CTM_RELAYER_QUEUE_COUNT_CACHE_MS`
  - If queue count is above watermark, relayer now immediately returns `RESOURCE_EXHAUSTED` without building/signing/sending.
  - Replaced per-request `getSlot()` call with cached latest-blockhash slot for default min-execute-slot derivation.
- Expected effect:
  - Lower relayer request latency under saturated queue conditions and reduced RPC/send load from doomed submits.
- Validation:
  - Relayer starts with watermark/cache params logged.
  - Under high queue occupancy, clients receive immediate backpressure responses instead of waiting on long RPC deadlines.

### 27) Cranker fast-send mode added for stress drain experiments
- Change:
  - Updated `ts/client/scripts/execution-queue/execution-queue-cranker.ts` with:
    - `EXECUTION_QUEUE_CRANK_CONFIRM_IN_BACKGROUND`
    - `EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT`
  - Stress runs can now enqueue execute txs without waiting for synchronous confirmation on every crank iteration.
- Validation:
  - Cranker startup log now reports `confirmInBackground` / `skipPreflight` settings.
  - In local testing, queue drain remained materially below target, indicating the primary ceiling is not just cranker-side confirmation latency.

### 28) Quoter targeted cancel mode added; prior nonblocking path skipped cancels almost entirely
- Symptom:
  - With `QUOTER_NONBLOCKING_SUBMIT=true`, the quoter rarely emitted cancel intents in `client-id` mode even after successful place traffic.
- Root cause:
  - `lastPlacedClientOrderId` was only updated after the async place submit resolved.
  - Subsequent pipelined ticks usually observed `null` and skipped cancel generation.
- Fix:
  - Updated `ts/client/scripts/execution-queue/random-sol-usdc-quoter-bot.ts` to:
    - support `QUOTER_CANCEL_MODE=all|client-id`,
    - pre-record the next `clientOrderId` before submit so the following pipelined tick can issue a targeted cancel-by-client-id.
- Validation:
  - Harness events now show both place (`variant=0`) and cancel-by-client-id (`variant=2`) accepts under fast-mode stress.

### 29) Relayer optional local user-signature verification bypass for local stress profiling
- Change:
  - Updated `ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts` with:
    - `CTM_RELAYER_VERIFY_USER_SIGNATURE` (default `true`)
  - When disabled for local stress profiling, relayer skips redundant local `nacl` verification and still relies on the ed25519 preinstruction embedded in the transaction for signature enforcement.
- Validation:
  - Fast-mode restart with `CTM_RELAYER_VERIFY_USER_SIGNATURE=false` remained functional.
  - In this environment, observed throughput changes were not sufficient to approach the 50 ops/s target.

### 30) Latest local throughput reality check after targeted-cancel + fast-relayer iterations
- Run profile A:
  - relayer `strict`, `maxInflight=128`, `maxQueued=512`
  - quoter: `10 bots`, `200ms`, `client-id` cancel mode
- Result:
  - 35s window: `~800 accepted place intents` and `0 accepted cancels`
  - sustained accepted place throughput: about `22.9/s`
- Run profile B:
  - relayer `fast`, `maxInflight=256`, `maxQueued=1024`
  - quoter: same `10 bots`, `200ms`, `client-id` cancel mode (before pre-record fix)
- Result:
  - early 8s window: `212 accepted place intents`, `0 accepted cancels` -> about `26.5 accepted place/s`
- Run profile C:
  - relayer `fast`, same gate settings, after pre-record cancel-id fix
  - quoter: same `10 bots`, `200ms`, `client-id` cancel mode
- Result:
  - early 8-9s sample: `139 accepted total = 49 place + 90 cancel-by-client-id`
  - combined accepted throughput remained about `15-24 ops/s` depending on sample window
- Implication:
  - The local stack is still materially below the `50 place/cancel ops/s` target.
  - Current dominant ceiling appears to be relayer submit-path / local validator enqueue throughput rather than quoter intent generation alone.

### 31) Execution queue refactored to single-account zero-copy ring, and fresh-localnet E2E now drains
- Change:
  - Refactored the onchain execution queue to a single zero-copy account with ring semantics:
    - removed the separate `ExecutionQueueBuffer` account
    - kept queue items in-account as `[QueueItem; 1000]`
    - added `head/count/next_sequence_to_execute/max_seen_sequence` header fields and ring helpers
    - added chunked `execution_queue_create` + `execution_queue_resize` + `execution_queue_init` bootstrap flow so the large PDA can be created reliably on localnet
  - Exported canonical execution-queue layout constants from the program state and updated the Rust executor and TS tooling to use one shared layout contract instead of duplicated magic offsets.
  - Added `ts/client/src/executionQueueLayout.ts` and moved queue-account decoding in:
    - `execution-queue-cranker.ts`
    - `inspect-queue-head.ts`
    - `local-perp-e2e-run.ts`
    - bootstrap scripts consuming `EXECUTION_QUEUE_ACCOUNT_SPACE`
  - Relaxed stale TS API compatibility so `executionQueueBuffer` is no longer required in the execution-queue builder types.
- Validation:
  - `cargo check --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl` passed.
  - `cargo check -p service-mango-execution-engine` passed.
  - `./node_modules/.bin/tsc --noEmit --pretty false` passed.
  - Fresh stack restart on localnet:
    - validator `http://127.0.0.1:8899`
    - harness `http://127.0.0.1:9091`
    - relayer `127.0.0.1:9090`
    - fresh config `/home/ec2-user/stagin4/mng-v4/.localnet/run/execution-queue-e2e-9123.json`
  - Queue-account inspection on fresh localnet showed:
    - `capacity=1000`
    - `count=2`
    - valid head item decode with expected `accountsHash`
  - End-to-end run succeeded on fresh localnet:
    - command: `E2E_OUTPUT_CONFIG_PATH=...9123.json CTM_RELAYER_ADDR=127.0.0.1:9090 CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 npm run -s execution-queue-local-perp-e2e-run`
    - result: `status=ok`
    - relayer submissions accepted sequences `2`, `3`, `4`
    - positions after drain: `makerBaseLots=10000`, `takerBaseLots=-10000`
  - Rust executor metrics during the successful run:
    - `execution_engine_execute_attempts_total 9`
    - `execution_engine_execute_sent_total 9`
    - `execution_engine_requests_ok_total 3`
- Follow-up:
  - The stale relayer process that startup launched before the rebuild stayed on the old binary; after replacing it with the rebuilt Rust engine, the executor immediately began draining.
  - The execution-queue refactor and local Mango integration path are now in working shape again; the remaining work returns to throughput optimization rather than queue-account correctness.

### 32) Fresh post-refactor throughput result: offchain submit path now clears 50 ops/s; queue drain is the new bottleneck
- Run profile:
  - fresh localnet group `9123`
  - Rust relayer/executor active on:
    - gRPC `127.0.0.1:9090`
    - HTTP metrics `127.0.0.1:9093`
  - 10 provisioned quote bots
  - quoter settings:
    - `QUOTER_INTERVAL_MS=100`
    - `QUOTER_NONBLOCKING_SUBMIT=true`
    - `QUOTER_MAX_INFLIGHT=500`
    - `QUOTER_CANCEL_MODE=client-id`
    - `QUOTER_CANCEL_EVERY_TICKS=1`
    - `QUOTER_CANCEL_BEFORE_PLACE=true`
    - `QUOTER_LOG_EACH_ORDER=false`
- Client-side observed generation:
  - quoter reported roughly `43-45 place intents/s`
  - quoter reported roughly `88-90 combined intents/s`
- Harness-validated acceptance over the main window:
  - `1841` `relay_intent_accepted` events
  - acceptance window duration: about `20.718s`
  - accepted throughput: about `88.86 ops/s`
- Relayer metrics during the run:
  - `execution_engine_requests_total 1844`
  - `execution_engine_requests_ok_total 1844`
  - `execution_engine_requests_error_total 0`
  - `execution_engine_submit_avg_ms 183.388`
- Queue / execute observations:
  - only `14` `queue_item_processed` events occurred in the same window
  - queue inspection immediately after the run showed:
    - `count=1000`
    - `next=20`
    - `max=1845`
  - this means the execution queue filled completely while accepted submissions continued at high rate
- Implication:
  - Offchain submit throughput is no longer the primary blocker for the `50 place/cancel ops/s` target.
  - The bottleneck has shifted to queue drain / onchain execution throughput.
