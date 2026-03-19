# Full Security & Functionality Audit: Mango V4 Protocol

**Date:** 2026-03-20 (updated from 2026-03-19 initial)
**Scope:** All onchain smart contract components and offchain interaction surfaces (relayer, execution cranker, continuum harness)

> **Design Principle:** Throughout this audit, fixes are evaluated against both security robustness AND high-throughput performance. Where tradeoffs exist, the optimal balanced solution is noted.

## Executive Summary

This audit covers all onchain smart contract components of the Mango V4 Solana program and their interaction surfaces with offchain infrastructure (relayer, execution cranker, continuum harness). The protocol is a sophisticated margin trading and derivatives platform with ~256 Rust source files in the onchain program, plus extensive TypeScript/Rust offchain services.

**Overall finding: The core protocol (health, flash loans, token ops, perps) is mature and well-engineered, but the newer execution queue / CTM subsystem introduces significant trust assumptions and attack surface. Several edge cases in bankruptcy handling and admin powers also warrant attention.**

| Severity | Count |
|----------|-------|
| CRITICAL | 5 |
| HIGH | 12 |
| MEDIUM | 16 |
| LOW | 13 |

---

## CRITICAL Findings

### C-1: `execute_multi` Trusts Caller-Provided Lane Hashes — Account Substitution
**File:** `instructions/execution_queue.rs:1509-1516`

The `execution_queue_execute_multi` instruction accepts `lane_hashes` as instruction data and uses them directly to match queue items to lanes. Unlike `execution_queue_execute` (which computes `provided_accounts_hash` from actual `dispatch_accounts`), `execute_multi` never verifies that the provided hashes correspond to the actual accounts in each lane.

A malicious executor can:
1. Provide `lane_hashes` matching a queued item's `accounts_hash`
2. Supply completely different actual accounts in the lane slice
3. The item dispatches against wrong accounts

For perp orders, the market/bids/asks are cross-validated, but the **mango account and owner** passed to `perp_place_order_from_account_infos` are NOT validated against the original intent's accounts hash. This allows placing orders on behalf of arbitrary users.

**Fix:**
```rust
// In execute_multi, compute hashes from actual accounts instead of trusting input:
let computed_hash = compute_accounts_hash(lane_slices[lane_idx]);
// Then match against candidate.accounts_hash using computed_hash
```

**Performance note:** Hash computation (SHA-256 over account keys) costs ~2,000-3,000 CU per lane. For a 4-lane execute_multi, this adds ~8,000-12,000 CU — negligible relative to the ~400,000-600,000 CU total cost of the instruction. No throughput impact.

---

### C-2: `perp_place_order_from_account_infos` Does Not Validate Owner Against MangoAccount
**File:** `instructions/perp_place_order.rs:186`

The owner is read as `let owner = *dispatch_accounts[2].key` and passed directly to `perp_place_order_inner`. `validate_perp_place_order_queue_accounts` does NOT verify this key matches the MangoAccount's actual owner. Combined with C-1, this enables full account impersonation.

**Fix:**
```rust
let account = dispatch_accounts[1].load::<MangoAccountFixed>()?;
require!(account.owner == *dispatch_accounts[2].key, MangoError::SomeError);
```

**Performance note:** Single field comparison on already-loaded zero-copy data. <100 CU cost. Zero throughput impact.

---

### C-3: `unsafe_deposit` Has No `is_testing()` Guard — Admin Can Mint Unbacked Deposits
**File:** `instructions/unsafe_deposit.rs:12`, `accounts_ix/unsafe_deposit.rs:6-31`

**Verified:** The `unsafe_deposit` instruction credits token deposits to a bank without any SPL token transfer. The only gate is `group.admin == admin.key()`. There is **no `is_testing()` constraint**, unlike similar unsafe operations. The comment says "Do not expose in production environments" but this is not enforced.

A compromised admin can credit arbitrary deposits, borrow against them, and drain real vault funds.

**Fix:**
```rust
#[account(
    constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    constraint = group.load()?.is_testing() @ MangoError::SomeError,  // ADD THIS
    constraint = group.load()?.is_ix_enabled(IxGate::TokenDeposit) @ MangoError::IxIsDisabled,
)]
pub group: AccountLoader<'info, Group>,
```

---

### C-4: Relayer Liveness — Single Point of Failure
**File:** `ctm-sequencer-relayer.ts` (entire), onchain `execution_queue.rs:1026-1030`

**Status: Reordering and censorship mitigated by external VDF-based sequencer.**

The reordering and censorship vectors identified in the initial audit are addressed by a separate VDF-based temporal FIFO sequencer (100μs resolution) with optional commit-reveal, hosted in a dedicated repository. This provides cryptographically verifiable ordering that the relayer cannot manipulate.

**Remaining attack vector: Liveness.** If the VDF sequencer + relayer infrastructure goes down, users have no fallback path to enqueue intents. The CTM co-signature requirement means users cannot self-submit.

**Fix — Direct-submit fallback with delayed execution:**

Allow users to enqueue directly using only their Ed25519 intent signature (no CTM co-signature required), but with a **10-slot delayed execution** (`min_execute_slot = current_slot + 10`). This:
1. **Does not induce race conditions** with properly sequenced transactions from the VDF relayer, since the 10-slot delay ensures direct-submit items execute well after any pending relayer-submitted items at the queue head.
2. **Provides a reusable escape hatch** for liveness failures — users can always get orders onchain, just with ~4 second extra latency.
3. **Discourages abuse** of the direct path during normal operation (the VDF path has lower latency), maintaining incentive to use the proper sequencer.

```rust
// In execution_queue_enqueue_ctm: add alternative path
if !has_ctm_signature {
    require!(has_valid_user_ed25519_signature, MangoError::InvalidSignature);
    let forced_min_execute = clock.slot + DIRECT_SUBMIT_DELAY_SLOTS; // 10
    envelope.min_execute_slot = envelope.min_execute_slot.max(forced_min_execute);
    // Skip CTM signer verification
}
```

**Performance note:** The 10-slot delay is only for the fallback path. Normal operation through the VDF sequencer is unaffected.

---

### C-5: Socialized Loss Vanishes When Open Interest Reaches Zero
**File:** `state/perp_market.rs:406-422`

**Verified:** When `open_interest == 0`, `socialize_loss()` returns `I80F48::ZERO` and the loss is silently discarded. The code's own AUDIT comment acknowledges this. If the last profitable counterparty settles PnL before the bankrupt party is liquidated, the loss cannot be socialized and manifests as an unrecoverable shortfall in the settle token vault.

**Fix:**
```rust
if self.open_interest == 0 {
    // Track unsocialized loss in a dedicated field for later recovery
    self.unsocialized_loss += loss;
    // Alternatively, route to insurance fund instead of discarding
    return Ok(loss); // Return as un-socialized for caller to handle
}
```

---

## HIGH Findings

### H-1: `enqueue_liquidity` Has No Signer — Griefing Vector
**File:** `accounts_ix/execution_queue.rs:92-100`

No signer is required. Anyone can fill the 128-slot liquidity queue with garbage items that will fail at execution (no owner signer in CPI). Cost: only transaction fees.

**Fix:** Require the mango account owner as a signer on the enqueue instruction. This adds one signature verification (~500 CU) — no throughput impact.

---

### H-2: `execute_multi` Skips All Liquidity Items
**File:** `instructions/execution_queue.rs:1446-1651`

The multi-lane execute path only processes CTM items, never liquidity items. If the system primarily uses `execute_multi`, liquidity items accumulate indefinitely.

**Analysis:** Adding a liquidity drain pass inline to `execute_multi` would add ~48,000-68,000 CU per liquidity item (CPI overhead to TokenDeposit/TokenWithdraw). More critically, liquidity items require a fundamentally different account set (token banks, vaults, SPL token program) vs. perp items (perp market, bids, asks, event queue). Mixing them in `remaining_accounts` would add complexity and inflate transaction size.

**Fix — Schedule separately in offchain cranker:** The existing single-lane `execution_queue_execute` already handles liquidity items as a fallback (lines 1211-1228). The offchain cranker should periodically call `execute` with a liquidity-focused account set. This keeps `execute_multi` lean and avoids CU waste on mixed account layouts.

**Implementation:** Add a dedicated liquidity drain loop in the cranker that calls `execution_queue_execute` every N slots (e.g., every 5 slots) when the liquidity queue is non-empty. This is a pure offchain change — no program modification needed.

---

### H-3: Insurance Fund Drainage via Rounding Dust in Bankruptcy
**Files:** `token_liq_bankruptcy.rs:114-126`, `perp_liq_negative_pnl_or_bankruptcy.rs:425-428`

Both bankruptcy liquidation paths use `.ceil()` on `insurance_transfer`. An attacker can repeatedly call bankruptcy with minimal `max_liab_transfer`, extracting 1 extra native insurance token per call.

**Fix:** Floor the insurance transfer amount, or require a minimum `max_liab_transfer` that makes the rounding insignificant relative to the transfer.

---

### H-4: Token Bankruptcy Can Produce Negative Deposit Index
**File:** `token_liq_bankruptcy.rs:246-250`

**Verified:** The code computes `new_deposit_index = liab_deposit_index - remaining_liab_loss / indexed_total_deposits`. The AUDIT comment asks "Could it happen that remaining_liab_loss > total_indexed_deposits * deposit_index?" If so, the deposit index goes negative, corrupting all depositor balances.

**Fix:**
```rust
let max_socializable = indexed_total_deposits * liab_deposit_index;
let capped_loss = remaining_liab_loss.min(max_socializable);
let new_deposit_index = liab_deposit_index - capped_loss / indexed_total_deposits;
require_gte!(new_deposit_index, I80F48::ZERO, MangoError::SomeError);
```

---

### H-5: Perp Event Queue Overflow Stalls Trading / Stale Positions
**File:** `instructions/perp_consume_events.rs:51`

Hard-limited to 8 events per consume call. During high-volatility periods, unconsumed fill events mean positions are stale — health checks use outdated data, potentially delaying critical liquidations.

**Bottleneck analysis for increasing to 16 (recommended) or 32 (not feasible):**

| Limit | Max Accounts (worst case) | Est. CU (all fills) | Feasible? |
|-------|--------------------------|---------------------|-----------|
| 8 (current) | 16 + 5 fixed = 21 | ~360K CU | Yes |
| **16 (recommended)** | 32 + 5 fixed = 37 | ~720K CU | **Yes**, within legacy tx limits |
| 24 | 48 + 5 fixed = 53 | ~1.08M CU | Tight — requires ALTs + max CU budget |
| 32 | 64 + 5 fixed = 69 | ~1.44M CU | **No** — exceeds legacy tx account limit; CU at ceiling |

**Key bottlenecks at 32:**
- **Transaction account limit:** 32 unique fill events can require up to 64 unique MangoAccounts in `remaining_accounts`. With 5 fixed accounts, this exceeds the 64-account legacy transaction limit. Versioned transactions with ALTs could theoretically support this, but the worst case is still marginal.
- **CU budget:** At ~45K CU per fill event (2 account loads + maker/taker execution + 3 event emissions), 32 events = ~1.44M CU, right at the 1.4M ceiling. Worst-case all-fill batches would fail.
- **Stack depth:** Not a bottleneck — the processing loop is flat and `emit_stack` uses `#[inline(never)]`.

**Fix:** Increase to **16** — a 2x improvement that stays safely within both legacy transaction account limits (37 accounts) and CU budget (~720K, leaving 50% headroom). Cranker frequency should also be increased to match.

```rust
let limit = std::cmp::min(limit, 16); // was 8
```

---

### H-6: Group Admin Privilege Escalation via `group_edit`
**File:** `instructions/group_edit.rs`

No timelocks, multisig, or governance. Admin can instantly: change admin key, enable `testing` mode (unlocking force-close), modify all economic parameters, change insurance fund destination. Combined with C-3 (`unsafe_deposit`), a single compromised admin key gives total fund control.

**Fix:** Introduce a **30-minute timelock** on critical parameter changes:
- Admin key rotation
- `testing` flag toggle
- Insurance fund changes
- `security_admin` changes

Implementation: Add `pending_admin`, `pending_admin_activate_slot` fields to `Group`. `group_edit` sets pending values; a separate `group_activate_pending` instruction applies them after the timelock. Non-critical parameters (economic tuning, fee rates) remain instant for operational flexibility.

**Performance note:** Zero impact on normal operations. The timelock only affects admin configuration changes, not user-facing instructions.

---

### H-7: CLMM Oracle Price Manipulation via Same-Transaction Sandwich
**File:** `state/oracle.rs:333-356`

**Severity upgraded based on deep analysis. This is the highest-risk oracle vector in the protocol.**

**Background — Mango v3 exploit (Oct 2022):** Avraham Eisenberg manipulated MNGO-PERP price on Mango's own orderbook by self-trading, inflating unrealized PnL, then borrowing $114M against it. V4 mitigates this specific vector by using external oracles (Pyth/Switchboard/CLMM) rather than internal mark prices. However, CLMM oracles reintroduce a similar attack surface.

**How CLMM oracles work in v4:** The code reads `sqrt_price` directly from Orca Whirlpool or Raydium CLMM pool accounts (`amm_cpi.rs`). This is a **spot value with zero TWAP or smoothing** — it reflects the last trade. The derived price is multiplied by the quote token's Pyth oracle price.

**Critical gaps in existing mitigations:**

| Mitigation | Effective Against CLMM Manipulation? |
|---|---|
| `conf_filter` | **NO** — deviation is inherited from the Pyth quote feed, not the pool. A manipulated `sqrt_price` produces zero deviation. |
| `max_staleness` | **NO** — checks the quote Pyth feed's slot, not pool trading activity. A stale pool passes if USDC Pyth is fresh. |
| `stable_price` | **Partial** — helps for Init health cross-tx attacks (price growth rate-limited to ~0.03%/s). Useless for Maint health and same-tx attacks. |
| `token_update_index_and_rate` whitelist | **NO** — only protects that single instruction. All other oracle consumers (withdraw, liquidation, perp orders, health checks) are unprotected. |

**Attack scenario — same-transaction sandwich (CONFIRMED VIABLE):**
1. **Instruction 1:** Swap on Orca/Raydium pool to inflate `sqrt_price` (cost depends on pool liquidity — as low as $50-200K for thin CLMM pools)
2. **Instruction 2:** Call `token_withdraw` on Mango v4 — the health check reads the now-inflated CLMM price via `bank.oracle_price()`, computing inflated collateral value
3. **Instruction 3:** Swap back to restore pool price
4. **Result:** Attacker borrows more than their collateral should allow

This works because `token_withdraw`, `perp_place_order`, liquidation instructions, and ALL health cache constructions read live CLMM `sqrt_price` without any cross-validation.

**Unprotected code paths that read CLMM oracles:**

| Instruction | Whitelist Protected? |
|---|---|
| `token_withdraw` | **NO** |
| `token_deposit` | **NO** |
| `perp_place_order` | **NO** |
| `perp_settle_fees` / `perp_settle_pnl` | **NO** |
| All liquidation instructions | **NO** |
| `serum3_place_order` / `serum3_settle_funds` | **NO** |
| Health cache construction (all health checks) | **NO** |

**Fix — Mandatory dual-oracle band check for CLMM tokens:**

Any token using a CLMM oracle (Orca or Raydium) MUST also have a Pyth or Switchboard oracle configured (via `fallback_oracle`). At every oracle price read, cross-validate:

```rust
fn validated_clmm_price(
    clmm_price: I80F48,
    reference_price: I80F48,  // Pyth or Switchboard
    max_deviation_bps: u16,   // e.g., 500 = 5%
) -> Result<OracleState> {
    let deviation = ((clmm_price - reference_price).abs()) / reference_price;
    let max_dev = I80F48::from_num(max_deviation_bps) / I80F48::from_num(10_000);

    if deviation > max_dev {
        // Outside band: use conservative price
        // For assets: min(clmm, reference) — undervalues collateral
        // For liabilities: max(clmm, reference) — overvalues debt
        // Or reject entirely:
        return Err(MangoError::OracleConfidence.into());
    }
    // Within band: use CLMM price (higher granularity for DEX-native tokens)
    Ok(clmm_price_state)
}
```

**Additional recommendations:**
1. **Enforce dual oracle at registration time:** When `oracle_config.oracle_type` is CLMM, require `fallback_oracle != Pubkey::default()` and verify it's Pyth/Switchboard.
2. **Add CLMM-specific deviation:** Compute deviation as `|clmm_price - reference_price| / reference_price` rather than inheriting quote feed deviation.
3. **Minimum pool liquidity check:** At registration time, verify TVL exceeds a minimum threshold (e.g., $1M). Consider periodic re-validation.
4. **Document which tokens use CLMM oracles** and ensure they are low-weight / low-borrow-limit assets proportional to pool liquidity.

**Performance note:** The dual-oracle read adds one additional account load (~2K CU) and one price comparison (~500 CU) per oracle read. For a typical health check scanning 4-6 tokens, this adds ~15K CU total — well within budget. The band check is a single comparison, not an expensive computation.

---

### H-8: Cranker MEV — Selective Lane Execution
**File:** `execution-queue-cranker.ts:478-543`, onchain `execution_queue.rs:1521-1547`

**Status: Substantially mitigated by VDF-based temporal FIFO.**

The VDF-based sequencer (separate repo) provides cryptographically verifiable ordering at 100μs resolution. Since enqueue order is determined by the VDF sequencer — not the cranker — the cranker cannot profitably reorder transactions. The onchain queue maintains the VDF-determined sequence.

**Remaining concern:** The `find_matching_ctm_item` scan-ahead allows the cranker to execute non-head items when the head doesn't match its provided lanes. This is a legitimate optimization for gap handling, but could theoretically allow executing a later item before an earlier one.

**Fix — Strict head-only FIFO with clean gap handling:**

1. **Remove `find_matching_ctm_item` scan-ahead** from `execute_multi`. The cranker must provide lanes matching the queue head.
2. **Gap handling via `gap_wait_slots`**: When the head is a gap, the cranker waits the configured `gap_wait_slots` (see M-3 analysis), then skips. This is the only permitted exception to strict FIFO.
3. **Stuck/failing head items:** If the head item fails dispatch (health check, expired, etc.), implement a **retry counter with max retries** (e.g., 3 retries). After max retries, the item is automatically marked `Failed` and cleared, allowing the queue to advance. The `retries` field already exists on `QueueItem`.

```rust
// In execute path, after dispatch failure:
if candidate.retries >= MAX_DISPATCH_RETRIES {
    queue.clear_ctm_item_at(candidate.sequence);
    emit!(QueueItemProcessed { status: Failed, sequence: candidate.sequence, ... });
    continue; // Advance to next item
}
queue.increment_retry(candidate.sequence);
break; // Stop this execute call, retry next time
```

**Performance note:** Removing scan-ahead slightly reduces the cranker's ability to pack multiple lanes efficiently when gaps exist. In practice, with proper `gap_wait_slots` and the VDF sequencer ensuring few gaps, this has negligible throughput impact.

---

### H-9: Harness PostOnly Order Matching Bug
**File:** `ts/client/src/continuumHarness.ts:1371`

The harness sets `canRest = !isImmediateOnly && !isPostOnly`, but PostOnly orders **do** rest if they don't cross. This causes the optimistic orderbook view to be wrong — phantom fills and missing resting orders.

**Fix:** `canRest = !isImmediateOnly` — PostOnly orders rest; they only fail if they *would* cross.

---

### H-10: gRPC/HTTP Bridge Has No Authentication
**File:** `ctm-sequencer-relayer.ts:867-868`, `ctm-relayer-http-bridge.ts`

The gRPC server uses `createInsecure()`. The HTTP bridge sets `Access-Control-Allow-Origin: *`. Any network actor can submit orders.

**Target functionality — API access control module (to be implemented separately):**

**Tier 1: Frontend users (whitelisted)**
- Access: IP whitelist for known frontend domains/CDN IPs
- Rate limit: 100 requests/second per IP
- Auth: None required (IP-based)
- CORS: Restrict `Access-Control-Allow-Origin` to frontend domain(s) only

**Tier 2: Free API key holders**
- Access: API key in `Authorization: Bearer <key>` header
- Free allocation: 10,000 requests/day per key
- Rate limit: 20 requests/second per key
- Burst: Up to 50 requests in a 1-second window
- Key provisioning: Self-service via registration endpoint

**Tier 3: Premium API access (future)**
- Higher limits, SLA guarantees, dedicated endpoints

**Placeholder configuration:**
```json
{
  "auth": {
    "frontend_whitelist_ips": ["<CDN_IP_RANGE>"],
    "frontend_rate_limit_rps": 100,
    "free_api_daily_quota": 10000,
    "free_api_rate_limit_rps": 20,
    "free_api_burst_limit": 50,
    "api_key_header": "Authorization",
    "cors_allowed_origins": ["https://app.example.com"]
  }
}
```

**Immediate mitigations (before auth module):**
1. Replace `Access-Control-Allow-Origin: *` with specific frontend origin
2. Enable TLS on gRPC (use `grpc.ServerCredentials.createSsl()`)
3. Add basic API key check as middleware

---

### H-11: Stale Oracle in PnL Settlement
**Files:** `perp_settle_pnl.rs:66-74`, `perp_settle_fees.rs:33-42`

Both pass `None` for staleness slot when fetching oracle prices. PnL settlement amounts are computed from potentially stale prices, enabling selective settlement timing.

**Fix:** Pass `Some(Clock::get()?.slot)` for staleness validation. `Clock::get()` is a Solana sysvar syscall available to any onchain instruction — it's already called on line 39 of `perp_settle_pnl.rs` for `now_ts`. The only added cost is a few CU for the staleness comparison inside `oracle_price`.

```rust
let slot = Clock::get()?.slot;
let oracle_price = perp_market.oracle_price(
    &OracleAccountInfos::from_reader(oracle_ref),
    Some(slot),  // was None
)?;
let settle_token_oracle_price = settle_bank.oracle_price(
    &OracleAccountInfos::from_reader(settle_oracle_ref),
    Some(slot),  // was None
)?;
```

---

### H-12: `force_close` in Testing Mode Destroys Positions
**File:** `instructions/account_close.rs`

Admin can enable `testing` mode on any group via `group_edit`, then force-close accounts with active borrows, perp positions, and open orders — silently erasing bad debt.

**Fix:** Prevent toggling `testing` flag on groups with nonzero total deposits, or require a separate governance vote. The H-6 timelock (30 min) on testing flag changes also mitigates this.

---

## MEDIUM Findings

### M-1: Envelope `expires_at_slot` Not Checked at Execute Time
**File:** `execution_queue.rs:1016`

The `expires_at_slot` field is only checked during `enqueue_ctm`. A queued item could sit in the queue indefinitely and still be executed long after its intended expiry, leading to stale orders placed at outdated prices.

**Fix:** Add `require!(item.expires_at_slot == 0 || clock.slot <= item.expires_at_slot)` in the execute path. ~200 CU cost per item check — negligible.

---

### M-2: CTM Signer Rotation Has No Minimum Delay
**File:** `execution_queue.rs:909-918`

`execution_queue_set_ctm_pending` allows the admin to set any `activate_at_slot`, including `0` or the current slot. Instant rotation prevents detection of compromised keys.

**Fix:** Enforce `activate_at_slot >= current_slot + MIN_ROTATION_DELAY`.

---

### M-3: Gap-Skip Tuning — Critical Design Choice
**File:** `execution_queue.rs:1179-1209`

**The exact exploit vector:**

1. The CTM signer assigns sequences. If a transaction carrying sequence N is dropped (RPC failure, network congestion) but sequence N+1 lands, a gap forms at N.
2. `gap_observed_slot` is set. The queue is **blocked** for `gap_wait_slots` slots — no CTM items execute.
3. After the wait, sequences are skipped (up to 32 per execute call), and the queue resumes.
4. A malicious or compromised CTM signer can intentionally create gaps to stall the queue. An attacker creating gaps every 10 sequences imposes repeated `gap_wait_slots * 400ms` stalls.

**Note:** With the VDF sequencer in place, intentional gap creation by the relayer is not possible (the VDF enforces sequence integrity). Gaps are caused only by network-level transaction drops.

**Solana inclusion rate data (sources: Chorus One, bloXroute, Helius):**

| Metric | Standard RPC | SWQoS RPC | SWQoS + Priority Fee |
|---|---|---|---|
| p50 inclusion | 5-10 slots (~2-4s) | 1-2 slots (~0.4-0.8s) | 1-2 slots (~0.4-0.8s) |
| p90 inclusion | 15-25 slots (~6-10s) | 3-4 slots (~1.2-1.6s) | 2-3 slots (~0.8-1.2s) |
| p95 inclusion | 25-40 slots (~10-16s) | 5-8 slots (~2-3.2s) | 4-6 slots (~1.6-2.4s) |
| p99 inclusion | 50-75+ slots (~20-30s) | 10-15 slots (~4-6s) | 8-12 slots (~3.2-4.8s) |

**Tradeoff matrix:**

| gap_wait_slots | Stall Duration | Queue Drain Impact | FIFO Strength | P(legit tx lands in time) with SWQoS | MEV Window |
|---|---|---|---|---|---|
| 2 (current) | 800ms | Minimal per-gap | Weak — ~60-70% of txs land | Low | 800ms |
| **4 (recommended)** | **1.6s** | **Minimal per-gap** | **Moderate — ~80-85%** | **Good** | **1.6s** |
| 6 | 2.4s | Low per-gap | Good — ~88-92% | Strong | 2.4s |
| 8 | 3.2s | Moderate per-gap | Strong — ~92-95% | Very strong | 3.2s |
| 10 | 4.0s | Noticeable per-gap | Very strong — ~95-97% | Near-perfect | 4.0s |
| 15 | 6.0s | Significant per-gap | Near-perfect — ~98-99% | Near-perfect | 6.0s |

**Key insight:** Throughput degradation scales sub-linearly with `gap_wait_slots` (the stall fraction difference between 4 and 10 slots is only ~1.2% at 5% gap rate). What changes is **latency** per stall and **FIFO ordering fidelity**.

**Recommended: `gap_wait_slots = 4`**

Rationale:
- With SWQoS RPCs (mandatory for production — see DevOps below), p80-p85 of transactions land within 4 slots.
- 1.6s stall per gap is short enough to not materially affect queue drain rate.
- The VDF sequencer eliminates intentional gap attacks, so gaps are purely network jitter — optimizing for the common case.
- 4 slots strikes the best balance between fast drain (critical for TPS) and inclusion likelihood.

**DevOps optimization spectrum (essential companion to gap_wait_slots tuning):**

| Strategy | Impact | Cost | Priority |
|---|---|---|---|
| **SWQoS RPCs** (Helius, Triton, bloXroute) | **3x** inclusion improvement — single highest-impact optimization | $500-5000/mo | **MANDATORY** |
| **Multi-endpoint submission** (2-3 providers) | Converts p90 → ~p99 landing rate | 2-3x RPC cost | HIGH |
| **Priority fees** (10k-50k μlamports/CU) | ~10-20% better landing rate | ~$0.001-0.01/tx | HIGH |
| **Transaction retry** (3 retries, 2-slot backoff) | Catches stragglers | Negligible | HIGH |
| **Jito bundles** | Atomicity, not latency | 10k-100k lamport tips | OPTIONAL |
| **Co-located validators** | Sub-50ms p99 latency | $2000+/mo | FUTURE |

**The full optimization picture:** `gap_wait_slots` is not a knob to tune in isolation. The optimal configuration is `gap_wait_slots = 4` **combined with** SWQoS RPCs + multi-endpoint submission + priority fees + retries. This combination targets p99 inclusion within 4 slots, making false gap-skips extremely rare while keeping stalls short.

---

### M-4: Sequence State on `/tmp` — Local DoS Vector
**File:** `ctm-sequencer-relayer.ts:71`

The sequence counter is persisted to a world-writable `/tmp` directory. Symlink attacks or direct writes can reset or corrupt the counter.

**Fix:** Use a secured path with restricted file permissions (`0600`).

---

### M-5: Unbounded Event Log Growth
**File:** `continuum-state-harness.ts:313-316`

Events are appended to a JSONL file without rotation. The `replayEventLogIfPresent` reads the entire file into memory on startup, eventually causing OOM.

**Fix:** Implement log rotation with a maximum file size.

---

### M-6: Race Condition in Non-Serialized Sequence Assignment
**File:** `ctm-sequencer-relayer.ts:795-805`

When `RELAYER_SERIALIZE_SUBMITS=false` (default), failed transactions leave permanent gaps in the queue. The TypeScript relayer lacks the recyclable sequence mechanism present in the Rust engine.

**Fix:** Port the recyclable sequence mechanism from the Rust engine.

---

### M-7: SQL Table Name Interpolation in HTTP Bridge
**File:** `ctm-relayer-http-bridge.ts:217-241`

`TSDB_TABLE` is interpolated directly into SQL queries without parameterization. Exploitable only via environment variable manipulation.

**Fix:** Validate against `^[a-z_][a-z0-9_]*$`.

---

### M-8: Health-Gated Dispatch Failure Rolls Back Entire Batch
**File:** `execution_queue.rs:1356-1365`

A single toxic order that fails the health check causes the entire transaction (including other users' orders) to revert.

**Analysis:** ALL `PerpPlaceOrderV2` items dispatched via the execution queue are health-gated (`queue_health_region_spec` returns `Some(...)` for this variant). Limiting `max_items=1` would mean **1 perp order per transaction** — a severe throughput regression (~4-8x reduction in peak order processing rate).

**Root cause:** `dispatch_perp_place_order_v2` mutates perp book state directly (not via CPI), so there's no per-item rollback boundary. A failed health check after mutation must roll back the entire transaction.

**Fix — Hybrid approach (balances throughput and safety):**

1. **Convert health-gated dispatches to CPI (invoke to self):** If `PerpPlaceOrderV2` is dispatched via CPI rather than direct function call, a failed CPI does not corrupt the parent state. The queue executor can catch the error, mark the item as failed, and continue to the next item. This adds ~25K CU per item for CPI overhead, but a batch of 4 orders would cost ~4 × 315K = ~1.26M CU — still within the 1.4M limit.

2. **Offchain cranker skip-list (interim, no program change):** The cranker maintains a skip-list of items that fail in simulation (`simulateTransaction`). Known-toxic items are excluded from `execute_multi` batches and flagged for admin cleanup via `execution_queue_drop_ctm`. This provides immediate relief without a program upgrade.

3. **Pre-validation fast-reject:** Before entering the health region, check cheap failure conditions (order expired? market paused? account exists?) and skip the item if they fail. Cost: ~5K CU vs. ~120K CU for a full health check that will fail anyway.

**Performance note:** The CPI approach trades ~25K CU per item for the ability to safely batch 4-5 health-gated orders per transaction. Net throughput increases ~3-4x vs. `max_items=1`, at the cost of ~8% CU overhead per item vs. direct dispatch.

---

### M-9: TCS Premium Uses f64 — Precision Loss for Extreme Price Ratios
**File:** `token_conditional_swap.rs:242-270`

TCS pricing computation uses `f64` for oracle price division and premium/fee calculations. For extreme price ratios (e.g., BTC/BONK), precision loss could lead to systematically unfair pricing.

**Fix:** Use I80F48 for intermediate calculations.

---

### M-10: Interest Accrual Capped at 1 Hour — Lost During Outages
**File:** `token_update_index_and_rate.rs:100-102`

If not called for more than 1 hour (Solana downtime), interest accrual is permanently lost, benefiting borrowers at depositors' expense.

**Fix:** Track and compensate for missed accrual, or increase the cap with rate smoothing.

---

### M-11: `token_force_withdraw` Is Permissionless Once Flag Set
**File:** `accounts_ix/token_force_withdraw.rs`

Once `bank.is_force_withdraw()` is true, anyone can trigger withdrawals on any account. No rate limiting.

**Fix:** Add rate limiting or require a specific authority as signer.

---

### M-12: Funding Rate Manipulable via Multi-Block Orderbook Spam
**File:** `perp_market.rs:304-377`

Funding rate is computed from impact price at update time. An attacker can skew the orderbook across multiple blocks to bias funding.

**Fix:** Add TWAP-based funding calculation.

---

### M-13: Collateral Fee Timing Is Gameable
**File:** `token_charge_collateral_fees.rs:37`

Charge capped at 2x the interval. A newly enabled fee with large interval gives discounts to existing borrowers; a bot can force-charge others at the earliest moment.

**Current code uses elapsed wall-clock time:**
```rust
let charge_seconds = (now_ts - last_charge_ts).min(2 * group.collateral_fee_interval);
```

**Fix — Use slots instead of seconds for more precise onchain timing:**

```rust
let charge_slots = (clock.slot - last_charge_slot).min(2 * group.collateral_fee_interval_slots);
let charge_duration = I80F48::from_num(charge_slots) * SLOT_DURATION_SECONDS; // ~0.4s
```

This requires adding `last_charge_slot` and `collateral_fee_interval_slots` fields. Slot-based timing is more deterministic onchain (no reliance on `unix_timestamp` approximations) and granular for short durations. For long durations (hours+), slot-based timing is equally valid since `slot * 0.4s` is a close approximation that doesn't drift significantly.

Remove the 2x cap — use actual elapsed slots. The cap was intended to prevent extreme catch-up charges, but it creates an exploitable discount. Instead, apply the full elapsed duration; if the interval is missed by a large margin, the charge should reflect reality.

**Performance note:** Replacing `unix_timestamp` with `slot` requires reading `Clock::get()?.slot` (already done in most instructions) — zero additional CU cost.

---

### M-14: IxGateSet Writes State Before Auth Check
**File:** `ix_gate_set.rs:104-118`

The `ix_gate` is written on line 104, but authorization is checked on line 107-118. Safe due to transaction atomicity, but violates defense-in-depth.

**Fix:** Move auth check before state write.

---

### M-15: Flash Loan Delegate Allowlist Hardcodes Jupiter Versions
**File:** `flash_loan.rs:143-151`

Jupiter v3/v4/v6 program IDs are hardcoded. New versions require a program upgrade; old vulnerable versions cannot be removed.

**Fix:** Make the allowlist configurable via group parameters.

---

### M-16: `compute_equity` Missing Perp Contributions
**File:** `equity.rs:61`

The function has a `// TODO: perp contributions` comment and returns an empty perps vector. Not used for safety-critical decisions (health uses HealthCache), but any dependent logic will undercount equity.

**Fix:** Implement perp equity computation.

---

## LOW Findings

| ID | Summary | File |
|----|---------|------|
| L-1 | Pyth fallback to `prev_price` when non-Trading — could be significantly stale | `oracle.rs:205-231` |
| L-2 | StubOracle staleness bypass when `last_update_slot == 0` — perpetually fresh | `oracle.rs:319-323` |
| L-3 | `TokenDepositIntoExisting` has no owner check — anyone can deposit into any account | `accounts_ix/token_deposit.rs` |
| L-4 | Insurance fund withdrawal has no destination constraint — admin can send anywhere | `accounts_ix/group_withdraw_insurance_fund.rs` |
| L-5 | Stable price model uses f64 arithmetic — platform-dependent rounding | `state/stable_price.rs` |
| L-6 | Dust accumulation in bank from rounding — small accounting deficit over time | `state/bank.rs:616-621` |
| L-7 | User intent signature doesn't bind sequence/timing — relayer can reuse | `execution_queue.rs:247-263` |
| L-8 | TCS ID counter uses `wrapping_add` — theoretical collision after 2^64 | `token_conditional_swap_create.rs:34` |
| L-9 | Self-trade doubles `closed_pnl` in fill log — misleading analytics | `perp_consume_events.rs:69-95` |
| L-10 | Settle fee is zero below 1% PnL/position ratio — no cranker incentive for small settlements | `perp_market.rs:440-443` |
| L-11 | Liquidity items have `sequence: 0` — no individual tracking in event logs | `execution_queue.rs:1114` |
| L-12 | No close/dealloc instruction for execution queue account (~2.9 SOL locked permanently) | N/A |
| L-13 | Blockhash cache staleness causes sporadic tx failures under load | `ctm-sequencer-relayer.ts:608-629` |

---

## Architectural Observations

### Trust Model
The execution queue / CTM system has a **layered trust model**:
- **VDF Sequencer** (separate repo): Provides cryptographically verifiable temporal FIFO ordering at 100μs resolution. Eliminates relayer reordering/front-running.
- **Relayer**: Submits VDF-sequenced transactions. Cannot reorder (VDF-bound), but remains a liveness dependency. Mitigated by direct-submit fallback (C-4 fix).
- **Cranker**: Executes queue items. Strict head-only FIFO (H-8 fix) prevents MEV extraction.
- **Harness**: Optimistic simulation diverges from onchain reality (PostOnly bug H-9, no liquidation modeling). Frontend should display confirmed state for financial decisions.
- **Admin**: Near-total control. Mitigated by 30-minute timelock (H-6) and `is_testing()` guard (C-3).

### What's Working Well
- Flash loan reentrancy protection (transaction introspection + CPI block) is excellent
- Health calculation is conservative by design (underestimates rather than overestimates)
- Liquidation phase ordering prevents premature socialized losses
- `being_liquidated` flag prevents cascading liquidation loops
- Token withdraw delegate restrictions are well-implemented
- Interest rate curve math is correct and well-tested
- The `new_health_cache_skipping_missing_banks_and_bad_oracles` variant correctly requires oracles for negative positions

### Priority Remediation Order
1. **Immediate (C-1, C-2):** Fix `execute_multi` lane hash verification — this is exploitable now
2. **Immediate (C-3):** Add `is_testing()` guard to `unsafe_deposit`
3. **Immediate (H-7):** Implement dual-oracle band check for CLMM tokens — v3-style exploit vector
4. **Short-term (C-4):** Implement direct-submit fallback with 10-slot delay
5. **Short-term (C-5, H-4):** Fix bankruptcy accounting edge cases
6. **Short-term (H-8, M-3):** Enforce strict FIFO + set `gap_wait_slots = 4` + deploy SWQoS infrastructure
7. **Medium-term (H-3):** Fix insurance fund rounding drainage
8. **Medium-term (H-6):** Implement 30-minute timelock on admin operations
9. **Medium-term (M-8):** Convert health-gated dispatch to CPI for safe batching
10. **Longer-term:** Auth module for API access (H-10), remaining MEDIUM/LOW items
