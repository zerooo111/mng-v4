# Execution Queue Dispatcher Design (V1)

## Scope

This document defines the next implementation step after queue ingress hardening:

- decode queued payloads into typed Mango operations;
- execute them on-chain through existing Mango instruction logic;
- keep execution deterministic for off-chain optimistic state simulation.

This is a V1 design for perps FIFO and delayed liquidity operations.

## Goals

- Preserve deterministic CTM ordering for CTM-wrapped flow (`sequence` ascending).
- Execute queue items by calling real Mango business logic, not dequeue-only stubs.
- Make permissionless cranking safe: any cranker can execute, but cannot alter semantics.
- Define a stable payload format that off-chain harnesses can parse and replay.
- Define account metas/auth requirements per payload type.

## Non-Goals

- Rewriting existing Mango core instruction handlers.
- Supporting arbitrary nested CPIs from queue payloads.
- Supporting dynamic account resolution heuristics in-program.

## Queue Payload Format

All queued payload bytes are encoded as `QueuePayloadV1`:

```text
u8 version                   // must be 1
u8 variant                   // QueueVariant
u16 flags                    // reserved, must be 0 in V1
[N bytes] borsh-encoded variant body
```

`QueueVariant`:

- `0`: `PerpPlaceOrderV2`
- `1`: `PerpModifyOrder`
- `2`: `PerpCancelOrder`
- `3`: `PerpCancelOrderByClientOrderId`
- `4`: `PerpCancelAllOrders`
- `5`: `PerpCancelAllOrdersBySide`
- `6`: `LiquidityDeposit`
- `7`: `LiquidityWithdraw`

Versioned envelope allows backward-compatible expansion.

## CTM Envelope Contract

For `execution_queue_enqueue_ctm`:

- `envelope.kind` must be `CtmWrapped`.
- `payload_hash = sha256(payload_bytes)`.
- `accounts_hash = sha256(concat(account_metas))` where each meta is:
  - `pubkey[32]`
  - `is_signer[1]`
  - `is_writable[1]`
- signed message is canonical hash over:
  - domain separator
  - group pubkey
  - sequence
  - min execute slot
  - kind
  - payload hash
  - accounts hash
  - expires slot

This means dispatch behavior is fully committed by signed bytes + account metas.

## Account Contract Per Item

Each queue item execution uses:

- fixed accounts from `ExecutionQueueExecute` context (`group`, `execution_queue`);
- remaining accounts copied exactly from enqueue-time metas and verified by hash;
- no account auto-discovery in execute path.

Implication: if account order or signer/writable flags differ from CTM-signed metas, execute fails deterministically.

## Auth Model

### CTM-wrapped variants

- Ingress requires valid CTM signature.
- Execute is permissionless.
- Execution authority semantics are carried by signed payload/account set.
- Signerless crank path:
  - payload account metas must place `owner` at index 2 as the `ExecutionQueue` PDA;
  - users authorize this by setting their Mango account delegate to the queue PDA;
  - execute dispatch upgrades owner meta to signer and uses `invoke_signed` with queue PDA seeds.

### Liquidity variants

- No CTM signature.
- Must include explicit owner authority account in metas.
- Execute checks owner/delegate auth using existing Mango account rules.
- `min_execute_slot` is enforced with `liquidity_delay_slots`.

## Execute Flow (V1)

1. Select next executable item:
   - CTM lane: `next_sequence_to_execute` first.
   - Liquidity lane only when CTM sequence cannot advance in this iteration.
2. Recompute `payload_hash` and `accounts_hash` from stored bytes and provided metas.
3. Decode `QueuePayloadV1`.
4. Dispatch by variant into an internal `execute_*` function.
5. On success:
   - mark item executed;
   - clear slot;
   - advance sequence for CTM lane.
6. On deterministic decode/auth failure:
   - mark failed and emit error code;
   - do not mutate sequence unless policy says skip/fail-open.
7. On transient failure (missing account state/oracle freshness etc.):
   - increment retry metadata;
   - keep pending with backoff policy.

For CTM perp variants the dispatch uses queue PDA signer authorization as described above, enabling permissionless cranking without end-user signatures.

## Failure Policy

V1 recommended defaults:

- Decode mismatch/hash mismatch/auth mismatch: terminal failure.
- State-dependent transient errors: retryable up to `max_retries`.
- Sequence gaps: keep existing `gap_wait_slots` skip policy only for truly absent sequence IDs.

## Off-Chain Harness Contract

Harness maintains deterministic mirror state by:

1. subscribing to `QueueItemEnqueued` + `QueueItemProcessed`;
2. storing payload bytes + account metas exactly as submitted;
3. replaying the same typed dispatcher logic locally;
4. exposing optimistic state immediately after CTM-signed enqueue acceptance.

Required harness invariants:

- identical payload codec;
- identical account-meta hashing;
- identical execution ordering policy;
- identical error classification for terminal vs retryable.

## Implementation Plan

1. Add `QueuePayloadV1` Rust types + strict decode/encode helpers.
2. Add execute dispatcher module with per-variant handlers.
3. Route handlers to existing Mango internal logic (no CPI to self).
4. Add terminal/retryable failure classification and queue item status transitions.
5. Add program tests:
   - CTM signature + hash validation;
   - per-variant decode/auth checks;
   - deterministic ordering with gap + liquidity delay interactions.
6. Add TS harness package that reuses shared payload codec constants.
