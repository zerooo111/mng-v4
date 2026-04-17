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
                  │                                               │
                  │  fanout-relay task ◄── /state/stream (SSE) ───┼──► BACKEND :9091 (harness)
                  │       │ POST /ingest ──────────────────────── ┼──► BACKEND :9094/ingest
                  └───────────────────────────────────────────────┘

                      BACKEND MACHINE  (this machine)
                  ┌───────────────────────────────────────────────┐
                  │  service-fanout  :9094                        │
                  │                                               │
                  │  POST /ingest ◄── from proxy relay task       │
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
port 9094**. Port 9091 (harness) remains accessible to the proxy for the relay task.

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
# Set the same value in the proxy's FANOUT_INGEST_SECRET.
Environment=FANOUT_INGEST_SECRET=<shared-ingest-secret>

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

## 2. Event pipeline — wiring harness → fanout

The execution engine sends events to the harness at
`http://127.0.0.1:9091/ingest/relay-intent`. The harness processes them for
on-chain state. The fanout service needs a copy of the same events so it can
broadcast them to SSE subscribers.

**Recommended: add a fanout relay task to `continuum-proxy-rust`.**

The proxy already has an identical pattern in `src/trade_ingest.rs`
(subscribes to `/state/stream/trades`, parses events, inserts to TimescaleDB).
A new `src/fanout_relay.rs` module follows the same shape:

1. Proxy connects to `STATE_HARNESS_ADDR/state/stream` (SSE, long-lived).
2. Each SSE `event: event` frame is forwarded via `POST FANOUT_ADDR/ingest`
   with header `X-Ingest-Secret: <FANOUT_INGEST_SECRET>`.
3. On upstream disconnect, back off and reconnect (same as `trade_ingest.rs`).

```rust
// src/fanout_relay.rs (sketch — mirrors trade_ingest.rs structure)
pub async fn run(http_client: reqwest::Client, harness_base: String, fanout_base: String, ingest_secret: Option<String>) {
    loop {
        if let Err(e) = connect_and_relay(&http_client, &harness_base, &fanout_base, &ingest_secret).await {
            warn!("Fanout relay error: {e}, reconnecting...");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn connect_and_relay(...) -> Result<()> {
    // GET {harness_base}/state/stream (SSE, accept: text/event-stream)
    // For each "data:" line received:
    //   POST {fanout_base}/ingest  with the raw JSON body
    //   X-Ingest-Secret: {secret}  (if configured)
    //   fire-and-forget — do not await the response or stall the stream
}
```

> **Note on event schema compatibility:** The fanout service requires each
> ingest payload to have a `"market"` field (market pubkey string) for routing.
> Verify that `/state/stream` events from the harness include this field before
> wiring. If the harness emits a different envelope, add a thin mapping step in
> `connect_and_relay` before forwarding.

Add to `src/main.rs`:
```rust
// Fanout relay — forward harness SSE events to fanout /ingest
if !args.fanout_addr.is_empty() {
    let relay_client = http_client.client.clone();
    let relay_harness = args.state_harness_addr.clone();
    let relay_fanout  = args.fanout_addr.clone();
    let relay_secret  = std::env::var("FANOUT_INGEST_SECRET").ok();
    tokio::spawn(async move {
        fanout_relay::run(relay_client, relay_harness, relay_fanout, relay_secret).await;
    });
}
```

---

## 3. Configure `continuum-proxy-rust` environment

Add to the proxy's `.env` or deployment config:

```bash
# Backend machine IP and fanout port
FANOUT_ADDR=http://<backend-machine-ip>:9094

# Must match FANOUT_INGEST_SECRET on the backend
FANOUT_INGEST_SECRET=<shared-ingest-secret>
```

Restart the proxy after setting these. The fanout relay task starts automatically
when `FANOUT_ADDR` is non-empty.

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
- External clients **cannot** reach `/ingest` directly — they must go through the proxy.
- The proxy can call `/ingest` (relay) and `/stream/:market`, `/snapshot/:market`, `/markets`.
- The execution engine calls the harness on loopback (unchanged).

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

### Phase 4 — Redis Scale-Out ⬜ (not needed yet)

Not implemented. Single fanout instance handles up to ~2000 concurrent SSE
connections (50 MB memory). Implement when that limit is approached.

---

## 9. What the firewall + dual-service model achieves

```
Goal from fanout.md                        Achieved?  How
─────────────────────────────────────────────────────────────────────────────
Zero code changes to execution engine      ✅         CTM_RELAYER_EVENT_SINK_URL unchanged;
                                                      relay task in proxy reads harness SSE

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

Horizontal scale-out path                  ⬜         Phase 4 Redis — not needed yet
WebSocket variant                          ❌         Phase 3 — not yet built
```

---

## 10. Remaining tasks before production

1. ~~**Implement `fanout_relay.rs` in the proxy**~~ ✅ Done — `src/fanout_relay.rs` written, wired into `main.rs`.
2. ~~**Verify harness SSE event schema**~~ ✅ Done — confirmed `"market"` is an integer index string (`"0"`, `"1"`, `"2"`); relay pre-filters on `"market"` key presence.
3. **Create the systemd unit file** on the backend machine (template in §1).
4. **Set firewall rules** (§4).
5. **Set `FANOUT_ADDR` and `FANOUT_INGEST_SECRET`** in the proxy environment (§3) and rebuild/restart the proxy.
6. **WebSocket variant** (`GET /ws/:market`) — implement in fanout Phase 3 if WS clients are needed.
