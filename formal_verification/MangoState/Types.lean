import QEDGen.Solana

open QEDGen.Solana

namespace MangoV4

/-!
# Mango V4 Core Types

Lean 4 models of the core types from the mango-v4 Solana program.
These map directly to the Rust on-chain structures.
-/

-- Token index (u16 in Rust, MAX = inactive)
abbrev TokenIndex := Nat
abbrev PerpMarketIndex := Nat

def TokenIndex.MAX : TokenIndex := 65535
def PerpMarketIndex.MAX : PerpMarketIndex := 65535

-- Fixed-point I80F48 modeled as integers scaled by 2^48
-- For verification we use Nat/Int and reason about bounds
abbrev I80F48 := Int

def I80F48.ZERO : I80F48 := 0
def I80F48.ONE : I80F48 := 1

-- Index start value: 1_000_000 in the Rust code
def INDEX_START : I80F48 := 1000000

-- Health type enum
inductive HealthType
  | Init
  | LiquidationEnd
  | Maint
  deriving Repr, DecidableEq

-- Reduce-only modes
inductive ReduceOnly
  | Off         -- 0
  | ReduceBoth  -- 1
  | ReduceBorrowsOnly -- 2
  deriving Repr, DecidableEq

-- Liquidation state
inductive LiquidationState
  | Normal
  | BeingLiquidated
  deriving Repr, DecidableEq

-- Flash loan state
inductive FlashLoanState
  | Idle      -- approved=0, initial=MAX
  | Active    -- approved>0, initial=balance
  deriving Repr, DecidableEq

-- Queue item status
inductive QueueItemStatus
  | Empty
  | Pending
  | Executed
  | Failed
  | Skipped
  deriving Repr, DecidableEq

end MangoV4
