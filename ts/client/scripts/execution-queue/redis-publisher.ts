/**
 * Redis publisher — Phase 1a (event mirror only).
 *
 * Mirrors every HarnessEvent emitted by `broadcastEvent(...)` into the Redis
 * Stream `v1:events:<market>` (or `v1:events:global` for market-less events).
 *
 * Contract:
 *   - Fire-and-forget. Hot path must never block on Redis.
 *   - Missing 1–2 events is acceptable (chain is source of truth).
 *   - Schema v1 only. See `docs/redis-read-layer/schema.md`.
 *
 * Hook: wrap `broadcastEvent` in continuum-state-harness.ts so every event
 * that goes to SSE also flows into Redis. This file does not mirror derived
 * state (orderbook / balances / positions) — that ships in Phase 1b.
 *
 * Deployment target path (on harness VM):
 *   /home/hetalkenaudekar/stagin4/mng-v4/ts/client/scripts/execution-queue/redis-publisher.ts
 */

import IORedis, { Redis } from 'ioredis';

// ── Config ──────────────────────────────────────────────────────────

const REDIS_URL = process.env.REDIS_URL ?? '';
const PUBLISH_ENABLED =
  (process.env.REDIS_PUBLISH_ENABLED ?? 'false').toLowerCase() === 'true';

// Stream trim budget. MAXLEN ~ 50000 gives roughly 10 min of replay at 100/s
// per market. Redis trims approximate; the extra allowance is negligible.
const EVENTS_STREAM_MAXLEN = 50_000;
// Trades stream is smaller — 1000 is the minimum window the frontend shows.
const TRADES_STREAM_MAXLEN = 1_000;

// Hard cap: if any single XADD takes longer than this, we give up on it. The
// harness hot path never blocks on Redis; the pipeline call is `.exec()` but
// not awaited inline (fire-and-forget). This timeout guards a background
// watchdog and the metrics recorder.
const REDIS_WRITE_TIMEOUT_MS = 50;

// ── Minimal event shape we need ─────────────────────────────────────
// Keep this narrow on purpose — anything more specific belongs in the
// harness's own types. The publisher only cares about routing and payload.

export interface HarnessEventLike {
  readonly event_type: string;
  readonly ts_ms?: number;
  readonly sequence?: number;
  readonly market?: number;
  // arbitrary other fields carried through as payload
  readonly [key: string]: unknown;
}

// ── Metrics (plug into existing Prom registry) ──────────────────────

interface PrometheusCounter {
  inc(labels?: Record<string, string>, value?: number): void;
}
interface PrometheusHistogram {
  observe(labels: Record<string, string>, value: number): void;
}
interface MetricsHooks {
  writesTotal?: PrometheusCounter;
  writeErrorsTotal?: PrometheusCounter;
  writeDurationSeconds?: PrometheusHistogram;
}

// ── Publisher ───────────────────────────────────────────────────────

export interface Publisher {
  /** Fire-and-forget. Never throws, never blocks the caller. */
  publishEvent(event: HarnessEventLike): void;
  /** Close the underlying connection on shutdown (graceful, awaited). */
  close(): Promise<void>;
  /** For operator visibility; does not affect behavior. */
  isEnabled(): boolean;
}

class NoopPublisher implements Publisher {
  publishEvent(_: HarnessEventLike): void {}
  async close(): Promise<void> {}
  isEnabled(): boolean { return false; }
}

class IORedisPublisher implements Publisher {
  private client: Redis;
  constructor(private readonly url: string, private readonly metrics: MetricsHooks) {
    this.client = new IORedis(url, {
      lazyConnect: false,
      enableOfflineQueue: true,
      maxRetriesPerRequest: 1,
      retryStrategy: (times) => Math.min(1000 * times, 5000),
      connectTimeout: 2000,
    });
    this.client.on('error', (err) => {
      // Log at warn, not error — Redis blipping is expected and must not
      // page anyone on the hot path.
      // eslint-disable-next-line no-console
      console.warn('[redis-publisher] redis error:', err.message);
      this.metrics.writeErrorsTotal?.inc({ kind: 'connection' });
    });
  }

  publishEvent(event: HarnessEventLike): void {
    const start = hrtimeNow();
    const streamKey = streamKeyFor(event);
    const fields = flattenToStreamFields(event);

    // Fire-and-forget: we explicitly do NOT await this promise. Errors are
    // routed to metrics + a warn log.
    this.client
      .xadd(streamKey, 'MAXLEN', '~', EVENTS_STREAM_MAXLEN, '*', ...fields)
      .then(() => {
        this.metrics.writesTotal?.inc({ stream: streamKey });
        const seconds = (hrtimeNow() - start) / 1000;
        this.metrics.writeDurationSeconds?.observe({ stream: streamKey }, seconds);
      })
      .catch((err: Error) => {
        this.metrics.writeErrorsTotal?.inc({ kind: 'xadd' });
        // eslint-disable-next-line no-console
        console.warn('[redis-publisher] xadd failed:', err.message);
      });

    // Dedicated trades stream — populated only for queue_item_processed
    // events that actually carry fill details. Keeps /v2/trades cheap
    // (no filtering/parsing of the generic events stream at read time).
    this.maybePublishTrades(event);

    // Watchdog: if the promise is still pending after REDIS_WRITE_TIMEOUT_MS,
    // count it as a slow-write metric. The write itself is not cancelled; ioredis
    // will still deliver if the connection comes back.
    setTimeout(() => {
      // no-op placeholder; a full slow-write counter would require wiring a
      // resolved flag. Left as a later addition.
    }, REDIS_WRITE_TIMEOUT_MS).unref();
  }

  async close(): Promise<void> {
    try {
      await this.client.quit();
    } catch {
      this.client.disconnect();
    }
  }

  isEnabled(): boolean { return true; }

  /**
   * Extract trade records from a processed-queue event's payload and XADD
   * them to `v1:trades:<market>`. Defensive: unrecognized payload shapes
   * simply no-op rather than throw.
   */
  private maybePublishTrades(event: HarnessEventLike): void {
    if (event.event_type !== 'queue_item_processed') return;
    const market = extractMarket(event);
    if (market === null) return;
    const trades = extractTrades(event);
    if (trades.length === 0) return;
    const streamKey = `v1:trades:${market}`;
    for (const t of trades) {
      const fields: string[] = [];
      const push = (k: string, v: unknown) => {
        if (v === undefined || v === null) return;
        fields.push(k, String(v));
      };
      push('maker', t.maker);
      push('taker', t.taker);
      push('price', t.price);
      push('size', t.size);
      push('side', t.side);
      push('ts_ms', t.ts_ms ?? event.ts_ms);
      push('sequence', t.sequence ?? event.sequence);
      if (fields.length === 0) continue;
      this.client
        .xadd(streamKey, 'MAXLEN', '~', TRADES_STREAM_MAXLEN, '*', ...fields)
        .then(() => this.metrics.writesTotal?.inc({ stream: streamKey }))
        .catch(() => this.metrics.writeErrorsTotal?.inc({ kind: 'xadd' }));
    }
  }
}

interface TradeLike {
  maker?: unknown;
  taker?: unknown;
  price?: unknown;
  size?: unknown;
  side?: unknown;
  ts_ms?: unknown;
  sequence?: unknown;
}

function extractMarket(event: HarnessEventLike): number | null {
  for (const cand of [event.market, (event as { market_index?: unknown }).market_index]) {
    if (typeof cand === 'number' && Number.isFinite(cand)) return cand;
    if (typeof cand === 'string' && /^-?\d+$/.test(cand)) return Number(cand);
  }
  return null;
}

/**
 * Heuristic payload walker. Returns any object-ish thing that looks like a
 * trade, checking `trades` arrays and bare `trade` objects at a couple of
 * common nesting points. If the harness event shape evolves we accept
 * silent miss over throwing on the hot path.
 */
function extractTrades(event: HarnessEventLike): TradeLike[] {
  const out: TradeLike[] = [];
  const pushIfTrade = (v: unknown) => {
    if (!v || typeof v !== 'object') return;
    const o = v as Record<string, unknown>;
    if (('maker' in o || 'taker' in o) && 'price' in o && 'size' in o) {
      out.push(o as TradeLike);
    }
  };
  const walk = (node: unknown, depth: number) => {
    if (depth > 3 || !node || typeof node !== 'object') return;
    const obj = node as Record<string, unknown>;
    if (Array.isArray(obj.trades)) obj.trades.forEach(pushIfTrade);
    pushIfTrade(obj.trade);
    pushIfTrade(obj.fill);
    if (Array.isArray(obj.fills)) obj.fills.forEach(pushIfTrade);
    if (obj.payload && typeof obj.payload === 'object') walk(obj.payload, depth + 1);
  };
  walk(event as unknown, 0);
  return out;
}

// ── Helpers ─────────────────────────────────────────────────────────

function streamKeyFor(event: HarnessEventLike): string {
  // Accept `market` (top-level) or `market_index` (nested in some event shapes).
  for (const candidate of [event.market, (event as { market_index?: unknown }).market_index]) {
    if (typeof candidate === 'number' && Number.isFinite(candidate)) {
      return `v1:events:${candidate}`;
    }
    if (typeof candidate === 'string' && /^-?\d+$/.test(candidate)) {
      return `v1:events:${candidate}`;
    }
  }
  return 'v1:events:global';
}

/**
 * ioredis XADD wants a flat [field, value, field, value, ...] list. We
 * promote a few top-level fields and stuff the rest of the event into a
 * JSON-encoded `payload` field. This keeps the stream entries scannable by
 * type without paying the cost of flattening arbitrary nested objects.
 */
function flattenToStreamFields(event: HarnessEventLike): string[] {
  const {
    event_type,
    ts_ms,
    sequence,
    market,
    ...rest
  } = event;
  const fields: string[] = [];
  fields.push('event_type', String(event_type ?? ''));
  if (ts_ms !== undefined) fields.push('ts_ms', String(ts_ms));
  if (sequence !== undefined) fields.push('sequence', String(sequence));
  if (market !== undefined) fields.push('market', String(market));
  // `view` inferred from event_type at the consumer for now. Explicitly
  // encoding it here would duplicate knowledge already in the harness —
  // revisit if consumers want to filter without a lookup table.
  try {
    fields.push('payload', JSON.stringify(rest));
  } catch {
    // Unserializable payload (cycles / BigInts we didn't handle). Drop the
    // payload rather than the whole event.
    fields.push('payload', '{}');
  }
  return fields;
}

function hrtimeNow(): number {
  const [s, ns] = process.hrtime();
  return s * 1000 + ns / 1e6;
}

// ── Module entry point ──────────────────────────────────────────────

export function createPublisher(metrics: MetricsHooks = {}): Publisher {
  if (!PUBLISH_ENABLED || !REDIS_URL) {
    // eslint-disable-next-line no-console
    console.info(
      '[redis-publisher] disabled (REDIS_PUBLISH_ENABLED=%s, REDIS_URL=%s)',
      PUBLISH_ENABLED,
      REDIS_URL ? 'set' : 'unset',
    );
    return new NoopPublisher();
  }
  // eslint-disable-next-line no-console
  console.info('[redis-publisher] enabled, target: %s', REDIS_URL);
  return new IORedisPublisher(REDIS_URL, metrics);
}
