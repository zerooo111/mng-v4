# CTO Mainnet Certification Memo — Fermi DEX (vc6)

**Date:** 2026-04-17
**Branch:** `vc6`
**Purpose:** Delta on top of `mainnet_roadmap_16apr.md` — production hardening that sits around, not inside, the existing roadmap items.

---

## Verdict

**Not ready.** The team has done real work — 9 OtterSec audits on the inherited base, 30/31 Lean properties, a full internal audit (`audit_mar20.md`), a concrete 25-item P0 roadmap (`mainnet_roadmap_16apr.md`), and a differential external audit package ready to ship. The on-chain surface area is narrow and well-scoped. But the operational substrate for a real mainnet perp DEX — HA signing, persistent ground truth, multi-RPC failover, incident tooling, governance ceremony, legal posture — is not yet in place. This memo layers on what the existing roadmap underweights, organized so it slots into the same P0/P1 format.

This memo deliberately **does not restate** the 25 P0s already in `mainnet_roadmap_16apr.md` (C-1..C-5, HI-1, multisig, sequencer HA, SWQoS, per-intent lifecycle, ?from=N reconnect, TLS, etc.). Treat those as assumed — what follows is the delta.

---

## A. On-chain hardening beyond the audit findings

### A1. Program upgrade ceremony (not just multisig existence)

Shipping `set-upgrade-authority → squads/realms 3-of-5` is table stakes; the ceremony around it is the actual control.

- Signed binary: `solana-verify build` + publish `programs/mango-v4.so` SHA-256 on the status page and in a signed release; reproducible build in CI.
- Each upgrade proposal must cite (a) the exact diff since the last deployed slot, (b) the audit trail reference, (c) a written blast-radius note.
- **Timelock of 48h (not 30 min)** for non-emergency upgrades; the 30-min admin timelock in H-6 is for parameter changes, not program bytes. An emergency-guardian 2/3 multisig may pause ingress / force reduce-only, but may **not** upgrade.
- Rehearse the full upgrade flow on devnet monthly. *If you cannot roll back within 30 minutes under pressure, you cannot upgrade under pressure.*

### A2. CTM signer rotation: staged + dual-approval

The roadmap covers `activate_at_slot ≥ current_slot + MIN_ROTATION_DELAY` (M-2). Add on-chain: reject rotation unless a distinct `security_admin` co-signs. Rationale: the relayer operational key and the signing authority over user intents must not share a compromise surface.

### A3. Oracle band + liveness checks as mandatory path

Today `oracle_state_unchecked()` is callable from token_deposit/withdraw. Make staleness + confidence validation structurally non-bypassable by:

- Deleting `*_unchecked` from the public surface (keep only for internal callers with documented invariants).
- Enforcing per-market `max_staleness_slots` and `max_confidence_bps` at the group level with non-zero defaults — a zero value should be treated as a misconfig, not "skip checks."
- Adding a **cross-source deviation band** for any token using CLMM oracles (H-7): require `|pyth_price − clmm_price| / pyth_price ≤ band_bps`; reject out of band. This is your only defense against single-block sandwiches that the audit already flagged.

### A4. Economic limits per market, per account, globally

Audit gave you the mechanics (insurance, health, liquidation); launch-day posture needs explicit caps:

- Per-market: max open interest, max single-account position, max leverage (start at 5×, not 20×).
- Per-account: max notional across markets, withdraw rate limit (sliding window).
- Global: kill-switch OI threshold that pauses new position-increasing ingress while allowing closes.
- These should be `group_admin`-editable with the 30-min timelock (H-6).

### A5. Kill-switch topology

One bit per ingress class is not enough. Minimum:

- `pause_new_positions` (allow cancels + closes)
- `pause_withdrawals` (asset quarantine)
- `pause_liquidations` (rare, but needed if a health-cache bug is suspected)
- `pause_all` (nuclear)

Each with separate ix_gate bits so partial degradation is possible without cratering liveness.

### A6. Direct-submit delay is not enough — also add user-initiated emergency cancel-all

Roadmap has C-4 (10-slot direct enqueue). Add a permissionless path `cancel_all_user_orders(mango_account)` callable by the owner with no CTM dependency and no queue dependency (touches the perp orderbook directly through an exception path). Users must always be able to reach zero exposure even if the queue is fully stalled.

### A7. Explicit reentrancy assertion in execute_multi

Internal exploration flagged this as "mitigated by health region but no explicit guard." Add `require!(!queue.in_dispatch, ReentrantDispatch)` around `dispatch_queue_payload()`. Cost is one byte + a compare; insurance against a future CPI edge we haven't thought of is unbounded.

---

## B. Off-chain security (signing, secrets, network, supply chain)

### B1. Signing-service isolation (P0, biggest gap)

Today `CTM_RELAYER_PAYER_KEYPAIR` loads a raw keypair file into the 12k-line relayer process. That process has:

- A gRPC server exposed to internal traffic
- An HTTP /metrics endpoint
- Runtime dependencies on mango-feeds-connector, reqwest, serde_json — a deserialization CVE in any transitive crate is a key exfil vector.

Move to a **signing sidecar** pattern:

- Local Unix-socket service written in minimal Rust (~500 LOC), holding the key, exposing only `sign_ctm_envelope(bytes)` and `sign_tx(msg_bytes)`.
- Allow-list: the sidecar parses the message, refuses to sign anything that isn't a known CTM envelope prefix or a tx calling our program ID with instructions in a whitelist. Hardware keys (YubiHSM 2 or AWS Nitro Enclaves with KMS-wrapped keys) behind that.
- Rate limit: sidecar enforces "max N signatures per second" and "max M per user per 24h" as a floor underneath the relayer's own limits.
- The relayer never sees the private key material; if it's compromised, the attacker can spam within the sidecar's budget, not drain.

### B2. Secret lifecycle

Move off env vars for long-lived secrets:

- Vault / AWS Secrets Manager / GCP Secret Manager with dynamic leases.
- Signed secret-rotation runbook; rotate all hot keys quarterly, immediately on any suspected compromise.
- No keypairs committed to `keypairs/` on the deploy host — that directory should be empty in prod, populated only by the deploy pipeline into a tmpfs.

### B3. Network zoning

- Three tiers: **public edge** (Cloudflare or equivalent) → **gateway VPC** (nginx / envoy + WAF) → **private VPC** (relayer, harness, executor, DB, signer). Only the gateway has a public IP.
- mTLS between gateway and internal services (roadmap has TLS for gRPC; require client certs too).
- Egress restricted: relayer can reach only whitelisted RPC endpoints + its signer socket. Blocks data exfiltration if a process is compromised.
- SSH via bastion only; no direct ssh to prod; session recording on bastion.

### B4. DDoS + WAF

Your gateway-machine-with-whitelist plan is good for internal / paying customers but insufficient once you advertise a public mainnet.

- Edge: Cloudflare (or Fastly / AWS Shield Advanced) with L3/L4 absorption, Bot Fight Mode, challenge pages for suspicious ASNs, per-IP + per-ASN rate limits.
- Geo-restriction at the edge for sanctioned jurisdictions (US OFAC list; consult counsel — this is non-negotiable, not a preference).
- WAF ruleset: OWASP Top 10 + custom rules for oversized JSON, malformed JWT, missing `Content-Type`, payload size caps (roadmap mentions this as P1 — make it P0).
- Circuit break at the gateway: if origin errors >5% over 30s, return cached 503s with a "read path still live" hint rather than DoS'ing the origin.

### B5. Tiered admission (layers your existing idea)

Your plan — free-tier IP allowlist + gas-account balance check above N tx/day — is the right shape. Two refinements:

- Gas account is **on-chain** (an escrow account per API key) so tier upgrade is a user action, not a manual process. Cron sweeps forfeit balance into insurance for spam-decayed accounts.
- Bind tier to **signed owner identity** (`mango_account.owner` signature on tier-change intent), not to API key alone — otherwise sybil bypass by rotating keys.
- Quota store in Postgres (same cluster as sequencer state — roadmap 9.2 already calls for HA Postgres).

### B6. Supply chain

- `cargo audit` + `cargo deny` in CI, fail on any RUSTSEC advisory.
- `cargo vet` for trusted-crate provenance.
- SBOM (CycloneDX) published with every release.
- Pin Rust toolchain, pin `solana-program` version, pin Anchor version; reject builds that diverge.
- Verify `anchor build --verifiable` matches CI-published SHA before mainnet deploy.
- Dependabot / Renovate with a **staging window** — no unreviewed auto-merges into the relayer crate.

---

## C. Liveness, HA, persistence

### C1. Persistent ground truth — beyond "replace /tmp files with Postgres"

Roadmap 9.2 is right but needs the operational detail:

- **Patroni + etcd** for Postgres primary/standby with automatic failover; or Cloud SQL / RDS Multi-AZ with sync replication.
- **WAL-G streaming** to S3/GCS with point-in-time recovery tested quarterly. RPO: ≤ 5 s. RTO: ≤ 5 min.
- **Schema**: `intents`, `intent_events` (append-only), `enqueue_tx`, `execute_tx`, `rate_limit_counters`, `sequencer_leases`, `admin_audit_log`. Every intent has an idempotency key; sequencer restart replays from the last committed `accepted` event.
- **Fencing**: leader lease uses a monotonic fencing token written into every tx; a deposed leader's in-flight work is rejected by the new leader.
- Schema migrations via `sqlx`/`refinery` with forward-only, additive-only in prod — never `ALTER TABLE DROP` on a live sequencer.

### C2. Exactly-once semantics are a design claim, not a library feature

Relayer must implement the full three-phase commit: (a) write intent with idempotency key to DB, (b) submit to chain, (c) mark submitted *after* seeing tx signature status. On restart, any `submitted` without confirmation must be probed before any new sequence is emitted. Current optimistic-advance logic is a stand-in; harden it with a DB round-trip per state transition.

### C3. RPC redundancy policy

- **Send path**: ≥3 providers (Helius, Triton, Jito), each with independent credentials, submit in parallel to top-2 scored by rolling p90 landing rate, fall back to rank 3 on failure. Priority fees: `cu_price` is a market — your keeper should publish and read the 75th-percentile fee from Helius priority-fee APIs, not hardcode 10k.
- **Read/WS path**: ≥2 providers with health scoring on `getSlot` latency, `getAccountInfo` lag vs. leader slot, and WS subscription miss rate. Auto-remove from rotation at 3 consecutive failures.
- **Your own backup validator** for queue-critical reads (optional but cheap if you already run one for Geyser).
- **Circuit break at relayer** on RPC regime (not just per-request): if the primary provider's p95 landing blows past 30 slots, fail over before the per-tx backoff accumulates.

### C4. Degradation ladder, published as a runbook

Normal → Elevated (SWQoS saturated) → Degraded (single-market gap skip) → Constrained (reject new positions, allow cancels) → Frozen (full pause). Each level has a defined operational trigger (metric + threshold + duration), a defined user-visible status page message, and a defined operator action. This is the thing that keeps an outage from becoming a loss event.

### C5. Insurance fund topology

One policy question deserves an explicit written decision:

- Cold-multisig-held insurance, transferred to the on-chain insurance vault only when needed? Or always on-chain (faster auto-socialize, larger hot-key blast radius)?
- Replenishment rule: X% of fees auto-accrue until fund ≥ Y% of total OI.
- Draw rule: drawable only through a specific admin instruction with the 48h timelock; emergency draws through the guardian 2/3 multisig with a post-hoc DAO ratification requirement.

---

## D. Observability (where most ops-incidents will be won or lost)

### D1. Three-pillar telemetry, not just metrics

- **Metrics** (Prometheus): already have ~40 on the relayer and ~6 on fanout. Formalize **SLIs/SLOs**: `intent_accepted_to_landed_p99 ≤ 3s`, `optimistic_divergence_per_hour ≤ 1`, `relayer_availability ≥ 99.5%`. Error-budget burn-rate alerts at 1h/6h/24h windows (Google SRE four-window method).
- **Logs** (Loki or ELK): structured JSON, every log line has `correlation_id`, `intent_id`, `mango_account`, `market_index`. Redact PII even if there's no PII today — drift happens.
- **Traces** (Tempo/Jaeger via OpenTelemetry): span from gRPC ingress → sequencer → RPC submit → log observation → harness update. A single trace for one intent across all services is the tool your on-call will reach for first.
- **Events** (audit pipe): a separate, append-only, off-host stream for every admin action (upgrades, param changes, ix_gate flips, rotations). Consumed by Slack/PagerDuty so every admin action is visible to all keyholders in real time and can be disputed within a 5-minute window.

### D2. Lifecycle store — specifics beyond "add a database"

Your observability idea ("harness exposes full tx status, optimistic success/fail, relayer submission with tx id, onchain exec success/fail with tx id") is right; make it a formal state machine:

```
received → validated → sequence_assigned → enqueue_submitted[tx_id]
 → enqueue_landed[slot] → optimistic_filled/optimistic_rejected
 → execute_landed[tx_id, slot] → confirmed_filled/confirmed_failed
 → settled (optional terminal for settlement)
```

Every transition is a DB row (append-only events table) and a Prom counter. Public API: `GET /intent/{id}` returns the full timeline; support reads this, users read this, your SLO dashboards derive from this. **Without this, your support cost/ticket diverges from zero the day you launch.**

### D3. Synthetic canaries, concrete

Roadmap 9.5 mentions these — detail:

- `canary-order-bot`: submits place + cancel every 30s from a 100-USDC account; alerts on end-to-end > 5s, any error > 0.1%, any divergence between optimistic and confirmed.
- `canary-price`: reads your oracle band every 10s, alerts if cross-source deviation exceeds configured threshold (catches H-7 regressions).
- `canary-withdraw`: 0.01 USDC deposit + withdraw once per hour from a dedicated account. Catches "withdrawals silently broken" which your SLOs won't.
- Canaries run from a **separate VPC** — not the prod VPC — so an internal-network failure doesn't silence them.

### D4. Status page and user comms as engineering deliverables

StatusPage.io or Upptime, auto-updated from the SLO system. Incident comms templates for: (a) RPC degradation, (b) sequencer failover, (c) oracle stale, (d) admin parameter change, (e) full pause. Pre-approved language so comms doesn't block on copyedits at 3am.

---

## E. Release management, chaos, process

### E1. Staged rollout, not flag day

- **Restricted launch** first: program live, but only whitelisted `market_index` set (1–3 markets), caps per account at 1000 USDC, `group_admin` still sole actor with manual sign-off on caps. 2-week soak.
- **Public launch** expands markets + caps on a schedule, gated by real load data from the restricted phase.
- **Blue/green at the gateway** for relayer upgrades — not for the on-chain program (that's timelock-governed), but for the offchain services, so a bad relayer version takes ≤ 60s to roll back.
- **Feature flags** for new ingress paths (direct-submit, new payload variants) — default off, cohort-tested, never global-on as first exposure.

### E2. Freeze windows

No non-emergency merges for 72h pre-launch. No deploys on Fridays (cargo-cult, but it works). No deploys during known event-risk windows (Fed announcement windows, known whale liquidation pre-events if you have observability that detects them).

### E3. Chaos / game days

Monthly drills, each with a written post-game:

- RPC primary goes dark (iptables block)
- Postgres primary killed mid-transaction
- Sequencer process SIGKILL'd mid-enqueue
- Oracle stale-pinned (test harness mode)
- Queue head stall injection (test harness mode)
- Admin key rotation ceremony end-to-end

Each drill produces a runbook update. The runbook with the most recent successful drill is authoritative.

### E4. Postmortem culture

Blameless, template-driven (cause, detection, mitigation, prevention, action items with owners + dates). Published internally within 72h, externally within 2 weeks for any user-facing incident above SEV2. This is an engineering-culture deliverable, not paperwork — without it you accumulate the same class of incident repeatedly.

---

## F. Governance, legal, economic security

### F1. Legal baseline (not optional)

- Terms of Service + Risk Disclosure (leverage, liquidation, oracle manipulation, smart-contract risk) — reviewed by counsel who has shipped DeFi products.
- Privacy policy including clear statement on what PII the fanout service sees (JWT claims may contain identifiers).
- Geofencing: OFAC-sanctioned jurisdictions blocked at edge; consider US/UK restrictions depending on counsel advice.
- Sanctions screening: Chainalysis or TRM integration checks depositing wallet addresses against sanctions lists at deposit time.
- Entity structure + domicile for the operating company. This is CTO-adjacent but it determines where your insurance fund lives.

### F2. External audit — two auditors, not one

Roadmap has OtterSec as primary (correct given 9 prior audits). Add a second opinion on the execution queue from a firm with strong Solana CPI / reentrancy practice (Neodyme, Zellic, or Sec3). Cost is marginal vs. the liability; disagreement between two audit reports is the signal you actually need.

### F3. Bug bounty scaled for mainnet

- Move from email intake to **Immunefi** with on-chain escrow (1–5M USDC escrowed via DAO vote).
- Cascade: critical $1M, high $50k, medium $5k, low $1k — match Immunefi market rates to attract top whitehats.
- Publish a detailed in-scope list that specifically includes the CTM signer, relayer, and harness (not just on-chain code) since off-chain compromise is now a fund-loss path.

### F4. Economic / MEV stress-test

Beyond the audit's qualitative vectors, run a quantitative attacker-profit model:

- For each CRITICAL finding, compute minimum capital + expected profit for an attack — if profit > cost even at 10% success rate, it's a P0 regardless of audit severity.
- Simulate liquidation cascade under correlated asset moves (SOL ±40% in one hour scenario). Verify socialize-loss path and insurance-fund sufficiency.
- Publish the model. Transparency here is a competitive asset.

### F5. Oracle redundancy policy (extends A3)

Minimum N=2 oracle providers per priced asset; if only one is available, asset goes into reduce-only. Written policy for adding a new asset: oracle source(s), deviation band, max_staleness, conf_filter, launched-in-reduce-only for first 7 days.

---

## G. Concrete additions to the P0 list (not yet in `mainnet_roadmap_16apr.md`)

| # | Pillar | Item | Relative size |
|---|---|---|---|
| P0-26 | On-chain | Emergency user `cancel_all` path independent of queue (A6) | 1 week |
| P0-27 | On-chain | Explicit reentrancy guard in execute_multi (A7) | 1 day |
| P0-28 | On-chain | Make oracle staleness/confidence non-bypassable (delete `*_unchecked` from ext. surface) (A3) | 3 days |
| P0-29 | Off-chain | Signing sidecar with allow-list (B1) | 2–3 weeks |
| P0-30 | Off-chain | Edge WAF + DDoS + OFAC geofence (B4) | 1 week (mostly config) |
| P0-31 | Persistence | Postgres HA ceremony: Patroni + WAL-G + PITR drill (C1) | 2 weeks incl. drill |
| P0-32 | Persistence | Three-phase commit for intent state machine (C2) | 2 weeks |
| P0-33 | RPC | Multi-provider send policy w/ dynamic priority fee (C3) | 1 week |
| P0-34 | Obs | SLO + error-budget burn-rate alerts (D1) | 1 week |
| P0-35 | Obs | Correlation-ID + OpenTelemetry tracing end-to-end (D1) | 2 weeks |
| P0-36 | Obs | Admin-action audit feed to Slack/PagerDuty (D1) | 2 days |
| P0-37 | Obs | Canary bot fleet in separate VPC (D3) | 1 week |
| P0-38 | Release | Staged market launch plan (restricted → expanded) + feature flags (E1) | 1 week planning |
| P0-39 | Release | Chaos drill playbook + first 3 drills executed (E3) | 2 weeks |
| P0-40 | Legal | ToS, privacy, risk disclosure, OFAC screening (F1) | 3–4 weeks counsel-led |
| P0-41 | Audit | Second external auditor engaged (F2) | 2–3 weeks audit |
| P0-42 | Bounty | Immunefi program with escrow (F3) | 2 weeks |
| P0-43 | Governance | Insurance fund topology + draw/replenish policy, written and ratified (C5) | 1 week |
| P0-44 | Governance | Runbooks: sequencer failover, queue stall, oracle stale, RPC outage, mass-liq, validator fork (C4) | 2 weeks |
| P0-45 | Economic | Attacker-profit model for each CRITICAL + liquidation-cascade simulation (F4) | 2 weeks |

P1 candidates to pull in before unrestricted public access: second signer tier for the CTM (dual-approval for high-value intents), on-chain admin audit mirror (so admin actions are on-chain and indexable, not just off-chain log feed), and a formal dependency freeze / reproducible-build attestation in the release pipeline.

---

## H. Suggested sequencing

The existing 8-week plan is aggressive. With the delta above the realistic expectation is **12–14 weeks** for a restricted launch, 18–20 for unrestricted public:

- **W1–3**: close CRITICALs (C-1..C-5), oracle non-bypassable (A3), reentrancy guard (A7), payload enum alignment, sidecar design complete.
- **W4–6**: Postgres HA + three-phase commit, multi-RPC policy, sidecar built, TLS everywhere, correlation IDs wired, lifecycle store v1.
- **W7–9**: second audit in parallel, multisig ceremonies, runbook drills ×3, staged market launch plan ratified, status page + comms templates, ToS + legal reviewed, canary fleet running.
- **W10–12**: restricted mainnet launch (whitelist + caps). 2-week soak.
- **W13–18**: expand markets + caps per plan; Redis scale-out on fanout when concurrent users cross the 2k threshold; economic stress-test publication; bug-bounty escrow funded and live.

---

## Bottom line

Fermi DEX is closer than many pre-launch perp DEXes — the on-chain work, formal verification, audit pipeline, and event-sink design are serious. The gaps are in the layers *around* the code: signing isolation, persistence for exactly-once claims, multi-provider RPC, end-to-end tracing, admin governance ceremony, legal posture, runbook muscle memory. Those layers are what separate a system that works on a good day from one that works on the worst day of the year. Close them before rotating the upgrade authority.

**Single highest-leverage item not yet on the list: P0-29 (signing sidecar with allow-list).** Every other recommendation in this memo implicitly assumes the hot keys cannot be stolen; without B1, that assumption is not defensible in a post-incident review.

---

## Addendum — 2026-04-20: per-market queue PDAs + oracle config lockdown

### P0-30. Oracle staleness + confidence lockdown (blocking)

Extends §A3 with concrete pre-launch deploy gates uncovered during the 2026-04-20 per-market-queue migration on devnet:

**Observed (devnet):** SOL/ETH/BTC perp markets in group `BrbJtc8ja8…` were running with `oracle_config.max_staleness_slots = 600` (~240 s at 400 ms/slot) and `conf_filter = 0.1` (10 %). These were tuned to survive the devnet sponsored Pyth feed's multi-minute publish_time gaps — unacceptable on mainnet.

**Pre-launch gate (must pass before upgrade authority rotation):**

1. **Program-side ergonomics** — fix the `max_staleness_slots` inversion in `programs/mango-v4/src/state/oracle.rs::check_staleness`: today `0` = strictest, negative = disabled, which lets an operator reading "0 means no check" brick the system. Either flip the semantics (0 = disabled, positive = max slots) behind a layout-version bump, or refuse `Some(0)` client-side with a confirmation prompt.
2. **Hard deploy-time caps.** Group admin deploy script MUST:
   - assert every perp market and every bank oracle has `max_staleness_slots ∈ (0, 25]` (10 s ceiling at 400 ms slot) AND `conf_filter ∈ (0, 0.01]`.
   - fail closed with a named operator confirmation if any value is outside these bounds.
3. **`*_unchecked` pruning.** Remove `oracle_state_unchecked` from any public CPI path (see §A3). These exist for internal callers that carry a documented invariant; anything reachable via user input must take the checked path.
4. **Cross-source deviation band.** Per §A3, reject when `|pyth − clmm| / pyth > band_bps` for any CLMM-oracle token. Fail-closed default 50 bps.
5. **Runbook: Pyth outage drill.** Rehearse monthly:
   - feed last publish > `max_staleness_slots` slots ago → `perp_admin_repair_stale_orders` + ingress freeze via emergency pause → 5-min SLA to expiry.
   - Document who pages, who hits the button, where the logs live.

### P0-31. Execution queue per-market PDA migration (done on devnet, carry forward)

The v5 queue account layout was changed from "one PDA per group with 16 sub-queue slots" to "one PDA per (group, market_index)" specifically to break the cross-market serialization the validator imposes when every reveal/commit writes to the same account. The payload:

- seeds: `[b"execution-queue-v5", group, market_index.to_le_bytes()]`
- state: `N_MAX_MARKETS = 1`, `PER_MARKET_CAPACITY = 1024` (≈ 99 KB per PDA vs 394 KB shared)
- executor derives per-market pubkeys from `(program_id, group, market_index)` — no env var needed

**Mainnet deploy order:**
1. Drain the existing shared queue (pause ingress + autodrop to live_count=0 OR drop authority-close it).
2. Upgrade the program (new seeds / new state shape).
3. Bootstrap per-market PDAs: `ts/client/scripts/execution-queue/v5-bootstrap-per-market.ts` with `V5_MARKETS=…`.
4. Bootstrap ALT covering all new queue pubkeys + per-market dispatch accounts: `v5-bootstrap-alt.ts`.
5. Restart relayer; it auto-derives per-market queues.

**Mainnet numbers to pick:**
- `EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY`: keep at 1024 for perp markets. Larger if bots burst > 1024 commits/s (not currently).
- `V4_AUTODROP_COUNT_PER_CALL`: keep at 50. Bigger values let a bad market block the autodrop worker longer per tick.

### P0-32. Ingress pre-check parity with reveal-side validation

The relayer's `v5_precheck` (`bin/service-mango-execution-engine/src/v5_precheck.rs`) currently runs stages 1, 2, 3 (lite), 4 (lite), 5, 8. Before mainnet, complete:

- **Stage 6** — mango_account state: exists, owner matches, group matches, not frozen, perp slot available.
- **Stage 7** — oracle freshness client-side: publish_time age ≤ `max_staleness_slots × slot_ms`, conf ratio ≤ `conf_filter`.
- **Stage 9** — health simulation: fetch banks + oracles, compute pre/post init-health, reject if post < dust-floor.
- **Stage 10** — payer balance: ≥ N × reveal_fee_lamports.

These are the stages that convert "every committed intent reveals cleanly" from an empirical property into a structural guarantee. Without them a bad oracle or an empty payer silently burns queue slots.

### P0-33. Reveal-handler allocation audit (batch ≥ 3)

During the 2026-04-20 investigation, batch=3 reveals reproducibly panicked with "Access violation in heap section" at a ~1 MB offset even with `ComputeBudgetInstruction::request_heap_frame(128 KB)` set. Root cause not yet nailed down; likely a `Vec` growth inside the reveal loop that allocates per-iteration (decoded_payload + dispatch_accounts slice + health cache) and hits a heap alignment issue when N ≥ 3.

**Gate:** before production-scale throughput rollout, audit `reveal_execute_market` to eliminate per-iteration heap growth. Either pre-allocate arenas up front or serialize to stack-local `[T; MAX_REVEALS]` with an explicit MAX. Current devnet is capped at `V5_REVEAL_BATCH_SIZE=2` as a workaround.

### Stack-wide WARN: devnet oracle config is **not** mainnet-safe

See `exec_q_debugging.md` "⚠️ WARN — Oracle staleness config" for the full list of param values that must be tightened before ANY mainnet deploy. Operator deploy scripts should read this file as a pre-flight checklist.
