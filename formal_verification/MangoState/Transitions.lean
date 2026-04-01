import MangoState.Types
import MangoState.Bank
import MangoState.Account
import MangoState.PerpMarket
import MangoState.Health
import QEDGen.Solana

open QEDGen.Solana
open MangoV4

namespace MangoV4

/-!
# Composite Transitions

Models multi-account transitions for liquidation, flash loans, etc.
-/

-- Spot liquidation: transfer liab from liqee to liqor, asset from liqee to liqor
structure SpotLiquidation where
  liqee           : MangoAccount
  liqor           : MangoAccount
  liab_bank       : Bank
  asset_bank      : Bank
  liab_transfer   : Nat   -- amount of liab transferred
  asset_transfer_to_liqor : Nat
  asset_transfer_from_liqee : Nat
  platform_fee    : Nat   -- asset_from_liqee - asset_to_liqor
  deriving Repr

-- Spot liquidation fee invariant
def SpotLiquidation.feeInvariant (liq : SpotLiquidation) : Prop :=
  liq.platform_fee = liq.asset_transfer_from_liqee - liq.asset_transfer_to_liqor ∧
  liq.asset_transfer_from_liqee >= liq.asset_transfer_to_liqor

-- Token bankruptcy with insurance
structure TokenBankruptcy where
  bank            : Bank
  insurance_transfer : Nat
  socialized_loss : Nat
  new_deposit_index : Int
  deriving Repr

-- Insurance fund bound invariant
def TokenBankruptcy.insuranceBound (tb : TokenBankruptcy) (insurance_vault_amount : Nat) : Prop :=
  tb.insurance_transfer <= insurance_vault_amount

-- Loss socialization correctness
def TokenBankruptcy.socializationCorrect (tb : TokenBankruptcy) (old_bank : Bank) : Prop :=
  old_bank.indexed_deposits > 0 →
  tb.new_deposit_index = old_bank.deposit_index - (tb.socialized_loss : Int) / old_bank.indexed_deposits

-- Perp liquidation
structure PerpLiquidation where
  market          : PerpMarket
  base_transfer   : Int    -- lots transferred (negative = reducing long)
  quote_transfer_liqee  : Int
  quote_transfer_liqor  : Int
  platform_fee    : Nat
  deriving Repr

-- Perp liquidation fee invariant
def PerpLiquidation.feeInvariant (pl : PerpLiquidation) : Prop :=
  (pl.platform_fee : Int) = max 0 (-pl.quote_transfer_liqor - pl.quote_transfer_liqee)

-- Flash loan cycle (Begin + End)
structure FlashLoanCycle where
  bank            : Bank
  approved_amount : Nat
  token_account_initial : Nat
  token_account_final   : Nat
  repay_amount    : Nat  -- max(final - initial, 0)
  vault_initial   : Nat
  vault_final     : Nat
  deriving Repr

-- Flash loan conservation: vault_final = vault_initial - approved + repay
def FlashLoanCycle.conservation (fl : FlashLoanCycle) : Prop :=
  fl.vault_final = fl.vault_initial - fl.approved_amount + fl.repay_amount ∧
  fl.repay_amount = if fl.token_account_final > fl.token_account_initial
                    then fl.token_account_final - fl.token_account_initial
                    else 0

end MangoV4
