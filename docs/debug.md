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

## 16. Systemd binary built from wrong source tree

**Symptom:** 100 % of `submit_intent` calls rejected with `group account mirror bootstrap found no bank/perp accounts for group=<X>`; 100 % of autodrop admin txs fail with `InstructionError(1, UnsupportedProgramId)`. Env `PROGRAM_ID` matches what's deployed on-chain; on-chain program is healthy. Orderbook stops updating.

**Root cause:** the systemd unit `stagin4-devnet-relayer.service` runs `/home/hetalkenaudekar/stagin4/mng-v4/target/release/service-mango-execution-engine`. That path resolves (via symlink) to `/home/hetalkenaudekar/mng-v4/…`, which is a **separate source checkout** on a different branch (`vc6`) with a different `declare_id!` in `programs/mango-v4/src/lib.rs`. When a binary is built there, `crate::ID` (used by Anchor's `Owner` derive for `Bank::owner()` etc.) is the stale pubkey. Mirror bootstrap's `LoadZeroCopy::load::<Bank>()` returns `AccountOwnedByWrongProgram` for every bank → mirror empty → every intent rejected. Same root cause blocks autodrop: admin ixs built with the stale program id hit a nonexistent address → `UnsupportedProgramId`.

**Diagnosis:**
```bash
# Compare declare_id across trees:
grep declare_id /home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4/programs/mango-v4/src/lib.rs
grep declare_id /home/hetalkenaudekar/mng-v4/programs/mango-v4/src/lib.rs
# Check which binary systemd actually runs:
ls -la /home/hetalkenaudekar/stagin4/mng-v4/target/release/service-mango-execution-engine
readlink -f /home/hetalkenaudekar/stagin4/mng-v4
```

**Fix:** rebuild from the tree whose `declare_id!` matches `PROGRAM_ID` in env, then swap the binary in place:
```bash
sudo systemctl stop stagin4-devnet-relayer.service
sudo cp /home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4/target/release/service-mango-execution-engine \
        /home/hetalkenaudekar/mng-v4/target/release/service-mango-execution-engine
sudo chown hetalkenaudekar:hetalkenaudekar /home/hetalkenaudekar/mng-v4/target/release/service-mango-execution-engine
sudo systemctl start stagin4-devnet-relayer.service
```

**Why swap-in-place instead of re-pointing systemd:** the symlink `stagin4/mng-v4 -> ~/mng-v4` is load-bearing for other processes (harness, bridge, scripts) that read `.devnet/run` and `.devnet/logs` under that path. Keeping the path stable and only replacing the binary is the minimum-blast-radius fix.

**Prevention:** only ever build+deploy from the tree whose `declare_id!` matches the on-chain program. If two trees exist (work-in-progress on a separate branch), keep one designated as "production source" and refuse to let systemd point at anything else. Consider adding a post-build hook that asserts `strings target/release/…` contains the raw bytes of the env `PROGRAM_ID` before the binary is blessed.

## 17. Permanent queue wedge: `drop_head_market: dropped 0 items` forever

**Symptom:** autodrop txs confirm successfully on-chain but `queue_root.next_sequence_to_execute` never advances; on-chain log shows `drop_head_market: dropped 0 items` every call. Reveals silently no-op at the same page-mismatch check. Eventually new commits hit `ExecutionQueueFull` because `next_sequence_to_execute + admission_limit` sits behind the enqueue tail.

**Root cause:** an on-chain invariant break. The slot holding the head's abs page has already been reassigned forward:
```
queue_root.head (next_sequence_to_execute=8398) → abs_page=32, page_slot=0
queue_page0.assigned_abs_page_no = 36   ← already advanced to the tail
```
`drop_head_market` bails at `queue_page.assigned_abs_page_no != head_abs_page` (line ~400 of `execution_queue_v4.rs`), and so does `reveal_execute_market` (line ~598). Neither can advance head. `queue_root.live_count` stays > 0, so the `admin_repair_orphaned_items` fast path (which triggers only when `queue_root.live_count == 0`) is also out of reach.

**How it happens:** `CommitPageV4::prepare_for_write_target` gates reassignment on the *per-page* `live_count` (line ~287), not on `queue_root.live_count`. If items for the head's abs page were cleared out-of-band (e.g., wiped by a mis-sized `prepare_for_write_target` in an earlier commit that raced with head advancement), the per-page count hits 0 while queue_root still thinks those sequences are live. Next commit for the slot wraps it to a new abs page and zeroes the items array — permanently burying the head's data.

**Recovery options** (neither is free):

1. **Fresh queue_root bootstrap** — close/retire the wedged `queue_root` + its pages, create new ones, update `V4_QUEUE_ROOT` / `V4_QUEUE_PAGE0` in `devnet-stack.env`, restart relayer + harness. Keeps the perp market + orderbook + balances intact; just resets the commit-reveal queue. ~15 min of admin ops, no code change.

2. **Program upgrade** — patch `drop_head_market` so that when `queue_page.assigned_abs_page_no` is *ahead of* `head_abs_page`, it treats the head's abs page as irrecoverable and jumps `next_sequence_to_execute` forward to the start of the page currently held. Requires rebuild + `solana program deploy --program-id <keypair>`. Preserves queue state.

**Operationally**, option 1 is the lower-risk path. Option 2 is the correct long-term fix — add it to the next program rev.

**Is this wedge reachable in normal operation?** No — the desync between `queue_page.live_count` and `queue_root.live_count` requires the interpretation of those on-disk bytes to change between the writer and the reader. Every *same-binary* code path that decrements per-page `live_count` also decrements queue-root `live_count` and advances the head in the same transaction; Solana serializes account writes, so there is no in-flight race. The vector is almost exclusively **in-place program upgrades that alter the packed layout of `CommitPageV4` or `PerpMarketCommitRootV4`** — field offsets shift, stale bytes re-interpret as 0 for the field the new binary reads, and the two counters diverge.

**Structural ways to make it impossible** (ordered by invasiveness):

1. **Gate page reassignment on queue_root progress, not per-page count.** In `CommitPageV4::prepare_for_write_target`, require `queue_root.next_sequence_to_execute >= (self.assigned_abs_page_no + 1) * page_size as u64` before allowing `self.assigned_abs_page_no` to change. This ties the slot's "free to reuse" state to the global head pointer — even if the per-page counter reads as 0 due to layout drift, the slot won't be overwritten while the head still references it.
2. **Schema versioning.** Add a `version: u8` to both structs; every load asserts `version == EXPECTED`; every upgrade that changes layout bumps the version and ships a one-shot migration ix. This contains the blast radius of layout drift to a known, operator-driven event.
3. **Append-only struct discipline.** Never move an existing field. Consume bytes from `_padding` / `reserved` when adding new state. If reserved space is exhausted, version-bump (rule 2) is the escape hatch.
4. **Emergency `admin_force_advance_head(new_head_sequence)` ix.** The eject valve when structural guards fail. Gated on admin key + `paused_execute`, bounded so it cannot rewind.

Rule 1 alone would have prevented this incident. Combined with rule 2 it's defense-in-depth.

**Observed recovery** (2026-04-19): a fresh `queue_root` + 4 pages were bootstrapped at `market_index=1` (same program, same group, same USDC bank, same Pyth SOL oracle), and `V4_QUEUE_ROOT` / `V4_QUEUE_PAGE0` / `V4_PERP_MARKET` / `V4_PERP_BIDS|ASKS|EVENT_QUEUE` / `V4_MARKET_INDEX` in `devnet-stack.env` flipped to the new values. Relayer + harness restarted. Head began advancing within seconds (0 → 202 in ~2 min). The wedged market-0 queue_root is left in place (there is no `close_market_root` handler, and closing pages requires `page.live_count == 0`) but is off the critical path.

**Detection signal**: autodrop log shows "admin drop landed" yet `queue_root state head=` stays at the same sequence across many polls, and `max_seen_sequence` also plateaus because new commits hit `ExecutionQueueFull` silently (skip_preflight buries it). Combined: `max_seen - head` approaches but never exceeds `num_pages × page_size`.

## 18. Drift-rewind reassigns in-flight sequences → 6114 `CommitRevealMismatch`

**Symptom:** ~50 % of `PerpPlaceOrderV2` reveals fail on-chain at the `commit_hash` check (error 6114) while `PerpCancelOrderByClientOrderId` reveals don't. On-chain orderbook never receives any real `perp_place_order` dispatch — bids/asks account data is 123 720 bytes of which only the 8-byte Anchor discriminator is nonzero. Autodrop picks up the dead items; head advances but no orders ever land.

**Root cause — four-step interaction, no program redeploy required:**

1. **Silent commit failures.** Relayer sends commits with `skip_preflight=true, max_retries=0` and never polls `getSignatureStatuses`. On-chain rejections (e.g. `InvalidSequenceNumber`, 6076 `ExecutionQueueFull`, 6106 `AssignedPageMismatch`) are invisible; the local `v4_next_sequence` counter still advanced via the prior `fetch_add`.
2. **Drift-rewind reassigns sequences.** `spawn_drift_resync_worker` rewinds the local counter to `on_chain_next_sequence_to_execute` whenever drift exceeds `V4_DRIFT_MAX_LOOKAHEAD` (default 64). The next `submit_intent` gets a sequence number that already had a commit_hash stored on-chain from a prior submit.
3. **`v4_reveal_store.insert(seq, entry)` overwrites.** `BTreeMap::insert` replaces an existing entry silently. The reveal_store now holds the NEW submit's (payload, dispatch, user_sig), but on-chain's `CommitItemV4[seq].commit_hash` still contains the ORIGINAL submit's commit_hash (new commit tx for the reassigned seq fails with duplicate-sequence on-chain, silently).
4. **6114 fires at reveal.** Reveal worker rebuilds the reveal tx from the (overwritten) stored payload; on-chain recomputes `expected_commit_hash` using `hashv(reveal.payload)` which is the NEW payload hash. `expected != head_item.commit_hash` → `ExecutionQueueV4CommitRevealMismatch`. Tx reverts; head stays on the item. Autodrop eventually clears it.

**Why place orders fail more than cancels:** coincidental timing — cancels tend to be sent for seqs that haven't been drift-rewound-reassigned (fewer bots cancel simultaneously), and place orders are the bulk of submit volume so they hit the reassigned seqs more often. The underlying mechanism is payload-agnostic.

**Diagnostic signature:** grep the relayer's `v4_commit_debug` log for a single sequence — if you see the same `seq=N` with two or more distinct `commit_hash_hex=` values in the same session, you're hitting this bug. In one sample session 325 sequences (3.3 %) had multiple distinct commit_hashes logged.

**Fix** (shipped in `v4_pipeline::spawn_drift_resync_worker`):
```rust
let max_stored_seq: u64 = {
    let store = reveal_store.lock();
    store.keys().next_back().copied().unwrap_or(0)
};
let safe_rewind_target = on_chain_next.max(max_stored_seq.saturating_add(1));
if local > safe_rewind_target {
    next_seq.store(safe_rewind_target, Ordering::Release);
}
```
Plus: GC `reveal_store` entries below `queue_root.next_sequence_to_execute` each tick, so the floor doesn't keep the rewind target artificially high.

After deploy, the per-seq duplicate-commit_hash pattern stopped and the post-fix test run showed `OK=51, ERR=0` on the next 51 `perp_market` txs (vs. the pre-fix 50/50 split).

**Lesson — structural, not redeploy-triggered:** the whole class of wedges documented in §17 was attributed to "in-place program redeploys that alter struct layout." This §18 failure shows the structural fixes listed there (gate page reassignment on `queue_root` progress; schema versioning) are also needed to defend against the *non-redeploy* version of the bug. The drift rewind + skip_preflight + in-memory reveal_store trio can produce the same wedge state without any schema drift.

**Three complementary relayer-side fixes** (first one shipped; others noted for follow-up):

1. ✅ Drift-rewind respects `max_stored_seq + 1` (this fix).
2. 📋 Poll `getSignatureStatuses` after commit send; remove reveal_store entry on failure. Catches #1 in the root-cause chain directly.
3. 📋 Persist reveal_store to disk (sqlite / JSON snapshot) so relayer restarts don't lose mid-flight commit material — prevents a separate mechanism that causes commit_hash↔payload divergence across process boundaries.

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
