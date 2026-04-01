import MangoState.Types
import QEDGen.Solana

open QEDGen.Solana
open MangoV4

namespace MangoV4

/-!
# Bank State Model

Models the Bank account which manages a single token's deposits, borrows,
interest rates, and vault.
-/

structure Bank where
  group           : Pubkey
  vault           : Pubkey
  mint            : Pubkey
  oracle          : Pubkey
  token_index     : TokenIndex

  -- Indices (monotonically non-decreasing, >= INDEX_START)
  deposit_index   : I80F48
  borrow_index    : I80F48
  indexed_deposits: I80F48
  indexed_borrows : I80F48

  -- Fees (all >= 0)
  loan_origination_fee_rate : Nat  -- scaled
  liquidation_fee           : Nat
  platform_liquidation_fee  : Nat
  collected_fees_native     : Nat  -- monotonically non-decreasing
  collected_liquidation_fees: Nat  -- monotonically non-decreasing

  -- Vault balance
  vault_balance   : Nat

  -- Risk weights
  maint_asset_weight : Nat
  init_asset_weight  : Nat
  maint_liab_weight  : Nat
  init_liab_weight   : Nat

  -- Operational modes
  reduce_only     : ReduceOnly
  force_close     : Bool

  -- Borrow limits
  min_vault_to_deposits_ratio : Nat  -- fraction scaled by 10000
  net_borrow_limit_per_window_quote : Int  -- -1 = disabled

  -- Flash loan state (transient)
  flash_loan_approved_amount : Nat
  flash_loan_token_account_initial : Nat  -- u64::MAX when idle

  -- Dust accumulator
  dust : Nat
  deriving Repr

-- Well-formedness predicate for Bank
def Bank.wellFormed (b : Bank) : Prop :=
  b.deposit_index >= INDEX_START ∧
  b.borrow_index >= INDEX_START ∧
  b.init_asset_weight <= b.maint_asset_weight ∧
  b.maint_liab_weight <= b.init_liab_weight

-- Native deposits = deposit_index * indexed_deposits
def Bank.native_deposits (b : Bank) : Int :=
  b.deposit_index * b.indexed_deposits

-- Native borrows = borrow_index * indexed_borrows
def Bank.native_borrows (b : Bank) : Int :=
  b.borrow_index * b.indexed_borrows

-- Flash loan is idle
def Bank.flashLoanIdle (b : Bank) : Prop :=
  b.flash_loan_approved_amount = 0 ∧
  b.flash_loan_token_account_initial = U64_MAX

/-!
## Token Position (within MangoAccount)
-/

structure TokenPosition where
  indexed_position : Int  -- > 0 deposit, < 0 borrow
  token_index      : TokenIndex
  in_use_count     : Nat
  deriving Repr

def TokenPosition.isActive (p : TokenPosition) : Prop :=
  p.token_index ≠ TokenIndex.MAX

def TokenPosition.isDeposit (p : TokenPosition) : Prop :=
  p.indexed_position >= 0

def TokenPosition.isBorrow (p : TokenPosition) : Prop :=
  p.indexed_position < 0

-- Native value of position given the bank
def TokenPosition.native (p : TokenPosition) (b : Bank) : Int :=
  if p.indexed_position >= 0
  then p.indexed_position * b.deposit_index
  else p.indexed_position * b.borrow_index

def TokenPosition.isInUse (p : TokenPosition) : Prop :=
  p.in_use_count > 0

/-!
## Deposit Transition
-/

-- Deposit into bank: increases vault, increases indexed_deposits (or reduces indexed_borrows)
def depositTransition (b : Bank) (amount : Nat)
    : Option Bank :=
  if amount > 0 then
    some {
      b with
      vault_balance := b.vault_balance + amount
      -- Simplified: in full model this adjusts indexed_deposits/borrows
      indexed_deposits := b.indexed_deposits + amount
    }
  else none

-- Withdraw from bank: decreases vault, must have sufficient vault balance
def withdrawTransition (b : Bank) (amount : Nat)
    : Option Bank :=
  if amount > 0 ∧ b.vault_balance >= amount then
    some {
      b with
      vault_balance := b.vault_balance - amount
    }
  else none

-- Flash loan begin: set active state, transfer from vault
def flashLoanBeginTransition (b : Bank) (loan_amount : Nat) (token_account_balance : Nat)
    : Option Bank :=
  if loan_amount > 0 ∧
     b.flash_loan_approved_amount = 0 ∧
     b.flash_loan_token_account_initial = U64_MAX ∧
     b.vault_balance >= loan_amount then
    some {
      b with
      flash_loan_approved_amount := loan_amount
      flash_loan_token_account_initial := token_account_balance
      vault_balance := b.vault_balance - loan_amount
    }
  else none

-- Flash loan end: repay and reset state
def flashLoanEndTransition (b : Bank) (token_account_final : Nat)
    : Option Bank :=
  if b.flash_loan_token_account_initial ≠ U64_MAX then
    let repay := if token_account_final > b.flash_loan_token_account_initial
                 then token_account_final - b.flash_loan_token_account_initial
                 else 0
    some {
      b with
      vault_balance := b.vault_balance + repay
      flash_loan_approved_amount := 0
      flash_loan_token_account_initial := U64_MAX
    }
  else none

-- Index update transition (interest accrual)
def indexUpdateTransition (b : Bank) (new_deposit_index new_borrow_index : I80F48)
    (new_fees : Nat) : Option Bank :=
  if new_deposit_index >= b.deposit_index ∧
     new_borrow_index >= b.borrow_index then
    some {
      b with
      deposit_index := new_deposit_index
      borrow_index := new_borrow_index
      collected_fees_native := b.collected_fees_native + new_fees
    }
  else none

-- Loss socialization: reduce deposit index proportionally
def socializeLossTransition (b : Bank) (loss : Nat) : Option Bank :=
  if b.indexed_deposits > 0 ∧
     loss <= b.indexed_deposits * b.deposit_index then
    let new_index := b.deposit_index - (loss : Int) / b.indexed_deposits
    some {
      b with
      deposit_index := new_index
    }
  else none

end MangoV4
