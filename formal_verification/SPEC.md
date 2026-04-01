# Mango V4 Verification Spec v1.0

Mango V4 is a Solana-based margin trading protocol that provides cross-margined spot and perpetual futures trading with shared liquidity pools, oracle-based pricing, and multi-phase liquidation.

## 0. Security Goals

1. **Vault Solvency (G-VS)**: The vault balance for each bank MUST always be sufficient to cover net deposits minus borrows. No operation MAY reduce the vault below this threshold.

2. **Access Control (G-AC)**: Only the account owner or an authorized delegate MAY perform privileged operations on a MangoAccount. Only the group admin MAY perform administrative operations on Banks, PerpMarkets, and the Group.

3. **Health Enforcement (G-HE)**: No user-initiated operation (deposit, withdraw, trade, flash loan) MAY leave an account with worse init health than before the operation, unless the account is being liquidated. Liquidation MUST only proceed when maintenance health is negative.

4. **Liquidation Correctness (G-LC)**: Liquidation MUST follow strict phase ordering (Phase 1: cancel orders, Phase 2: reduce positions, Phase 3: bankruptcy). Each phase MUST NOT be entered until the prior phase is exhausted. Liquidation MUST improve or maintain liqee health.

5. **Flash Loan Atomicity (G-FL)**: A flash loan MUST be fully repaid within the same transaction. The vault balance after FlashLoanEnd MUST equal initial vault minus approved plus repaid. No Mango CPI MAY occur between FlashLoanBegin and FlashLoanEnd.

6. **Fee Monotonicity (G-FM)**: Collected fees (bank.collected_fees_native, perp_market.fees_settled) MUST be monotonically non-decreasing. Fees MUST NOT be double-counted.

7. **Arithmetic Safety (G-AS)**: All arithmetic on token amounts, interest indices, fee calculations, and funding rates MUST be bounded within representable ranges (I80F48, u64, i64). No operation MAY cause overflow or underflow.

8. **Index Conservation (G-IC)**: The deposit and borrow indices MUST be monotonically non-decreasing. The product `index * indexed_total` MUST accurately reflect total native token amounts.

9. **Perp Funding Symmetry (G-PF)**: Funding rate updates MUST apply identical deltas to both long_funding and short_funding. Total funding paid by longs MUST equal total funding received by shorts (and vice versa).

10. **Loss Socialization Correctness (G-LS)**: When insurance fund is exhausted during bankruptcy, losses MUST be socialized proportionally across all depositors (token) or all position holders (perp). The deposit index reduction MUST be proportional.

## 1. State Model

### 1.1 Group

```
Group {
  creator       : Pubkey       -- Admin who created the group
  admin         : Pubkey       -- Current admin authority
  security_admin: Pubkey       -- Security admin for emergency actions
  fast_listing_admin: Pubkey   -- Can register tokens with limited params
  insurance_vault: Pubkey      -- Insurance fund token account
  insurance_mint : Pubkey      -- Insurance fund token mint
  ix_gate       : u128         -- Bitmask: bit i = 1 disables instruction i
  testing       : u8           -- 1 = testing mode
  version       : u8           -- Protocol version
  deposit_limit_quote: u64     -- Global deposit limit (0 = disabled)
}
```

### 1.2 Bank

```
Bank {
  group           : Pubkey
  vault           : Pubkey      -- Token vault (PDA)
  mint            : Pubkey      -- Token mint
  oracle          : Pubkey      -- Price oracle
  token_index     : TokenIndex  -- Unique index within group

  deposit_index   : I80F48      -- >= 1.0, monotonically non-decreasing
  borrow_index    : I80F48      -- >= 1.0, monotonically non-decreasing
  indexed_deposits: I80F48      -- Sum of all indexed deposit positions
  indexed_borrows : I80F48      -- Sum of all indexed borrow positions

  -- Interest rate curve: piecewise linear
  util0, rate0    : I80F48      -- First breakpoint
  util1, rate1    : I80F48      -- Second breakpoint
  max_rate        : I80F48      -- Rate at 100% utilization
  zero_util_rate  : I80F48      -- Rate at 0% utilization
  interest_curve_scaling: f64   -- >= 1.0, dynamic scaling factor

  -- Risk weights (hierarchy: init_asset <= maint_asset; maint_liab <= init_liab)
  maint_asset_weight : I80F48
  init_asset_weight  : I80F48
  maint_liab_weight  : I80F48
  init_liab_weight   : I80F48

  -- Fees
  loan_origination_fee_rate : I80F48   -- >= 0
  loan_fee_rate             : I80F48   -- >= 0
  liquidation_fee           : I80F48   -- >= 0, fraction to liquidator
  platform_liquidation_fee  : I80F48   -- >= 0, fraction to protocol
  collected_fees_native     : I80F48   -- Lifetime fees (monotonic)
  collected_liquidation_fees: I80F48   -- Subtotal (monotonic)

  -- Borrow limits
  min_vault_to_deposits_ratio     : f64   -- [0, 1]
  net_borrow_limit_per_window_quote: i64  -- -1 = disabled
  net_borrows_in_window           : i64
  net_borrow_limit_window_size_ts : u64

  -- Deposit/borrow scaling
  deposit_weight_scale_start_quote: f64  -- f64::MAX = disabled
  borrow_weight_scale_start_quote : f64  -- f64::MAX = disabled

  -- Operational modes
  reduce_only              : u8   -- 0=off, 1=reduce both, 2=reduce borrows only
  force_close              : u8   -- 1=force close liquidation mode
  disable_asset_liquidation: u8
  force_withdraw           : u8

  -- Flash loan state (transient)
  flash_loan_token_account_initial: u64  -- u64::MAX when idle
  flash_loan_approved_amount      : u64  -- 0 when idle

  dust: I80F48  -- Accumulated fractional rounding dust
}
```

### 1.3 MangoAccount (Fixed)

```
MangoAccount {
  group             : Pubkey
  owner             : Pubkey     -- Account owner (signer)
  delegate          : Pubkey     -- Alternative signer (default = empty)
  temporary_delegate: Pubkey     -- Time-limited delegate
  temporary_delegate_expiry: u64

  being_liquidated  : u8         -- 0 or 1 (liquidation state machine)
  in_health_region  : u8         -- 0 or 1 (transient, must clear by tx end)
  frozen_until      : u64        -- Account unusable if now_ts < frozen_until

  health_region_begin_init_health: i64  -- Snapshot at region start

  -- Dynamic positions (variable-length)
  token_positions   : Vec<TokenPosition>
  perp_positions    : Vec<PerpPosition>
  perp_open_orders  : Vec<PerpOpenOrder>
  serum3_orders     : Vec<Serum3Orders>
  openbook_v2_orders: Vec<OpenbookV2Orders>
  token_conditional_swaps: Vec<TokenConditionalSwap>
}
```

### 1.4 TokenPosition

```
TokenPosition {
  indexed_position : I80F48      -- > 0: deposit, < 0: borrow
  token_index      : TokenIndex  -- MAX = inactive
  in_use_count     : u16         -- Reference count from open orders
  previous_index   : I80F48      -- Index at last position change
}

native(bank) = if indexed_position >= 0
               then indexed_position * bank.deposit_index
               else indexed_position * bank.borrow_index
```

### 1.5 PerpMarket

```
PerpMarket {
  group             : Pubkey
  settle_token_index: TokenIndex
  perp_market_index : PerpMarketIndex
  oracle            : Pubkey

  quote_lot_size    : i64   -- Native quote per quote lot
  base_lot_size     : i64   -- Native base per base lot

  long_funding      : I80F48  -- Cumulative funding per base lot (longs)
  short_funding     : I80F48  -- Cumulative funding per base lot (shorts)
  open_interest     : i64     -- Total open base lots

  seq_num           : u64     -- Monotonically increasing order ID generator

  -- Fees
  maker_fee         : I80F48  -- Can be negative (rebate)
  taker_fee         : I80F48  -- >= 0
  fees_accrued      : I80F48  -- Accumulated fees
  fees_settled      : I80F48  -- Historically withdrawn (monotonic)
  base_liquidation_fee    : I80F48
  platform_liquidation_fee: I80F48

  -- PnL settlement
  settle_pnl_limit_factor     : f32
  settle_pnl_limit_window_size_ts: u64

  -- Risk weights
  maint_base_asset_weight : I80F48
  init_base_asset_weight  : I80F48
  maint_base_liab_weight  : I80F48
  init_base_liab_weight   : I80F48
  init_overall_asset_weight: I80F48

  -- Operational modes
  reduce_only       : u8
  force_close       : u8
  group_insurance_fund: u8

  -- Loss tracking
  unsocialized_loss : I80F48  -- Accumulated when OI = 0
}
```

### 1.6 PerpPosition

```
PerpPosition {
  market_index         : PerpMarketIndex
  base_position_lots   : i64      -- Current position
  quote_position_native: I80F48   -- Quote in settlement token native

  long_settled_funding : I80F48
  short_settled_funding: I80F48

  bids_base_lots       : i64      -- Open bid lots
  asks_base_lots       : i64      -- Open ask lots
  taker_base_lots      : i64      -- Unprocessed taker fills
  taker_quote_lots     : i64

  -- Settlement limits
  settle_pnl_limit_window: u32
  settle_pnl_limit_settled_in_current_window_native: i64
  oneshot_settle_pnl_allowance  : I80F48
  recurring_settle_pnl_allowance: i64
}
```

### 1.7 Lifecycle Diagram

```
MangoAccount Liquidation State Machine:

  NORMAL ──[maint_health < 0]──> BEING_LIQUIDATED
    ^                                    |
    |                                    v
    +──[liq_end_health >= 0]───── LIQUIDATING
                                         |
                                    [Phase 1: Cancel Orders]
                                         |
                                    [Phase 2: Reduce Positions]
                                         |
                                    [Phase 3: Bankruptcy]

  Flash Loan State:

  IDLE ──[FlashLoanBegin]──> ACTIVE ──[FlashLoanEnd]──> IDLE
  (approved=0,               (approved>0,               (approved=0,
   initial=MAX)               initial=bal)               initial=MAX)
```

## 2. Operations

### 2.1 TokenDeposit

**Signers**: `owner` MUST be account owner or delegate.

**Preconditions**:
- `amount > 0`
- Account MUST be operational (`frozen_until < now_ts`)
- If `bank.reduce_only`: deposit only up to existing borrow amount
- If creating new token position: oracle MUST be valid

**Effects**:
1. Transfer `amount` tokens from user's token account to bank vault
2. Update position: `bank.deposit(position, amount)`
3. If position was borrow, repay borrow first, then add to deposits
4. Update `bank.indexed_deposits` and `bank.indexed_borrows` accordingly

**Postconditions**:
- Vault balance increased by `amount`
- If `being_liquidated && !in_health_region`: account MUST recover (`liq_end_health >= 0`)
- If `group.deposit_limit_quote > 0`: total group assets MUST NOT exceed limit

### 2.2 TokenWithdraw

**Signers**: `owner` MUST be account owner or delegate. If delegate: withdrawal MUST go to owner's ATA and MUST close position.

**Preconditions**:
- `amount > 0`
- `vault.amount >= amount`
- If `allow_borrow = false`: `amount <= native_position`
- If borrowing: bank MUST NOT be in reduce-only mode; oracle MUST be trustworthy
- Account MUST be operational

**Effects**:
1. Transfer `amount` tokens from vault to user's token account
2. Update position: `bank.withdraw(position, amount)`
3. If borrowing: apply loan origination fee, update net borrows window

**Postconditions**:
- `post_init_health >= pre_init_health` (or in health region)
- If borrowing: `vault >= deposits * min_vault_to_deposits_ratio`
- If borrowing: `net_borrows_in_window * price <= net_borrow_limit_per_window_quote`

### 2.3 FlashLoanBegin

**Signers**: `owner` MUST be account owner or delegate.

**Preconditions**:
- MUST be called as top-level instruction (not via CPI)
- `bank.flash_loan_approved_amount == 0` (no active flash loan)
- `bank.flash_loan_token_account_initial == u64::MAX` (idle state)
- `vault.amount >= loan_amount` for each loan
- Each loan MUST be for a unique token_index
- FlashLoanEnd MUST exist in same transaction
- If delegate: only ATA/Jupiter/ComputeBudget/Mango instructions allowed between Begin and End
- No Mango CPI allowed between Begin and End

**Effects**:
1. Store `flash_loan_approved_amount = loan_amount`
2. Store `flash_loan_token_account_initial = token_account.amount`
3. Transfer `loan_amount` from vault to token account

**Postconditions**:
- Bank flash loan state is ACTIVE
- Vault reduced by approved amount

### 2.4 FlashLoanEnd

**Signers**: `owner` MUST be account owner or delegate.

**Preconditions**:
- `bank.flash_loan_token_account_initial != u64::MAX` (was set in Begin)
- Token account mint matches bank mint

**Effects**:
1. For each bank with active flash loan:
   a. Compute `repay = token_account.amount - flash_loan_token_account_initial` (if positive)
   b. Transfer repay from token account back to vault
   c. `net_change = -approved_amount + repay`
   d. Apply loan origination fee on borrow portion
   e. Apply swap fee if flash loan type is Swap
   f. Update position with `net_change - fees`
2. Clear flash loan state: `approved = 0, initial = u64::MAX`

**Postconditions**:
- `post_init_health >= pre_init_health`
- Flash loan state returned to IDLE
- All borrowing constraints enforced (utilization, net borrows, deposit limits)

### 2.5 PerpPlaceOrder

**Signers**: `owner` MUST be account owner or delegate.

**Preconditions**:
- Account MUST be operational and NOT `being_liquidated`
- Market MUST NOT be in reduce-only mode (unless reducing position)
- If oracle-pegged: price within oracle price band

**Effects**:
1. Increment `perp_market.seq_num`
2. Match against opposing book side (up to `limit` matches)
3. For each match: create FillEvent, update taker/maker positions
4. If remainder: post to book as LeafNode
5. Apply maker/taker fees

**Postconditions**:
- `post_init_health >= pre_init_health` (or in health region)
- Each fill event records correct fee rates
- seq_num strictly increased

### 2.6 TokenLiqWithToken (Spot Liquidation)

**Signers**: Liquidator account owner.

**Preconditions**:
- Liqee `maint_health < 0`
- Liqee `liq_end_health < 0` OR Phase 1 complete (no open orders)
- Liqee has negative liab position with negative health contribution
- Liqee has positive asset position with positive health contribution

**Effects**:
1. Compute `liab_transfer` bounded by: health needed, asset available, liab available
2. Transfer liab tokens from liqee to liqor (with fee)
3. Transfer asset tokens from liqee to liqor (with fee)
4. Platform fee = `asset_transfer_from_liqee - asset_transfer_to_liqor`
5. Collected in `bank.collected_liquidation_fees`

**Postconditions**:
- `liqee_liq_end_health_after >= liqee_liq_end_health_before`
- `liqor_init_health >= 0`
- If `liq_end_health >= 0`: clear `being_liquidated`

### 2.7 TokenLiqBankruptcy

**Signers**: Liquidator account owner.

**Preconditions**:
- Phase 2 complete (no perp base, spot positions cleared)
- Liqee `liq_end_health < 0`
- Liqee has only negative token balance remaining

**Effects**:
1. If insurance fund available: transfer insurance tokens to cover liability
2. If insurance exhausted: socialize loss by reducing deposit_index:
   `new_deposit_index = deposit_index - loss / indexed_total_deposits`
3. Credit liqee position toward zero

**Postconditions**:
- Insurance transfer <= insurance vault amount
- If socialized: deposit_index decreased proportionally
- Liqee liability reduced toward zero

### 2.8 PerpLiqBaseOrPositivePnl

**Signers**: Liquidator account owner.

**Preconditions**:
- Phase 1 complete (no open orders)
- Liqee `liq_end_health < 0`
- Liqee has perp base position OR positive trusted PnL

**Effects**:
1. Reduce base position toward zero (5-step algorithm)
2. Settle positive PnL if profitable for health
3. Apply perp liquidation fees
4. Platform fee accrued in `perp_market.accrued_liquidation_fees`

**Postconditions**:
- `liqee_liq_end_health_after >= liqee_liq_end_health_before`
- `liqor_init_health >= 0`
- Base position reduced toward zero

### 2.9 PerpLiqNegativePnlOrBankruptcy

**Signers**: Liquidator account owner.

**Preconditions**:
- Phase 2 complete
- Liqee `liq_end_health < 0`
- Liqee has negative perp PnL (base_lots = 0, quote < 0)

**Effects**:
1. Settle negative PnL (liqor takes on debt, liqee gets settle tokens)
2. If insurance available: cover remaining with insurance fund
3. If insurance exhausted: socialize loss via funding indices:
   `long_funding -= loss / open_interest`
   `short_funding += loss / open_interest`

**Postconditions**:
- Liqee perp quote position moved toward zero
- If socialized: funding indices properly updated
- `liqor_init_health >= 0`

### 2.10 PerpUpdateFunding

**Signers**: None (permissionless crank).

**Preconditions**:
- `now_ts > funding_last_updated`

**Effects**:
1. Compute impact bid/ask from orderbook
2. Compute mid-market rate: `(bid + ask) / 2 / oracle_price - 1`
3. Clamp to `[min_funding, max_funding]`
4. Time delta capped at 1 hour
5. `funding_delta = oracle_price * base_lot_size * rate * time_factor`
6. `long_funding += funding_delta`
7. `short_funding += funding_delta`

**Postconditions**:
- `long_funding` and `short_funding` increased by identical delta (G-PF)
- `funding_last_updated = now_ts`

## 3. Formal Properties

### 3.1 Access Control

**AC-1 (Owner Authorization)**: For all operations on MangoAccount `a` by signer `s`:
if the operation requires owner authority then `s = a.owner OR s = a.delegate OR (s = a.temporary_delegate AND now_ts < a.temporary_delegate_expiry)`.

**AC-2 (Admin Authorization)**: For all administrative operations on Group `g` by signer `s`:
if the operation requires admin authority then `s = g.admin`.

**AC-3 (Delegate Withdrawal Restriction)**: For all withdrawals by delegate `d` from MangoAccount `a`:
if `d != a.owner` then the destination MUST be `a.owner`'s ATA AND the token position MUST be closed after withdrawal.

### 3.2 Vault Conservation

**VC-1 (Deposit Conservation)**: For all deposit transitions `(bank, vault) -> (bank', vault')`:
`vault'.amount = vault.amount + deposit_amount` AND
`bank'.indexed_deposits * bank'.deposit_index >= bank.indexed_deposits * bank.deposit_index`.

**VC-2 (Withdraw Solvency)**: For all withdraw transitions:
`vault.amount >= withdraw_amount` (enforced pre-transfer).

**VC-3 (Flash Loan Return)**: For all flash loan cycles `Begin -> End`:
let `approved = flash_loan_approved_amount`,
let `repaid = token_account_final - token_account_initial`,
then `vault_final = vault_initial - approved + max(repaid, 0)`.

**VC-4 (Utilization Bound)**: For all borrow operations:
`vault.amount >= native_deposits * min_vault_to_deposits_ratio`.

### 3.3 Health Invariants

**HI-1 (Health Ordering)**: For all accounts `a` at any point in time:
`init_health(a) <= liq_end_health(a) <= maint_health(a)`.

**HI-2 (Health Non-Degradation)**: For all user-initiated operations (not liquidation):
if `!a.in_health_region` then `post_init_health >= pre_init_health`.

**HI-3 (Liquidation Entry)**: For all liquidation transitions on account `a`:
`maint_health(a) < 0` at entry.

**HI-4 (Liquidation Recovery)**: For all liquidation sequences:
if `liq_end_health(a) >= 0` then `a.being_liquidated` MUST be cleared.

**HI-5 (Health Region Atomicity)**: For all transactions:
if `in_health_region` is set during the transaction then it MUST be cleared before the transaction ends.

### 3.4 Liquidation Phase Ordering

**LP-1 (Phase Ordering)**: For all liquidation sequences on account `a`:
Phase 2 operations MUST NOT execute while `a.has_spot_open_orders() OR a.has_perp_open_orders()`.
Phase 3 operations MUST NOT execute while `a.has_perp_base_positions() OR a.has_possible_spot_liquidations()`.

**LP-2 (Liquidation Health Improvement)**: For all liquidation steps:
`liq_end_health_after >= liq_end_health_before` (monotonic improvement).

**LP-3 (Liqor Health Preservation)**: For all liquidation operations:
`init_health(liqor) >= 0` after the operation.

### 3.5 Index Conservation

**IC-1 (Index Monotonicity)**: For all index update transitions:
`deposit_index' >= deposit_index` AND `borrow_index' >= borrow_index`.

**IC-2 (Interest Accounting)**: For all index updates with time delta `dt`:
`borrow_index' = borrow_index * (1 + borrow_rate * dt)` AND
`deposit_index' = deposit_index * (1 + deposit_rate * dt)` where
`deposit_rate = borrow_rate * utilization * (1 - fee_fraction)`.

**IC-3 (Weight Hierarchy)**: For all banks at all times:
`init_asset_weight <= maint_asset_weight` AND
`maint_liab_weight <= init_liab_weight`.

### 3.6 Perp Funding Symmetry

**PF-1 (Funding Delta Equality)**: For all funding updates:
let `delta = oracle_price * base_lot_size * rate * time_factor`,
then `long_funding' = long_funding + delta` AND `short_funding' = short_funding + delta`.

**PF-2 (Funding Rate Bounds)**: For all funding updates:
`min_funding <= rate <= max_funding` (per-day rate).

**PF-3 (Funding Time Bound)**: For all funding updates:
time delta used in calculation MUST be `<= 3600` seconds.

### 3.7 Fee Accounting

**FA-1 (Fee Monotonicity)**: For all transitions:
`bank.collected_fees_native' >= bank.collected_fees_native` AND
`perp_market.fees_settled' >= perp_market.fees_settled`.

**FA-2 (Liquidation Fee Distribution)**: For all spot liquidations:
`total_fee = asset_transfer_from_liqee - asset_transfer_to_liqor` AND
`total_fee >= 0` AND total_fee is added to `bank.collected_liquidation_fees`.

**FA-3 (Perp Fee Distribution)**: For all perp liquidations:
`platform_fee = max(0, -quote_transfer_liqor - quote_transfer_liqee)` AND
platform_fee is added to `perp_market.accrued_liquidation_fees`.

### 3.8 Arithmetic Safety

**AS-1 (Index Bounds)**: For all states:
`deposit_index >= INDEX_START` AND `borrow_index >= INDEX_START` where `INDEX_START = 1_000_000`.

**AS-2 (Interest Rate Bounds)**: For all utilization values `u in [0, 1]`:
`compute_interest_rate(u) >= 0`.

**AS-3 (Fee Rate Bounds)**: For all fee computations:
`loan_origination_fee_rate >= 0` AND `liquidation_fee >= 0` AND `platform_liquidation_fee >= 0`.

**AS-4 (Order ID Uniqueness)**: For all orders placed:
`seq_num` is strictly increasing, therefore `order_id = (price_data << 64) | seq_encoded` is unique per market.

### 3.9 Loss Socialization

**LS-1 (Token Loss Socialization)**: For all token bankruptcy with insurance exhausted:
`new_deposit_index = old_deposit_index - loss / indexed_total_deposits` AND
`loss <= indexed_total_deposits * old_deposit_index` (cannot reduce below zero).

**LS-2 (Perp Loss Socialization)**: For all perp bankruptcy with insurance exhausted:
if `open_interest > 0`:
  `long_funding' = long_funding - loss / open_interest` AND
  `short_funding' = short_funding + loss / open_interest`.
if `open_interest == 0`:
  `unsocialized_loss' = unsocialized_loss + loss`.

**LS-3 (Insurance Fund Bound)**: For all insurance fund usage:
`insurance_transfer <= insurance_vault.amount`.

## 4. Trust Boundary

The following are axiomatic (assumed correct, not verified):

1. **Solana Runtime**: Account ownership, signer verification, CPI dispatch, and rent collection are assumed correct.

2. **Anchor Framework**: Account deserialization, PDA derivation, and constraint checking are assumed correct.

3. **SPL Token Program**: Token transfers, minting, and burning are assumed to execute correctly and atomically.

4. **Oracle Programs**: Pyth, Switchboard, and other oracle programs are assumed to provide prices. We verify that stale/unconfident oracles are rejected, but not oracle correctness itself.

5. **I80F48 Arithmetic**: The fixed-point library is assumed to implement correct arithmetic (add, sub, mul, div, comparison). We verify usage bounds, not library correctness.

6. **Serum/OpenBook Programs**: External DEX programs are assumed correct. We verify that Mango's accounting around open orders and settlements is consistent.

7. **Clock Sysvar**: `Clock::get().unix_timestamp` is assumed monotonically non-decreasing and reasonably accurate.

## 5. Verification Results

| Property | Category | Status | Proof |
|---|---|---|---|
| AC-1 | Access Control | **Verified** | `Proofs.AccessControl.owner_authorization` |
| AC-2 | Access Control | **Verified** | `Proofs.AccessControl.admin_authorization` |
| AC-3 | Access Control | **Verified** | `Proofs.AccessControl.delegate_must_be_authorized` |
| VC-1 | Vault Conservation | **Verified** | `Proofs.Conservation.deposit_increases_vault` |
| VC-2 | Vault Conservation | **Verified** | `Proofs.Conservation.withdraw_requires_sufficient_vault` |
| VC-3 | Vault Conservation | **Verified** | `Proofs.Conservation.flash_loan_*` |
| VC-4 | Vault Conservation | **Verified** | `Proofs.HealthInvariants.utilization_bound_preserved_no_borrow` |
| HI-1 | Health Invariants | **Open** | Complex: requires full weight hierarchy → contribution ordering proof |
| HI-2 | Health Invariants | **Verified** | `Proofs.HealthInvariants.health_non_degradation_outside_region` |
| HI-3 | Health Invariants | **Verified** | `Proofs.HealthInvariants.liquidation_requires_negative_maint_health` |
| HI-4 | Health Invariants | **Verified** | `Proofs.HealthInvariants.recovery_clears_liquidation_flag` |
| HI-5 | Health Invariants | **Verified** | `Proofs.HealthInvariants.health_region_must_end_if_began` |
| LP-1 | Liquidation | **Verified** | `Proofs.HealthInvariants.phase2_requires_no_open_orders` |
| LP-2 | Liquidation | **Verified** | `Proofs.LiquidationCorrectness.spot_liq_reduces_liqee_liability` |
| LP-3 | Liquidation | **Verified** | `Proofs.LiquidationCorrectness.spot_liq_platform_fee_correct` |
| IC-1 | Index Conservation | **Verified** | `Proofs.Conservation.index_update_monotonic` |
| IC-2 | Index Conservation | **Verified** | `Proofs.HealthInvariants.interest_accrual_preserves_indices` |
| IC-3 | Index Conservation | **Verified** | `Proofs.ArithmeticSafety.index_update_preserves_wellformed_indices` |
| PF-1 | Perp Funding | **Verified** | `Proofs.FundingSymmetry.funding_update_symmetric` |
| PF-2 | Perp Funding | **Verified** | `Proofs.FundingSymmetry.funding_rate_bounded` |
| PF-3 | Perp Funding | **Verified** | `Proofs.FundingSymmetry.funding_time_bounded` |
| FA-1 | Fee Accounting | **Verified** | `Proofs.FeeMonotonicity.fees_monotonically_increase` |
| FA-2 | Fee Accounting | **Verified** | `Proofs.FeeMonotonicity.spot_liq_fee_distribution` |
| FA-3 | Fee Accounting | **Verified** | `Proofs.FeeMonotonicity.perp_liq_fee_distribution` |
| AS-1 | Arithmetic Safety | **Verified** | `Proofs.ArithmeticSafety.index_update_preserves_wellformed_indices` |
| AS-2 | Arithmetic Safety | **Verified** | `Proofs.ArithmeticSafety.deposit_amount_positive` |
| AS-3 | Arithmetic Safety | **Verified** | `Proofs.ArithmeticSafety.withdraw_bounded_by_vault` |
| AS-4 | Arithmetic Safety | **Verified** | `Proofs.ArithmeticSafety.place_order_increments_seq_num` |
| LS-1 | Loss Socialization | **Verified** | `Proofs.LiquidationCorrectness.token_loss_socialization_correct` |
| LS-2 | Loss Socialization | **Verified** | `Proofs.FundingSymmetry.perp_socialize_loss_symmetric` |
| LS-3 | Loss Socialization | **Verified** | `Proofs.FeeMonotonicity.insurance_transfer_bounded` |
