import MangoState.Types
import QEDGen.Solana

open QEDGen.Solana
open MangoV4

namespace MangoV4

/-!
# PerpMarket State Model

Models the perpetual futures market including funding, fees, and order sequencing.
-/

structure PerpMarket where
  group              : Pubkey
  settle_token_index : TokenIndex
  perp_market_index  : PerpMarketIndex
  oracle             : Pubkey

  quote_lot_size     : Nat
  base_lot_size      : Nat

  -- Funding accumulators (per base lot)
  long_funding       : Int
  short_funding      : Int
  funding_last_updated : Nat

  -- Funding rate bounds (per day, scaled)
  min_funding        : Int
  max_funding        : Int

  -- Open interest
  open_interest      : Nat

  -- Order sequencing
  seq_num            : Nat

  -- Fees
  maker_fee          : Int   -- can be negative (rebate)
  taker_fee          : Nat   -- >= 0
  fees_accrued       : Int
  fees_settled       : Nat   -- monotonically non-decreasing
  base_liquidation_fee    : Nat
  platform_liquidation_fee: Nat
  accrued_liquidation_fees: Nat  -- monotonically non-decreasing

  -- Risk weights
  init_overall_asset_weight : Nat

  -- Loss tracking
  unsocialized_loss  : Int  -- accumulates when OI = 0

  -- Operational modes
  reduce_only        : Bool
  force_close        : Bool
  group_insurance_fund : Bool
  deriving Repr

/-!
## Perp Position (within MangoAccount)
-/

structure PerpPosition where
  market_index         : PerpMarketIndex
  base_position_lots   : Int
  quote_position_native: Int

  long_settled_funding : Int
  short_settled_funding: Int

  bids_base_lots       : Nat
  asks_base_lots       : Nat
  taker_base_lots      : Int
  taker_quote_lots     : Int

  -- Settlement limits
  settle_pnl_limit_settled_in_current_window_native : Int
  oneshot_settle_pnl_allowance   : Int
  recurring_settle_pnl_allowance : Int
  deriving Repr

def PerpPosition.isActive (p : PerpPosition) : Prop :=
  p.market_index ≠ PerpMarketIndex.MAX

def PerpPosition.hasBasePosition (p : PerpPosition) : Prop :=
  p.base_position_lots ≠ 0

def PerpPosition.hasOpenOrders (p : PerpPosition) : Prop :=
  p.bids_base_lots > 0 ∨ p.asks_base_lots > 0

/-!
## Funding Update Transition

Property PF-1: Both long and short funding change by identical delta.
-/

def fundingUpdateTransition (m : PerpMarket) (funding_rate : Int)
    (oracle_price : Nat) (time_delta : Nat) : Option PerpMarket :=
  if time_delta > 0 ∧ time_delta <= 3600 ∧
     funding_rate >= m.min_funding ∧ funding_rate <= m.max_funding then
    let delta : Int := oracle_price * m.base_lot_size * funding_rate * time_delta
    some {
      m with
      long_funding := m.long_funding + delta
      short_funding := m.short_funding + delta
      funding_last_updated := m.funding_last_updated + time_delta
    }
  else none

/-!
## Order Placement Transition (seq_num increment)
-/

def placeOrderTransition (m : PerpMarket) : PerpMarket :=
  { m with seq_num := m.seq_num + 1 }

/-!
## Loss Socialization Transition
-/

def perpSocializeLossTransition (m : PerpMarket) (loss : Int)
    : Option PerpMarket :=
  if loss <= 0 then
    if m.open_interest > 0 then
      let per_contract := loss / (m.open_interest : Int)
      some {
        m with
        long_funding := m.long_funding - per_contract
        short_funding := m.short_funding + per_contract
      }
    else
      some {
        m with
        unsocialized_loss := m.unsocialized_loss + loss
      }
  else none

/-!
## Fee Accrual Transition
-/

def feeAccrualTransition (m : PerpMarket) (maker_fees taker_fees : Int)
    : PerpMarket :=
  { m with
    fees_accrued := m.fees_accrued + taker_fees + maker_fees
  }

end MangoV4
