import { spawnSync } from 'child_process';
import fs from 'fs';
import path from 'path';
import type { ContinuumHarnessBackend } from './continuumHarnessBackend';
import type { PerpFillEvent } from './continuumHarnessTsEngine';
import type {
  CanonicalIntent,
  DivergenceEvent,
  EngineSnapshot,
  HarnessEvent,
    MarketCandle,
    MarketState,
    MarketTrade,
    OpenOrderSummary,
  QueueItemEnqueuedEvent,
  QueueItemProcessedEvent,
  QueueState,
  QueueView,
    RelayIntentAcceptedEvent,
    RelayIntentStatusEvent,
    UserBalances,
    UserState,
    ValidatedLocalPayload,
  } from './continuumHarness';

type NativeContinuumStateEngine = {
  bootstrapFromOnchainSnapshotJsonAsync(snapshotJson: string): Promise<void>;
  bootstrapFromOnchainSnapshotJson(snapshotJson: string): void;
  ingestRelayIntentJson(eventJson: string): void;
  ingestRelayIntentStatusJson?: (eventJson: string) => void;
  ingestQueueEnqueuedJson(eventJson: string): void;
  ingestQueueProcessedJson(eventJson: string): void;
  listDivergencesJson(limit: number): string;
  listIntentsJson(): string;
  findIntentJson(
    group: string,
    sequence: string,
    kind: number,
    marketIndex?: number | null,
  ): string | null;
  getValidatedLocalPayloadJson(
    group: string,
    sequence: string,
    kind: number,
    includeOwnerState: boolean,
    includeMarketState: boolean,
    includeMarketOpenOrders: boolean,
    marketIndex?: number | null,
  ): string | null;
  getSnapshotJson(view: QueueView): string;
  getMarketStateJson(market: string, view: QueueView): string;
  getUserStateJson(owner: string, view: QueueView): string;
  getQueueStateJson(market: string): string;
  getBalancesJson(owner: string, view: QueueView): string;
  getOrdersJson(
    market: string,
    owner: string | null,
    view: QueueView,
  ): string;
  getTradesJson(market: string, view: QueueView, limit: number): string;
  getAllTradesJson(view: QueueView, limit: number): string;
  getTradesFilteredJson(
    market: string | null,
    owner: string | null,
    view: QueueView,
    limit: number,
  ): string;
  getCandlesJson(
    market: string,
    view: QueueView,
    resolutionSec: number,
    limit: number,
  ): string;
  reportExternalDivergenceJson(reason: string, key: string, detailsJson: string): void;
};

type NativeBindingModule = {
  NativeContinuumStateEngine?: new () => NativeContinuumStateEngine;
  default?: {
    NativeContinuumStateEngine?: new () => NativeContinuumStateEngine;
  };
};

class RustContinuumStateEngine implements ContinuumHarnessBackend {
  private readonly listeners = new Set<(event: HarnessEvent) => void>();

  /**
   * TS-side cache of chain-authoritative fills ingested from the
   * relayer's `perp_fill` events. Held here rather than inside the
   * native rust engine so we can layer this on without rebuilding the
   * native addon; the native engine's in-process matcher still drives
   * order-book / position state, we just surface fills that the native
   * matcher can't reproduce (it lacks the maker side of v5 reveals).
   * Keyed by market id (same string id the native engine uses).
   */
  private readonly chainFillsByMarket = new Map<string, MarketTrade[]>();
  private readonly chainFillSeenKeys = new Set<string>();
  private static readonly CHAIN_FILL_MAX_PER_MARKET = 10000;

  constructor(private readonly native: NativeContinuumStateEngine) {}

  async bootstrapFromOnchainSnapshot(snapshot: EngineSnapshot): Promise<void> {
    await this.native.bootstrapFromOnchainSnapshotJsonAsync(JSON.stringify(snapshot));
  }

  findIntent(
    group: string,
    sequence: string | bigint,
    kind: string | number,
    marketIndex?: number | null,
  ): CanonicalIntent | null {
    const raw = this.native.findIntentJson(
      group,
      String(sequence),
      Number(kind),
      marketIndex ?? undefined,
    );
    return raw ? (JSON.parse(raw) as unknown as CanonicalIntent) : null;
  }

  getValidatedLocalPayload(
    group: string,
    sequence: string | bigint,
    kind: string | number,
    opts?: {
      includeOwnerState?: boolean;
      includeMarketState?: boolean;
      includeMarketOpenOrders?: boolean;
      marketIndex?: number | null;
    },
  ): ValidatedLocalPayload | null {
    const includeOwnerState = opts?.includeOwnerState !== false;
    const includeMarketState = opts?.includeMarketState !== false;
    const includeMarketOpenOrders = opts?.includeMarketOpenOrders !== false;
    const raw = this.native.getValidatedLocalPayloadJson(
      group,
      String(sequence),
      Number(kind),
      includeOwnerState,
      includeMarketState,
      includeMarketOpenOrders,
      opts?.marketIndex ?? undefined,
    );
    return raw ? (JSON.parse(raw) as ValidatedLocalPayload) : null;
  }

  getAllTrades(view: QueueView, limit = 200): MarketTrade[] {
    const native = JSON.parse(
      this.native.getAllTradesJson(view, limit),
    ) as MarketTrade[];
    if (this.chainFillsByMarket.size === 0) return native;
    // Union: prefer chain fills for any market we've seen them on, and
    // fall back to native trades for markets we haven't seen.
    const chainMarkets = new Set(this.chainFillsByMarket.keys());
    const nativeKept = native.filter((t) => !chainMarkets.has(t.market));
    const chainAll: MarketTrade[] = [];
    for (const [market, fills] of this.chainFillsByMarket.entries()) {
      for (const f of fills) chainAll.push({ ...f, view });
      void market;
    }
    const merged = [...nativeKept, ...chainAll].sort(
      (a, b) => a.ts_ms - b.ts_ms,
    );
    return merged.slice(Math.max(0, merged.length - limit));
  }

  getBalances(owner: string, view: QueueView): UserBalances {
    return JSON.parse(this.native.getBalancesJson(owner, view)) as UserBalances;
  }

  getCandles(
    market: string,
    view: QueueView,
    resolutionSec: number,
    limit = 200,
  ): MarketCandle[] {
    return JSON.parse(
      this.native.getCandlesJson(market, view, resolutionSec, limit),
    ) as MarketCandle[];
  }

  getQueueState(market: string): QueueState {
    return JSON.parse(this.native.getQueueStateJson(market)) as QueueState;
  }

  getMarketState(market: string, view: QueueView): MarketState {
    return JSON.parse(this.native.getMarketStateJson(market, view)) as MarketState;
  }

  getOrders(
    market: string,
    owner: string | null,
    view: QueueView,
  ): OpenOrderSummary[] {
    return JSON.parse(this.native.getOrdersJson(market, owner, view)) as OpenOrderSummary[];
  }

  getSnapshot(view: QueueView): EngineSnapshot {
    return JSON.parse(this.native.getSnapshotJson(view)) as EngineSnapshot;
  }

  getTrades(market: string, view: QueueView, limit = 200): MarketTrade[] {
    return this.mergeChainFillsForMarket(
      market,
      view,
      limit,
      () =>
        JSON.parse(
          this.native.getTradesJson(market, view, limit),
        ) as MarketTrade[],
    );
  }

  getTradesFiltered(params: {
    market?: string | null;
    owner?: string | null;
    view: QueueView;
    limit?: number;
  }): MarketTrade[] {
    const limit = params.limit ?? 200;
    // Per-market + owner-filter case: use chain fills for the market
    // (filtered by owner below), falling back to native only if none.
    if (params.market) {
      const merged = this.mergeChainFillsForMarket(
        params.market,
        params.view,
        limit,
        () =>
          JSON.parse(
            this.native.getTradesFilteredJson(
              params.market || null,
              params.owner || null,
              params.view,
              limit,
            ),
          ) as MarketTrade[],
      );
      if (!params.owner) return merged;
      return merged.filter(
        (t) => t.maker_owner === params.owner || t.taker_owner === params.owner,
      );
    }
    // All-markets case: reuse getAllTrades merge then apply owner filter.
    const all = this.getAllTrades(params.view, limit);
    if (!params.owner) return all;
    return all.filter(
      (t) => t.maker_owner === params.owner || t.taker_owner === params.owner,
    );
  }

  getUserState(owner: string, view: QueueView): UserState {
    return JSON.parse(this.native.getUserStateJson(owner, view)) as UserState;
  }

  ingestQueueEnqueued(event: QueueItemEnqueuedEvent): void {
    this.native.ingestQueueEnqueuedJson(JSON.stringify(event));
    this.emit(event);
  }

  ingestQueueProcessed(event: QueueItemProcessedEvent): void {
    this.native.ingestQueueProcessedJson(JSON.stringify(event));
    this.emit(event);
  }

  ingestRelayIntent(event: RelayIntentAcceptedEvent): void {
    this.native.ingestRelayIntentJson(JSON.stringify(event));
    this.emit(event);
  }

  ingestRelayIntentStatus(event: RelayIntentStatusEvent): void {
    this.native.ingestRelayIntentStatusJson?.(JSON.stringify(event));
    this.emit(event);
  }

  ingestPerpFill(event: PerpFillEvent): void {
    const marketIdStr =
      event.market != null
        ? String(event.market)
        : event.market_index != null
          ? String(event.market_index)
          : null;
    if (marketIdStr === null) return;

    const txSig = String(event.tx_signature ?? '');
    const priceLotsStr = String(event.price_lots ?? '0');
    const baseLotsStr = String(event.base_lots ?? '0');
    const tsMs = Number(event.ts_ms ?? Date.now());
    const dedupKey = txSig
      ? `${txSig}:${marketIdStr}:${event.taker ?? ''}:${priceLotsStr}:${baseLotsStr}:${tsMs}`
      : `nosig:${marketIdStr}:${tsMs}:${event.taker ?? ''}:${priceLotsStr}:${baseLotsStr}`;
    if (this.chainFillSeenKeys.has(dedupKey)) return;
    this.chainFillSeenKeys.add(dedupKey);

    const priceLotsNum = Number(priceLotsStr);
    const baseLotsNum = Number(baseLotsStr);
    const quoteLotsStr = (() => {
      if (typeof event.quote_lots === 'string' && event.quote_lots.length) {
        return event.quote_lots;
      }
      if (typeof event.quote_lots === 'number' && Number.isFinite(event.quote_lots)) {
        return String(event.quote_lots);
      }
      // Fallback: derive from price * base if not provided.
      if (Number.isFinite(priceLotsNum) && Number.isFinite(baseLotsNum)) {
        return String(priceLotsNum * baseLotsNum);
      }
      return '0';
    })();

    const takerSide: 'bid' | 'ask' =
      Number(event.taker_side ?? 0) === 0 ? 'bid' : 'ask';

    const bucket = this.chainFillsByMarket.get(marketIdStr) || [];
    const trade: MarketTrade = {
      trade_id: txSig
        ? `${txSig}:${marketIdStr}:${bucket.length}`
        : `${marketIdStr}:${tsMs}:${bucket.length}`,
      market: marketIdStr,
      price_lots: priceLotsStr,
      base_lots: baseLotsStr,
      quote_lots: quoteLotsStr,
      taker_side: takerSide,
      maker_owner: String(event.maker ?? ''),
      taker_owner: String(event.taker ?? ''),
      maker_order_id: String(event.maker_client_order_id ?? ''),
      taker_sequence: String(event.seq_num ?? '0'),
      ts_ms: tsMs,
      view: 'confirmed',
    } as MarketTrade;
    bucket.push(trade);
    if (bucket.length > RustContinuumStateEngine.CHAIN_FILL_MAX_PER_MARKET) {
      bucket.splice(
        0,
        bucket.length - RustContinuumStateEngine.CHAIN_FILL_MAX_PER_MARKET,
      );
    }
    this.chainFillsByMarket.set(marketIdStr, bucket);
    // Fan out to subscribers so downstream mirrors (Redis publisher,
    // state-mirror snapshot debouncer) see the new fill. Emitting the
    // raw `perp_fill` shape — the publisher recognizes it directly.
    this.emit({
      ...event,
      event_type: 'perp_fill',
      // Normalize the market field so the publisher's `streamKeyFor`
      // and `extractMarket` land on the right stream key.
      market: marketIdStr,
      market_index: Number(marketIdStr),
    } as HarnessEvent);
  }

  /**
   * Return chain-authoritative fills for a market if any have been
   * ingested via `ingestPerpFill`, else fall back to whatever the native
   * engine's simulator produced. Chain fills win because the simulator's
   * orderbook can drift from chain under v5 (it doesn't see the maker
   * side of reveals).
   */
  private mergeChainFillsForMarket(
    market: string,
    view: QueueView,
    limit: number,
    nativeFallback: () => MarketTrade[],
  ): MarketTrade[] {
    const chain = this.chainFillsByMarket.get(market);
    if (!chain || chain.length === 0) return nativeFallback();
    const capped = Math.max(0, Math.min(limit, chain.length));
    const tail = chain.slice(chain.length - capped);
    // Stamp the requested view on the returned records so callers can
    // key off it; the underlying record was stored as 'confirmed'.
    return tail.map((t) => ({ ...t, view }));
  }

  listDivergences(limit = 200): DivergenceEvent[] {
    return JSON.parse(this.native.listDivergencesJson(limit)) as DivergenceEvent[];
  }

  listIntents(): CanonicalIntent[] {
    return JSON.parse(this.native.listIntentsJson()) as unknown as CanonicalIntent[];
  }

  reportExternalDivergence(
    reason: string,
    key: string,
    details: Record<string, string>,
  ): void {
    this.native.reportExternalDivergenceJson(reason, key, JSON.stringify(details));
    this.emit({
      event_type: 'divergence_event',
      ts_ms: Date.now(),
      reason,
      key,
      details,
    });
  }

  subscribe(listener: (event: HarnessEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(event: HarnessEvent): void {
    for (const listener of this.listeners) {
      listener(event);
    }
  }
}

export function createRustBackend(): ContinuumHarnessBackend {
  const native = instantiateNativeBinding();
  return new RustContinuumStateEngine(native);
}

function instantiateNativeBinding(): NativeContinuumStateEngine {
  const addonPath = ensureNativeAddonPath();
  const loaded = require(addonPath) as NativeBindingModule;
  const NativeCtor =
    loaded.NativeContinuumStateEngine ||
    loaded.default?.NativeContinuumStateEngine;
  if (!NativeCtor) {
    throw new Error(
      `rust-backend addon at "${addonPath}" did not export NativeContinuumStateEngine`,
    );
  }
  return new NativeCtor();
}

function ensureNativeAddonPath(): string {
  const explicit = process.env.CONTINUUM_HARNESS_RUST_NATIVE_PATH;
  if (explicit) {
    return explicit;
  }

  const repoRoot = findRepoRoot(__dirname);
  const targetDir = path.join(repoRoot, 'target');
  const profile =
    (process.env.CONTINUUM_HARNESS_RUST_PROFILE || 'release').trim() || 'release';
  const addonPath = path.join(targetDir, profile, 'rust_harness.node');
  const sharedLibPath = detectSharedLibraryPath(targetDir, profile);

  if (fs.existsSync(addonPath) && (!sharedLibPath || !shouldRefreshAddon(sharedLibPath, addonPath))) {
    return addonPath;
  }

  const build = spawnSync(
    process.execPath,
    [path.join(repoRoot, 'scripts', 'build-rust-harness-native.js'), '--profile', profile],
    {
      cwd: repoRoot,
      env: process.env,
      stdio: 'pipe',
      encoding: 'utf8',
    },
  );
  if (build.status !== 0) {
    throw new Error(
      `failed to build rust-backend addon: ${build.stderr || build.stdout || `builder exited ${build.status}`}`,
    );
  }

  if (!fs.existsSync(addonPath)) {
    throw new Error(
      `rust-backend addon was not produced at "${addonPath}"`,
    );
  }
  return addonPath;
}

function shouldRefreshAddon(sharedLibPath: string, addonPath: string): boolean {
  if (!fs.existsSync(addonPath)) {
    return true;
  }
  try {
    const sharedStat = fs.statSync(sharedLibPath);
    const addonStat = fs.statSync(addonPath);
    return sharedStat.mtimeMs >= addonStat.mtimeMs;
  } catch {
    return true;
  }
}

function detectSharedLibraryPath(targetDir: string, profile: string): string | null {
  const candidates =
    process.platform === 'win32'
      ? [path.join(targetDir, profile, 'rust_harness.dll')]
      : process.platform === 'darwin'
        ? [path.join(targetDir, profile, 'librust_harness.dylib')]
        : [path.join(targetDir, profile, 'librust_harness.so')];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

function findRepoRoot(startDir: string): string {
  let current = path.resolve(startDir);
  while (true) {
    if (fs.existsSync(path.join(current, 'rust-harness', 'Cargo.toml'))) {
      return current;
    }
    const parent = path.dirname(current);
    if (parent === current) {
      throw new Error(`could not locate repo root from "${startDir}"`);
    }
    current = parent;
  }
}
