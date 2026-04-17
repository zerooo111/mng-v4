# Fanout Service — Public API Reference

`service-fanout` is a standalone Rust process that receives execution-engine events and fans them out to authenticated external clients via SSE (Server-Sent Events). It is the correct read path for 100–1000+ concurrent real-time subscribers.

Binary: `bin/service-fanout`  
Default bind: `0.0.0.0:9094` (recommended; see [Deployment](#deployment))  
Plan: `fanout.md`

---

## Authentication

All `/stream/`, `/snapshot/`, and `/markets` endpoints require an `Authorization: Bearer <token>` header. Two token types are accepted:

| Type | Format | Notes |
|------|--------|-------|
| API key | opaque string | Pre-shared; loaded from `FANOUT_API_KEYS` env var at startup |
| JWT | HS256 compact | Signed with `FANOUT_JWT_SECRET`; `sub` → user_id, `exp` required |

**API key format** (`FANOUT_API_KEYS` env var — comma-separated):
```
key1:user_alice:pro,key2:user_bob,key3:user_carol:enterprise
```
Fields: `<key>:<user_id>:<tier>`. User ID and tier are optional (defaults to `anonymous` / `free`).

**JWT claims:**
```json
{ "sub": "user_alice", "exp": 1800000000, "tier": "pro" }
```

**Auth disabled** (local dev): set `FANOUT_AUTH_DISABLED=true`.

---

## Endpoints

### POST /ingest

Receives a single event from the execution engine. **Not exposed externally** — blocked at the nginx gateway; called only from localhost.

```
POST /ingest
Content-Type: application/json
X-Ingest-Secret: <FANOUT_INGEST_SECRET>   (if configured)

{ "event_type": "relay_intent_accepted", "market": "<pubkey>", "sequence": "42", "ts_ms": "1700000000000", ... }
```

Responses: `200 OK`, `400 Bad Request` (missing market field), `401 Unauthorized` (wrong ingest secret).

---

### POST /ingest/batch

Same as `/ingest` but accepts a JSON array. Processes all events atomically in a single lock pass per market.

```
POST /ingest/batch
Content-Type: application/json

[ { "event_type": "...", "market": "<pubkey>", ... }, ... ]
```

---

### GET /stream/{market}

SSE stream of real-time events for a given market.

**Path parameter:** `{market}` — market pubkey string (e.g. `AscEq4AL6sBjag3P3RUUNyDV5J7p9Hcx5Dr8PhPNXrDT`).

**Query parameter:** `?from=<sequence>` — optional watermark. Events with `sequence ≤ from` are skipped. Used after fetching a snapshot to avoid replaying already-seen data.

**Recommended client flow:**
```
1. GET /snapshot/{market}           → note last_sequence: N
2. GET /stream/{market}?from=N      → receive events with sequence > N
3. On event: resync                 → re-do steps 1 and 2
```

**Response:** `Content-Type: text/event-stream`

```
event: event
data: {"event_type":"relay_intent_accepted","market":"Asc...","sequence":"43","ts_ms":"1700000001000",...}

event: event
data: {"event_type":"relay_intent_status","market":"Asc...","sequence":"44","ts_ms":"1700000002000",...}

event: resync
data: lagged_by=120
```

| SSE event name | Meaning |
|----------------|---------|
| `event` | Normal execution-engine event. JSON payload is the raw envelope. |
| `resync` | Client fell behind the 4096-event ring buffer. Re-fetch snapshot and re-subscribe. |

**Error responses:**
- `401 Unauthorized` — missing/invalid token
- `404 Not Found` — market not registered (no events seen yet; try again shortly after engine startup)
- `429 Too Many Requests` — per-user connection limit reached (default: 10)
- `503 Service Unavailable` — global connection limit reached (default: 2000)

---

### GET /snapshot/{market}

Returns the current market snapshot (last N events seen) as JSON. No streaming.

**Path parameter:** `{market}` — market pubkey.

**Response:** `200 OK`, `Content-Type: application/json`

```json
{
  "market": "AscEq4AL6sBjag3P3RUUNyDV5J7p9Hcx5Dr8PhPNXrDT",
  "last_sequence": 44,
  "updated_at_ms": 1700000002000,
  "event_count": 44,
  "recent_events": [
    { "event_type": "relay_intent_accepted", "market": "Asc...", "sequence": "43", ... },
    { "event_type": "relay_intent_status",   "market": "Asc...", "sequence": "44", ... }
  ]
}
```

`recent_events` contains up to 256 most recent events (configurable via `FANOUT_SNAPSHOT_MAX_EVENTS`). Use `last_sequence` as the `from=` parameter when opening an SSE stream.

**Error responses:** `401 Unauthorized`, `404 Not Found` (no events seen for market yet).

---

### GET /markets

Returns a sorted list of all market keys that have received at least one event.

**Response:** `200 OK`, `Content-Type: application/json`

```json
["AscEq4AL6sBjag3P3RUUNyDV5J7p9Hcx5Dr8PhPNXrDT", "..."]
```

---

### GET /healthz

Liveness check. No auth required.

**Response:** `200 OK`

```json
{ "ok": true }
```

---

### GET /metrics

Prometheus / OpenMetrics text format. No auth required.

```
# HELP fanout_events_ingested_total Total events received on /ingest
# TYPE fanout_events_ingested_total counter
fanout_events_ingested_total 12345

# HELP fanout_active_subscribers Current live SSE connections
# TYPE fanout_active_subscribers gauge
fanout_active_subscribers 47

# HELP fanout_lagged_events_total SSE subscribers that fell behind the ring buffer
# TYPE fanout_lagged_events_total counter
fanout_lagged_events_total 3

# HELP fanout_connections_rejected_total Connections rejected (semaphore or per-user limit)
# TYPE fanout_connections_rejected_total counter
fanout_connections_rejected_total 0

# HELP fanout_auth_failures_total Failed authentication attempts
# TYPE fanout_auth_failures_total counter
fanout_auth_failures_total 0

# HELP fanout_ingest_errors_total Malformed or rejected ingest payloads
# TYPE fanout_ingest_errors_total counter
fanout_ingest_errors_total 0

# HELP fanout_events_no_subscriber_total Events ingested with no active subscribers
# TYPE fanout_events_no_subscriber_total counter
fanout_events_no_subscriber_total 0
```

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FANOUT_BIND_ADDR` | `0.0.0.0:9094` | TCP listen address |
| `FANOUT_CHANNEL_CAPACITY` | `4096` | Broadcast ring size per market |
| `FANOUT_SNAPSHOT_MAX_EVENTS` | `256` | Recent-event ring per snapshot |
| `FANOUT_MAX_CONNECTIONS` | `2000` | Global SSE connection cap |
| `FANOUT_MAX_CONNECTIONS_PER_USER` | `10` | Per-user SSE connection cap |
| `FANOUT_AUTH_DISABLED` | `false` | Skip auth (local dev only) |
| `FANOUT_API_KEYS` | *(none)* | `key[:user[:tier]],...` |
| `FANOUT_JWT_SECRET` | *(none)* | HMAC-SHA256 secret for JWT auth |
| `FANOUT_INGEST_SECRET` | *(none)* | `X-Ingest-Secret` header value for `/ingest` |
| `RUST_LOG` | `info` | Tracing filter |

---

## Event Envelope Schema

All events are opaque `serde_json::Value` wrappers. The fanout service reads only three fields for routing and filtering; the rest is passed through verbatim.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `event_type` | string | no | e.g. `relay_intent_accepted`, `relay_intent_status` |
| `market` | string | **yes** | Market index as decimal string — used as the channel key for fanout routing. Values are `"0"`, `"1"`, `"2"`, etc. (not a pubkey). |
| `sequence` | string (decimal) | no | Monotonic per-market counter; used for `?from=` deduplication |
| `ts_ms` | number | no | Event timestamp in milliseconds |
| `group` | string | no | The group/program pubkey (e.g. `AscEq4AL6sBjag3P3RUUNyDV5J7p9Hcx5Dr8PhPNXrDT`) |

Any additional fields in the JSON object are forwarded to subscribers unchanged.

---

## Deployment

### Recommended port

Port 9094 — consistent with the devnet stack (`9091` harness, `9092` bridge, `9093` relayer).

Set `FANOUT_BIND_ADDR=0.0.0.0:9094` in the environment file.

### systemd unit

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
Environment=FANOUT_BIND_ADDR=0.0.0.0:9094
Environment=FANOUT_INGEST_SECRET=<shared-secret>
Environment=FANOUT_JWT_SECRET=<jwt-secret>
Environment=FANOUT_API_KEYS=<key1>:<user1>:<tier>,...
ExecStart=/home/hetalkenaudekar/mng-v4/bin/service-fanout/target/release/service-fanout
Restart=on-failure
RestartSec=5
StandardOutput=append:/home/hetalkenaudekar/mng-v4/.devnet/logs/service-fanout.log
StandardError=append:/home/hetalkenaudekar/mng-v4/.devnet/logs/service-fanout.log

[Install]
WantedBy=stagin4-devnet.target
```

### nginx location blocks (add to `fermi-devnet-routes.conf`)

See `gateway-wiring.md` for the full nginx integration plan.
