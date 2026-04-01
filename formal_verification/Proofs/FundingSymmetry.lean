import MangoState
import QEDGen.Solana
import Mathlib.Tactic.SplitIfs
import Mathlib.Tactic.Ring

open QEDGen.Solana
open MangoV4

/-!
# Perp Funding Symmetry Proofs

Properties PF-1 through PF-3 from SPEC.md
-/

namespace MangoV4.Proofs.FundingSymmetry

/-!
## PF-1: Funding Delta Equality

Both long_funding and short_funding change by the same delta.
-/

theorem funding_update_symmetric (m m' : PerpMarket)
    (funding_rate : Int) (oracle_price : Nat) (time_delta : Nat)
    (h : fundingUpdateTransition m funding_rate oracle_price time_delta = some m') :
    m'.long_funding - m.long_funding = m'.short_funding - m.short_funding := by
  unfold fundingUpdateTransition at h
  split_ifs at h with h_cond
  · cases h; ring

theorem funding_delta_is_identical (m m' : PerpMarket)
    (funding_rate : Int) (oracle_price : Nat) (time_delta : Nat)
    (h : fundingUpdateTransition m funding_rate oracle_price time_delta = some m') :
    let delta := (oracle_price : Int) * m.base_lot_size * funding_rate * time_delta
    m'.long_funding = m.long_funding + delta ∧
    m'.short_funding = m.short_funding + delta := by
  unfold fundingUpdateTransition at h
  split_ifs at h with h_cond
  · cases h; exact ⟨rfl, rfl⟩

/-!
## PF-2: Funding Rate Bounds

The funding rate is clamped to [min_funding, max_funding].
-/

theorem funding_rate_bounded (m m' : PerpMarket)
    (funding_rate : Int) (oracle_price : Nat) (time_delta : Nat)
    (h : fundingUpdateTransition m funding_rate oracle_price time_delta = some m') :
    funding_rate >= m.min_funding ∧ funding_rate <= m.max_funding := by
  unfold fundingUpdateTransition at h
  split_ifs at h with h_cond
  · exact ⟨h_cond.2.2.1, h_cond.2.2.2⟩

/-!
## PF-3: Funding Time Bound

Time delta used in funding calculation is <= 3600 seconds (1 hour).
-/

theorem funding_time_bounded (m m' : PerpMarket)
    (funding_rate : Int) (oracle_price : Nat) (time_delta : Nat)
    (h : fundingUpdateTransition m funding_rate oracle_price time_delta = some m') :
    time_delta <= 3600 := by
  unfold fundingUpdateTransition at h
  split_ifs at h with h_cond
  · exact h_cond.2.1

theorem funding_time_positive (m m' : PerpMarket)
    (funding_rate : Int) (oracle_price : Nat) (time_delta : Nat)
    (h : fundingUpdateTransition m funding_rate oracle_price time_delta = some m') :
    time_delta > 0 := by
  unfold fundingUpdateTransition at h
  split_ifs at h with h_cond
  · exact h_cond.1

/-!
## Perp Loss Socialization Symmetry

When OI > 0, socialized loss is distributed symmetrically to long/short funding.
-/

theorem perp_socialize_loss_symmetric (m m' : PerpMarket) (loss : Int)
    (h : perpSocializeLossTransition m loss = some m')
    (h_oi : m.open_interest > 0) :
    let per_contract := loss / (m.open_interest : Int)
    m'.long_funding = m.long_funding - per_contract ∧
    m'.short_funding = m.short_funding + per_contract := by
  unfold perpSocializeLossTransition at h
  split_ifs at h with h_loss h_has_oi
  · cases h; exact ⟨rfl, rfl⟩

theorem perp_socialize_loss_to_unsocialized_when_no_oi (m m' : PerpMarket) (loss : Int)
    (h : perpSocializeLossTransition m loss = some m')
    (h_oi : m.open_interest = 0) :
    m'.unsocialized_loss = m.unsocialized_loss + loss := by
  unfold perpSocializeLossTransition at h
  split_ifs at h with h_loss h_has_oi
  · exfalso; omega
  · cases h; rfl

end MangoV4.Proofs.FundingSymmetry
