# Mainnet Readiness Roadmap — 16 April 2026

**Protocol:** Fermi DEX — FIFO Perps on Solana  
**Branch:** `vc6`  
**Devnet program:** `9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF`  
**Gate standard:** all P0 items closed before mainnet deploy; all P1 items closed before unrestricted public access.

---

## Architecture Snapshot

The system is a three-tier stack:

1. **On-chain program** — Anchor/Rust. Mango v4 core (banks, perp markets, health engine, liquidation, flash loans) plus the Fermi Execution Queue (v2 sub-queue layout, 16 markets × 64 CTM slots + 128 liquidity slots in a single 425 KB account). ED25519-verified CTM envelopes, payload-hash-locked dispatch, accounts-hash-locked lane matching.

2. **Off-chain execution engine** — Rust binary (`bin/service-mango-execution-engine`). Dual roles: (a) gRPC relayer/sequencer that accepts `SubmitIntentRequest`, assigns sequences, co-signs CTM envelopes, submits enqueue transactions; (b) executor/cranker that polls queue state and submits `execute_multi` transactions.

3. **State harness + fanout** — Rust harness (`rust-harness/`) maintains optimistic and confirmed views of queue + orderbook state updated in <10 ms via event forwarding. `bin/service-fanout/` fans events out to up to ~2000 SSE/WS clients with JWT/API-key auth and per-user connection limits.

Devnet E2E correctness is confirmed. Onchain matching, PnL, and settlement work. Relayer throughput cleared 89 ops/s (place + cancel intents) on localnet; queue drain is the current bottleneck.

---

## 1. Formal Verification — 30/31 Done

### Status

30 of 31 Lean 4 properties verified with zero `sorry` markers. Build is clean via `lake build`. Coverage spans:

| Category | Result |
|---|---|
| Access Control (AC-1..3) | 3/3 ✓ |
| Vault Conservation (VC-1..4) | 4/4 ✓ |
| Health Invariants (HI-2..5) | 3/4 — HI-1 open |
| Liquidation Phase Ordering (LP-1..3) | 3/3 ✓ |
| Index Conservation (IC-1..3) | 3/3 ✓ |
| Perp Funding Symmetry (PF-1..3) | 3/3 ✓ |
| Fee Accounting (FA-1..3) | 3/3 ✓ |
| Arithmetic Safety (AS-1..4) | 4/4 ✓ |
| Loss Socialization (LS-1..3) | 3/3 ✓ |

### Open: HI-1 (Health Ordering)

**Statement:** For all accounts at any time, `init_health ≤ liq_end_health ≤ maint_health`.

**Why it matters:** This is the foundation of the liquidation safety argument. If it fails, an account could be liquidated when `init_health ≥ 0`, or the liq-end condition could clear prematurely.

**Why it is hard:** The proof requires an inductive argument over all token + perp positions showing that the weight hierarchy (`init_asset_weight ≤ maint_asset_weight`, `maint_liab_weight ≤ init_liab_weight`) plus conservative price selection for init health (min/max of oracle vs stable price) produces a contribution ordering that is preserved under summation. Mathlib lemmas for ordered weighted sums over `Finset` are required.

**Path to closure:** The proof proposal is in `formal_verification/hi-1-proof-proposal.md`. Estimated effort: 2–4 days of focused Lean 4 work. This is a pure mathematical task; the Rust code is correct — the gap is the machine-checked certificate.

### FV Scope Gap: Execution Queue Not Covered

The 30 verified properties cover `Bank`, `MangoAccount`, `Group`, `PerpMarket`, and their health/liquidation/funding/fee transitions. The Execution Queue module (state machine, CTM dispatch, lane verification, gap handling, retry logic) has **no Lean 4 coverage**. It is covered by the internal 43-item code review (`internal_audit_mar25.md`) and the `audit-scope.md` differential scope, but not formal proofs.

Given the complexity of the queue state machine, formal verification of even the key invariants (head monotonicity, counter consistency, accounts-hash binding) would add confidence. These are practical to prove; the unit tests already express them as Rust assertions.

### Remaining Steps

- [ ] **P0** — Close HI-1. Complete the Lean 4 proof for health ordering using the proposal in `hi-1-proof-proposal.md`. Verify `lake build` with zero sorry.
- [ ] **P1** — FV scope statement for execution queue: explicitly document which queue invariants are covered by unit tests only and which, if any, will receive Lean proofs. Publish this as part of the external audit package.

---

## 2. API Gating and Rate Limits — Integrate

### Status

**Write path (relayer):** Per-user token-bucket rate limiter is implemented in the Rust execution engine (`ingress_rate_limit_enabled`, `ingress_rate_limit_burst`, `ingress_rate_limit_per_sec`). Backpressure controls exist: `CTM_RELAYER_MAX_INFLIGHT`, `CTM_RELAYER_MAX_QUEUED`, queue-watermark-based fast-fail rejection, and per-user `RESOURCE_EXHAUSTED` responses.

**Read path (fanout):** `bin/service-fanout` implements Phase 1 + 2: JWT (HS256) and static API key auth, per-user connection counter via `DashMap<user_id, AtomicU32>`, global Semaphore connection cap. Ingest is protected with `X-Ingest-Secret`.

**Harness read endpoints:** No authentication on `/state/*` endpoints in the harness HTTP server. These are currently internal, but any public exposure requires auth.

### Gaps

1. **TLS on gRPC relayer.** The gRPC server still uses `createInsecure()` (audit H-10). All relayer traffic should be TLS-terminated at minimum at the edge proxy; the gRPC server itself should either use `createSsl()` or be behind a TLS-terminating proxy with mTLS for internal service-to-service calls.

2. **Tier-based token buckets on fanout.** `Tier` enum exists (`Free`, `Pro`, `Enterprise`) but per-tier token bucket enforcement is not yet wired. The planned design (token bucket per `(user_id, market)`) from `fanout.md` is not yet in `sse.rs` or `snapshot.rs`.

3. **Gateway-level enforcement.** The current `ops/nginx/perps-dex-devnet.conf` has no auth or rate limiting. All ingress limits must be enforced at the gateway, not only inside individual services.

4. **Quota and billing tie-in.** Per `mainnet_roadmap_1.md`: limits should be tied to signed owner identity plus gas-account balance, not IP only, to resist sybil bypass.

5. **Body size and expensive-query caps.** No body-size limits or explicit caps on expensive harness queries (full snapshot, multi-market queries).

### Specific Tests Required

- Submit 1,001 requests/sec from a single API key; verify the 1,001st returns `RESOURCE_EXHAUSTED` with latency < 10 ms.
- Open 11 SSE connections from one user; verify the 11th returns `429 Too Many Requests`.
- Open 2,001 global connections; verify the 2,001st returns `503 Service Unavailable`.
- Send a forged JWT (wrong HMAC key); verify `401 Unauthorized`, no state leak.
- Send a valid expired JWT; verify `401 Unauthorized`.
- Attempt to POST `/ingest` without `X-Ingest-Secret`; verify `403`.
- Verify fanout metrics (`active_subscribers`, `auth_failures`, `connections_rejected`) increment correctly under each of the above.

### Remaining Steps

- [ ] **P0** — Wire per-tier token bucket into fanout SSE and snapshot handlers. Free tier: 20 RPS with burst 50. Pro/Enterprise: configurable.
- [ ] **P0** — TLS-terminate gRPC relayer. At minimum via nginx/envoy proxy; ideally with `createSsl()` server credentials.
- [ ] **P0** — Add auth and rate limiting to the nginx edge config. Restrict `Access-Control-Allow-Origin` to specific frontend origin.
- [ ] **P0** — Protect harness `/state/*` read endpoints with the same API key / JWT middleware as fanout, or keep them strictly internal behind a private network boundary.
- [ ] **P1** — Body size limits on all POST endpoints. Cap expensive fanout queries.
- [ ] **P1** — Per-key quota store (daily/monthly request counters) backed by Redis or Postgres.

---

## 3. Event Sink and Redis — WIP

### Status

`bin/service-fanout` Phase 1 and 2 are implemented (~1360 LOC):
- `POST /ingest` → `broadcast::channel(4096)` per market → SSE to N subscribers.
- `GET /stream/:market` with lag detection + resync signal.
- `GET /snapshot/:market` returning recent-event ring.
- JWT + API key auth, connection limits, Prometheus metrics.

Redis Pub/Sub scale-out (Phase 4 from `fanout.md`) is **not yet implemented**. The single-instance design supports ~2000 concurrent SSE connections.

### Gaps

1. **Redis pub/sub not wired.** For public mainnet with >2000 concurrent readers, horizontal fanout is required. The `FANOUT_REDIS_URL` flag and `redis.rs` module are planned but absent from the current source.

2. **User snapshot endpoint missing.** `GET /snapshot/user/{owner}` (async fetch from harness, `moka::Cache` with TTL) is specified in Phase 3 but not implemented.

3. **WebSocket variant not implemented.** `GET /ws/:market` is planned but absent.

4. **Snapshot gap on reconnect.** The `?from=N` query param to skip already-seen events is described in `fanout.md` but not present in the current `sse.rs` handler. Clients that reconnect after a lag may replay stale events or have a gap.

5. **No durable ingest backlog.** If the fanout service restarts, in-flight events from the execution engine that arrived during the restart window are lost. Clients that reconnect after a fanout restart see a gap. A Redis stream (XADD/XREAD) would provide durable replay.

### Specific Tests Required

- Subscribe to market 0, disconnect at event N, reconnect with `?from=N`; verify next event received is N+1.
- Flood ingest with 5000 events/sec; verify `Lagged` resync signal fires before subscriber OOM.
- Start two fanout instances with `FANOUT_REDIS_URL` set; verify an event POSTed to instance-0 appears on an SSE stream connected to instance-1 within 5 ms.
- Kill fanout instance mid-stream; verify SSE client receives a `resync` event on reconnect (not a silent drop).

### Remaining Steps

- [ ] **P0** — Implement `?from=N` sequence-filtered reconnect in `sse.rs`. This is required to prevent data gaps on reconnect at any subscriber count.
- [ ] **P0** — Implement `GET /snapshot/user/{owner}` with `moka::Cache` TTL (500 ms suggested). Required for client UX.
- [ ] **P1** — Implement Redis Pub/Sub scale-out (`redis.rs`, `FANOUT_REDIS_URL` env flag). Required before public launch with >2000 users or multi-region deployment.
- [ ] **P1** — Implement `GET /ws/:market` WebSocket variant.
- [ ] **P1** — Add event-backlog durability (Redis streams XADD) so a fanout restart does not produce client-visible gaps.

---

## 4. Fee Model for Submit Spam — Integrate

### Status

Current anti-spam controls are **rate-based only** (token buckets per user, queue watermarks, backpressure `RESOURCE_EXHAUSTED`). There is no economic fee model — i.e., no per-submission deposit, SOL burn, or token stake required to enqueue. Transaction fees (~5000 lamports per enqueue tx) are the only economic friction, which is insufficient to deter sustained spam at scale.

The Rust execution engine does have the plumbing (`ingress_rate_limit_*` env vars), and the queue has `queue_soft_limit` / `queue_hard_limit` backpressure. In practice, the relayer enforces limits before the chain sees spam.

### Gaps

1. **No economic anti-spam.** A well-resourced attacker with many API keys can cycle through free-tier limits. The missing layer is a deposit-backed or stake-weighted admission model.

2. **Liquidity queue has no signer (audit H-1).** Anyone can fill the 128-slot liquidity ring with zero-cost garbage items. Fix: require mango account owner as signer on `enqueue_liquidity`.

3. **Sequence gaps from abandoned submits.** When the relayer submits an enqueue tx that fails or is dropped, the sequence number is consumed. Under high-spam conditions this can create large gap spans that stall the executor (`gap_wait_slots` × 400ms per gap). The gap-skip cost is borne by all users of that market queue. A per-submission deposit that is refunded only on successful execution would make gap creation expensive.

4. **No on-chain circuit breaker for queue fill rate.** If the queue fills to capacity, new intents are rejected with `ExecutionQueueFull` (6076). There is no backpressure that increases cost as the queue fills, only hard rejection.

### Proposed Model

**Relayer-side (immediate):** A tiered admission fee structure, charged off-chain via the API key billing system:
- Free tier: 1,000 intents/day, 20 RPS burst.
- Market-maker tier (registered, API key + program-verified address): 100,000 intents/day, 100 RPS.
- Protocol tier (audited program): unlimited with on-chain stake verification.

**On-chain (medium-term):** A small SOL deposit (`ENQUEUE_DEPOSIT_LAMPORTS`, suggested 10,000 lamports ≈ $0.001) credited at enqueue and refunded at successful execution or explicit cancel. Items that expire or are skipped forfeit the deposit to the protocol treasury. This makes deliberate gap attacks expensive: 1000 gaps × 10,000 lamports = 0.01 SOL per intentional stall.

### Specific Tests Required

- Submit 1,001 intents in < 60 seconds from a free-tier key; verify the 1,001st is rejected with `RESOURCE_EXHAUSTED`, not silently dropped.
- Submit 10 unsigned liquidity enqueue instructions; verify all reject with `MissingRequiredSignature` after the signer fix is deployed.
- Simulate 500 enqueue-tx drops (sequence gaps) on a test market; verify executor stalls for at most `gap_wait_slots × 400ms × gaps` and resumes cleanly.
- Verify that after the deposit model, a queue fill-to-capacity attack costs > 0.1 SOL/minute to sustain.

### Remaining Steps

- [ ] **P0** — Fix `enqueue_liquidity` to require mango account owner as signer (audit H-1). Low risk, low effort.
- [ ] **P0** — Document and enforce tiered rate limits at the gateway, tied to API key identity (not IP only). Free, market-maker, and protocol tiers.
- [ ] **P1** — Implement SOL deposit-at-enqueue model for CTM items. Deposit stored in a per-item lamport vault or a per-owner escrow account. Refund on execute, forfeit on expiry/skip.
- [ ] **P1** — Add on-chain circuit breaker: when `ctm_count > soft_limit`, require `min_execute_slot_offset ≥ CONGESTION_BACKOFF_SLOTS` to self-throttle submitters.

---

## 5. Paginated Execution Queue — WIP

### Status

The full v3 design is specified in `exec-queue-amq.md`. **No code has been written yet.** The current on-chain queue is v2: 16 sub-queues × 64 CTM slots = 1024 total CTM items, 128 liquidity slots, all inline in a single 425 KB account. A single hot market can fill its 64-slot sub-queue at ~90 relayer ops/s in approximately 0.7 seconds.

The stress test log (entry 32 in `qa-fixes.md`) confirms this: at 89 ops/s, the queue fills to capacity (count=1000) within ~12 seconds.

### Design Summary (v3)

- One paged FIFO queue root per perp market (`MarketQueueRootV2` PDA: `["perp-queue-root", group, market_index, shard_id=0]`)
- One separate liquidity queue root (`LiquidityQueueRootV2`)
- Items stored in page PDAs (`QueuePageV2`): `page_size=128`, `num_pages=16`, capacity=2048 per market
- Self-describing items with `mango_account_hint`, `account_recipe`, `expires_at_slot`, `failure_code`
- Queue-side deterministic pre-checks for expiry, malformed payload, market status
- Lane reconstruction from queued metadata + on-chain mirrors (eliminates event-log dependency for recovery)
- Phase 2: `MakerReplaceBandV1` maker-band compaction

### Why This Matters for Mainnet

At 64 items/market, the queue under normal market conditions is a liability. A single active market with 5 quoter bots at 100ms cadence fills the queue in ~3 seconds. The v3 design raises the hard cap to 2048 items/market, giving ~23 seconds of runway at 89 ops/s — sufficient for typical burst handling.

More importantly, v3 eliminates the cross-market queue contamination risk: in v2, a stalled head on one market's sub-queue does not block other markets, but they all share the same account. In v3, stalling one market's queue root has zero effect on other markets.

### Specific Tests Required

- Initialize two market queue roots (market 0 and market 1); fill market 0 to capacity; verify market 1 continues to accept and execute intents.
- Enqueue 129 items on market 0 (crosses page boundary); verify item 129 lands in page slot 1 and executes correctly after page slot 0 drains.
- Kill the relayer mid-run; restart; verify lanes are reconstructed from `mango_account_hint` + `account_recipe` without event-log replay.
- Enqueue an item with `expires_at_slot = current_slot - 1`; verify it is deterministically failed at execute time (not retried).
- Verify independent sequence namespaces: market 0 at sequence 100 and market 1 at sequence 5 are independent; no cross-market sequence math.

### Remaining Steps

- [ ] **P0 (blocking for sustained mainnet load)** — Implement v3 paginated queue Phase 1:
  1. `QueueAuthorityState`, `MarketQueueRootV2`, `LiquidityQueueRootV2`, `QueuePageV2` account structs.
  2. `execution_queue_init_authority_state`, `init_market_root`, `init_liquidity_root`, `init_page` instructions.
  3. `execution_queue_enqueue_market`, `execution_queue_enqueue_liquidity` (domain-aware, preserving v2 user-intent-v2 hash contract).
  4. `execution_queue_execute_market`, `execution_queue_execute_multi_market`, `execution_queue_execute_liquidity`.
  5. Deterministic pre-checks: expiry, malformed payload, invalid market status.
  6. Migration runbook: pause v2, drain, create v3 roots, switch relayer/executor.
- [ ] **P0** — Align payload enum across on-chain Rust, executor Rust, and TS client (noted as drifted in `exec-queue-amq.md` Step 0).
- [ ] **P1** — Implement Phase 2: `MakerReplaceBandV1` and bounded maker-band compaction.
- [ ] **P1** — Update all tooling (cranker, harness, inspect scripts) to decode and display v3 queue layout.

---

## 6. MEV Risk Modelling — WIP

### Status

The primary MEV vectors are identified and documented in `audit_mar20.md`. The VDF-based temporal sequencer (separate repo) addresses ordering MEV: the cranker cannot reorder or front-run because the queue sequence is cryptographically committed by the VDF sequencer before the cranker ever sees it. The `find_matching_ctm_item` scan-ahead is confirmed **unused** in the current instruction code (`internal_audit_mar25.md`, Item 5) — execution is strictly head-only.

### Open Vectors

**C-1/C-2 — execute_multi lane hash not verified against actual accounts.**  
The `execute_multi` instruction accepts `lane_hashes` as instruction data and never computes the hash from the actual accounts passed in the lane slice. A malicious or compromised executor can provide a `lane_hash` matching a queued item but supply different accounts, potentially dispatching orders against the wrong mango account.

Current mitigation: the executor is the same process as the relayer and operates under the same trust boundary. But this assumes the executor binary is uncompromised. For mainnet, the executor should be treated as a potentially adversarial process.

**Fix (from audit):** In `execute_multi`, compute `let computed_hash = compute_accounts_hash(lane_slices[lane_idx])` and match against `candidate.accounts_hash`. Cost: ~8–12K CU for a 4-lane batch — negligible.

**H-7 — CLMM oracle same-transaction sandwich.**  
Any token using an Orca/Raydium CLMM oracle is vulnerable to a same-transaction price manipulation attack: swap on the CLMM pool in instruction 1, call `token_withdraw` or `perp_place_order` in instruction 2 (health check reads inflated price), swap back in instruction 3. The audit confirms this is viable against all health-consuming code paths. The mitigation (dual-oracle band check) is specified but **implementation status is unconfirmed** in the current branch.

**C-5/H-4 — Bankruptcy accounting edge cases.**  
When `open_interest == 0`, `socialize_loss()` discards the loss silently. When insurance is exhausted, the deposit index can go negative. Both are acknowledged in `AUDIT comment` annotations in the code. These are low-probability but catastrophic when triggered.

**M-1 — expires_at_slot not checked at execute time.**  
Queued items that miss their execution window (relayer dropped, queue full) are not expired at execute time. They can be executed arbitrarily later at a stale price. Fix: add `require!(item.expires_at_slot == 0 || clock.slot <= item.expires_at_slot)` in the execute path (~200 CU).

**L-7 — User intent signature does not bind sequence/timing.**  
The `user_intent_message_v2` does not include `sequence`, `min_execute_slot`, or `expires_at_slot`. The same user signature can theoretically be reused by the relayer for multiple envelopes with the same `kind/payload_hash/accounts_hash`. The relayer controls scheduling, and this is by design, but it creates a trust assumption that the relayer will not replay signatures. For mainnet, this should be explicit in the trust model documentation.

### Direct-Submit Fallback (C-4)

The design for a user-initiated direct enqueue with 10-slot delay is specified in the audit (C-4). This provides liveness if the relayer goes down. **Implementation status is unconfirmed.** This is a critical safety valve for mainnet — without it, relayer downtime means zero order flow.

### Specific Tests Required

- Submit `execute_multi` with `lane_hashes` matching a queued item but different actual accounts in the lane; verify the transaction fails with an accounts-hash mismatch error (after C-1/C-2 fix).
- Enqueue an item with `expires_at_slot = current_slot + 2`; wait 3 slots; attempt execute; verify item is marked `Failed` with expiry reason and head advances (after M-1 fix).
- Submit a user intent signature twice with different sequences assigned by the relayer; verify on-chain behavior — intended to confirm whether both or only one can execute.
- If CLMM oracle is used: sandwich test — swap on pool in same tx as `token_withdraw`; verify dual-oracle band check rejects inflated price.
- With VDF sequencer down and relayer down: use direct-enqueue fallback; verify order lands after 10-slot delay.

### Remaining Steps

- [ ] **P0 — Fix C-1/C-2:** In `execute_multi`, compute lane hashes from actual accounts instead of trusting instruction data. This is a critical security fix; ~3 hours of Rust work.
- [ ] **P0 — Fix C-3:** Add `group.load()?.is_testing()` constraint to `unsafe_deposit`. Single line change.
- [ ] **P0 — Fix C-5/H-4:** Cap socialized loss at `indexed_total_deposits * deposit_index`; assert `new_deposit_index >= 0`; accumulate unsocialized loss in dedicated field.
- [ ] **P0 — Fix M-1:** Check `expires_at_slot` at execute time. Single `require!` line in execute path.
- [ ] **P0 — Implement C-4:** Direct-submit fallback with 10-slot delay (`min_execute_slot = current_slot + DIRECT_SUBMIT_DELAY_SLOTS`). No CTM co-signature required. This is the user escape hatch.
- [ ] **P0 — Verify/implement H-7 dual-oracle band check:** Confirm whether the dual-oracle check is already in the current branch or still pending. If pending, implement `validated_clmm_price()` and wire it into all oracle-consuming code paths.
- [ ] **P1 — Fix H-3 (insurance rounding):** Floor insurance transfer amounts.
- [ ] **P1 — Fix H-6 (admin timelock):** 30-minute pending-admin activation mechanism for critical group parameter changes.
- [ ] **P1 — Fix M-2 (CTM signer rotation delay):** Enforce `activate_at_slot >= current_slot + MIN_ROTATION_DELAY`.

---

## 7. Fuzzing and Stress Testing — WIP

### Status

The QA log (`qa-fixes.md`) documents 33 iterations of stress testing, fixing quoter throughput, relayer stability, cranker lane matching, backpressure, and serialization bottlenecks. The system now achieves ~89 accepted ops/s through the relayer on a local single-threaded validator. Key findings:

- Offchain submit path is no longer the bottleneck.
- Queue drain (on-chain execute throughput) is the bottleneck: ~13 execute txs/sec × ~7 items/tx ≈ 91 items/sec drain rate on localnet.
- `100+ avg place TPS` target is not yet met; the single-threaded validator caps localnet results. Devnet with parallel tx processing should unlock significantly higher throughput.
- The `qa-fixes.md` stress testing is script-based, not an automated fuzz harness.

### Missing: Fuzzing Infrastructure

There is no dedicated fuzzing setup. No Trident (Anchor fuzz framework), no `cargo-fuzz` targets, no property-based test beyond scattered `proptest` in the qedgen examples. The on-chain execution queue has 14+ unit tests in `state/execution_queue.rs` covering ring operations, but these are deterministic, not fuzz-driven.

### Required Fuzzing Targets

**On-chain (Trident or cargo-fuzz):**
1. `fuzz_execution_queue_enqueue_execute` — random sequences, random statuses, random gap patterns. Assert: counter consistency (`total == ctm + liq`), head monotonicity, no sequence reuse.
2. `fuzz_health_cache_construction` — random token/perp position sets, random oracle prices. Assert: `init ≤ liq_end ≤ maint` at all times (this would also cover HI-1 empirically even before the formal proof closes).
3. `fuzz_liquidation_phases` — random account states including negative health, random phase entry. Assert: phase ordering invariants.
4. `fuzz_interest_accrual` — random time deltas, random utilization. Assert: index monotonicity, no negative indices.

**Off-chain (proptest or quickcheck):**
5. `fuzz_queue_layout_decode` — random bytes as queue account data. Assert: no panics, error on invalid layout.
6. `fuzz_relay_intent_replay` — random event sequences (accepted, processed, skipped, failed, out-of-order). Assert: harness divergence count stays consistent, no duplicate sequence counting.

### Required Stress Scenarios (not yet run)

| Scenario | Target | Status |
|---|---|---|
| 100+ avg place TPS on devnet | Confirmed via devnet parallel validators | Not yet run |
| 48-hour continuous devnet run | Queue stability, no leaked SOL, clean restart | Not yet run |
| Simulate 50% RPC packet loss | Executor recovery, gap handling | Not yet run |
| Liquidity queue fill + drain | 128-slot full, all items execute | Not run |
| Multi-market parallel load (8 markets × 10 quoters) | Per-market isolation, no cross-market stall | Not run |
| Queue stall + manual admin recovery | Admin gap-drop, head resume | Not yet drilled |
| Harness restart mid-run | Divergence recovery, snapshot reload | Not drilled |

### Remaining Steps

- [ ] **P0** — Run 100+ avg place TPS test on devnet. Use 10 persistent quoter bots, SWQoS RPC (Helius or Triton), priority fees. Target: ≥100 avg place TPS sustained over 5 minutes with queue draining within 2× enqueue rate. Document result.
- [ ] **P0** — Run 24-hour devnet soak. Monitor: queue head age, gap count, execute success rate, relayer restart survival, harness divergence events, SOL burn rate.
- [ ] **P1** — Set up Trident fuzz harness for `fuzz_execution_queue_enqueue_execute` and `fuzz_health_cache_construction`. Run for minimum 24 hours. Fix any panics or assertion failures.
- [ ] **P1** — proptest suite for queue layout decode, relay intent replay, and harness divergence accounting.
- [ ] **P1** — Multi-market parallel load test (8 markets). Verify per-market isolation.
- [ ] **P1** — Admin recovery drill: intentionally stall a queue head via gap; execute admin drop procedure; verify queue resumes.

---

## 8. Internal Audit — Todo

### Status

**Internal (done):** `audit_mar20.md` is a comprehensive 46-finding internal audit (5 CRITICAL, 12 HIGH, 16 MEDIUM, 13 LOW). `internal_audit_mar25.md` is a 43-item verification of the execution queue module specifically, with most items passing. `audit-scope.md` prepares a differential audit scope narrowed to the 3 new execution queue files (state, instructions, accounts_ix).

**External audit: NOT yet engaged.** `audit-scope.md` is ready to hand to an auditor. The scope is well-defined: the 3 new execution queue files plus thin dispatch wrappers on 5 existing instructions. The core Mango v4 systems (unchanged from last OtterSec-audited state) are out of scope.

### Internal Audit Open Items (not yet fixed per code review)

The following items from `audit_mar20.md` have specified fixes but unconfirmed implementation in the current `vc6` branch:

| ID | Severity | Finding | Fix Status |
|---|---|---|---|
| C-1 | CRITICAL | execute_multi trusts caller lane hashes | Unconfirmed — must verify |
| C-2 | CRITICAL | Owner not validated in perp_place_order dispatch | Unconfirmed — must verify |
| C-3 | CRITICAL | unsafe_deposit lacks is_testing() guard | Unconfirmed — must verify |
| C-4 | CRITICAL | No direct-submit fallback | Not implemented |
| C-5 | CRITICAL | Socialized loss discarded when OI=0 | Unconfirmed — must verify |
| H-1 | HIGH | enqueue_liquidity has no signer | Unconfirmed — must verify |
| H-3 | HIGH | Insurance drainage via rounding | Unconfirmed |
| H-4 | HIGH | Token bankruptcy can produce negative deposit index | Unconfirmed |
| H-6 | HIGH | No admin timelock | Not implemented |
| H-7 | HIGH | CLMM oracle sandwich | Implementation unconfirmed |
| H-9 | HIGH | Harness PostOnly order matching bug | Unconfirmed |
| H-10 | HIGH | gRPC/HTTP no TLS | Not implemented |
| H-11 | HIGH | Stale oracle in PnL settlement | Unconfirmed |
| M-1 | MEDIUM | expires_at_slot not checked at execute | Unconfirmed |
| M-4 | MEDIUM | Sequence state on /tmp | Unconfirmed |
| M-5 | MEDIUM | Unbounded event log growth | Partially addressed |

### Path to External Audit

The recommended sequence:
1. Confirm or fix all CRITICAL items (C-1..C-5) in the current branch. These are the only items that create direct fund risk.
2. Submit `audit-scope.md` and the fixed code to an external auditor (OtterSec preferred given 9 prior Mango v4 audits).
3. Run the audit in parallel with v3 paginated queue development. The execution queue differential scope is narrow (~3 files, ~3000 LOC net new) — a focused engagement is appropriate.
4. Incorporate audit feedback before mainnet deploy.

### Remaining Steps

- [ ] **P0** — Audit status pass: go through each C and H finding in `audit_mar20.md`, verify fix is in `vc6`, add a comment with commit reference or document it as open.
- [ ] **P0** — Fix all confirmed-open CRITICAL items (C-1, C-2, C-3, C-4, C-5).
- [ ] **P0** — Engage external auditor. Provide `audit-scope.md`, current branch, and this roadmap as context. Target a 2–3 week engagement window.
- [ ] **P1** — Fix remaining HIGH items (H-1, H-3, H-4, H-6, H-7, H-9, H-10, H-11).
- [ ] **P1** — Address all MEDIUM items before public launch.

---

## 9. Additional Concerns

### 9.1 Governance and Key Management

Not addressed in any in-progress item. The current deployment has a single admin key controlling program upgrades, group parameters, insurance fund, and CTM signer rotation. A single compromised key gives total fund control (audit H-6). For mainnet:

- Program upgrade authority → 3/5 multisig.
- Group admin, security admin → separate 2/3 multisigs with distinct keyholders.
- 24–48h timelock on upgrades and critical parameter changes; emergency guardian (2/3) allowed only to pause ingress or force reduce-only.
- Hot executor/relayer keys: KMS-backed (AWS KMS, GCP HSM, or hardware wallet) — no exportable private key files in production.
- CTM signer rotation: require dual approval even though staged rotation exists on-chain.

- [ ] **P0** — Deploy program upgrade authority to multisig before mainnet.
- [ ] **P0** — Deploy group admin and security admin to separate multisigs.
- [ ] **P0** — Move hot relayer/executor key into KMS-backed signer.
- [ ] **P1** — Implement 30-minute admin timelock on-chain (H-6 fix).

### 9.2 Sequencer Durability and HA

Current relayer persists sequence state to local files. Restart after crash requires careful sequence reconciliation to avoid gaps or duplicates. `mainnet_roadmap_1.md` calls for HA Postgres as the control-plane source of truth.

- [ ] **P0** — Replace local sequence files with HA Postgres (sequence cursors, accepted intents, idempotency keys, enqueue tx state, execute tx state, failover lease).
- [ ] **P0** — Hot standby relayer with advisory lock / leader lease. Exactly one active sequencer at a time; standby stays hot but fenced.
- [ ] **P0** — Use ≥2 independent sender RPCs (Helius/Triton SWQoS) + ≥2 read/WS RPCs with health scoring and automatic failover.

### 9.3 SWQoS RPC (Critical for TPS)

Devnet TPS results showed 429 rate limiting dominated before on-chain capacity was reached. The `audit_mar20.md` M-3 analysis is explicit: SWQoS RPCs (Helius, Triton, bloXroute) provide 3× improvement in tx inclusion rate and are **mandatory for production**. p85 of transactions land within 4 slots with SWQoS vs. 10–25 slots with standard RPC.

- [ ] **P0** — Provision SWQoS RPC endpoint (Helius or Triton staked connection). Use for all enqueue and execute submissions.
- [ ] **P0** — Use multi-endpoint submission: 2–3 providers, converting p90 → ~p99 landing rate.
- [ ] **P0** — Set priority fees: 10K–50K µlamports/CU on all critical transactions.

### 9.4 Perp Event Consumer Backpressure (Audit H-5)

The perp event queue consumer runs every 2s and consumes max 8 events/tx. Under burst trading, unconsumed fill events block health checks with stale data and can prevent new order placement. The audit recommends increasing the limit to 16 (stays within CU budget) and increasing cranker frequency.

- [ ] **P1** — Increase `perp_consume_events` limit from 8 to 16.
- [ ] **P1** — Increase executor's `executor_perp_consume_interval_ms` from 2000ms to 500ms in production.

### 9.5 Lifecycle Observability

Current observability: queue metrics (count, head, next sequence), harness divergence events, relayer accept/error counters. Missing: per-intent lifecycle states from `received` through `confirmed_view_applied`, indexed by `intent_id`, `sequence`, `client_order_id`, `enqueue_tx_signature`, `execute_tx_signature`.

Without per-intent tracing, debugging a "my order didn't fill" support request requires correlating multiple log sources. This is operationally unsustainable at scale.

- [ ] **P0** — Add authoritative intent-lifecycle store: database record per intent with full state transitions. Support lookup by any of the above keys.
- [ ] **P0** — Extend harness/bridge APIs with per-intent status endpoint.
- [ ] **P0** — Add synthetic canaries: internal bot that submits, cancels, and closes on a tiny account every 60s; alert if end-to-end latency > 5s or any step fails.
- [ ] **P1** — Standard correlation ID from gateway through relayer, executor, and harness in all logs, metrics, and traces.

### 9.6 PayloadEnum Drift

`exec-queue-amq.md` Step 0 documents that the payload enum variants have drifted across on-chain Rust, executor Rust, and TS client. This causes silent mismatches where a payload is accepted by one layer but misinterpreted by another. This must be fixed before any v3 queue migration — a migration that encounters mismatched enum tags will produce hard-to-diagnose failures.

- [ ] **P0** — Audit and align payload enum variants across all three layers. Add a compile-time or CI check that the canonical variant list is consistent.

### 9.7 Token Conditional Swaps

TCS is implemented but contains multiple TODOs and has received less review than perp and token paths. `audit_mar20.md` M-9 notes f64 precision loss in TCS pricing for extreme price ratios.

- [ ] **P1** — Do not enable TCS on mainnet until a dedicated TCS audit pass is complete.
- [ ] **P1** — Fix M-9 (TCS premium uses f64 for extreme price ratios).

---

## Priority Summary

### P0 — Mainnet Blockers (must close before deploy)

| # | Area | Item |
|---|---|---|
| 1 | Formal Verification | Close HI-1 (health ordering proof) |
| 2 | MEV / Audit | Fix C-1/C-2: compute lane hashes from actual accounts in execute_multi |
| 3 | MEV / Audit | Fix C-3: add is_testing() guard to unsafe_deposit |
| 4 | MEV / Audit | Fix C-4: implement direct-submit fallback with 10-slot delay |
| 5 | MEV / Audit | Fix C-5/H-4: cap socialized loss, assert deposit index ≥ 0 |
| 6 | MEV / Audit | Fix M-1: check expires_at_slot at execute time |
| 7 | MEV / Audit | Confirm/implement H-7 dual-oracle band check for CLMM tokens |
| 8 | Fee / Spam | Fix H-1: require owner signer on enqueue_liquidity |
| 9 | API / Auth | Wire per-tier token bucket into fanout handlers |
| 10 | API / Auth | TLS-terminate gRPC relayer |
| 11 | API / Auth | Protect harness read endpoints (internal-only or auth-gated) |
| 12 | Fanout | Implement ?from=N reconnect in SSE handler |
| 13 | Paged Queue | Align payload enum across all layers (prerequisite for v3) |
| 14 | Paged Queue | Implement v3 paginated queue Phase 1 |
| 15 | Governance | Program upgrade authority → multisig |
| 16 | Governance | Group admin and security admin → separate multisigs |
| 17 | Governance | Hot relayer/executor key → KMS-backed signer |
| 18 | Sequencer HA | Replace local sequence files with HA Postgres |
| 19 | Sequencer HA | Hot standby relayer with leader lease |
| 20 | RPC | Provision SWQoS RPC; multi-endpoint submission; priority fees |
| 21 | Stress | 100+ avg place TPS devnet test (confirmed pass or known-bound documented) |
| 22 | Stress | 24-hour devnet soak |
| 23 | Audit | External audit engagement (provide audit-scope.md + fixed code) |
| 24 | Observability | Per-intent lifecycle store and API |
| 25 | Observability | Synthetic canaries |

### P1 — Before Unrestricted Public Access

| # | Area | Item |
|---|---|---|
| 1 | Formal Verification | FV scope statement for execution queue |
| 2 | API / Auth | Body size limits; expensive-query caps |
| 3 | API / Auth | Per-key quota store (daily/monthly) |
| 4 | Fanout | Redis Pub/Sub scale-out |
| 5 | Fanout | GET /snapshot/user/{owner} |
| 6 | Fanout | WebSocket variant |
| 7 | Fanout | Durable ingest backlog (Redis streams) |
| 8 | Fee / Spam | SOL deposit-at-enqueue model |
| 9 | Paged Queue | v3 Phase 2: MakerReplaceBandV1 compaction |
| 10 | MEV / Audit | Fix H-1, H-3, H-6, H-9, H-10, H-11; all MEDIUM items |
| 11 | Stress | Trident fuzz harness (queue, health cache) |
| 12 | Stress | Multi-market parallel load test (8 markets) |
| 13 | Stress | Admin recovery drill (queue stall + gap drop) |
| 14 | Perp Events | Increase consume limit 8→16; interval 2000ms→500ms |
| 15 | Governance | On-chain 30-minute admin timelock (H-6) |
| 16 | TCS | Do not enable on mainnet until TCS audit pass |
| 17 | Observability | Correlation ID across all services |

---

## Suggested Sequencing

**Week 1–2:** Close all CRITICAL audit findings (C-1..C-5). Confirm H-7 dual-oracle status. Fix enqueue_liquidity signer (H-1). Align payload enums. Begin HI-1 Lean proof.

**Week 3–4:** v3 paginated queue Phase 1 implementation. Sequencer HA design and Postgres migration. SWQoS RPC provisioning. Fanout reconnect (?from=N), user snapshot endpoint.

**Week 5–6:** External audit engagement (runs in parallel). Per-tier rate limiting wired. Multisig deployments. 100+ TPS devnet run. 24-hour soak.

**Week 7–8:** Address audit feedback. Admin timelock on-chain. Trident fuzz setup. Per-intent lifecycle API. Synthetic canaries. Public launch candidate.
