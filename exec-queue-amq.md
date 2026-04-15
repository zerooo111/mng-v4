# Execution Queue AMQ Plan

## Goal

Turn the current Mango v4 execution queue into a paged queue topology that scales with backlog while preserving the actual fairness and safety contract the code has today:

- FIFO is local to a queue domain.
- For perp flow, the primary queue domain is the perp market.
- Liquidity remains a separate queue domain.
- `accounts_hash` remains the execution anti-tamper root.
- Head-driven execution, bounded gap handling, and deterministic failure handling stay on-chain.

This note has two parts:

1. The minimal viable implementation against the current codebase.
2. A concrete pagination plan for per-market queue roots.

## Ground Truth From Current Code

The current implementation is already closer to "per-market queue domains" than the older docs imply.

### What the code actually does

- `programs/mango-v4/src/state/execution_queue.rs` is `layout_version = 2`, with:
  - 16 CTM sub-queues, one per market index slot.
  - 64 CTM items per market sub-queue.
  - 1 separate liquidity ring.
- CTM sequence reservation in `bin/service-mango-execution-engine/src/main.rs` is already keyed per `{group}:{market}`.
- Execution is still head-based within a queue.
- Queue items currently persist payload bytes plus `payload_hash` and `accounts_hash`, but not enough self-describing metadata to reconstruct canonical remaining accounts after relayer restart without off-chain lane state.

### What is still missing

- Docs still describe an older single-root, single-head, 1024-item CTM ring.
- Executor recovery still depends on off-chain lane knowledge and event-log replay.
- On-chain terminal prevalidation is narrow.
- Health-gated place-order failures can still pin the queue head because the instruction rolls back after book mutation failure.
- There is no paged storage model. The CTM backlog per market is hard-capped by the 64 in-account slots.
- There is no compactable maker-band operation today.

### Consequence

The MVP should not pretend the system is a single global FIFO queue. The clean model is:

- one FIFO queue per perp market
- one separate FIFO queue for liquidity
- pagination applied independently to each queue root

That matches the current code direction and avoids a larger redesign around a fake global head.

## Part 1: Minimal Viable Implementation

## MVP Scope

The minimal viable implementation should land in two phases.

### Phase 1

Phase 1 is the actual MVP:

- introduce paged queue storage per queue domain
- make CTM items self-describing enough for restart-safe execution
- keep current per-market FIFO semantics
- keep `accounts_hash` verification as the execution gate
- add deterministic pre-checks and explicit queue-side expiry handling
- do not add compaction yet

### Phase 2

Phase 2 adds:

- `MakerReplaceBandV1`
- `MakerBandState`
- bounded head compaction under the narrow safety rule

Compaction should not be in the first delivery. Pagination and restart-safe execution solve the bigger operational problem with much less risk.

## Phase 1 Design

### 1. Introduce a new queue layout version

Add a new queue layout version for paged queues.

- Do not overload the current `layout_version = 2`.
- Existing Rust, TS, and relayer parsers already hard-code v2 offsets and semantics.
- Use a clean version boundary for migration and rollout.

Recommended name:

- `ExecutionQueueLayoutVersion::V3`

### 2. Keep queue fairness domains explicit

Define queue identity explicitly.

```text
QueueDomain
- PerpMarket { market_index: u16, shard_id: u8 }
- Liquidity
```

Rules:

- In v3, executable perp flow always uses `shard_id = 0`.
- Liquidity stays separate from perp queues.
- Sequence numbers are local to the queue root, not global to the group.

### 3. Replace fixed CTM in-account rings with paged CTM storage

The current root queue account should stop storing CTM items inline for v3.

Keep the root account for:

- per-queue head state
- per-queue max seen sequence
- pause flags
- queue geometry
- signer/admin metadata

Move CTM items into page PDAs.

The liquidity queue can either:

- stay inline in the root for the first v3 cut, or
- use the same page model with smaller geometry

The cleaner long-term answer is to use the same page abstraction for liquidity too, but CTM pagination is the primary scaling target.

### 4. Add self-describing queued metadata

For CTM items, add the minimum metadata needed for safe restart and bounded future compaction:

```text
QueueItemV2 / QueueItemV3 (CTM page item)
- sequence: u64
- min_execute_slot: u64
- ingress_slot: u64
- first_failure_slot: u64
- expires_at_slot: u64
- kind: u8
- status: u8
- retries: u8
- flags: u8
- op_class: u8
- market_index: u16
- failure_code: u16
- recipe_kind: u8
- recipe_len: u8
- account_recipe: [u8; 24]
- compact_key: [u8; 16]
- superseded_by: u64
- match_kind: u8
- match_side: u8
- match_limit_price_lots: i64
- mango_account_hint: Pubkey
- payload_len: u16
- payload_hash: [u8; 32]
- accounts_hash: [u8; 32]
- payload: [u8; N]
```

Important MVP rule:

- `mango_account_hint` should be stored explicitly.
- Do not force the MVP to encode the primary Mango account only indirectly inside the 24-byte recipe blob.

The executor already knows how to derive canonical perp remaining accounts from on-chain mirrors when given `{group, market_index, mango_account}`. The queue should persist exactly that much identity directly.

### 5. Add deterministic queue-side pre-checks

Before dispatch, the queue should do deterministic, read-only checks and consume failures on-chain instead of leaving a sticky head where possible.

For Phase 1:

- expired item by `expires_at_slot`
- malformed payload or unsupported payload kind
- obvious numeric bounds
- valid side and nonzero size where relevant
- account existence and ownership shape that can be checked without mutation
- market existence and active status
- conservative account-recipe shape checks

On deterministic failure:

- mark item `Failed`
- set `failure_code`
- advance head

This is the queue-level hardening that the current stall analysis calls for.

### 6. Reconstruct lanes from queued metadata plus on-chain mirrors

The relayer/executor should stop depending on off-chain event logs as the primary lane recovery path.

For perp CTM items, the relayer should derive the canonical remaining account lane from:

- queue domain
- `market_index`
- `mango_account_hint`
- `account_recipe`
- current on-chain group, market, bank, and oracle mirrors

The program still verifies `accounts_hash` at execution time. Recipe metadata is advisory and reconstructive only.

### 7. Keep current execution ordering semantics

The execution contract should remain:

1. Load the head item for the queue root.
2. Apply slot gate.
3. Apply bounded gap handling.
4. If expired, fail and advance.
5. Verify provided remaining accounts against `accounts_hash`.
6. Run deterministic pre-check.
7. Dispatch payload.
8. If success, mark executed and advance.
9. If a health-gated place order mutates state and later fails, handle according to the current compensating-cancel design when implemented.

The MVP does not need to solve every health-region rollback edge case on day one, but it should restructure the item format and instruction surface so compensating-cancel can be added without another storage migration.

### 8. Add a hard admission bound backed by queue geometry

For each queue root:

```text
capacity = page_size * num_pages
valid enqueue window =
  [next_sequence_to_execute, next_sequence_to_execute + capacity)
```

Also add:

- `soft_limit` for operational backpressure
- optional stricter relayer-side limit before on-chain hard rejection

### 9. Clean up type drift before adding new ops

Before adding `MakerReplaceBandV1`, align payload enums across:

- on-chain Rust
- executor Rust
- TS client builders
- TS raw queue decoders

There is already enum drift in the current tree. Fix that first.

### 10. Migration and rollout

Use a drained migration, not an in-place mixed semantics rollout.

Migration steps:

1. Pause enqueue on the old queue.
2. Drain or explicitly drop old pending items.
3. Create v3 queue roots.
4. Start relayer/executor in dual-read mode if needed during rollout.
5. Switch enqueue to v3.
6. Switch execute to v3-only.
7. Remove old lane-recovery dependence on event logs after verification.

## Phase 2 Design

Phase 2 adds the narrow compactable operation and nothing more.

### Phase 2 features

- add `MakerReplaceBandV1`
- add `MakerBandState`
- allow supersession links only for maker-band items
- compact only by skipping the earlier head item
- never execute the later item early
- fail closed on uncertainty

### Phase 2 constraints

- generic `PerpPlaceOrderV2` remains non-compactable
- generic cancels remain non-compactable
- liquidation and admin ops remain non-compactable
- compaction only applies under fixed-band-budget reservation mode
- any same-maker intervening op blocks compaction

## Part 2: Concrete Pagination Plan

## Queue Topology

Pagination is per queue domain, and the primary queue domain is the perp market.

Do not define one giant paged CTM queue for the whole group.

The concrete v3 topology should be:

- one paged FIFO queue root per perp market
- one separate paged FIFO queue root for liquidity
- no global CTM head across markets

This makes queue identity explicit and resolves the current doc ambiguity between the old single-ring description and the actual per-market sequencing behavior.

## PDA Layout

### 1. Group-global signer and admin state

```text
QueueAuthorityState
PDA seeds = ["queue-authority", group]
- group: Pubkey
- ctm_signer: Pubkey
- pending_ctm_signer: Pubkey
- pending_ctm_activation_slot: u64
- admin: Pubkey
- bump: u8
```

Purpose:

- make relayer signer rotation group-global
- avoid duplicating signer state across market queue roots

### 2. Per-market queue root

```text
MarketQueueRootV2
PDA seeds = ["perp-queue-root", group, market_index, shard_id]
- group: Pubkey
- market_index: u16
- shard_id: u8
- next_sequence_to_execute: u64
- max_seen_sequence: u64
- live_count: u32
- gap_observed_slot: u64
- gap_wait_slots: u8
- page_size: u16
- num_pages: u16
- soft_limit: u16
- recipe_version: u16
- max_compaction_distance: u8
- min_expiry_buffer_slots: u8
- execute_paused: bool
- enqueue_paused: bool
- bump: u8
```

Rules:

- `shard_id = 0` for all executable perp flow in v3.
- `max_seen_sequence` and `next_sequence_to_execute` are local to this queue root only.

### 3. Liquidity queue root

```text
LiquidityQueueRootV2
PDA seeds = ["liq-queue-root", group]
- group: Pubkey
- next_sequence_to_execute: u64
- max_seen_sequence: u64
- live_count: u32
- gap_observed_slot: u64
- liquidity_delay_slots: u8
- page_size: u16
- num_pages: u16
- execute_paused: bool
- enqueue_paused: bool
- bump: u8
```

### 4. Queue page

```text
QueuePageV2
PDA seeds = ["queue-page", queue_root, page_slot]
- queue_root: Pubkey
- page_slot: u16
- assigned_abs_page_no: u64
- live_count: u16
- first_pending_offset: u16
- page_state: u8
- bump: u8
- items: [QueueItemV3; PAGE_SIZE]
```

Rules:

- A page belongs to exactly one queue root.
- A page never mixes markets.
- `page_slot` is the ring slot inside the queue root.
- `assigned_abs_page_no` is the absolute page number currently occupying that slot.
- Reuse of a `page_slot` is only allowed after the prior absolute page is fully drained and retired.

## Queue Geometry and Sequence Math

For a given queue root `Q`:

```text
capacity(Q) = page_size(Q) * num_pages(Q)
abs_page_no = sequence / page_size(Q)
offset      = sequence % page_size(Q)
page_slot   = abs_page_no % num_pages(Q)
```

Valid enqueue window:

```text
[next_sequence_to_execute(Q),
 next_sequence_to_execute(Q) + capacity(Q))
```

This means:

- pagination is local to each queue root
- a hot market can build backlog without consuming buffer for other markets
- each market has its own head, gap timer, and admission window

## Queue Item Format in the Paged Model

For v3 CTM items, use the enriched item format from Part 1.

Minimum required additions beyond the current item:

- `expires_at_slot`
- `failure_code`
- `flags`
- `op_class`
- `recipe_version`
- `account_recipe`
- `match_kind`
- `match_side`
- `match_limit_price_lots`
- `compact_key`
- `superseded_by`
- `mango_account_hint`

For liquidity items, reuse the same page item envelope where practical, but only require the fields needed by liquidity execution.

## Signed Domain Binding

Queue identity must be bound into both user and relayer signatures.

### User intent hash

```text
user_intent_message = SHA256(
    "fermi:user-intent-v2",
    group,
    domain_tag,
    market_index,
    shard_id,
    mango_account,
    user_owner,
    kind,
    payload_hash,
    accounts_hash
)
```

### Relayer envelope hash

```text
envelope_message = SHA256(
    "fermi:ctm-envelope-v2",
    group,
    domain_tag,
    market_index,
    shard_id,
    sequence,
    min_execute_slot,
    kind,
    payload_hash,
    accounts_hash,
    expires_at_slot
)
```

Rules:

- `domain_tag = PerpMarket` for perp flow
- `domain_tag = Liquidity` for liquidity
- `market_index = 0` for liquidity
- `shard_id = 0` in v3

Without this binding, the same payload could be replayed into the wrong queue namespace.

## Instruction Surface

## Root management

Add root lifecycle instructions:

- `execution_queue_init_authority_state(group, admin, ctm_signer)`
- `execution_queue_init_market_root(group, market_index, shard_id, page_size, num_pages, soft_limit, ...)`
- `execution_queue_init_liquidity_root(group, page_size, num_pages, ...)`
- `execution_queue_update_root_config(...)`
- `execution_queue_rotate_ctm_signer(...)`

## Page management

Add explicit page lifecycle instructions:

- `execution_queue_init_page(queue_root, page_slot)`
- `execution_queue_close_page(queue_root, page_slot)`

Page creation should be lazy on first use.

## Enqueue

Split enqueue by queue domain:

- `execution_queue_enqueue_market(...)`
- `execution_queue_enqueue_liquidity(...)`

For market enqueue:

1. Decode `market_index` from payload.
2. Set `domain = PerpMarket { market_index, shard_id = 0 }`.
3. Derive the market queue root.
4. Check queue-local sequence window.
5. Materialize item metadata.
6. Resolve target page and offset.
7. Create or validate the page PDA.
8. Write the item.
9. Update queue-root `max_seen_sequence` and `live_count`.

For liquidity enqueue:

1. Derive the liquidity root.
2. Apply liquidity delay semantics.
3. Resolve target page and offset.
4. Write the item and update the root.

## Execute

Execution should be scoped to a single queue root:

- `execution_queue_execute_market(queue_root, head_page, lookahead_page, ...)`
- `execution_queue_execute_multi_market(queue_root, head_page, lookahead_page, ...)`
- `execution_queue_execute_liquidity(liq_root, head_page, lookahead_page, ...)`

Properties:

- each call advances only that queue root
- gap handling is local to that queue root
- retries are local to that queue root
- one market queue stall does not block another market queue

`lookahead_page` is optional in the MVP but should be part of the interface now so Phase 2 compaction can scan within `max_compaction_distance` without another instruction break.

## Executor Scheduling Rules

Multiple market queues do not remove runtime lock contention.

The important distinction is:

- fairness domain = queue root
- runtime lock domain = actual Solana write set

The executor should enforce:

- no concurrent execute transactions for the same `mango_account_hint`
- no concurrent execute transactions for the same queue root

This prevents avoidable self-conflicts while allowing true cross-market parallelism when write sets do not overlap.

## Explicit Non-Goals for v3

These should be written into the spec so the topology stays narrow:

- no global CTM FIFO queue across markets
- no multiple executable queue heads inside one perp market
- no arbitrary intra-market sharding in v3
- no compaction for generic place orders
- no early execution of a later superseding item

## Future Intra-Market Sharding

Keep room for future sharding via `shard_id`, but do not enable it in v3.

Only allow `shard_id > 0` in a future version if all are true:

1. routing from payload to shard is deterministic and signed
2. different shards cannot mutate the same book or event-queue state
3. orders in different shards cannot cross each other
4. FIFO is only claimed within the shard

Until then:

- `QueueDomain::PerpMarket` uses `shard_id = 0` only

## Recommended Defaults

Recommended initial geometry:

```text
Perp market queue:
- page_size = 128
- num_pages = 16
- hard_cap = 2048
- soft_limit = 256

Liquidity queue:
- page_size = 64
- num_pages = 4
- hard_cap = 256
```

Operational defaults:

```text
gap_wait_slots = observed_p99_slots
max_compaction_distance = 16
min_expiry_buffer_slots = 4
MAX_BAND_LEVELS = 8
```

Page geometry should be immutable per queue root after initialization.

## Implementation Order

## Step 0: Documentation and enum cleanup

- align docs to the actual current v2 implementation
- align payload enums across Rust and TS
- document queue-domain-local sequence namespaces

## Step 1: Add root and page accounts

- add `QueueAuthorityState`
- add `MarketQueueRootV2`
- add `LiquidityQueueRootV2`
- add `QueuePageV2`
- add init and admin instructions

## Step 2: Add paged enqueue and execute instructions

- add market enqueue
- add liquidity enqueue
- add market execute
- add liquidity execute
- preserve current `accounts_hash` verification

## Step 3: Add self-describing metadata and restart-safe executor recovery

- persist `mango_account_hint`
- persist `account_recipe`
- persist match metadata
- derive lanes from queued metadata plus on-chain mirrors
- stop depending on event logs as the primary recovery source

## Step 4: Add deterministic queue-side failure consumption

- explicit expiry failure
- explicit pre-check failure codes
- head advancement on deterministic failure

## Step 5: Add `MakerReplaceBandV1` and compaction

- add maker band state
- add supersession links
- add bounded safe compaction scan
- keep compaction narrow and fail closed

## Test Plan

At minimum, add tests for:

- independent sequence namespaces for two perp markets
- one market queue stalled while another continues executing
- queue-local gap handling
- queue-local admission bounds
- lazy page creation and page-slot reuse after drain
- relayer restart with lane reconstruction from queued metadata
- `accounts_hash` mismatch leaves the head unchanged
- explicit expiry failure advances head
- deterministic pre-check failure advances head
- liquidity queue remains independent from perp queues
- Phase 2 only: safe compaction succeeds for clean maker-band supersession
- Phase 2 only: compaction blocked by matching taker
- Phase 2 only: compaction blocked by same-maker intervening op

## Bottom Line

The clean pagination design is:

- one paged FIFO queue root per perp market
- one separate paged liquidity queue
- sequence, head, gap, and capacity local to each queue root
- queue identity bound into the signed domain
- no intra-market multi-head sharding for executable order flow in v3

The real MVP is not "build compaction first." It is:

- paged per-market queue roots
- richer self-describing queued metadata
- queue-side deterministic failure consumption
- restart-safe canonical lane reconstruction

That lands the scaling improvement and the operational hardening first, while leaving the narrow maker-band compaction feature to a controlled second phase.
