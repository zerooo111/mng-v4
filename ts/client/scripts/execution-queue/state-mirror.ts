/**
 * Redis state mirror — Phase 1b.
 *
 * Mirrors derived engine state (orderbook / user positions / balance summary)
 * into Redis after every burst of harness events. Coalesces bursts with a
 * trailing debounce so a 50-event tick produces one Redis write pass.
 *
 * Contract (same as Phase 1a publisher):
 *   - Fire-and-forget. Hot path must never block on Redis.
 *   - Missing 1–2 frames is acceptable (chain is source of truth).
 *   - Schema v1 only. See `docs/redis-read-layer/schema.md`.
 *
 * Target path on harness VM:
 *   /home/hetalkenaudekar/stagin4/mng-v4/ts/client/scripts/execution-queue/state-mirror.ts
 */

import IORedis, { Redis } from 'ioredis';

import { recordMirrorTick } from './metrics';

// ── Config ──────────────────────────────────────────────────────────

const REDIS_URL = process.env.REDIS_URL ?? '';
const MIRROR_ENABLED =
  (process.env.REDIS_MIRROR_ENABLED ?? 'false').toLowerCase() === 'true';

// Trailing-debounce window. Bigger = fewer Redis writes but staler snapshots.
// 100ms = ~10 writes/s max per process. Halves mirror CPU vs 50ms; users
// don't notice 100ms of snapshot staleness on the read path. Trade latency
// is protected by keeping the hot path fire-and-forget regardless.
const MIRROR_DEBOUNCE_MS = 100;

// Safety cap on orders mirrored per market per view. Book depth > this is
// truncated from the tail (lowest-priority side). Protects against runaway
// books bloating Redis.
const MIRROR_MAX_ORDERS_PER_MARKET = 500;

// Tick-notification streams. Consumers (continuum-proxy-rust, others) that
// want push semantics instead of polling the book/meta HASHes can
// `XREAD BLOCK 0` one of these streams and — on any new entry — fetch the
// current hash state. Entries are small (just ts_ms + tag) so the stream
// cost is negligible; MAXLEN keeps memory bounded. This decouples cadence
// from consumer poll intervals and was the reason for the v2 proxy's
// observed ~15× fewer meta updates vs v1 (polling at 2000ms vs harness
// push at ~17ms).
const TICK_STREAM_MAXLEN = 100;

// ── Types (structural subset of EngineSnapshot) ─────────────────────

type ViewTag = 'opt' | 'conf';

interface OpenOrderLike {
  readonly order_id: string;
  readonly owner: string;
  readonly market: string;
  readonly side: 'bid' | 'ask';
  readonly price_lots: string;
  readonly base_lots: string;
  readonly client_order_id?: string;
}

/**
 * Rich, slow-changing identity fields carried on the harness's own Market
 * record (mirrors the shape returned by `GET /state/markets/:id.metadata`).
 * Every field optional so the mirror stays resilient to harness upgrades.
 */
interface MarketMetadataLike {
  readonly market_index?: number | string;
  readonly name?: string;
  readonly base_symbol?: string;
  readonly quote_symbol?: string;
  readonly base_mint?: string;
  readonly quote_mint?: string;
  readonly perp_market?: string;
  readonly oracle?: string;
  readonly bids?: string;
  readonly asks?: string;
  readonly event_queue?: string;
  readonly base_decimals?: number | string;
  readonly quote_decimals?: number | string;
  readonly base_lot_size?: number | string;
  readonly quote_lot_size?: number | string;
  readonly open_interest?: number | string;
}

interface MarketLike {
  readonly market: string;
  readonly open_orders?: OpenOrderLike[];
  readonly metadata?: MarketMetadataLike;
}

interface PerMarketStateLike {
  readonly market: string;
  readonly base_position_lots: string;
  readonly quote_position_native: string;
  readonly open_order_base_lots_bid: string;
  readonly open_order_base_lots_ask: string;
  readonly quote_reserved_lots: string;
}

interface UserLike {
  readonly owner: string;
  readonly per_market?: PerMarketStateLike[];
  readonly margin_summary?: unknown;
  // Per-asset token balances as computed by the harness's optimistic
  // collateral pass. When present, mirrored into v1:balance:<owner> alongside
  // margin_summary so the frontend's vault / wallet-balance panels can read
  // them without a separate /state/balances HTTP hop. Optional so the mirror
  // stays compatible with harness builds that don't compute this field yet.
  readonly optimistic_collateral?: unknown;
  // Aggregate reserves (total_quote_reserved_lots, etc.) — used by v1's
  // /state/balances composer to report per-mint reserved amounts. Mirrored
  // verbatim so the gateway can expose the same derivation.
  readonly totals?: unknown;
}

interface PerpMarketSyncLike {
  readonly market?: string;
  readonly market_index?: number | string;
  readonly oracle_price?: string;
  readonly stable_price?: string;
  readonly base_lot_size?: string;
  readonly quote_lot_size?: string;
  readonly long_funding?: string;
  readonly short_funding?: string;
  // Computed metrics attached by the harness `enrichedPerp` block in
  // continuum-state-harness.ts. All optional so the mirror stays compatible
  // with harness builds that haven't shipped the enrichment yet.
  readonly mark_price?: number | string | null;
  readonly funding_rate_daily?: number | string | null;
  readonly funding_rate_hourly?: number | string | null;
  readonly maker_fee?: string;
  readonly taker_fee?: string;
  readonly settle_token_index?: number | string;
}

export interface EngineSnapshotLike {
  readonly view: string;
  readonly markets?: Record<string, MarketLike>;
  readonly users?: Record<string, UserLike>;
  readonly perp_markets?: Record<string, PerpMarketSyncLike>;
  readonly generated_ts_ms?: number;
}

export type SnapshotProvider = (view: 'optimistic' | 'confirmed') => EngineSnapshotLike | null | undefined;

// ── Mirror interface ────────────────────────────────────────────────

export interface Mirror {
  /** Schedule a debounced mirror tick. Never throws, never blocks. */
  onEvent(): void;
  close(): Promise<void>;
  isEnabled(): boolean;
}

class NoopMirror implements Mirror {
  onEvent(): void {}
  async close(): Promise<void> {}
  isEnabled(): boolean { return false; }
}

class IORedisMirror implements Mirror {
  private client: Redis;
  private timer: NodeJS.Timeout | null = null;
  private inFlight = false;
  private pendingWhileInFlight = false;

  constructor(
    url: string,
    private readonly getSnapshot: SnapshotProvider,
  ) {
    this.client = new IORedis(url, {
      lazyConnect: false,
      enableOfflineQueue: true,
      maxRetriesPerRequest: 1,
      retryStrategy: (times) => Math.min(1000 * times, 5000),
      connectTimeout: 2000,
    });
    this.client.on('error', (err) => {
      // eslint-disable-next-line no-console
      console.warn('[state-mirror] redis error:', err.message);
    });
  }

  onEvent(): void {
    if (this.inFlight) {
      this.pendingWhileInFlight = true;
      return;
    }
    if (this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.tick().catch((err: Error) => {
        // eslint-disable-next-line no-console
        console.warn('[state-mirror] tick failed:', err.message);
      });
    }, MIRROR_DEBOUNCE_MS);
    // Don't keep the event loop alive purely for a mirror tick.
    this.timer.unref();
  }

  private async tick(): Promise<void> {
    this.inFlight = true;
    try {
      for (const [view, tag] of [
        ['optimistic', 'opt'] as const,
        ['confirmed', 'conf'] as const,
      ]) {
        const started = process.hrtime.bigint();
        let ok = true;
        let snap: EngineSnapshotLike | null | undefined;
        try {
          snap = this.getSnapshot(view);
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn(`[state-mirror] getSnapshot(${view}) threw:`, (err as Error).message);
          recordMirrorTick(tag, 0, false);
          continue;
        }
        if (!snap) continue;
        try {
          await this.writeSnapshot(tag, snap);
        } catch (err) {
          ok = false;
          // eslint-disable-next-line no-console
          console.warn(`[state-mirror] writeSnapshot(${tag}) failed:`, (err as Error).message);
        }
        const durSec = Number(process.hrtime.bigint() - started) / 1e9;
        recordMirrorTick(tag, durSec, ok);
      }
    } finally {
      this.inFlight = false;
      if (this.pendingWhileInFlight) {
        this.pendingWhileInFlight = false;
        this.onEvent();
      }
    }
  }

  private async writeSnapshot(tag: ViewTag, snap: EngineSnapshotLike): Promise<void> {
    const pipeline = this.client.pipeline();
    const tsMs = String(snap.generated_ts_ms ?? Date.now());

    // ── Orderbook + per-owner orders index ──
    // The orders index (`v1:orders:<view>:<owner>`) is rebuilt each tick
    // from the current open_orders list. Stale owners aren't explicitly
    // cleared — their keys fall out when the mirror is rebuilt on next
    // startup. (Sparse per-owner keys are cheap.)
    const ordersByOwner = new Map<string, Map<string, string>>(); // owner → (order_id → market_id)
    const bookTickedMarkets: string[] = [];
    if (snap.markets) {
      for (const [marketKey, market] of Object.entries(snap.markets)) {
        const bidsKey = `v1:book:${tag}:${marketKey}:bids`;
        const asksKey = `v1:book:${tag}:${marketKey}:asks`;
        const ordersKey = `v1:book:${tag}:${marketKey}:orders`;

        pipeline.del(bidsKey, asksKey, ordersKey);
        bookTickedMarkets.push(marketKey);

        const orders = market.open_orders ?? [];
        const trimmed = orders.slice(0, MIRROR_MAX_ORDERS_PER_MARKET);

        for (const ord of trimmed) {
          const score = Number(ord.price_lots);
          if (!Number.isFinite(score)) continue;

          const orderJson = JSON.stringify({
            owner: ord.owner,
            price: ord.price_lots,
            size: ord.base_lots,
            side: ord.side,
            ts_ms: snap.generated_ts_ms ?? null,
            client_id: ord.client_order_id ?? null,
          });

          if (ord.side === 'bid') {
            pipeline.zadd(bidsKey, score, ord.order_id);
          } else {
            pipeline.zadd(asksKey, score, ord.order_id);
          }
          pipeline.hset(ordersKey, ord.order_id, orderJson);

          let owned = ordersByOwner.get(ord.owner);
          if (!owned) {
            owned = new Map();
            ordersByOwner.set(ord.owner, owned);
          }
          owned.set(ord.order_id, marketKey);
        }
      }
    }

    for (const [owner, orderMap] of ordersByOwner) {
      const key = `v1:orders:${tag}:${owner}`;
      pipeline.del(key);
      const obj: Record<string, string> = {};
      for (const [oid, mkt] of orderMap) {
        obj[oid] = mkt;
      }
      if (Object.keys(obj).length > 0) pipeline.hset(key, obj);
    }

    // Book-tick notification streams. One XADD per market per tick so
    // `XREAD BLOCK 0 STREAMS v1:ticks:book:<market> $` wakes the consumer
    // exactly when the underlying ZSET/HSET has been updated.
    for (const marketKey of bookTickedMarkets) {
      const tickKey = `v1:ticks:book:${tag}:${marketKey}`;
      pipeline.xadd(
        tickKey,
        'MAXLEN',
        '~',
        TICK_STREAM_MAXLEN,
        '*',
        'ts_ms',
        tsMs,
        'tag',
        tag,
        'market',
        marketKey,
      );
    }

    // ── User positions + balance summary ──
    if (snap.users) {
      for (const [owner, user] of Object.entries(snap.users)) {
        for (const pm of user.per_market ?? []) {
          const posKey = `v1:position:${tag}:${owner}:${pm.market}`;
          pipeline.del(posKey);
          pipeline.hset(posKey, {
            base: pm.base_position_lots,
            quote: pm.quote_position_native,
            open_bid: pm.open_order_base_lots_bid,
            open_ask: pm.open_order_base_lots_ask,
            reserved: pm.quote_reserved_lots,
            ts_ms: tsMs,
          });
        }

        if (
          user.margin_summary !== undefined ||
          user.optimistic_collateral !== undefined ||
          user.totals !== undefined
        ) {
          const balKey = `v1:balance:${tag}:${owner}`;
          const hash: Record<string, string> = { ts_ms: tsMs };
          const stringifyOrEmpty = (v: unknown): string => {
            try {
              return JSON.stringify(v);
            } catch {
              return '{}';
            }
          };
          if (user.margin_summary !== undefined) {
            hash.summary = stringifyOrEmpty(user.margin_summary);
          }
          if (user.optimistic_collateral !== undefined) {
            // Per-asset token balances from the harness's optimistic-collateral
            // computation. Writing as a JSON blob under one field rather than
            // exploding into a sub-hash keeps the HSET atomic with the summary
            // + totals write, and the data is small enough (<1KB per user)
            // that parsing on read is cheap. Frontend vault panel + wallet
            // balance lookup read this instead of calling /state/balances.
            hash.tokens = stringifyOrEmpty(user.optimistic_collateral);
          }
          if (user.totals !== undefined) {
            hash.totals = stringifyOrEmpty(user.totals);
          }
          pipeline.del(balKey);
          pipeline.hset(balKey, hash);
        }
      }
    }

    // ── Market metadata (Phase 1c + 5) ──
    // Single-view: market metadata is view-agnostic. We still write under the
    // `opt` tag only because the data is identical across views and the
    // gateway reads this without a view parameter.
    //
    // Payload merges two sources:
    //   - `perp_markets[id]` — fast-changing: oracle/stable prices, funding,
    //     fees. Mirrored every tick.
    //   - `markets[id].metadata` — slow-changing identity: name, symbols,
    //     mints, PDA pubkeys, decimals, open_interest. Frontend needs these
    //     once at boot but the values rarely change; re-writing each tick is
    //     cheap (a single HSET per market).
    //
    // Together, `v1:meta:market:<id>` becomes a complete replacement for the
    // v1 `/state/markets/:id` payload's `metadata` + `metrics` subobjects.
    if (snap.perp_markets && tag === 'opt') {
      const collect: Record<string, Record<string, string>> = {};

      const addField = (id: string, k: string, v: unknown) => {
        if (v === undefined || v === null) return;
        (collect[id] ??= {})[k] = String(v);
      };

      // 1. Dynamic pricing / funding / fees from perp_markets.
      for (const [key, pm] of Object.entries(snap.perp_markets)) {
        const id = String(pm.market_index ?? key);
        addField(id, 'ts_ms', tsMs);
        for (const k of [
          'oracle_price',
          'stable_price',
          'base_lot_size',
          'quote_lot_size',
          'long_funding',
          'short_funding',
          'maker_fee',
          'taker_fee',
          'settle_token_index',
          'market',
          'mark_price',
          'funding_rate_daily',
          'funding_rate_hourly',
        ] as const) {
          addField(id, k, pm[k]);
        }
      }

      // 2. Static identity from markets[id].metadata (if present).
      // NOTE: base_lot_size and quote_lot_size are deliberately NOT copied
      // here — they are already written from perp_markets above. Writing
      // them from both sources would silently overwrite one with the other
      // on drift; perp_markets is the canonical source.
      if (snap.markets) {
        for (const [key, m] of Object.entries(snap.markets)) {
          const md = m.metadata;
          if (!md) continue;
          const id = String(md.market_index ?? m.market ?? key);
          for (const k of [
            'name',
            'base_symbol',
            'quote_symbol',
            'base_mint',
            'quote_mint',
            'perp_market',
            'oracle',
            'bids',
            'asks',
            'event_queue',
            'base_decimals',
            'quote_decimals',
            'open_interest',
          ] as const) {
            addField(id, k, md[k]);
          }
        }
      }

      const ids = Object.keys(collect);
      if (ids.length > 0) {
        pipeline.sadd('v1:meta:markets', ...ids);
        for (const id of ids) {
          const key = `v1:meta:market:${id}`;
          pipeline.del(key);
          pipeline.hset(key, collect[id]);

          // Meta-tick notification. Symmetrical with the book-tick stream
          // above: consumers XREAD BLOCK 0 this and refetch the hash.
          const tickKey = `v1:ticks:meta:${id}`;
          pipeline.xadd(
            tickKey,
            'MAXLEN',
            '~',
            TICK_STREAM_MAXLEN,
            '*',
            'ts_ms',
            tsMs,
            'market',
            id,
          );
        }
      }
    }

    // Fire-and-forget. `.exec()` awaits pipeline flush, which is what we want
    // here (inside tick), but the tick itself is called from a setTimeout —
    // the hot path (broadcastEvent) never awaits this.
    try {
      await pipeline.exec();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('[state-mirror] pipeline.exec failed:', (err as Error).message);
    }
  }

  async close(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    try {
      await this.client.quit();
    } catch {
      this.client.disconnect();
    }
  }

  isEnabled(): boolean { return true; }
}

// ── Module entry point ──────────────────────────────────────────────

export function createMirror(getSnapshot: SnapshotProvider): Mirror {
  if (!MIRROR_ENABLED || !REDIS_URL) {
    // eslint-disable-next-line no-console
    console.info(
      '[state-mirror] disabled (REDIS_MIRROR_ENABLED=%s, REDIS_URL=%s)',
      MIRROR_ENABLED,
      REDIS_URL ? 'set' : 'unset',
    );
    return new NoopMirror();
  }
  // eslint-disable-next-line no-console
  console.info('[state-mirror] enabled, target: %s', REDIS_URL);
  return new IORedisMirror(REDIS_URL, getSnapshot);
}
