# Mainnet Launch Certification: Exhaustive Readiness Analysis

## Context

This plan is a CTO-level certification assessment for launching the Fermi DEX (Mango Markets v4 fork + execution queue + Continuum sequencer) and its offchain stack (relayer, executor, harness, crank) to Solana mainnet. The system currently runs on devnet with real functionality but lacks several production hardening layers. This document enumerates every gap and recommends changes across safety, liveness, access control, and observability, prioritized as **P0** (launch blocker), **P1** (add within 2 weeks post-launch), or **P2** (post-launch roadmap).

---

## 1. PROGRAM SECURITY

### 1.1 Multisig Upgrade Authority on BPF Program — P0, Low complexity

**Today:** Program upgrade authority is a single Solana keypair. No multisig governance.

**Recommendation:** Transfer BPF upgrade authority to a Squads Protocol v4 multisig vault PDA.
- 3-of-5 threshold, geographically distributed signers, HSM-backed member keys.
- 24-hour timelock on program upgrades; 2-hour timelock on emergency config changes.
- Steps: deploy Squads multisig -> `solana program set-upgrade-authority <program-id> --new-upgrade-authority <squads-vault-pda>` -> verify -> test upgrade cycle on devnet through multisig before mainnet transfer.
- **No code changes required** — purely operational procedure + runbook.

### 1.2 Group Admin Multisig — P0, Low complexity

**Today:** `Group.admin` is a single pubkey controlling `group_edit`, `token_register`, `perp_create_market`, `ix_gate_set` (re-enable), `execution_queue` config, CTM signer rotation.

**Recommendation:** Transfer `Group.admin` to the same Squads vault via `group_edit(admin_opt = Some(squads_vault_pda))`. Keep `security_admin` as a separate hot key (for fast ix_gate *disabling* in emergencies — it cannot re-enable, per `ix_gate_set.rs:107-118`).

### 1.3 Execution Queue Admin Split — P1, Medium complexity

**Today:** `ExecutionQueue.admin` controls both operational tasks (pause/unpause, drop stuck heads) and governance tasks (CTM signer rotation). The executor needs sub-second signing for auto-drop flows.

**Recommendation:** Keep a dedicated hot key for operational admin (pause, drop). Route CTM signer rotation (`execution_queue_set_ctm_pending`) through the Group admin multisig. May require an on-chain change to accept `group.admin` as an alternative signer for the CTM rotation instruction.

- **File:** `programs/mango-v4/src/instructions/execution_queue.rs` (line ~946-955 for `set_ctm_pending`)

### 1.4 Emergency Halt Procedures — P0, Low complexity (documentation + testing)

**Today:** `ix_gate` u128 bitmap can disable 77 instructions; execution queue has `paused_ingress` and `paused_execute` flags. No documented halt procedures or pre-signed transactions.

**Recommendation:**
- Define 3 halt tiers: **(a)** Queue-only halt (pause_ingress + pause_execute), **(b)** Trading halt (ix_gate all order placement instructions), **(c)** Full halt (ix_gate all non-close instructions).
- Pre-sign disable transactions for security_admin (store offline, test quarterly).
- Measure time-to-halt on devnet for each tier.
- Create runbook document with exact commands and decision tree.

### 1.5 Verify `unsafe_deposit` Is Gated — P0, Low complexity

**Today:** `unsafe_deposit` is guarded by `group.is_testing()` constraint. Must verify mainnet group is created with `testing = 0`. Additionally gate via `ix_gate` as defense-in-depth.

### 1.6 Audit Finding Remediation Confirmation — P0, Varies

**Today:** Internal audit found 5 Critical, 12 High, 16 Medium, 13 Low findings (all reportedly addressed).

**Recommendation:**
- Verify remediation of C-1 (execute_multi hash recomputation), C-2 (owner validation), C-5 (socialized loss at zero OI), H-1 (enqueue_liquidity signer), H-3 (insurance fund rounding), H-4 (negative deposit index) against current on-chain code.
- Commission a focused external audit of the execution queue subsystem (~2,500 LOC across 3 files) from OtterSec or equivalent.
- **Files:** `instructions/execution_queue.rs`, `state/execution_queue.rs`, `accounts_ix/execution_queue.rs`

---

## 2. ORACLE HARDENING

### 2.1 Enable Staleness Checks — P0, Low complexity

**Today:** `OracleConfigParams.max_staleness_slots` defaults to `None` which maps to `-1` (disabled) in `oracle.rs:98`. With `-1`, `check_staleness()` at line 132-141 is a no-op. Stale oracle data is silently accepted.

**Recommendation:** For every mainnet token bank and perp market, set via `token_edit` / `perp_edit_market`:
- Major tokens (SOL, BTC, ETH): `max_staleness_slots = 120` (~48s at 400ms slots; Pyth publishes ~1s)
- Minor tokens: `max_staleness_slots = 300` (~2 min)
- Perp markets: `max_staleness_slots = 120`
- **Never leave -1 on any mainnet oracle config.**

### 2.2 Enable Confidence Interval Checks — P0, Low complexity

**Today:** `conf_filter` defaults to `0.0`, meaning `check_confidence()` at oracle.rs:144-149 allows any deviation.

**Recommendation:**
- Major tokens: `conf_filter = 0.10` (10% max confidence band as ratio of price)
- Minor tokens: `conf_filter = 0.20–0.50` depending on liquidity
- Tune based on historical Pyth confidence data per oracle.

### 2.3 Configure Fallback Oracles — P1, Low complexity

**Today:** `bank.fallback_oracle` field exists; health retriever supports fallback. Not configured.

**Recommendation:** For every mainnet token:
- Primary: Pyth feed
- Fallback: OrcaCLMM or RaydiumCLMM for liquid pairs (5% max deviation already hardcoded at `CLMM_MAX_DEVIATION_FROM_REFERENCE_BPS = 500`)
- Set via `token_edit` with `set_fallback_oracle = true`

### 2.4 Offchain Oracle Circuit Breaker — P1, Medium complexity

**Today:** No price-rate-of-change monitoring on the offchain side.

**Recommendation:** Implement a sidecar (or extend the keeper at `bin/keeper/`) that:
1. Tracks rate of change per oracle price
2. If price moves >20% in 10 slots, triggers `ix_gate` to pause trading on that market
3. Sends alert to operations team
4. Re-enables only after manual review

---

## 3. DATA PERSISTENCE & FAILOVER

### 3.1 Move State Files Off `/tmp` — P0, Low complexity (config only)

**Today:**
- Sequence state: `/tmp/ctm-sequences.json` (main.rs:236-238)
- Continuum cursor: `/tmp/ctm-continuum-cursor.json` (main.rs:248-249)
- Tick history: `/tmp/continuum-sequencer-ticks.json`

On most Linux distros `/tmp` is cleared on reboot. Loss of sequence state causes sequence gaps that stall the execution queue.

**Recommendation:**
- Create `/var/lib/fermi-dex/` on a persistent filesystem (EBS volume on AWS with automated snapshots)
- Update env vars: `CTM_RELAYER_SEQUENCE_STATE_PATH`, `CTM_RELAYER_CONTINUUM_CURSOR_PATH`, tick_history_path
- **Files:** `scripts/render_devnet_runtime.sh`, `ops/systemd/stagin4-devnet-relayer.service` env file

### 3.2 Database-Backed Sequence State — P1, Medium complexity

**Today:** File-based JSON with coalesced 250ms writes. If process crashes between sequence reserve and flush, in-memory pending state is lost (though the `next_sequence` floor survives).

**Recommendation:** Migrate to SQLite with WAL mode at the persistent path. Schema: `CREATE TABLE sequence_state (key TEXT PRIMARY KEY, next_sequence INTEGER)`. Flush on every `commit_success` instead of coalesced.

- **File:** `bin/service-mango-execution-engine/src/main.rs` (SequenceStore struct, lines ~948-1216)

### 3.3 RPC Failover — P0, High complexity

**Today:** Single `RpcClient` instance (main.rs:4738-4740). If the RPC endpoint goes down, the entire relayer stops.

**Recommendation:**
1. Configure 2-3 RPC endpoints (e.g., primary Helius, failover Triton + QuickNode)
2. Blockhash manager (main.rs:896-946) fetches from multiple RPCs, selects most recent
3. Transaction submission fans out to all RPCs for better landing rates
4. Per-RPC circuit breaker: >50% errors over 10s -> remove from pool for 30s
- **File:** `bin/service-mango-execution-engine/src/main.rs` (Engine.rpc, BlockhashManager)

### 3.4 Harness State Snapshots — P1, Medium complexity

**Today:** Optimistic + confirmed state in memory, reconciles with on-chain every 10s. All optimistic state lost on restart.

**Recommendation:**
- Snapshot harness state to disk each reconciliation cycle for warm-start on restart
- Implement catch-up mode: replay recent execution queue events from the JSONL event log
- Rotate JSONL event logs (size-cap) to prevent unbounded disk growth
- **File:** `ts/client/scripts/execution-queue/continuum-state-harness.ts`

### 3.5 Sequencer Persistence — P1, Medium complexity

**Today:** VDF state and event bus are ephemeral. Tick history path defaults to `/tmp`.

**Recommendation:**
- Move tick_history_path off `/tmp`
- Persist last VDF proof hash on orderly shutdown for verifiable continuity on restart
- Write WAL-style logs of committed ticks so consumers can catch up after missed streaming data

---

## 4. DDOS & ACCESS CONTROL

### 4.1 Gateway with IP Whitelist & API Keys — P0, Medium complexity

**Today:** Nginx at `ops/nginx/perps-dex-devnet.conf` listens on `0.0.0.0:8080` with **zero** IP restrictions, rate limiting, authentication, or TLS. Exposes `/harness/`, `/bridge/`, `/relay/`, `/solana-rpc/`, `/solana-ws/` directly.

**Recommendation:** Create a production nginx config with:

1. **Separate gateway machine** — public-facing, terminates TLS, forwards to internal services
2. **IP whitelisting on write paths:**
   ```nginx
   location /relay/ {
       allow <known-trader-IPs>;
       allow <frontend-server-ip>;
       deny all;
       # ...
   }
   ```
3. **API key check on relay/bridge endpoints:**
   ```nginx
   location /relay/ {
       if ($http_x_api_key = "") { return 401; }
       # validate against a map of known keys
       # ...
   }
   ```
4. **gRPC port (9090) must NOT be internet-exposed** — bind to `127.0.0.1` or firewall
5. **SSE endpoint rate limiting** — `limit_conn` to prevent connection exhaustion
6. **Enforce HTTPS** with TLS 1.2+, redirect HTTP -> HTTPS
7. **Read-only endpoints** (harness /state/*, /healthz) can be more permissive but still rate-limited

- **File:** `ops/nginx/perps-dex-devnet.conf` (create new `ops/nginx/perps-dex-mainnet.conf`)

### 4.2 Per-User gRPC Rate Limiting — P0, Medium complexity

**Today:** Semaphore-based InflightGate (max_inflight=128, queue_wait_timeout=500ms). No per-user accounting. `submit_intent_inner` at main.rs:2431 treats all users equally.

**Recommendation:**
- Add `HashMap<Pubkey, (u64, u32)>` tracking (window_start_ms, request_count) per `user_owner` pubkey
- Reject with `RESOURCE_EXHAUSTED` when user exceeds budget
- Check early in `submit_intent_inner` *before* acquiring semaphore permit

- **File:** `bin/service-mango-execution-engine/src/main.rs` (Engine, submit_intent_inner)

### 4.3 Fee-Based Rate Limiting — P0 (free tier) / P2 (gas accounts)

**Today:** Relayer pays all Solana tx fees via its own payer keypair. Users pay nothing.

**Recommendation — phased:**
- **Phase 1 (P0 — launch):** 20 free txns per `user_owner` per 24h UTC window. In-memory counter with periodic persistence. Hard reject excess with `RESOURCE_EXHAUSTED`.
- **Phase 2 (P2 — post-launch):** On-chain gas account escrow. Relayer checks `getBalance` on user's gas PDA before accepting excess intents.

### 4.4 TLS & mTLS — P0 (external) / P1 (internal)

**Today:** `deploy.sh` references TLS certs for nginx, but devnet config uses plain HTTP on port 8080. Internal gRPC (relayer <-> sequencer) is plain TCP.

**Recommendation:**
- External: Enforce HTTPS on nginx, TLS 1.2+ only, strong cipher suites, HSTS headers
- Internal: mTLS on gRPC between relayer and Continuum sequencer (`build_continuum_channel` at main.rs:4756-4759)

---

## 5. LOAD MANAGEMENT

### 5.1 Backpressure Tuning — P0, Low complexity (config only)

**Today:** Soft limits mostly disabled (default 0). InflightGate at generous 128.

**Recommendation — mainnet env vars:**
| Variable | Devnet Default | Mainnet Recommended |
|---|---|---|
| `CTM_RELAYER_MAX_INFLIGHT` | 128 | 64 |
| `CTM_RELAYER_QUEUE_SOFT_LIMIT` | 0 (disabled) | 512 (50% of 1024 CTM cap) |
| `CTM_RELAYER_QUEUE_GAP_SOFT_LIMIT` | 256 | 128 |
| `CTM_RELAYER_SEQUENCE_PENDING_SOFT_LIMIT` | 0 (disabled) | 32 |
| `CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS` | 500 | 200 (fail fast) |
| `CTM_RELAYER_VERIFY_USER_SIGNATURE` | true | **true (MUST stay true)** |
| `CTM_RELAYER_SUBMIT_MODE` | strict | strict (**not** "fast" on mainnet) |
| `EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT` | true | **false** (enable preflight on mainnet) |
| `CTM_RELAYER_PRIORITIZATION_FEE` | 0 | 10000+ (microlamports) |
| `EXECUTION_QUEUE_CRANK_PRIORITIZATION_FEE` | 0 | 10000+ (microlamports) |

### 5.2 Graceful Degradation — P1, Medium complexity

**Today:** Harness readiness check rejects all submissions when any market diverges.

**Recommendation:**
- Verify per-market drift detection (main.rs:1755-1766) works correctly so market A divergence doesn't block market B
- Add "maintenance mode" flag for graceful rejection with human-readable message
- Implement orderly shutdown: on SIGTERM, stop accepting requests -> drain semaphore -> flush state -> exit

### 5.3 Executor Robustness — P1, Medium complexity

**Today:** Auto-drop expired heads after 5s grace, lane failure threshold=3 with 10s backoff.

**Recommendation:**
- Increase `EXECUTION_QUEUE_CRANK_EXPIRED_HEAD_DROP_GRACE_SECS` from 5 to 10-15s (avoid dropping slow-to-confirm items)
- Move lane cache off `/tmp` if applicable
- Add RPC circuit breaker in executor loop: 10 consecutive failures -> pause 30s + alert

---

## 6. OBSERVABILITY & MONITORING

### 6.1 Persistent Metrics via Prometheus + Grafana — P0, Low complexity

**Today:** In-memory `AtomicU64` counters (main.rs:424-451) reset on restart. `/metrics` endpoint renders Prometheus text format. Comprehensive counters exist but are ephemeral.

**Recommendation:**
1. Deploy Prometheus to scrape `/metrics` every 15s from both relayer and harness
2. Deploy Grafana with dashboards for: queue depth, submission rate, execution latency, error rates, RPC health
3. Add missing gauge metrics:
   - `execution_engine_queue_depth` (current on-chain queue size)
   - `execution_engine_queue_gap` (max_seen - next_execute)
   - `execution_engine_rpc_errors_total` (by RPC method)
   - `execution_engine_blockhash_refresh_failures_total`
   - Per-user submission counts

- **File:** `bin/service-mango-execution-engine/src/main.rs` (Metrics struct, lines ~424-451)

### 6.2 Alerting — P0, Medium complexity

**Today:** Watchdog (`scripts/devnet_stack_watchdog.sh`) auto-restarts services but sends no notifications. Meta-watchdog (`watch_the_watcher.sh`) is dev-oriented. No PagerDuty, no Slack alerts.

**Recommendation:** Deploy Alertmanager connected to Prometheus:

| Condition | Severity | Channel |
|---|---|---|
| Service down > 30s | Critical | PagerDuty |
| Queue depth > 768 (75% CTM cap) | Warning | Slack |
| Queue depth > 960 (94%) | Critical | PagerDuty |
| Execute errors > 10/min | Warning | Slack |
| Harness reconciliation drift > 0 markets | Warning | Slack |
| RPC error rate > 20% | Warning | Slack |
| RPC error rate > 50% | Critical | PagerDuty |
| Blockhash refresh failure > 3 consecutive | Critical | PagerDuty |
| Oracle staleness trip on any market | Critical | PagerDuty |
| Watchdog restart of any service | Warning | Slack |

### 6.3 Transaction Lifecycle Tracking — P0 (logging) / P1 (DB + API)

**Today:** Events emitted to event sink URL; harness tracks optimistic vs confirmed; tx signatures returned in `SubmitIntentResponse`. But no unified lifecycle view.

**Recommendation — full tx lifecycle:**
1. **Submission:** Log `(sequence, user_owner, market, tx_signature, timestamp, status=submitted)`
2. **On-chain ingress:** Detect `QueueItemEnqueued` event (execution_queue.rs:33-39) via Geyser or log parsing
3. **Execution:** Detect `QueueItemProcessed` event (lines 41-47) with final status (Executed/Failed/Skipped) + tx_signature
4. Store in PostgreSQL (already used by fills/health/orderbook services)
5. Expose `GET /status/{sequence}` API returning full lifecycle
6. The harness SSE stream should include tx_signature at each stage

### 6.4 Centralized Logging & Audit Trail — P1, Medium complexity

**Today:** JSONL event logs on disk. systemd captures stdout/stderr. No centralized aggregation.

**Recommendation:**
- Ship all logs to ELK/Loki/CloudWatch
- Structured audit entries for all admin operations (ix_gate changes, CTM signer rotations, queue pauses)
- 90-day retention minimum
- Log rotation on local files to prevent disk exhaustion

---

## 7. OPERATIONAL READINESS

### 7.1 Incident Response Runbooks — P0, Low complexity (documentation)

Create runbooks for:
1. **Service failure:** Diagnose, restart, verify each component
2. **Queue stall:** Check head status -> check lane matching -> check RPC -> admin-drop decision tree
3. **Oracle failure:** Verify staleness -> pause affected markets -> communicate -> resume
4. **Security incident:** Emergency halt (tier a/b/c) -> communication plan -> fund recovery
5. **Key compromise:** Rotation procedures per key type (payer, CTM signer, admin, security_admin)
6. **RPC degradation:** Failover activation, secondary RPC enrollment
7. **Data corruption:** Sequence state recovery, harness cold start, queue draining

### 7.2 Key Management — P0 (permissions + rotation docs) / P1 (KMS/HSM)

**Today:** Keypairs in JSON files loaded at startup. No HSM, no encrypted-at-rest.

**Recommendation:**
- **Immediate:** `chmod 400` all keypair files, owned by service user only
- **Document + test** CTM signer rotation via `execution_queue_set_ctm_pending` (built-in activation slot delay)
- **Post-launch:** Migrate payer and CTM signer keys to AWS KMS / GCP Cloud KMS with audit logging
- **Admin member keys:** Hardware wallets (Ledger) for multisig members

### 7.3 Mainnet Configuration — P0, Low complexity

**Today:** All config via `devnet-stack.env`. No mainnet-specific config exists.

**Recommendation:** Create `mainnet-stack.env` with production overrides (see section 5.1 table). Critical checks:
- `CLUSTER_URL_OVERRIDE` = premium mainnet RPC (Helius/Triton), not devnet
- All `/tmp/` paths -> `/var/lib/fermi-dex/`
- Non-zero priority fees
- Preflight enabled
- User signature verification enforced
- `group.testing = 0` confirmed

### 7.4 Deployment Procedures — P0, Medium complexity

**Today:** `deploy.sh` (12K lines) is idempotent but no blue-green or rollback plan.

**Recommendation:**
- **Verifiable build verification:** Compare deployed program hash with CI artifact SHA256 before `solana program deploy`
- **Deployment checklist:** Pre-deploy (backup state, verify build) -> deploy (upgrade program via multisig, restart services) -> post-deploy (verify health, queue progression, harness reconciliation)
- **Rollback plan:** Document how to redeploy previous program version via upgrade buffer
- **Blue-green (P1):** Maintain hot standby that can be promoted

---

## 8. TESTING & VALIDATION

### 8.1 End-to-End Integration Tests — P0, High complexity

**Today:** 47 Rust program tests, TypeScript spec stubs (many skipped), soak/stress scripts. No automated full-pipeline E2E tests.

**Recommendation:** Build automated E2E test:
1. Submit intent via gRPC -> verify on-chain enqueue -> verify executor processes -> verify harness state -> verify final perp position
2. Run as CI job on every PR
3. Run soak tests on mainnet-fork (local validator with mainnet account snapshots)

### 8.2 Chaos Testing — P1, High complexity

Fault injection tests:
- RPC blackout 30s -> verify recovery
- Harness crash mid-operation -> verify relayer rejects + recovers
- Inject always-failing queue item -> verify auto-drop recovery
- Sequencer failure -> verify fallback ingress mode
- Clock advance -> trigger oracle staleness -> verify market pause
- Disk full -> verify graceful failure of state persistence

### 8.3 Load Testing at Mainnet Scale — P1, Medium complexity

- 2x expected peak throughput
- P50/P95/P99 latency measurement for: submission, queue-to-execution, end-to-end
- 100+ concurrent users at max rate
- Verify queue capacity limits (1024 CTM, 128 liquidity) work under load

### 8.4 Focused Security Audit of Execution Queue — P0, External

- Commission external audit of the 3-file execution queue subsystem (~2,500 LOC)
- Fuzzing with Trident or equivalent Anchor fuzzer
- Penetration test on gateway/nginx, gRPC, harness SSE
- Bug bounty ($1M cap) publicly advertised with clear scope

---

## PRIORITY SUMMARY

### P0 — Launch Blockers (18 items)

| # | Item | Complexity |
|---|---|---|
| 1.1 | BPF upgrade authority -> multisig | Med |
| 1.2 | Group admin -> multisig | Low |
| 1.4 | Emergency halt procedures + runbook | Low |
| 1.5 | Verify unsafe_deposit gated | Low |
| 1.6 | Audit finding remediation confirmed | Varies |
| 2.1 | Oracle staleness checks enabled | Low |
| 2.2 | Oracle confidence checks enabled | Low |
| 3.1 | State files off /tmp | Low |
| 3.3 | RPC multi-endpoint failover | High |
| 4.1 | Gateway: IP whitelist + API keys + TLS | Med |
| 4.2 | Per-user rate limiting | Med |
| 4.3 | Free tier cap (20 txns/day/user) | Low |
| 5.1 | Backpressure tuning for mainnet | Low |
| 6.1 | Prometheus + Grafana deployment | Low |
| 6.2 | Alerting (PagerDuty + Slack) | Med |
| 7.1 | Incident response runbooks | Low |
| 7.3 | Mainnet env config | Low |
| 7.4 | Deployment checklist + rollback plan | Med |
| 8.1 | E2E integration test | High |
| 8.4 | External security audit (exec queue) | External |

### P1 — Within 2 Weeks Post-Launch (14 items)

1.3 Exec queue admin split, 2.3 Fallback oracles, 2.4 Oracle circuit breaker, 3.2 SQLite sequence state, 3.4 Harness snapshots, 3.5 Sequencer persistence, 4.4 Internal mTLS, 5.2 Graceful degradation, 5.3 Executor robustness, 6.3 Tx lifecycle DB + API, 6.4 Centralized logging, 7.2 KMS/HSM migration, 8.2 Chaos testing, 8.3 Load testing at scale

### P2 — Post-Launch Roadmap (1 item)

4.3 On-chain gas account escrow for paid tier

---

## VERIFICATION

After implementing changes, verify mainnet readiness by:

1. **Multisig:** Execute a no-op program upgrade through Squads on devnet; verify all 3 halt tiers via multisig
2. **Oracles:** Set staleness to 1 slot on devnet, advance clock, confirm `OracleStale` error fires; repeat for confidence
3. **Persistence:** Reboot the relayer VM, verify sequence state survives and queue resumes without gaps
4. **RPC failover:** Block primary RPC at firewall, verify automatic failover to secondary within 5s
5. **DDoS:** From an unlisted IP, attempt relay submission -> confirm 403; submit 21 intents from one user -> confirm 21st rejected
6. **Observability:** Verify Prometheus scrapes populate Grafana dashboards; trigger an alert rule, confirm PagerDuty/Slack delivery
7. **E2E:** Run full pipeline test: intent -> enqueue -> execute -> harness state matches -> on-chain position correct
8. **Emergency halt:** Execute each halt tier, measure time-to-halt, verify trading stops, verify resume works
9. **Soak test:** 60-minute sustained load at target throughput on mainnet-fork validator
