# Mainnet Certification Plan for Mango v4 + Execution Queue

## Summary

- Verdict: not certifiable for mainnet as-is. The core onchain controls are meaningful, but the offchain stack is still single-host/devnet-shaped: relayer sequencing is file-backed in `bin/service-mango-execution-engine/src/main.rs`, harness durability is JSONL-on-local-disk in `ts/client/scripts/execution-queue/continuum-state-harness.ts`, watchdog logic is shell restart/truncate behavior in `scripts/run_service_monitor.sh`, and the sample edge proxy has no auth or rate limiting in `ops/nginx/perps-dex-devnet.conf`.
- Launch gate: complete P0 across governance, HA sequencing, oracle/risk sentinels, ingress protection, lifecycle observability, and failure drills before mainnet.
- Existing strengths to preserve: onchain queue pause flags, staged CTM signer rotation, oracle confidence/staleness checks, fallback oracles, direct-enqueue fallback, and harness divergence tooling.

## Key Changes

### Governance and key management

- Move program upgrade authority, group admin, security admin, queue admin, and treasury controls to separate multisigs.
- Add a 24-48h timelock for upgrades and parameter changes; keep a smaller emergency guardian multisig allowed only to pause ingress, force reduce-only, or disable markets.
- Keep hot relayer/executor keys in a remote signer or KMS-backed signer; keep upgrade/admin keys cold and non-exportable.
- Require dual approval for CTM signer rotation even though staged rotation already exists onchain.
- Ship reproducible releases with artifact hash, program hash attestation, SBOM, and canary soak before any authority action.

### Write path, sequencing, and liveness

- Replace local sequence files with HA Postgres as the control-plane source of truth.
- Persist, transactionally: market sequence cursors, accepted intents, idempotency keys, enqueue tx state, execute tx state, and failover lease state.
- Run exactly one active relayer leader at a time via lease/advisory lock; standby relayer stays hot but fenced until takeover.
- Do not run active-active sequencers. FIFO requires single-writer sequencing unless allocation is externalized.
- Split roles into dedicated relayer leader, dedicated executors/event crankers, and dedicated keepers/liquidators. Keep embedded executor only as tertiary backup.
- Use at least 2 independent sender RPCs and 2 independent read/WebSocket RPCs with health scoring and automatic failover.
- Prefer a private staked/SWQoS sender endpoint; optionally add Jito low-latency send/bundle path for execute transactions.
- Use `signatureSubscribe` plus `getSignatureStatuses(searchTransactionHistory=true)` fallback for tx tracking.
- Estimate fees from `getRecentPrioritizationFees`; default public flow to `skipPreflight=false` at launch and enable fast mode only behind a feature flag after soak.

### Safety and risk controls

- Review every market/bank oracle config and set explicit `conf_filter`, `max_staleness_slots`, and fallback oracle policy per asset.
- Add an offchain oracle sentinel that monitors staleness, confidence width, primary-vs-fallback deviation, external venue drift, and market-hours anomalies.
- Fail closed only for risk-increasing actions. If oracle, harness, or reconciliation degrades, reject new opens, but keep cancels, reduce-only closes, liquidations, withdrawals, and direct-enqueue available.
- Add an admission controller ahead of relayer submission: signature verification, replay nonce, ownership checks, idempotency, quota/billing checks, and risk classing.
- Prioritize cancels, reduce-only, liquidations, and admin recovery ahead of new opens and quote spam.
- Turn direct-enqueue into a real production feature: audited SDK/CLI, docs, runbooks, and status-page instructions.
- Remove production footguns: no public `/airdrop` or `/airdrop-deposit`, no test-only admin actions in production builds, no self-heal logic that truncates authoritative logs.

### Ingress, DDoS, and graceful load handling

- Keep only the gateway public. Relayer gRPC, harness ingest, metrics, admin endpoints, Postgres, and RPC proxies stay private.
- Use TLS everywhere and mTLS for internal service-to-service calls.
- Enforce API keys and IP allowlists at the gateway as planned, but add per-key, per-owner, per-market, and per-endpoint token buckets.
- Separate limits for submit, SSE, snapshot, and admin paths; cap body size, concurrent streams, and expensive query fanout.
- Tie paid/free-tier policy to signed owner plus gas-account balance, not only IP, to reduce sybil bypass.
- Add overload modes: reject new opens first, keep cancel/reduce-only lanes open, cap optimistic fanout, cap SSE clients, and return explicit backpressure errors rather than silently queueing.

### Lifecycle observability and monitoring

- Add an authoritative intent-lifecycle store and API with a stable `intent_id`.
- Required lifecycle states: `received`, `admitted`, `sequenced`, `submitted(txid)`, `enqueue_landed(slot, commitment)`, `execute_sent(txid)`, `executed|failed|skipped`, `confirmed_view_applied`.
- Extend harness/bridge APIs with per-intent endpoints and SSE events; aggregate queue metrics alone are insufficient for mainnet support.
- Support lookup by `intent_id`, `sequence`, `client_order_id`, `enqueue_tx_signature`, and `execute_tx_signature`.
- Standardize logs, metrics, and traces on one correlation id from gateway through relayer, executor, and harness.
- Run Prometheus, Grafana, Alertmanager, and structured log storage; keep Timescale as analytics/candles storage, not control-plane truth.
- Page on relayer leader loss, Postgres failover/lag, queue head age, queue growth, sequence gaps, duplicate sequence allocation, enqueue non-landing, execute non-landing, oracle stale/wide, harness divergence, RPC unhealthy, rising 429/5xx, gas-account depletion, and abnormal API-key traffic.
- Run continuous synthetic canaries that submit, cancel, and close on a tiny internal account and verify end-to-end lifecycle latency.

## Test Plan

- Certification blockers before launch: relayer leader failover drill, Postgres failover drill, RPC outage drill, oracle degrade drill, queue stall drill, DDoS/load-shed drill, hot-key rotation drill, and direct-enqueue user fallback drill.
- Required automated coverage: queue-path program tests, relayer sequence/idempotency tests, harness replay determinism, lifecycle-state tests, failover replay from durable store, and oracle-threshold tests per market.
- Required SLO gates: submit availability, enqueue landing latency, execute latency, queue drain rate, divergence recovery time, false-positive throttle rate, and MTTR for relayer/RPC loss.

## Assumptions and defaults

- Initial mainnet posture is conservative: single active sequencer, multi-replica read path, private write path, and stronger protection of cancel/reduce-only liveness than new-order throughput.
- Keep the current systemd-style deployment model for launch if desired, but only on multiple dedicated hosts/AZs with immutable config management; do not introduce Kubernetes pre-launch unless the team already runs it well.
- External standards referenced: Solana `sendTransaction`, `simulateTransaction`, `getSignatureStatuses`, `getHealth`, `getRecentPrioritizationFees`, and `signatureSubscribe`; Pyth best practices on confidence intervals, aggregation, and market-hours behavior; Jito low-latency send/ShredStream docs.

## Reference sources

- https://solana.com/docs/rpc/http/sendtransaction
- https://solana.com/docs/rpc/http/simulatetransaction
- https://solana.com/docs/rpc/http/getsignaturestatuses
- https://solana.com/docs/rpc/http/gethealth
- https://solana.com/docs/rpc/http/getrecentprioritizationfees
- https://solana.com/docs/rpc/websocket/signaturesubscribe
- https://docs.pyth.network/price-feeds/best-practices
- https://docs.pyth.network/price-feeds/how-pyth-works/oracle-program
- https://docs.pyth.network/price-feeds/market-hours
- https://docs.jito.wtf/lowlatencytxnfeed/
