# Egress Plan For Harness And Relayer Read Traffic

## Scope

This note evaluates whether the current relayer and Continuum harness read surfaces are suitable for exposing authenticated real-time data to roughly `100-1000+` concurrent users.

The short answer is:

- the relayer/execution-engine is not currently the main problem, because it is not a broad public read API today
- the current harness should not be exposed directly as the public read plane at that scale
- the correct addition is a separate read-optimized fanout tier fed by the authoritative harness/relayer event stream

## Current State

### Execution Engine

The execution engine currently exposes:

- gRPC `submit_intent`
- HTTP `/healthz`
- HTTP `/metrics`

That is a narrow surface. It is not currently acting as a general user-facing REST or SSE service.

### Continuum Harness

The harness is the actual public read surface today. It serves:

- REST state endpoints
- SSE streams
- internal ingestion and reconciliation duties
- periodic on-chain refresh and drift checks

Those responsibilities all sit inside one Node process and share the same event loop.

## Assessment

## What is good

- The system already distinguishes between authoritative execution state and projected read state.
- The Rust backend does cache snapshots.
- Some runtime metrics and latency tracking already exist.
- The relayer itself is not bloated with public read traffic yet.

## What is not sufficient for 100-1000+ concurrent readers

### 1. Single-process coupling

The harness combines:

- HTTP request handling
- SSE fanout
- sequencer/event ingestion
- on-chain reconciliation
- periodic refresh tasks

This means user read load competes with core harness work on the same event loop.

### 2. SSE capacity is explicitly capped low

The configured ceiling for SSE clients is currently `250` by default.

That is already below the target range.

### 3. SSE fanout is not backpressure-aware

The fanout path writes directly to every subscriber synchronously.

Risks:

- a slow client can accumulate kernel/socket buffers
- memory can grow with many slow clients
- event fanout time rises with subscriber count
- fanout work runs on the main process that also handles ingestion and HTTP

### 4. Rust backend reads are serialized

The Rust N-API backend uses a single `Mutex<ContinuumStateEngine>`.

That means:

- only one backend call runs at a time
- concurrent readers do not get true parallelism
- reads and writes contend on the same native engine lock

### 5. Cache invalidation is frequent

The Rust replay engine invalidates cached snapshots whenever state mutates.

Under active traffic, the next read can rebuild a projection under the same serialized engine path.

### 6. Some read endpoints still hit live RPC

Several user-facing endpoints enrich responses with live on-chain reads by default.

At scale, this causes:

- RPC amplification
- more latency variance
- more CPU spent marshaling enriched payloads
- more coupling between public traffic and cluster/provider health

### 7. Large JSON payloads are built on the main thread

Endpoints like full snapshot reads and rich frontend SSE payloads require repeated JSON serialization and object construction in Node.

That becomes expensive under high fanout.

## Conclusion

The current harness design is acceptable for:

- local/dev workflows
- operator tooling
- limited internal consumers
- low to moderate concurrent read traffic

It is not the right design to expose directly to `100-1000+` concurrent public readers if the requirement is:

- minimal impact on core relayer/harness work
- predictable latency
- good memory behavior
- resilient fanout for real-time subscriptions

## Recommended Architecture

Add a separate read plane.

## Target shape

### 1. Keep the current harness/relayer as the authoritative write and compute plane

Responsibilities:

- ingest accepted intents
- process queue updates
- reconcile with on-chain truth
- emit authoritative state-change events

This tier should not be the public fanout tier.

### 2. Add an internal event bus

Publish compact deltas such as:

- relay intent accepted
- relay intent status
- queue item processed
- market state changed
- user/account state changed
- queue watermark changed

Good options:

- `NATS JetStream` for low-latency fanout and replay
- `Redis Streams` if operational simplicity matters more than durability semantics
- `Kafka` only if you expect much larger multi-service data pipelines

For this system, `NATS JetStream` is the cleanest fit.

### 3. Add a read materializer service

This service should:

- consume the internal event stream
- maintain read-optimized market and user projections
- store hot state in memory
- persist shared read state in Redis
- precompute lightweight REST payloads for common queries

This tier should own:

- `/state/users`
- `/state/balances`
- `/state/markets`
- `/state/trades`
- `/state/queue`
- frontend snapshot payloads

It should not perform live RPC on public request paths.

### 4. Add a dedicated realtime fanout tier

This tier should:

- serve SSE or WebSocket to end users
- subscribe to the materializer output or Redis pub/sub topics
- handle per-client buffering and backpressure
- drop or downgrade slow subscribers safely
- scale horizontally behind a load balancer

This is where authentication, quotas, and per-user/topic subscriptions should live.

### 5. Put auth, rate limiting, and caching at the edge

Add an API gateway or reverse proxy layer for:

- bearer token auth
- per-user and per-IP rate limits
- request shaping
- gzip/brotli
- connection quotas
- optional CDN/cache for non-real-time REST reads

## Why this is the optimal addition

This is better than trying to harden the current harness into the public fanout service because it cleanly separates concerns:

- authoritative state computation stays small and deterministic
- public read traffic becomes horizontally scalable
- slow clients no longer threaten ingestion/reconciliation latency
- read payloads can be cached and specialized for UI use cases
- the system can support both REST snapshots and realtime streams without repeated replay work

## Minimal Rollout Plan

### Phase 1

- keep current harness private
- emit authoritative delta events to an internal bus
- build a read materializer that mirrors the current REST schemas

### Phase 2

- move public REST reads to the materializer
- disable live on-chain enrichment on public endpoints
- keep operator-only diagnostics on the private harness

### Phase 3

- introduce a dedicated SSE/WebSocket gateway
- move public stream traffic off the harness entirely
- add backpressure policy and subscriber quotas

### Phase 4

- shrink the private harness surface to internal/operator APIs only
- optionally replace remaining Node read fanout paths with a smaller internal-only control plane

## Practical Recommendation

Do not expose the current harness directly to `1000` users.

Instead:

1. Treat the harness and relayer as authoritative internal services.
2. Publish deltas onto an internal bus.
3. Build a read materializer plus fanout gateway for public traffic.
4. Serve all user REST and realtime subscriptions from that read plane.

If you want the smallest viable step with the highest leverage, build the event bus plus materializer first. That removes most of the scaling risk even before adding the final dedicated fanout tier.

## Code Review

### Findings

- High: The advertised snapshot-then-stream contract can lose events. `dispatch()` broadcasts first and only then updates the snapshot, so a client that reads `/snapshot` in that window and then subscribes with `?from=last_sequence` can miss the already-broadcast event entirely because `broadcast::subscribe()` does not replay old messages.
- High: Multi-instance Redis mode is not correctness-safe yet. It uses plain `PUBLISH`/`PSUBSCRIBE`, so reconnecting instances have no replay path, and on publish failure the code falls back to local-only dispatch, which means one instance advances while the others silently miss the event. That can permanently diverge snapshots and streams across instances.
- Medium: Fanout CPU still scales with subscriber count because each subscriber reserializes the same JSON event independently. The type docs say subscribers get original bytes “with zero re-serialisation cost”, but the SSE path does `serde_json::to_string(&ev.0)` per subscriber per event. That is materially better than the Node harness, but it is still not ideal for `100-1000+` readers.
- Medium: `/stream/:market` creates channels for arbitrary market ids, even when no such market has ever emitted an event. Since `/markets` enumerates channel keys, authenticated clients can create phantom markets and consume memory with bogus subscriptions. `get_sender()` already exists to avoid this, but is not used.
- Medium: The replay model assumes every event has a monotonic `sequence`, but execution-engine status events explicitly allow `sequence: Option<String>`. That means some reject or failure statuses cannot be deduped or gap-checked via `?from=N`, so reconnect semantics are incomplete even aside from the snapshot race.
- Low: `ConnectionGuard` uses a raw metrics pointer plus `unsafe impl Send` where an `Arc<Metrics>` would do. It is probably fine in steady state, but it adds avoidable unsafety to the longest-lived request path.

### Open Questions

- The default devnet wiring still points `CTM_RELAYER_EVENT_SINK_URL` at the harness, not `service-fanout`, so unless deployment has diverged this is not the live read path yet.
- This review assumes the immediate goal is realtime relay-intent fanout only. `service-fanout` is not yet a read materializer; it exposes `/stream`, `/snapshot`, and `/markets`, not the broader public REST or state surface described above.
- `cargo test` in `bin/service-fanout` builds cleanly, but there are `0` tests. The missing coverage matters here because the main remaining risks are races and failure semantics, not compile errors.

### Degree Of Completion

Against the target egress plan, the current work is a solid start on the dedicated realtime fanout tier, but not the full read plane. It is materially better than exposing the Node harness directly, but it should not yet be treated as complete for `100-1000+` public readers.

What is done well:

- separate Rust process from the harness hot path
- async server, auth, per-user and global connection caps, bounded ring buffer, lag and resync signaling, health, and metrics
- an initial horizontal scale-out hook

What is still missing relative to the plan and general best practice:

- a durable internal bus with replay semantics
- a real read materializer for market and user state, and migration of public REST off the harness
- deployment wiring, edge rate limiting and caching, and load or soak testing at target concurrency
- a few correctness fixes before the current fanout service is production-ready
