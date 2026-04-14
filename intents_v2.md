# Intents V2

## Purpose

This document defines the full migration from the current `v1` CTM user-intent format to a relayer-owned `v2` format.

The goal is:

- users sign order semantics,
- the relayer derives dispatch accounts,
- the CTM/envelope still commits to the final `accounts_hash`,
- the executor still routes and matches by `accounts_hash`,
- on-chain still verifies that the dispatched accounts match the queued item.

This is the design required to make relayer-side account derivation safe.

## Why V1 Is Wrong For This

Today the user-intent signature is over:

- `group`
- `mango_account`
- `user_owner`
- `kind`
- `payload_hash`
- `accounts_hash`

Current references:

- [executionQueue.ts](/home/hetalkenaudekar/stagin4/mng-v4/ts/client/src/executionQueue.ts:643)
- [execution_queue.rs](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:259)
- [main.rs](/home/hetalkenaudekar/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs:8248)

That means the sender is not only signing the order payload. The sender is also signing the exact dispatch account layout.

Consequences:

- the relayer cannot repair malformed `remaining_accounts`,
- market routing is only indirectly bound through `accounts_hash`,
- a bad sender account order becomes an auth failure, not just an execution failure,
- a relayer-owned account builder is impossible without changing the signature message.

One more issue: the current user-signed message does not bind `market_index` explicitly. In practice `market_index` is only protected because the market account participates in `accounts_hash`.

## Final V2 Contract

### Core Rule

For `v2`, the user signature must bind:

- `group`
- `mango_account`
- `user_owner`
- `queue item kind`
- `payload_hash`
- explicit target identity

The user signature must **not** bind:

- `remaining_accounts`
- `accounts_hash`
- relayer-derived health accounts
- relayer-derived bank/oracle/fallback account order

The CTM/envelope signature continues to bind:

- `sequence`
- `min_execute_slot`
- `expires_at_slot`
- `payload_hash`
- `accounts_hash`

This preserves queue/executor integrity while moving account construction to the relayer.

### Explicit Target Identity

`v2` must bind the target explicitly.

For perp payloads:

- `target_kind = PerpMarket`
- `target_index = market_index`

For liquidity payloads:

- `target_kind = Token`
- `target_index = token_index`

If more queue payload families are added later, they must also define an explicit signed target.

## Canonical User-Intent Message V2

The final `v2` user-intent message should be a single canonical struct:

```text
domain = "mango-v4-user-intent-v2"

hash(
  domain,
  group,
  mango_account,
  user_owner,
  queue_item_kind,
  target_kind,
  target_index,
  payload_hash
)
```

Recommended encodings:

- `queue_item_kind: u8`
- `target_kind: u8`
- `target_index: u16`
- `payload_hash: [u8; 32]`

This is the minimum needed to let the relayer choose accounts while preventing retargeting to the wrong market or token.

## Important Note About The Current In-Tree Half-Step

There is already a provisional `mango-v4-user-intent-v2` path in tree that removes `accounts_hash` from the user message.

Current references:

- [executionQueue.ts](/home/hetalkenaudekar/stagin4/mng-v4/ts/client/src/executionQueue.ts:14)
- [execution_queue.rs](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:277)
- [main.rs](/home/hetalkenaudekar/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs:8266)

That is not the final design, because it still does **not** bind `market_index` or `token_index`.

That temporary form should not be treated as the finished protocol. The finished protocol is the market-bound/token-bound design in this document.

## API Changes

### gRPC / HTTP Submit API

Current request:

- [ctm_sequencer.proto](/home/hetalkenaudekar/stagin4/mng-v4/ts/client/scripts/execution-queue/ctm_sequencer.proto:9)

Current fields:

- `group`
- `execution_queue`
- `market`
- `payload`
- `remaining_accounts`
- `min_execute_slot`
- `expires_at_slot`
- `user_owner`
- `mango_account`
- `user_signature`

Required `v2` request changes:

- add `intent_version`
- add `target_kind`
- add `target_index`
- deprecate `market`
- deprecate `remaining_accounts`

Recommended shape:

```proto
message SubmitIntentRequest {
  string group = 1;
  string execution_queue = 2;
  string market = 3;                // legacy v1 only
  bytes payload = 4;
  repeated AccountMeta remaining_accounts = 5; // legacy/debug only
  uint64 min_execute_slot = 6;
  uint64 expires_at_slot = 7;
  string user_owner = 8;
  string mango_account = 9;
  bytes user_signature = 10;

  uint32 intent_version = 11;       // 1 or 2
  uint32 target_kind = 12;          // perp=0, token=1
  uint32 target_index = 13;         // market_index or token_index
}
```

Behavior by version:

- `v1`: relayer may only accept if caller-supplied accounts already match the relayer-derived canonical accounts
- `v2`: relayer ignores `remaining_accounts` for execution and derives them locally

The relayer must never try to "repair" `v1` accounts, because that invalidates the user signature.

### Submit Response

The submit response should keep:

- `sequence`
- `tx_signature`
- `user_intent_message`
- `ctm_envelope_message`

It should additionally expose:

- resolved `intent_version`
- resolved `target_kind`
- resolved `target_index`
- resolved `accounts_hash`

This makes debugging much easier when v1 and v2 coexist.

### Relay Event Schema

`relay_intent_accepted` and status events should include:

- `intent_version`
- `target_kind`
- `target_index`
- `accounts_hash`
- `remaining_accounts_source`

`remaining_accounts_source` should be one of:

- `legacy_caller_supplied`
- `relayer_derived`

The accepted event must remain the executor and harness source of truth for derived accounts.

## Client / Bot Changes

### Signing Helpers

Current helper:

- [executionQueue.ts](/home/hetalkenaudekar/stagin4/mng-v4/ts/client/src/executionQueue.ts:606)

Required changes:

- replace current helper with the final market-bound/token-bound `v2` hash
- keep an explicit legacy helper for `v1`
- make the default helper use final `v2`
- expose `target_kind` and `target_index` in helper params

Recommended helper split:

- `buildUserIntentMessageV1(...)`
- `buildUserIntentMessageV2(...)`
- `buildExecutionQueueUserIntent(...)` dispatches by version

The upstream TS helper layer also needs a direct-enqueue builder for the
on-chain fallback path. Right now there is only a CTM-signed builder path in:

- [executionQueue.ts](/home/hetalkenaudekar/stagin4/mng-v4/ts/client/src/executionQueue.ts:809)

The SDK stack needs a second builder for direct user-submitted enqueue that:

- builds the user Ed25519 preinstruction only,
- does not build a CTM preinstruction,
- builds `execution_queue_enqueue_direct`,
- does not require the caller to provide a sequence,
- still computes `payload_hash` and `accounts_hash` locally.

### Callers And Bots

All submitters must be updated:

- relayer bridge
- taker bots
- quoter bots
- test scripts
- manual send scripts
- local/devnet E2E flows

The sender must provide:

- `intent_version = 2`
- `target_kind`
- `target_index`
- `payload`
- `user_signature`

The sender should not need to provide:

- canonical `remaining_accounts`

During migration, callers may still send `remaining_accounts` for logging/debugging, but they should be treated as hints only for `v2`.

### SDK Surface Area

The SDK should expose two explicit intent-placement paths.

#### Path 1: Via Relayer

This is the normal fast path.

Required SDK API shape:

- `submitPerpOrderViaRelayerV2(...)`
- `cancelPerpOrderByClientIdViaRelayerV2(...)`
- `cancelAllPerpOrdersViaRelayerV2(...)`

Behavior:

- caller signs final `v2` user-intent message,
- caller sends `intent_version`, `target_kind`, `target_index`, `payload`, and `user_signature`,
- relayer derives canonical dispatch accounts,
- relayer computes `accounts_hash`,
- CTM signs envelope and submits `execution_queue_enqueue_ctm`.

#### Path 2: Direct On-Chain Enqueue

This is the liveness fallback path for direct user submission to chain.

It must use the on-chain instruction:

- [execution_queue_enqueue_direct](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/lib.rs:620)
- implementation at [execution_queue.rs](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:1421)

Required SDK API shape:

- `enqueuePerpOrderDirectV2(...)`
- `cancelPerpOrderByClientIdDirectV2(...)`
- `cancelAllPerpOrdersDirectV2(...)`

Direct-path behavior from on-chain code:

- no CTM signer verification,
- user signature is still mandatory,
- program assigns `sequence = max_seen_sequence + 1`,
- SDK should not ask the caller for sequence,
- `min_execute_slot` is forced to at least `clock.slot + DIRECT_SUBMIT_DELAY_SLOTS`,
- current constant is `10` slots in [execution_queue.rs](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:64),
- caller still must provide canonical dispatch `remaining_accounts` in the transaction,
- `payload_hash` and `accounts_hash` are still checked on-chain.

So the two SDK paths are not equivalent:

- relayer path: relayer derives accounts,
- direct on-chain path: SDK derives accounts locally and sends them on-chain.

That distinction should be explicit in the public SDK API.

### cont-sdk-fresh Changes

Yes, `cont-sdk-fresh` needs explicit work as part of this migration.

Current relayer-only surfaces:

- [src/trading.ts](/home/hetalkenaudekar/stagin4/cont-sdk-fresh/src/trading.ts:83)
- [src/relayerClient.ts](/home/hetalkenaudekar/stagin4/cont-sdk-fresh/src/relayerClient.ts:12)
- [src/context.ts](/home/hetalkenaudekar/stagin4/cont-sdk-fresh/src/context.ts:95)
- [intent_reference.md](/home/hetalkenaudekar/stagin4/cont-sdk-fresh/intent_reference.md:1)

Required `cont-sdk-fresh` changes:

- extend `proto/ctm_sequencer.proto` with `intent_version`, `target_kind`, `target_index`,
- update `src/relayerClient.ts` request types,
- replace current relayer signing flow in `src/trading.ts` with final target-bound `v2`,
- stop requiring caller-built `remaining_accounts` for relayer submits,
- keep local account derivation in `src/context.ts` for direct on-chain enqueue helpers,
- add direct enqueue helpers in `src/trading.ts` for the on-chain fallback path,
- document both flows in `README.md` and replace the current `v1`-specific signing notes in `intent_reference.md`.

`cont-sdk-fresh` should present the two write paths clearly:

- relayer intent submit,
- direct on-chain enqueue.

It should not force external users to reverse-engineer when they need local account derivation versus when the relayer owns that derivation.

### Payload Variants That Must Be Covered

The final `v2` path must cover every user-originated queue payload, not only `PerpPlaceOrderV2`.

That includes:

- `PerpPlaceOrderV2`
- `PerpCancelOrder`
- `PerpCancelOrderByClientOrderId`
- `PerpCancelOrderBySlot`
- `PerpCancelAllOrders`
- `PerpCancelAllOrdersBySide`
- `PerpBatchIntent`
- `LiquidityDeposit`
- `LiquidityWithdraw`

Current gaps:

- [execution_queue.rs](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:343) does not treat `PerpCancelOrderBySlot` or `PerpBatchIntent` as user-signed
- liquidity enqueue currently has no user-signature path at all in [execution_queue.rs](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:1573)

These gaps must be closed as part of the `v2` migration.

## Relayer Changes

### Relayer Becomes The Account Builder

The relayer must derive canonical dispatch accounts from:

- static group metadata
- local `MangoAccount` mirror
- signed target
- payload variant

Current relayer derivation work is already started in:

- [main.rs](/home/hetalkenaudekar/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs:3081)
- [main.rs](/home/hetalkenaudekar/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs:3261)
- [main.rs](/home/hetalkenaudekar/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs:3306)
- [main.rs](/home/hetalkenaudekar/stagin4/mng-v4/bin/service-mango-execution-engine/src/main.rs:7837)

Required relayer mirror state:

- static group mirror keyed by `group`
- mango account mirror keyed by `mango_account`

Static group mirror contents:

- token index -> bank
- token index -> oracle
- token index -> fallback oracles
- perp market index -> perp market
- perp market index -> oracle
- perp market index -> bids
- perp market index -> asks
- perp market index -> event queue
- openbook / serum metadata as needed later

Mango account mirror contents:

- active token indices
- active perp market indices
- serum open-orders accounts
- openbook open-orders accounts

### Bootstrap Policy

For an unseen `MangoAccount`:

- do a one-time RPC fetch
- parse the account
- seed the local mirror
- proceed without any further synchronous account RPC for that user

For static group data:

- bootstrap once per group from `getProgramAccounts`
- cache banks, perps, oracles, fallback oracles

### Freshness Model

This design assumes account shape changes only through the relayer path, or that external mutations are out of scope.

Under that assumption:

- a one-time bootstrap is enough,
- ongoing mirror updates can be driven by accepted intents and confirmed queue/account outcomes,
- hot-path RPC stays off.

If external account mutations are later allowed, subscriptions or explicit invalidation must be added.

### V1 / V2 Verification Rules In The Relayer

For `v1`:

- parse caller-supplied `remaining_accounts`
- derive canonical accounts locally
- if they differ, reject the request
- do not rewrite and continue

For `v2`:

- verify the user signature against the final target-bound `v2` message
- derive accounts locally
- ignore caller-supplied `remaining_accounts` for execution
- optionally compare and log mismatch

The direct on-chain SDK path is different:

- there is no relayer,
- there is no server-side derivation,
- the SDK must derive canonical accounts locally and pass them in the instruction.

### Perp Derivation

For perp payloads the relayer derives:

- fixed dispatch prefix
- market account
- bids
- asks
- event queue
- market oracle
- canonical health accounts:
  - banks
  - token oracles
  - perp markets
  - perp oracles
  - serum OOs
  - openbook OOs
  - fallback oracles

### Liquidity Derivation

Liquidity also needs full relayer derivation.

That requires a signed `token_index` target and a canonical builder for:

- bank
- vault / token account path
- bank oracle
- any fallback or auxiliary accounts required by `TokenDeposit` / `TokenWithdraw`

Today liquidity is still account-driven and unsigned. That must change for full `v2`.

## Executor Changes

The executor should continue to operate on `accounts_hash` and derived lane accounts, not on sender-provided account lists.

Required executor rules:

- register dynamic lanes from `relay_intent_accepted.remaining_accounts`
- treat accepted-event accounts as canonical
- continue to match queue items by `accounts_hash`
- continue to auto-drop terminal failures

The executor does not need a new auth model. The user auth change is upstream of it.

What does need to change:

- no executor path should depend on caller-supplied metas surviving unchanged
- any lane-cache or harness ingestion logic must preserve `intent_version`, `target_kind`, and `target_index`

## On-Chain Changes

### User Signature Verification

The program must verify final `v2` messages that include explicit target identity.

Required functions:

- `canonical_user_intent_message_v1(...)`
- `canonical_user_intent_message_v2(...)`

The final `v2` function must hash:

- `group`
- `mango_account`
- `user_owner`
- `queue_item_kind`
- `target_kind`
- `target_index`
- `payload_hash`

It must not hash:

- `accounts_hash`

### Enqueue CTM And Direct

Both paths must verify `v2` first and `v1` second during migration:

- [execution_queue_enqueue_ctm](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:1252)
- [execution_queue_enqueue_direct](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:1422)

For perp intents:

- bind `market_index` from the instruction arg into the `v2` user-intent message
- keep `require_dispatch_market_index(...)` to verify the derived dispatch account really is that market
- keep `execution_queue_enqueue_direct` semantics where the program allocates the sequence itself and enforces the speedbump

For liquidity intents:

- add explicit `token_index` to the enqueue instruction
- add `require_dispatch_token_index(...)`
- include `token_index` in the `v2` user-intent message

### Direct Enqueue Semantics

The SDK docs must describe direct enqueue exactly as the program behaves today.

From [execution_queue_enqueue_direct](/home/hetalkenaudekar/stagin4/mng-v4/programs/mango-v4/src/instructions/execution_queue.rs:1421):

- the instruction still takes `market_index`, `envelope`, and `payload`,
- the program still requires canonical dispatch accounts and validates the market account with `require_dispatch_market_index(...)`,
- the program verifies only the user Ed25519 preinstruction, not the CTM signature,
- the program computes the actual queued sequence itself from `max_seen_sequence + 1`,
- the program forces `min_execute_slot` to be at least `clock.slot + DIRECT_SUBMIT_DELAY_SLOTS`,
- the queued item stores the recomputed `account_hash` from the provided dispatch accounts.

So the SDK should hide or constrain direct-path fields as follows:

- do not expose `sequence` as a caller input,
- either omit `min_execute_slot` or clearly document that it is lower-bounded by the speedbump,
- do expose `expires_at_slot`,
- do expose or derive canonical `remaining_accounts`,
- do not expose CTM signing for this path.

### User-Signed Variant Coverage

On-chain `variant_uses_user_signature(...)` must be expanded to every user-originated payload variant.

That means:

- add `PerpCancelOrderBySlot`
- add `PerpBatchIntent`
- decide and implement user-signature coverage for liquidity variants

### Queue Integrity

The following stays unchanged:

- `payload_hash == envelope.payload_hash`
- derived dispatch accounts hash to `envelope.accounts_hash`
- execute paths still require provided lane accounts to hash to the queued `accounts_hash`

This is the key separation:

- user signs intent semantics and target
- CTM signs derived execution context
- queue/executor dispatch still enforces exact account integrity

## Harness / Observability Changes

The harness and downstream tooling should understand `v2` fields.

Required changes:

- ingest and store `intent_version`
- ingest and store `target_kind`
- ingest and store `target_index`
- display whether accounts were caller-supplied or relayer-derived
- surface both `payload_hash` and `accounts_hash`

This is important when debugging mismatches between:

- signer intent,
- relayer derivation,
- on-chain enqueue,
- executor lane selection.

## Migration Plan

### Phase 1

- add final `v2` spec to clients and relayer
- keep `v1` acceptance in relayer and program
- relayer rejects malformed `v1` instead of attempting repair

### Phase 2

- deploy relayer-side derivation
- update all bots and manual senders to sign final `v2`
- stop depending on sender-built `remaining_accounts`

### Phase 3

- deploy the program update that verifies final target-bound `v2`
- enable liquidity target binding if liquidity intents remain externally submit-able

### Phase 4

- monitor:
  - `v1` vs `v2` volume
  - signature reject rate
  - relayer-derived account mismatches
  - queue execution failures by reason

### Phase 5

- remove `v1`
- remove `market` from the public submit API
- remove `remaining_accounts` from the public submit API
- remove legacy verify paths and legacy helper builders

## Test Matrix

Required tests:

- user-intent `v2` hash changes when `market_index` changes
- user-intent `v2` hash changes when `token_index` changes
- user-intent `v2` hash does not change when `accounts_hash` changes
- relayer derives canonical perp accounts for a bootstrapped user with no hot-path RPC
- relayer derives canonical liquidity accounts for a bootstrapped user with no hot-path RPC
- `v1` mismatch is rejected, not repaired
- `v2` mismatch in caller hints is ignored and logged
- on-chain enqueue accepts valid `v2` and valid legacy `v1`
- on-chain enqueue rejects wrong target index even if payload is otherwise valid
- executor lane matching still works with relayer-derived accounts
- harness and event sinks preserve `intent_version`, `target_kind`, `target_index`, and `accounts_hash`

## Summary

The real `v2` migration is not just "drop `accounts_hash` from the user signature".

It is:

- move account derivation to the relayer,
- add explicit signed target identity,
- keep CTM/envelope binding to derived `accounts_hash`,
- keep executor matching by `accounts_hash`,
- keep on-chain verification of exact dispatch accounts,
- migrate all callers away from sender-built `remaining_accounts`.
