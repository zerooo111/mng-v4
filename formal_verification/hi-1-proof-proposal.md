# HI-1 Proof Proposal: Health Ordering Invariant

**Property**: For all MangoAccounts at any point in time,
```
init_health(a) <= liq_end_health(a) <= maint_health(a)
```

**Status**: Open — the only unproven property among 31 in the Mango V4 formal specification.

**Source**: `programs/mango-v4/src/health/cache.rs:76-77`

---

## 1. Why This Property Matters

The health ordering invariant is the structural backbone of the entire liquidation system. If it were violated:

- An account could be liquidatable (maint < 0) yet simultaneously have positive init health, allowing it to open new positions while being liquidated.
- An account could reach liq_end_health >= 0 (ending liquidation) while init_health < 0, creating a state where the account exits liquidation but cannot perform any user operations.
- The protocol's solvency model breaks down: positions could be opened that the liquidation engine cannot unwind.

Every other verified property (LP-1 through LP-3, HI-3, HI-4) depends on this ordering holding. Without HI-1, the liquidation state machine proofs are vacuously true — they prove phase ordering and recovery behavior, but the conditions triggering those phases would be incoherent.

---

## 2. Anatomy of the Health Computation

Health for a MangoAccount is computed as a sum over three independent contribution sources:

```
health(account, ht) = Σ token_contrib(i, ht)
                    + Σ spot_oo_contrib(j, ht)
                    + Σ perp_contrib(k, ht)     [folded into token balances]
```

where `ht ∈ {Init, Maint, LiquidationEnd}`.

### 2.1 Token Health Contribution

For each token position with native balance `b` and token info `T`:

```
token_contrib(T, ht) = b * weighted_price(T, ht, sign(b))
```

where:

```
                     ┌ asset_weight(T, ht) * asset_price(T, ht)   if b >= 0
weighted_price(T) = ─┤
                     └ liab_weight(T, ht)  * liab_price(T, ht)    if b < 0
```

### 2.2 Weight Selection

| Health Type       | Asset Weight               | Liab Weight                |
|-------------------|----------------------------|----------------------------|
| Init              | `init_scaled_asset_weight` | `init_scaled_liab_weight`  |
| LiquidationEnd    | `init_asset_weight`        | `init_liab_weight`         |
| Maint             | `maint_asset_weight`       | `maint_liab_weight`        |

### 2.3 Price Selection

| Health Type       | Asset Price              | Liab Price               |
|-------------------|--------------------------|--------------------------|
| Init              | `min(oracle, stable)`    | `max(oracle, stable)`    |
| LiquidationEnd    | `oracle`                 | `oracle`                 |
| Maint             | `oracle`                 | `oracle`                 |

### 2.4 Weight Hierarchy (Enforced On-Chain)

The following invariants are enforced in `Bank::valid()` (`state/bank.rs:413-416`):

```
0 <= init_asset_weight <= maint_asset_weight
0 <= maint_liab_weight <= init_liab_weight
```

And by construction of scaled weights (`state/bank.rs:1248-1282`):

```
init_scaled_asset_weight <= init_asset_weight
init_liab_weight         <= init_scaled_liab_weight
```

Combining:

```
Asset weights:  init_scaled <= init <= maint        (lower = more conservative)
Liab weights:   maint <= init <= init_scaled        (higher = more conservative)
```

### 2.5 Perp Health Contribution

Perps contribute to health via unsettled PnL folded into the settle token balance:

```
effective_balance(settle_token, ht) = spot_balance + Σ perp_health_unsettled_pnl(p, ht)
```

where:

```
perp_health_unsettled_pnl(p, ht) = overall_weight(ht) * unweighted_hupnl(p, ht)
```

The `unweighted_hupnl` considers worst-case order execution scenarios and applies base weights per health type, following the same weight hierarchy as tokens.

### 2.6 Spot Open Orders Contribution

Spot open orders contribute a worst-case health impact by considering two scenarios:
1. All reserved base is converted to quote
2. All reserved quote is converted to base

The minimum (worst case) is taken, using health-type-appropriate weights and prices.

---

## 3. Proof Decomposition

The proof of HI-1 decomposes into three layers, each building on the previous.

### Layer 1: Single-Token Ordering (the core lemma)

**Statement**: For a single token position with balance `b` and info `T` satisfying the weight hierarchy:

```
token_contrib(T, Init) <= token_contrib(T, LiquidationEnd) <= token_contrib(T, Maint)
```

**Approach**: Split into two cases based on the sign of `b`.

#### Case A: `b >= 0` (asset position)

```
token_contrib(T, ht) = b * asset_weight(T, ht) * asset_price(T, ht)
```

Since `b >= 0`, the contribution is non-decreasing in both weight and price. We need:

```
init_scaled_asset_weight * min(oracle, stable)        -- Init
<=  init_asset_weight * oracle                        -- LiquidationEnd
<=  maint_asset_weight * oracle                       -- Maint
```

**Init <= LiquidationEnd** requires proving:

```
init_scaled * min(oracle, stable) <= init * oracle
```

This holds because:
- `init_scaled <= init` (by weight hierarchy)
- `min(oracle, stable) <= oracle` (by definition of min)
- Both factors are non-negative (weights >= 0, prices >= 0)
- Therefore the product is non-decreasing in both factors

**LiquidationEnd <= Maint** requires proving:

```
init * oracle <= maint * oracle
```

This holds because:
- `init <= maint` (by weight hierarchy)
- `oracle >= 0`
- Product is non-decreasing in the weight factor

#### Case B: `b < 0` (liability position)

```
token_contrib(T, ht) = b * liab_weight(T, ht) * liab_price(T, ht)
```

Since `b < 0`, the contribution is non-increasing in both weight and price (larger weight * price = more negative contribution). We need the **reverse** ordering of `weight * price`:

```
init_scaled_liab_weight * max(oracle, stable)         -- Init (most negative)
>=  init_liab_weight * oracle                         -- LiquidationEnd
>=  maint_liab_weight * oracle                        -- Maint (least negative)
```

which, after multiplying by negative `b`, gives:

```
b * init_scaled * max(o,s) <= b * init * o <= b * maint * o
```

**Init <= LiquidationEnd** requires:

```
b * (init_scaled * max(oracle, stable)) <= b * (init * oracle)
```

Since `b < 0`, this flips to:

```
init_scaled * max(oracle, stable) >= init * oracle
```

This holds because:
- `init_scaled >= init` (liab weight hierarchy: init <= init_scaled)
- `max(oracle, stable) >= oracle` (by definition of max)
- Both factors are non-negative
- Product of larger factors >= product of smaller factors

**LiquidationEnd <= Maint** requires:

```
b * (init * oracle) <= b * (maint * oracle)
```

Since `b < 0`, this flips to:

```
init * oracle >= maint * oracle
```

This holds because:
- `init >= maint` (liab weight hierarchy: maint <= init)
- `oracle >= 0`

### Layer 2: Summation Preserves Ordering

**Statement**: If for every token `i`:

```
token_contrib(T_i, Init) <= token_contrib(T_i, LiquidationEnd) <= token_contrib(T_i, Maint)
```

then:

```
Σ token_contrib(T_i, Init) <= Σ token_contrib(T_i, LiquidationEnd) <= Σ token_contrib(T_i, Maint)
```

**Approach**: Straightforward induction on the list of tokens. The sum of pointwise-ordered terms preserves the ordering. This is the standard `List.foldl_le_foldl` pattern.

**Lean 4 sketch**:

```lean
theorem sum_preserves_ordering (tokens : List TokenHealthInfo)
    (h_all : ∀ t ∈ tokens, weightHierarchyHolds t) :
    computeHealth tokens HealthType.Init ≤
    computeHealth tokens HealthType.LiquidationEnd ∧
    computeHealth tokens HealthType.LiquidationEnd ≤
    computeHealth tokens HealthType.Maint := by
  induction tokens with
  | nil => simp [computeHealth]; exact ⟨le_refl 0, le_refl 0⟩
  | cons t ts ih =>
    -- Use: single-token ordering for t + inductive hypothesis for ts
    -- foldl distributes: foldl f (a + b) xs = foldl f a xs + b
    sorry
```

### Layer 3: Perp and Spot Contributions

The perp and spot open order contributions follow the same pattern:

**Perps**: The `unweighted_hupnl` computation uses base weights and prices that follow the same hierarchy. The `overall_weight` satisfies `init_overall <= maint_overall`. For negative unweighted PnL, the overall weight is 1.0 (no weighting), so ordering is preserved. For positive unweighted PnL, `init_overall * uhupnl <= maint_overall * uhupnl` since `init_overall <= maint_overall` and `uhupnl >= 0`.

**Spot open orders**: The worst-case computation takes the minimum of two scenarios, both using health-type-appropriate weights. Since weights and prices are ordered, the worst-case contributions are also ordered.

---

## 4. Required Lean 4 Infrastructure

### 4.1 New Definitions Needed

```lean
-- Weighted price product for a given health type and balance sign
def weightedPrice (info : TokenHealthInfo) (ht : HealthType) (is_asset : Bool) : Nat :=
  if is_asset then
    info.assetWeight ht * info.effectivePrice ht true
  else
    info.liabWeight ht * info.effectivePrice ht false
```

### 4.2 Key Lemmas Needed

**Lemma 1 — Product monotonicity** (from Mathlib or manual):
```lean
lemma Nat.mul_le_mul_of_le_of_le {a b c d : Nat}
    (hab : a ≤ b) (hcd : c ≤ d) : a * c ≤ b * d
```

**Lemma 2 — Negation reverses ordering for Int**:
```lean
lemma Int.neg_mul_le_neg_mul {a b : Int} {x : Int}
    (hx : x < 0) (hab : a ≥ b) : x * a ≤ x * b
```

**Lemma 3 — min/max bounds**:
```lean
lemma min_le_left (a b : Nat) : min a b ≤ a
lemma le_max_left (a b : Nat) : a ≤ max a b
```

**Lemma 4 — foldl ordering preservation**:
```lean
lemma List.foldl_add_le_foldl_add (f g : α → Int) (xs : List α) (h0 : a0 ≤ b0)
    (h : ∀ x ∈ xs, f x ≤ g x) :
    xs.foldl (fun acc x => acc + f x) a0 ≤ xs.foldl (fun acc x => acc + g x) b0
```

### 4.3 Mathlib Dependencies

The proof will likely require:
- `Mathlib.Order.Basic` — for `le_trans`, `le_refl`
- `Mathlib.Algebra.Order.Ring.Lemmas` — for `mul_le_mul` variants
- `Mathlib.Data.Int.Order` — for Int multiplication ordering with sign
- `Mathlib.Data.List.Basic` — for foldl induction
- `Mathlib.Tactic.Linarith` — for closing linear arithmetic goals

All of these are already transitively available through our current Mathlib dependency.

---

## 5. Proof Outline in Lean 4

### 5.1 Single-Token Asset Case

```lean
theorem asset_contrib_ordered (info : TokenHealthInfo)
    (h_wh : weightHierarchyHolds info)
    (h_bal : info.balance_native >= 0) :
    tokenHealthContribution info HealthType.Init ≤
    tokenHealthContribution info HealthType.LiquidationEnd ∧
    tokenHealthContribution info HealthType.LiquidationEnd ≤
    tokenHealthContribution info HealthType.Maint := by
  unfold tokenHealthContribution
  simp [h_bal]
  unfold TokenHealthInfo.assetWeight TokenHealthInfo.effectivePrice
  -- Goal reduces to:
  -- balance * init_scaled * min(oracle, stable) ≤ balance * init * oracle
  -- ∧ balance * init * oracle ≤ balance * maint * oracle
  --
  -- Since balance >= 0, sufficient to show:
  -- init_scaled * min(oracle, stable) ≤ init * oracle
  -- ∧ init * oracle ≤ maint * oracle
  --
  -- First: init_scaled ≤ init ∧ min(o,s) ≤ o → product ≤ product
  -- Second: init ≤ maint → init * oracle ≤ maint * oracle
  constructor
  · apply Int.mul_le_mul_of_nonneg_left _ h_bal
    apply Nat.mul_le_mul h_wh.2.1 (min_le_left _ _)
  · apply Int.mul_le_mul_of_nonneg_left _ h_bal
    apply Nat.mul_le_mul (le_trans h_wh.2.1 h_wh.1) (le_refl _)
```

### 5.2 Single-Token Liability Case

```lean
theorem liab_contrib_ordered (info : TokenHealthInfo)
    (h_wh : weightHierarchyHolds info)
    (h_bal : info.balance_native < 0) :
    tokenHealthContribution info HealthType.Init ≤
    tokenHealthContribution info HealthType.LiquidationEnd ∧
    tokenHealthContribution info HealthType.LiquidationEnd ≤
    tokenHealthContribution info HealthType.Maint := by
  unfold tokenHealthContribution
  -- balance < 0, so we're in the liab branch
  -- contrib = balance * liab_weight * liab_price
  --
  -- Since balance < 0, larger (weight * price) → smaller (more negative) contribution
  -- Init uses init_scaled_liab * max(o,s) — largest product → most negative
  -- LiqEnd uses init_liab * oracle — middle product
  -- Maint uses maint_liab * oracle — smallest product → least negative
  --
  -- Need: balance * (init_scaled * max(o,s)) ≤ balance * (init * o) ≤ balance * (maint * o)
  -- Since balance < 0, this requires:
  --   init_scaled * max(o,s) >= init * o >= maint * o
  sorry -- Detailed Int multiplication with negative factor
```

### 5.3 Summation Theorem

```lean
theorem health_ordering (tokens : List TokenHealthInfo)
    (h_all : ∀ t ∈ tokens, weightHierarchyHolds t) :
    computeHealth tokens HealthType.Init ≤
    computeHealth tokens HealthType.LiquidationEnd ∧
    computeHealth tokens HealthType.LiquidationEnd ≤
    computeHealth tokens HealthType.Maint := by
  unfold computeHealth
  -- Induction on tokens, using foldl_add_le_foldl_add
  -- Base: 0 ≤ 0
  -- Step: if acc_init ≤ acc_liqend and contrib_init(t) ≤ contrib_liqend(t)
  --        then acc_init + contrib_init(t) ≤ acc_liqend + contrib_liqend(t)
  sorry
```

---

## 6. Complexity Assessment

| Component | Difficulty | Estimated Theorems | Notes |
|---|---|---|---|
| Asset case (Layer 1A) | Medium | 3-4 | Product monotonicity with Nat multiplication |
| Liability case (Layer 1B) | Hard | 4-5 | Int multiplication sign reversal, Nat↔Int coercion |
| Single-token combined | Easy | 1 | Case split on balance sign |
| Summation (Layer 2) | Medium | 2-3 | foldl induction, needs custom lemma |
| Perp ordering (Layer 3) | Hard | 5-6 | Worst-case min, overall weight, base weights |
| Spot OO ordering (Layer 3) | Hard | 4-5 | Worst-case min of two scenarios |
| **Total** | | **~20-24** | |

The hardest parts are:

1. **Liability case Int arithmetic**: Multiplying a negative `Int` balance by `Nat` weights/prices requires careful coercion handling. The key insight (`negative * larger >= negative * smaller` flips to `larger ≥ smaller`) needs `Int.mul_le_mul_of_nonpos_left` or equivalent.

2. **Perp worst-case ordering**: The `unweighted_hupnl` function takes `min` of two order-execution scenarios. Proving that the minimum of Init-weighted scenarios is ≤ the minimum of Maint-weighted scenarios requires showing both individual scenarios are ordered, then using monotonicity of `min`.

3. **Nat↔Int coercion boundaries**: Weights and prices are `Nat` but balances are `Int`. The `tokenHealthContribution` function computes `Int * Nat * Nat` which Lean auto-coerces. Proving monotonicity across this boundary requires `Int.ofNat_le` and `Int.mul_le_mul` variants.

---

## 7. Proposed Test Strategy (Empirical Validation)

While the full formal proof is being developed, we can gain confidence through exhaustive property-based testing.

### 7.1 QuickCheck-Style Tests

```rust
#[test]
fn health_ordering_fuzz() {
    use proptest::prelude::*;
    
    proptest!(|(
        balance in -1_000_000i64..1_000_000,
        oracle_price in 1u64..1_000_000,
        stable_price in 1u64..1_000_000,
        init_asset in 0u64..10_000,
        maint_asset in 0u64..10_000,
        init_liab in 0u64..10_000,
        maint_liab in 0u64..10_000,
    )| {
        // Enforce weight hierarchy
        let init_asset = init_asset.min(maint_asset);
        let maint_liab = maint_liab.min(init_liab);
        let init_scaled_asset = init_asset;  // worst case: no scaling
        let init_scaled_liab = init_liab;    // worst case: no scaling
        
        let init_h = compute_contrib(balance, init_scaled_asset, init_scaled_liab,
            oracle_price.min(stable_price), oracle_price.max(stable_price));
        let liqend_h = compute_contrib(balance, init_asset, init_liab,
            oracle_price, oracle_price);
        let maint_h = compute_contrib(balance, maint_asset, maint_liab,
            oracle_price, oracle_price);
        
        prop_assert!(init_h <= liqend_h, "init > liqend: {} > {}", init_h, liqend_h);
        prop_assert!(liqend_h <= maint_h, "liqend > maint: {} > {}", liqend_h, maint_h);
    });
}
```

### 7.2 Boundary Value Tests

Test the property at critical boundaries:

| Scenario | Balance | Oracle | Stable | Expected |
|---|---|---|---|---|
| Zero balance | 0 | any | any | init = liqend = maint = 0 |
| Pure asset, oracle = stable | +100 | 50 | 50 | init ≤ liqend ≤ maint (weight ordering only) |
| Pure asset, oracle < stable | +100 | 40 | 60 | init uses 40, liqend/maint use 40 |
| Pure asset, oracle > stable | +100 | 60 | 40 | init uses 40, liqend/maint use 60 |
| Pure liab, oracle = stable | -100 | 50 | 50 | init ≤ liqend ≤ maint (weight ordering only) |
| Pure liab, oracle < stable | -100 | 40 | 60 | init uses 60 (max), liqend/maint use 40 |
| Max weight scaling | +100 | 50 | 50 | init_scaled ≤ init ≤ maint |
| Zero init weight | +100 | 50 | 50 | init = 0 ≤ liqend ≤ maint |
| Equal init/maint weights | +100 | 50 | 50 | init ≤ liqend = maint |

### 7.3 Multi-Token Summation Tests

- 2 assets: both positive → sum ordering holds
- 2 liabs: both negative → sum ordering holds
- Mixed: 1 asset + 1 liab → sum ordering holds
- Adversarial: large asset canceling large liab → ordering at the margin
- Many tokens (100+): random mix → statistical confidence

### 7.4 Integration Test with Actual Health Cache

```rust
#[test]
fn health_ordering_integration() {
    // Set up a MangoAccount with various positions
    // Compute health for all three types via the real HealthCache
    // Assert ordering
    let health_cache = create_test_health_cache();
    let init = health_cache.health(HealthType::Init);
    let liqend = health_cache.health(HealthType::LiquidationEnd);
    let maint = health_cache.health(HealthType::Maint);
    assert!(init <= liqend, "init={} > liqend={}", init, liqend);
    assert!(liqend <= maint, "liqend={} > maint={}", liqend, maint);
}
```

---

## 8. Implementation Plan

### Phase 1: Infrastructure (1-2 hours)

1. Add `weightedPrice` and `weightedPriceOrdered` definitions to `MangoState/Health.lean`
2. Add helper lemmas for `Nat.mul_le_mul`, `Int.mul_le_mul_of_nonneg_left`, `Int.mul_le_mul_of_nonpos_left`
3. Add `min_le_left`, `le_max_left` lemmas (likely already in Mathlib)
4. Add `List.foldl_add_le_foldl_add` lemma

### Phase 2: Single-Token Proof (2-3 hours)

1. Prove `asset_weighted_price_ordered`: Init ≤ LiqEnd ≤ Maint for asset weighted prices
2. Prove `liab_weighted_price_ordered`: Init ≥ LiqEnd ≥ Maint for liab weighted prices (reversed)
3. Prove `asset_contrib_ordered`: for b >= 0
4. Prove `liab_contrib_ordered`: for b < 0
5. Combine into `single_token_contrib_ordered`: for all b

### Phase 3: Summation Proof (1-2 hours)

1. Prove `foldl_add_preserves_le`: if each addend is ordered, the sum is ordered
2. Prove `computeHealth_ordered`: the full health ordering theorem
3. Clean up and connect to SPEC.md property

### Phase 4: Perp and Spot Extensions (3-4 hours, optional)

1. Model `PerpHealthInfo` with base weights and overall weights
2. Prove worst-case min ordering for perp contributions
3. Model `SpotReserved` worst-case scenarios
4. Prove spot open order contribution ordering
5. Combine all three contribution sources

### Phase 5: Testing (1-2 hours, parallel with proof work)

1. Implement QuickCheck-style property tests in Rust
2. Run boundary value tests
3. Run integration tests with real HealthCache

---

## 9. Risk Assessment

| Risk | Likelihood | Mitigation |
|---|---|---|
| Int/Nat coercion blocks omega/linarith | High | Use explicit `have` bindings with `Int.ofNat_nonneg`, `Int.coe_nat_le` |
| Perp worst-case min ordering is unprovable in simplified model | Medium | Extend model to include full PerpInfo; or prove token-only ordering first |
| Spot OO two-scenario min is complex | Medium | Prove ordering for each scenario independently, then use min monotonicity |
| Lean 4 performance on large proof terms | Low | Use `set_option maxHeartbeats` and factor into small lemmas |
| Weight scaling invalidates simplified model | Low | Scaling only makes init weights more conservative → ordering still holds |

---

## 10. Expected Outcome

After completing Phases 1-3, we will have a machine-checked proof that:

> For any list of token positions where each position's weight hierarchy holds, the health computed with Init weights/prices is less than or equal to health computed with LiquidationEnd weights/prices, which is less than or equal to health computed with Maint weights/prices.

This covers the core token contribution ordering, which is the primary source of the invariant. The perp and spot extensions (Phase 4) would complete the proof for the full health computation including all three contribution sources.

Combined with the 30 already-verified properties, completing HI-1 would bring the Mango V4 formal verification to **31/31 properties verified** — full coverage of the specification.
