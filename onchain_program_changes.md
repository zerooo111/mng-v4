# Onchain Program Changes

Updated: 2026-03-24 UTC

## Current Devnet Deployment

- Program id: `4CGsiGHZXSnweudEcN235xkLz4twT2DJB35hS7t89cUm`
- Upgrade authority / admin used for deploy: `CyJSpqonriELcXeSQXnZ17AQsb77ZsHWFdttMmBstq8s`
- Deployment date: 2026-03-24
- Deployment note: deployed with explicit `--max-len 4100000` to keep rent close to the actual binary size instead of the oversized default allocation

## Onchain Queue Fixes

### 1. Empty Queue Gap Normalization

File:
- `programs/mango-v4/src/state/execution_queue.rs`

Change:
- Added `normalize_empty_ctm_head()`
- Called from:
  - `clear_current_ctm_head_and_advance()`
  - `clear_ctm_item_at()`

Behavior:
- When `ctm_count == 0`, the queue now normalizes `next_sequence_to_execute` to `max_seen_sequence + 1`
- `gap_observed_slot` is cleared when the ring becomes empty

Reason:
- Fixes the observed edge case where the queue ended in an empty-but-gapful state:
  - `count = 0`
  - `next_sequence_to_execute < max_seen_sequence`

Operational effect:
- Native skip advancement and final head clearing can no longer leave a stale skipped-gap head behind when the queue is already empty

### 2. Terminal CTM Prevalidation

File:
- `programs/mango-v4/src/instructions/execution_queue.rs`

Added:
- `TerminalCtmFailureReason`
- `prevalidate_terminal_ctm_payload(payload, now_ts)`
- `terminal_ctm_failure_msg(reason)`

Current terminal-invalid classification:
- `PerpPlaceOrderV2.price_lots < 0`
- `PerpPlaceOrderV2.expiry_timestamp != 0 && expiry_timestamp <= now_ts`

Applied at:
- enqueue-time in `execution_queue_enqueue_ctm`
- enqueue-time in `execution_queue_enqueue_direct`
- execute-time in `execution_queue_execute`
- execute-time in `execution_queue_execute_multi`

Behavior:
- enqueue rejects terminal-invalid CTMs before they enter the queue
- execute clears terminal-invalid CTMs onchain before health-region dispatch
- execute emits failed processing state instead of allowing indefinite head stalls on deterministic terminal-invalid payloads

Reason:
- Avoids revert-driven queue stalls for payloads that are deterministically invalid from immutable inputs and time

### 3. Regression Coverage

File:
- `programs/mango-v4/src/state/execution_queue.rs`

Added test coverage for:
- clearing the last pending CTM item advancing the queue past a previously resolved gap

## Build and Dependency Changes Required To Produce A Fresh SBF Artifact

These were necessary to get a fresh deployable `target/deploy/mango_v4.so` on this machine.

### 1. Solana Workspace Version Pin

File:
- `Cargo.toml`

Change:
- pinned workspace Solana crates to exact `=1.16.14`

Reason:
- keeps dependency resolution aligned with the local Solana 1.16.14 SBF toolchain
- prevents resolution drift to newer `1.16.17+` crates during `cargo-build-sbf`

### 2. Local OpenBook Patch Simplification

Files:
- `third-party/openbook-v2-patched/programs/openbook-v2/Cargo.toml`
- `third-party/openbook-v2-patched/programs/openbook-v2/src/state/oracle.rs`

Change:
- removed `switchboard-program` and `switchboard-solana` dependencies from the local OpenBook fork
- removed Switchboard oracle parsing from the local OpenBook oracle module
- retained supported local paths for:
  - Pyth
  - Stub oracle
  - Raydium CLMM

Reason:
- the Switchboard path dragged in newer Anchor/SPL/Solana dependency chains that broke the local SBF build
- this repo's local build/test flow does not depend on Switchboard oracle support in the patched OpenBook path

### 3. Local Compatibility Patches

Files:
- `Cargo.toml`
- `third-party/quote-1.0.45-local/Cargo.toml`
- `third-party/zmij-1.0.21-local/Cargo.toml`
- `third-party/num_enum-0.7.6-local/Cargo.toml`
- `third-party/num_enum_derive-0.7.6-local/Cargo.toml`
- `third-party/proc-macro-crate-1.3.1-local`
- `third-party/unicode-ident-1.0.24-local/Cargo.toml`
- `third-party/syn-2.0.117-local/Cargo.toml`

Change:
- added local crate patches and compatibility edits to get the resolver/toolchain path through `cargo-build-sbf`

Note:
- several of these patches stopped being active once the workspace/OpenBook/Solana graph was pinned correctly
- they remain in-tree for now because they were part of the successful build-unblock sequence

## Deployment Notes

Artifact:
- `target/deploy/mango_v4.so`

Artifact size:
- `4,035,552` bytes

Observed rent behavior:
- rent-exempt minimum for the program-size range is about `27-28 SOL`
- default deploy behavior attempted a much larger allocation and required about `56 SOL`
- explicit `--max-len 4100000` restored deploy cost to about `28.56 SOL`

Observed deploy sequence:
- buffer initialization funded an intermediate buffer
- final deploy transaction created and initialized program `4CGsiGH...`
- deploy completed successfully on 2026-03-24

## Follow-up Validation To Run

- cut the running harness / relayer / executor / guard stack over to `4CGsiGH...`
- re-bootstrap the devnet group and queue state as needed for the new program id
- rerun the real-executor queue test
- verify the queue no longer ends in an empty-but-gapful state
- recheck divergence and skipped-gap accounting under live bot flow
