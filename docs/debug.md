# v4 devnet debug log — April 2026

Cataloged fixes applied during the v4 devnet stand-up. Each section is a self-contained debugging walkthrough: symptom → root cause → fix. Intended as a reference for future operators who hit similar failure modes.

## 1. `group account mirror bootstrap found no bank/perp accounts`

**Symptom:** gRPC `submit_intent` rejected at first request with the above message, even though on-chain `get_program_accounts(<program_id>)` returns banks and perp markets owned by that program with `bank.group == v4_group`.

**Root cause:** `LoadZeroCopy::load<T>()` (in `programs/mango-v4/src/accounts_zerocopy.rs`) checks `self.owner() != &T::owner()` where `T::owner()` resolves via Anchor's `Owner` derive to `crate::ID` (the value in `declare_id!`). The relayer links against the `mango-v4` crate, so its `T::owner()` call returns whatever is hard-coded in `programs/mango-v4/src/lib.rs`. If the on-chain accounts are owned by program X but the relayer binary was compiled with `declare_id!("Y")`, every `load::<Bank>()` in the mirror bootstrap fails with `AccountOwnedByWrongProgram`, and the mirror ends up empty even though the underlying account data is sound.

**Fix:** keep `declare_id!` in `programs/mango-v4/src/lib.rs` aligned with the deployed program ID and rebuild BOTH the SBF binary AND the relayer binary after any program re-deploy. Verify with `strings target/deploy/mango_v4.so | grep ExecutionQueueV4` and by scanning the relayer binary for the raw 32-byte program-id bytes.

**Related pitfall:** the systemd env file (`/home/hetalkenaudekar/stagin4/mng-v4/.devnet/systemd/devnet-stack.env`) holds `PROGRAM_ID`, `CTM_RELAYER_PROGRAM_ID`, `V4_*`, `CONTINUUM_HARNESS_*`, and `EXECUTION_QUEUE_*`. Drifting any of them produces the same "no accounts" error for different reasons (wrong program → nothing to mirror; wrong group → nothing matches the filter).

## 2. `ExecutionQueueV3AssignedPageMismatch` (custom 6106) at commit

**Symptom:** every `commit_market_batch` tx fails on-chain with error 6106 at `execution_queue_v4.rs:278`. Relayer log shows `status_label="submitted"` (gRPC accepted), but nothing reaches the orderbook.

**Root causes (compound):**
1. Only `queue_page0` was initialized but `queue_root.num_pages > 1`. When sequence crosses `page_size` (256 by default), the handler's `page_slot_for_sequence(seq)` returns 1/2/3, but `queue_page0.page_slot == 0` → mismatch at line 278.
2. The relayer's `build_commit_market_tx` always sent the SAME `mkt.queue_page0` regardless of which page the current sequence mapped to.
3. A previous drift problem (see §3) pushed the local counter far ahead of on-chain, amplifying #1: commits were arriving at sequences well past any initialized page.

**Fix:**
- Added `V4MarketState::queue_page_for_sequence(sequence)` that derives the correct page PDA from `page_size`/`num_pages`.
- Both `build_commit_market_tx` and `build_reveal_execute_tx` now use it.
- Added a **page-init worker** (`spawn_page_init_worker`) that scans `[0, num_pages)` at startup + periodically (`V4_PAGE_INIT_REFRESH_SECS`) and creates missing pages via `create_market_page → resize_market_page×N → init_market_page`. The resize loop handles the BPF 10 240-byte per-tx grow cap.

**Key constant:** `CommitPageV4` is 24 648 bytes full-size. Create starts at 8 bytes, needs 3 resizes to reach full size, then `init_market_page` can run.

## 3. Relayer local-seq drift from on-chain

**Symptom:** commits fail with 6106/ExecutionQueueFull, but logs show them "succeeding" from the gRPC layer. Admin balance drains rapidly on tx fees but orderbook never updates.

**Root cause:** `v4_next_sequence.fetch_add(1)` is called on every successful gRPC submit, then the commit tx is sent with `skip_preflight=true, max_retries=0`. On-chain failures are **silent** (the relayer never polls signature status). So the local counter grows monotonically while `queue_root.next_enqueue_sequence` stays stuck at whatever actually landed. Drift of 70+ sequences accumulates in minutes; eventually the local counter passes the admission window (`page_size × num_pages`) and every commit is rejected.

**Fix:** `spawn_drift_resync_worker` runs every `V4_DRIFT_RESYNC_INTERVAL_MS` (default 3 s). It reads `queue_root.next_enqueue_sequence` and if `local_counter > on_chain_next + V4_DRIFT_MAX_LOOKAHEAD`, stores `on_chain_next` into the atomic counter. Safe even with concurrent submits — the on-chain `validate_commit_sequence` will reject duplicates and the next resync re-aligns.

**Log signature:**
```
v4_drift: local counter drifted; rewinding to on_chain_next local=1350 on_chain_next=1278 drift=72
```

## 4. Silent reveal worker — `Err(_e) => {}`

**Symptom:** head stuck at a specific sequence for hours, no visible errors, `live_count` grows unbounded.

**Root cause:** the original `spawn_reveal_worker` used `match rpc.send_transaction_with_config(...) { Ok(_) => {}, Err(_e) => {} }` — ALL send failures were swallowed. On-chain reveal failures (e.g. hash mismatch, dispatch-payload mismatch) were similarly invisible: the worker fired, got a sig back, and never polled status.

**Fix:**
- `Err(_e) => {}` replaced with `warn!(target: "v4_reveal", seq, error = %e, "reveal send failed")`.
- Added a periodic `info!` heartbeat at `v4_reveal` showing `head`, `max_seen`, `live_count`, `last_fired` every 15 s or on head change.
- Added `V4HeadStallState` shared with the autodrop worker so reveal-attempt count per stalled head is tracked.

## 5. Autodrop infrastructure + "ExecutionQueueFull"

**Symptom:** queue saturates at `num_pages × page_size` live items. New commits fail with `ExecutionQueueFull` (error 6076 at line 287). Manual intervention (`drop_head_market` via CLI) required to unstick.

**Design:** `spawn_autodrop_worker` detects unadvanceable heads and sends `pause_execute → drop_head_market(N) → unpause_execute` as a single atomic tx, signed by the admin/CTM key.

**Detection:** head has not advanced for `V4_AUTODROP_STALL_SECS` AND the reveal worker has attempted at least `V4_AUTODROP_MIN_ATTEMPTS` reveals on it AND `live_count > 0`.

**Rate limit:** `V4_AUTODROP_MAX_PER_MIN` calls per minute, each dropping up to `V4_AUTODROP_COUNT_PER_CALL` items (default 1, but for a backlog bump to 50).

**Back-off on unsupported program:** if the deployed program lacks the `drop_head_market` handler (emits Anchor custom error 101, `InstructionFallbackNotFound`), the worker disables itself for 1 h and logs:
```
deployed program lacks drop_head_market handler — autodrop disabled for 1h.
Redeploy the on-chain program (<program_id>) to enable autodrop.
```

**Typical defaults during normal operation:**
```
V4_AUTODROP_STALL_SECS=30
V4_AUTODROP_MIN_ATTEMPTS=6
V4_AUTODROP_MAX_PER_MIN=30
V4_AUTODROP_COUNT_PER_CALL=1
V4_AUTODROP_POLL_SECS=10
```

**Emergency backlog drain** (raise when live_count >> head velocity):
```
V4_AUTODROP_STALL_SECS=5
V4_AUTODROP_MIN_ATTEMPTS=3
V4_AUTODROP_MAX_PER_MIN=120
V4_AUTODROP_COUNT_PER_CALL=50
V4_AUTODROP_POLL_SECS=3
```

## 6. `ExecutionQueueV4CommitRevealMismatch` (custom 6114) — the hash-scope bug

**Symptom:** 100 % of reveals fail at reveal time with error 6114. On-chain log: `execution queue v4 reveal does not match the stored commit hash`. Commits all succeed; reveals all fail the same way.

**Root cause:** the `accounts_hash` component of the `commit_hash` computed by the relayer did not match what the on-chain reveal path re-computed. The on-chain verifier calls `account_metas_from_infos(dispatch_accounts)`, which reads flags from `AccountInfo` — those flags are **globally flag-OR'd by the Solana runtime across the entire tx**, including the fixed `ExecutionQueueV4RevealExecuteMarket` accounts. The relayer's `hash_account_metas_runtime` flag-OR'd only WITHIN the dispatch list. `group` is declared `#[account(mut)]` in the reveal Accounts struct → writable in the tx → merged to writable at EVERY position (including the dispatch duplicate at index 0 of the dispatch slice) → relayer hashed it as readonly, on-chain saw it as writable → divergent hashes.

**Confirming evidence:** at commit time, `accounts_hash_hex=68c81a22de175375...`; at reveal time (same seq, same dispatch_accounts, same relayer-computed hash via old function) also `68c81a22de175375...`; on-chain `expected_commit_hash` differs only in the accounts_hash byte subfield.

**Fix:** new helper `hash_dispatch_accounts_for_reveal(mkt, dispatch_accounts)` in `bin/service-mango-execution-engine/src/v4_pipeline.rs`. Before hashing, it flag-OR's dispatch_accounts against the six accounts the reveal tx adds implicitly:

```rust
let fixed: [AccountMeta; 6] = [
    AccountMeta::new(mkt.group, false),              // #[account(mut)]
    AccountMeta::new_readonly(mkt.authority_state, false),
    AccountMeta::new(mkt.queue_root, false),         // #[account(mut)]
    AccountMeta::new(mkt.queue_page0, false),        // #[account(mut)], any page — same flags
    AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false),
    AccountMeta::new_readonly(mkt.program_id, false),// trailing
];
```

Any future change to the reveal Accounts struct (adding a `mut` field or swapping a `#[account(mut)]` for `#[account]`) must be mirrored here or 6114s return.

**Result:** reveal success rate 16 % → 100 %.

## 7. Relayer ignores caller-supplied `remaining_accounts`

**Symptom:** when a market is redeployed (e.g., perp_market close + recreate with new `bids`/`asks`/`event_queue` keypairs), senders still reference the old accounts and every intent fails.

**Fix:** `CTM_RELAYER_IGNORE_SUPPLIED_REMAINING_ACCOUNTS=true` env var routes all v1/v2 intents through the relayer's `build_derived_perp_remaining_accounts`. The caller's list is logged at `DEBUG` and discarded. Senders can keep sending a stale list (or an empty one) indefinitely, and the relayer's mirror-derived set is the single source of truth.

Wired in `derive_submit_remaining_accounts` in `bin/service-mango-execution-engine/src/main.rs`.

## 8. `InstructionFallbackNotFound` (custom 101) — stale program binary

**Symptom:** admin tx (e.g., `drop_head_market`) fails at the handler dispatch with `Anchor custom error 101`. Discriminator bytes computed by the client look correct. No ix-name string for the failing handler appears in the deployed binary.

**Root cause:** the program binary on-chain is older than the source code. `cargo build-sbf` hasn't been run (or hasn't been redeployed) since the handler was added. Anchor's dispatch match fails → returns `InstructionFallbackNotFound`.

**Diagnosis:**
```bash
solana --url <rpc> program dump <program_id> /tmp/onchain.so
strings /tmp/onchain.so | grep "Instruction: ExecutionQueueV4"
```
Compare with the expected list from `programs/mango-v4/src/lib.rs` — any missing name means the program is stale.

**Fix:** rebuild with `cargo build-sbf --features enable-gpl` and `solana program deploy --program-id <keypair> <path>.so`. If you don't have the program keypair on disk, the address is irrecoverable — see §9.

## 9. Program keypair persistence

**Rule (also in top-level CLAUDE.md):** every keypair generated for any operation goes into `/home/hetalkenaudekar/secure/keypairs/` (mode 700), with a self-describing filename. Never `Keypair::new()` and discard. Never `solana-keygen new --outfile /tmp/*` — keypairs have been lost that way.

**Why it matters:** `solana program close` returns the PROGRAMDATA rent, but the address is permanently retired afterward — it cannot be redeployed. If the program keypair is on disk you at least retain the option of re-initializing a buffer there (there are unofficial tools), but without the keypair and without the ability to close+re-deploy, you're stuck paying 34 SOL forever on a busted program or eating the full bootstrap cost of a replacement group/market/queue.

**How we lost and recovered 5KaJhG2…:** the original v4 program keypair was not on the current machine. When an in-place upgrade was needed (to add `drop_head_market`), we couldn't do it — the upgrade path needs ~34 SOL of temporary buffer rent and the admin wallet was low. The only way to recycle the locked SOL was `program close`, which retired the address. We then generated a fresh keypair, deployed `9Vua6curf…`, re-bootstrapped group/bank/perp/queue/pages, and re-airdropped bot USDC under the new mint. About two hours of work and a lot of blockhash churn.

## 10. `Blockhash not found` on Helius preflight

**Symptom:** `solana program deploy` or admin ops fail intermittently with `Transaction simulation failed: Blockhash not found` even when the blockhash was just fetched.

**Root cause:** Helius's preflight path uses a different commitment level than `get_latest_blockhash()` returns. Fetched-and-submitted blockhash has not yet propagated to the preflight node's view.

**Fix:** use `get_latest_blockhash_with_commitment(finalized)` + `skip_preflight=true` + manual `getSignatureStatuses` polling. Wrapped in `send_and_confirm_skip_preflight(...)` in `v4_pipeline.rs`. Used by all admin workers (page init, autodrop).

## 11. Two harness / two relayer processes

**Symptom:** updated env vars in `devnet-stack.env`, restarted `stagin4-devnet-relayer.service`, but the running process still has the old values.

**Root cause:** an orphan autoheal shell (launched manually earlier) was still running alongside the systemd-managed one. Both tried to bind the same port; whichever started first won. Same story for the `v4-harness` autoheal vs `harness` systemd unit — one is a leftover from pre-systemd days.

**Fix:** `sudo -n systemctl stop <svc>` + `ps aux | grep -E "autoheal.*relayer|service-mango-execution-engine"` — kill any orphaned PIDs with `sudo -n kill`, then start the service. See the memory at `/home/dm/.claude/projects/.../memory/ref_devnet_relayer_systemd.md`.

## 12. Reveal store wiped on restart → orphan commits

**Symptom:** after a relayer restart, the next several hundred sequences appear stuck (head doesn't advance, reveals never fire).

**Root cause:** the `v4_reveal_store` is an in-memory `BTreeMap<u64, V4RevealEntry>`. On restart, it's empty. Any sequences already committed on-chain but not yet revealed are orphaned — the relayer has no payload/dispatch_accounts to rebuild their reveal.

**Mitigation:** the autodrop worker drains orphans at `V4_AUTODROP_COUNT_PER_CALL` per call. For a known-orphan backlog, raise `V4_AUTODROP_COUNT_PER_CALL=50` and `V4_AUTODROP_POLL_SECS=3` temporarily (see §5 "emergency backlog drain"). Restore defaults once `live_count == 0`.

**Long-term:** persist the reveal store across restarts (local sqlite or JSON snapshot every N seconds). Also mark orphan attempts so reveal worker immediately notes them rather than cycling through empty sends.

## 13. Lot-size misconfiguration → $1 000/SOL tick

**Symptom:** orderbook endpoint shows `best_ask_ui ≈ 1_000_000_000` for SOL-PERP while oracle reports ~$86. Reveals succeed but no real matching happens.

**Root cause:** perp market created with `base_decimals=9, base_lot_size=100, quote_lot_size=100` → `price_tick_usd = (quote_lot_size × 10^base_dec) / (base_lot_size × 10^quote_dec) = 100 × 10^9 / (100 × 10^6) = 1000`. Bots submitting `price_lots = 1_000_100` mean "$1 000 100 000 per SOL" — nonsense.

**Fix:** redeploy the perp market with `base_decimals=5, base_lot_size=100, quote_lot_size=1` → tick = $0.001/SOL. Use the `redeploy-perp-market.ts` helper (close + recreate at same `perp_market_index`, new `bids`/`asks`/`event_queue`). Must update `V4_PERP_BIDS`, `V4_PERP_ASKS`, `V4_PERP_EVENT_QUEUE` in systemd env and restart.

## 14. Harness with wrong group → `mango_accounts: []` for every user

**Symptom:** harness `/state/users/<owner>` returns `"mango_accounts": []` even though on-chain `getAccountInfo(<mango_account_pda>)` shows a populated 4 500+ byte account.

**Root cause:** `CONTINUUM_HARNESS_GROUP_PK` in systemd env is either empty or pointing at a prior group. With `groupPk = null`, `OnchainContext.mangoClient` is never initialized, so `getAllMangoAccounts(group)` is never called and the engine's user map stays empty. With the wrong group, `getAllMangoAccounts` returns a different set.

**Fix:** update `CONTINUUM_HARNESS_PROGRAM_ID`, `CONTINUUM_HARNESS_GROUP_PK`, `CONTINUUM_HARNESS_USDC_MINT` together to match the current deployment, kill any orphan `v4-harness` processes, restart the systemd `stagin4-devnet-harness.service`.

## 15. Enqueue-time validation: fail at gRPC, not at reveal

**Design goal:** every check the on-chain `perp_place_order` runs should also run at the relayer's `submit_intent` endpoint, so that doomed orders are rejected BEFORE they eat a commit tx slot, queue capacity, and reveal CU. Bots get a synchronous `Status::FAILED_PRECONDITION` back and can retry intelligently.

**Current coverage** (`fn evaluate_submit_margin_precheck` in main.rs):

| Check | What on-chain would reject with | Relayer precheck |
|---|---|---|
| Group/market accounts owned by current program | `AccountOwnedByWrongProgram` / no-mirror | ✅ §1 |
| Payload version / variant / flags | `ExecutionQueuePayloadVersionUnsupported` / `…PayloadVariantInvalid` | ✅ `decode_margin_check_ops` |
| Payload body length | decode error at reveal | ✅ `decode_perp_place_order_margin_op` |
| Commit sequence in range | `ExecutionQueueV4InvalidCommitBatch` | on-chain only (counter rewind prevents drift) |
| Post-trade `init_health >= 0` | `HealthMustBePositiveOrIncrease` | ✅ optimistic apply + `health_cache.recompute` |
| Order `expiry_timestamp < now_ts` | drops as terminal-expired | ✅ **§15** |
| Order price outside `inside_price_limit(stable_price)` band | `OrderPriceOutsideBand` at reveal | ✅ **§15** (uses stable_price as a lagged oracle proxy) |
| Free perp OO slot | `NoFreePerpOpenOrderIndex` | ⚠️ not yet — low-priority, usually only trips on brand-new accounts before a deposit |
| Reduce-only market/bank mode | `MarketInReduceOnlyMode` / `TokenInReduceOnlyMode` | ⚠️ partial — margin precheck clamps `effective_base_lots` for `reduce_only=true`, but doesn't fail-fast on the MARKET-level flag |
| Self-trade block | `WouldSelfTrade` | cannot pre-check without a full orderbook view |
| Instantaneous fill price vs oracle (taker) | `OrderPriceOutsideBand` at match time | needs orderbook state; skipped |

**Why stable_price and not the spot oracle?** The stable_price is a field on `PerpMarket` itself (already loaded for health calc), so the check is free — no extra RPC. It's a lagged-EMA of the oracle, strictly more conservative than the spot price for post-only orders. An order priced so far out that it fails against even the lagged price would fail against the spot oracle too. Margin precheck still consults the spot oracle for health math; the band-check is just a cheap smoke test.

**What remains rolled back to the on-chain verifier:**
- Cross-order self-trade
- Precise CU-aware matching (an order that WOULD fill if the book had X at price Y)
- Slot-dependent `min_execute_slot` admission (on-chain is authoritative)
- Fresh oracle (stable_price is a lagged proxy; if a bot quotes very fresh but the stable price lags, we'd accept an order that fails on-chain against the wider band — rare, but not impossible)

**Log signatures for the new rejections:**
```
direct submit rejected reason=order already expired at enqueue: expiry_timestamp=<N> < now_ts=<M>
direct submit rejected reason=order price outside oracle band (stable): side=Bid price_lots=… native_price=… stable_price=…
```

## Quick reference — self-healing worker log targets

| Worker | Target | Key events |
|---|---|---|
| reveal  | `v4_reveal`       | `queue_root state`, `reveal send failed`, `no reveal material in store` |
| pages   | `v4_pages`        | `creating page account`, `resizing page`, `page initialized`, `page init failed` |
| drift   | `v4_drift`        | `local counter drifted; rewinding`, `local counter behind on-chain` |
| autodrop| `v4_autodrop`     | `head unadvanceable`, `admin drop sent/landed/failed`, `deployed program lacks drop_head_market handler` |
| commit  | `v4_commit_debug` (DEBUG) | per-intent accounts_hash/commit_hash/min/expires |
| reveal  | `v4_reveal_debug` (DEBUG) | per-reveal dispatch layout |

Enable the debug targets for diagnosis:
```
RUST_LOG=info,v4_commit_debug=debug,v4_reveal_debug=debug
```
