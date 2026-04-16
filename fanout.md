# Fanout Service — Architectural Plan

## Problem Statement

The execution engine (`bin/service-mango-execution-engine`) is a high-throughput gRPC write path. It exposes no SSE or real-time read API, has a single `event_sink_url` (one consumer only), and shares the `ContinuumStateEngine` mutex between the submit hot path and any state reads. Serving 100-1000+ concurrent authenticated readers directly from the engine would:

- Contend with margin-check/sign hot path on the `parking_lot::Mutex<ContinuumStateEngine>`
- Saturate the bg_submit worker pool (16 tokio tasks) with SSE keepalives
- Deliver events to only one downstream consumer (current sink design)
- Provide no snapshot/reconnect primitive for new subscribers

The execution engine requires **zero code changes**. Only `event_sink_url` is pointed at the new fanout service.

---

## Target Architecture

```
                  ┌──────────────────────────────────────────────┐
                  │          EXECUTION ENGINE (unchanged)         │
[Client] -gRPC -> │  submit_intent -> sign -> mpsc -> bg_workers  │
                  │              |                                │
                  │      event_sink POST (fire-and-forget)        │
                  └──────────────┬───────────────────────────────┘
                                 │  HTTP POST /ingest
                                 ▼
                  ┌──────────────────────────────────────────────┐
                  │          FANOUT SERVICE  (new binary)         │
                  │                                              │
                  │  POST /ingest  <── from execution engine     │
                  │                                              │
                  │  broadcast::channel(4096) per market shard   │
                  │        |            |            |           │
                  │   [market 0]   [market 1]   [market N]       │
                  └──────┬───────────────────────────────────────┘
                         │
           ┌─────────────┼──────────────────┐
           ▼             ▼                  ▼
    [SSE /stream]  [WS /ws/stream]  [REST /snapshot]
           │             │                  │
       auth middleware (JWT / API key)
           │             │                  │
  ┌────────▼─────────────▼──────────────────▼────────┐
  │          100–1000+ authenticated clients           │
  └───────────────────────────────────────────────────┘
```

---

## Components

### 1. `bin/service-fanout` — New Rust Binary

New crate at `bin/service-fanout/`. Axum + Tokio. Completely separate process with its own tokio runtime, memory, and CPU budget.

**Endpoints:**

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/ingest` | Receives events from execution engine `event_sink_url` |
| GET | `/stream/{market_index}` | SSE stream of real-time events, authenticated |
| GET | `/ws/{market_index}` | WebSocket stream (optional, same data) |
| GET | `/snapshot/{market_index}` | Current order book / market state snapshot |
| GET | `/snapshot/user/{owner}` | User position snapshot (TTL-cached from harness) |
| GET | `/healthz` | Health check |
| GET | `/metrics` | Prometheus metrics |

**Core data structure — per-market broadcast channel:**

```rust
// One channel per market index. O(1) send delivers to all N subscribers.
let (tx, _) = tokio::sync::broadcast::channel::<Arc<EventEnvelope>>(4096);

// Global registry
type MarketChannels = Arc<DashMap<u16, broadcast::Sender<Arc<EventEnvelope>>>>;
```

**Fanout on ingest:**

```rust
// POST /ingest handler — called by execution engine
async fn ingest(
    State(channels): State<MarketChannels>,
    Json(event): Json<EventEnvelope>,
) {
    let ev = Arc::new(event);
    if let Some(tx) = channels.get(&ev.market_index) {
        // O(1): one send, all N subscribers receive a clone of the Arc
        let _ = tx.send(Arc::clone(&ev));
    }
    // Also update snapshot cache (see §2)
}
```

**SSE subscriber:**

```rust
async fn sse_stream(
    Path(market_index): Path<u16>,
    State(state): State<AppState>,
    auth: AuthClaims,            // rejected by middleware if invalid
) -> impl IntoResponse {
    let mut rx = state.channels
        .get(&market_index)
        .map(|tx| tx.subscribe())
        .ok_or(StatusCode::NOT_FOUND)?;

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(ev) => yield Ok(Event::default().json_data(&*ev).unwrap()),
                Err(RecvError::Lagged(n)) => {
                    // Client fell behind — tell it to re-fetch snapshot
                    yield Ok(Event::default().event("resync").data(n.to_string()));
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
```

Key properties:
- `Arc<EventEnvelope>` — zero-copy; all N subscribers share one heap allocation per event
- `broadcast::channel(4096)` — 4096-event ring per market; slow clients get `Lagged`, not a stalled publisher
- `Lagged` → resync signal → client re-GETs `/snapshot` then re-subscribes; no data gap possible if snapshot covers the lag window

---

### 2. Snapshot Cache

In-memory snapshot per market, updated by the ingest path. Readers never touch the execution engine or harness.

```rust
#[derive(Clone, Serialize)]
struct MarketSnapshot {
    market_index: u16,
    order_book: OrderBook,        // bids + asks
    last_sequence: u64,           // monotonic, matches engine sequence
    updated_at_ms: u64,
}

// Many concurrent readers, single ingest writer per market
type SnapshotCache = Arc<DashMap<u16, Arc<RwLock<MarketSnapshot>>>>;
```

New client connection flow:
1. `GET /snapshot/{market}` → returns `MarketSnapshot` + `last_sequence: N`
2. `GET /stream/{market}?from=N` → receives all events with `sequence > N`
3. Broadcast channel buffer (4096 events) covers the window between step 1 and step 2

The `from=N` filter in the SSE handler drops already-seen events from the channel buffer, preventing replay:

```rust
// In sse_stream: skip events already covered by snapshot
while let Ok(ev) = rx.recv().await {
    if ev.sequence > from_sequence {
        yield ev;
    }
}
```

User snapshot (`/snapshot/user/{owner}`) is fetched from harness on first request and TTL-cached (configurable, suggested 500ms). Cache is a `moka::Cache` with async loader.

---

### 3. Authentication Middleware

Auth lives entirely in the fanout service — the execution engine is unaffected.

```rust
// Axum extractor — rejects request before handler is called
struct AuthClaims { user_id: UserId, tier: Tier }

impl<S> FromRequestParts<S> for AuthClaims {
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, StatusCode> {
        let token = extract_bearer(parts)?;
        verify_jwt(token, &state.jwks).await   // cached JWKS, O(1) verify
    }
}
```

Additional controls in the fanout service:

| Control | Implementation | Purpose |
|---------|---------------|---------|
| JWT / API key verification | Axum extractor, cached JWKS | Identity |
| Per-user connection limit | `DashMap<UserId, AtomicU32>` | Prevent single-user DoS |
| Global connection limit | `tokio::sync::Semaphore` | Memory ceiling |
| Per-tier rate limit | Token bucket per `(user_id, market)` | Fair bandwidth |

---

### 4. Optional: Redis Pub/Sub for Horizontal Scale

If a single fanout service instance is insufficient (>~2000 concurrent SSE connections is the practical limit per instance on modest hardware):

```
execution engine
      |
      POST /ingest
      |
      ▼
[Redis PUBLISH market:{index}]
      |
      ├── fanout-service-0 (SUBSCRIBE) --> ~700 SSE clients
      ├── fanout-service-1 (SUBSCRIBE) --> ~700 SSE clients
      └── fanout-service-2 (SUBSCRIBE) --> ~700 SSE clients
```

Each instance subscribes to all market channels via Redis Pub/Sub. The execution engine POSTs to a single `/ingest` endpoint (any instance, or a load balancer VIP) which publishes to Redis; all instances fan out locally to their connected clients.

Redis adds ~100-200µs end-to-end latency. Use only if a single instance is saturated. For 100-1000 users the single-instance design is sufficient.

**Redis dependency is optional** — the fanout service starts in single-instance mode by default and enables Redis fanout via config flag `FANOUT_REDIS_URL`.

---

### 5. Execution Engine Config Change (only required change)

```bash
# Before
CTM_EVENT_SINK_URL=http://some-single-consumer/events

# After
CTM_EVENT_SINK_URL=http://fanout-service:8080/ingest
```

No source changes to the execution engine.

---

## Why Not Modify the Execution Engine

| Approach | Problem |
|----------|---------|
| Add SSE to execution engine | SSE keepalive timers + 1000 open connections consume tokio workers shared with bg_submit pool (16 workers) |
| Add `/state` REST to engine | Every read acquires `ContinuumStateEngine` mutex — same lock submit-path holds for margin checks |
| Add broadcast channel inside engine | Channel memory lives inside hot-path process; slow consumer risks exhausting channel buffer; adds latency variance to the publisher |
| Fan out from existing `event_sink_url` | Single URL — cannot deliver to N consumers without an intermediary |

The engine's hot path must never block on external consumers. The fanout service provides the required isolation.

---

## Implementation Plan

### Phase 1 — Core Fanout (single instance, SSE only)

- [ ] Create `bin/service-fanout/` crate, add to workspace `Cargo.toml`
- [ ] Add dependencies: `axum`, `tokio`, `serde_json`, `dashmap`, `async-stream`, `axum-extra` (SSE)
- [ ] Implement `EventEnvelope` type mirroring execution engine's `RelayIntentStatusEvent` / `RelayIntentAcceptedEvent` JSON shapes
- [ ] Implement `MarketChannels` registry with `DashMap<u16, broadcast::Sender<Arc<EventEnvelope>>>`
- [ ] Implement `POST /ingest` handler — deserialize, broadcast, update snapshot cache
- [ ] Implement `GET /stream/{market_index}` SSE handler with lag detection + resync event
- [ ] Implement `GET /snapshot/{market_index}` REST handler
- [ ] Implement `/healthz` and `/metrics` (Prometheus counters: events_ingested, active_subscribers, lagged_events)
- [ ] Wire up Axum router + Tokio multi-thread runtime
- [ ] Dockerfile + systemd unit

### Phase 2 — Auth + Connection Controls

- [ ] JWT middleware (Axum extractor, async JWKS fetch + cache)
- [ ] API key middleware (alternative to JWT, same extractor trait)
- [ ] Per-user connection counter (`DashMap<UserId, AtomicU32>`)
- [ ] Global connection semaphore
- [ ] Per-tier rate limiting (token bucket)

### Phase 3 — User Snapshots + Harness Proxy

- [ ] `GET /snapshot/user/{owner}` — async fetch from harness, `moka::Cache` with TTL
- [ ] `GET /ws/{market_index}` — WebSocket variant of the SSE stream

### Phase 4 — Redis Scale-Out (if needed)

- [ ] Add `redis-rs` (async) dependency, feature-flagged
- [ ] On startup: if `FANOUT_REDIS_URL` set, subscribe to `market:*` channels
- [ ] Replace local broadcast send with Redis PUBLISH in `/ingest`
- [ ] Each instance subscribes and drives its local broadcast channels from Redis messages
- [ ] Update deployment: N instances behind load balancer, sticky sessions not required (all instances carry all market channels)

---

## Resource Budget (single instance, 1000 clients)

| Resource | Estimate | Notes |
|----------|----------|-------|
| Memory per SSE connection | ~50 KB | Axum connection state + broadcast receiver |
| Memory for 1000 connections | ~50 MB | Well within typical 512 MB container |
| CPU for broadcast fanout | O(1) per event | `Arc::clone` × N, no serialization per subscriber |
| Latency added (ingest → SSE delivery) | <1 ms | tokio task wakeup, no disk/network on hot path |
| Latency added (ingest → Redis → SSE) | ~1-2 ms | Redis round-trip if scale-out enabled |
| Events per second budget | >50,000 | Typical tokio broadcast throughput |

---

## Files to Create

```
bin/service-fanout/
  Cargo.toml
  src/
    main.rs          -- runtime, config, router wiring
    ingest.rs        -- POST /ingest handler + broadcast dispatch
    sse.rs           -- GET /stream/{market} SSE handler
    snapshot.rs      -- GET /snapshot/{market} and /snapshot/user/{owner}
    auth.rs          -- JWT + API key Axum extractors
    channels.rs      -- MarketChannels registry, broadcast channel lifecycle
    metrics.rs       -- Prometheus counters + /metrics endpoint
    redis.rs         -- Optional Redis Pub/Sub subscriber (Phase 4)
    types.rs         -- EventEnvelope, MarketSnapshot, config structs
```
