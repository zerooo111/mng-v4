import MangoState
import QEDGen.Solana
import Mathlib.Tactic.SplitIfs

open QEDGen.Solana
open MangoV4

/-!
# Health Invariant Proofs

Properties HI-1 through HI-5 from SPEC.md
-/

namespace MangoV4.Proofs.HealthInvariants

/-!
## HI-3: Liquidation Entry

Liquidation can only begin when maintenance health is negative.
-/

theorem liquidation_requires_negative_maint_health (maint_health : Int)
    (h : canLiquidate maint_health) :
    maint_health < 0 := by
  exact h

theorem positive_maint_health_prevents_liquidation (maint_health : Int)
    (h : maint_health >= 0) :
    ¬ canLiquidate maint_health := by
  unfold canLiquidate
  omega

/-!
## HI-4: Liquidation Recovery

When liq_end_health >= 0, being_liquidated is cleared.
-/

theorem recovery_clears_liquidation_flag (a : MangoAccount) (liq_end_health : Int)
    (h_liq : a.being_liquidated = true)
    (h_health : liq_end_health >= 0) :
    (MangoAccount.maybeRecoverFromLiquidation a liq_end_health).being_liquidated = false := by
  unfold MangoAccount.maybeRecoverFromLiquidation
  split_ifs with h_cond
  · rfl
  · exfalso; apply h_cond; exact ⟨h_liq, h_health⟩

theorem no_recovery_when_unhealthy (a : MangoAccount) (liq_end_health : Int)
    (h_liq : a.being_liquidated = true)
    (h_health : liq_end_health < 0) :
    (MangoAccount.maybeRecoverFromLiquidation a liq_end_health).being_liquidated = true := by
  unfold MangoAccount.maybeRecoverFromLiquidation
  split_ifs with h_cond
  · exfalso; omega
  · exact h_liq

theorem non_liquidated_account_unchanged (a : MangoAccount) (liq_end_health : Int)
    (h_not_liq : a.being_liquidated = false) :
    MangoAccount.maybeRecoverFromLiquidation a liq_end_health = a := by
  unfold MangoAccount.maybeRecoverFromLiquidation
  split_ifs with h_cond
  · exfalso; simp [h_not_liq] at h_cond
  · rfl

/-!
## LP-1: Liquidation Phase Ordering

Phase 2 requires Phase 1 complete. Phase 3 requires both Phase 1 and 2 complete.
-/

theorem phase2_requires_no_open_orders
    (has_spot_oo has_perp_oo : Bool)
    (h : canEnterPhase2 has_spot_oo has_perp_oo) :
    has_spot_oo = false ∧ has_perp_oo = false := by
  unfold canEnterPhase2 phase1Complete at h
  constructor
  · cases has_spot_oo with
    | false => rfl
    | true => exact absurd rfl h.1
  · cases has_perp_oo with
    | false => rfl
    | true => exact absurd rfl h.2

theorem phase3_requires_no_positions
    (has_perp_base has_spot_liqs has_spot_oo has_perp_oo : Bool)
    (h : canEnterPhase3 has_perp_base has_spot_liqs has_spot_oo has_perp_oo) :
    has_spot_oo = false ∧ has_perp_oo = false ∧
    has_perp_base = false ∧ has_spot_liqs = false := by
  unfold canEnterPhase3 phase1Complete phase2Complete at h
  obtain ⟨⟨h1, h2⟩, ⟨h3, h4⟩⟩ := h
  refine ⟨?_, ?_, ?_, ?_⟩
  · cases has_spot_oo with
    | false => rfl
    | true => exact absurd rfl h1
  · cases has_perp_oo with
    | false => rfl
    | true => exact absurd rfl h2
  · cases has_perp_base with
    | false => rfl
    | true => exact absurd rfl h3
  · cases has_spot_liqs with
    | false => rfl
    | true => exact absurd rfl h4

theorem cannot_enter_phase2_with_open_orders :
    ¬ canEnterPhase2 true true := by
  unfold canEnterPhase2 phase1Complete
  intro ⟨h, _⟩
  exact h rfl

theorem cannot_enter_phase3_with_perp_base :
    ¬ canEnterPhase3 true false false false := by
  unfold canEnterPhase3 phase1Complete phase2Complete
  intro ⟨_, h3, _⟩
  exact h3 rfl

/-!
## HI-2: Health Non-Degradation

For user-initiated operations, health must not degrade unless in health region.
-/

theorem health_checked_transition_preserves_or_region
    (pre_health post_health : Int) (in_health_region : Bool)
    (h : healthCheckedTransition pre_health post_health in_health_region) :
    in_health_region = true ∨ post_health >= pre_health := by
  exact h

theorem health_non_degradation_outside_region
    (pre_health post_health : Int)
    (h : healthCheckedTransition pre_health post_health false) :
    post_health >= pre_health := by
  unfold healthCheckedTransition at h
  cases h with
  | inl h => contradiction
  | inr h => exact h

/-!
## HI-5: Health Region Atomicity

If health region began, it must end within the same transaction.
-/

theorem health_region_must_end_if_began
    (began ended : Bool)
    (h : healthRegionBalanced began ended)
    (h_began : began = true) :
    ended = true := by
  exact h h_began

theorem health_region_not_began_is_trivially_balanced
    (ended : Bool) :
    healthRegionBalanced false ended := by
  unfold healthRegionBalanced
  intro h
  contradiction

/-!
## VC-4: Utilization Bound

After borrowing, vault must maintain minimum ratio to deposits.
-/

theorem utilization_bound_preserved_no_borrow
    (vault_balance native_deposits min_ratio_bps : Nat)
    (h : utilizationBoundHolds vault_balance native_deposits min_ratio_bps) :
    vault_balance * 10000 >= native_deposits * min_ratio_bps := by
  exact h

/-!
## IC-2: Interest Accounting Validity

Interest accrual produces valid new indices when rates are non-negative.
-/

theorem interest_accrual_preserves_indices
    (old_di new_di old_bi new_bi : I80F48)
    (dt : Nat) (borrow_rate deposit_rate : I80F48)
    (h : interestAccrualValid old_di new_di old_bi new_bi dt borrow_rate deposit_rate)
    (h_dt : dt > 0) (h_br : borrow_rate >= 0) (h_dr : deposit_rate >= 0) :
    new_bi >= old_bi ∧ new_di >= old_di := by
  exact h h_dt h_br h_dr

end MangoV4.Proofs.HealthInvariants
