# Continuum Ingress Integration Summary

Moving to the continuum ingress path as the primary means for transaction ingestion. This document reviews current state, lays out a head-to-head benchmark plan vs. the direct intent path, and proposes per-API-key ingress gating for production rollout.

---

## 1. Current Implementation

### Continuum ingress (`continuum-proxy-rust`)
- **Listeners**: gRPC `:50052`, REST `:4000`, WS `:50053` — `src/main.rs:33-345`
- **Auth**: `x-api-key` → PG `api_keys` table, tier-linked via FK to `rate_limit_tiers`, moka-cached for 60s (`src/auth.rs:57-100`)
- **Rate limit**: token bucket per-key (tier `requests_per_second`/`requests_per_minute`/`burst_size`) + per-IP fallback (`src/rate_limit.rs`, `src/middleware/rate_limit.rs`)
- **DDoS**: max connections per IP, connection tracker per-key (`src/middleware/ddos.rs`, `auth.rs:103-116`)
- **Handoff**: batched gRPC stream (100ms tick interval) → sequencer `:50051`; REST `/submit-tx` proxied to upstream gRPC via tonic

### Direct intent path
- **Route**: `POST /relay/submit-intent` → relayer bridge `:9092` (`src/services/relayer_bridge.rs:54-65`)
- **Auth**: **optional** — whitelisted origin bypasses key check entirely (`api.rs:88-90`)
- **No connection tracking**, no batching; pass-through
- Same tier rate-limit applies *if* a key is present, otherwise per-IP

### Gap
The direct path is effectively a back door:
- No per-endpoint tier enforcement
- No per-path metrics
- No uniform telemetry across ingress paths
- Origin whitelist skips auth entirely — can't attribute traffic to a key

---

## 2. Benchmark Plan (continuum vs. direct)

### Existing data
- `soak_test_report.log`: harness sustains ~1.3 TPS avg with 0 stall windows over 60 min — functional soak, not saturation
- Memory (Apr 14 lane-guard fix): intent acceptance rate lifted 48% → 78% after `LANE_PENDING_SOFT_LIMIT` 2→20, stale 8s→2s, `TS_NODE_TRANSPILE_ONLY=1`
- Neither measures ingress paths head-to-head

### Proposed head-to-head
Devnet, identical signed intents, same downstream harness.

| Dimension | Method | Tool |
|---|---|---|
| **Latency** | single-shot RTT p50/p95/p99 for `submit-tx` (continuum gRPC + REST) vs `submit-intent` (direct REST), 10k samples each at 10/100/500 rps | `ghz` for gRPC, `k6`/`wrk` for REST |
| **Throughput** | ramp 50→2000 rps until error-rate > 1% or p99 > 100ms; record saturation point | `k6` constant-arrival-rate |
| **Stability** | 60-min soak at 80% of saturation; track stall windows, queue depth, lane rejections, RPC reconciliation conflicts (the harness congestion root cause from memory) | extend `scripts/run_60min_soak_test.sh` with a `--ingress=continuum\|direct` flag |
| **Overhead isolation** | same test with auth cache warm vs. cold; with/without DDoS middleware — quantifies continuum's added hops | toggle via env |

### Metrics to add before running
Already wired: `gateway_db_query_duration_seconds`, `gateway_trade_ingest_*`.

Add for clean comparison:
- `gateway_intent_submit_latency_seconds{path}` histogram
- `gateway_intent_submit_total{path,status}` counter

Without these the comparison is noisy.

---

## 3. Per-API-Key Ingress Gating for Production

The existing schema has what's needed — extend rather than rebuild.

### Schema change (additive, safe migration)
```sql
ALTER TABLE api_keys
  ADD COLUMN allowed_ingress TEXT[] NOT NULL DEFAULT '{continuum}';
-- values: 'continuum','direct'
```
Default `{continuum}` makes continuum the default for **new** keys. Backfill existing keys with `{continuum,direct}` during migration to avoid breaking them, then roll off `direct` per-key.

### Enforcement points
- Auth middleware loads `allowed_ingress` alongside `tier_id` (already cached in moka — free)
- `src/middleware/auth.rs` (REST) and `src/proxy.rs:98-106` (gRPC) reject with 403 if the current route's path-tag ∉ `allowed_ingress`
- Tag routes at router setup:
  - `/relay/submit-intent` → `"direct"`
  - `/submit-tx` + gRPC `SubmitTx` → `"continuum"`

### Close the origin bypass
Remove the "whitelisted origin skips auth" branch in `api.rs:88-90` for intent routes — require a key always. Origin check stays for CORS, not auth.

### Rollout
1. Ship schema + enforcement behind a `PER_KEY_INGRESS_GATING=shadow` env flag (log-only violations for 1 week)
2. Flip to `enforce` after reviewing violation log
3. Per-key cutover: update `allowed_ingress` to remove `'direct'` as each client migrates; no code deploy needed per cutover
4. Final step: drop the direct route entirely once all keys show zero direct usage for N days

### Admin surface
Need `PATCH /admin/keys/:id/ingress` behind a separate admin key so ops can flip a key without a DB shell.

This keeps the existing database-driven control plane and avoids a parallel config system.
