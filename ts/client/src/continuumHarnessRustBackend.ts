import { spawnSync } from 'child_process';
import fs from 'fs';
import path from 'path';
import type { ContinuumHarnessBackend } from './continuumHarnessBackend';
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
} from './continuumHarness';

type NativeContinuumStateEngine = {
  bootstrapFromOnchainSnapshotJson(snapshotJson: string): void;
  ingestRelayIntentJson(eventJson: string): void;
  ingestRelayIntentStatusJson?: (eventJson: string) => void;
  ingestQueueEnqueuedJson(eventJson: string): void;
  ingestQueueProcessedJson(eventJson: string): void;
  listDivergencesJson(limit: number): string;
  listIntentsJson(): string;
  findIntentJson(group: string, sequence: string, kind: number): string | null;
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

  constructor(private readonly native: NativeContinuumStateEngine) {}

  bootstrapFromOnchainSnapshot(snapshot: EngineSnapshot): void {
    this.native.bootstrapFromOnchainSnapshotJson(JSON.stringify(snapshot));
  }

  findIntent(
    group: string,
    sequence: string | bigint,
    kind: string | number,
  ): CanonicalIntent | null {
    const raw = this.native.findIntentJson(group, String(sequence), Number(kind));
    return raw ? (JSON.parse(raw) as unknown as CanonicalIntent) : null;
  }

  getAllTrades(view: QueueView, limit = 200): MarketTrade[] {
    return JSON.parse(this.native.getAllTradesJson(view, limit)) as MarketTrade[];
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
    return JSON.parse(this.native.getTradesJson(market, view, limit)) as MarketTrade[];
  }

  getTradesFiltered(params: {
    market?: string | null;
    owner?: string | null;
    view: QueueView;
    limit?: number;
  }): MarketTrade[] {
    return JSON.parse(
      this.native.getTradesFilteredJson(
        params.market || null,
        params.owner || null,
        params.view,
        params.limit ?? 200,
      ),
    ) as MarketTrade[];
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
