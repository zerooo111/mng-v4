import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import {
  createAssociatedTokenAccountIdempotent,
  getMint,
  mintTo,
} from '@solana/spl-token';
import {
  Cluster,
  Commitment,
  Connection,
  Keypair,
  Logs,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import http, { IncomingMessage, ServerResponse } from 'http';
import path from 'path';
import { monitorEventLoopDelay, performance } from 'perf_hooks';
import { MANGO_V4_ID } from '../../src/constants';
import {
  EngineSnapshot,
  HarnessEvent,
  MarginSummary,
  MarginSummaryAccount,
  MarketState,
  MarketTrade,
  OpenOrderSummary,
  QueueState,
  QueueView,
  RelayIntentAcceptedEvent,
  UserState,
  decodeQueuePayload,
  decodeQueueAnchorEvent,
  parseProgramDataLogLine,
} from '../../src/continuumHarness';
import {
  ContinuumHarnessBackend,
  HarnessBackendKind,
  createContinuumHarnessBackend,
  parseHarnessBackendKind,
} from '../../src/continuumHarnessBackend';
import { MangoClient } from '../../src/client';
import { HealthType } from '../../src/accounts/mangoAccount';
import { HealthCache } from '../../src/accounts/healthCache';
import {
  decodeExecutionQueueHeader,
  decodeExecutionQueuePendingItems,
} from '../../src/executionQueueLayout';
import { I80F48, ZERO_I80F48 } from '../../src/numbers/I80F48';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const PROGRAM_ID_OVERRIDE = process.env.CONTINUUM_HARNESS_PROGRAM_ID;
const HARNESS_BIND_ADDR =
  process.env.CONTINUUM_HARNESS_BIND_ADDR || '0.0.0.0:9091';
const HARNESS_MODE = process.env.CONTINUUM_HARNESS_MODE || 'local';
const HARNESS_BACKEND: HarnessBackendKind = parseHarnessBackendKind(
  process.env.CONTINUUM_HARNESS_BACKEND,
);
const HARNESS_PROCESS_STARTED_TS_MS = Date.now();
const HARNESS_INSTANCE_ID = `${process.pid}-${HARNESS_PROCESS_STARTED_TS_MS}`;
const HARNESS_EVENT_LOG_PATH =
  process.env.CONTINUUM_HARNESS_EVENT_LOG_PATH ||
  '/tmp/continuum-harness-events.jsonl';
const HARNESS_RELAY_INGEST_TOKEN =
  process.env.CONTINUUM_HARNESS_RELAY_INGEST_TOKEN || '';
const HARNESS_REPLAY_LOG =
  (process.env.CONTINUUM_HARNESS_REPLAY_LOG || 'true') === 'true';
const HARNESS_BACKFILL_SIGNATURE_LIMIT = Number(
  process.env.CONTINUUM_HARNESS_BACKFILL_SIGNATURE_LIMIT || '0',
);
const HARNESS_COMMITMENT: Commitment =
  (process.env.CONTINUUM_HARNESS_COMMITMENT as Commitment) || 'confirmed';
const REQUEST_BODY_MAX_BYTES = Number(
  process.env.CONTINUUM_HARNESS_REQUEST_BODY_MAX_BYTES || '1048576',
);
const HARNESS_ENABLE_AIRDROP =
  (process.env.CONTINUUM_HARNESS_ENABLE_AIRDROP ||
    (HARNESS_MODE === 'local' ? 'true' : 'false')) === 'true';
const HARNESS_USDC_MINT = process.env.CONTINUUM_HARNESS_USDC_MINT || '';
const HARNESS_AIRDROP_KEYPAIR =
  process.env.CONTINUUM_HARNESS_AIRDROP_KEYPAIR ||
  process.env.MB_PAYER_KEYPAIR ||
  '';
const HARNESS_AIRDROP_DEFAULT_UI_AMOUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_DEFAULT_UI_AMOUNT || '1000',
);
const HARNESS_AIRDROP_MAX_UI_AMOUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_MAX_UI_AMOUNT || '100000',
);
const HARNESS_GROUP_PK =
  process.env.CONTINUUM_HARNESS_GROUP_PK ||
  process.env.EXECUTION_QUEUE_GROUP_PK ||
  '';
const HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT || '1000',
);
const HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT =
  (process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT ||
    (HARNESS_MODE === 'local' ? 'true' : 'false')) === 'true';
const HARNESS_AIRDROP_AUTO_CREATE_ACCOUNT_NUM = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_ACCOUNT_NUM || '0',
);
const HARNESS_AIRDROP_AUTO_CREATE_TOKEN_COUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_TOKEN_COUNT || '8',
);
const HARNESS_AIRDROP_AUTO_CREATE_SERUM3_COUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_SERUM3_COUNT || '4',
);
const HARNESS_AIRDROP_AUTO_CREATE_PERP_COUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_PERP_COUNT || '4',
);
const HARNESS_AIRDROP_AUTO_CREATE_PERP_OO_COUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_PERP_OO_COUNT || '32',
);
const HARNESS_AIRDROP_AUTO_CREATE_NAME =
  process.env.CONTINUUM_HARNESS_AIRDROP_AUTO_CREATE_NAME || 'harness-auto';
const HARNESS_SANITY_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_SANITY_INTERVAL_MS || '5000',
);
const HARNESS_ONCHAIN_CACHE_TTL_MS = Number(
  process.env.CONTINUUM_HARNESS_ONCHAIN_CACHE_TTL_MS || '5000',
);
const HARNESS_RECONCILE_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_RECONCILE_INTERVAL_MS || '10000',
);
const HARNESS_DIRECT_ONCHAIN_REBASE_DEBOUNCE_MS = Number(
  process.env.CONTINUUM_HARNESS_DIRECT_ONCHAIN_REBASE_DEBOUNCE_MS || '750',
);
const HARNESS_QUEUE_SYNC_DEBOUNCE_MS = Number(
  process.env.CONTINUUM_HARNESS_QUEUE_SYNC_DEBOUNCE_MS || '250',
);
const HARNESS_PROGRAM_LOG_BLOCK_TIME_CACHE_LIMIT = 4096;
const HARNESS_EVENT_LOG_MAX_BYTES = Number(
  process.env.CONTINUUM_HARNESS_EVENT_LOG_MAX_BYTES || '134217728',
);
const HARNESS_RUNTIME_ERROR_LOG_PATH =
  process.env.CONTINUUM_HARNESS_RUNTIME_ERROR_LOG_PATH ||
  `${HARNESS_EVENT_LOG_PATH}.errors.jsonl`;
const HARNESS_RUNTIME_ERROR_LOG_MAX_BYTES = Number(
  process.env.CONTINUUM_HARNESS_RUNTIME_ERROR_LOG_MAX_BYTES || '16777216',
);
const HARNESS_RUNTIME_ERROR_HISTORY_LIMIT = Number(
  process.env.CONTINUUM_HARNESS_RUNTIME_ERROR_HISTORY_LIMIT || '200',
);
const HARNESS_RUNTIME_LATENCY_LOG_PATH =
  process.env.CONTINUUM_HARNESS_RUNTIME_LATENCY_LOG_PATH ||
  `${HARNESS_EVENT_LOG_PATH}.latency.jsonl`;
const HARNESS_RUNTIME_LATENCY_LOG_MAX_BYTES = Number(
  process.env.CONTINUUM_HARNESS_RUNTIME_LATENCY_LOG_MAX_BYTES || '33554432',
);
const HARNESS_DISCREPANCY_LOG_PATH =
  process.env.CONTINUUM_HARNESS_DISCREPANCY_LOG_PATH ||
  `${HARNESS_EVENT_LOG_PATH}.discrepancies.jsonl`;
const HARNESS_DISCREPANCY_LOG_MAX_BYTES = Number(
  process.env.CONTINUUM_HARNESS_DISCREPANCY_LOG_MAX_BYTES || '33554432',
);
const HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT = Number(
  process.env.CONTINUUM_HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT || '500',
);
const HARNESS_MAX_SSE_CLIENTS = Number(
  process.env.CONTINUUM_HARNESS_MAX_SSE_CLIENTS || '250',
);
const HARNESS_HTTP_REQUEST_TIMEOUT_MS = Number(
  process.env.CONTINUUM_HARNESS_HTTP_REQUEST_TIMEOUT_MS || '30000',
);
const HARNESS_HTTP_HEADERS_TIMEOUT_MS = Number(
  process.env.CONTINUUM_HARNESS_HTTP_HEADERS_TIMEOUT_MS || '35000',
);
const HARNESS_HTTP_KEEPALIVE_TIMEOUT_MS = Number(
  process.env.CONTINUUM_HARNESS_HTTP_KEEPALIVE_TIMEOUT_MS || '15000',
);
const HARNESS_MAX_HTTP_CONNECTIONS = Number(
  process.env.CONTINUUM_HARNESS_MAX_HTTP_CONNECTIONS || '0',
);
const HARNESS_SLOW_COMPONENT_MS = Number(
  process.env.CONTINUUM_HARNESS_SLOW_COMPONENT_MS || '25',
);
const HARNESS_SLOW_HTTP_REQUEST_MS = Number(
  process.env.CONTINUUM_HARNESS_SLOW_HTTP_REQUEST_MS || '250',
);
const HARNESS_SLOW_SSE_NOTIFY_MS = Number(
  process.env.CONTINUUM_HARNESS_SLOW_SSE_NOTIFY_MS || '100',
);
const HARNESS_SLOW_BACKEND_CALL_MS = Number(
  process.env.CONTINUUM_HARNESS_SLOW_BACKEND_CALL_MS || '25',
);
const HARNESS_SLOW_PERIODIC_TASK_MS = Number(
  process.env.CONTINUUM_HARNESS_SLOW_PERIODIC_TASK_MS || '500',
);
const HARNESS_EVENT_LOOP_SAMPLE_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_EVENT_LOOP_SAMPLE_INTERVAL_MS || '15000',
);
const HARNESS_EVENT_LOOP_LAG_WARN_MS = Number(
  process.env.CONTINUUM_HARNESS_EVENT_LOOP_LAG_WARN_MS || '50',
);

let engine!: ContinuumHarnessBackend;
const sseClients = new Set<ServerResponse>();
const sanityStateByOwner = new Map<string, string>();
let lastReconciliationSig = '';
let nextRuntimeErrorSeq = 1;
let totalRuntimeErrors = 0;
let fatalStartupError: RuntimeErrorEntry | null = null;
const runtimeErrors: RuntimeErrorEntry[] = [];
let nextRuntimeLatencySeq = 1;
let nextDiscrepancySeq = 1;
let totalLatencySamples = 0;
let totalSlowLatencySamples = 0;
let totalLatencyErrorSamples = 0;
const runtimeLatencyHistory: RuntimeLatencyEntry[] = [];
const runtimeLatencyAggregates = new Map<string, RuntimeLatencyAggregate>();
let lastEventLoopLagSnapshot: EventLoopLagSnapshot | null = null;
const fileSizeCache = new Map<string, number>();
let tradeStreamNotifyRunning = false;
let tradeStreamNotifyPendingForceAll = false;
let frontendStreamNotifyRunning = false;
let frontendStreamNotifyPendingForceAll = false;
let diagnosticsHoldIntervalStarted = false;
const eventLoopDelayMonitor = monitorEventLoopDelay({ resolution: 20 });
eventLoopDelayMonitor.enable();

type HarnessGroup = Awaited<ReturnType<MangoClient['getGroup']>>;

type HarnessMarketMetadata = {
  market_index: number;
  name: string;
  base_symbol: string;
  quote_symbol: string;
  base_mint: string;
  quote_mint: string;
  perp_market: string;
  oracle: string;
  bids: string;
  asks: string;
  event_queue: string;
  base_decimals: number;
  quote_decimals: number;
  base_lot_size: string;
  quote_lot_size: string;
  open_interest: string;
};

type OnchainContext = {
  connection: Connection;
  groupPk: PublicKey | null;
  mangoClient: MangoClient | null;
  usdcMint: PublicKey | null;
  programId: PublicKey;
  executionQueuePk: PublicKey | null;
  cachedGroup: HarnessGroup | null;
  cachedGroupFetchedAtMs: number;
  cachedMarketMetadata: Record<string, HarnessMarketMetadata> | null;
  cachedMarketMetadataFetchedAtMs: number;
};

type AirdropContext = OnchainContext & {
  usdcMint: PublicKey;
  decimals: number;
  faucet: Keypair;
  defaultUiAmount: number;
  maxUiAmount: number;
  depositUiAmount: number;
};

type ReconciliationMarketDrift = {
  replay_open_orders: number;
  onchain_open_orders: number;
  replay_best_bid: string | null;
  onchain_best_bid: string | null;
  replay_best_ask: string | null;
  onchain_best_ask: string | null;
  bid_base_lots_abs_diff: string;
  ask_base_lots_abs_diff: string;
};

type ReconciliationSnapshot = {
  ts_ms: number;
  replay_generated_ts_ms: number;
  onchain_generated_ts_ms: number;
  totals: {
    replay_open_orders: number;
    onchain_open_orders: number;
    bid_base_lots_abs_diff: string;
    ask_base_lots_abs_diff: string;
    markets_with_drift: number;
  };
  markets: Record<string, ReconciliationMarketDrift>;
};

type OnchainQueuePendingItem = {
  market: string;
  sequence: string;
  kind: number;
  min_execute_slot: string;
  section: 'ctm' | 'liquidity';
};

type OnchainQueueSnapshot = {
  generated_ts_ms: number;
  observed_slot: number;
  next_sequence: string;
  max_seen_sequence: string;
  total_count: number;
  items: OnchainQueuePendingItem[];
};

type OnchainSyncState = {
  snapshot: EngineSnapshot | null;
  drift: ReconciliationSnapshot | null;
  queue: OnchainQueueSnapshot | null;
  last_error: string | null;
};

type DecodedQueueLogEvent = NonNullable<
  ReturnType<typeof decodeQueueAnchorEvent>
>;

const programLogBlockTimeCache = new Map<number, number | null>();
const programLogBlockTimeInflight = new Map<number, Promise<number | null>>();
let programLogHandlingQueue: Promise<void> = Promise.resolve();

type RuntimeErrorEntry = {
  id: number;
  ts_ms: number;
  source: string;
  message: string;
  stack: string | null;
  context: Record<string, string> | null;
};

type RuntimeLatencyEntry = {
  id: number;
  ts_ms: number;
  component: string;
  duration_ms: number;
  threshold_ms: number;
  outcome: 'slow' | 'error';
  detail: string | null;
  context: Record<string, string> | null;
};

type RuntimeLatencyAggregate = {
  component: string;
  samples: number;
  slow_samples: number;
  error_samples: number;
  total_ms: number;
  max_ms: number;
  last_ms: number;
  last_ts_ms: number;
};

type DiscrepancyLogEntry = {
  id: number;
  ts_ms: number;
  reason: 'onchain_reconciliation_drift';
  snapshot: ReconciliationSnapshot;
};

type EventLoopLagSnapshot = {
  ts_ms: number;
  min_ms: number;
  mean_ms: number;
  max_ms: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  stddev_ms: number;
  exceeds_threshold: boolean;
};

type OrderbookLevelView = {
  price_lots: string;
  base_lots: string;
  price_ui: number | null;
  qty_ui: number | null;
};

type OrderbookSummaryView = {
  depth: number;
  bids: OrderbookLevelView[];
  asks: OrderbookLevelView[];
};

type MarketTradeSummary = {
  market: string;
  view: QueueView;
  window_ms: number;
  trade_count: number;
  last_trade_ts_ms: number | null;
  last_price_lots: string | null;
  last_price_ui: number | null;
  open_price_lots: string | null;
  open_price_ui: number | null;
  high_price_lots: string | null;
  high_price_ui: number | null;
  low_price_lots: string | null;
  low_price_ui: number | null;
  change_24h_pct: number | null;
  volume_base_lots: string;
  volume_quote_lots: string;
  volume_base_ui: number | null;
  volume_quote_ui: number | null;
};

type MarketRuntimeMetrics = {
  market: string;
  oracle_price_ui: number | null;
  mark_price_ui: number | null;
  funding_rate_daily_pct: number | null;
  funding_rate_hourly_pct: number | null;
  open_interest_base_lots: string | null;
  open_interest_base_ui: number | null;
  best_bid_ui: number | null;
  best_ask_ui: number | null;
  updated_ts_ms: number;
};

type MarketListItem = {
  market: string;
  view: QueueView;
  metadata: HarnessMarketMetadata | null;
  data: EngineSnapshot['markets'][string];
  orderbook_summary: OrderbookSummaryView;
  trade_summary: MarketTradeSummary;
  metrics: MarketRuntimeMetrics | null;
};

type StubbedAccountMetrics = {
  status: 'stub';
  source: 'pending-subtree';
  updated_ts_ms: number;
  fields: {
    margin_used: null;
    health_init: null;
    health_maint: null;
    pnl_realized: null;
    pnl_unrealized: null;
    equity: null;
    liquidation_price_by_market: null;
  };
};

type FrontendAccountMetrics =
  | StubbedAccountMetrics
  | {
      status: 'empty' | 'ok';
      source:
        | 'onchain-mango-health'
        | 'rust-replay-perp-token-health'
        | 'rust-replay-perp-token-health-partial';
      updated_ts_ms: number;
      account_count: number;
      mango_account: string | null;
      totals: {
        equity_native_quote: string;
        pnl_native_quote: string;
        assets_native_quote: string;
        liabs_native_quote: string;
        init_health_native_quote: string;
        maint_health_native_quote: string;
        margin_usage_fraction: number;
      };
      accounts: MarginSummaryAccount[];
      fields: {
        margin_used: number;
        health_init: string;
        health_maint: string;
        pnl_realized: null;
        pnl_unrealized: string;
        equity: string;
        liquidation_price_by_market: null;
      };
    };

type FrontendOwnerSlice = {
  owner: string;
  mango_account: string | null;
  view: QueueView;
  positions_scope: 'owner_aggregate';
  positions: UserState['per_market'];
  open_orders: OpenOrderSummary[];
  trades: MarketTrade[];
  account_metrics: FrontendAccountMetrics;
};

type FrontendMarketSlice = {
  market: string;
  view: QueueView;
  metadata: HarnessMarketMetadata | null;
  metrics: MarketRuntimeMetrics | null;
  trade_summary: MarketTradeSummary;
  orderbook_summary: OrderbookSummaryView;
  orderbook: EngineSnapshot['markets'][string] | null;
};

type TradeStreamSubscriber = {
  id: string;
  res: ServerResponse;
  view: QueueView;
  market: string | null;
  owner: string | null;
  backfillLimit: number;
};

type FrontendStreamSubscriber = {
  id: string;
  res: ServerResponse;
  view: QueueView;
  owner: string | null;
  mangoAccount: string | null;
  market: string | null;
  depth: number;
  tradesLimit: number;
  include: Set<string>;
  orderbookMode: 'summary' | 'full';
  ownerSignature: string | null;
  marketSignature: string | null;
};

type FrontendPayloadBuildCache = {
  snapshotsByView: Map<QueueView, EngineSnapshot>;
  metadataMapPromise: Promise<Record<string, HarnessMarketMetadata>>;
  ownerSliceCache: Map<string, Promise<FrontendOwnerSlice>>;
  marketSliceCache: Map<string, Promise<FrontendMarketSlice>>;
  tradeCache: Map<string, MarketTrade[]>;
};

const TRADE_SUMMARY_WINDOW_MS = 24 * 60 * 60 * 1000;
const DEFAULT_STREAM_BACKFILL = 50;
const DEFAULT_FRONTEND_TRADES_LIMIT = 50;
const DEFAULT_ORDERBOOK_DEPTH = 10;
const tradeStreamSubscribers = new Map<string, TradeStreamSubscriber>();
const frontendStreamSubscribers = new Map<string, FrontendStreamSubscriber>();
const tradeStreamCursors = new Map<string, string | null>();
const marketRuntimeMetricsCache = new Map<
  string,
  { fetchedAtMs: number; data: MarketRuntimeMetrics | null }
>();
let nextStreamSubscriberSeq = 1;
let directOnchainRebaseTimer: NodeJS.Timeout | null = null;
let directOnchainRebaseInFlight: Promise<void> | null = null;
let directOnchainRebasePending = false;
let queueSyncTimer: NodeJS.Timeout | null = null;
let queueSyncInFlight: Promise<void> | null = null;
let queueSyncPending = false;

function ensureDirForFile(filePath: string): void {
  fs.mkdirSync(path.dirname(path.resolve(filePath)), { recursive: true });
}

function normalizeError(err: unknown): {
  message: string;
  stack: string | null;
} {
  if (err instanceof Error) {
    return {
      message: err.message || `${err}`,
      stack: err.stack || null,
    };
  }
  return {
    message: `${err}`,
    stack: null,
  };
}

function truncateForLog(value: string, limit: number): string {
  if (value.length <= limit) {
    return value;
  }
  return `${value.slice(0, limit)}…`;
}

function sanitizeLogContext(
  context?: Record<string, string | number | boolean | null | undefined>,
): Record<string, string> | null {
  if (!context || !Object.keys(context).length) {
    return null;
  }
  return Object.fromEntries(
    Object.entries(context).map(([key, value]) => [
      key,
      truncateForLog(String(value), 1_000),
    ]),
  );
}

function appendLineWithRotation(
  filePath: string,
  line: string,
  maxBytes: number,
): void {
  ensureDirForFile(filePath);
  const resolved = path.resolve(filePath);
  let currentSize = fileSizeCache.get(resolved);
  if (currentSize === undefined) {
    try {
      currentSize = fs.existsSync(resolved) ? fs.statSync(resolved).size : 0;
    } catch {
      currentSize = 0;
    }
  }

  const lineBytes = Buffer.byteLength(line);
  if (maxBytes > 0 && currentSize + lineBytes > maxBytes) {
    const rotatedPath = `${resolved}.1`;
    try {
      if (fs.existsSync(rotatedPath)) {
        fs.unlinkSync(rotatedPath);
      }
      if (fs.existsSync(resolved)) {
        fs.renameSync(resolved, rotatedPath);
      }
      currentSize = 0;
    } catch (err) {
      const normalized = normalizeError(err);
      console.error(
        `[harness:log_rotate] failed to rotate ${resolved}: ${normalized.message}`,
      );
    }
  }

  fs.appendFileSync(resolved, line);
  fileSizeCache.set(resolved, currentSize + lineBytes);
}

function recordRuntimeError(
  source: string,
  err: unknown,
  context?: Record<string, string | number | boolean | null | undefined>,
): RuntimeErrorEntry {
  const normalized = normalizeError(err);
  const entry: RuntimeErrorEntry = {
    id: nextRuntimeErrorSeq++,
    ts_ms: Date.now(),
    source,
    message: truncateForLog(normalized.message, 4_000),
    stack: normalized.stack ? truncateForLog(normalized.stack, 12_000) : null,
    context: sanitizeLogContext(context),
  };

  totalRuntimeErrors += 1;
  runtimeErrors.push(entry);
  if (runtimeErrors.length > HARNESS_RUNTIME_ERROR_HISTORY_LIMIT) {
    runtimeErrors.splice(
      0,
      runtimeErrors.length - HARNESS_RUNTIME_ERROR_HISTORY_LIMIT,
    );
  }

  try {
    appendLineWithRotation(
      HARNESS_RUNTIME_ERROR_LOG_PATH,
      `${JSON.stringify(entry)}\n`,
      HARNESS_RUNTIME_ERROR_LOG_MAX_BYTES,
    );
  } catch (logErr) {
    const logNormalized = normalizeError(logErr);
    console.error(
      `[harness:${source}] failed to append runtime error log: ${logNormalized.message}`,
    );
  }

  console.error(
    `[harness:${source}] ${entry.message}`,
    entry.stack ? `\n${entry.stack}` : '',
    entry.context ? `\ncontext=${JSON.stringify(entry.context)}` : '',
  );
  return entry;
}

function getRecentRuntimeErrors(
  limit = HARNESS_RUNTIME_ERROR_HISTORY_LIMIT,
): RuntimeErrorEntry[] {
  const safeLimit = Math.max(
    0,
    Math.min(limit, HARNESS_RUNTIME_ERROR_HISTORY_LIMIT),
  );
  return runtimeErrors.slice(Math.max(0, runtimeErrors.length - safeLimit));
}

function marketHasReconciliationDrift(
  market: ReconciliationMarketDrift,
): boolean {
  return (
    market.replay_open_orders !== market.onchain_open_orders ||
    market.replay_best_bid !== market.onchain_best_bid ||
    market.replay_best_ask !== market.onchain_best_ask ||
    market.bid_base_lots_abs_diff !== '0' ||
    market.ask_base_lots_abs_diff !== '0'
  );
}

function trimReconciliationSnapshotToDrift(
  snapshot: ReconciliationSnapshot,
): ReconciliationSnapshot {
  return {
    ...snapshot,
    markets: Object.fromEntries(
      Object.entries(snapshot.markets).filter(([, market]) =>
        marketHasReconciliationDrift(market),
      ),
    ),
  };
}

function recordDiscrepancy(entry: DiscrepancyLogEntry): void {
  try {
    appendLineWithRotation(
      HARNESS_DISCREPANCY_LOG_PATH,
      `${JSON.stringify(entry)}\n`,
      HARNESS_DISCREPANCY_LOG_MAX_BYTES,
    );
  } catch (err) {
    recordRuntimeError('append_discrepancy_log', err, {
      reason: entry.reason,
    });
  }
}

function recordLatencySample(
  component: string,
  durationMs: number,
  options: {
    thresholdMs?: number;
    context?: Record<string, string | number | boolean | null | undefined>;
    outcome?: 'slow' | 'error';
    detail?: string | null;
    forceRecord?: boolean;
  } = {},
): void {
  const thresholdMs = Math.max(
    0,
    options.thresholdMs ?? HARNESS_SLOW_COMPONENT_MS,
  );
  const now = Date.now();
  const aggregate = runtimeLatencyAggregates.get(component) || {
    component,
    samples: 0,
    slow_samples: 0,
    error_samples: 0,
    total_ms: 0,
    max_ms: 0,
    last_ms: 0,
    last_ts_ms: 0,
  };

  aggregate.samples += 1;
  aggregate.total_ms += durationMs;
  aggregate.max_ms = Math.max(aggregate.max_ms, durationMs);
  aggregate.last_ms = durationMs;
  aggregate.last_ts_ms = now;
  totalLatencySamples += 1;

  const outcome =
    options.outcome || (durationMs >= thresholdMs ? 'slow' : null);
  if (outcome === 'slow') {
    aggregate.slow_samples += 1;
    totalSlowLatencySamples += 1;
  } else if (outcome === 'error') {
    aggregate.error_samples += 1;
    totalLatencyErrorSamples += 1;
  }
  runtimeLatencyAggregates.set(component, aggregate);

  if (!options.forceRecord && !outcome) {
    return;
  }

  const entry: RuntimeLatencyEntry = {
    id: nextRuntimeLatencySeq++,
    ts_ms: now,
    component,
    duration_ms: Number(durationMs.toFixed(3)),
    threshold_ms: thresholdMs,
    outcome: outcome || 'slow',
    detail: options.detail ? truncateForLog(options.detail, 4_000) : null,
    context: sanitizeLogContext(options.context),
  };

  runtimeLatencyHistory.push(entry);
  if (runtimeLatencyHistory.length > HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT) {
    runtimeLatencyHistory.splice(
      0,
      runtimeLatencyHistory.length - HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT,
    );
  }

  try {
    appendLineWithRotation(
      HARNESS_RUNTIME_LATENCY_LOG_PATH,
      `${JSON.stringify(entry)}\n`,
      HARNESS_RUNTIME_LATENCY_LOG_MAX_BYTES,
    );
  } catch (err) {
    const normalized = normalizeError(err);
    console.error(
      `[harness:${component}] failed to append latency log: ${normalized.message}`,
    );
  }

  console.warn(
    `[harness:${component}] ${
      entry.outcome
    } duration=${entry.duration_ms.toFixed(3)}ms threshold=${thresholdMs}ms`,
    entry.detail ? `detail=${entry.detail}` : '',
    entry.context ? `context=${JSON.stringify(entry.context)}` : '',
  );
}

function getRecentLatencyEntries(
  limit = HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT,
): RuntimeLatencyEntry[] {
  const safeLimit = Math.max(
    0,
    Math.min(limit, HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT),
  );
  return runtimeLatencyHistory.slice(
    Math.max(0, runtimeLatencyHistory.length - safeLimit),
  );
}

function getLatencyAggregates(limit = 25): RuntimeLatencyAggregate[] {
  return Array.from(runtimeLatencyAggregates.values())
    .sort((a, b) => {
      if (b.max_ms !== a.max_ms) {
        return b.max_ms - a.max_ms;
      }
      if (b.slow_samples !== a.slow_samples) {
        return b.slow_samples - a.slow_samples;
      }
      return b.samples - a.samples;
    })
    .slice(0, Math.max(0, limit))
    .map((entry) => ({
      ...entry,
      total_ms: Number(entry.total_ms.toFixed(3)),
      max_ms: Number(entry.max_ms.toFixed(3)),
      last_ms: Number(entry.last_ms.toFixed(3)),
    }));
}

function measureSync<T>(
  component: string,
  fn: () => T,
  options: {
    thresholdMs?: number;
    context?: Record<string, string | number | boolean | null | undefined>;
  } = {},
): T {
  const startedAt = performance.now();
  try {
    const result = fn();
    recordLatencySample(component, performance.now() - startedAt, {
      thresholdMs: options.thresholdMs,
      context: options.context,
    });
    return result;
  } catch (err) {
    const normalized = normalizeError(err);
    recordLatencySample(component, performance.now() - startedAt, {
      thresholdMs: options.thresholdMs,
      context: options.context,
      outcome: 'error',
      detail: normalized.message,
      forceRecord: true,
    });
    throw err;
  }
}

async function measureAsync<T>(
  component: string,
  fn: () => Promise<T>,
  options: {
    thresholdMs?: number;
    context?: Record<string, string | number | boolean | null | undefined>;
  } = {},
): Promise<T> {
  const startedAt = performance.now();
  try {
    const result = await fn();
    recordLatencySample(component, performance.now() - startedAt, {
      thresholdMs: options.thresholdMs,
      context: options.context,
    });
    return result;
  } catch (err) {
    const normalized = normalizeError(err);
    recordLatencySample(component, performance.now() - startedAt, {
      thresholdMs: options.thresholdMs,
      context: options.context,
      outcome: 'error',
      detail: normalized.message,
      forceRecord: true,
    });
    throw err;
  }
}

function histogramNsToMs(value: number): number {
  return Number.isFinite(value) && value >= 0
    ? Number((value / 1_000_000).toFixed(3))
    : 0;
}

function sampleEventLoopLag(): void {
  const snapshot: EventLoopLagSnapshot = {
    ts_ms: Date.now(),
    min_ms: histogramNsToMs(eventLoopDelayMonitor.min),
    mean_ms: histogramNsToMs(eventLoopDelayMonitor.mean),
    max_ms: histogramNsToMs(eventLoopDelayMonitor.max),
    p50_ms: histogramNsToMs(eventLoopDelayMonitor.percentile(50)),
    p95_ms: histogramNsToMs(eventLoopDelayMonitor.percentile(95)),
    p99_ms: histogramNsToMs(eventLoopDelayMonitor.percentile(99)),
    stddev_ms: histogramNsToMs(eventLoopDelayMonitor.stddev),
    exceeds_threshold: false,
  };
  snapshot.exceeds_threshold =
    snapshot.max_ms >= HARNESS_EVENT_LOOP_LAG_WARN_MS ||
    snapshot.p99_ms >= HARNESS_EVENT_LOOP_LAG_WARN_MS;
  lastEventLoopLagSnapshot = snapshot;
  recordLatencySample(
    'event_loop',
    Math.max(snapshot.p99_ms, snapshot.max_ms),
    {
      thresholdMs: HARNESS_EVENT_LOOP_LAG_WARN_MS,
      context: {
        mean_ms: snapshot.mean_ms,
        max_ms: snapshot.max_ms,
        p95_ms: snapshot.p95_ms,
        p99_ms: snapshot.p99_ms,
        stddev_ms: snapshot.stddev_ms,
      },
    },
  );
  eventLoopDelayMonitor.reset();
}

function currentSseClientCount(): number {
  return (
    sseClients.size +
    tradeStreamSubscribers.size +
    frontendStreamSubscribers.size
  );
}

function summarizeBackendCallArgs(
  method: string,
  args: unknown[],
): Record<string, string | number | boolean | null | undefined> | undefined {
  switch (method) {
    case 'bootstrapFromOnchainSnapshot': {
      const snapshot = args[0] as EngineSnapshot | undefined;
      return snapshot
        ? {
            view: snapshot.view,
            markets: Object.keys(snapshot.markets || {}).length,
            users: Object.keys(snapshot.users || {}).length,
            accounts: Object.keys(snapshot.accounts || {}).length,
          }
        : undefined;
    }
    case 'getSnapshot':
    case 'listDivergences':
      return { view: String(args[0] ?? '') };
    case 'getUserState':
    case 'getBalances':
      return { owner: String(args[0] ?? ''), view: String(args[1] ?? '') };
    case 'getMarketState':
    case 'getQueueState':
      return { market: String(args[0] ?? ''), view: String(args[1] ?? '') };
    case 'getOrders':
      return {
        market: String(args[0] ?? ''),
        owner: args[1] == null ? 'all' : String(args[1]),
        view: String(args[2] ?? ''),
      };
    case 'getTrades':
      return {
        market: String(args[0] ?? ''),
        view: String(args[1] ?? ''),
        limit: Number(args[2] ?? 0),
      };
    case 'getTradesFiltered': {
      const params = (args[0] || {}) as {
        market?: string | null;
        owner?: string | null;
        view?: QueueView;
        limit?: number;
      };
      return {
        market: params.market || 'all',
        owner: params.owner || 'all',
        view: params.view || 'unknown',
        limit: params.limit ?? 0,
      };
    }
    case 'getCandles':
      return {
        market: String(args[0] ?? ''),
        view: String(args[1] ?? ''),
        resolution_sec: Number(args[2] ?? 0),
        limit: Number(args[3] ?? 0),
      };
    case 'findIntent':
      return {
        group: String(args[0] ?? ''),
        sequence: String(args[1] ?? ''),
        kind: String(args[2] ?? ''),
      };
    case 'ingestRelayIntent': {
      const event = args[0] as RelayIntentAcceptedEvent | undefined;
      return event
        ? {
            event_type: event.event_type,
            market: event.market,
            sequence: event.sequence,
            kind: event.kind,
          }
        : undefined;
    }
    case 'ingestQueueEnqueued':
    case 'ingestQueueProcessed': {
      const event = (args[0] || {}) as {
        event_type?: string;
        market?: string;
        sequence?: string;
        slot?: string;
        status?: string | number;
      };
      return {
        event_type: event.event_type || method,
        market: event.market || 'unknown',
        sequence: event.sequence || 'unknown',
        slot: event.slot || 'unknown',
        status: event.status == null ? 'n/a' : String(event.status),
      };
    }
    case 'reportExternalDivergence':
      return {
        reason: String(args[0] ?? ''),
        key: String(args[1] ?? ''),
      };
    default:
      return { arg_count: args.length };
  }
}

function instrumentBackend(
  backend: ContinuumHarnessBackend,
): ContinuumHarnessBackend {
  const wrappedSubscribe = (
    listener: (event: HarnessEvent) => void,
  ): (() => void) =>
    measureSync(
      'backend.subscribe',
      () =>
        backend.subscribe((event) =>
          measureSync('backend.listener', () => listener(event), {
            thresholdMs: HARNESS_SLOW_COMPONENT_MS,
            context: {
              event_type: event.event_type,
            },
          }),
        ),
      { thresholdMs: HARNESS_SLOW_BACKEND_CALL_MS },
    );

  return new Proxy(
    backend as ContinuumHarnessBackend & Record<string, unknown>,
    {
      get(target, prop, receiver) {
        if (prop === 'subscribe') {
          return wrappedSubscribe;
        }
        const value = Reflect.get(target, prop, receiver);
        if (typeof value !== 'function') {
          return value;
        }
        return (...args: unknown[]) =>
          measureSync(
            `backend.${String(prop)}`,
            () =>
              (value as (...callArgs: unknown[]) => unknown).apply(
                target,
                args,
              ),
            {
              thresholdMs: HARNESS_SLOW_BACKEND_CALL_MS,
              context: summarizeBackendCallArgs(String(prop), args),
            },
          );
      },
    },
  ) as ContinuumHarnessBackend;
}

function isWritableResponse(res: ServerResponse): boolean {
  const asAny = res as ServerResponse & { closed?: boolean };
  return !res.writableEnded && !res.destroyed && !asAny.closed;
}

function safeWriteResponse(
  res: ServerResponse,
  payload: string,
  source: string,
  context?: Record<string, string | number | boolean | null | undefined>,
): boolean {
  if (!isWritableResponse(res)) {
    return false;
  }
  try {
    res.write(payload);
    return true;
  } catch (err) {
    recordRuntimeError(source, err, context);
    return false;
  }
}

function attachStreamCleanup(
  req: IncomingMessage,
  res: ServerResponse,
  cleanup: () => void,
): void {
  let cleaned = false;
  const wrapped = () => {
    if (cleaned) {
      return;
    }
    cleaned = true;
    cleanup();
  };
  req.on('close', wrapped);
  req.on('aborted', wrapped);
  req.on('error', () => wrapped());
  res.on('close', wrapped);
  res.on('finish', wrapped);
  res.on('error', () => wrapped());
}

function writeJson(res: ServerResponse, status: number, body: unknown): void {
  if (res.headersSent) return; // Guard against double-response crashes
  res.statusCode = status;
  res.setHeader('Content-Type', 'application/json');
  res.end(JSON.stringify(body));
}

function writeText(res: ServerResponse, status: number, body: string): void {
  if (res.headersSent) return;
  res.statusCode = status;
  res.setHeader('Content-Type', 'text/plain; charset=utf-8');
  res.end(body);
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > REQUEST_BODY_MAX_BYTES) {
        reject(new Error('request body too large'));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf-8')));
    req.on('error', reject);
  });
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parsePathAndQuery(req: IncomingMessage): URL {
  return new URL(req.url || '/', 'http://127.0.0.1');
}

function parseView(url: URL): QueueView {
  const view = (url.searchParams.get('view') || 'optimistic').toLowerCase();
  return view === 'confirmed' ? 'confirmed' : 'optimistic';
}

function baseSymbolFromPerpName(name: string): string {
  const trimmed = name.trim();
  if (trimmed.endsWith('-PERP')) {
    return trimmed.slice(0, -'-PERP'.length);
  }
  if (trimmed.includes('/')) {
    return trimmed.split('/')[0].trim();
  }
  return trimmed.split(' ')[0]?.trim() || trimmed;
}

function symbolToCanonicalMint(
  symbol: string,
  usdcMint: PublicKey | null,
): string {
  switch (symbol.trim().toUpperCase()) {
    case 'SOL':
      return 'So11111111111111111111111111111111111111112';
    case 'USDC':
      return usdcMint?.toBase58() || '';
    default:
      return '';
  }
}

async function getFreshGroup(
  onchain: OnchainContext | null,
): Promise<HarnessGroup | null> {
  if (!onchain?.groupPk) {
    return null;
  }

  if (!onchain.mangoClient) {
    try {
      const provider = new AnchorProvider(
        onchain.connection,
        new Wallet(Keypair.generate()),
        AnchorProvider.defaultOptions(),
      );
      onchain.mangoClient = await MangoClient.connect(
        provider,
        CLUSTER,
        onchain.programId,
        {
          idsSource: 'get-program-accounts',
        },
      );
      onchain.cachedGroup = await onchain.mangoClient.getGroup(onchain.groupPk);
      await onchain.cachedGroup.reloadAll(onchain.mangoClient);
      onchain.cachedGroupFetchedAtMs = Date.now();
      if (!onchain.usdcMint) {
        try {
          onchain.usdcMint =
            onchain.cachedGroup.getFirstBankForPerpSettlement().mint;
        } catch {
          // Leave unset when the settlement bank cannot be resolved.
        }
      }
      console.log(
        `Onchain read context recovered: group=${onchain.groupPk.toBase58()}, usdc_mint=${
          onchain.usdcMint?.toBase58() || 'unresolved'
        }`,
      );
    } catch (err) {
      console.warn(`onchain read context recovery failed: ${err}`);
      onchain.mangoClient = null;
      onchain.cachedGroup = null;
      onchain.cachedGroupFetchedAtMs = 0;
      onchain.cachedMarketMetadata = null;
      onchain.cachedMarketMetadataFetchedAtMs = 0;
      return null;
    }
  }

  if (
    onchain.cachedGroup &&
    Date.now() - onchain.cachedGroupFetchedAtMs < HARNESS_ONCHAIN_CACHE_TTL_MS
  ) {
    return onchain.cachedGroup;
  }

  const group = await onchain.mangoClient.getGroup(onchain.groupPk);
  await group.reloadAll(onchain.mangoClient);
  onchain.cachedGroup = group;
  onchain.cachedGroupFetchedAtMs = Date.now();
  if (!onchain.usdcMint) {
    try {
      onchain.usdcMint = group.getFirstBankForPerpSettlement().mint;
    } catch {
      // Leave unset when the settlement bank cannot be resolved.
    }
  }
  return group;
}

async function getMarketMetadataMap(
  onchain: OnchainContext | null,
): Promise<Record<string, HarnessMarketMetadata>> {
  if (!onchain?.mangoClient || !onchain.groupPk) {
    return {};
  }

  if (
    onchain.cachedMarketMetadata &&
    Date.now() - onchain.cachedMarketMetadataFetchedAtMs <
      HARNESS_ONCHAIN_CACHE_TTL_MS
  ) {
    return onchain.cachedMarketMetadata;
  }

  const group = await getFreshGroup(onchain);
  if (!group) {
    return {};
  }

  const metadata: Record<string, HarnessMarketMetadata> = {};
  for (const [
    marketIndex,
    perpMarket,
  ] of group.perpMarketsMapByMarketIndex.entries()) {
    let settleBank;
    try {
      settleBank = group.getFirstBankByTokenIndex(perpMarket.settleTokenIndex);
    } catch {
      continue;
    }

    const marketKey = Number(marketIndex).toString();
    const baseSymbol = baseSymbolFromPerpName(perpMarket.name);
    metadata[marketKey] = {
      market_index: Number(marketIndex),
      name: perpMarket.name,
      base_symbol: baseSymbol,
      quote_symbol: settleBank.name,
      base_mint: symbolToCanonicalMint(baseSymbol, onchain.usdcMint),
      quote_mint: settleBank.mint.toBase58(),
      perp_market: perpMarket.publicKey.toBase58(),
      oracle: perpMarket.oracle.toBase58(),
      bids: perpMarket.bids.toBase58(),
      asks: perpMarket.asks.toBase58(),
      event_queue: perpMarket.eventQueue.toBase58(),
      base_decimals: perpMarket.baseDecimals,
      quote_decimals: settleBank.mintDecimals,
      base_lot_size: perpMarket.baseLotSize.toString(),
      quote_lot_size: perpMarket.quoteLotSize.toString(),
      open_interest: perpMarket.openInterest.toString(),
    };
  }

  onchain.cachedMarketMetadata = metadata;
  onchain.cachedMarketMetadataFetchedAtMs = Date.now();
  return metadata;
}

async function getMarketMetadataMapSafe(
  onchain: OnchainContext | null,
): Promise<Record<string, HarnessMarketMetadata>> {
  try {
    return await getMarketMetadataMap(onchain);
  } catch (err) {
    console.warn(`market metadata fetch failed: ${err}`);
    return onchain?.cachedMarketMetadata || {};
  }
}

function emptyUserState(owner: string): UserState {
  return {
    owner,
    mango_accounts: [],
    open_orders: [],
    per_market: [],
    margin_summary: {
      status: 'placeholder',
      source: 'onchain-sync',
    },
  };
}

function emptyQueueState(market: string): QueueState {
  return {
    market,
    pending_count: 0,
    processed_count: 0,
    failed_count: 0,
    skipped_count: 0,
    last_processed_sequence: '0',
    lag_slots: '0',
    unmatched_processed_count: 0,
  };
}

function baseSnapshotForView(view: QueueView): EngineSnapshot {
  return engine.getSnapshot(view);
}

function buildQueueViewForSnapshot(
  view: QueueView,
  snapshot: EngineSnapshot,
  onchainSync: OnchainSyncState,
): Record<string, QueueState> {
  const queueSnapshot = onchainSync.queue;
  if (!queueSnapshot) {
    return snapshot.queue;
  }

  const observedSlot = BigInt(queueSnapshot.observed_slot);
  const nextSequence = maybeToBigInt(queueSnapshot.next_sequence) || 0n;
  const byMarket = new Map<
    string,
    {
      sequences: Set<string>;
      pendingCount: number;
      minPendingExecuteSlot: bigint | null;
    }
  >();
  const ensureAggregate = (market: string) => {
    const existing = byMarket.get(market);
    if (existing) {
      return existing;
    }
    const created = {
      sequences: new Set<string>(),
      pendingCount: 0,
      minPendingExecuteSlot: null as bigint | null,
    };
    byMarket.set(market, created);
    return created;
  };

  for (const item of queueSnapshot.items) {
    const aggregate = ensureAggregate(item.market);
    aggregate.sequences.add(item.sequence);
    aggregate.pendingCount += 1;
    const minExecuteSlot = maybeToBigInt(item.min_execute_slot);
    if (minExecuteSlot !== null) {
      aggregate.minPendingExecuteSlot =
        aggregate.minPendingExecuteSlot === null
          ? minExecuteSlot
          : aggregate.minPendingExecuteSlot < minExecuteSlot
            ? aggregate.minPendingExecuteSlot
            : minExecuteSlot;
    }
  }

  if (view === 'optimistic') {
    for (const intent of engine.listIntents()) {
      const kind = Number(intent.kind);
      const processedStatus =
        intent.processed_status === null || intent.processed_status === undefined
          ? null
          : Number(intent.processed_status);
      if (kind !== 0 || processedStatus !== null) {
        continue;
      }
      const market = intent.market || 'unknown';
      const sequence = maybeToBigInt(String(intent.sequence));
      if (sequence === null || sequence < nextSequence) {
        continue;
      }
      const aggregate = ensureAggregate(market);
      const sequenceKey = sequence.toString();
      if (aggregate.sequences.has(sequenceKey)) {
        continue;
      }
      aggregate.sequences.add(sequenceKey);
      aggregate.pendingCount += 1;
      const minExecuteSlot = maybeToBigInt(String(intent.min_execute_slot));
      if (minExecuteSlot !== null) {
        aggregate.minPendingExecuteSlot =
          aggregate.minPendingExecuteSlot === null
            ? minExecuteSlot
            : aggregate.minPendingExecuteSlot < minExecuteSlot
              ? aggregate.minPendingExecuteSlot
              : minExecuteSlot;
      }
    }
  }

  const merged: Record<string, QueueState> = {};
  const markets = new Set<string>([
    ...Object.keys(snapshot.queue || {}),
    ...Array.from(byMarket.keys()),
  ]);
  const lastProcessedSequence =
    nextSequence > 0n ? (nextSequence - 1n).toString() : '0';
  for (const market of markets) {
    const base = snapshot.queue?.[market] || emptyQueueState(market);
    const aggregate = byMarket.get(market);
    const minPendingExecuteSlot = aggregate?.minPendingExecuteSlot || null;
    const lagSlots =
      minPendingExecuteSlot !== null && observedSlot > minPendingExecuteSlot
        ? observedSlot - minPendingExecuteSlot
        : 0n;
    merged[market] = {
      ...base,
      market,
      pending_count: aggregate?.pendingCount || 0,
      last_processed_sequence: lastProcessedSequence,
      lag_slots: lagSlots.toString(),
    };
  }

  return merged;
}

function getSnapshotForView(
  view: QueueView,
  onchainSync: OnchainSyncState,
): EngineSnapshot {
  const snapshot = baseSnapshotForView(view);
  const queue = buildQueueViewForSnapshot(view, snapshot, onchainSync);
  if (queue === snapshot.queue) {
    return snapshot;
  }
  return {
    ...snapshot,
    queue,
  };
}

function getUserStateForView(
  owner: string,
  view: QueueView,
  onchainSync: OnchainSyncState,
): UserState {
  return getSnapshotForView(view, onchainSync).users[owner] || emptyUserState(owner);
}

function getMarketStateForView(
  market: string,
  view: QueueView,
  onchainSync: OnchainSyncState,
): MarketState {
  return getSnapshotForView(view, onchainSync).markets[market] || emptyMarketState(market);
}

function getBalancesForUserState(user: UserState, view: QueueView) {
  let totalBid = 0n;
  let totalAsk = 0n;
  let totalQuoteReserved = 0n;
  for (const entry of user.per_market) {
    totalBid += BigInt(entry.open_order_base_lots_bid);
    totalAsk += BigInt(entry.open_order_base_lots_ask);
    totalQuoteReserved += BigInt(entry.quote_reserved_lots);
  }

  return {
    owner: user.owner,
    mango_accounts: user.mango_accounts,
    per_market: user.per_market,
    totals: {
      total_open_order_base_lots_bid: totalBid.toString(),
      total_open_order_base_lots_ask: totalAsk.toString(),
      total_quote_reserved_lots: totalQuoteReserved.toString(),
    },
    margin_summary: user.margin_summary,
    view,
  };
}

function getBalancesForView(
  owner: string,
  view: QueueView,
  onchainSync: OnchainSyncState,
) {
  return getBalancesForUserState(
    getUserStateForView(owner, view, onchainSync),
    view,
  );
}

function nextStreamSubscriberId(): string {
  const id = nextStreamSubscriberSeq;
  nextStreamSubscriberSeq += 1;
  return `${id}`;
}

function parseNonNegativeInteger(
  raw: string | null,
  fallback: number,
  { min = 0, max }: { min?: number; max?: number } = {},
): number {
  const parsed = raw === null ? fallback : Number(raw);
  if (!Number.isFinite(parsed)) {
    return fallback;
  }
  let next = Math.floor(parsed);
  if (next < min) {
    next = min;
  }
  if (max !== undefined && next > max) {
    next = max;
  }
  return next;
}

function parseCommaSeparatedList(raw: string | null): string[] | null {
  if (!raw) {
    return null;
  }
  const items = raw
    .split(',')
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
  return items.length ? items : null;
}

function parseIncludeSet(url: URL, defaults: string[]): Set<string> {
  return new Set(
    parseCommaSeparatedList(url.searchParams.get('include')) || defaults,
  );
}

function emptyMarketState(market: string): EngineSnapshot['markets'][string] {
  return {
    market,
    bids: [],
    asks: [],
    open_orders: [],
    watermarks: {
      optimistic_seq: '0',
      confirmed_seq: '0',
      last_slot: '0',
    },
  };
}

function maybeToBigInt(raw: string | null | undefined): bigint | null {
  if (!raw || !raw.length) {
    return null;
  }
  try {
    return BigInt(raw);
  } catch {
    return null;
  }
}

function priceLotsToUi(
  priceLots: string | null,
  metadata: HarnessMarketMetadata | null,
): number | null {
  if (!priceLots || !metadata) {
    return null;
  }
  const priceLotsBig = maybeToBigInt(priceLots);
  const baseLotSize = maybeToBigInt(metadata.base_lot_size);
  const quoteLotSize = maybeToBigInt(metadata.quote_lot_size);
  if (
    priceLotsBig === null ||
    baseLotSize === null ||
    quoteLotSize === null ||
    baseLotSize === 0n
  ) {
    return null;
  }
  const scalar =
    (Number(quoteLotSize) * Math.pow(10, metadata.base_decimals)) /
    (Number(baseLotSize) * Math.pow(10, metadata.quote_decimals));
  return Number(priceLotsBig) * scalar;
}

function baseLotsToUi(
  baseLots: string | null,
  metadata: HarnessMarketMetadata | null,
): number | null {
  if (!baseLots || !metadata) {
    return null;
  }
  const baseLotsBig = maybeToBigInt(baseLots);
  const baseLotSize = maybeToBigInt(metadata.base_lot_size);
  if (baseLotsBig === null || baseLotSize === null) {
    return null;
  }
  return (
    (Number(baseLotsBig) * Number(baseLotSize)) /
    Math.pow(10, metadata.base_decimals)
  );
}

function quoteLotsToUi(
  quoteLots: string | null,
  metadata: HarnessMarketMetadata | null,
): number | null {
  if (!quoteLots || !metadata) {
    return null;
  }
  const quoteLotsBig = maybeToBigInt(quoteLots);
  const quoteLotSize = maybeToBigInt(metadata.quote_lot_size);
  if (quoteLotsBig === null || quoteLotSize === null) {
    return null;
  }
  return (
    (Number(quoteLotsBig) * Number(quoteLotSize)) /
    Math.pow(10, metadata.quote_decimals)
  );
}

function buildOrderbookLevels(
  levels: Array<{ price_lots: string; base_lots: string }>,
  metadata: HarnessMarketMetadata | null,
  depth: number,
): OrderbookLevelView[] {
  return levels.slice(0, depth).map((level) => ({
    price_lots: level.price_lots,
    base_lots: level.base_lots,
    price_ui: priceLotsToUi(level.price_lots, metadata),
    qty_ui: baseLotsToUi(level.base_lots, metadata),
  }));
}

function buildOrderbookSummary(
  marketState: EngineSnapshot['markets'][string] | null,
  metadata: HarnessMarketMetadata | null,
  depth: number,
): OrderbookSummaryView {
  const safeDepth = Math.max(1, depth);
  return {
    depth: safeDepth,
    bids: buildOrderbookLevels(marketState?.bids || [], metadata, safeDepth),
    asks: buildOrderbookLevels(marketState?.asks || [], metadata, safeDepth),
  };
}

function buildTradeSummary(
  market: string,
  view: QueueView,
  trades: MarketTrade[],
  metadata: HarnessMarketMetadata | null,
  windowMs = TRADE_SUMMARY_WINDOW_MS,
): MarketTradeSummary {
  const now = Date.now();
  const windowTrades = trades.filter((trade) => trade.ts_ms >= now - windowMs);
  const sourceTrades = windowTrades.length ? windowTrades : trades;
  const lastTrade = trades[trades.length - 1] || null;
  const openTrade = sourceTrades[0] || null;
  let highPrice: bigint | null = null;
  let lowPrice: bigint | null = null;
  let volumeBaseLots = 0n;
  let volumeQuoteLots = 0n;

  for (const trade of sourceTrades) {
    const priceLots = BigInt(trade.price_lots);
    if (highPrice === null || priceLots > highPrice) {
      highPrice = priceLots;
    }
    if (lowPrice === null || priceLots < lowPrice) {
      lowPrice = priceLots;
    }
    volumeBaseLots += BigInt(trade.base_lots);
    volumeQuoteLots += BigInt(trade.quote_lots);
  }

  const openPriceLots = openTrade?.price_lots || null;
  const lastPriceLots = lastTrade?.price_lots || null;
  const openPriceUi = priceLotsToUi(openPriceLots, metadata);
  const lastPriceUi = priceLotsToUi(lastPriceLots, metadata);
  const change24hPct =
    openPriceUi !== null && lastPriceUi !== null && openPriceUi !== 0
      ? ((lastPriceUi - openPriceUi) / openPriceUi) * 100
      : null;

  return {
    market,
    view,
    window_ms: windowMs,
    trade_count: sourceTrades.length,
    last_trade_ts_ms: lastTrade?.ts_ms || null,
    last_price_lots: lastPriceLots,
    last_price_ui: lastPriceUi,
    open_price_lots: openPriceLots,
    open_price_ui: openPriceUi,
    high_price_lots: highPrice?.toString() || null,
    high_price_ui: priceLotsToUi(highPrice?.toString() || null, metadata),
    low_price_lots: lowPrice?.toString() || null,
    low_price_ui: priceLotsToUi(lowPrice?.toString() || null, metadata),
    change_24h_pct: change24hPct,
    volume_base_lots: volumeBaseLots.toString(),
    volume_quote_lots: volumeQuoteLots.toString(),
    volume_base_ui: baseLotsToUi(volumeBaseLots.toString(), metadata),
    volume_quote_ui: quoteLotsToUi(volumeQuoteLots.toString(), metadata),
  };
}

function bucketTradesByMarket(
  trades: MarketTrade[],
): Map<string, MarketTrade[]> {
  const buckets = new Map<string, MarketTrade[]>();
  for (const trade of trades) {
    const bucket = buckets.get(trade.market);
    if (bucket) {
      bucket.push(trade);
    } else {
      buckets.set(trade.market, [trade]);
    }
  }
  return buckets;
}

function buildStubbedAccountMetrics(): StubbedAccountMetrics {
  return {
    status: 'stub',
    source: 'pending-subtree',
    updated_ts_ms: 0,
    fields: {
      margin_used: null,
      health_init: null,
      health_maint: null,
      pnl_realized: null,
      pnl_unrealized: null,
      equity: null,
      liquidation_price_by_market: null,
    },
  };
}

function buildFrontendAccountMetrics(
  marginSummary: MarginSummary,
  mangoAccount: string | null,
): FrontendAccountMetrics {
  if (marginSummary.status === 'placeholder') {
    return buildStubbedAccountMetrics();
  }

  if (marginSummary.status === 'empty') {
    return {
      status: 'empty',
      source: marginSummary.source,
      updated_ts_ms: Date.now(),
      account_count: 0,
      mango_account: mangoAccount,
      totals: marginSummary.totals,
      accounts: [],
      fields: {
        margin_used: marginSummary.totals.margin_usage_fraction,
        health_init: marginSummary.totals.init_health_native_quote,
        health_maint: marginSummary.totals.maint_health_native_quote,
        pnl_realized: null,
        pnl_unrealized: marginSummary.totals.pnl_native_quote,
        equity: marginSummary.totals.equity_native_quote,
        liquidation_price_by_market: null,
      },
    };
  }

  const selectedAccounts = mangoAccount
    ? marginSummary.accounts.filter(
        (account) => account.mango_account === mangoAccount,
      )
    : marginSummary.accounts;
  const totals =
    selectedAccounts.length === marginSummary.accounts.length
      ? marginSummary.totals
      : aggregateMarginSummaryAccounts(selectedAccounts);
  const selectedAccount = selectedAccounts[0] || null;

  return {
    status: 'ok',
    source: marginSummary.source,
    updated_ts_ms: Date.now(),
    account_count: selectedAccounts.length,
    mango_account: mangoAccount,
    totals,
    accounts: selectedAccounts,
    fields: {
      margin_used: totals.margin_usage_fraction,
      health_init:
        selectedAccount?.init_health_native_quote ||
        totals.init_health_native_quote,
      health_maint:
        selectedAccount?.maint_health_native_quote ||
        totals.maint_health_native_quote,
      pnl_realized: null,
      pnl_unrealized:
        selectedAccount?.pnl_native_quote || totals.pnl_native_quote,
      equity:
        selectedAccount?.equity_native_quote || totals.equity_native_quote,
      liquidation_price_by_market: null,
    },
  };
}

function aggregateMarginSummaryAccounts(accounts: MarginSummaryAccount[]) {
  const totals = {
    equity_native_quote: ZERO_I80F48(),
    pnl_native_quote: ZERO_I80F48(),
    assets_native_quote: ZERO_I80F48(),
    liabs_native_quote: ZERO_I80F48(),
    init_health_native_quote: ZERO_I80F48(),
    maint_health_native_quote: ZERO_I80F48(),
  };

  for (const account of accounts) {
    totals.equity_native_quote.iadd(
      I80F48.fromString(account.equity_native_quote),
    );
    totals.pnl_native_quote.iadd(I80F48.fromString(account.pnl_native_quote));
    totals.assets_native_quote.iadd(
      I80F48.fromString(account.assets_native_quote),
    );
    totals.liabs_native_quote.iadd(
      I80F48.fromString(account.liabs_native_quote),
    );
    totals.init_health_native_quote.iadd(
      I80F48.fromString(account.init_health_native_quote),
    );
    totals.maint_health_native_quote.iadd(
      I80F48.fromString(account.maint_health_native_quote),
    );
  }

  const assets = totals.assets_native_quote.toNumber();
  const liabs = totals.liabs_native_quote.toNumber();
  return {
    equity_native_quote: totals.equity_native_quote.toString(),
    pnl_native_quote: totals.pnl_native_quote.toString(),
    assets_native_quote: totals.assets_native_quote.toString(),
    liabs_native_quote: totals.liabs_native_quote.toString(),
    init_health_native_quote: totals.init_health_native_quote.toString(),
    maint_health_native_quote: totals.maint_health_native_quote.toString(),
    margin_usage_fraction: assets > 0 ? liabs / assets : 0,
  };
}

function getImpactPriceUiFromLevels(
  levels: Array<{ price_lots: string; base_lots: string }>,
  impactBaseLots: BN,
  metadata: HarnessMarketMetadata | null,
): number | null {
  let accumulated = 0n;
  const target = BigInt(impactBaseLots.toString());
  for (const level of levels) {
    accumulated += BigInt(level.base_lots);
    if (accumulated >= target) {
      return priceLotsToUi(level.price_lots, metadata);
    }
  }
  return null;
}

function computeFundingRateDailyPct(
  oraclePriceUi: number | null,
  bidImpactUi: number | null,
  askImpactUi: number | null,
  minFunding: number,
  maxFunding: number,
): number | null {
  if (oraclePriceUi === null || oraclePriceUi === 0) {
    return null;
  }
  let funding: number;
  if (bidImpactUi !== null && askImpactUi !== null) {
    const bookPrice = (bidImpactUi + askImpactUi) / 2;
    funding = Math.min(
      Math.max(bookPrice / oraclePriceUi - 1, minFunding),
      maxFunding,
    );
  } else if (bidImpactUi !== null) {
    funding = maxFunding;
  } else if (askImpactUi !== null) {
    funding = minFunding;
  } else {
    funding = 0;
  }
  return funding * 100;
}

async function getMarketRuntimeMetrics(
  market: string,
  marketState: EngineSnapshot['markets'][string] | null,
  metadata: HarnessMarketMetadata | null,
  onchain: OnchainContext | null,
): Promise<MarketRuntimeMetrics | null> {
  const cached = marketRuntimeMetricsCache.get(market);
  if (
    cached &&
    Date.now() - cached.fetchedAtMs < HARNESS_ONCHAIN_CACHE_TTL_MS
  ) {
    return cached.data;
  }

  const group = await getFreshGroup(onchain);
  if (!group) {
    marketRuntimeMetricsCache.set(market, {
      fetchedAtMs: Date.now(),
      data: null,
    });
    return null;
  }

  const marketIndex = Number(market);
  if (!Number.isInteger(marketIndex)) {
    marketRuntimeMetricsCache.set(market, {
      fetchedAtMs: Date.now(),
      data: null,
    });
    return null;
  }

  const perpMarket = group.perpMarketsMapByMarketIndex.get(
    marketIndex as never,
  );
  if (!perpMarket) {
    marketRuntimeMetricsCache.set(market, {
      fetchedAtMs: Date.now(),
      data: null,
    });
    return null;
  }

  const bestBidUi = priceLotsToUi(
    marketState?.bids?.[0]?.price_lots || null,
    metadata,
  );
  const bestAskUi = priceLotsToUi(
    marketState?.asks?.[0]?.price_lots || null,
    metadata,
  );
  const bidImpactUi = getImpactPriceUiFromLevels(
    marketState?.bids || [],
    perpMarket.impactQuantity,
    metadata,
  );
  const askImpactUi = getImpactPriceUiFromLevels(
    marketState?.asks || [],
    perpMarket.impactQuantity,
    metadata,
  );
  let oraclePriceUi: number | null = null;
  try {
    oraclePriceUi = Number.isFinite(perpMarket.uiPrice)
      ? perpMarket.uiPrice
      : null;
  } catch {
    oraclePriceUi = null;
  }
  const markPriceUi =
    bestBidUi !== null && bestAskUi !== null
      ? (bestBidUi + bestAskUi) / 2
      : oraclePriceUi;
  const fundingRateDailyPct = computeFundingRateDailyPct(
    oraclePriceUi,
    bidImpactUi,
    askImpactUi,
    perpMarket.minFunding.toNumber(),
    perpMarket.maxFunding.toNumber(),
  );
  const data: MarketRuntimeMetrics = {
    market,
    oracle_price_ui: oraclePriceUi,
    mark_price_ui: markPriceUi,
    funding_rate_daily_pct: fundingRateDailyPct,
    funding_rate_hourly_pct:
      fundingRateDailyPct === null ? null : fundingRateDailyPct / 24,
    open_interest_base_lots: perpMarket.openInterest.toString(),
    open_interest_base_ui: perpMarket.baseLotsToUi(perpMarket.openInterest),
    best_bid_ui: bestBidUi,
    best_ask_ui: bestAskUi,
    updated_ts_ms: Date.now(),
  };
  marketRuntimeMetricsCache.set(market, {
    fetchedAtMs: data.updated_ts_ms,
    data,
  });
  return data;
}

function resolveOwnerFromSnapshot(
  snapshot: EngineSnapshot,
  owner: string | null,
  mangoAccount: string | null,
): string | null {
  if (owner) {
    return owner;
  }
  if (!mangoAccount) {
    return null;
  }
  const projectedOwner = snapshot.accounts?.[mangoAccount]?.owner;
  if (projectedOwner) {
    return projectedOwner;
  }
  for (const [candidateOwner, user] of Object.entries(snapshot.users)) {
    if (user.mango_accounts.includes(mangoAccount)) {
      return candidateOwner;
    }
  }
  return null;
}

async function buildFrontendOwnerSlice(
  snapshot: EngineSnapshot,
  owner: string,
  mangoAccount: string | null,
  market: string | null,
  view: QueueView,
  tradesLimit: number,
  onchain: OnchainContext | null,
  trades?: MarketTrade[],
): Promise<FrontendOwnerSlice> {
  const baseUserState = snapshot.users[owner] || emptyUserState(owner);
  const userState =
    HARNESS_BACKEND === 'rust-backend'
      ? baseUserState
      : ((await enrichOwnerStateWithOnchain(
          owner,
          baseUserState,
          onchain,
          view,
        )) as UserState);
  return {
    owner,
    mango_account: mangoAccount,
    view,
    positions_scope: 'owner_aggregate',
    positions: userState.per_market.filter((position) => {
      return !market || position.market === market;
    }),
    open_orders: userState.open_orders.filter((order) => {
      return (
        (!market || order.market === market) &&
        (!mangoAccount || order.mango_account === mangoAccount)
      );
    }),
    trades:
      trades ||
      engine.getTradesFiltered({
        view,
        owner,
        market,
        limit: tradesLimit,
      }),
    account_metrics: buildFrontendAccountMetrics(
      userState.margin_summary,
      mangoAccount,
    ),
  };
}

async function buildFrontendMarketSlice(
  market: string,
  marketState: EngineSnapshot['markets'][string] | null,
  metadata: HarnessMarketMetadata | null,
  onchain: OnchainContext | null,
  view: QueueView,
  depth: number,
  orderbookMode: 'summary' | 'full',
  trades?: MarketTrade[],
): Promise<FrontendMarketSlice> {
  const data = marketState || emptyMarketState(market);
  return {
    market,
    view,
    metadata,
    metrics: await getMarketRuntimeMetrics(market, data, metadata, onchain),
    trade_summary: buildTradeSummary(
      market,
      view,
      trades || engine.getTrades(market, view, 5000),
      metadata,
    ),
    orderbook_summary: buildOrderbookSummary(data, metadata, depth),
    orderbook: orderbookMode === 'full' ? data : null,
  };
}

async function buildMarketListItems(
  snapshot: EngineSnapshot,
  onchain: OnchainContext | null,
  view: QueueView,
  marketsFilter: string[] | null,
  depth: number,
  includeFullBook: boolean,
): Promise<MarketListItem[]> {
  const metadataMap = await getMarketMetadataMapSafe(onchain);
  const tradesByMarket = bucketTradesByMarket(engine.getAllTrades(view, 5000));
  const marketIds = new Set<string>([
    ...Object.keys(snapshot.markets),
    ...Object.keys(metadataMap),
    ...(marketsFilter || []),
  ]);
  const selectedMarkets = Array.from(marketIds)
    .filter((market) => !marketsFilter || marketsFilter.includes(market))
    .sort((a, b) => Number(a) - Number(b));

  return await Promise.all(
    selectedMarkets.map(async (market) => {
      const metadata = metadataMap[market] || null;
      const data = snapshot.markets[market] || emptyMarketState(market);
      return {
        market,
        view,
        metadata,
        data: includeFullBook
          ? data
          : {
              ...data,
              bids: [],
              asks: [],
            },
        orderbook_summary: buildOrderbookSummary(data, metadata, depth),
        trade_summary: buildTradeSummary(
          market,
          view,
          tradesByMarket.get(market) || [],
          metadata,
        ),
        metrics: await getMarketRuntimeMetrics(market, data, metadata, onchain),
      };
    }),
  );
}

function buildTradeSummaryCollection(
  view: QueueView,
  snapshot: EngineSnapshot,
  metadataMap: Record<string, HarnessMarketMetadata>,
  market: string | null,
  owner: string | null,
): MarketTradeSummary[] {
  if (market) {
    return [
      buildTradeSummary(
        market,
        view,
        engine.getTradesFiltered({ view, market, owner, limit: 5000 }),
        metadataMap[market] || null,
      ),
    ];
  }

  const filteredTrades = owner
    ? engine.getTradesFiltered({ view, owner, limit: 5000 })
    : engine.getAllTrades(view, 5000);
  const tradesByMarket = bucketTradesByMarket(filteredTrades);
  const candidateMarkets = new Set<string>([
    ...Object.keys(metadataMap),
    ...Object.keys(snapshot.markets),
    ...tradesByMarket.keys(),
  ]);
  return Array.from(candidateMarkets)
    .sort((a, b) => Number(a) - Number(b))
    .map((marketId) =>
      buildTradeSummary(
        marketId,
        view,
        tradesByMarket.get(marketId) || [],
        metadataMap[marketId] || null,
      ),
    );
}

function aggregateDepth(
  levels: Array<{ price_lots: string; base_lots: string }>,
): Map<string, bigint> {
  const out = new Map<string, bigint>();
  for (const level of levels) {
    const current = out.get(level.price_lots) || 0n;
    out.set(level.price_lots, current + BigInt(level.base_lots));
  }
  return out;
}

function absDiffAcrossLevels(
  lhs: Map<string, bigint>,
  rhs: Map<string, bigint>,
): bigint {
  let total = 0n;
  const keys = new Set<string>([...lhs.keys(), ...rhs.keys()]);
  for (const key of keys) {
    const diff = (lhs.get(key) || 0n) - (rhs.get(key) || 0n);
    total += diff < 0n ? -diff : diff;
  }
  return total;
}

function decimalStringToBigintTrunc(value: string): bigint {
  const trimmed = value.trim();
  if (!trimmed.length) {
    return 0n;
  }
  const negative = trimmed.startsWith('-');
  const unsigned = negative ? trimmed.slice(1) : trimmed;
  const whole = unsigned.split('.')[0] || '0';
  const parsed = BigInt(whole);
  return negative ? -parsed : parsed;
}

function buildReconciliationSnapshot(
  replay: EngineSnapshot,
  onchainSnapshot: EngineSnapshot,
): ReconciliationSnapshot {
  const marketKeys = new Set<string>([
    ...Object.keys(replay.markets),
    ...Object.keys(onchainSnapshot.markets),
  ]);
  const markets: Record<string, ReconciliationMarketDrift> = {};
  let totalReplayOpenOrders = 0;
  let totalOnchainOpenOrders = 0;
  let totalBidAbsDiff = 0n;
  let totalAskAbsDiff = 0n;
  let marketsWithDrift = 0;

  for (const market of marketKeys) {
    const replayMarket = replay.markets[market];
    const onchainMarket = onchainSnapshot.markets[market];
    const replayOrders = replayMarket?.open_orders.length || 0;
    const onchainOrders = onchainMarket?.open_orders.length || 0;
    const replayBids = aggregateDepth(replayMarket?.bids || []);
    const onchainBids = aggregateDepth(onchainMarket?.bids || []);
    const replayAsks = aggregateDepth(replayMarket?.asks || []);
    const onchainAsks = aggregateDepth(onchainMarket?.asks || []);
    const bidDiff = absDiffAcrossLevels(replayBids, onchainBids);
    const askDiff = absDiffAcrossLevels(replayAsks, onchainAsks);
    const replayBestBid = replayMarket?.bids?.[0]?.price_lots || null;
    const onchainBestBid = onchainMarket?.bids?.[0]?.price_lots || null;
    const replayBestAsk = replayMarket?.asks?.[0]?.price_lots || null;
    const onchainBestAsk = onchainMarket?.asks?.[0]?.price_lots || null;

    totalReplayOpenOrders += replayOrders;
    totalOnchainOpenOrders += onchainOrders;
    totalBidAbsDiff += bidDiff;
    totalAskAbsDiff += askDiff;

    if (
      replayOrders !== onchainOrders ||
      bidDiff > 0n ||
      askDiff > 0n ||
      replayBestBid !== onchainBestBid ||
      replayBestAsk !== onchainBestAsk
    ) {
      marketsWithDrift += 1;
    }

    markets[market] = {
      replay_open_orders: replayOrders,
      onchain_open_orders: onchainOrders,
      replay_best_bid: replayBestBid,
      onchain_best_bid: onchainBestBid,
      replay_best_ask: replayBestAsk,
      onchain_best_ask: onchainBestAsk,
      bid_base_lots_abs_diff: bidDiff.toString(),
      ask_base_lots_abs_diff: askDiff.toString(),
    };
  }

  return {
    ts_ms: Date.now(),
    replay_generated_ts_ms: replay.generated_ts_ms,
    onchain_generated_ts_ms: onchainSnapshot.generated_ts_ms,
    totals: {
      replay_open_orders: totalReplayOpenOrders,
      onchain_open_orders: totalOnchainOpenOrders,
      bid_base_lots_abs_diff: totalBidAbsDiff.toString(),
      ask_base_lots_abs_diff: totalAskAbsDiff.toString(),
      markets_with_drift: marketsWithDrift,
    },
    markets,
  };
}

function writeSseEvent(
  res: ServerResponse,
  eventName: string,
  data: unknown,
  source = 'sse_event',
): boolean {
  return (
    safeWriteResponse(res, `event: ${eventName}\n`, source, {
      event: eventName,
    }) &&
    safeWriteResponse(res, `data: ${JSON.stringify(data)}\n\n`, source, {
      event: eventName,
    })
  );
}

function broadcastEvent(event: HarnessEvent): void {
  measureSync(
    'broadcast_event',
    () => {
      const name = event.event_type;
      for (const client of Array.from(sseClients)) {
        if (!writeSseEvent(client, name, event, 'broadcast_event')) {
          sseClients.delete(client);
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_SSE_NOTIFY_MS,
      context: {
        event_type: event.event_type,
        clients: sseClients.size,
      },
    },
  );
}

function appendEventLog(event: HarnessEvent): void {
  if (!HARNESS_REPLAY_LOG) {
    return;
  }
  measureSync(
    'append_event_log',
    () => {
      try {
        appendLineWithRotation(
          HARNESS_EVENT_LOG_PATH,
          `${JSON.stringify(event)}\n`,
          HARNESS_EVENT_LOG_MAX_BYTES,
        );
      } catch (err) {
        recordRuntimeError('append_event_log', err, {
          event_type: event.event_type,
        });
      }
    },
    {
      thresholdMs: HARNESS_SLOW_COMPONENT_MS,
      context: {
        event_type: event.event_type,
      },
    },
  );
}

function parseRelayIntentEvent(raw: any): RelayIntentAcceptedEvent {
  if (!raw || raw.event_type !== 'relay_intent_accepted') {
    throw new Error('invalid relay intent event_type');
  }
  const required = [
    'group',
    'execution_queue',
    'market',
    'sequence',
    'payload_b64',
    'user_owner',
    'mango_account',
    'enqueue_tx_signature',
  ];
  for (const key of required) {
    if (!raw[key]) {
      throw new Error(`missing field: ${key}`);
    }
  }

  return {
    event_type: 'relay_intent_accepted',
    ts_ms: Number(raw.ts_ms || Date.now()),
    group: String(raw.group),
    execution_queue: String(raw.execution_queue),
    market: String(raw.market),
    sequence: String(raw.sequence),
    kind: Number(raw.kind ?? 0),
    payload_b64: String(raw.payload_b64),
    remaining_accounts: Array.isArray(raw.remaining_accounts)
      ? raw.remaining_accounts.map((a: any) => ({
          pubkey: String(a.pubkey),
          is_signer: !!a.is_signer,
          is_writable: !!a.is_writable,
        }))
      : [],
    min_execute_slot: String(raw.min_execute_slot || '0'),
    expires_at_slot: String(raw.expires_at_slot || '0'),
    user_owner: String(raw.user_owner),
    mango_account: String(raw.mango_account),
    enqueue_tx_signature: String(raw.enqueue_tx_signature),
  };
}

function checkRelayIngestAuth(req: IncomingMessage): boolean {
  if (!HARNESS_RELAY_INGEST_TOKEN.length) {
    return true;
  }
  const header = req.headers.authorization || '';
  const expected = `Bearer ${HARNESS_RELAY_INGEST_TOKEN}`;
  return header === expected;
}

function statsSnapshot() {
  let optimisticMarkets = 0;
  let optimisticUsers = 0;
  let queueViews = 0;
  let intentsTotal = 0;
  let divergencesTotal = 0;
  try {
    const optimistic = engine.getSnapshot('optimistic');
    optimisticMarkets = Object.keys(optimistic.markets).length;
    optimisticUsers = Object.keys(optimistic.users).length;
    queueViews = Object.keys(optimistic.queue).length;
    intentsTotal = engine.listIntents().length;
    divergencesTotal = engine.listDivergences(10_000).length;
  } catch (err) {
    recordRuntimeError('stats_snapshot', err);
  }
  const memory = process.memoryUsage();
  const topLatencyComponents = getLatencyAggregates(5);
  return {
    mode: HARNESS_MODE,
    backend: HARNESS_BACKEND,
    instance_id: HARNESS_INSTANCE_ID,
    pid: process.pid,
    bind_addr: HARNESS_BIND_ADDR,
    process_started_ts_ms: HARNESS_PROCESS_STARTED_TS_MS,
    cwd: process.cwd(),
    intents_total: intentsTotal,
    divergences_total: divergencesTotal,
    markets_total: optimisticMarkets,
    users_total: optimisticUsers,
    queue_views_total: queueViews,
    sse_clients: sseClients.size,
    trade_stream_subscribers: tradeStreamSubscribers.size,
    frontend_stream_subscribers: frontendStreamSubscribers.size,
    runtime_errors_total: totalRuntimeErrors,
    latency_samples_total: totalLatencySamples,
    latency_slow_samples_total: totalSlowLatencySamples,
    latency_error_samples_total: totalLatencyErrorSamples,
    latency_components_total: runtimeLatencyAggregates.size,
    top_latency_components: topLatencyComponents,
    event_loop_lag: lastEventLoopLagSnapshot,
    heap_used_bytes: memory.heapUsed,
    rss_bytes: memory.rss,
    external_bytes: memory.external,
    array_buffers_bytes: memory.arrayBuffers,
  };
}

function metricsText(): string {
  const stats = statsSnapshot();
  return [
    '# TYPE continuum_harness_intents_total gauge',
    `continuum_harness_intents_total ${stats.intents_total}`,
    '# TYPE continuum_harness_divergences_total gauge',
    `continuum_harness_divergences_total ${stats.divergences_total}`,
    '# TYPE continuum_harness_markets_total gauge',
    `continuum_harness_markets_total ${stats.markets_total}`,
    '# TYPE continuum_harness_users_total gauge',
    `continuum_harness_users_total ${stats.users_total}`,
    '# TYPE continuum_harness_sse_clients gauge',
    `continuum_harness_sse_clients ${stats.sse_clients}`,
    '# TYPE continuum_harness_trade_stream_subscribers gauge',
    `continuum_harness_trade_stream_subscribers ${stats.trade_stream_subscribers}`,
    '# TYPE continuum_harness_frontend_stream_subscribers gauge',
    `continuum_harness_frontend_stream_subscribers ${stats.frontend_stream_subscribers}`,
    '# TYPE continuum_harness_runtime_errors_total gauge',
    `continuum_harness_runtime_errors_total ${stats.runtime_errors_total}`,
    '# TYPE continuum_harness_latency_samples_total gauge',
    `continuum_harness_latency_samples_total ${stats.latency_samples_total}`,
    '# TYPE continuum_harness_latency_slow_samples_total gauge',
    `continuum_harness_latency_slow_samples_total ${stats.latency_slow_samples_total}`,
    '# TYPE continuum_harness_latency_error_samples_total gauge',
    `continuum_harness_latency_error_samples_total ${stats.latency_error_samples_total}`,
    '# TYPE continuum_harness_latency_components_total gauge',
    `continuum_harness_latency_components_total ${stats.latency_components_total}`,
    '# TYPE continuum_harness_event_loop_lag_p99_ms gauge',
    `continuum_harness_event_loop_lag_p99_ms ${
      stats.event_loop_lag?.p99_ms || 0
    }`,
    '# TYPE continuum_harness_event_loop_lag_max_ms gauge',
    `continuum_harness_event_loop_lag_max_ms ${
      stats.event_loop_lag?.max_ms || 0
    }`,
    '# TYPE continuum_harness_heap_used_bytes gauge',
    `continuum_harness_heap_used_bytes ${stats.heap_used_bytes}`,
    '# TYPE continuum_harness_rss_bytes gauge',
    `continuum_harness_rss_bytes ${stats.rss_bytes}`,
    '# TYPE continuum_harness_external_bytes gauge',
    `continuum_harness_external_bytes ${stats.external_bytes}`,
    '# TYPE continuum_harness_array_buffers_bytes gauge',
    `continuum_harness_array_buffers_bytes ${stats.array_buffers_bytes}`,
  ].join('\n');
}

async function runOnchainBalanceSanityCheck(
  onchain: OnchainContext | null,
): Promise<void> {
  const group = await getFreshGroup(onchain);
  if (!group || !onchain?.mangoClient || !onchain.usdcMint) {
    return;
  }

  const allAccounts = await onchain.mangoClient.getAllMangoAccounts(group);
  const snapshot = engine.getSnapshot('confirmed');
  const usdcMint = onchain.usdcMint.toBase58();

  const ownerToOnchain = new Map<
    string,
    { mangoAccounts: string[]; usdcUiBalance: number }
  >();
  for (const account of allAccounts) {
    const owner = account.owner.toBase58();
    let row = ownerToOnchain.get(owner);
    if (!row) {
      row = { mangoAccounts: [], usdcUiBalance: 0 };
      ownerToOnchain.set(owner, row);
    }
    row.mangoAccounts.push(account.publicKey.toBase58());
    for (const tokenPos of account.tokensActive()) {
      const bank = group.getFirstBankByTokenIndex(tokenPos.tokenIndex);
      if (bank.mint.toBase58() === usdcMint) {
        row.usdcUiBalance += tokenPos.balanceUi(bank);
      }
    }
  }

  const owners = new Set<string>([
    ...Object.keys(snapshot.users),
    ...Array.from(ownerToOnchain.keys()),
  ]);

  for (const owner of owners) {
    const harnessUser = engine.getUserState(owner, 'confirmed');
    const onchain = ownerToOnchain.get(owner) || {
      mangoAccounts: [],
      usdcUiBalance: 0,
    };

    const harnessAccounts = [...harnessUser.mango_accounts].sort();
    const onchainAccounts = [...onchain.mangoAccounts].sort();
    const harnessUsdcUiBalance = 0; // queue replay does not yet model token collateral.
    const usdcDelta = onchain.usdcUiBalance - harnessUsdcUiBalance;
    const accountsMatch =
      harnessAccounts.length === onchainAccounts.length &&
      harnessAccounts.every((v, i) => v === onchainAccounts[i]);
    const usdcMatch = Math.abs(usdcDelta) < 1e-9;

    const stateSig = JSON.stringify({
      accountsMatch,
      usdcMatch,
      harnessAccounts,
      onchainAccounts,
      harnessUsdcUiBalance,
      onchainUsdcUiBalance: onchain.usdcUiBalance,
    });
    if (sanityStateByOwner.get(owner) === stateSig) {
      continue;
    }
    sanityStateByOwner.set(owner, stateSig);

    if (!accountsMatch || !usdcMatch) {
      engine.reportExternalDivergence(
        'onchain_balance_sanity_mismatch',
        `owner:${owner}`,
        {
          owner,
          harness_mango_accounts: harnessAccounts.join(','),
          onchain_mango_accounts: onchainAccounts.join(','),
          harness_usdc_ui_balance: harnessUsdcUiBalance.toString(),
          onchain_usdc_ui_balance: onchain.usdcUiBalance.toString(),
          usdc_delta: usdcDelta.toString(),
        },
      );
    }
  }

  for (const owner of Array.from(sanityStateByOwner.keys())) {
    if (!owners.has(owner)) {
      sanityStateByOwner.delete(owner);
    }
  }
}

function replayEventLogIfPresent(): void {
  if (!HARNESS_REPLAY_LOG) {
    return;
  }
  const logPath = path.resolve(HARNESS_EVENT_LOG_PATH);
  if (!fs.existsSync(logPath)) {
    return;
  }

  measureSync(
    'replay_event_log',
    () => {
      const content = readBoundedTextFile(logPath, HARNESS_EVENT_LOG_MAX_BYTES);
      if (!content.trim().length) {
        return;
      }

      let lineCount = 0;
      for (const line of content.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed.length) {
          continue;
        }
        lineCount += 1;
        try {
          const event = JSON.parse(trimmed) as HarnessEvent;
          if (event.event_type === 'relay_intent_accepted') {
            engine.ingestRelayIntent(event);
          } else if (event.event_type === 'queue_item_enqueued') {
            engine.ingestQueueEnqueued(event);
          } else if (event.event_type === 'queue_item_processed') {
            engine.ingestQueueProcessed(event);
          }
        } catch (err) {
          recordRuntimeError('replay_event_log_line', err, {
            log_path: logPath,
            line: lineCount,
          });
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_COMPONENT_MS,
      context: {
        log_path: logPath,
      },
    },
  );
}

function readBoundedTextFile(filePath: string, maxBytes: number): string {
  const stats = fs.statSync(filePath);
  if (maxBytes <= 0 || stats.size <= maxBytes) {
    return fs.readFileSync(filePath, 'utf-8');
  }

  const fd = fs.openSync(filePath, 'r');
  try {
    const start = Math.max(0, stats.size - maxBytes);
    const buffer = Buffer.alloc(Math.min(maxBytes, stats.size - start));
    fs.readSync(fd, buffer, 0, buffer.length, start);
    const text = buffer.toString('utf-8');
    const firstNewline = text.indexOf('\n');
    return firstNewline >= 0 ? text.slice(firstNewline + 1) : text;
  } finally {
    fs.closeSync(fd);
  }
}

function parseHeaderSingle(
  req: IncomingMessage,
  key: string,
): string | undefined {
  const value = req.headers[key];
  if (!value) {
    return undefined;
  }
  if (Array.isArray(value)) {
    return value[0];
  }
  return value;
}

function toPositiveUiAmount(raw: unknown, fallback: number): number {
  const parsed = raw === undefined || raw === null ? fallback : Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error('ui_amount must be a positive number');
  }
  return parsed;
}

function toRawAmount(uiAmount: number, decimals: number): bigint {
  const scale = 10 ** decimals;
  const raw = Math.round(uiAmount * scale);
  if (!Number.isFinite(raw) || raw <= 0) {
    throw new Error('failed to convert ui_amount to raw amount');
  }
  return BigInt(raw);
}

function anchorDiscriminator(ixName: string): Buffer {
  return createHash('sha256')
    .update(`global:${ixName}`)
    .digest()
    .subarray(0, 8);
}

function u64ToLe(value: bigint): Buffer {
  const out = Buffer.alloc(8);
  out.writeBigUInt64LE(value);
  return out;
}

function u32ToLe(value: number): Buffer {
  if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) {
    throw new Error(`u32 out of range: ${value}`);
  }
  const out = Buffer.alloc(4);
  out.writeUInt32LE(value, 0);
  return out;
}

function anchorString(value: string): Buffer {
  const encoded = Buffer.from(value, 'utf8');
  return Buffer.concat([u32ToLe(encoded.length), encoded]);
}

function parseOwnerFromAirdropRequest(
  req: IncomingMessage,
  url: URL,
  payload: any,
): string {
  const fromBody =
    payload?.owner ||
    payload?.wallet ||
    payload?.wallet_pubkey ||
    payload?.user_owner;
  const fromQuery = url.searchParams.get('owner');
  const fromHeader = parseHeaderSingle(req, 'x-wallet-pubkey');
  const owner = fromBody || fromQuery || fromHeader;
  if (!owner || typeof owner !== 'string') {
    throw new Error(
      'owner is required (body.owner, query ?owner=, or x-wallet-pubkey header)',
    );
  }
  return owner;
}

function buildEmptyOnchainMarginSummary(): MarginSummary {
  return {
    status: 'empty',
    source: 'onchain-mango-health',
    account_count: 0,
    totals: {
      equity_native_quote: '0',
      pnl_native_quote: '0',
      assets_native_quote: '0',
      liabs_native_quote: '0',
      init_health_native_quote: '0',
      maint_health_native_quote: '0',
      margin_usage_fraction: 0,
    },
    accounts: [],
  };
}

function buildOnchainMarginSummaryFromAccounts(
  group: HarnessGroup,
  ownerAccounts: Awaited<ReturnType<MangoClient['getAllMangoAccounts']>>,
): MarginSummary {
  if (!ownerAccounts.length) {
    return buildEmptyOnchainMarginSummary();
  }

  const aggregate = {
    equity: ZERO_I80F48(),
    pnl: ZERO_I80F48(),
    assets: ZERO_I80F48(),
    liabs: ZERO_I80F48(),
    initHealth: ZERO_I80F48(),
    maintHealth: ZERO_I80F48(),
  };

  const accounts: MarginSummaryAccount[] = ownerAccounts.map((account) => {
    const healthCache = HealthCache.fromMangoAccount(group, account);
    const initAssetsAndLiabs = healthCache.healthAssetsAndLiabsStableLiabs(
      HealthType.init,
    );
    const equity = account.getEquity(group);
    const pnl = account.getPnl(group);
    const assets = initAssetsAndLiabs.assets;
    const liabs = initAssetsAndLiabs.liabs;
    const initHealth = account.getHealth(group, HealthType.init);
    const maintHealth = account.getHealth(group, HealthType.maint);
    const initHealthRatio = account.getHealthRatio(group, HealthType.init);
    const maintHealthRatio = account.getHealthRatio(group, HealthType.maint);
    aggregate.equity.iadd(equity);
    aggregate.pnl.iadd(pnl);
    aggregate.assets.iadd(assets);
    aggregate.liabs.iadd(liabs);
    aggregate.initHealth.iadd(initHealth);
    aggregate.maintHealth.iadd(maintHealth);
    const assetsNum = assets.toNumber();
    const liabsNum = liabs.toNumber();
    const marginUsage = assetsNum > 0 ? liabsNum / assetsNum : 0;
    return {
      mango_account: account.publicKey.toBase58(),
      owner: account.owner.toBase58(),
      equity_native_quote: equity.toString(),
      pnl_native_quote: pnl.toString(),
      assets_native_quote: assets.toString(),
      liabs_native_quote: liabs.toString(),
      init_health_native_quote: initHealth.toString(),
      maint_health_native_quote: maintHealth.toString(),
      init_health_ratio: initHealthRatio.toString(),
      maint_health_ratio: maintHealthRatio.toString(),
      margin_usage_fraction: Number.isFinite(marginUsage) ? marginUsage : 0,
      perp_positions: account.perpActive().map((p) => ({
        market_index: p.marketIndex,
        base_position_lots: p.basePositionLots.toString(),
        quote_position_native: p.quotePositionNative.toString(),
      })),
    };
  });

  const totalMarginUsage =
    aggregate.assets.toNumber() > 0
      ? aggregate.liabs.div(aggregate.assets).toNumber()
      : 0;

  return {
    status: 'ok',
    source: 'onchain-mango-health',
    account_count: accounts.length,
    totals: {
      equity_native_quote: aggregate.equity.toString(),
      pnl_native_quote: aggregate.pnl.toString(),
      assets_native_quote: aggregate.assets.toString(),
      liabs_native_quote: aggregate.liabs.toString(),
      init_health_native_quote: aggregate.initHealth.toString(),
      maint_health_native_quote: aggregate.maintHealth.toString(),
      margin_usage_fraction: Number.isFinite(totalMarginUsage)
        ? totalMarginUsage
        : 0,
    },
    equity_native_quote: aggregate.equity.toString(),
    pnl_native_quote: aggregate.pnl.toString(),
    assets_native_quote: aggregate.assets.toString(),
    liabs_native_quote: aggregate.liabs.toString(),
    init_health_native_quote: aggregate.initHealth.toString(),
    maint_health_native_quote: aggregate.maintHealth.toString(),
    margin_usage_fraction: Number.isFinite(totalMarginUsage)
      ? totalMarginUsage
      : 0,
    accounts,
  };
}

function buildOwnerTokenTotals(
  group: HarnessGroup,
  ownerAccounts: Awaited<ReturnType<MangoClient['getAllMangoAccounts']>>,
) {
  const tokenTotals = new Map<
    number,
    {
      token_index: number;
      mint: string;
      ui_balance: number;
      ui_deposits: number;
      ui_borrows: number;
    }
  >();

  for (const account of ownerAccounts) {
    for (const tokenPos of account.tokensActive()) {
      let bank;
      try {
        bank = group.getFirstBankByTokenIndex(tokenPos.tokenIndex);
      } catch {
        continue;
      }
      const existing = tokenTotals.get(tokenPos.tokenIndex) || {
        token_index: tokenPos.tokenIndex,
        mint: bank.mint.toBase58(),
        ui_balance: 0,
        ui_deposits: 0,
        ui_borrows: 0,
      };
      existing.ui_balance += tokenPos.balanceUi(bank);
      existing.ui_deposits += tokenPos.depositsUi(bank);
      existing.ui_borrows += tokenPos.borrowsUi(bank);
      tokenTotals.set(tokenPos.tokenIndex, existing);
    }
  }

  return Array.from(tokenTotals.values()).sort(
    (a, b) => a.token_index - b.token_index,
  );
}

async function enrichOwnerStateWithOnchain(
  ownerRaw: string,
  baseState: any,
  onchain: OnchainContext | null,
  view: QueueView,
): Promise<any> {
  const group = await getFreshGroup(onchain);
  if (!group || !onchain?.mangoClient) {
    return baseState;
  }

  let owner: PublicKey;
  try {
    owner = new PublicKey(ownerRaw);
  } catch {
    return baseState;
  }

  try {
    const ownerAccounts = await onchain.mangoClient.getMangoAccountsForOwner(
      group,
      owner,
    );
    if (!ownerAccounts.length) {
      return {
        ...baseState,
        margin_summary: buildEmptyOnchainMarginSummary(),
      };
    }

    const tokens = buildOwnerTokenTotals(group, ownerAccounts);
    const usdcMint = onchain.usdcMint?.toBase58() || tokens[0]?.mint || '';
    const usdcToken = tokens.find((t) => t.mint === usdcMint);
    const marginSummary = buildOnchainMarginSummaryFromAccounts(
      group,
      ownerAccounts,
    );

    const collateralPayload = {
      source: view === 'optimistic' ? 'onchain-baseline' : 'onchain',
      usdc_mint: usdcMint,
      usdc_ui_balance: usdcToken?.ui_balance || 0,
      tokens,
    };

    return {
      ...baseState,
      mango_accounts: baseState.mango_accounts?.length
        ? baseState.mango_accounts
        : ownerAccounts.map((a) => a.publicKey.toBase58()),
      margin_summary: marginSummary,
      ...(view === 'confirmed'
        ? { confirmed_collateral: collateralPayload }
        : { optimistic_collateral: collateralPayload }),
    };
  } catch (err) {
    console.warn(`failed to enrich ${view} balances for ${ownerRaw}: ${err}`);
    return baseState;
  }
}

async function resolveExecutionQueuePk(
  onchain: OnchainContext | null,
): Promise<PublicKey | null> {
  if (!onchain) {
    return null;
  }
  if (onchain.executionQueuePk) {
    return onchain.executionQueuePk;
  }
  const group = await getFreshGroup(onchain);
  if (!group) {
    return null;
  }
  const [executionQueuePk] = PublicKey.findProgramAddressSync(
    [Buffer.from('ExecutionQueue'), group.publicKey.toBuffer()],
    onchain.programId,
  );
  onchain.executionQueuePk = executionQueuePk;
  return executionQueuePk;
}

async function buildOnchainQueueSnapshot(
  onchain: OnchainContext | null,
): Promise<OnchainQueueSnapshot | null> {
  const executionQueuePk = await resolveExecutionQueuePk(onchain);
  if (!executionQueuePk || !onchain?.groupPk) {
    return null;
  }

  const queueAccount = await onchain.connection.getAccountInfoAndContext(
    executionQueuePk,
    HARNESS_COMMITMENT,
  );
  if (!queueAccount.value?.data) {
    return null;
  }

  const data = Buffer.from(queueAccount.value.data);
  const header = decodeExecutionQueueHeader(data);
  const group = onchain.groupPk.toBase58();
  const items = decodeExecutionQueuePendingItems(data).map((item) => {
    const intent =
      item.section === 'ctm'
        ? engine.findIntent(group, item.sequence.toString(), item.kind)
        : null;
    return {
      market:
        intent?.market ||
        (item.section === 'liquidity' ? 'liquidity' : 'unknown'),
      sequence: item.sequence.toString(),
      kind: item.kind,
      min_execute_slot: item.minExecuteSlot.toString(),
      section: item.section,
    } satisfies OnchainQueuePendingItem;
  });

  return {
    generated_ts_ms: Date.now(),
    observed_slot: queueAccount.context.slot,
    next_sequence: header.nextSequence.toString(),
    max_seen_sequence: header.maxSeenSequence.toString(),
    total_count: header.totalCount,
    items,
  };
}

/** Yield to the event loop so HTTP/SSE handlers can run between heavy RPC calls. */
function yieldToEventLoop(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

async function buildOnchainConfirmedSnapshot(
  onchain: OnchainContext | null,
): Promise<{
  snapshot: EngineSnapshot;
  queue: OnchainQueueSnapshot | null;
} | null> {
  const group = await getFreshGroup(onchain);
  if (!group || !onchain?.mangoClient) {
    return null;
  }

  await yieldToEventLoop();
  const replayConfirmed = engine.getSnapshot('confirmed');
  const onchainQueue = await buildOnchainQueueSnapshot(onchain);
  await yieldToEventLoop();
  const allAccounts = await onchain.mangoClient.getAllMangoAccounts(group);
  await yieldToEventLoop();
  const ownerByMangoAccount = new Map<string, string>();
  const ownerAccountsMap = new Map<
    string,
    Awaited<ReturnType<MangoClient['getAllMangoAccounts']>>
  >();
  const users = new Map<string, UserState>();
  const accountStates = {} as NonNullable<EngineSnapshot['accounts']>;
  const perUserMarket = new Map<
    string,
    Map<
      string,
      {
        openBid: bigint;
        openAsk: bigint;
        quoteReserved: bigint;
        basePositionLots: bigint;
        quotePositionNative: bigint;
      }
    >
  >();
  const tokenBanks = {} as NonNullable<EngineSnapshot['token_banks']>;

  for (const [tokenIndex, banks] of group.banksMapByTokenIndex.entries()) {
    const bank = banks[0];
    if (!bank) {
      continue;
    }
    const liabPrice = bank.getLiabPrice();
    const [maintAssetWeight, maintLiabWeight] = bank.maintWeights();
    tokenBanks[`${tokenIndex}`] = {
      token_index: tokenIndex,
      mint: bank.mint.toBase58(),
      deposit_index: bank.depositIndex.toString(),
      borrow_index: bank.borrowIndex.toString(),
      oracle_price: bank.price.toString(),
      stable_price: I80F48.fromNumber(
        bank.stablePriceModel.stablePrice,
      ).toString(),
      maint_asset_weight: maintAssetWeight.toString(),
      init_asset_weight: bank.initAssetWeight.toString(),
      init_scaled_asset_weight: bank
        .scaledInitAssetWeight(liabPrice)
        .toString(),
      maint_liab_weight: maintLiabWeight.toString(),
      init_liab_weight: bank.initLiabWeight.toString(),
      init_scaled_liab_weight: bank.scaledInitLiabWeight(liabPrice).toString(),
    };
  }

  const ensureUser = (owner: string): UserState => {
    const existing = users.get(owner);
    if (existing) {
      return existing;
    }
    const created = emptyUserState(owner);
    users.set(owner, created);
    return created;
  };

  const ensureUserMarket = (owner: string, market: string) => {
    let ownerMap = perUserMarket.get(owner);
    if (!ownerMap) {
      ownerMap = new Map();
      perUserMarket.set(owner, ownerMap);
    }
    const existing = ownerMap.get(market);
    if (existing) {
      return existing;
    }
    const created = {
      openBid: 0n,
      openAsk: 0n,
      quoteReserved: 0n,
      basePositionLots: 0n,
      quotePositionNative: 0n,
    };
    ownerMap.set(market, created);
    return created;
  };

  for (let _ai = 0; _ai < allAccounts.length; _ai++) {
    if (_ai > 0 && _ai % 10 === 0) await yieldToEventLoop();
    const account = allAccounts[_ai];
    const mangoAccount = account.publicKey.toBase58();
    const owner = account.owner.toBase58();
    ownerByMangoAccount.set(mangoAccount, owner);
    const ownerAccounts = ownerAccountsMap.get(owner) || [];
    ownerAccounts.push(account);
    ownerAccountsMap.set(owner, ownerAccounts);
    const user = ensureUser(owner);
    if (!user.mango_accounts.includes(mangoAccount)) {
      user.mango_accounts.push(mangoAccount);
    }
    accountStates[mangoAccount] = {
      owner,
      mango_account: mangoAccount,
      net_deposits: account.netDeposits.toString(),
      open_orders: [],
      token_positions: account.tokensActive().flatMap((tokenPosition) => {
        try {
          const bank = group.getFirstBankByTokenIndex(tokenPosition.tokenIndex);
          return [
            {
              token_index: tokenPosition.tokenIndex,
              indexed_position: tokenPosition.indexedPosition.toString(),
              native_balance: tokenPosition.balance(bank).toString(),
              previous_index: tokenPosition.previousIndex.toString(),
              cumulative_deposit_interest: `${tokenPosition.cumulativeDepositInterest}`,
              cumulative_borrow_interest: `${tokenPosition.cumulativeBorrowInterest}`,
              in_use_count: tokenPosition.inUseCount,
            },
          ];
        } catch {
          return [];
        }
      }),
      perp_positions: account.perpActive().map((perpPosition) => ({
        market_index: perpPosition.marketIndex,
        settle_pnl_limit_window: perpPosition.settlePnlLimitWindow,
        settle_pnl_limit_settled_in_current_window_native:
          perpPosition.settlePnlLimitSettledInCurrentWindowNative.toString(),
        base_position_lots: perpPosition.basePositionLots.toString(),
        quote_position_native: perpPosition.quotePositionNative.toString(),
        quote_running_native: perpPosition.quoteRunningNative.toString(),
        long_settled_funding: perpPosition.longSettledFunding.toString(),
        short_settled_funding: perpPosition.shortSettledFunding.toString(),
        open_bid_base_lots: perpPosition.bidsBaseLots.toString(),
        open_ask_base_lots: perpPosition.asksBaseLots.toString(),
        taker_base_lots: perpPosition.takerBaseLots.toString(),
        taker_quote_lots: perpPosition.takerQuoteLots.toString(),
        cumulative_long_funding: `${perpPosition.cumulativeLongFunding}`,
        cumulative_short_funding: `${perpPosition.cumulativeShortFunding}`,
        maker_volume: perpPosition.makerVolume.toString(),
        taker_volume: perpPosition.takerVolume.toString(),
        perp_spot_transfers: perpPosition.perpSpotTransfers.toString(),
        avg_entry_price_per_base_lot: `${perpPosition.avgEntryPricePerBaseLot}`,
        oneshot_settle_pnl_allowance:
          perpPosition.oneshotSettlePnlAllowance.toString(),
        recurring_settle_pnl_allowance:
          perpPosition.recurringSettlePnlAllowance.toString(),
        realized_pnl_for_position_native:
          perpPosition.realizedPnlForPositionNative.toString(),
      })),
      unsupported_exposures: [
        ...(account.serum3Active().length ? ['serum3'] : []),
        ...(account.openbookV2Active().length ? ['openbook_v2'] : []),
        ...(account.tokenConditionalSwapsActive().length
          ? ['token_conditional_swap']
          : []),
      ],
    };
    for (const perpPosition of account.perpActive()) {
      const agg = ensureUserMarket(owner, `${perpPosition.marketIndex}`);
      agg.basePositionLots += BigInt(perpPosition.basePositionLots.toString());
      agg.quotePositionNative += decimalStringToBigintTrunc(
        perpPosition.quotePositionNative.toString(),
      );
    }
  }

  const markets = {} as EngineSnapshot['markets'];
  const perpMarkets = {} as NonNullable<EngineSnapshot['perp_markets']>;
  for (const [
    marketIndex,
    perpMarket,
  ] of group.perpMarketsMapByMarketIndex.entries()) {
    const market = `${marketIndex}`;
    perpMarkets[market] = {
      market,
      market_index: marketIndex,
      settle_token_index: perpMarket.settleTokenIndex,
      oracle_price: perpMarket.price.toString(),
      stable_price: I80F48.fromNumber(
        perpMarket.stablePriceModel.stablePrice,
      ).toString(),
      base_lot_size: perpMarket.baseLotSize.toString(),
      quote_lot_size: perpMarket.quoteLotSize.toString(),
      maint_base_asset_weight: perpMarket.maintBaseAssetWeight.toString(),
      init_base_asset_weight: perpMarket.initBaseAssetWeight.toString(),
      maint_base_liab_weight: perpMarket.maintBaseLiabWeight.toString(),
      init_base_liab_weight: perpMarket.initBaseLiabWeight.toString(),
      maint_overall_asset_weight: perpMarket.maintOverallAssetWeight.toString(),
      init_overall_asset_weight: perpMarket.initOverallAssetWeight.toString(),
      long_funding: perpMarket.longFunding.toString(),
      short_funding: perpMarket.shortFunding.toString(),
      maker_fee: perpMarket.makerFee.toString(),
      taker_fee: perpMarket.takerFee.toString(),
    };
    await yieldToEventLoop();
    const [bidsBook, asksBook] = await Promise.all([
      perpMarket.loadBids(onchain.mangoClient, true),
      perpMarket.loadAsks(onchain.mangoClient, true),
    ]);
    await yieldToEventLoop();
    const bids = new Map<string, bigint>();
    const asks = new Map<string, bigint>();
    const openOrders: OpenOrderSummary[] = [];

    const recordOrder = (
      side: 'bid' | 'ask',
      priceLotsStr: string,
      baseLots: bigint,
      quoteLots: bigint,
      mangoAccount: string,
      owner: string,
      sequence: string,
      expiryTimestamp: string,
      orderId: string,
    ) => {
      const depth = side === 'bid' ? bids : asks;
      depth.set(priceLotsStr, (depth.get(priceLotsStr) || 0n) + baseLots);
      const user = ensureUser(owner);
      if (!user.mango_accounts.includes(mangoAccount)) {
        user.mango_accounts.push(mangoAccount);
      }
      const orderSummary = {
        order_id: orderId,
        owner,
        mango_account: mangoAccount,
        market,
        side,
        price_lots: priceLotsStr,
        base_lots: baseLots.toString(),
        quote_lots: quoteLots.toString(),
        client_order_id: '0',
        sequence,
        expiry_timestamp: expiryTimestamp,
        status: 'open',
      } satisfies OpenOrderSummary;
      openOrders.push(orderSummary);
      user.open_orders.push(orderSummary);
      accountStates[mangoAccount]?.open_orders.push(orderSummary);
      const agg = ensureUserMarket(owner, market);
      if (side === 'bid') {
        agg.openBid += baseLots;
      } else {
        agg.openAsk += baseLots;
      }
      agg.quoteReserved += quoteLots;
    };

    for (const order of bidsBook.itemsValid()) {
      const mangoAccount = order.owner.toBase58();
      const owner = ownerByMangoAccount.get(mangoAccount) || mangoAccount;
      const baseLots = BigInt(order.sizeLots.toString());
      const priceLotsStr = order.priceLots.toString();
      const quoteLots = BigInt(order.priceLots.toString()) * baseLots;
      recordOrder(
        'bid',
        priceLotsStr,
        baseLots,
        quoteLots,
        mangoAccount,
        owner,
        order.seqNum.toString(),
        order.expiryTimestamp.toString(),
        `${group.publicKey.toBase58()}:${market}:${order.orderId.toString()}`,
      );
    }

    for (const order of asksBook.itemsValid()) {
      const mangoAccount = order.owner.toBase58();
      const owner = ownerByMangoAccount.get(mangoAccount) || mangoAccount;
      const baseLots = BigInt(order.sizeLots.toString());
      const priceLotsStr = order.priceLots.toString();
      const quoteLots = BigInt(order.priceLots.toString()) * baseLots;
      recordOrder(
        'ask',
        priceLotsStr,
        baseLots,
        quoteLots,
        mangoAccount,
        owner,
        order.seqNum.toString(),
        order.expiryTimestamp.toString(),
        `${group.publicKey.toBase58()}:${market}:${order.orderId.toString()}`,
      );
    }

    const toLevels = (depth: Map<string, bigint>, descending: boolean) =>
      Array.from(depth.entries())
        .map(([price_lots, base_lots]) => ({
          price_lots,
          base_lots: base_lots.toString(),
        }))
        .sort((a, b) => {
          const av = BigInt(a.price_lots);
          const bv = BigInt(b.price_lots);
          if (av === bv) {
            return 0;
          }
          if (descending) {
            return av > bv ? -1 : 1;
          }
          return av < bv ? -1 : 1;
        });

    const replayMarket = replayConfirmed.markets[market];
    markets[market] = {
      market,
      bids: toLevels(bids, true),
      asks: toLevels(asks, false),
      open_orders: openOrders,
      watermarks: replayMarket?.watermarks || {
        optimistic_seq: '0',
        confirmed_seq: '0',
        last_slot: '0',
      },
    };
  }

  const userStates = {} as EngineSnapshot['users'];
  for (const [owner, user] of users.entries()) {
    const perMarketEntries = Array.from(
      (perUserMarket.get(owner) || new Map()).entries(),
    )
      .map(([market, agg]) => ({
        market,
        open_order_base_lots_bid: agg.openBid.toString(),
        open_order_base_lots_ask: agg.openAsk.toString(),
        quote_reserved_lots: agg.quoteReserved.toString(),
        base_position_lots: agg.basePositionLots.toString(),
        quote_position_native: agg.quotePositionNative.toString(),
      }))
      .sort((a, b) => a.market.localeCompare(b.market));
    userStates[owner] = {
      owner,
      mango_accounts: [...user.mango_accounts].sort(),
      open_orders: [...user.open_orders].sort((a, b) => {
        if (a.market !== b.market) {
          return a.market.localeCompare(b.market);
        }
        if (a.side !== b.side) {
          return a.side.localeCompare(b.side);
        }
        const ap = BigInt(a.price_lots);
        const bp = BigInt(b.price_lots);
        if (ap !== bp) {
          return ap < bp ? -1 : 1;
        }
        return BigInt(a.sequence) < BigInt(b.sequence) ? -1 : 1;
      }),
      per_market: perMarketEntries,
      margin_summary: buildOnchainMarginSummaryFromAccounts(
        group,
        ownerAccountsMap.get(owner) || [],
      ),
    };
  }

  return {
    snapshot: {
      view: 'confirmed',
      markets,
      users: userStates,
      queue: onchainQueue
        ? buildQueueViewForSnapshot(
            'confirmed',
            replayConfirmed,
            {
              snapshot: null,
              drift: null,
              queue: onchainQueue,
              last_error: null,
            },
          )
        : replayConfirmed.queue,
      accounts: accountStates,
      perp_markets: perpMarkets,
      token_banks: tokenBanks,
      generated_ts_ms: Date.now(),
    },
    queue: onchainQueue,
  };
}

async function runOnchainReconciliation(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
): Promise<void> {
  const onchainState = await buildOnchainConfirmedSnapshot(onchain);
  if (!onchainState) {
    return;
  }
  const onchainSnapshot = onchainState.snapshot;
  const replaySnapshot = engine.getSnapshot('confirmed');
  const drift = buildReconciliationSnapshot(replaySnapshot, onchainSnapshot);
  if (HARNESS_BACKEND === 'rust-backend') {
    engine.bootstrapFromOnchainSnapshot(onchainSnapshot);
  }
  onchainSync.snapshot = onchainSnapshot;
  onchainSync.queue = onchainState.queue;
  onchainSync.drift = drift;
  onchainSync.last_error = null;

  const driftSig = JSON.stringify(drift.totals);
  if (driftSig !== lastReconciliationSig) {
    lastReconciliationSig = driftSig;
    console.log(
      JSON.stringify({
        ts: new Date().toISOString(),
        msg: 'continuum-onchain-reconciliation',
        ...drift,
      }),
    );
    if (drift.totals.markets_with_drift > 0) {
      engine.reportExternalDivergence(
        'onchain_reconciliation_drift',
        'confirmed',
        {
          replay_open_orders: `${drift.totals.replay_open_orders}`,
          onchain_open_orders: `${drift.totals.onchain_open_orders}`,
          bid_base_lots_abs_diff: drift.totals.bid_base_lots_abs_diff,
          ask_base_lots_abs_diff: drift.totals.ask_base_lots_abs_diff,
          markets_with_drift: `${drift.totals.markets_with_drift}`,
        },
      );
      recordDiscrepancy({
        id: nextDiscrepancySeq++,
        ts_ms: drift.ts_ms,
        reason: 'onchain_reconciliation_drift',
        snapshot: trimReconciliationSnapshotToDrift(drift),
      });
    }
  }
}

async function refreshOnchainConfirmedState(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
): Promise<void> {
  const onchainState = await buildOnchainConfirmedSnapshot(onchain);
  if (!onchainState) {
    return;
  }
  if (HARNESS_BACKEND === 'rust-backend') {
    engine.bootstrapFromOnchainSnapshot(onchainState.snapshot);
  }
  onchainSync.snapshot = onchainState.snapshot;
  onchainSync.queue = onchainState.queue;
  onchainSync.last_error = null;
}

async function refreshOnchainQueueState(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
): Promise<void> {
  onchainSync.queue = await buildOnchainQueueSnapshot(onchain);
}

function scheduleDirectOnchainRebase(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
  context: Record<string, string>,
): void {
  if (!onchain?.mangoClient || !onchain.groupPk) {
    return;
  }
  if (directOnchainRebaseTimer) {
    clearTimeout(directOnchainRebaseTimer);
  }
  directOnchainRebasePending = true;
  directOnchainRebaseTimer = setTimeout(() => {
    directOnchainRebaseTimer = null;
    if (directOnchainRebaseInFlight) {
      directOnchainRebasePending = true;
      return;
    }
    directOnchainRebasePending = false;
    directOnchainRebaseInFlight = measureAsync(
      'onchain_direct_rebase',
      async () => {
        await refreshOnchainConfirmedState(onchain, onchainSync);
        marketRuntimeMetricsCache.clear();
        scheduleFrontendStreamNotification(onchain, onchainSync, {
          forceAll: true,
        });
      },
      context,
    )
      .catch((err) => {
        onchainSync.last_error = `${err}`;
        recordRuntimeError('onchain_direct_rebase', err, context);
      })
      .finally(() => {
        directOnchainRebaseInFlight = null;
        if (directOnchainRebasePending) {
          scheduleDirectOnchainRebase(onchain, onchainSync, context);
        }
      });
  }, HARNESS_DIRECT_ONCHAIN_REBASE_DEBOUNCE_MS);
}

function scheduleQueueSync(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
  context: Record<string, string>,
): void {
  if (!onchain?.groupPk) {
    return;
  }
  if (queueSyncTimer) {
    clearTimeout(queueSyncTimer);
  }
  queueSyncPending = true;
  queueSyncTimer = setTimeout(() => {
    queueSyncTimer = null;
    if (queueSyncInFlight) {
      queueSyncPending = true;
      return;
    }
    queueSyncPending = false;
    queueSyncInFlight = measureAsync(
      'onchain_queue_sync',
      async () => {
        await refreshOnchainQueueState(onchain, onchainSync);
        scheduleFrontendStreamNotification(onchain, onchainSync, {
          forceAll: true,
        });
      },
      context,
    )
      .catch((err) => {
        recordRuntimeError('onchain_queue_sync', err, context);
      })
      .finally(() => {
        queueSyncInFlight = null;
        if (queueSyncPending) {
          scheduleQueueSync(onchain, onchainSync, context);
        }
      });
  }, HARNESS_QUEUE_SYNC_DEBOUNCE_MS);
}

async function buildAirdropContext(
  onchain: OnchainContext,
): Promise<AirdropContext | null> {
  if (!HARNESS_ENABLE_AIRDROP) {
    return null;
  }
  await getFreshGroup(onchain);
  if (!onchain.usdcMint || !HARNESS_AIRDROP_KEYPAIR.length) {
    console.warn(
      'airdrop endpoint disabled: unable to resolve USDC mint or faucet keypair',
    );
    return null;
  }

  const usdcMint = onchain.usdcMint;
  const faucet = readKeypair(HARNESS_AIRDROP_KEYPAIR);
  const mintInfo = await getMint(onchain.connection, usdcMint);

  if (
    !mintInfo.mintAuthority ||
    !mintInfo.mintAuthority.equals(faucet.publicKey)
  ) {
    console.warn(
      `airdrop endpoint warning: faucet keypair may not be mint authority for ${usdcMint.toBase58()}`,
    );
  }

  return {
    ...onchain,
    usdcMint,
    decimals: mintInfo.decimals,
    faucet,
    defaultUiAmount: HARNESS_AIRDROP_DEFAULT_UI_AMOUNT,
    maxUiAmount: HARNESS_AIRDROP_MAX_UI_AMOUNT,
    depositUiAmount: HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT,
  };
}

async function buildOnchainContext(
  connection: Connection,
  programId: PublicKey,
): Promise<OnchainContext> {
  const groupPk = HARNESS_GROUP_PK.length
    ? new PublicKey(HARNESS_GROUP_PK)
    : null;
  let mangoClient: MangoClient | null = null;
  let cachedGroup: HarnessGroup | null = null;
  let usdcMint: PublicKey | null = HARNESS_USDC_MINT.length
    ? new PublicKey(HARNESS_USDC_MINT)
    : null;
  let executionQueuePk: PublicKey | null =
    process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
    process.env.EXECUTION_QUEUE_PK
      ? new PublicKey(
          process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
            process.env.EXECUTION_QUEUE_PK ||
            '',
        )
      : null;

  if (groupPk) {
    try {
      const provider = new AnchorProvider(
        connection,
        new Wallet(Keypair.generate()),
        AnchorProvider.defaultOptions(),
      );
      mangoClient = await MangoClient.connect(provider, CLUSTER, programId, {
        idsSource: 'get-program-accounts',
      });
      cachedGroup = await mangoClient.getGroup(groupPk);
      await cachedGroup.reloadAll(mangoClient);
      if (!usdcMint) {
        usdcMint = cachedGroup.getFirstBankForPerpSettlement().mint;
      }
      if (!executionQueuePk) {
        [executionQueuePk] = PublicKey.findProgramAddressSync(
          [Buffer.from('ExecutionQueue'), cachedGroup.publicKey.toBuffer()],
          programId,
        );
      }
    } catch (err) {
      console.warn(
        `onchain read context disabled: failed to initialize group/mango client (${err})`,
      );
      mangoClient = null;
      cachedGroup = null;
      executionQueuePk =
        process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
        process.env.EXECUTION_QUEUE_PK
          ? new PublicKey(
              process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
                process.env.EXECUTION_QUEUE_PK ||
                '',
            )
          : null;
      usdcMint = HARNESS_USDC_MINT.length
        ? new PublicKey(HARNESS_USDC_MINT)
        : null;
    }
  }

  return {
    connection,
    groupPk,
    mangoClient,
    usdcMint,
    programId,
    executionQueuePk,
    cachedGroup,
    cachedGroupFetchedAtMs: cachedGroup ? Date.now() : 0,
    cachedMarketMetadata: null,
    cachedMarketMetadataFetchedAtMs: 0,
  };
}

async function maybeBackfillProgramLogs(
  connection: Connection,
  programId: PublicKey,
): Promise<void> {
  if (
    !HARNESS_BACKFILL_SIGNATURE_LIMIT ||
    HARNESS_BACKFILL_SIGNATURE_LIMIT <= 0
  ) {
    return;
  }

  const signatures = await connection.getSignaturesForAddress(programId, {
    limit: HARNESS_BACKFILL_SIGNATURE_LIMIT,
  });
  const txCommitment =
    HARNESS_COMMITMENT === 'finalized' ? 'finalized' : 'confirmed';

  for (const sig of signatures.reverse()) {
    if (!sig.signature) {
      continue;
    }
    const tx = await connection.getTransaction(sig.signature, {
      commitment: txCommitment,
      maxSupportedTransactionVersion: 0,
    });
    if (!tx?.meta?.logMessages) {
      continue;
    }
    await handleProgramLogs(
      connection,
      {
        err: tx.meta.err,
        logs: tx.meta.logMessages,
        signature: sig.signature,
      },
      tx.slot,
      undefined,
      undefined,
      tx.blockTime ?? null,
    );
  }
}

function rememberProgramLogUnixTs(
  slot: number,
  unixTs: number | null,
): number | null {
  if (programLogBlockTimeCache.has(slot)) {
    programLogBlockTimeCache.delete(slot);
  }
  programLogBlockTimeCache.set(slot, unixTs);
  while (
    programLogBlockTimeCache.size > HARNESS_PROGRAM_LOG_BLOCK_TIME_CACHE_LIMIT
  ) {
    const oldest = programLogBlockTimeCache.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    programLogBlockTimeCache.delete(oldest);
  }
  return unixTs;
}

async function resolveProgramLogUnixTs(
  connection: Connection,
  slot: number,
  signature: string,
  preloadedUnixTs?: number | null,
): Promise<number | null> {
  if (typeof preloadedUnixTs === 'number') {
    return rememberProgramLogUnixTs(slot, preloadedUnixTs);
  }
  if (programLogBlockTimeCache.has(slot)) {
    return programLogBlockTimeCache.get(slot) ?? null;
  }

  let inflight = programLogBlockTimeInflight.get(slot);
  if (!inflight) {
    inflight = (async () => {
      try {
        const blockTime = await connection.getBlockTime(slot);
        if (typeof blockTime === 'number') {
          return rememberProgramLogUnixTs(slot, blockTime);
        }
        const txCommitment =
          HARNESS_COMMITMENT === 'finalized' ? 'finalized' : 'confirmed';
        const tx = await connection.getTransaction(signature, {
          commitment: txCommitment,
          maxSupportedTransactionVersion: 0,
        });
        return rememberProgramLogUnixTs(slot, tx?.blockTime ?? null);
      } finally {
        programLogBlockTimeInflight.delete(slot);
      }
    })();
    programLogBlockTimeInflight.set(slot, inflight);
  }

  return inflight;
}

async function handleProgramLogs(
  connection: Connection,
  logs: Logs,
  slot: number,
  onchain?: OnchainContext | null,
  onchainSync?: OnchainSyncState,
  preloadedUnixTs?: number | null,
): Promise<void> {
  if (logs.err) {
    return;
  }

  const decodedEvents: DecodedQueueLogEvent[] = [];
  let sawUnsupportedQueueMutation = false;
  for (const line of logs.logs) {
    const raw = parseProgramDataLogLine(line);
    if (!raw) {
      continue;
    }
    const decoded = decodeQueueAnchorEvent(raw);
    if (!decoded) {
      continue;
    }
    decodedEvents.push(decoded);
    if (decoded.kind !== 0) {
      sawUnsupportedQueueMutation = true;
    }
  }

  const sawQueueEvent = decodedEvents.length > 0;
  const processedUnixTs = sawQueueEvent
    ? await resolveProgramLogUnixTs(
        connection,
        slot,
        logs.signature,
        preloadedUnixTs,
      )
    : null;

  for (const decoded of decodedEvents) {
    try {
      if (decoded.type === 'QueueItemEnqueued') {
        engine.ingestQueueEnqueued({
          event_type: 'queue_item_enqueued',
          ts_ms: Date.now(),
          group: decoded.group,
          sequence: decoded.sequence.toString(),
          kind: decoded.kind,
          min_execute_slot: decoded.min_execute_slot.toString(),
          slot: slot.toString(),
          tx_signature: logs.signature,
        });
      } else {
        engine.ingestQueueProcessed({
          event_type: 'queue_item_processed',
          ts_ms: Date.now(),
          group: decoded.group,
          sequence: decoded.sequence.toString(),
          kind: decoded.kind,
          status: decoded.status,
          slot: slot.toString(),
          tx_signature: logs.signature,
          processed_unix_ts: processedUnixTs,
        });
      }
    } catch (err) {
      recordRuntimeError('handle_program_logs', err, {
        slot,
        signature: logs.signature,
        decoded_type: decoded.type,
      });
    }
  }

  if (onchain && onchainSync) {
    if (sawQueueEvent) {
      scheduleQueueSync(onchain, onchainSync, {
        slot: `${slot}`,
        signature: logs.signature,
        reason: 'queue_event',
      });
      if (sawUnsupportedQueueMutation) {
        scheduleDirectOnchainRebase(onchain, onchainSync, {
          slot: `${slot}`,
          signature: logs.signature,
          reason: 'unsupported_queue_mutation',
        });
      }
    } else if (!logs.err) {
      scheduleDirectOnchainRebase(onchain, onchainSync, {
        slot: `${slot}`,
        signature: logs.signature,
        reason: 'direct_program_write',
      });
    }
  }
}

async function processAirdropRequest(
  req: IncomingMessage,
  url: URL,
  airdrop: AirdropContext | null,
): Promise<{
  ok: boolean;
  owner: string;
  mint: string;
  destination_token_account: string;
  ui_amount: number;
  raw_amount: string;
  tx_signature: string;
}> {
  if (!airdrop) {
    throw new Error('airdrop endpoint is disabled');
  }

  const body = await readBody(req);
  const payload = body.trim().length ? JSON.parse(body) : {};
  const ownerRaw = parseOwnerFromAirdropRequest(req, url, payload);
  const owner = new PublicKey(ownerRaw);

  const uiAmount = toPositiveUiAmount(
    payload?.ui_amount,
    airdrop.defaultUiAmount,
  );
  if (uiAmount > airdrop.maxUiAmount) {
    throw new Error(`ui_amount exceeds max ${airdrop.maxUiAmount}`);
  }

  const rawAmount = toRawAmount(uiAmount, airdrop.decimals);
  const destination = await createAssociatedTokenAccountIdempotent(
    airdrop.connection,
    airdrop.faucet,
    airdrop.usdcMint,
    owner,
  );
  const signature = await mintTo(
    airdrop.connection,
    airdrop.faucet,
    airdrop.usdcMint,
    destination,
    airdrop.faucet,
    rawAmount,
  );
  await airdrop.connection.confirmTransaction(signature, HARNESS_COMMITMENT);

  return {
    ok: true,
    owner: owner.toBase58(),
    mint: airdrop.usdcMint.toBase58(),
    destination_token_account: destination.toBase58(),
    ui_amount: uiAmount,
    raw_amount: rawAmount.toString(),
    tx_signature: signature,
  };
}

async function createUnsafeMangoAccountForOwner(
  airdrop: AirdropContext,
  group: any,
  owner: PublicKey,
  accountNum: number,
): Promise<{
  mangoAccountPk: PublicKey;
  created: boolean;
  txSignature: string | null;
}> {
  const [mangoAccountPk] = PublicKey.findProgramAddressSync(
    [
      Buffer.from('MangoAccount'),
      group.publicKey.toBuffer(),
      owner.toBuffer(),
      u32ToLe(accountNum),
    ],
    airdrop.programId,
  );

  const existingAi = await airdrop.connection.getAccountInfo(
    mangoAccountPk,
    HARNESS_COMMITMENT,
  );
  if (existingAi) {
    return { mangoAccountPk, created: false, txSignature: null };
  }

  const ixData = Buffer.concat([
    anchorDiscriminator('unsafe_account_create'),
    u32ToLe(accountNum),
    Buffer.from([
      HARNESS_AIRDROP_AUTO_CREATE_TOKEN_COUNT,
      HARNESS_AIRDROP_AUTO_CREATE_SERUM3_COUNT,
      HARNESS_AIRDROP_AUTO_CREATE_PERP_COUNT,
      HARNESS_AIRDROP_AUTO_CREATE_PERP_OO_COUNT,
    ]),
    anchorString(HARNESS_AIRDROP_AUTO_CREATE_NAME),
  ]);
  const createIx = new TransactionInstruction({
    programId: airdrop.programId,
    keys: [
      { pubkey: group.publicKey, isSigner: false, isWritable: false },
      { pubkey: mangoAccountPk, isSigner: false, isWritable: true },
      { pubkey: owner, isSigner: false, isWritable: false },
      { pubkey: airdrop.faucet.publicKey, isSigner: true, isWritable: false },
      { pubkey: airdrop.faucet.publicKey, isSigner: true, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: ixData,
  });

  try {
    const status = await airdrop.mangoClient!.sendAndConfirmTransactionForGroup(
      group,
      [createIx],
    );
    return { mangoAccountPk, created: true, txSignature: status.signature };
  } catch (err) {
    // Handle races where another request creates the same account first.
    const raceAi = await airdrop.connection.getAccountInfo(
      mangoAccountPk,
      HARNESS_COMMITMENT,
    );
    if (raceAi) {
      return { mangoAccountPk, created: false, txSignature: null };
    }
    throw err;
  }
}

async function processAirdropDepositRequest(
  req: IncomingMessage,
  url: URL,
  airdrop: AirdropContext | null,
): Promise<{
  ok: boolean;
  owner: string;
  mango_account: string;
  group: string;
  mint: string;
  ui_amount: number;
  raw_amount: string;
  unsafe_deposit_tx_signature: string;
  execution_path: 'unsafe_deposit' | 'token_deposit_into_existing_fallback';
  auto_created_mango_account: boolean;
  unsafe_account_create_tx_signature: string | null;
}> {
  if (!airdrop) {
    throw new Error('airdrop endpoint is disabled');
  }
  if (!airdrop.mangoClient || !airdrop.groupPk) {
    throw new Error(
      'airdrop-deposit endpoint is disabled: group/client not configured',
    );
  }

  const body = await readBody(req);
  const payload = body.trim().length ? JSON.parse(body) : {};
  if (
    payload?.ui_amount !== undefined &&
    Number(payload.ui_amount) !== airdrop.depositUiAmount
  ) {
    throw new Error(
      `ui_amount is fixed to ${airdrop.depositUiAmount} for /airdrop-deposit`,
    );
  }

  const ownerRaw = parseOwnerFromAirdropRequest(req, url, payload);
  const owner = new PublicKey(ownerRaw);
  const group = await airdrop.mangoClient.getGroup(airdrop.groupPk);
  let autoCreatedMangoAccount = false;
  let unsafeAccountCreateTxSignature: string | null = null;

  let mangoAccount;
  const requestedMangoAccount = payload?.mango_account as string | undefined;
  if (requestedMangoAccount?.length) {
    mangoAccount = await airdrop.mangoClient.getMangoAccount(
      new PublicKey(requestedMangoAccount),
    );
    if (!mangoAccount.owner.equals(owner)) {
      throw new Error('mango_account owner mismatch');
    }
    if (!mangoAccount.group.equals(group.publicKey)) {
      throw new Error('mango_account group mismatch');
    }
  } else {
    const ownerAccounts = await airdrop.mangoClient.getMangoAccountsForOwner(
      group,
      owner,
    );
    if (!ownerAccounts.length) {
      if (!HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT) {
        throw new Error('no mango account found for owner in configured group');
      }
      const requestedAccountNum = payload?.account_num;
      const accountNum =
        requestedAccountNum === undefined
          ? HARNESS_AIRDROP_AUTO_CREATE_ACCOUNT_NUM
          : Number(requestedAccountNum);
      if (
        !Number.isInteger(accountNum) ||
        accountNum < 0 ||
        accountNum > 0xffffffff
      ) {
        throw new Error('account_num must be a valid u32');
      }
      const createResult = await createUnsafeMangoAccountForOwner(
        airdrop,
        group,
        owner,
        accountNum,
      );
      mangoAccount = await airdrop.mangoClient.getMangoAccount(
        createResult.mangoAccountPk,
      );
      autoCreatedMangoAccount = createResult.created;
      unsafeAccountCreateTxSignature = createResult.txSignature;
    } else {
      mangoAccount = ownerAccounts[0];
    }
  }

  const rawAmount = toRawAmount(airdrop.depositUiAmount, airdrop.decimals);
  const usdcBank = group.getFirstBankByMint(airdrop.usdcMint);
  const ixData = Buffer.concat([
    anchorDiscriminator('unsafe_deposit'),
    u64ToLe(rawAmount),
  ]);
  const unsafeDepositIx = new TransactionInstruction({
    programId: airdrop.programId,
    keys: [
      { pubkey: group.publicKey, isSigner: false, isWritable: false },
      { pubkey: mangoAccount.publicKey, isSigner: false, isWritable: true },
      { pubkey: airdrop.faucet.publicKey, isSigner: true, isWritable: false },
      { pubkey: usdcBank.publicKey, isSigner: false, isWritable: true },
      { pubkey: usdcBank.oracle, isSigner: false, isWritable: false },
    ],
    data: ixData,
  });
  let unsafeDepositSignature = '';
  let executionPath: 'unsafe_deposit' | 'token_deposit_into_existing_fallback' =
    'unsafe_deposit';
  try {
    const unsafeDepositStatus =
      await airdrop.mangoClient.sendAndConfirmTransactionForGroup(group, [
        unsafeDepositIx,
      ]);
    unsafeDepositSignature = unsafeDepositStatus.signature;
  } catch (err: any) {
    const asText = err?.message || `${err}`;
    const missingUnsafeDepositIx =
      asText.includes('"Custom":101') ||
      asText.includes('InstructionFallbackNotFound');
    if (!missingUnsafeDepositIx) {
      throw err;
    }

    // Fallback path for nodes still running older program binaries.
    const faucetTokenAccount = await createAssociatedTokenAccountIdempotent(
      airdrop.connection,
      airdrop.faucet,
      airdrop.usdcMint,
      airdrop.faucet.publicKey,
    );
    await mintTo(
      airdrop.connection,
      airdrop.faucet,
      airdrop.usdcMint,
      faucetTokenAccount,
      airdrop.faucet,
      rawAmount,
    );
    const fallbackStatus = await airdrop.mangoClient.tokenDepositNative(
      group,
      mangoAccount,
      airdrop.usdcMint,
      new BN(rawAmount.toString()),
      false,
      true,
    );
    unsafeDepositSignature = fallbackStatus.signature;
    executionPath = 'token_deposit_into_existing_fallback';
  }

  return {
    ok: true,
    owner: owner.toBase58(),
    mango_account: mangoAccount.publicKey.toBase58(),
    group: group.publicKey.toBase58(),
    mint: airdrop.usdcMint.toBase58(),
    ui_amount: airdrop.depositUiAmount,
    raw_amount: rawAmount.toString(),
    unsafe_deposit_tx_signature: unsafeDepositSignature,
    execution_path: executionPath,
    auto_created_mango_account: autoCreatedMangoAccount,
    unsafe_account_create_tx_signature: unsafeAccountCreateTxSignature,
  };
}

async function processDepositContextRequest(
  url: URL,
  onchain: OnchainContext | null,
): Promise<{
  owner: string;
  group: string;
  program_id: string;
  quote_mint: string;
  quote_decimals: number;
  quote_bank: string;
  quote_vault: string;
  quote_oracle: string;
  mango_account: string;
  mango_account_exists: boolean;
  account_num: number;
  health_remaining_accounts: string[];
  default_ui_amount: number;
}> {
  if (!onchain?.mangoClient || !onchain.groupPk) {
    throw new Error('deposit context unavailable: group/client not configured');
  }

  const ownerRaw = decodeURIComponent(
    url.pathname.split('/').pop() || '',
  ).trim();
  if (!ownerRaw.length) {
    throw new Error('owner is required');
  }
  const owner = new PublicKey(ownerRaw);
  const group = await getFreshGroup(onchain);
  if (!group) {
    throw new Error('deposit context unavailable: group could not be loaded');
  }

  const quoteMint =
    onchain.usdcMint ?? group.getFirstBankForPerpSettlement().mint;
  const quoteBank = group.getFirstBankByMint(quoteMint);
  const requestedAccountNumRaw = url.searchParams.get('account_num');
  const requestedAccountNum = requestedAccountNumRaw
    ? Number(requestedAccountNumRaw)
    : 0;
  if (
    !Number.isInteger(requestedAccountNum) ||
    requestedAccountNum < 0 ||
    requestedAccountNum > 0xffffffff
  ) {
    throw new Error('account_num must be a valid u32');
  }

  let mangoAccountPk: PublicKey;
  let mangoAccountExists = false;
  let healthRemainingAccounts: PublicKey[] = [];

  const requestedMangoAccountRaw = url.searchParams.get('mango_account') || '';
  if (requestedMangoAccountRaw.length) {
    const mangoAccount = await onchain.mangoClient.getMangoAccount(
      new PublicKey(requestedMangoAccountRaw),
    );
    if (!mangoAccount.owner.equals(owner)) {
      throw new Error('mango_account owner mismatch');
    }
    if (!mangoAccount.group.equals(group.publicKey)) {
      throw new Error('mango_account group mismatch');
    }
    mangoAccountPk = mangoAccount.publicKey;
    mangoAccountExists = true;
    healthRemainingAccounts =
      await onchain.mangoClient.buildHealthRemainingAccounts(
        group,
        [mangoAccount],
        [quoteBank],
        [],
      );
  } else {
    const ownerAccounts = await onchain.mangoClient.getMangoAccountsForOwner(
      group,
      owner,
    );
    const requestedAccount = ownerAccounts.find(
      (account) => account.accountNum === requestedAccountNum,
    );
    const mangoAccount = requestedAccount ?? ownerAccounts[0] ?? null;
    if (mangoAccount) {
      mangoAccountPk = mangoAccount.publicKey;
      mangoAccountExists = true;
      healthRemainingAccounts =
        await onchain.mangoClient.buildHealthRemainingAccounts(
          group,
          [mangoAccount],
          [quoteBank],
          [],
        );
    } else {
      [mangoAccountPk] = PublicKey.findProgramAddressSync(
        [
          Buffer.from('MangoAccount'),
          group.publicKey.toBuffer(),
          owner.toBuffer(),
          u32ToLe(requestedAccountNum),
        ],
        onchain.programId,
      );
      const fallbackMap =
        await onchain.mangoClient.deriveFallbackOracleContexts(group);
      const fallbackAccounts =
        fallbackMap.get(quoteBank.oracle.toBase58()) || [];
      healthRemainingAccounts = [quoteBank.publicKey, quoteBank.oracle];
      for (const fallback of fallbackAccounts) {
        if (
          !fallback.equals(PublicKey.default) &&
          !healthRemainingAccounts.find((existing) => existing.equals(fallback))
        ) {
          healthRemainingAccounts.push(fallback);
        }
      }
    }
  }

  return {
    owner: owner.toBase58(),
    group: group.publicKey.toBase58(),
    program_id: onchain.programId.toBase58(),
    quote_mint: quoteMint.toBase58(),
    quote_decimals: quoteBank.mintDecimals,
    quote_bank: quoteBank.publicKey.toBase58(),
    quote_vault: quoteBank.vault.toBase58(),
    quote_oracle: quoteBank.oracle.toBase58(),
    mango_account: mangoAccountPk.toBase58(),
    mango_account_exists: mangoAccountExists,
    account_num: requestedAccountNum,
    health_remaining_accounts: healthRemainingAccounts.map((pk) =>
      pk.toBase58(),
    ),
    default_ui_amount: HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT,
  };
}

async function buildLaneForOwner(
  ownerRaw: string,
  onchain: OnchainContext | null,
): Promise<{
  ok: boolean;
  owner: string;
  mango_account: string;
  lane: {
    name: string;
    remainingAccounts: Array<{
      pubkey: string;
      isWritable: boolean;
      isSigner: boolean;
    }>;
  };
  accounts_hash: string;
}> {
  if (!onchain?.mangoClient || !onchain.groupPk) {
    throw new Error(
      'lane registration unavailable: group/client not configured',
    );
  }
  const owner = new PublicKey(ownerRaw);
  const group = await getFreshGroup(onchain);
  if (!group) {
    throw new Error('lane registration unavailable: group could not be loaded');
  }

  // Resolve Mango account
  const ownerAccounts = await onchain.mangoClient.getMangoAccountsForOwner(
    group,
    owner,
  );
  const mangoAccount = ownerAccounts[0];
  if (!mangoAccount) {
    throw new Error(`no Mango account found for owner ${ownerRaw}`);
  }

  // Get first perp market
  const perpMarkets = Array.from(group.perpMarketsMapByMarketIndex.values());
  if (!perpMarkets.length) {
    throw new Error('no perp markets configured');
  }
  const perpMarket = perpMarkets[0];

  const executionQueuePk = new PublicKey(
    process.env.EXECUTION_QUEUE_PK ||
      process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
      '',
  );

  // Build remaining_accounts in the EXACT order the frontend/relayer uses.
  // This must match what remapLaneAccountsForOwner produces.
  const remainingAccounts = [
    { pubkey: group.publicKey, isSigner: false, isWritable: false },
    { pubkey: mangoAccount.publicKey, isSigner: false, isWritable: true },
    { pubkey: owner, isSigner: false, isWritable: false },
    { pubkey: perpMarket.publicKey, isSigner: false, isWritable: true },
    { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
    { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
    { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
    { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
  ];

  // Check if there are additional health accounts from the lane config
  // by reading the static lane file and matching the account count
  const laneConfigPath =
    process.env.EXECUTION_QUEUE_CRANK_LANES_JSON_PATH || '';
  if (laneConfigPath && fs.existsSync(laneConfigPath)) {
    try {
      const staticLanes = JSON.parse(fs.readFileSync(laneConfigPath, 'utf-8'));
      const templateLane = staticLanes.find(
        (l: { name?: string; remainingAccounts: unknown[] }) =>
          l.remainingAccounts.length > remainingAccounts.length &&
          (l.name || '').includes('place'),
      );
      if (templateLane) {
        // Append any extra accounts from the template that aren't user-specific
        const knownPubkeys = new Set(
          remainingAccounts.map((a) => a.pubkey.toString()),
        );
        for (
          let i = remainingAccounts.length;
          i < templateLane.remainingAccounts.length;
          i++
        ) {
          const ta = templateLane.remainingAccounts[i];
          if (!knownPubkeys.has(ta.pubkey)) {
            remainingAccounts.push({
              pubkey: new PublicKey(ta.pubkey),
              isSigner: ta.isSigner ?? false,
              isWritable: ta.isWritable ?? false,
            });
          } else {
            // Shared account — keep same flags as template
            remainingAccounts.push({
              pubkey: new PublicKey(ta.pubkey),
              isSigner: ta.isSigner ?? false,
              isWritable: ta.isWritable ?? false,
            });
          }
        }
      }
    } catch {
      // ignore lane config read failures
    }
  }

  // Compute accounts hash the same way the relayer does
  const accountsForHash = remainingAccounts.map((a) => ({
    pubkey: a.pubkey instanceof PublicKey ? a.pubkey : new PublicKey(a.pubkey),
    isSigner: a.isSigner,
    isWritable: a.isWritable,
  }));

  // Merge runtime flags with fixed accounts (group=writable, queue=writable, sysvar=readonly)
  const fixedAccounts = [
    { pubkey: group.publicKey, isSigner: false, isWritable: true },
    { pubkey: executionQueuePk, isSigner: false, isWritable: true },
    {
      pubkey: new PublicKey('Sysvar1nstructions1111111111111111111111111'),
      isSigner: false,
      isWritable: false,
    },
  ];
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const a of [...fixedAccounts, ...accountsForHash]) {
    const key = a.pubkey.toBase58();
    const existing = merged.get(key);
    if (existing) {
      existing.isSigner = existing.isSigner || a.isSigner;
      existing.isWritable = existing.isWritable || a.isWritable;
    } else {
      merged.set(key, { isSigner: a.isSigner, isWritable: a.isWritable });
    }
  }
  const effectiveAccounts = accountsForHash.map((a) => {
    const key = a.pubkey.toBase58();
    const flags = merged.get(key)!;
    return {
      pubkey: a.pubkey,
      isSigner: flags.isSigner,
      isWritable: flags.isWritable,
    };
  });

  const hashData = Buffer.concat(
    effectiveAccounts.map((a) =>
      Buffer.concat([
        a.pubkey.toBuffer(),
        Buffer.from([a.isSigner ? 1 : 0, a.isWritable ? 1 : 0]),
      ]),
    ),
  );
  const accountsHash = createHash('sha256').update(hashData).digest('hex');

  // Write to lane cache if path is configured
  const laneCachePath = process.env.EXECUTION_QUEUE_LANE_CACHE_PATH || '';
  const laneName = `registered-${owner.toBase58().slice(0, 12)}`;
  if (laneCachePath) {
    let cache: Array<{
      name: string;
      user_owner: string;
      remainingAccounts: Array<{
        pubkey: string;
        isWritable: boolean;
        isSigner: boolean;
      }>;
    }> = [];
    try {
      if (fs.existsSync(laneCachePath)) {
        cache = JSON.parse(fs.readFileSync(laneCachePath, 'utf-8'));
      }
    } catch {
      cache = [];
    }
    cache = cache.filter((l) => l.user_owner !== ownerRaw);
    cache.push({
      name: laneName,
      user_owner: ownerRaw,
      remainingAccounts: remainingAccounts.map((a) => ({
        pubkey: (a.pubkey instanceof PublicKey
          ? a.pubkey
          : new PublicKey(a.pubkey)
        ).toBase58(),
        isWritable: a.isWritable,
        isSigner: a.isSigner,
      })),
    });
    fs.writeFileSync(laneCachePath, JSON.stringify(cache, null, 2));
  }

  return {
    ok: true,
    owner: owner.toBase58(),
    mango_account: mangoAccount.publicKey.toBase58(),
    lane: {
      name: laneName,
      remainingAccounts: remainingAccounts.map((a) => ({
        pubkey: (a.pubkey instanceof PublicKey
          ? a.pubkey
          : new PublicKey(a.pubkey)
        ).toBase58(),
        isWritable: a.isWritable,
        isSigner: a.isSigner,
      })),
    },
    accounts_hash: accountsHash,
  };
}

function initializeSse(res: ServerResponse): void {
  res.statusCode = 200;
  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache');
  res.setHeader('Connection', 'keep-alive');
  res.setHeader('X-Accel-Buffering', 'no');
  res.flushHeaders?.();
}

function rejectWhenSseCapacityExceeded(res: ServerResponse): boolean {
  if (
    HARNESS_MAX_SSE_CLIENTS > 0 &&
    currentSseClientCount() >= HARNESS_MAX_SSE_CLIENTS
  ) {
    writeJson(res, 503, {
      error: 'too_many_sse_clients',
      limit: HARNESS_MAX_SSE_CLIENTS,
    });
    return true;
  }
  return false;
}

function tradeCursorKey(
  view: QueueView,
  market: string | null,
  owner: string | null,
): string {
  return `${view}:${market || '*'}:${owner || '*'}`;
}

function primeTradeCursor(
  view: QueueView,
  market: string | null,
  owner: string | null,
): void {
  const key = tradeCursorKey(view, market, owner);
  if (tradeStreamCursors.has(key)) {
    return;
  }
  const trades =
    market || owner
      ? engine.getTradesFiltered({ view, market, owner, limit: 5000 })
      : engine.getAllTrades(view, 5000);
  tradeStreamCursors.set(key, trades[trades.length - 1]?.trade_id || null);
}

function pullTradeDelta(
  view: QueueView,
  market: string | null,
  owner: string | null,
): MarketTrade[] {
  const key = tradeCursorKey(view, market, owner);
  const trades =
    market || owner
      ? engine.getTradesFiltered({ view, market, owner, limit: 5000 })
      : engine.getAllTrades(view, 5000);
  const previousLastTradeId = tradeStreamCursors.get(key);
  const nextLastTradeId = trades[trades.length - 1]?.trade_id || null;
  tradeStreamCursors.set(key, nextLastTradeId);
  if (previousLastTradeId === undefined) {
    return [];
  }
  if (!previousLastTradeId) {
    return trades;
  }
  const idx = trades.findIndex(
    (trade) => trade.trade_id === previousLastTradeId,
  );
  if (idx >= 0) {
    return trades.slice(idx + 1);
  }
  return trades.length ? trades.slice(-1) : [];
}

function dropTradeCursorIfUnused(
  view: QueueView,
  market: string | null,
  owner: string | null,
): void {
  for (const subscriber of tradeStreamSubscribers.values()) {
    if (
      subscriber.view === view &&
      subscriber.market === market &&
      subscriber.owner === owner
    ) {
      return;
    }
  }
  tradeStreamCursors.delete(tradeCursorKey(view, market, owner));
}

function deriveEventContext(event: HarnessEvent): {
  market: string | null;
  owner: string | null;
  mangoAccount: string | null;
} {
  if (event.event_type === 'relay_intent_accepted') {
    return {
      market: event.market,
      owner: event.user_owner,
      mangoAccount: event.mango_account,
    };
  }
  if (
    event.event_type === 'queue_item_enqueued' ||
    event.event_type === 'queue_item_processed'
  ) {
    const intent = engine.findIntent(event.group, event.sequence, event.kind);
    return {
      market: intent?.market || null,
      owner: intent?.user_owner || null,
      mangoAccount: intent?.mango_account || null,
    };
  }
  return {
    market: null,
    owner: null,
    mangoAccount: null,
  };
}

async function buildFrontendSnapshotPayload(
  subscriber: FrontendStreamSubscriber,
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
  cache?: FrontendPayloadBuildCache,
): Promise<{
  owner: FrontendOwnerSlice | null;
  market: FrontendMarketSlice | null;
}> {
  const snapshot =
    cache?.snapshotsByView.get(subscriber.view) ||
    getSnapshotForView(subscriber.view, onchainSync);
  cache?.snapshotsByView.set(subscriber.view, snapshot);
  const metadataMap = cache
    ? await cache.metadataMapPromise
    : await getMarketMetadataMapSafe(onchain);
  const resolvedOwner = resolveOwnerFromSnapshot(
    snapshot,
    subscriber.owner,
    subscriber.mangoAccount,
  );

  const getCachedTrades = (
    key: string,
    loader: () => MarketTrade[],
  ): MarketTrade[] => {
    const existing = cache?.tradeCache.get(key);
    if (existing) {
      return existing;
    }
    const next = loader();
    cache?.tradeCache.set(key, next);
    return next;
  };

  return {
    owner:
      resolvedOwner &&
      (subscriber.include.has('positions') ||
        subscriber.include.has('open_orders') ||
        subscriber.include.has('trades') ||
        subscriber.include.has('account_metrics'))
        ? await (cache?.ownerSliceCache.get(
            [
              subscriber.view,
              resolvedOwner,
              subscriber.mangoAccount || '*',
              subscriber.market || '*',
              subscriber.tradesLimit,
            ].join(':'),
          ) ||
            (() => {
              const cacheKey = [
                subscriber.view,
                resolvedOwner,
                subscriber.mangoAccount || '*',
                subscriber.market || '*',
                subscriber.tradesLimit,
              ].join(':');
              const promise = buildFrontendOwnerSlice(
                snapshot,
                resolvedOwner,
                subscriber.mangoAccount,
                subscriber.market,
                subscriber.view,
                subscriber.tradesLimit,
                onchain,
                getCachedTrades(
                  [
                    'owner',
                    subscriber.view,
                    resolvedOwner,
                    subscriber.market || '*',
                    subscriber.tradesLimit,
                  ].join(':'),
                  () =>
                    engine.getTradesFiltered({
                      view: subscriber.view,
                      owner: resolvedOwner,
                      market: subscriber.market,
                      limit: subscriber.tradesLimit,
                    }),
                ),
              );
              cache?.ownerSliceCache.set(cacheKey, promise);
              return promise;
            })())
        : null,
    market:
      subscriber.market &&
      (subscriber.include.has('market_metrics') ||
        subscriber.include.has('trade_summary') ||
        subscriber.include.has('orderbook') ||
        subscriber.include.has('orderbook_summary'))
        ? await (cache?.marketSliceCache.get(
            [
              subscriber.view,
              subscriber.market,
              subscriber.depth,
              subscriber.orderbookMode,
            ].join(':'),
          ) ||
            (() => {
              const cacheKey = [
                subscriber.view,
                subscriber.market,
                subscriber.depth,
                subscriber.orderbookMode,
              ].join(':');
              const promise = buildFrontendMarketSlice(
                subscriber.market,
                getMarketStateForView(
                  subscriber.market,
                  subscriber.view,
                  onchainSync,
                ),
                metadataMap[subscriber.market] || null,
                onchain,
                subscriber.view,
                subscriber.depth,
                subscriber.orderbookMode,
                getCachedTrades(
                  ['market', subscriber.view, subscriber.market, '5000'].join(
                    ':',
                  ),
                  () =>
                    engine.getTrades(subscriber.market!, subscriber.view, 5000),
                ),
              );
              cache?.marketSliceCache.set(cacheKey, promise);
              return promise;
            })())
        : null,
  };
}

async function notifyTradeStreamSubscribers(
  event?: HarnessEvent | null,
): Promise<void> {
  if (!tradeStreamSubscribers.size) {
    return;
  }
  await measureAsync(
    'notify_trade_stream_subscribers',
    async () => {
      const context = event ? deriveEventContext(event) : null;
      const deltaByKey = new Map<string, MarketTrade[]>();
      for (const subscriber of Array.from(tradeStreamSubscribers.values())) {
        if (
          context &&
          subscriber.market &&
          context.market &&
          subscriber.market !== context.market
        ) {
          continue;
        }
        const key = tradeCursorKey(
          subscriber.view,
          subscriber.market,
          subscriber.owner,
        );
        if (!deltaByKey.has(key)) {
          deltaByKey.set(
            key,
            pullTradeDelta(
              subscriber.view,
              subscriber.market,
              subscriber.owner,
            ),
          );
        }
        for (const trade of deltaByKey.get(key) || []) {
          if (
            !writeSseEvent(
              subscriber.res,
              'trade',
              trade,
              'trade_stream_notify',
            )
          ) {
            tradeStreamSubscribers.delete(subscriber.id);
            dropTradeCursorIfUnused(
              subscriber.view,
              subscriber.market,
              subscriber.owner,
            );
            break;
          }
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_SSE_NOTIFY_MS,
      context: {
        subscribers: tradeStreamSubscribers.size,
        event_type: event?.event_type || 'full_refresh',
      },
    },
  );
}

async function notifyFrontendSubscribers(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
  options: {
    owner?: string | null;
    mangoAccount?: string | null;
    market?: string | null;
    forceAll?: boolean;
  } = {},
): Promise<void> {
  if (!frontendStreamSubscribers.size) {
    return;
  }
  await measureAsync(
    'notify_frontend_subscribers',
    async () => {
      const buildCache: FrontendPayloadBuildCache = {
        snapshotsByView: new Map(),
        metadataMapPromise: getMarketMetadataMapSafe(onchain),
        ownerSliceCache: new Map(),
        marketSliceCache: new Map(),
        tradeCache: new Map(),
      };

      for (const subscriber of Array.from(frontendStreamSubscribers.values())) {
        const shouldConsiderOwner =
          options.forceAll ||
          (!!options.owner &&
            (!subscriber.owner || subscriber.owner === options.owner)) ||
          (!!options.mangoAccount &&
            (!subscriber.mangoAccount ||
              subscriber.mangoAccount === options.mangoAccount));
        const shouldConsiderMarket =
          options.forceAll ||
          (!!options.market &&
            (!subscriber.market || subscriber.market === options.market));
        if (
          !options.forceAll &&
          !shouldConsiderOwner &&
          !shouldConsiderMarket
        ) {
          continue;
        }

        const payload = await buildFrontendSnapshotPayload(
          subscriber,
          onchain,
          onchainSync,
          buildCache,
        );
        if (payload.owner) {
          const nextSignature = JSON.stringify(payload.owner);
          if (subscriber.ownerSignature !== nextSignature) {
            subscriber.ownerSignature = nextSignature;
            if (
              !writeSseEvent(
                subscriber.res,
                'account_update',
                payload.owner,
                'frontend_stream_account_update',
              )
            ) {
              frontendStreamSubscribers.delete(subscriber.id);
              continue;
            }
          }
        }
        if (payload.market) {
          const nextSignature = JSON.stringify(payload.market);
          if (subscriber.marketSignature !== nextSignature) {
            subscriber.marketSignature = nextSignature;
            if (
              !writeSseEvent(
                subscriber.res,
                'market_update',
                payload.market,
                'frontend_stream_market_update',
              )
            ) {
              frontendStreamSubscribers.delete(subscriber.id);
              continue;
            }
          }
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_SSE_NOTIFY_MS,
      context: {
        subscribers: frontendStreamSubscribers.size,
        force_all: !!options.forceAll,
        owner: options.owner || 'all',
        mango_account: options.mangoAccount || 'all',
        market: options.market || 'all',
      },
    },
  );
}

function scheduleTradeStreamNotification(event: HarnessEvent): void {
  if (tradeStreamNotifyRunning) {
    tradeStreamNotifyPendingForceAll = true;
    return;
  }
  tradeStreamNotifyRunning = true;
  void (async () => {
    let nextEvent: HarnessEvent | null | undefined = event;
    let forceAll = false;
    while (nextEvent || forceAll) {
      try {
        await notifyTradeStreamSubscribers(forceAll ? null : nextEvent);
      } catch (err) {
        recordRuntimeError('trade_stream_notify', err, {
          force_all: forceAll,
        });
      }
      if (tradeStreamNotifyPendingForceAll) {
        tradeStreamNotifyPendingForceAll = false;
        nextEvent = null;
        forceAll = true;
      } else {
        nextEvent = null;
        forceAll = false;
      }
    }
  })()
    .catch((err) => {
      recordRuntimeError('trade_stream_notify_loop', err);
    })
    .finally(() => {
      tradeStreamNotifyRunning = false;
      if (tradeStreamNotifyPendingForceAll) {
        tradeStreamNotifyPendingForceAll = false;
        scheduleTradeStreamNotification({
          event_type: 'divergence_event',
          ts_ms: Date.now(),
          reason: 'coalesced_trade_stream_recovery',
          key: 'trade-stream',
          details: {},
        });
      }
    });
}

function scheduleFrontendStreamNotification(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
  options: {
    owner?: string | null;
    mangoAccount?: string | null;
    market?: string | null;
    forceAll?: boolean;
  } = {},
): void {
  if (frontendStreamNotifyRunning) {
    frontendStreamNotifyPendingForceAll = true;
    return;
  }
  frontendStreamNotifyRunning = true;
  void (async () => {
    let currentOptions = options;
    while (true) {
      try {
        await notifyFrontendSubscribers(onchain, onchainSync, currentOptions);
      } catch (err) {
        recordRuntimeError('frontend_stream_notify', err, {
          force_all: !!currentOptions.forceAll,
        });
      }
      if (!frontendStreamNotifyPendingForceAll) {
        break;
      }
      frontendStreamNotifyPendingForceAll = false;
      currentOptions = { forceAll: true };
    }
  })()
    .catch((err) => {
      recordRuntimeError('frontend_stream_notify_loop', err);
    })
    .finally(() => {
      frontendStreamNotifyRunning = false;
      if (frontendStreamNotifyPendingForceAll) {
        frontendStreamNotifyPendingForceAll = false;
        scheduleFrontendStreamNotification(onchain, onchainSync, {
          forceAll: true,
        });
      }
    });
}

function writeHeartbeatToAllStreams(): void {
  measureSync(
    'stream_heartbeat',
    () => {
      const heartbeat = `: heartbeat ${Date.now()}\n\n`;
      for (const client of Array.from(sseClients)) {
        if (!safeWriteResponse(client, heartbeat, 'sse_heartbeat')) {
          sseClients.delete(client);
        }
      }
      for (const subscriber of Array.from(tradeStreamSubscribers.values())) {
        if (
          !safeWriteResponse(
            subscriber.res,
            heartbeat,
            'trade_stream_heartbeat',
            {
              subscriber: subscriber.id,
            },
          )
        ) {
          tradeStreamSubscribers.delete(subscriber.id);
          dropTradeCursorIfUnused(
            subscriber.view,
            subscriber.market,
            subscriber.owner,
          );
        }
      }
      for (const subscriber of Array.from(frontendStreamSubscribers.values())) {
        if (
          !safeWriteResponse(
            subscriber.res,
            heartbeat,
            'frontend_stream_heartbeat',
            {
              subscriber: subscriber.id,
            },
          )
        ) {
          frontendStreamSubscribers.delete(subscriber.id);
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_SSE_NOTIFY_MS,
      context: {
        clients: sseClients.size,
        trade_subscribers: tradeStreamSubscribers.size,
        frontend_subscribers: frontendStreamSubscribers.size,
      },
    },
  );
}

function startPeriodicTask(
  name: string,
  intervalMs: number,
  task: () => void | Promise<void>,
  opts: { runImmediately?: boolean } = {},
): void {
  const safeIntervalMs = Math.max(1000, intervalMs);
  const runImmediately = opts.runImmediately !== false;
  let running = false;

  const run = async () => {
    if (running) {
      return;
    }
    running = true;
    const startedAt = performance.now();
    try {
      await task();
    } catch (err) {
      recordRuntimeError(`periodic:${name}`, err);
    } finally {
      recordLatencySample(`periodic:${name}`, performance.now() - startedAt, {
        thresholdMs: HARNESS_SLOW_PERIODIC_TASK_MS,
        context: {
          interval_ms: safeIntervalMs,
        },
      });
      running = false;
      setTimeout(() => {
        void run();
      }, safeIntervalMs);
    }
  };

  if (runImmediately) {
    void run();
  } else {
    setTimeout(() => {
      void run();
    }, safeIntervalMs);
  }
}

function buildHttpServer(
  onchain: OnchainContext | null,
  airdrop: AirdropContext | null,
  onchainSync: OnchainSyncState,
): http.Server {
  const server = http.createServer(async (req, res) => {
    const requestStartedAt = performance.now();
    const requestMethod = req.method || 'GET';
    let requestPath = req.url || '';
    try {
      const url = parsePathAndQuery(req);
      const method = requestMethod;
      requestPath = url.pathname;

      if (method === 'GET' && url.pathname === '/healthz') {
        // healthz must NEVER fail — the relayer gates intent submission on it.
        // Wrap everything in try/catch so engine errors don't return 500.
        let stats: Record<string, unknown> = {};
        try {
          stats = statsSnapshot();
        } catch {
          /* engine error — report ok anyway */
        }
        const cachedMetadataCount = onchain?.cachedMarketMetadata
          ? Object.keys(onchain.cachedMarketMetadata).length
          : 0;
        writeJson(res, 200, {
          ok: true,
          ...stats,
          onchain_read_enabled: !!onchain?.groupPk && !!onchain?.mangoClient,
          market_metadata_total: cachedMetadataCount,
          airdrop_enabled: !!airdrop,
          airdrop_deposit_enabled: !!airdrop?.groupPk && !!airdrop?.mangoClient,
          reconcile_interval_ms: Math.max(1000, HARNESS_RECONCILE_INTERVAL_MS),
          last_reconcile_ts_ms: onchainSync.drift?.ts_ms || null,
          reconcile_markets_with_drift:
            onchainSync.drift?.totals.markets_with_drift || 0,
          last_runtime_error: runtimeErrors[runtimeErrors.length - 1] || null,
          fatal_startup_error: fatalStartupError,
          generated_ts_ms: Date.now(),
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/livez') {
        writeJson(res, 200, { ok: true, ts_ms: Date.now() });
        return;
      }

      if (method === 'GET' && url.pathname === '/metrics') {
        writeText(res, 200, metricsText());
        return;
      }

      if (method === 'GET' && url.pathname === '/diagnostics/errors') {
        const limit = parseNonNegativeInteger(
          url.searchParams.get('limit'),
          HARNESS_RUNTIME_ERROR_HISTORY_LIMIT,
          { min: 0, max: HARNESS_RUNTIME_ERROR_HISTORY_LIMIT },
        );
        writeJson(res, 200, {
          items: getRecentRuntimeErrors(limit),
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/diagnostics/latency') {
        const limit = parseNonNegativeInteger(
          url.searchParams.get('limit'),
          HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT,
          { min: 0, max: HARNESS_RUNTIME_LATENCY_HISTORY_LIMIT },
        );
        writeJson(res, 200, {
          items: getRecentLatencyEntries(limit),
          components: getLatencyAggregates(limit),
          event_loop: lastEventLoopLagSnapshot,
          totals: {
            samples: totalLatencySamples,
            slow_samples: totalSlowLatencySamples,
            error_samples: totalLatencyErrorSamples,
          },
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/diagnostics/divergence') {
        const limit = Number(url.searchParams.get('limit') || '200');
        writeJson(res, 200, {
          items: engine.listDivergences(limit),
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/diagnostics/reconciliation') {
        writeJson(res, 200, {
          data: onchainSync.drift,
          last_error: onchainSync.last_error,
        });
        return;
      }

      if (method === 'POST' && url.pathname === '/admin/replay') {
        const optimistic = engine.getSnapshot('optimistic');
        const confirmed = engine.getSnapshot('confirmed');
        writeJson(res, 200, {
          ok: true,
          optimistic_generated_ts_ms: optimistic.generated_ts_ms,
          confirmed_generated_ts_ms: confirmed.generated_ts_ms,
        });
        return;
      }

      if (method === 'POST' && url.pathname === '/admin/register-lane') {
        try {
          const body = JSON.parse(await readBody(req));
          const ownerPk = body.owner;
          if (!ownerPk) {
            writeJson(res, 400, { error: 'owner is required' });
            return;
          }
          const result = await buildLaneForOwner(ownerPk, onchain);
          writeJson(res, 200, result);
        } catch (err: any) {
          writeJson(res, 500, { error: err?.message || `${err}` });
        }
        return;
      }

      if (method === 'POST' && url.pathname === '/ingest/relay-intent') {
        if (!checkRelayIngestAuth(req)) {
          writeJson(res, 401, { error: 'unauthorized' });
          return;
        }
        const body = await readBody(req);
        const payload = parseRelayIntentEvent(JSON.parse(body));
        engine.ingestRelayIntent(payload);
        writeJson(res, 202, {
          ok: true,
          key: `${payload.group}:${payload.sequence}:${payload.kind}`,
        });
        return;
      }

      if (method === 'POST' && url.pathname === '/airdrop') {
        try {
          const result = await processAirdropRequest(req, url, airdrop);
          writeJson(res, 200, result);
        } catch (err: any) {
          const message = err?.message || `${err}`;
          const status =
            message.includes('required') ||
            message.includes('invalid') ||
            message.includes('disabled') ||
            message.includes('exceeds') ||
            message.includes('ui_amount')
              ? 400
              : 500;
          writeJson(res, status, { error: message });
        }
        return;
      }

      if (method === 'POST' && url.pathname === '/airdrop-deposit') {
        try {
          const result = await processAirdropDepositRequest(req, url, airdrop);
          writeJson(res, 200, result);
        } catch (err: any) {
          const message = err?.message || `${err}`;
          const status =
            message.includes('required') ||
            message.includes('invalid') ||
            message.includes('disabled') ||
            message.includes('exceeds') ||
            message.includes('ui_amount') ||
            message.includes('mismatch') ||
            message.includes('not found')
              ? 400
              : 500;
          writeJson(res, status, { error: message });
        }
        return;
      }

      if (
        method === 'GET' &&
        url.pathname.startsWith('/state/deposit-context/')
      ) {
        try {
          const result = await processDepositContextRequest(url, onchain);
          writeJson(res, 200, result);
        } catch (err: any) {
          const message = err?.message || `${err}`;
          const status =
            message.includes('required') ||
            message.includes('invalid') ||
            message.includes('disabled') ||
            message.includes('mismatch') ||
            message.includes('not configured') ||
            message.includes('not found')
              ? 400
              : 500;
          writeJson(res, status, { error: message });
        }
        return;
      }

      if (method === 'GET' && url.pathname === '/state/stream/trades') {
        if (rejectWhenSseCapacityExceeded(res)) {
          return;
        }
        const market = url.searchParams.get('market');
        const owner = url.searchParams.get('owner');
        const view = parseView(url);
        const backfillLimit = parseNonNegativeInteger(
          url.searchParams.get('backfill_n'),
          DEFAULT_STREAM_BACKFILL,
          { min: 0, max: 1000 },
        );
        initializeSse(res);
        const subscriber: TradeStreamSubscriber = {
          id: nextStreamSubscriberId(),
          res,
          view,
          market,
          owner,
          backfillLimit,
        };
        tradeStreamSubscribers.set(subscriber.id, subscriber);
        writeSseEvent(res, 'connected', {
          ts_ms: Date.now(),
          mode: HARNESS_MODE,
          view,
          market,
          owner,
        });
        writeSseEvent(res, 'snapshot', {
          view,
          market,
          owner,
          data: engine.getTradesFiltered({
            view,
            market,
            owner,
            limit: backfillLimit,
          }),
        });
        primeTradeCursor(view, market, owner);
        attachStreamCleanup(req, res, () => {
          tradeStreamSubscribers.delete(subscriber.id);
          dropTradeCursorIfUnused(
            subscriber.view,
            subscriber.market,
            subscriber.owner,
          );
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/state/stream/frontend') {
        if (rejectWhenSseCapacityExceeded(res)) {
          return;
        }
        const owner = url.searchParams.get('owner');
        const mangoAccount = url.searchParams.get('mango_account');
        const market = url.searchParams.get('market');
        if (!owner && !mangoAccount && !market) {
          writeJson(res, 400, {
            error:
              'frontend stream requires at least one of owner, mango_account, or market',
          });
          return;
        }
        const view = parseView(url);
        const depth = parseNonNegativeInteger(
          url.searchParams.get('depth'),
          DEFAULT_ORDERBOOK_DEPTH,
          { min: 1, max: 500 },
        );
        const tradesLimit = parseNonNegativeInteger(
          url.searchParams.get('trades_limit'),
          DEFAULT_FRONTEND_TRADES_LIMIT,
          { min: 0, max: 1000 },
        );
        const orderbookMode =
          (url.searchParams.get('orderbook') || 'summary').toLowerCase() ===
          'full'
            ? 'full'
            : 'summary';
        const defaultIncludes = [
          ...(owner || mangoAccount
            ? ['positions', 'open_orders', 'trades', 'account_metrics']
            : []),
          ...(market
            ? ['market_metrics', 'trade_summary', 'orderbook_summary']
            : []),
        ];
        if (market && orderbookMode === 'full') {
          defaultIncludes.push('orderbook');
        }
        const include = parseIncludeSet(url, defaultIncludes);
        initializeSse(res);
        const subscriber: FrontendStreamSubscriber = {
          id: nextStreamSubscriberId(),
          res,
          view,
          owner,
          mangoAccount,
          market,
          depth,
          tradesLimit,
          include,
          orderbookMode,
          ownerSignature: null,
          marketSignature: null,
        };
        frontendStreamSubscribers.set(subscriber.id, subscriber);
        const payload = await buildFrontendSnapshotPayload(
          subscriber,
          onchain,
          onchainSync,
        );
        subscriber.ownerSignature = payload.owner
          ? JSON.stringify(payload.owner)
          : null;
        subscriber.marketSignature = payload.market
          ? JSON.stringify(payload.market)
          : null;
        writeSseEvent(res, 'connected', {
          ts_ms: Date.now(),
          mode: HARNESS_MODE,
          view,
          owner,
          mango_account: mangoAccount,
          market,
        });
        writeSseEvent(res, 'snapshot', {
          view,
          owner: payload.owner,
          market: payload.market,
        });
        attachStreamCleanup(req, res, () => {
          frontendStreamSubscribers.delete(subscriber.id);
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/state/stream') {
        if (rejectWhenSseCapacityExceeded(res)) {
          return;
        }
        initializeSse(res);

        sseClients.add(res);
        writeSseEvent(res, 'connected', {
          ts_ms: Date.now(),
          mode: HARNESS_MODE,
        });

        const optimistic = engine.getSnapshot('optimistic');
        writeSseEvent(res, 'market_state_updated', {
          ts_ms: Date.now(),
          view: 'optimistic',
          markets: Object.keys(optimistic.markets).length,
        });

        attachStreamCleanup(req, res, () => {
          sseClients.delete(res);
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/state/markets') {
        const view = parseView(url);
        const snapshot = getSnapshotForView(view, onchainSync);
        const responseView = snapshot.view;
        const depth = parseNonNegativeInteger(
          url.searchParams.get('depth'),
          DEFAULT_ORDERBOOK_DEPTH,
          { min: 1, max: 500 },
        );
        const marketsFilter = parseCommaSeparatedList(
          url.searchParams.get('markets'),
        );
        const includeFullBook =
          (url.searchParams.get('book') || 'summary').toLowerCase() === 'full';
        writeJson(res, 200, {
          view: responseView,
          items: await buildMarketListItems(
            snapshot,
            onchain,
            responseView,
            marketsFilter,
            depth,
            includeFullBook,
          ),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/markets/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/markets/'.length),
        );
        const view = parseView(url);
        const snapshot = getSnapshotForView(view, onchainSync);
        const responseView = snapshot.view;
        const marketMetadata = await getMarketMetadataMapSafe(onchain);
        const data = getMarketStateForView(market, responseView, onchainSync);
        writeJson(res, 200, {
          view: responseView,
          metadata: marketMetadata[market] || null,
          orderbook_summary: buildOrderbookSummary(
            data,
            marketMetadata[market] || null,
            parseNonNegativeInteger(
              url.searchParams.get('depth'),
              DEFAULT_ORDERBOOK_DEPTH,
              { min: 1, max: 500 },
            ),
          ),
          trade_summary: buildTradeSummary(
            market,
            responseView,
            engine.getTrades(market, responseView, 5000),
            marketMetadata[market] || null,
          ),
          metrics: await getMarketRuntimeMetrics(
            market,
            data,
            marketMetadata[market] || null,
            onchain,
          ),
          data,
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/users/')) {
        const owner = decodeURIComponent(
          url.pathname.slice('/state/users/'.length),
        );
        const view = parseView(url);
        const snapshot = getSnapshotForView(view, onchainSync);
        const responseView = snapshot.view;
        const baseUserState = getUserStateForView(
          owner,
          responseView,
          onchainSync,
        );
        const includeOnchain =
          (
            url.searchParams.get('onchain') || 'true'
          ).toLowerCase() !== 'false';
        const data = includeOnchain
          ? await enrichOwnerStateWithOnchain(
              owner,
              baseUserState,
              onchain,
              responseView,
            )
          : baseUserState;
        writeJson(res, 200, {
          view: responseView,
          data,
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/balances/')) {
        const owner = decodeURIComponent(
          url.pathname.slice('/state/balances/'.length),
        );
        const view = parseView(url);
        const snapshot = getSnapshotForView(view, onchainSync);
        const responseView = snapshot.view;
        const baseBalances = getBalancesForView(
          owner,
          responseView,
          onchainSync,
        );
        const includeOnchain =
          (
            url.searchParams.get('onchain') || 'true'
          ).toLowerCase() !== 'false';
        const data = includeOnchain
          ? await enrichOwnerStateWithOnchain(
              owner,
              baseBalances,
              onchain,
              responseView,
            )
          : baseBalances;
        writeJson(res, 200, {
          view: responseView,
          data,
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/orders/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/orders/'.length),
        );
        const view = parseView(url);
        const owner = url.searchParams.get('owner');
        const responseView = view;
        const data = (
          getMarketStateForView(market, responseView, onchainSync).open_orders ||
          []
        ).filter((order) => !owner || order.owner === owner);
        writeJson(res, 200, {
          view: responseView,
          market,
          owner,
          data,
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/state/trades/summary') {
        const view = parseView(url);
        const market = url.searchParams.get('market');
        const owner = url.searchParams.get('owner');
        const snapshot = getSnapshotForView(view, onchainSync);
        const marketMetadata = await getMarketMetadataMapSafe(onchain);
        const items = buildTradeSummaryCollection(
          view,
          snapshot,
          marketMetadata,
          market,
          owner,
        );
        writeJson(
          res,
          200,
          market
            ? {
                view,
                market,
                owner,
                data:
                  items[0] ||
                  buildTradeSummary(
                    market,
                    view,
                    [],
                    marketMetadata[market] || null,
                  ),
              }
            : {
                view,
                market: null,
                owner,
                items,
              },
        );
        return;
      }

      if (method === 'GET' && url.pathname === '/state/trades') {
        const view = parseView(url);
        const market = url.searchParams.get('market');
        const owner = url.searchParams.get('owner');
        const limit = parseNonNegativeInteger(
          url.searchParams.get('limit'),
          200,
          {
            min: 0,
            max: 5000,
          },
        );
        writeJson(res, 200, {
          view,
          market,
          owner,
          data: engine.getTradesFiltered({
            view,
            market,
            owner,
            limit,
          }),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/trades/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/trades/'.length),
        );
        const view = parseView(url);
        const limit = parseNonNegativeInteger(
          url.searchParams.get('limit'),
          200,
          {
            min: 0,
            max: 5000,
          },
        );
        writeJson(res, 200, {
          view,
          market,
          data: engine.getTrades(market, view, limit),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/candles/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/candles/'.length),
        );
        const view = parseView(url);
        const limit = Number(url.searchParams.get('limit') || '200');
        const resolutionSec = Number(
          url.searchParams.get('resolution_sec') || '60',
        );
        writeJson(res, 200, {
          view,
          market,
          resolution_sec:
            Number.isFinite(resolutionSec) && resolutionSec > 0
              ? Math.floor(resolutionSec)
              : 60,
          data: engine.getCandles(
            market,
            view,
            resolutionSec,
            Number.isFinite(limit) ? Math.max(0, Math.floor(limit)) : 200,
          ),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/queue/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/queue/'.length),
        );
        const view = parseView(url);
        const snapshot = getSnapshotForView(view, onchainSync);
        writeJson(res, 200, {
          view,
          market,
          data: snapshot.queue[market] || emptyQueueState(market),
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/state/full') {
        const view = parseView(url);
        const market = url.searchParams.get('market');
        const snapshot = getSnapshotForView(view, onchainSync);
        const responseView = snapshot.view;
        const marketMetadata = await getMarketMetadataMapSafe(onchain);
        if (!market) {
          writeJson(res, 200, {
            ...snapshot,
            market_metadata: marketMetadata,
          });
          return;
        }
        const marketState = snapshot.markets[market] || null;
        const queueState = snapshot.queue[market] || null;
        const accounts = Object.fromEntries(
          Object.entries(snapshot.accounts || {}).filter(([, account]) => {
            return (
              account.open_orders.some((order) => order.market === market) ||
              account.perp_positions.some(
                (position) => `${position.market_index}` === market,
              )
            );
          }),
        );
        const users = Object.fromEntries(
          Object.entries(snapshot.users).filter(([, user]) => {
            return (
              user.open_orders.some((order) => order.market === market) ||
              user.per_market.some((position) => position.market === market) ||
              Object.values(accounts).some(
                (account) => account.owner === user.owner,
              )
            );
          }),
        );
        writeJson(res, 200, {
          view: responseView,
          generated_ts_ms: snapshot.generated_ts_ms,
          market: marketState,
          market_metadata: marketMetadata[market] || null,
          queue: queueState,
          users,
          accounts,
        });
        return;
      }

      writeJson(res, 404, {
        error: 'not_found',
        path: url.pathname,
      });
    } catch (err: any) {
      recordRuntimeError('http_request', err, {
        method: requestMethod,
        path: requestPath,
      });
      writeJson(res, 500, {
        error: err?.message || `${err}`,
      });
    } finally {
      recordLatencySample(
        'http_request',
        performance.now() - requestStartedAt,
        {
          thresholdMs: HARNESS_SLOW_HTTP_REQUEST_MS,
          context: {
            method: requestMethod,
            path: requestPath,
            status_code: res.statusCode || 0,
          },
        },
      );
    }
  });
  server.requestTimeout = HARNESS_HTTP_REQUEST_TIMEOUT_MS;
  server.headersTimeout = HARNESS_HTTP_HEADERS_TIMEOUT_MS;
  server.keepAliveTimeout = HARNESS_HTTP_KEEPALIVE_TIMEOUT_MS;
  if (HARNESS_MAX_HTTP_CONNECTIONS > 0) {
    server.maxConnections = HARNESS_MAX_HTTP_CONNECTIONS;
  }
  server.on('clientError', (err, socket) => {
    recordRuntimeError('http_client_error', err);
    if (socket.writable) {
      socket.end('HTTP/1.1 400 Bad Request\r\n\r\n');
    }
  });
  server.on('error', (err) => {
    recordRuntimeError('http_server_error', err);
  });
  return server;
}

function parseBindAddress(bindAddr: string): { host: string; port: number } {
  const parts = bindAddr.split(':');
  if (parts.length < 2) {
    throw new Error(`invalid bind addr, expected host:port, got ${bindAddr}`);
  }
  const host = parts.slice(0, parts.length - 1).join(':');
  const port = Number(parts[parts.length - 1]);
  if (!Number.isFinite(port) || port <= 0) {
    throw new Error(`invalid bind port: ${bindAddr}`);
  }
  return { host, port };
}

function installRuntimeErrorHandlers(): void {
  process.on('unhandledRejection', (reason) => {
    recordRuntimeError('process_unhandled_rejection', reason);
  });
  process.on('uncaughtException', (err) => {
    recordRuntimeError('process_uncaught_exception', err);
  });
  process.on('warning', (warning) => {
    recordRuntimeError('process_warning', warning, {
      name: warning.name,
    });
  });
}

function holdProcessForDiagnostics(): void {
  if (diagnosticsHoldIntervalStarted) {
    return;
  }
  diagnosticsHoldIntervalStarted = true;
  setInterval(() => {
    // Keep the process alive for external diagnostics if startup cannot recover.
  }, 60_000);
}

function startFatalServer(entry: RuntimeErrorEntry): void {
  const { host, port } = parseBindAddress(HARNESS_BIND_ADDR);
  const server = http.createServer((req, res) => {
    const url = parsePathAndQuery(req);
    if (url.pathname === '/livez') {
      writeJson(res, 200, { ok: true, degraded: true, ts_ms: Date.now() });
      return;
    }
    if (url.pathname === '/healthz') {
      writeJson(res, 503, {
        ok: false,
        degraded: true,
        backend: HARNESS_BACKEND,
        fatal_startup_error: entry,
        last_runtime_error: runtimeErrors[runtimeErrors.length - 1] || null,
        generated_ts_ms: Date.now(),
      });
      return;
    }
    if (url.pathname === '/diagnostics/errors') {
      writeJson(res, 200, { items: getRecentRuntimeErrors() });
      return;
    }
    if (url.pathname === '/diagnostics/latency') {
      writeJson(res, 200, {
        items: getRecentLatencyEntries(),
        components: getLatencyAggregates(),
        event_loop: lastEventLoopLagSnapshot,
      });
      return;
    }
    writeJson(res, 503, {
      ok: false,
      degraded: true,
      error: entry.message,
    });
  });
  server.requestTimeout = HARNESS_HTTP_REQUEST_TIMEOUT_MS;
  server.headersTimeout = HARNESS_HTTP_HEADERS_TIMEOUT_MS;
  server.keepAliveTimeout = HARNESS_HTTP_KEEPALIVE_TIMEOUT_MS;
  server.listen(port, host, () => {
    console.error(
      `Continuum state harness started in degraded mode on http://${host}:${port}`,
    );
  });
  server.on('error', (err) => {
    recordRuntimeError('fatal_http_server_error', err);
    holdProcessForDiagnostics();
  });
}

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }

  engine = instrumentBackend(
    await createContinuumHarnessBackend(HARNESS_BACKEND),
  );
  console.log(`Continuum harness backend: ${HARNESS_BACKEND}`);
  console.log(
    `Continuum harness instance: id=${HARNESS_INSTANCE_ID}, pid=${process.pid}, cwd=${process.cwd()}, bind=${HARNESS_BIND_ADDR}`,
  );

  const connection = new Connection(
    CLUSTER_URL,
    HARNESS_COMMITMENT === 'processed' ? 'confirmed' : HARNESS_COMMITMENT,
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];

  replayEventLogIfPresent();
  await measureAsync('startup.backfill_program_logs', async () => {
    await maybeBackfillProgramLogs(connection, programId);
  });
  const onchain = await measureAsync(
    'startup.build_onchain_context',
    async () => await buildOnchainContext(connection, programId),
  );
  let bootstrappedOnchainState:
    | {
        snapshot: EngineSnapshot;
        queue: OnchainQueueSnapshot | null;
      }
    | null = null;

  // ── Bootstrap engine from on-chain confirmed state ──────────────────
  // Seeds baseline positions so the harness starts in sync with on-chain
  // rather than from zero. This is critical for accuracy after restarts.
  if (onchain.groupPk && onchain.mangoClient) {
    console.log('Bootstrapping engine from on-chain confirmed state...');
    try {
      bootstrappedOnchainState = await measureAsync(
        'startup.build_onchain_confirmed_snapshot',
        async () => await buildOnchainConfirmedSnapshot(onchain),
      );
      if (bootstrappedOnchainState) {
        measureSync('startup.bootstrap_from_onchain_snapshot', () => {
          engine.bootstrapFromOnchainSnapshot(bootstrappedOnchainState.snapshot);
        });
        const userCount = Object.keys(
          bootstrappedOnchainState.snapshot.users,
        ).length;
        const marketCount = Object.keys(
          bootstrappedOnchainState.snapshot.markets,
        ).length;
        console.log(
          `On-chain bootstrap complete: ${userCount} users, ${marketCount} markets`,
        );
      } else {
        console.warn(
          'On-chain bootstrap returned null snapshot — starting from zero',
        );
      }
    } catch (err) {
      console.warn(`On-chain bootstrap failed (starting from zero): ${err}`);
    }
  }
  // ────────────────────────────────────────────────────────────────────

  const airdrop = await buildAirdropContext(onchain);
  const onchainSync: OnchainSyncState = {
    snapshot: bootstrappedOnchainState?.snapshot || null,
    drift: null,
    queue: bootstrappedOnchainState?.queue || null,
    last_error: null,
  };

  engine.subscribe((event) => {
    try {
      appendEventLog(event);
      broadcastEvent(event);
      const context = deriveEventContext(event);
      if (context.market) {
        marketRuntimeMetricsCache.delete(context.market);
      }
      scheduleTradeStreamNotification(event);
      scheduleFrontendStreamNotification(onchain, onchainSync, {
        owner: context.owner,
        mangoAccount: context.mangoAccount,
        market: context.market,
      });
    } catch (err) {
      recordRuntimeError('engine_subscriber', err, {
        event_type: event.event_type,
      });
    }
  });

  await connection.onLogs(
    programId,
    (logs, ctx) => {
      programLogHandlingQueue = programLogHandlingQueue.then(async () => {
        try {
          await handleProgramLogs(connection, logs, ctx.slot, onchain, onchainSync);
        } catch (err) {
          recordRuntimeError('connection_on_logs', err, {
            slot: ctx.slot,
            signature: logs.signature,
          });
        }
      });
    },
    HARNESS_COMMITMENT,
  );

  const server = buildHttpServer(onchain, airdrop, onchainSync);
  const { host, port } = parseBindAddress(HARNESS_BIND_ADDR);

  startPeriodicTask(
    'stream_heartbeat',
    15000,
    () => {
      writeHeartbeatToAllStreams();
    },
    { runImmediately: false },
  );

  startPeriodicTask('onchain_sanity', HARNESS_SANITY_INTERVAL_MS, async () => {
    await runOnchainBalanceSanityCheck(onchain);
  });

  startPeriodicTask(
    'onchain_reconciliation',
    HARNESS_RECONCILE_INTERVAL_MS,
    async () => {
      try {
        await runOnchainReconciliation(onchain, onchainSync);
        onchainSync.last_error = null;
        marketRuntimeMetricsCache.clear();
        scheduleFrontendStreamNotification(onchain, onchainSync, {
          forceAll: true,
        });
      } catch (err) {
        onchainSync.last_error = `${err}`;
        throw err;
      }
    },
  );

  startPeriodicTask(
    'frontend_market_refresh',
    HARNESS_ONCHAIN_CACHE_TTL_MS,
    async () => {
      marketRuntimeMetricsCache.clear();
      scheduleFrontendStreamNotification(onchain, onchainSync, {
        forceAll: true,
      });
    },
    { runImmediately: false },
  );

  startPeriodicTask(
    'event_loop_lag_sample',
    HARNESS_EVENT_LOOP_SAMPLE_INTERVAL_MS,
    () => {
      sampleEventLoopLag();
    },
    { runImmediately: false },
  );

  server.listen(port, host, () => {
    console.log(
      `Continuum state harness listening on http://${host}:${port}, mode=${HARNESS_MODE}, program=${programId.toBase58()}`,
    );
    if (onchain?.groupPk && onchain.mangoClient) {
      console.log(
        `Onchain read context enabled: group=${onchain.groupPk.toBase58()}, usdc_mint=${
          onchain.usdcMint?.toBase58() || 'unresolved'
        }`,
      );
    }
    if (airdrop) {
      console.log(
        `USDC airdrop enabled: mint=${airdrop.usdcMint.toBase58()}, default_ui_amount=${
          airdrop.defaultUiAmount
        }`,
      );
      if (airdrop.groupPk && airdrop.mangoClient) {
        console.log(
          `USDC airdrop-deposit enabled: group=${airdrop.groupPk.toBase58()}, ui_amount=${
            airdrop.depositUiAmount
          }`,
        );
      }
    }
  });
}

installRuntimeErrorHandlers();

void main().catch((err) => {
  fatalStartupError = recordRuntimeError('main_startup', err);
  try {
    startFatalServer(fatalStartupError);
  } catch (fatalErr) {
    recordRuntimeError('main_startup_fatal_server', fatalErr);
    holdProcessForDiagnostics();
  }
});
