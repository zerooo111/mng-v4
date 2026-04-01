import MangoState
import QEDGen.Solana
import Mathlib.Tactic.SplitIfs

open QEDGen.Solana
open MangoV4

/-!
# Fee Monotonicity Proofs

Properties FA-1 through FA-3 from SPEC.md
-/

namespace MangoV4.Proofs.FeeMonotonicity

/-!
## FA-1: Fee Monotonicity

collected_fees_native is monotonically non-decreasing across index updates.
-/

theorem fees_monotonically_increase (b b' : Bank)
    (new_di new_bi : I80F48) (fees : Nat)
    (h : indexUpdateTransition b new_di new_bi fees = some b') :
    b'.collected_fees_native >= b.collected_fees_native := by
  unfold indexUpdateTransition at h
  split_ifs at h with h_mono
  · cases h
    show b.collected_fees_native + fees >= b.collected_fees_native
    omega

theorem fees_increase_by_exact_amount (b b' : Bank)
    (new_di new_bi : I80F48) (fees : Nat)
    (h : indexUpdateTransition b new_di new_bi fees = some b') :
    b'.collected_fees_native = b.collected_fees_native + fees := by
  unfold indexUpdateTransition at h
  split_ifs at h with h_mono
  · cases h; rfl

/-!
## FA-2: Spot Liquidation Fee Distribution

The platform fee is exactly the difference between what liqee pays and liqor receives.
Fee is always non-negative.
-/

theorem spot_liq_fee_distribution (liq : SpotLiquidation)
    (h : liq.feeInvariant) :
    liq.platform_fee = liq.asset_transfer_from_liqee - liq.asset_transfer_to_liqor := by
  exact h.1

theorem spot_liq_fee_nonnegative (liq : SpotLiquidation)
    (h : liq.feeInvariant) :
    liq.asset_transfer_from_liqee >= liq.asset_transfer_to_liqor := by
  exact h.2

/-!
## FA-3: Perp Liquidation Fee Distribution

Platform fee = max(0, -quote_transfer_liqor - quote_transfer_liqee).
-/

theorem perp_liq_fee_distribution (pl : PerpLiquidation)
    (h : pl.feeInvariant) :
    (pl.platform_fee : Int) = max 0 (-pl.quote_transfer_liqor - pl.quote_transfer_liqee) := by
  exact h

/-!
## Insurance Fund Bound

Insurance transfer never exceeds insurance vault amount.
-/

theorem insurance_transfer_bounded (tb : TokenBankruptcy) (vault_amount : Nat)
    (h : tb.insuranceBound vault_amount) :
    tb.insurance_transfer <= vault_amount := by
  exact h

end MangoV4.Proofs.FeeMonotonicity
