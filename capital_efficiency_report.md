# Capital Efficiency Report

Date: 2026-03-25

## Executive Summary

Mango already behaves very differently for perps versus spot orderbooks.

- Perps already have close to "margin used on fill, not on placement" semantics. `perp_place_order` does not withdraw settle tokens when an order is posted; it only updates the book and then recomputes health using the perp position plus open orders (`programs/mango-v4/src/instructions/perp_place_order.rs:67`, `programs/mango-v4/src/instructions/perp_place_order.rs:116`, `programs/mango-v4/src/health/cache.rs:489`, `programs/mango-v4/src/health/cache.rs:545`, `programs/mango-v4/src/health/cache.rs:1182`).
- Spot on Serum3/OpenBook v2 does not. The payer-side vault balance is reduced at order placement, Mango debits the payer token position immediately, and the open orders account becomes the escrow location for those tokens (`programs/mango-v4/src/instructions/serum3_place_order.rs:190`, `programs/mango-v4/src/instructions/serum3_place_order.rs:338`, `programs/mango-v4/src/instructions/serum3_place_order.rs:364`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:186`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:320`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:342`).

So the design goal is already mostly achieved for perps, but not for external spot orderbooks.

The minimum safe implementation path is:

1. Do not attempt this for Serum3/OpenBook v2 first.
2. If you want more capital efficiency, change perp init-health treatment of maker open orders only.
3. Pair that with short expiries and automatic/keeper-driven cancellation based on a stricter "full-reservation" view of risk.

My recommendation is a perps-only rollout with a configurable open-order reservation ratio, starting much higher than 5%. I would start around 15% to 25%, not 5%.

## 1. What Mango Already Does Today

### Perps

Perp order placement already avoids an upfront token debit.

- `perp_place_order` computes pre-health, inserts/matches the order, recomputes perp info, and checks post-health. There is no settle-token vault withdrawal in this path (`programs/mango-v4/src/instructions/perp_place_order.rs:67`, `programs/mango-v4/src/instructions/perp_place_order.rs:116`, `programs/mango-v4/src/instructions/perp_place_order.rs:127`).
- Health accounts for perp open orders by folding `bids_base_lots` and `asks_base_lots` into worst-case execution scenarios inside `PerpInfo::unweighted_health_unsettled_pnl()` (`programs/mango-v4/src/health/cache.rs:479`, `programs/mango-v4/src/health/cache.rs:569`).
- Positive perp pnl is already haircut by `init_overall_asset_weight` / `maint_overall_asset_weight`, so Mango does not fully trust unrealized perp gains as collateral (`programs/mango-v4/src/state/perp_market.rs:183`, `programs/mango-v4/src/health/cache.rs:545`, `programs/mango-v4/src/health/cache.rs:552`).
- Taker matches are recorded before event consumption, so fill risk is reflected promptly in the account state (`programs/mango-v4/src/state/orderbook/book.rs:215`, `programs/mango-v4/src/state/mango_account_components.rs:478`).

Implication:

- For perps, Mango already lets the same spot collateral support quotes across multiple markets.
- What limits reuse is not escrow, but health. The account is charged for worst-case open-order exposure at placement time.

This means current perp design is already capital efficient relative to a "reserve full quote notional in cash" model, but it is not as permissive as "only 5% margin while resting".

### Spot on Serum3/OpenBook v2

Spot is materially different.

- The payer bank/vault is checked for available funds before placement (`programs/mango-v4/src/instructions/serum3_place_order.rs:215`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:202`).
- After the CPI, Mango measures the vault delta and debits the payer token position immediately via `apply_vault_difference()` (`programs/mango-v4/src/instructions/serum3_place_order.rs:338`, `programs/mango-v4/src/instructions/serum3_place_order.rs:364`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:320`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:342`, `programs/mango-v4/src/instructions/serum3_place_order.rs:562`).
- Health still models reserved OO funds conservatively, but that is on top of physical escrow, not instead of it (`programs/mango-v4/src/health/cache.rs:242`, `programs/mango-v4/src/health/cache.rs:370`, `programs/mango-v4/src/health/cache.rs:1127`).

Implication:

- On external spot venues, Mango cannot make capital stay fully unused while the order is resting unless it stops pre-funding the external venue.
- That is not a small change. It is a different execution architecture.

## 2. Minimum Surface Area To Implement A More Capital-Efficient Design

### A. Minimum viable change: perps only

If the desired change is "resting maker orders consume only a fraction of their current health charge", the narrowest code surface is:

- Add 1 new risk parameter to `PerpMarket`, for example `open_order_init_margin_ratio`.
- Thread it through `perp_create_market` and `perp_edit_market`.
- Modify only the perp health formula so init health charges a fraction of maker open-order exposure, while keeping actual filled exposure unchanged.

Concrete file surface:

- `programs/mango-v4/src/state/perp_market.rs`
- `programs/mango-v4/src/instructions/perp_create_market.rs`
- `programs/mango-v4/src/instructions/perp_edit_market.rs`
- `programs/mango-v4/src/health/cache.rs`
- tests in `programs/mango-v4/tests/cases/` and/or `programs/mango-v4/src/health/test.rs`

The least invasive implementation is in `PerpInfo::unweighted_health_unsettled_pnl()`: scale only the maker-order component (`bids_base_lots` / `asks_base_lots`) in init health, while leaving:

- current filled base position at 100%
- taker fills at 100%
- negative pnl at 100%

That keeps matching, event processing, settlement, and liquidation instructions unchanged.

### B. Small-but-important companion change

If init health only charges 5% of resting exposure, you need a second view of risk for cancellation. Otherwise accounts can quote far more notional than they can survive if multiple markets trade at once.

I would add a second check:

- `full_open_order_health`: existing current formula, unchanged
- `entry_open_order_health`: discounted formula used only for order admission

Then keepers or liquidators use `full_open_order_health` to decide when to cancel all orders aggressively.

That still keeps the change relatively local:

- health cache helper for "discounted" vs "full" perp open-order contribution
- a read path or instruction path for keeper-triggered cancellation

### C. What is not minimal

Doing this for Serum3/OpenBook v2 is not small surface area.

Reason:

- those venues require actual token escrow into the external market vault at placement (`programs/mango-v4/src/instructions/serum3_place_order.rs:364`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:342`)
- Mango's current accounting, deposit-limit logic, and borrow tracking assume that behavior (`programs/mango-v4/src/instructions/serum3_place_order.rs:345`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:377`, `programs/mango-v4/src/state/bank.rs:981`)

To get "only 5% used until fill" for spot, you would need one of:

- an internal Mango-controlled order layer that does not pre-fund the external venue
- JIT posting/canceling through a keeper just before likely execution
- a different external venue model that supports credit-style maker quoting

That is not a localized health tweak.

## 3. Best Tunable Parameters

### Primary parameter: resting-order reservation ratio

Suggested new perp parameter:

- `open_order_init_margin_ratio` in `[0, 1]`

Meaning:

- `1.0` = today's behavior
- `0.2` = only 20% of resting maker open-order exposure counts at admission
- `0.05` = the aggressive target in your prompt

Recommendation:

- Start at `0.15` to `0.25`
- Do not start at `0.05`

Reason:

- a 5% reservation lets an account quote roughly 20x the notional it can actually absorb
- if correlated fills happen across markets in one short interval, liquidation/cancel can be too late

### Price-band controls

These already exist and are important anti-phantom-liquidity controls.

- Spot has `oracle_price_band` on both Serum3 and OpenBook v2 markets (`programs/mango-v4/src/state/serum3_market.rs:31`, `programs/mango-v4/src/state/openbook_v2_market.rs:27`)
- Placement enforces those bands so absurd bids/asks far from oracle cannot sit on-book just to game potential/liquidity accounting (`programs/mango-v4/src/instructions/serum3_place_order.rs:415`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:391`)
- Perps also have posting price limits enforced before posting the remainder (`programs/mango-v4/src/state/orderbook/book.rs:261`)

Recommendation:

- tighten these when open-order reservation is relaxed
- low reservation ratio and loose price bands should not coexist

### Expiry / time-to-live

Today:

- perps support `time_in_force` and lazy expiry removal (`programs/mango-v4/src/state/orderbook/order.rs:20`, `programs/mango-v4/src/state/orderbook/nodes.rs:230`, `programs/mango-v4/src/state/orderbook/book.rs:272`)
- OpenBook v2 placement already sends `expiry_timestamp` to the external venue (`programs/mango-v4/src/instructions/openbook_v2_place_order.rs:632`, `programs/mango-v4/src/instructions/openbook_v2_place_order.rs:673`)
- Serum3 path currently uses `max_ts: i64::MAX`, so no TTL is applied (`programs/mango-v4/src/instructions/serum3_place_order.rs:248`)

Recommendation:

- use short default TTLs for any discounted-margin perp quoting
- add a TTL / `max_ts` input to Serum3 placement if you want parity there

### Market-level kill switches

Existing controls:

- `reduce_only`
- `force_close`

These already gate force-cancel behavior in liquidation paths (`programs/mango-v4/src/state/perp_market.rs:175`, `programs/mango-v4/src/state/serum3_market.rs:18`, `programs/mango-v4/src/state/openbook_v2_market.rs:19`, `programs/mango-v4/src/instructions/perp_liq_force_cancel_orders.rs:27`, `programs/mango-v4/src/instructions/serum3_liq_force_cancel_orders.rs:105`, `programs/mango-v4/src/instructions/openbook_v2_liq_force_cancel_orders.rs:117`).

Recommendation:

- when introducing lower resting margin, operational playbooks should use `force_close` faster

## 4. Preventing Spoofed / Phantom Liquidity

The core tradeoff is simple:

- lower resting margin increases quote density and market-maker capital efficiency
- it also increases the gap between displayed liquidity and actually absorbable liquidity

Main failure modes:

- one account quotes many markets with the same collateral and gets hit on several at once
- displayed size is economically impossible to honor once correlated fills arrive
- keepers cancel too slowly, especially during outages or congestion

The best mitigations are:

1. Discount only maker resting orders, not filled or taker exposure.
2. Use a nonzero floor. I would keep at least 10% to 15%.
3. Enforce tighter price bands when reservation ratio is lower.
4. Require short TTLs.
5. Add keeper-side cancellation based on the non-discounted risk view.
6. Cap aggregate discounted open-order notional per account or per settle token.

That last point is important. A single ratio is not enough by itself. You also want an absolute cap such as:

- max discounted maker notional as a multiple of account equity
- max discounted maker notional per settle token
- max discounted maker notional per market tier

Without an absolute cap, very small reservation ratios create "infinite quoting until cancelled" incentives.

## 5. Mechanisms To Auto-Cancel / Expire When Underlying Liquidity Disappears

### What already exists

- Perp orders can expire via TIF, and expired orders are removed lazily when the book is touched (`programs/mango-v4/src/state/orderbook/nodes.rs:240`, `programs/mango-v4/src/state/orderbook/book.rs:92`, `programs/mango-v4/src/state/orderbook/book.rs:272`)
- Execution queue payloads for perps can carry expiries and are prevalidated before dispatch (`programs/mango-v4/src/instructions/execution_queue.rs:460`, `programs/mango-v4/src/instructions/execution_queue.rs:514`)
- Liquidation paths already force-cancel perp/Serum/OpenBook orders when the account is unhealthy, frozen, or the market is in force close (`programs/mango-v4/src/instructions/perp_liq_force_cancel_orders.rs:27`, `programs/mango-v4/src/instructions/serum3_liq_force_cancel_orders.rs:105`, `programs/mango-v4/src/instructions/openbook_v2_liq_force_cancel_orders.rs:117`)

### What does not exist on-chain by itself

There is no autonomous background process inside the program. If "underlying liquidity disappears" means:

- oracle stale
- external venue disconnected
- top-of-book depth vanishes
- hedge venue spread blows out

then some actor must submit the cancel transaction.

So the right architecture is:

- on-chain: cheap predicates and cancel instructions
- off-chain keeper: watches venue quality and submits cancel-all

### Best practical keeper triggers

For perps:

- cancel if `full_open_order_health < X`
- cancel if oracle is stale or confidence widens
- cancel if expected hedge slippage on external venues exceeds threshold
- cancel if basis between Mango and hedge venue exceeds threshold

For spot:

- cancel if external market depth falls below required hedge size
- cancel if oracle-to-book deviation exceeds configured band
- cancel if bank borrow utilization or net-borrow headroom deteriorates sharply

### One specific improvement worth making

Serum3 currently hardcodes no expiry at placement (`max_ts = i64::MAX`).

Adding a `max_ts` or TTL field to Mango's Serum3 order instruction is a moderate, worthwhile change because it:

- reduces stale resting orders
- makes keeper dependence less acute
- reduces phantom displayed liquidity during venue disconnects

This is independent of the larger capital-efficiency redesign.

## 6. Recommended Design

### Phase 1: perps only

Implement:

- `PerpMarket.open_order_init_margin_ratio`
- discounted maker open-order treatment in init health only
- keeper cancellation based on today's full-reservation health
- short TIF defaults for MM flow

Do not change:

- actual filled exposure accounting
- taker-fill accounting
- settlement logic
- liquidation mechanics

### Phase 2: optional hardening

Add:

- absolute cap on discounted resting notional per account
- separate ratios by market tier
- observability: discounted health, full-reservation health, resting notional, cancellation reason

### Phase 3: spot, only if you want a different execution architecture

If you want the same capital to quote many spot markets without prefunding each venue, the right answer is not a small tweak to `serum3_place_order` / `openbook_v2_place_order`.

It is a new architecture:

- internal intent/quote layer
- JIT external posting
- or a venue model that supports credit rather than prefunded escrow

## Final Recommendation

The present Mango design already does most of what you want for perps, and almost none of it for external spot.

If the goal is fast progress with minimum code surface and acceptable risk:

- do this only for perps
- add a discounted resting-order init margin ratio
- keep full-risk accounting for cancellations and monitoring
- start at 15% to 25%, not 5%
- combine it with short expiries and aggressive keeper cancellation

If the goal is true cross-market shared capital for spot MM quoting, that is not a risk-parameter change. It is a different execution model.
