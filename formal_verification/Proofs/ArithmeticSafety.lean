import MangoState
import QEDGen.Solana
import Mathlib.Tactic.SplitIfs
import Mathlib.Tactic.Linarith

open QEDGen.Solana
open MangoV4

/-!
# Arithmetic Safety Proofs

Properties AS-1 through AS-4 from SPEC.md
-/

namespace MangoV4.Proofs.ArithmeticSafety

/-!
## AS-1: Index Bounds

deposit_index and borrow_index are always >= INDEX_START after any valid update.
-/

theorem index_update_preserves_minimum (b b' : Bank) (new_di new_bi : I80F48) (fees : Nat)
    (h_update : indexUpdateTransition b new_di new_bi fees = some b') :
    b'.deposit_index >= b.deposit_index ∧ b'.borrow_index >= b.borrow_index := by
  unfold indexUpdateTransition at h_update
  split_ifs at h_update with h_mono
  · cases h_update; exact h_mono

theorem index_update_preserves_wellformed_indices (b b' : Bank) (new_di new_bi : I80F48) (fees : Nat)
    (h_wf : Bank.wellFormed b)
    (h_update : indexUpdateTransition b new_di new_bi fees = some b') :
    b'.deposit_index >= INDEX_START ∧ b'.borrow_index >= INDEX_START := by
  have h_mono := index_update_preserves_minimum b b' new_di new_bi fees h_update
  unfold Bank.wellFormed at h_wf
  constructor
  · linarith [h_wf.1, h_mono.1]
  · linarith [h_wf.2.1, h_mono.2]

/-!
## AS-2: Deposit requires positive amount (prevents zero-amount exploits)
-/

theorem deposit_amount_positive (b b' : Bank) (amount : Nat)
    (h : depositTransition b amount = some b') :
    amount > 0 := by
  unfold depositTransition at h
  split_ifs at h with h_pos
  · exact h_pos

/-!
## AS-3: Withdraw amount bounded by vault
-/

theorem withdraw_bounded_by_vault (b b' : Bank) (amount : Nat)
    (h : withdrawTransition b amount = some b') :
    amount <= b.vault_balance := by
  unfold withdrawTransition at h
  split_ifs at h with h_cond
  · exact h_cond.2

/-!
## AS-4: Order ID Uniqueness (seq_num strictly increasing)
-/

theorem place_order_increments_seq_num (m : PerpMarket) :
    (placeOrderTransition m).seq_num = m.seq_num + 1 := by
  unfold placeOrderTransition
  rfl

theorem place_order_seq_num_strictly_increases (m : PerpMarket) :
    (placeOrderTransition m).seq_num > m.seq_num := by
  have := place_order_increments_seq_num m
  omega

theorem two_consecutive_orders_have_different_seq_nums (m : PerpMarket) :
    let m1 := placeOrderTransition m
    let m2 := placeOrderTransition m1
    m2.seq_num ≠ m1.seq_num ∧ m2.seq_num ≠ m.seq_num := by
  have h1 := place_order_increments_seq_num m
  have h2 := place_order_increments_seq_num (placeOrderTransition m)
  constructor <;> omega

/-!
## Loss socialization bounded by total deposits
-/

theorem socialize_loss_requires_deposits (b : Bank) (loss : Nat)
    (h : socializeLossTransition b loss ≠ none) :
    b.indexed_deposits > 0 := by
  unfold socializeLossTransition at h
  split_ifs at h with h_cond
  · exact h_cond.1
  · contradiction

theorem socialize_loss_bounded (b : Bank) (loss : Nat)
    (h : socializeLossTransition b loss ≠ none) :
    (loss : Int) <= b.indexed_deposits * b.deposit_index := by
  unfold socializeLossTransition at h
  split_ifs at h with h_cond
  · exact h_cond.2
  · contradiction

end MangoV4.Proofs.ArithmeticSafety
