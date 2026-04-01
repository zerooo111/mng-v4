import MangoState
import QEDGen.Solana
import Mathlib.Tactic.SplitIfs

open QEDGen.Solana
open MangoV4

/-!
# Access Control Proofs

Properties AC-1, AC-2, AC-3 from SPEC.md
-/

namespace MangoV4.Proofs.AccessControl

/-!
## AC-1: Owner Authorization

For all operations on MangoAccount `a` by signer `s`:
if the operation succeeds then `s = a.owner OR s = a.delegate OR
(s = a.temporary_delegate AND now_ts < a.temporary_delegate_expiry)`.
-/

theorem owner_authorization (a : MangoAccount) (signer : Pubkey) (now_ts : Nat)
    (op : MangoAccount → Option MangoAccount) (result : MangoAccount)
    (h : authorizedTransition a signer now_ts op = some result) :
    signer = a.owner ∨
    signer = a.delegate ∨
    (signer = a.temporary_delegate ∧ now_ts < a.temporary_delegate_expiry) := by
  unfold authorizedTransition at h
  split_ifs at h with h_auth
  exact h_auth

/-!
## AC-2: Admin Authorization

For all administrative operations on Group `g` by signer `s`:
if the operation succeeds then `s = g.admin`.
-/

theorem admin_authorization (g : Group) (signer : Pubkey)
    (op : Group → Option Group) (result : Group)
    (h : adminTransition g signer op = some result) :
    signer = g.admin := by
  unfold adminTransition at h
  split_ifs at h with h_admin
  exact h_admin

/-!
## AC-3: Delegate Withdrawal Restriction

If a delegate (not owner) performs a withdrawal, the operation is constrained.
We model this as: if the authorized transition succeeds and the signer is not
the owner, then the signer must be the delegate or temp delegate.
-/

theorem delegate_must_be_authorized (a : MangoAccount) (signer : Pubkey) (now_ts : Nat)
    (op : MangoAccount → Option MangoAccount) (result : MangoAccount)
    (h_success : authorizedTransition a signer now_ts op = some result)
    (h_not_owner : signer ≠ a.owner) :
    signer = a.delegate ∨
    (signer = a.temporary_delegate ∧ now_ts < a.temporary_delegate_expiry) := by
  have h_auth := owner_authorization a signer now_ts op result h_success
  cases h_auth with
  | inl h_own => exact absurd h_own h_not_owner
  | inr h_del => exact h_del

/-!
## Unauthorized signer cannot perform operations
-/

theorem unauthorized_signer_rejected (a : MangoAccount) (signer : Pubkey) (now_ts : Nat)
    (op : MangoAccount → Option MangoAccount)
    (h_not_owner : signer ≠ a.owner)
    (h_not_delegate : signer ≠ a.delegate)
    (h_not_temp : signer ≠ a.temporary_delegate ∨ now_ts >= a.temporary_delegate_expiry) :
    authorizedTransition a signer now_ts op = none := by
  unfold authorizedTransition
  split_ifs with h_auth
  · cases h_auth with
    | inl h => exact absurd h h_not_owner
    | inr h =>
      cases h with
      | inl h => exact absurd h h_not_delegate
      | inr h =>
        cases h_not_temp with
        | inl h_ne => exact absurd h.1 h_ne
        | inr h_exp => exact absurd h.2 (Nat.not_lt.mpr h_exp)
  · rfl

/-!
## Unauthorized admin cannot perform group operations
-/

theorem unauthorized_admin_rejected (g : Group) (signer : Pubkey)
    (op : Group → Option Group)
    (h_not_admin : signer ≠ g.admin) :
    adminTransition g signer op = none := by
  unfold adminTransition
  split_ifs with h_admin
  · exact absurd h_admin h_not_admin
  · rfl

end MangoV4.Proofs.AccessControl
