import MangoState.Types
import MangoState.Bank
import MangoState.Account
import MangoState.PerpMarket
import QEDGen.Solana

open QEDGen.Solana
open MangoV4

namespace MangoV4

/-!
# Health Computation Model

Models the health computation for MangoAccounts.
Health determines whether operations are allowed and whether liquidation can proceed.

Key invariant HI-1: init_health <= liq_end_health <= maint_health
-/

-- Prices used in health computation
structure Prices where
  oracle : Nat
  stable : Nat
  deriving Repr

-- Token info for health computation
structure TokenHealthInfo where
  maint_asset_weight        : Nat
  init_asset_weight         : Nat
  init_scaled_asset_weight  : Nat
  maint_liab_weight         : Nat
  init_liab_weight          : Nat
  init_scaled_liab_weight   : Nat
  prices                    : Prices
  balance_native            : Int
  deriving Repr

-- Select weight based on health type
def TokenHealthInfo.assetWeight (info : TokenHealthInfo) (ht : HealthType) : Nat :=
  match ht with
  | HealthType.Init => info.init_scaled_asset_weight
  | HealthType.LiquidationEnd => info.init_asset_weight
  | HealthType.Maint => info.maint_asset_weight

def TokenHealthInfo.liabWeight (info : TokenHealthInfo) (ht : HealthType) : Nat :=
  match ht with
  | HealthType.Init => info.init_scaled_liab_weight
  | HealthType.LiquidationEnd => info.init_liab_weight
  | HealthType.Maint => info.maint_liab_weight

-- Select price based on health type and balance sign
def TokenHealthInfo.effectivePrice (info : TokenHealthInfo) (ht : HealthType) (is_asset : Bool) : Nat :=
  match ht with
  | HealthType.Init =>
    if is_asset then min info.prices.oracle info.prices.stable
    else max info.prices.oracle info.prices.stable
  | HealthType.LiquidationEnd => info.prices.oracle
  | HealthType.Maint => info.prices.oracle

-- Health contribution of a single token
def tokenHealthContribution (info : TokenHealthInfo) (ht : HealthType) : Int :=
  if info.balance_native >= 0 then
    let w := info.assetWeight ht
    let p := info.effectivePrice ht true
    info.balance_native * w * p
  else
    let w := info.liabWeight ht
    let p := info.effectivePrice ht false
    info.balance_native * w * p

-- Total health = sum of all token contributions (simplified, no perps)
def computeHealth (tokens : List TokenHealthInfo) (ht : HealthType) : Int :=
  tokens.foldl (fun acc info => acc + tokenHealthContribution info ht) 0

/-!
## Health Ordering Property

Theorem HI-1: For any set of token positions, init_health <= maint_health.

This follows from the weight hierarchy:
  init_asset_weight <= maint_asset_weight  (for assets, smaller weight = smaller contribution)
  maint_liab_weight <= init_liab_weight    (for liabs, larger weight = more negative contribution)

For Init specifically, scaled weights are <= unscaled, and prices are adjusted conservatively.
-/

-- Weight hierarchy predicate
def weightHierarchyHolds (info : TokenHealthInfo) : Prop :=
  info.init_asset_weight <= info.maint_asset_weight ∧
  info.init_scaled_asset_weight <= info.init_asset_weight ∧
  info.maint_liab_weight <= info.init_liab_weight ∧
  info.init_liab_weight <= info.init_scaled_liab_weight ∧
  info.prices.stable >= 0 ∧
  info.prices.oracle >= 0

/-!
## Liquidation State Machine Transitions
-/

-- Liquidation can begin when maint health < 0
def canLiquidate (maint_health : Int) : Prop :=
  maint_health < 0

-- Liquidation ends when liq_end_health >= 0
def shouldRecoverFromLiquidation (liq_end_health : Int) : Prop :=
  liq_end_health >= 0

-- Liquidation phases
inductive LiquidationPhase
  | Phase1_CancelOrders
  | Phase2_ReducePositions
  | Phase3_Bankruptcy
  deriving Repr, DecidableEq

-- Phase 1 complete: no open orders remain
def phase1Complete (has_spot_open_orders has_perp_open_orders : Bool) : Prop :=
  ¬has_spot_open_orders ∧ ¬has_perp_open_orders

-- Phase 2 complete: no base positions, no spot liquidation opportunities
def phase2Complete (has_perp_base has_spot_liqs : Bool) : Prop :=
  ¬has_perp_base ∧ ¬has_spot_liqs

-- Phase ordering check: can enter Phase 2 only if Phase 1 done
def canEnterPhase2 (has_spot_oo has_perp_oo : Bool) : Prop :=
  phase1Complete has_spot_oo has_perp_oo

-- Phase ordering check: can enter Phase 3 only if Phase 2 done
def canEnterPhase3 (has_perp_base has_spot_liqs has_spot_oo has_perp_oo : Bool) : Prop :=
  phase1Complete has_spot_oo has_perp_oo ∧
  phase2Complete has_perp_base has_spot_liqs

/-!
## HI-1: Health Ordering from Weight Hierarchy

For a single token, if the weight hierarchy holds, then:
  init contribution <= liq_end contribution <= maint contribution
This follows from init weights being more conservative than maint weights.
-/

-- Single-token health contribution ordering (simplified model)
-- Uses the fact that for assets: init_weight <= maint_weight → init_contrib <= maint_contrib
-- And for liabs: maint_weight <= init_weight → init_contrib <= maint_contrib (more negative)
def healthContributionOrdered (info : TokenHealthInfo) : Prop :=
  tokenHealthContribution info HealthType.Init ≤
  tokenHealthContribution info HealthType.Maint

/-!
## HI-2: Health Non-Degradation

For user-initiated operations (not liquidation), if the account is not in a health
region, then post_init_health >= pre_init_health.
-/

-- Model a health-checked transition: operation only succeeds if health doesn't degrade
def healthCheckedTransition (pre_health post_health : Int) (in_health_region : Bool)
    : Prop :=
  in_health_region = true ∨ post_health >= pre_health

/-!
## HI-5: Health Region Atomicity

If in_health_region is set during a transaction, it must be cleared by tx end.
-/

-- Model: a transaction is a sequence of operations. Health region must be balanced.
def healthRegionBalanced (began : Bool) (ended : Bool) : Prop :=
  began = true → ended = true

/-!
## VC-4: Utilization Bound

After any borrow operation, vault >= deposits * min_vault_to_deposits_ratio.
-/

def utilizationBoundHolds (vault_balance : Nat) (native_deposits : Nat)
    (min_ratio_bps : Nat) : Prop :=
  vault_balance * 10000 >= native_deposits * min_ratio_bps

/-!
## IC-2: Interest Accounting

For all index updates with time delta dt:
borrow_index' = borrow_index * (1 + borrow_rate * dt)
deposit_index' = deposit_index * (1 + deposit_rate * dt)
where deposit_rate = borrow_rate * utilization * (1 - fee_fraction).
-/

-- Simplified: new indices must be at least old indices scaled by (1 + rate * dt)
-- We verify the structural property that indices grow proportionally
def interestAccrualValid (old_di new_di old_bi new_bi : I80F48)
    (dt : Nat) (borrow_rate deposit_rate : I80F48) : Prop :=
  dt > 0 →
  borrow_rate >= 0 →
  deposit_rate >= 0 →
  new_bi >= old_bi ∧ new_di >= old_di

end MangoV4
