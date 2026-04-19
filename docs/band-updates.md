# `PerpReplaceBand` — atomic cancel + re-quote for makers

## Motivation — fee cost

A market maker refreshing a ±N-level book around the oracle today issues **2 × (1 + N) Solana transactions per refresh**: one `PerpCancelAllOrdersBySide` for each side, plus one `PerpPlaceOrderV2` per level, each paying a full commit + reveal round.

### Fee anatomy per op (v4 devnet, current observed)

| Cost line | `commit_market` | `reveal_execute_market` (one place) |
|---|---|---|
| Base fee (5000 lamports/sig) | 5 000 | 5 000 |
| CU consumed | ~13 000 | ~200 000–400 000 |
| Priority fee (at 0 micro-lamports/CU) | 0 | 0 |
| Priority fee (at 1 µlamports/CU) | ~13 | ~300 |
| Data locked in queue_page until reveal/drop | 48 bytes of `CommitItemV4` × 1 | — |

At a priority-fee tier of 10 µlamports/CU (devnet is happy to take 0; production will need some), a single place round-trip costs roughly **5 313 + 9 000 = ~14 000 lamports = ~0.0000014 SOL**. Cancel is cheaper (~5 300 CU). Round the refresh math:

**Today**, a 10-level × 2-sided quote refresh:

```
2 cancel_all_by_side commits + 2 reveals      = 4 txs
10 place commits + 10 reveals                 = 20 txs
                                              -----
                                              24 txs
```

**With `PerpReplaceBand`** — a single intent that carries (cancel_all + N places):

```
1 replace_band commit + 1 reveal              = 2 txs
```

**12× fewer txs.** At nominal priority-fees this is ~10 000–50 000 lamports saved per refresh; aggregated across a fleet of 20 makers refreshing every 500 ms, it's **~3 SOL/day** that the relayer (or whoever pays the CTM tx fees) no longer burns.

The savings matter because the relayer's `CTM_RELAYER_PAYER_KEYPAIR` pays ALL v4 on-chain fees — bots themselves don't sign the Solana tx. Every tx we eliminate is subtracted from the admin-wallet burn rate.

---

## Design

### New payload variant

```rust
// programs/mango-v4/src/instructions/execution_queue.rs
pub enum QueuePayloadVariant {
    PerpPlaceOrderV2 = 0,
    PerpCancelOrder = 1,
    PerpCancelOrderByClientOrderId = 2,
    PerpCancelAllOrders = 3,
    PerpCancelAllOrdersBySide = 4,
    LiquidityDeposit = 5,
    LiquidityWithdraw = 6,
    PerpReplaceBand = 7,  // NEW
}
```

### Wire format

```
# Outer queue payload header (unchanged)
u8   version = 1
u8   variant = 7 (PerpReplaceBand)
u16  flags   = 0
# Body — length-prefixed list of place entries + cancel semantics flags
u8   cancel_bids     # 0 = skip, 1 = cancel-all-bids before placing
u8   cancel_asks     # 0 = skip, 1 = cancel-all-asks before placing
u8   place_count     # number of (PlaceEntry) that follow, max 16
PlaceEntry × place_count
```

`PlaceEntry` (32 bytes):
```rust
struct PlaceEntry {
    side: u8,             //  0 = Bid, 1 = Ask
    order_type: u8,       //  matches PlaceOrderType
    self_trade: u8,       //  matches SelfTradeBehavior
    reduce_only: u8,
    price_lots: i64,
    max_base_lots: i64,
    client_order_id: u64,
}
```

### Max batch size

Solana tx limit: 1232 bytes. Minus:
- Signatures (~128 B for 1 sig)
- Message header + keys + blockhash (~350 B for our typical reveal tx with 18 accounts)
- Anchor discriminator + RevealArgsV4 framing (~24 B)
- ed25519 pre-ix with user sig (~160 B)
- CU budget ix (~10 B)
- `cancel_bids`, `cancel_asks`, `place_count` (3 B)

Leaves ~557 bytes for `PlaceEntry` array. At 32 B/entry → **17 entries max**. Set the hard cap to **16** (round down, one-entry margin).

### On-chain handler

`programs/mango-v4/src/instructions/perp_replace_band.rs`:

```rust
pub fn perp_replace_band(ctx: &PerpPlaceOrder, ...) -> Result<()> {
    // 1. (optional) cancel_all_orders_by_side(Bid) if cancel_bids
    // 2. (optional) cancel_all_orders_by_side(Ask) if cancel_asks
    // 3. for each PlaceEntry: call the same place_order logic
    //    each place is subject to existing gates:
    //      - inside_price_limit(side, native_price, oracle)
    //      - reduce_only clamp
    //      - free perp_open_order slot
    //      - expiry_timestamp
    //    HARD FAIL the whole batch if any entry fails (atomicity)
}
```

Reuse the already-factored primitives in `state/orderbook/book.rs` — no new matching logic.

Accounts required are the SAME set as `perp_place_order`, so `remaining_accounts` for the reveal ix is unchanged. This means:
- `accounts_hash` logic (flag-OR with reveal fixed accounts) is unchanged
- Dispatch accounts list is unchanged
- `derive_submit_remaining_accounts` in the relayer is unchanged

Only the payload body (variant + entries) differs.

### Relayer — `MarginCheckOp` batch

Today the payload decoder emits one `MarginCheckOp::Place`. For a `PerpReplaceBand` payload, emit:
- `MarginCheckOp::CancelAllBySide { side: Bid }` (if `cancel_bids`)
- `MarginCheckOp::CancelAllBySide { side: Ask }` (if `cancel_asks`)
- One `MarginCheckOp::Place { … }` per `PlaceEntry`

`evaluate_submit_margin_precheck` already iterates `margin_ops` — just extend `MarginCheckOp` with the cancel-all variant and teach `apply_margin_check_ops` to zero out `bids_base_lots` / `asks_base_lots` on the position snapshot. The new enqueue-time `expiry_timestamp` and stable-price `inside_price_limit` checks I added for single-place will fan out automatically over all N places.

Atomicity on margin: apply the cancels FIRST (freeing reserved margin), then the places, then recompute `init_health`. This is the same order the on-chain handler runs, so the margin numbers match.

### Relayer — no change to commit/reveal flow

- `canonical_commit_hash` is variant-agnostic (hashes the opaque payload bytes)
- `hash_dispatch_accounts_for_reveal` is unchanged (same dispatch accounts)
- `build_commit_market_tx` / `build_reveal_execute_tx` — unchanged
- The `CTM_RELAYER_IGNORE_SUPPLIED_REMAINING_ACCOUNTS` flag keeps working

So the reveal worker pipeline doesn't care that the payload is bigger — it just builds a reveal tx with the same accounts and whatever `reveal_entry.payload` the store holds.

### Client SDK (mango-v4 client.ts)

New helper:

```ts
public async perpReplaceBand(
  group: Group,
  mangoAccount: MangoAccount,
  perpMarketIndex: PerpMarketIndex,
  opts: {
    cancelBids: boolean;
    cancelAsks: boolean;
    places: Array<{
      side: PerpOrderSide;
      priceLots: BN;
      maxBaseLots: BN;
      clientOrderId: BN;
      orderType?: PerpOrderType;
      reduceOnly?: boolean;
    }>;
  },
): Promise<{ payload: Buffer; ... }>;
```

Returns the encoded payload ready to pass to the gRPC submit.

---

## Rollout

### Phase 1 — program-side (~1 hour)

1. Add `PerpReplaceBand` enum variant (enum tag must be 7).
2. Add `perp_replace_band.rs` instruction + `accounts_ix/perp_replace_band.rs`.
3. Extend `queue_payload_variant_from_u8` and `queue_item_kind_for_payload_variant` mappings.
4. Wire into `execution_queue_v4_reveal_execute_market` variant dispatch.
5. Rebuild SBF, in-place upgrade (we have the keypair — no address churn).

### Phase 2 — relayer (~1 hour)

1. Extend `MarginCheckOp` with `CancelAllBySide { side }`.
2. Update `apply_margin_check_ops` + `effective_requested_base_lots` helper to handle cancel-all.
3. Update `decode_margin_check_ops` to recognize variant=7, iterate place entries.
4. Existing `expiry_timestamp` and stable-price band checks fan out naturally.
5. Rebuild relayer, restart.

### Phase 3 — client SDK + bots (~1 hour)

1. Add `perpReplaceBand` helper in `ts/client/src/client.ts`.
2. Update the quoter-bot script (`random-sol-usdc-quoter-bot.ts` or similar) to use `perpReplaceBand` every refresh cycle instead of cancel + N places.
3. Measure before/after: tx count, lamports burned, end-to-end refresh latency.

### Phase 4 — monitor

- Watch relayer admin balance burn rate — expect ~12× drop.
- Watch `v4_reveal: queue_root state live_count` — expect smoother (fewer sequences in flight at peak).
- Validate a maker side-by-side: one bot on old flow, one on replace-band, compare quote cadence + fill rate.

---

## Fallbacks and safety

- **Partial-success semantics:** the on-chain handler MUST be all-or-nothing. If entry 7 fails price-band, the whole tx fails; bot retries. Half-replaced quote states are a footgun (maker thinks book X is up, actually book Y is).
- **CU budget:** each place is ~30k CU. 16 places + 2 cancels + housekeeping ≈ 500k CU. Within a 1.4M cap with room for health calc.
- **Compatibility:** variant=7 is additive. Existing variants (0–6) still work. No client is forced to migrate.
- **Downgrade path:** if a maker still emits 11 individual intents, everything works exactly as before. No behavior regression for conservative bots.

---

## Open decisions

1. **Should cancel_bids / cancel_asks be separate flags or always-both?** Current market-maker patterns usually replace both sides together. Separate flags cost 0 bytes (both fit in a single `u8` if we want) but add flexibility. Recommend: separate flags, easy.
2. **Ordering within a refresh** — place-inside-first or place-outside-first? Doesn't affect correctness, affects the price/time priority during the brief (sub-slot) window after cancel. Recommend: place in user-supplied order (let bot decide).
3. **Fee discount?** Since replace_band does more work per tx, Mango could argue a bulk fee discount (lower per-lot taker fee). Out of scope for this patch; can come later.

---

## If you don't want to build this right now

The current system works. Queue is healthy, reveals are 100% successful, spreads are tight. The cost case is real but not pressing — at devnet volumes, priority fees are 0 and base fees are dust. Revisit when:
- Admin wallet burn becomes a visible cost line
- Priority fees on mainnet are non-zero
- Bots start complaining about quote-refresh latency
- Or someone actually uses the 2-second refresh window as an arb opportunity

None of those are current pain points.
