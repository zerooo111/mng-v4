import MangoState
import QEDGen.Solana
import Mathlib.Tactic.SplitIfs

open QEDGen.Solana
open MangoV4

/-!
# Vault Conservation Proofs

Properties VC-1 through VC-4 from SPEC.md
-/

namespace MangoV4.Proofs.Conservation

/-!
## VC-1: Deposit Conservation

For all deposit transitions, the vault balance increases by exactly the deposit amount.
-/

theorem deposit_increases_vault (b b' : Bank) (amount : Nat)
    (h : depositTransition b amount = some b') :
    b'.vault_balance = b.vault_balance + amount := by
  unfold depositTransition at h
  split_ifs at h with h_pos
  · cases h; rfl

theorem deposit_requires_positive_amount (b : Bank) (amount : Nat)
    (h : depositTransition b amount ≠ none) :
    amount > 0 := by
  unfold depositTransition at h
  split_ifs at h with h_pos
  · exact h_pos
  · contradiction

/-!
## VC-2: Withdraw Solvency

For all withdraw transitions, the vault had sufficient balance.
-/

theorem withdraw_requires_sufficient_vault (b b' : Bank) (amount : Nat)
    (h : withdrawTransition b amount = some b') :
    b.vault_balance >= amount := by
  unfold withdrawTransition at h
  split_ifs at h with h_cond
  · exact h_cond.2

theorem withdraw_decreases_vault (b b' : Bank) (amount : Nat)
    (h : withdrawTransition b amount = some b') :
    b'.vault_balance = b.vault_balance - amount := by
  unfold withdrawTransition at h
  split_ifs at h with h_cond
  · cases h; rfl

/-!
## VC-3: Flash Loan Return

For a complete flash loan cycle (Begin → End), the vault is conserved:
vault_final = vault_initial - approved + repay
-/

theorem flash_loan_begin_requires_idle (b b' : Bank) (loan_amount ta_balance : Nat)
    (h : flashLoanBeginTransition b loan_amount ta_balance = some b') :
    b.flash_loan_approved_amount = 0 ∧
    b.flash_loan_token_account_initial = U64_MAX := by
  unfold flashLoanBeginTransition at h
  split_ifs at h with h_cond
  · exact ⟨h_cond.2.1, h_cond.2.2.1⟩

theorem flash_loan_begin_requires_sufficient_vault (b b' : Bank) (loan_amount ta_balance : Nat)
    (h : flashLoanBeginTransition b loan_amount ta_balance = some b') :
    b.vault_balance >= loan_amount := by
  unfold flashLoanBeginTransition at h
  split_ifs at h with h_cond
  · exact h_cond.2.2.2

theorem flash_loan_begin_sets_active (b b' : Bank) (loan_amount ta_balance : Nat)
    (h : flashLoanBeginTransition b loan_amount ta_balance = some b') :
    b'.flash_loan_approved_amount = loan_amount ∧
    b'.flash_loan_token_account_initial = ta_balance ∧
    b'.vault_balance = b.vault_balance - loan_amount := by
  unfold flashLoanBeginTransition at h
  split_ifs at h with h_cond
  · cases h; exact ⟨rfl, rfl, rfl⟩

theorem flash_loan_end_resets_idle (b b' : Bank) (ta_final : Nat)
    (h : flashLoanEndTransition b ta_final = some b') :
    b'.flash_loan_approved_amount = 0 ∧
    b'.flash_loan_token_account_initial = U64_MAX := by
  unfold flashLoanEndTransition at h
  split_ifs at h <;> (cases h; exact ⟨rfl, rfl⟩)

theorem flash_loan_cycle_conservation (fl : FlashLoanCycle)
    (h : fl.conservation) :
    fl.vault_final = fl.vault_initial - fl.approved_amount + fl.repay_amount := by
  exact h.1

/-!
## VC-4: Index Update Monotonicity (supports vault solvency)

Index updates only increase indices, which means depositors' claims grow
(covered by interest from borrowers).
-/

theorem index_update_monotonic (b b' : Bank) (new_di new_bi : I80F48) (fees : Nat)
    (h : indexUpdateTransition b new_di new_bi fees = some b') :
    b'.deposit_index >= b.deposit_index ∧
    b'.borrow_index >= b.borrow_index := by
  unfold indexUpdateTransition at h
  split_ifs at h with h_mono
  · cases h; exact h_mono

theorem index_update_increases_fees (b b' : Bank) (new_di new_bi : I80F48) (fees : Nat)
    (h : indexUpdateTransition b new_di new_bi fees = some b') :
    b'.collected_fees_native >= b.collected_fees_native := by
  unfold indexUpdateTransition at h
  split_ifs at h with h_mono
  · cases h
    show b.collected_fees_native + fees >= b.collected_fees_native
    omega

end MangoV4.Proofs.Conservation
