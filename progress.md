# FIFO Execution Queue Optimization Progress

## Current Status: Ready for Devnet Testing

Deploy keypair: `4zDJ8FiJu7A3ZHjCwdKF2JPyLYheFRpGVvCvHS32XnhM`
(at `/home/ec2-user/.config/solana/id.json`)

## Localnet Results (single-threaded test-validator)

| Metric | v20 (baseline) | v49 (optimized) | Notes |
|---|---|---|---|
| peakWindowPlaceTps | 52 | 55 | Validator-capped |
| peakWindowIntentTps | 102 | 95 | |
| avgPlaceTps | 34.28 | 34.88 | Limited by validator capacity |
| execute_head_missing | 30 | 0 | Eliminated |
| tickErrorCount | 7 | 2 | Near-zero |
| items per landed execute tx | ~10 (1 lane) | ~7 (5 lanes) | Multi-lane working |
| execute tx send rate | 5.5/s | 13/s | 2.4x improvement |
| submit_prepare_avg_ms | 30 | 9.3 | 3x faster |

### Why localnet caps at ~35 avgPlaceTps

The test-validator is single-threaded with ~200 tx/s total capacity. Enqueue and execute txs compete for that capacity. On devnet with parallel processing, the multi-lane execute should unlock significantly higher throughput.

## Changes Made

### Onchain Program (`programs/mango-v4/src/`)

1. **`execution_queue_execute_multi` instruction** (instructions/execution_queue.rs)
   - Processes items from up to 5 lane account sets in one tx
   - Lane hashes passed as instruction data (avoids Solana runtime flag OR mismatch)
   - Health region switching between lanes
   - Out-of-order item processing via `find_matching_ctm_item` scan

2. **`merge_effective_runtime_flags_for_hash`** helper for hash computation

3. **Registered in lib.rs** as `execution_queue_execute_multi(ctx, max_items, lane_count, accounts_per_lane, lane_hashes)`

### Rust Executor (`bin/service-mango-execution-engine/src/main.rs`)

1. **Multi-lane dispatch**: builds one execute tx covering up to 5 lanes (de-dup by hash)
2. **Cached queue head inspection**: only fetches 424KB queue account every 500ms (was every iteration)
3. **Removed per-pending-tx status polling**: timeout-only cleanup (eliminated N sequential RPC calls per cycle)
4. **Concurrent tx sends**: via `tokio::spawn` with awaited results
5. **Lane hash alignment**: uses `hash_execution_queue_accounts_for_ctm_enqueue` (same as enqueue path)
6. **`build_execute_multi_tx`** and **`build_execute_multi_instruction`**: new instruction builders

### Config (`startup_all_local.sh`)

```
EXECUTION_QUEUE_CRANK_MATCH_HEAD_ONLY=false
EXECUTION_QUEUE_CRANK_HEAD_LOCK_MS=5
EXECUTION_QUEUE_CRANK_BUSY_INTERVAL_MS=25
EXECUTION_QUEUE_CRANK_MAX_PENDING_TXS=12
EXECUTION_QUEUE_CRANK_PENDING_TIMEOUT_MS=100
CTM_RELAYER_QUEUE_SOFT_LIMIT=1024
CTM_RELAYER_QUEUE_GAP_SOFT_LIMIT=1024
```

### Key Bug Fixed

**`startup_all_local.sh` was overriding `startup_local.sh` env vars**: the parent launcher had its own hardcoded relayer config (match_head_only=true, max_pending=4) that silently overrode all changes made to `startup_local.sh`. All benchmarks v21-v32 ran with default config. Fixed by updating the parent launcher.

## Architecture Overview

```
Bot (TS/Rust) → gRPC → Relayer (Rust) → RPC → Validator → Onchain Program
                         ↓                                      ↓
                    Sequence mgmt                    ExecutionQueue account
                    Backpressure                     (1024 CTM + 128 liquidity slots)
                         ↓
                    Executor loop → execute_multi tx → Onchain execute_multi
                    (multi-lane,    (5 lanes,          (match any lane hash,
                     cached head,    pre-computed        health region switching,
                     throttled)      hashes)             out-of-order dispatch)
```

## Devnet Testing Plan

1. Airdrop SOL to deploy keypair
2. Deploy program with `execution_queue_execute_multi` instruction
3. Bootstrap group + execution queue
4. Run benchmark with devnet validators (parallel tx processing)
5. Expected: multi-lane execute should drain 5x faster than single-lane
6. Target: 100+ avgPlaceTps sustained

## Key Files Modified

- `programs/mango-v4/src/instructions/execution_queue.rs` - multi-lane execute, hash helpers
- `programs/mango-v4/src/lib.rs` - new instruction registration
- `bin/service-mango-execution-engine/src/main.rs` - executor overhaul
- `startup_all_local.sh` - config tuning
- `startup_local.sh` - config tuning (secondary)
