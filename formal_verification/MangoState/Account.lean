import MangoState.Types
import MangoState.Bank
import QEDGen.Solana

open QEDGen.Solana
open MangoV4

namespace MangoV4

/-!
# MangoAccount State Model

Models the user's margin account including positions, liquidation state,
and health region management.
-/

structure MangoAccount where
  group              : Pubkey
  owner              : Pubkey
  delegate           : Pubkey  -- 0 = no delegate
  temporary_delegate : Pubkey  -- 0 = no temp delegate
  temporary_delegate_expiry : Nat

  being_liquidated   : Bool
  in_health_region   : Bool
  frozen_until       : Nat

  -- Health snapshot for health region
  health_region_begin_init_health : Int

  -- Token positions
  token_positions    : List TokenPosition
  deriving Repr

-- Account is operational (not frozen)
def MangoAccount.isOperational (a : MangoAccount) (now_ts : Nat) : Prop :=
  a.frozen_until < now_ts

-- Check if signer is owner or delegate
def MangoAccount.isOwnerOrDelegate (a : MangoAccount) (signer : Pubkey) (now_ts : Nat) : Prop :=
  signer = a.owner ∨
  signer = a.delegate ∨
  (signer = a.temporary_delegate ∧ now_ts < a.temporary_delegate_expiry)

-- Liquidation state machine: recover if liq_end_health >= 0
-- (threshold is actually > -1 USDC native, modeled as >= 0 here)
def MangoAccount.maybeRecoverFromLiquidation (a : MangoAccount) (liq_end_health : Int)
    : MangoAccount :=
  if a.being_liquidated ∧ liq_end_health >= 0 then
    { a with being_liquidated := false }
  else a

/-!
## Group State Model
-/

structure Group where
  creator    : Pubkey
  admin      : Pubkey
  security_admin : Pubkey
  fast_listing_admin : Pubkey
  insurance_vault : Pubkey
  ix_gate    : Nat  -- u128 bitmask
  testing    : Bool
  version    : Nat
  deposit_limit_quote : Nat  -- 0 = disabled
  deriving Repr

-- Check if an instruction is enabled (bit = 0 means enabled)
def Group.isIxEnabled (g : Group) (ix_index : Nat) : Prop :=
  (g.ix_gate / (2 ^ ix_index)) % 2 = 0

-- Admin authorization check
def Group.isAdmin (g : Group) (signer : Pubkey) : Prop :=
  signer = g.admin

/-!
## Transition: Account operations requiring owner/delegate
-/

-- Generic authorized operation: checks signer is owner or delegate
def authorizedTransition (a : MangoAccount) (signer : Pubkey) (now_ts : Nat)
    (operation : MangoAccount → Option MangoAccount) : Option MangoAccount :=
  if signer = a.owner ∨
     signer = a.delegate ∨
     (signer = a.temporary_delegate ∧ now_ts < a.temporary_delegate_expiry) then
    operation a
  else none

-- Admin-only operation on Group
def adminTransition (g : Group) (signer : Pubkey)
    (operation : Group → Option Group) : Option Group :=
  if signer = g.admin then
    operation g
  else none

end MangoV4
