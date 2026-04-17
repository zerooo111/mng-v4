# Fanout Service — Setup & Operations Guide

This guide covers the complete two-machine deployment of `service-fanout` on the
backend instance and `continuum-proxy-rust` on the gateway instance.

---

## Topology

```
                      GATEWAY MACHINE
                  ┌───────────────────────────────────────────────┐
[external client] │  continuum-proxy (port 4000, behind nginx)    │
  x-api-key ────► │                                               │
                  │  auth_middleware  (UUID key → PG lookup)      │
                  │  key_rate_limit   (rps/rpm/burst from DB)     │
                  │  ddos_middleware  (IP filter)                  │
                  │                                               │
                  │  GET /events/stream/:market ──────────────────┼──► BACKEND :9094/stream/:market
                  │  GET /events/snapshot/:market ────────────────┼──► BACKEND :9094/snapshot/:market
                  │  GET /events/markets ──────────────────────── ┼──► BACKEND :9094/markets
                  └───────────────────────────────────────────────┘

                      BACKEND MACHINE  (this machine)
                  ┌───────────────────────────────────────────────┐
                  │  execution engine  :9090 / :9093             │
                  │       │                                       │
                  │       └──── POST /ingest ──────────────────►  │
                  │  service-fanout  :9094                        │
                  │                                               │
                  │  POST /ingest ◄── from execution engine       │
                  │       │                                       │
                  │       └──── POST /ingest/relay-intent ─────►  │
                  │                      harness :9091             │
                  │       │                                       │
                  │  broadcast::channel per market                │
                  │       │                                       │
                  │  GET /stream/:market  ──► proxy ──► clients   │
                  │  GET /snapshot/:market ─► proxy ──► clients   │
                  │  GET /markets ──────────► proxy ──► clients   │
                  │  GET /healthz (unauthenticated)               │
                  │  GET /metrics (unauthenticated)               │
                  └───────────────────────────────────────────────┘
```

Firewall rule on the backend: **only the gateway machine's IP is allowed to reach
port 9094**. Port 9091 (harness) remains accessible only where the proxy still
needs direct harness reads.

---

## 1. Build & install `service-fanout` on the backend

```bash
cd /home/hetalkenaudekar/mng-v4/bin/service-fanout
cargo build --release
# binary at: target/release/service-fanout
```

### Systemd unit

Create `/etc/systemd/system/stagin4-devnet-fanout.service`:

```ini
[Unit]
Description=Fermi DEX Fanout Service (real-time SSE event stream)
PartOf=stagin4-devnet.target
After=stagin4-devnet-harness.service

[Service]
Type=simple
User=hetalkenaudekar
WorkingDirectory=/home/hetalkenaudekar/mng-v4
EnvironmentFile=/home/hetalkenaudekar/mng-v4/.devnet/systemd/devnet-stack.env

# Core config
Environment=FANOUT_BIND_ADDR=0.0.0.0:9094
Environment=FANOUT_CHANNEL_CAPACITY=4096
Environment=FANOUT_SNAPSHOT_MAX_EVENTS=256
Environment=FANOUT_MAX_CONNECTIONS=2000

# Auth is handled entirely by continuum-proxy.
# Fanout is an internal service — only the proxy machine reaches it.
Environment=FANOUT_AUTH_DISABLED=true

# Guards /ingest against rogue event injection.
# Set the same value in CTM_RELAYER_EVENT_SINK_AUTH_TOKEN if you enable
# authenticated relayer -> fanout ingest in a future relayer build.
Environment=FANOUT_INGEST_SECRET=<shared-ingest-secret>

# Preserve harness lane-guard + diagnostics side effects.
Environment=FANOUT_UPSTREAM_INGEST_URL=http://127.0.0.1:9091/ingest/relay-intent

ExecStart=/home/hetalkenaudekar/mng-v4/bin/service-fanout/target/release/service-fanout
Restart=on-failure
RestartSec=5
StandardOutput=append:/home/hetalkenaudekar/mng-v4/.devnet/logs/service-fanout.log
StandardError=append:/home/hetalkenaudekar/mng-v4/.devnet/logs/service-fanout.log

[Install]
WantedBy=stagin4-devnet.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable stagin4-devnet-fanout
sudo systemctl start  stagin4-devnet-fanout
sudo systemctl status stagin4-devnet-fanout
```

---

## 2. Event pipeline — execution engine → fanout → harness

Keep fanout behind the relayer feature gate until the deployment decision is
final. Enable local/backend fanout with:

```bash
CTM_FANOUT_MODE=local
CTM_FANOUT_BASE_URL=http://127.0.0.1:9094
```

Fanout handles the realtime reader plane and mirrors the same payload upstream
to the harness when `FANOUT_UPSTREAM_INGEST_URL` is configured:

1. Execution engine posts `relay_intent_accepted` / `relay_intent_status` to `service-fanout`.
2. Fanout updates its per-market snapshot ring and SSE broadcast channels.
3. Fanout forwards the same JSON payload to `http://127.0.0.1:9091/ingest/relay-intent`.
4. The harness keeps its existing lane-guard, optimistic-status, and diagnostics side effects.

This keeps the relayer's hot path isolated from public readers without forcing
the harness to give up its internal relay-intent ingest semantics.

---

## 3. Configure `continuum-proxy-rust` environment

Add to the proxy's `.env` or deployment config:

```bash
# Backend machine IP and fanout port
FANOUT_ADDR=http://<backend-machine-ip>:9094
```

Restart the proxy after setting this so `/events/*` routes resolve to the
backend fanout service.

---

## 4. Firewall rules (backend machine)

Allow **only** the gateway machine's IP to reach port 9094 (fanout read + ingest).
All other external access to 9094 should be blocked.

```bash
# Allow gateway machine to reach fanout (read + ingest)
sudo ufw allow from <gateway-ip> to any port 9094 proto tcp

# Deny all other external access to 9094
sudo ufw deny 9094

# Port 9091 (harness) — allow gateway for the proxy relay task
sudo ufw allow from <gateway-ip> to any port 9091 proto tcp

# Verify
sudo ufw status numbered
```

With these rules:
- External clients **cannot** reach `/ingest` directly — they must go through the proxy for reads only.
- The proxy can call `/stream/:market`, `/snapshot/:market`, and `/markets`.
- The execution engine calls fanout on loopback, and fanout mirrors to the harness on loopback.

---

## 5. API key management

API keys are managed in the proxy's PostgreSQL database. The proxy's admin
dashboard (port 50054) provides CRUD operations.

Each key carries:

| Field | Description |
|-------|-------------|
| `api_key` | UUID, sent by clients as `x-api-key` header |
| `client_name` | Human label |
| `active` | Enable/disable without deleting |
| `max_connections` | Max simultaneous SSE streams for this key |
| `requests_per_second` | Token bucket refill rate |
| `requests_per_minute` | Sliding window limit |
| `burst_size` | Token bucket capacity |

Direct DB insert example:
```sql
INSERT INTO api_keys (client_name, active, max_connections)
VALUES ('my-trading-bot', true, 5)
RETURNING api_key;
-- assign the UUID to a rate_limit_tier if needed:
UPDATE api_keys SET tier_id = 2 WHERE client_name = 'my-trading-bot';
```

---

## 6. Client usage

### Step 1 — Discover available markets

```bash
curl -H "x-api-key: <uuid>" https://<gateway>/events/markets
# ["0", "1", "2", ...]
```

Market keys are **integer index strings** (`"0"`, `"1"`, `"2"`, …), not pubkeys.
The `group` field in each event carries the program/group pubkey if you need it.

### Step 2 — Fetch snapshot

```bash
curl -H "x-api-key: <uuid>" https://<gateway>/events/snapshot/0
# { "market": "0", "last_sequence": 1042, "updated_at_ms": ..., "recent_events": [...] }
```

Note the `last_sequence` value — use it as the `from` parameter next.

### Step 3 — Open SSE stream

```bash
curl -H "x-api-key: <uuid>" \
     -H "Accept: text/event-stream" \
     "https://<gateway>/events/stream/0?from=1042"
```

Events arrive as:
```
event: event
data: {"event_type":"relay_intent_accepted","market":"0","group":"AscEq4A...","sequence":"1043",...}

event: event
data: {"event_type":"relay_intent_status","market":"0","group":"AscEq4A...","sequence":"1044",...}

event: resync
data: lagged_by=120
```

On `event: resync` — repeat steps 2 and 3.

### Response headers from the proxy

```
x-ratelimit-limit: 20
x-ratelimit-remaining: 17
```

---

## 7. Health checks & monitoring

```bash
# Fanout liveness (no auth, direct or via proxy)
curl http://<backend>:9094/healthz
curl https://<gateway>/events/healthz    # proxied

# Fanout Prometheus metrics (backend direct — not exposed through proxy)
curl http://<backend>:9094/metrics

# Proxy metrics
curl http://<gateway>:9090/metrics       # metrics-exporter-prometheus port
```

Key fanout metrics to watch:

| Metric | Alert threshold |
|--------|----------------|
| `fanout_active_subscribers` | > 1800 (near 2000 global cap) |
| `fanout_lagged_events_total` rate | > 0/s (clients falling behind) |
| `fanout_connections_rejected_total` rate | > 0/s (cap hit) |
| `gateway_fanout_sse_connections_active` | Monitor for leaks |

---

## 8. Phase completion audit against `fanout.md`

### Phase 1 — Core Fanout ✅ (complete)

| Item | Status | Notes |
|------|--------|-------|
| `POST /ingest` broadcast + snapshot update | ✅ | `ingest.rs` |
| `POST /ingest/batch` | ✅ | `ingest.rs` |
| `GET /stream/:market` SSE + lag/resync | ✅ | `sse.rs` |
| `GET /snapshot/:market` with 256-event ring | ✅ | `snapshot.rs` |
| `GET /markets` | ✅ | `snapshot.rs` |
| `/healthz`, `/metrics` | ✅ | `main.rs`, `metrics.rs` |
| Systemd unit | ⚠️ | Template in this guide; file not yet created |

### Phase 2 — Auth + Connection Controls ✅ (superseded by proxy — better)

| Item | fanout.md plan | Actual implementation |
|------|---------------|----------------------|
| API key verification | `FANOUT_API_KEYS` env var (HashMap) | **Proxy PostgreSQL DB** with `moka` TTL cache — DB-backed, hot-reloadable, per-key limits |
| JWT verification | HS256 manual impl | Not used (proxy uses UUID keys) — code present in `auth.rs` as fallback |
| Per-user connection counter | `DashMap<user_id, AtomicU32>` | **Proxy `ConnectionTracker`** — shared with WebSocket, consistent counting |
| Global connection semaphore | `Semaphore(2000)` | **Proxy `max_connections` per key** — DB-controlled; fanout's semaphore is a backstop |
| Rate limiting (token bucket) | Deferred in plan | **Proxy `key_rate_limit`** — `rps/rpm/burst_size` per key from DB ✅ (was deferred; now done) |
| `RAII` connection release on disconnect | `ConnectionGuard` in fanout | **`GuardedStream`** in proxy fanout service routes — same pattern |
| Auth disabled fast-path | `FANOUT_AUTH_DISABLED=true` | Used in production (fanout is internal); proxy handles all auth |

### Phase 3 — User Snapshots + WebSocket ⚠️ (partially covered)

| Item | Status | Notes |
|------|--------|-------|
| `GET /snapshot/user/{owner}` | ⚠️ Not in fanout | **Covered by proxy's existing** `GET /state/users/:owner` and `GET /state/balances/:owner` (public routes, proxied from harness). No auth required. |
| `GET /ws/:market` WebSocket | ❌ Not implemented | Proxy's existing WebSocket server (port 50053) streams sequencer ticks. Fanout-specific per-market WS not yet built. |

Phase 3's user snapshot functionality is effectively served by the harness read
path already exposed through the proxy. The per-market WebSocket variant is the
only genuinely missing item.

### Phase 4 — Redis Scale-Out ⚠️ (implemented, not default)

Redis-backed multi-instance fanout now uses Redis Streams for replayable
distribution between fanout instances. Single-instance mode remains the default
for devnet and modest deployments; enable Redis only when you actually need
horizontal fanout capacity.

---

## 9. What the firewall + dual-service model achieves

```
Goal from fanout.md                        Achieved?  How
─────────────────────────────────────────────────────────────────────────────
Zero code changes to execution engine      ✅         Submit/execute path stays untouched;
                                                      only relayer env selects the sink topology

No mutex contention on engine hot path     ✅         Fanout is a separate process;
                                                      engine never knows clients exist

100-1000+ concurrent authenticated readers ✅         Proxy: auth + rate limit + connection cap
                                                      Fanout: broadcast O(1) per event, Arc clone

Per-client rate limiting                   ✅         rps/rpm/burst_size per UUID key in DB
                                                      (better than the deferred token bucket plan)

Snapshot / reconnect for new subscribers   ✅         GET /events/snapshot → GET /events/stream?from=N
                                                      256-event ring bridges the gap

Lag detection for slow clients             ✅         event: resync frame; client re-syncs from snapshot

Single source of truth for auth            ✅         Proxy PG database; one place to issue,
                                                      revoke, and rate-limit keys

TLS + DDoS protection                      ✅         Proxy handles TLS; ddos_middleware on all routes

Internal /ingest not reachable externally  ✅         Firewall blocks port 9094 except from gateway IP;
                                                      /events/ingest blocked in proxy route table

Prometheus metrics                         ✅         Both proxy and fanout expose OpenMetrics

Horizontal scale-out path                  ⚠️         Redis Streams-based fanout is implemented;
                                                      enable only when a single instance is saturated
WebSocket variant                          ❌         Phase 3 — not yet built
```

---

## 10. Remaining tasks before production

1. **Create the systemd unit file** on the backend machine (template in §1).
2. **Set firewall rules** (§4).
3. **Set `CTM_FANOUT_MODE=local` and `CTM_FANOUT_BASE_URL=http://127.0.0.1:9094`** in the relayer environment when you want backend fanout enabled.
4. **Set `FANOUT_ADDR`** in the proxy environment (§3) and rebuild/restart the proxy.
5. **WebSocket variant** (`GET /ws/:market`) — implement in fanout Phase 3 if WS clients are needed.
