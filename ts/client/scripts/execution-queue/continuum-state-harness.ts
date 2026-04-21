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
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import fs from 'fs';
// [redis-phase1a] redis event mirror (Phase 1a)
import { createPublisher, type Publisher, type HarnessEventLike } from './redis-publisher';
// [redis-phase1b] state mirror (book/position/balance)
import { createMirror, type Mirror } from './state-mirror';
// [redis-phase6] prom metrics listener + hooks
import { startMetricsServer, publisherMetricsHooks } from './metrics';
startMetricsServer();
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
  QueueItemProcessedEvent,
  QueuePayloadVariantHarness,
  QueueState,
  QueueView,
  RelayIntentAcceptedEvent,
  RelayIntentStatusEvent,
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
  ContinuumHarnessLaneGuard,
  LaneGuardDecision,
  LaneGuardMarketSuppression,
  relayIntentTrackingKey as buildRelayIntentTrackingKey,
} from '../../src/continuumHarnessLaneGuard';
import {
  decodeExecutionQueueHeader,
  decodeExecutionQueuePendingItems,
} from '../../src/executionQueueLayout';
import {
  decodeExecutionQueueV3MarketRoot,
  executionQueueV3NextEnqueueSequence,
} from '../../src/executionQueue';
import { I80F48, ZERO_I80F48 } from '../../src/numbers/I80F48';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const CLUSTER_WS_URL =
  process.env.CLUSTER_WS_URL_OVERRIDE || process.env.MB_CLUSTER_WS_URL || '';
const TRITON_RPC_URL = process.env.TRITON_RPC_URL || '';
const TRITON_WS_URL = process.env.TRITON_WS_URL || '';
const PROGRAM_ID_OVERRIDE = process.env.CONTINUUM_HARNESS_PROGRAM_ID;
const HARNESS_BIND_ADDR =
  process.env.CONTINUUM_HARNESS_BIND_ADDR || '0.0.0.0:9091';
const HARNESS_MODE = process.env.CONTINUUM_HARNESS_MODE || 'local';
const HARNESS_BACKEND: HarnessBackendKind = parseHarnessBackendKind(
  process.env.CONTINUUM_HARNESS_BACKEND,
);
const HARNESS_USE_LIGHTWEIGHT_WRAPPER_DEFAULTS =
  HARNESS_BACKEND === 'rust-backend';
const HARNESS_PROCESS_STARTED_TS_MS = Date.now();
const HARNESS_INSTANCE_ID = `${process.pid}-${HARNESS_PROCESS_STARTED_TS_MS}`;
const HARNESS_EVENT_LOG_PATH =
  process.env.CONTINUUM_HARNESS_EVENT_LOG_PATH ||
  '/tmp/continuum-harness-events.jsonl';
const HARNESS_TXN_LOG_PATH =
  process.env.CONTINUUM_HARNESS_TXN_LOG_PATH ||
  HARNESS_EVENT_LOG_PATH.replace(/\.jsonl$/i, '.txns.jsonl');
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
const HARNESS_EXECUTION_QUEUE_TOPOLOGY = (
  process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_TOPOLOGY ||
  process.env.EXECUTION_QUEUE_TOPOLOGY ||
  'v2'
).toLowerCase();
const HARNESS_USES_V3_QUEUE = HARNESS_EXECUTION_QUEUE_TOPOLOGY.startsWith('v3');
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
  process.env.CONTINUUM_HARNESS_SANITY_INTERVAL_MS ||
    (HARNESS_USE_LIGHTWEIGHT_WRAPPER_DEFAULTS ? '0' : '5000'),
);
const HARNESS_ONCHAIN_CACHE_TTL_MS = Number(
  process.env.CONTINUUM_HARNESS_ONCHAIN_CACHE_TTL_MS || '5000',
);
const HARNESS_MARKET_METADATA_CACHE_TTL_MS = Number(
  process.env.CONTINUUM_HARNESS_MARKET_METADATA_CACHE_TTL_MS || '60000',
);
const HARNESS_RECONCILE_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_RECONCILE_INTERVAL_MS ||
    (HARNESS_USE_LIGHTWEIGHT_WRAPPER_DEFAULTS ? '30000' : '10000'),
);
const HARNESS_DIRECT_ONCHAIN_REBASE_DEBOUNCE_MS = Number(
  process.env.CONTINUUM_HARNESS_DIRECT_ONCHAIN_REBASE_DEBOUNCE_MS || '750',
);
const HARNESS_QUEUE_SYNC_DEBOUNCE_MS = Number(
  process.env.CONTINUUM_HARNESS_QUEUE_SYNC_DEBOUNCE_MS || '250',
);
// Periodic fallback for onchainSync.queue when the WebSocket log subscription
// doesn't deliver QueueItemEnqueued events (e.g. Helius WS instability).
// Only reads the queue account — no getAllMangoAccounts() — so it's safe
// to run at 2 s. Set to 0 to disable.
const HARNESS_QUEUE_REFRESH_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_QUEUE_REFRESH_INTERVAL_MS || '2000',
);
const HARNESS_FRONTEND_REFRESH_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_FRONTEND_REFRESH_INTERVAL_MS ||
    `${Math.max(HARNESS_ONCHAIN_CACHE_TTL_MS, 10000)}`,
);
const HARNESS_READ_FAILURE_WINDOW_MS = Math.max(
  1000,
  Number(process.env.CONTINUUM_HARNESS_READ_FAILURE_WINDOW_MS || '30000'),
);
const HARNESS_READ_FAILURE_MIN_SAMPLES = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_READ_FAILURE_MIN_SAMPLES || '2'),
);
const HARNESS_READ_FAILURE_RATE_THRESHOLD = Math.min(
  1,
  Math.max(
    0,
    Number(process.env.CONTINUUM_HARNESS_READ_FAILURE_RATE_THRESHOLD || '0.15'),
  ),
);
const HARNESS_READ_SLOW_THRESHOLD_MS = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_READ_SLOW_THRESHOLD_MS || '200'),
);
const HARNESS_READ_SLOW_MIN_SAMPLES = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_READ_SLOW_MIN_SAMPLES || '3'),
);
const HARNESS_READ_SLOW_RATE_THRESHOLD = Math.min(
  1,
  Math.max(
    0,
    Number(process.env.CONTINUUM_HARNESS_READ_SLOW_RATE_THRESHOLD || '0.2'),
  ),
);
const HARNESS_READ_IMMEDIATE_FAILOVER_ON_ERROR =
  (process.env.CONTINUUM_HARNESS_READ_IMMEDIATE_FAILOVER_ON_ERROR || 'true') ===
  'true';
const HARNESS_READ_IMMEDIATE_SLOW_THRESHOLD_MS = Math.max(
  HARNESS_READ_SLOW_THRESHOLD_MS,
  Number(process.env.CONTINUUM_HARNESS_READ_IMMEDIATE_SLOW_THRESHOLD_MS || '750'),
);
const HARNESS_PRIMARY_WS_ERROR_WINDOW_MS = Math.max(
  1000,
  Number(process.env.CONTINUUM_HARNESS_PRIMARY_WS_ERROR_WINDOW_MS || '60000'),
);
const HARNESS_PRIMARY_WS_ERROR_THRESHOLD = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_PRIMARY_WS_ERROR_THRESHOLD || '1'),
);
const HARNESS_WS_ERROR_BODY_MAX_BYTES = Math.max(
  256,
  Number(process.env.CONTINUUM_HARNESS_WS_ERROR_BODY_MAX_BYTES || '4096'),
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
const HARNESS_SEQUENCER_GRPC_ADDR =
  process.env.CONTINUUM_HARNESS_SEQUENCER_GRPC_ADDR || '';
const HARNESS_SEQUENCER_ACCEPTED_STREAM_URL =
  process.env.CONTINUUM_HARNESS_SEQUENCER_ACCEPTED_STREAM_URL || '';
const HARNESS_SEQUENCER_TICK_STREAM_URL =
  process.env.CONTINUUM_HARNESS_SEQUENCER_TICK_STREAM_URL || '';
const HARNESS_SEQUENCER_RUNTIME_INFO_PATH =
  process.env.CONTINUUM_HARNESS_SEQUENCER_RUNTIME_INFO_PATH ||
  '/tmp/continuum-sequencer-runtime.json';
const HARNESS_SEQUENCER_PREFER_INTERNAL_HTTP =
  (process.env.CONTINUUM_HARNESS_SEQUENCER_PREFER_INTERNAL_HTTP || 'true') ===
  'true';
const HARNESS_SEQUENCER_GRPC_RECONNECT_MS = Number(
  process.env.CONTINUUM_HARNESS_SEQUENCER_GRPC_RECONNECT_MS || '1000',
);
const HARNESS_SEQUENCER_TICK_BATCH_LIMIT = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_SEQUENCER_TICK_BATCH_LIMIT || '8'),
);
const HARNESS_SEQUENCER_TICK_BATCH_BUDGET_MS = Math.max(
  1,
  Number(
    process.env.CONTINUUM_HARNESS_SEQUENCER_TICK_BATCH_BUDGET_MS || '2',
  ),
);
const HARNESS_LANE_MAX_OPTIMISTIC_PENDING = Math.max(
  0,
  Number(process.env.CONTINUUM_HARNESS_LANE_MAX_OPTIMISTIC_PENDING || '1'),
);
const HARNESS_LANE_PENDING_SOFT_LIMIT = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_LANE_PENDING_SOFT_LIMIT || '2'),
);
const HARNESS_LANE_FAILURE_THRESHOLD = Math.max(
  1,
  Number(process.env.CONTINUUM_HARNESS_LANE_FAILURE_THRESHOLD || '2'),
);
const HARNESS_LANE_SUPPRESSION_MS = Math.max(
  250,
  Number(process.env.CONTINUUM_HARNESS_LANE_SUPPRESSION_MS || '5000'),
);
const HARNESS_LANE_PENDING_STALE_MS = Math.max(
  100,
  Number(process.env.CONTINUUM_HARNESS_LANE_PENDING_STALE_MS || '15000'),
);
const SEQUENCER_TICK_PROCESSED_SIGNATURE_PREFIX = 'sequencer-tick:';
const LOCAL_QUEUE_PROCESS_EXECUTED = 2;

// ── Market stats collection ──────────────────────────────────────────────────
const HARNESS_MARKET_STATS_INTERVAL_MS = Number(
  process.env.CONTINUUM_HARNESS_MARKET_STATS_INTERVAL_MS ||
    (HARNESS_USE_LIGHTWEIGHT_WRAPPER_DEFAULTS ? '0' : '120000'),
);
const HARNESS_MARKET_STATS_PATH =
  process.env.CONTINUUM_HARNESS_MARKET_STATS_PATH ||
  `${path.dirname(path.resolve(HARNESS_EVENT_LOG_PATH))}/market_stats_snapshot.json`;
// Number of funding-rate history entries to keep per market (720 × 120 s = 24 h).
const HARNESS_FUNDING_HISTORY_MAX_ENTRIES = Number(
  process.env.CONTINUUM_HARNESS_FUNDING_HISTORY_MAX_ENTRIES || '720',
);

let engine!: ContinuumHarnessBackend;
let activeSequencerAcceptedTransport: 'none' | 'grpc' | 'http' = 'none';
let activeSequencerAcceptedStreamUrl: string | null = null;
let activeSequencerTickTransport: 'none' | 'grpc' | 'http' = 'none';
let activeSequencerTickStreamUrl: string | null = null;
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

type ReadProviderKey = 'primary' | 'fallback';

type OnchainReadOutcome = {
  ts_ms: number;
  ok: boolean;
  latency_ms: number;
  slow: boolean;
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
  primaryConnection: Connection;
  primaryRpcUrl: string;
  primaryWsUrl: string | null;
  fallbackConnection: Connection | null;
  fallbackRpcUrl: string | null;
  fallbackWsUrl: string | null;
  activeReadProvider: ReadProviderKey;
  primaryReadOutcomes: OnchainReadOutcome[];
  primaryWsUnexpectedResponses: number[];
  lastReadProviderSwitchTsMs: number | null;
  lastReadProviderSwitchReason: string | null;
  lastReadError: string | null;
  lastReadErrorTsMs: number | null;
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
  synthetic_reason?: string | null;
  suppressed_lane_count?: number;
  suppressed_pending_count?: number;
  suppressed_pending_sequences?: string[];
  suppressed_until_ts_ms?: number | null;
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

type ContinuumSequencerIntentMetadataWire = {
  group?: string;
  execution_queue?: string;
  market?: string;
  kind?: string | number;
  remaining_accounts?: Array<{
    pubkey?: string;
    is_signer?: boolean;
    is_writable?: boolean;
  }>;
  min_execute_slot?: string | number;
  expires_at_slot?: string | number;
  user_owner?: string;
  mango_account?: string;
} | null;

type ContinuumSequencerTransactionWire = {
  tx_id?: string;
  payload?: Buffer | Uint8Array | string;
  intent_metadata?: ContinuumSequencerIntentMetadataWire;
} | null;

type ContinuumSequencerAcceptedTransactionWire = {
  transaction?: {
    tx_id?: string;
    payload?: Buffer | Uint8Array | string;
    intent_metadata?: ContinuumSequencerIntentMetadataWire;
  } | null;
  sequence_number?: string | number;
  tx_hash?: string;
  ingestion_timestamp?: string | number;
};

type ContinuumSequencerOrderedTransactionWire = {
  transaction?: ContinuumSequencerTransactionWire;
  sequence_number?: string | number;
  tx_hash?: string;
  ingestion_timestamp?: string | number;
};

type ContinuumSequencerTickWire = {
  tick_number?: string | number;
  transactions?: ContinuumSequencerOrderedTransactionWire[];
  timestamp?: string | number;
};

type ContinuumSequencerRuntimeInfo = {
  endpoints?: {
    internal_grpc?: string;
    external_grpc?: string;
    internal_api?: string;
    external_api?: string;
  } | null;
} | null;

type ContinuumSequencerClient = grpc.Client & {
  streamAcceptedTransactions(
    request: Record<string, never>,
  ): grpc.ClientReadableStream<ContinuumSequencerAcceptedTransactionWire>;
  streamTicks(
    request: { start_tick: number; transactions_only?: boolean },
  ): grpc.ClientReadableStream<ContinuumSequencerTickWire>;
};

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

// ── Per-market stats (persisted to HARNESS_MARKET_STATS_PATH) ────────────────
type FundingRateEntry = {
  ts_ms: number;
  hourly_pct: number | null;
  daily_pct: number | null;
  long_funding: string;
  short_funding: string;
  oracle_price_ui: number | null;
};

type PerMarketStats = {
  /** Numeric market-index key (matches EngineSnapshot market key). */
  market: string;
  market_name: string;
  last_collected_ms: number;
  /** Running total accumulated since tracking began (survives restarts). */
  cumulative_volume_base_lots: string;
  cumulative_volume_quote_lots: string;
  /**
   * Highest taker_sequence counted into the cumulative volume totals.
   * Persisted so restarts don't re-count already-counted fills.
   * Using sequence as the high-water mark avoids stalls when the
   * 5000-trade in-memory ring buffer rotates old fills out.
   */
  cumulative_volume_seq_hwm: string;
  /** Latest on-chain values; null when on-chain read is unavailable. */
  fees_accrued_native: string | null;
  fees_settled_native: string | null;
  open_interest_base_lots: string | null;
  open_interest_base_ui: number | null;
  /** Ring-buffer capped at HARNESS_FUNDING_HISTORY_MAX_ENTRIES. */
  funding_rate_history: FundingRateEntry[];
  /** De-duplicated set of owner pubkeys that ever had a position here. */
  all_time_unique_users: string[];
};

type MarketStatsStore = {
  version: 1;
  written_ts_ms: number;
  markets: Record<string, PerMarketStats>;
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

type FrontendPreconfirmIntent =
  | {
      action: 'place_order';
      side: 'bid' | 'ask';
      price_lots: string;
      max_base_lots: string;
      max_quote_lots: string;
      client_order_id: string;
      order_type: number;
      self_trade_behavior: number;
      reduce_only: boolean;
      expiry_timestamp: string;
      limit: number;
      reserve_estimate: {
        base_lots: string | null;
        quote_lots: string | null;
      };
    }
  | {
      action: 'cancel_order';
      order_id: string;
    }
  | {
      action: 'cancel_order_by_client_order_id';
      client_order_id: string;
    }
  | {
      action: 'cancel_all_orders';
      limit: number;
    }
  | {
      action: 'cancel_all_orders_by_side';
      side: 'bid' | 'ask' | 'all';
      limit: number;
    }
  | {
      action: 'liquidity_deposit';
      amount: string;
      reduce_only: boolean;
    }
  | {
      action: 'liquidity_withdraw';
      amount: string;
      allow_borrow: boolean;
    }
  | {
      action: 'unknown';
      payload_bytes: number;
    };

type FrontendPreconfirmEvent = {
  phase: 'pre_confirmed';
  source: 'sequencer_ack';
  ts_ms: number;
  view: 'optimistic';
  accepted_source: string | null;
  fast_lane: boolean;
  sequencer_ingest_ts_ms: number | null;
  harness_accept_received_ts_ms: number | null;
  harness_preconfirm_emit_ts_ms: number | null;
  tracking_key: string;
  request_id: string | null;
  group: string;
  execution_queue: string;
  market: string;
  sequence: string;
  kind: number;
  owner: string;
  mango_account: string;
  enqueue_tx_signature: string;
  min_execute_slot: string;
  expires_at_slot: string;
  intent: FrontendPreconfirmIntent;
};

type FrontendValidatedLocalEvent = {
  phase: 'validated_local';
  source: 'sequencer_tick';
  ts_ms: number;
  harness_tick_received_ts_ms: number | null;
  harness_validated_local_emit_ts_ms: number | null;
  view: 'confirmed';
  tracking_key: string;
  request_id: string | null;
  group: string;
  execution_queue: string;
  market: string;
  sequence: string;
  kind: number;
  owner: string;
  mango_account: string;
  queue_process_status: number;
  queue_process_status_name: string;
  validation_status: 'executed' | 'failed';
  validation_error: string | null;
  tx_signature: string;
  processed_slot: string;
  owner_state: UserState | null;
  market_state: MarketState | null;
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
const FAST_PRECONFIRM_KEY_LIMIT = 16_384;
const tradeStreamSubscribers = new Map<string, TradeStreamSubscriber>();
const frontendStreamSubscribers = new Map<string, FrontendStreamSubscriber>();
const tradeStreamCursors = new Map<string, string | null>();
const fastPreconfirmKeys = new Set<string>();
const fastPreconfirmKeyOrder: string[] = [];
const deferredAcceptedIntentIngestQueue: RelayIntentAcceptedEvent[] = [];
let deferredAcceptedIntentIngestScheduled = false;
const marketRuntimeMetricsCache = new Map<
  string,
  { fetchedAtMs: number; data: MarketRuntimeMetrics | null }
>();
// [redis-phase5] Module-level mirror of onchain.cachedMarketMetadata so the
// state-mirror snapshot provider can attach rich identity fields without a
// dependency on a specific OnchainContext reference or on any HTTP call.
let harnessMarketMetadataSnapshot: Record<string, HarnessMarketMetadata> = {};
// ── Market stats state ───────────────────────────────────────────────────────
let marketStatsStore: MarketStatsStore = {
  version: 1,
  written_ts_ms: 0,
  markets: {},
};
// (Volume checkpointing migrated to per-market cumulative_volume_seq_hwm in
// PerMarketStats — persisted in the stats file for restart safety.)
const bufferedLogWriters = new Map<
  string,
  {
    lines: string[];
    maxBytes: number;
    flushing: boolean;
  }
>();
let nextStreamSubscriberSeq = 1;
let directOnchainRebaseTimer: NodeJS.Timeout | null = null;
let directOnchainRebaseInFlight: Promise<void> | null = null;
let directOnchainRebasePending = false;
let queueSyncTimer: NodeJS.Timeout | null = null;
let queueSyncInFlight: Promise<void> | null = null;
let queueSyncPending = false;
const laneGuard = new ContinuumHarnessLaneGuard({
  maxOptimisticPendingPerLane: HARNESS_LANE_MAX_OPTIMISTIC_PENDING,
  marketPendingSoftLimit: HARNESS_LANE_PENDING_SOFT_LIMIT,
  failureThreshold: HARNESS_LANE_FAILURE_THRESHOLD,
  suppressionMs: HARNESS_LANE_SUPPRESSION_MS,
  pendingStaleMs: HARNESS_LANE_PENDING_STALE_MS,
});

/**
 * v2 sub-queue: parse a `market` field (which can be a stringified u16, an
 * already-numeric value, or a non-numeric label like "liquidity"/"unknown") into
 * an optional u16. Returning `null` here means "no market hint available" and
 * keeps `engine.findIntent` on its slow scan fallback path; returning a number
 * keeps it on the O(1) direct lookup.
 */
function parseMarketIndexHint(
  market: string | number | null | undefined,
): number | null {
  if (market === null || market === undefined) return null;
  if (typeof market === 'number') {
    return Number.isInteger(market) && market >= 0 && market < 65536
      ? market
      : null;
  }
  const trimmed = market.trim();
  if (trimmed === '' || trimmed === 'unknown' || trimmed === 'liquidity') {
    return null;
  }
  const parsed = Number(trimmed);
  return Number.isInteger(parsed) && parsed >= 0 && parsed < 65536
    ? parsed
    : null;
}

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

function sanitizeStringForLogs(value: string, limit = 1_000): string {
  return truncateForLog(
    value.replace(
      /((?:api[-_]?key|token|authorization|auth|signature)=)[^&\s]+/gi,
      '$1<redacted>',
    ),
    limit,
  );
}

function sanitizeUrlForLogs(value?: string | null): string {
  if (!value) {
    return '';
  }
  try {
    const parsed = new URL(value);
    for (const key of parsed.searchParams.keys()) {
      if (/(?:api[-_]?key|token|authorization|auth|signature)/i.test(key)) {
        parsed.searchParams.set(key, '<redacted>');
      }
    }
    return truncateForLog(parsed.toString(), 1_000);
  } catch {
    return sanitizeStringForLogs(value, 1_000);
  }
}

function sanitizeHeadersForLogs(headers: Record<string, unknown> | undefined): string {
  if (!headers) {
    return '{}';
  }
  const sanitized = Object.fromEntries(
    Object.entries(headers).map(([key, rawValue]) => {
      const value = Array.isArray(rawValue)
        ? rawValue.join(', ')
        : rawValue == null
          ? ''
          : String(rawValue);
      return [
        key,
        /(?:api[-_]?key|token|authorization|auth|signature|cookie)/i.test(key)
          ? '<redacted>'
          : sanitizeStringForLogs(value, 1_000),
      ];
    }),
  );
  return truncateForLog(JSON.stringify(sanitized), 2_000);
}

function inferRpcProviderLabel(url: string, fallback: string): string {
  const lower = url.toLowerCase();
  if (lower.includes('helius')) {
    return 'helius';
  }
  if (lower.includes('rpcpool') || lower.includes('triton')) {
    return 'triton';
  }
  if (lower.includes('quicknode')) {
    return 'quicknode';
  }
  return fallback;
}

function createReadConnection(rpcUrl: string, wsUrl: string | null): Connection {
  return new Connection(rpcUrl, {
    commitment:
      HARNESS_COMMITMENT === 'processed' ? 'confirmed' : HARNESS_COMMITMENT,
    wsEndpoint: wsUrl || undefined,
  });
}

function getReadProviderLabel(
  onchain: OnchainContext,
  provider: ReadProviderKey = onchain.activeReadProvider,
): string {
  if (provider === 'primary') {
    return inferRpcProviderLabel(onchain.primaryRpcUrl, 'primary');
  }
  return inferRpcProviderLabel(onchain.fallbackRpcUrl || '', 'fallback');
}

function getReadProviderUrls(
  onchain: OnchainContext,
  provider: ReadProviderKey = onchain.activeReadProvider,
): { rpcUrl: string; wsUrl: string | null } {
  if (provider === 'primary') {
    return {
      rpcUrl: onchain.primaryRpcUrl,
      wsUrl: onchain.primaryWsUrl,
    };
  }
  return {
    rpcUrl: onchain.fallbackRpcUrl || '',
    wsUrl: onchain.fallbackWsUrl,
  };
}

function prunePrimaryReadOutcomes(onchain: OnchainContext, now = Date.now()): void {
  const minTs = now - HARNESS_READ_FAILURE_WINDOW_MS;
  while (
    onchain.primaryReadOutcomes.length > 0 &&
    onchain.primaryReadOutcomes[0]!.ts_ms < minTs
  ) {
    onchain.primaryReadOutcomes.shift();
  }
}

function getPrimaryReadFailureStats(onchain: OnchainContext): {
  samples: number;
  failures: number;
  failureRate: number;
} {
  prunePrimaryReadOutcomes(onchain);
  const samples = onchain.primaryReadOutcomes.length;
  const failures = onchain.primaryReadOutcomes.filter((item) => !item.ok).length;
  return {
    samples,
    failures,
    failureRate: samples > 0 ? failures / samples : 0,
  };
}

function getPrimaryReadSlowStats(onchain: OnchainContext): {
  samples: number;
  slowSamples: number;
  slowRate: number;
} {
  prunePrimaryReadOutcomes(onchain);
  const samples = onchain.primaryReadOutcomes.length;
  const slowSamples = onchain.primaryReadOutcomes.filter((item) => item.slow).length;
  return {
    samples,
    slowSamples,
    slowRate: samples > 0 ? slowSamples / samples : 0,
  };
}

function prunePrimaryWsUnexpectedResponses(
  onchain: OnchainContext,
  now = Date.now(),
): void {
  const minTs = now - HARNESS_PRIMARY_WS_ERROR_WINDOW_MS;
  while (
    onchain.primaryWsUnexpectedResponses.length > 0 &&
    onchain.primaryWsUnexpectedResponses[0]! < minTs
  ) {
    onchain.primaryWsUnexpectedResponses.shift();
  }
}

function getPrimaryWsUnexpectedResponseStats(onchain: OnchainContext): {
  count: number;
} {
  prunePrimaryWsUnexpectedResponses(onchain);
  return {
    count: onchain.primaryWsUnexpectedResponses.length,
  };
}

function notePrimaryWsUnexpectedResponse(
  onchain: OnchainContext,
  statusCode: number | null,
  statusMessage: string,
): void {
  onchain.primaryWsUnexpectedResponses.push(Date.now());
  prunePrimaryWsUnexpectedResponses(onchain);
  maybeActivateFallbackProvider(
    onchain,
    `websocket unexpected response ${statusCode ?? 'unknown'} ${statusMessage}`.trim(),
  );
}

function switchReadProviderToFallback(
  onchain: OnchainContext,
  reason: string,
): boolean {
  if (!onchain.fallbackConnection || onchain.activeReadProvider === 'fallback') {
    return false;
  }
  onchain.activeReadProvider = 'fallback';
  onchain.connection = onchain.fallbackConnection;
  onchain.mangoClient = null;
  onchain.cachedGroup = null;
  onchain.cachedGroupFetchedAtMs = 0;
  onchain.cachedMarketMetadata = null;
  onchain.cachedMarketMetadataFetchedAtMs = 0;
  onchain.lastReadProviderSwitchTsMs = Date.now();
  onchain.lastReadProviderSwitchReason = reason;
  console.warn(
    `[harness:onchain_read_failover] switching reads to ${getReadProviderLabel(
      onchain,
    )}: ${reason}`,
  );
  return true;
}

function maybeActivateFallbackProvider(
  onchain: OnchainContext,
  operation: string,
): boolean {
  if (!onchain.fallbackConnection || onchain.activeReadProvider === 'fallback') {
    return false;
  }
  const wsStats = getPrimaryWsUnexpectedResponseStats(onchain);
  if (wsStats.count >= HARNESS_PRIMARY_WS_ERROR_THRESHOLD) {
    return switchReadProviderToFallback(
      onchain,
      `primary websocket unexpected responses ${wsStats.count} ` +
        `within ${HARNESS_PRIMARY_WS_ERROR_WINDOW_MS}ms during ${operation}`,
    );
  }
  const stats = getPrimaryReadFailureStats(onchain);
  if (
    stats.samples >= HARNESS_READ_FAILURE_MIN_SAMPLES &&
    stats.failureRate >= HARNESS_READ_FAILURE_RATE_THRESHOLD
  ) {
    return switchReadProviderToFallback(
      onchain,
      `primary read failure rate ${stats.failureRate.toFixed(2)} ` +
        `(${stats.failures}/${stats.samples}) during ${operation}`,
    );
  }
  const slowStats = getPrimaryReadSlowStats(onchain);
  if (
    slowStats.samples >= HARNESS_READ_SLOW_MIN_SAMPLES &&
    slowStats.slowRate >= HARNESS_READ_SLOW_RATE_THRESHOLD
  ) {
    return switchReadProviderToFallback(
      onchain,
      `primary read slow rate ${slowStats.slowRate.toFixed(2)} ` +
        `(${slowStats.slowSamples}/${slowStats.samples}) over ` +
        `${HARNESS_READ_SLOW_THRESHOLD_MS}ms during ${operation}`,
    );
  }
  return false;
}

async function withOnchainRead<T>(
  onchain: OnchainContext | null,
  operation: string,
  read: () => Promise<T>,
): Promise<T> {
  if (!onchain) {
    return await read();
  }
  const startedAt = performance.now();

  try {
    const result = await read();
    if (onchain.activeReadProvider === 'primary') {
      const latencyMs = performance.now() - startedAt;
      onchain.primaryReadOutcomes.push({
        ts_ms: Date.now(),
        ok: true,
        latency_ms: latencyMs,
        slow: latencyMs >= HARNESS_READ_SLOW_THRESHOLD_MS,
      });
      prunePrimaryReadOutcomes(onchain);
      if (
        latencyMs >= HARNESS_READ_IMMEDIATE_SLOW_THRESHOLD_MS &&
        switchReadProviderToFallback(
          onchain,
          `primary read latency ${latencyMs.toFixed(2)}ms exceeded ` +
            `${HARNESS_READ_IMMEDIATE_SLOW_THRESHOLD_MS}ms during ${operation}`,
        )
      ) {
        // Keep the successful result and route subsequent reads to fallback.
      } else {
        maybeActivateFallbackProvider(onchain, operation);
      }
    }
    return result;
  } catch (err) {
    const provider = onchain.activeReadProvider;
    const urls = getReadProviderUrls(onchain, provider);
    const normalized = normalizeError(err);
    const latencyMs = performance.now() - startedAt;
    onchain.lastReadError = normalized.message;
    onchain.lastReadErrorTsMs = Date.now();
    if (provider === 'primary') {
      onchain.primaryReadOutcomes.push({
        ts_ms: onchain.lastReadErrorTsMs,
        ok: false,
        latency_ms: latencyMs,
        slow: latencyMs >= HARNESS_READ_SLOW_THRESHOLD_MS,
      });
      prunePrimaryReadOutcomes(onchain);
    }
    recordRuntimeError('onchain_read_failure', err, {
      operation,
      provider: getReadProviderLabel(onchain, provider),
      rpc_url: sanitizeUrlForLogs(urls.rpcUrl),
      ws_url: sanitizeUrlForLogs(urls.wsUrl),
      active_provider: getReadProviderLabel(onchain),
      latency_ms: latencyMs.toFixed(3),
    });
    if (
      provider === 'primary' &&
      HARNESS_READ_IMMEDIATE_FAILOVER_ON_ERROR &&
      switchReadProviderToFallback(
        onchain,
        `primary read error during ${operation}: ${normalized.message}`,
      )
    ) {
      return await withOnchainRead(onchain, `${operation}:fallback_retry`, read);
    }
    if (provider === 'primary' && maybeActivateFallbackProvider(onchain, operation)) {
      return await withOnchainRead(onchain, `${operation}:fallback_retry`, read);
    }
    throw err;
  }
}

function getReadProviderDiagnostics(
  onchain: OnchainContext | null,
): Record<string, unknown> | null {
  if (!onchain) {
    return null;
  }
  const primaryFailureStats = getPrimaryReadFailureStats(onchain);
  const primarySlowStats = getPrimaryReadSlowStats(onchain);
  const primaryWsStats = getPrimaryWsUnexpectedResponseStats(onchain);
  return {
    active_provider: getReadProviderLabel(onchain),
    active_provider_key: onchain.activeReadProvider,
    primary_provider: getReadProviderLabel(onchain, 'primary'),
    primary_rpc_url: sanitizeUrlForLogs(onchain.primaryRpcUrl),
    primary_ws_url: sanitizeUrlForLogs(onchain.primaryWsUrl),
    fallback_provider: onchain.fallbackConnection
      ? getReadProviderLabel(onchain, 'fallback')
      : null,
    fallback_rpc_url: sanitizeUrlForLogs(onchain.fallbackRpcUrl),
    fallback_ws_url: sanitizeUrlForLogs(onchain.fallbackWsUrl),
    fallback_configured: !!onchain.fallbackConnection,
    primary_failure_window_ms: HARNESS_READ_FAILURE_WINDOW_MS,
    primary_failure_min_samples: HARNESS_READ_FAILURE_MIN_SAMPLES,
    primary_failure_rate_threshold: HARNESS_READ_FAILURE_RATE_THRESHOLD,
    primary_failure_stats: primaryFailureStats,
    primary_slow_threshold_ms: HARNESS_READ_SLOW_THRESHOLD_MS,
    primary_slow_min_samples: HARNESS_READ_SLOW_MIN_SAMPLES,
    primary_slow_rate_threshold: HARNESS_READ_SLOW_RATE_THRESHOLD,
    primary_slow_stats: primarySlowStats,
    primary_immediate_failover_on_error: HARNESS_READ_IMMEDIATE_FAILOVER_ON_ERROR,
    primary_immediate_slow_threshold_ms: HARNESS_READ_IMMEDIATE_SLOW_THRESHOLD_MS,
    primary_ws_error_window_ms: HARNESS_PRIMARY_WS_ERROR_WINDOW_MS,
    primary_ws_error_threshold: HARNESS_PRIMARY_WS_ERROR_THRESHOLD,
    primary_ws_error_stats: primaryWsStats,
    last_switch_ts_ms: onchain.lastReadProviderSwitchTsMs,
    last_switch_reason: onchain.lastReadProviderSwitchReason,
    last_error: onchain.lastReadError,
    last_error_ts_ms: onchain.lastReadErrorTsMs,
  };
}

function attachUnexpectedResponseDiagnostics(
  socket: {
    on?: (event: string, listener: (...args: any[]) => void) => void;
    __continuumUnexpectedResponseAttached?: boolean;
  },
  providerLabel: string,
  rpcUrl: string,
  wsUrl: string | null,
  onUnexpectedResponse?: (details: {
    statusCode: number | null;
    statusMessage: string;
    bodySnippet: string;
  }) => void,
): void {
  if (
    !socket ||
    typeof socket.on !== 'function' ||
    socket.__continuumUnexpectedResponseAttached
  ) {
    return;
  }
  socket.__continuumUnexpectedResponseAttached = true;
  socket.on('unexpected-response', (request: any, response: any) => {
    const chunks: Buffer[] = [];
    let emitted = false;
    const flush = (phase: string) => {
      if (emitted) {
        return;
      }
      emitted = true;
      const bodySnippet = sanitizeStringForLogs(
        Buffer.concat(chunks).toString('utf-8'),
        HARNESS_WS_ERROR_BODY_MAX_BYTES,
      );
      onUnexpectedResponse?.({
        statusCode:
          typeof response?.statusCode === 'number' ? response.statusCode : null,
        statusMessage: sanitizeStringForLogs(
          String(response?.statusMessage || ''),
          256,
        ),
        bodySnippet,
      });
      recordRuntimeError(
        'rpc_websocket_unexpected_response',
        new Error(
          `WebSocket unexpected response ${response?.statusCode || 'unknown'} ${
            response?.statusMessage || ''
          }`.trim(),
        ),
        {
          provider: providerLabel,
          phase,
          rpc_url: sanitizeUrlForLogs(rpcUrl),
          ws_url: sanitizeUrlForLogs(wsUrl),
          request_path: sanitizeStringForLogs(
            String(request?.path || request?.url || ''),
            1_000,
          ),
          status_code: response?.statusCode ?? '',
          status_message: sanitizeStringForLogs(
            String(response?.statusMessage || ''),
            256,
          ),
          headers: sanitizeHeadersForLogs(response?.headers),
          body_snippet: bodySnippet || '<empty>',
        },
      );
    };
    response?.on?.('data', (chunk: Buffer | string) => {
      if (Buffer.concat(chunks).length >= HARNESS_WS_ERROR_BODY_MAX_BYTES) {
        return;
      }
      const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk));
      const remaining = HARNESS_WS_ERROR_BODY_MAX_BYTES - Buffer.concat(chunks).length;
      chunks.push(buffer.subarray(0, Math.max(0, remaining)));
    });
    response?.on?.('end', () => flush('end'));
    response?.on?.('close', () => flush('close'));
    response?.on?.('aborted', () => flush('aborted'));
    setTimeout(() => flush('timeout'), 250).unref?.();
  });
}

function attachRpcWebSocketDiagnostics(
  connection: Connection,
  providerLabel: string,
  rpcUrl: string,
  wsUrl: string | null,
  onUnexpectedResponse?: (details: {
    statusCode: number | null;
    statusMessage: string;
    bodySnippet: string;
  }) => void,
): void {
  const rpcWebSocket = (connection as Connection & {
    _rpcWebSocket?: {
      on?: (event: string, listener: (...args: any[]) => void) => void;
      socket?: {
        on?: (event: string, listener: (...args: any[]) => void) => void;
        __continuumUnexpectedResponseAttached?: boolean;
      };
      webSocketFactory?: (...args: any[]) => any;
      __continuumDiagnosticsAttached?: boolean;
    };
  })._rpcWebSocket;
  if (
    !rpcWebSocket ||
    typeof rpcWebSocket.on !== 'function' ||
    rpcWebSocket.__continuumDiagnosticsAttached
  ) {
    return;
  }
  rpcWebSocket.__continuumDiagnosticsAttached = true;
  rpcWebSocket.on('error', (err: unknown) => {
    recordRuntimeError('rpc_websocket_error', err, {
      provider: providerLabel,
      rpc_url: sanitizeUrlForLogs(rpcUrl),
      ws_url: sanitizeUrlForLogs(wsUrl),
    });
  });
  rpcWebSocket.on('close', (code: number, reason: Buffer | string) => {
    console.warn(
      `[harness:rpc_websocket_close] provider=${providerLabel} code=${code} reason=${sanitizeStringForLogs(
        Buffer.isBuffer(reason) ? reason.toString('utf-8') : String(reason || ''),
        512,
      )}`,
    );
  });
  if (typeof rpcWebSocket.webSocketFactory === 'function') {
    const originalFactory = rpcWebSocket.webSocketFactory.bind(rpcWebSocket);
    rpcWebSocket.webSocketFactory = (...args: any[]) => {
      const socket = originalFactory(...args);
      attachUnexpectedResponseDiagnostics(
        socket,
        providerLabel,
        rpcUrl,
        wsUrl,
        onUnexpectedResponse,
      );
      return socket;
    };
  }
  if (rpcWebSocket.socket) {
    attachUnexpectedResponseDiagnostics(
      rpcWebSocket.socket,
      providerLabel,
      rpcUrl,
      wsUrl,
      onUnexpectedResponse,
    );
  }
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

async function appendLinesWithRotationAsync(
  filePath: string,
  lines: string[],
  maxBytes: number,
): Promise<void> {
  if (!lines.length) {
    return;
  }
  const resolved = path.resolve(filePath);
  ensureDirForFile(resolved);
  let currentSize = fileSizeCache.get(resolved);
  if (currentSize === undefined) {
    try {
      currentSize = (await fs.promises.stat(resolved)).size;
    } catch {
      currentSize = 0;
    }
  }

  const batch = lines.join('');
  const batchBytes = Buffer.byteLength(batch);
  if (maxBytes > 0 && currentSize + batchBytes > maxBytes) {
    const rotatedPath = `${resolved}.1`;
    try {
      await fs.promises.unlink(rotatedPath).catch((err: any) => {
        if (err?.code !== 'ENOENT') {
          throw err;
        }
      });
      await fs.promises.rename(resolved, rotatedPath).catch((err: any) => {
        if (err?.code !== 'ENOENT') {
          throw err;
        }
      });
      currentSize = 0;
    } catch (err) {
      const normalized = normalizeError(err);
      console.error(
        `[harness:log_rotate] failed to rotate ${resolved}: ${normalized.message}`,
      );
    }
  }

  await fs.promises.appendFile(resolved, batch);
  fileSizeCache.set(resolved, currentSize + batchBytes);
}

function queueBufferedLogLine(
  filePath: string,
  line: string,
  maxBytes: number,
  source: string,
): void {
  const resolved = path.resolve(filePath);
  const writer =
    bufferedLogWriters.get(resolved) || {
      lines: [],
      maxBytes,
      flushing: false,
    };
  writer.lines.push(line);
  writer.maxBytes = maxBytes;
  bufferedLogWriters.set(resolved, writer);
  if (writer.flushing) {
    return;
  }
  writer.flushing = true;
  setImmediate(() => {
    void flushBufferedLogWriter(resolved, source);
  });
}

async function flushBufferedLogWriter(
  filePath: string,
  source: string,
): Promise<void> {
  const writer = bufferedLogWriters.get(filePath);
  if (!writer) {
    return;
  }
  while (writer.lines.length) {
    const batch = writer.lines.splice(0, writer.lines.length);
    const startedAt = performance.now();
    try {
      await appendLinesWithRotationAsync(filePath, batch, writer.maxBytes);
      recordLatencySample('flush_buffered_log', performance.now() - startedAt, {
        thresholdMs: HARNESS_SLOW_COMPONENT_MS,
        context: {
          path: filePath,
          lines: batch.length,
        },
      });
    } catch (err) {
      recordRuntimeError(source, err, {
        path: filePath,
        lines: batch.length,
      });
    }
  }
  writer.flushing = false;
  if (writer.lines.length) {
    writer.flushing = true;
    setImmediate(() => {
      void flushBufferedLogWriter(filePath, source);
    });
    return;
  }
  bufferedLogWriters.delete(filePath);
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
    case 'getValidatedLocalPayload':
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
    case 'ingestRelayIntentStatus': {
      const event = args[0] as RelayIntentStatusEvent | undefined;
      return event
        ? {
            event_type: event.event_type,
            request_id: event.request_id,
            status_code: event.status_code,
            sequence: event.sequence || 'none',
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
  const groupPk = onchain.groupPk;
  try {
    return await withOnchainRead(onchain, 'get_fresh_group', async () => {
      if (!onchain.mangoClient) {
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
        onchain.cachedGroup = await onchain.mangoClient.getGroup(groupPk);
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
          `Onchain read context recovered: provider=${getReadProviderLabel(
            onchain,
          )}, group=${groupPk.toBase58()}, usdc_mint=${
            onchain.usdcMint?.toBase58() || 'unresolved'
          }`,
        );
      }

      if (
        onchain.cachedGroup &&
        Date.now() - onchain.cachedGroupFetchedAtMs < HARNESS_ONCHAIN_CACHE_TTL_MS
      ) {
        return onchain.cachedGroup;
      }

      const group = await onchain.mangoClient.getGroup(groupPk);
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
    });
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

async function getMarketMetadataMap(
  onchain: OnchainContext | null,
): Promise<Record<string, HarnessMarketMetadata>> {
  if (!onchain?.groupPk) {
    return {};
  }
  return await withOnchainRead(onchain, 'get_market_metadata_map', async () => {
    if (
      onchain.cachedMarketMetadata &&
      Date.now() - onchain.cachedMarketMetadataFetchedAtMs <
        HARNESS_MARKET_METADATA_CACHE_TTL_MS
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
    // [redis-phase5] keep module-level mirror in sync so the state-mirror
    // snapshot provider reads the freshest metadata without an onchain ref.
    harnessMarketMetadataSnapshot = metadata;
    onchain.cachedMarketMetadataFetchedAtMs = Date.now();
    return metadata;
  });
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

// ---------------------------------------------------------------------------
// v5 per-market queue helpers
//
// The harness was originally written against the v2/v3 "single shared queue"
// topology and only tracks one PDA via CONTINUUM_HARNESS_EXECUTION_QUEUE_PK.
// After the v5 migration, queues are per-market PDAs derived with
//   seeds = ["execution-queue-v5", group, market_index u16 LE]
// The helpers below let the harness do a fresh RPC read of the per-market
// queue account on request, bypassing the legacy single-queue replay model
// so /state/queue/{m} and /state/book/{m} can serve post-upgrade truth.
//
// Byte layout of ExecutionQueueV5 (see programs/mango-v4/src/state/execution_queue_v5.rs):
//   [0..8)           anchor discriminator
//   [8..152)         ExecutionQueueV5Header  (group, authority_state, bump, pauses, layout_version, max_retries, _padding, total_count, reserved[64])
//   [152..216)       SubQueueHeaderV5[0]     (N_MAX_MARKETS=1)
//     +0  market_index: u16
//     +2  active: u8
//     +3  paused_ingress: u8
//     +4  paused_execute: u8
//     +5  shard_id: u8
//     +8  live_count: u32
//     +12 gap_wait_slots: u16
//     +14 soft_limit: u16
//     +16 next_sequence_to_execute: u64
//     +24 max_seen_sequence: u64
//   [216..)          items array (per-market ring)
// ---------------------------------------------------------------------------
const V5_QUEUE_SEED = Buffer.from('execution-queue-v5');

function deriveV5QueuePda(
  programId: PublicKey,
  group: PublicKey,
  marketIndex: number,
): PublicKey {
  const leBytes = Buffer.alloc(2);
  leBytes.writeUInt16LE(marketIndex, 0);
  const [pda] = PublicKey.findProgramAddressSync(
    [V5_QUEUE_SEED, group.toBuffer(), leBytes],
    programId,
  );
  return pda;
}

interface V5QueueLiveState {
  pda: string;
  layout_version: number;
  paused_ingress: boolean;
  paused_execute: boolean;
  total_count: string;
  sub_queue: {
    market_index: number;
    active: boolean;
    live_count: number;
    gap_wait_slots: number;
    soft_limit: number;
    next_sequence_to_execute: string;
    max_seen_sequence: string;
    paused_ingress: boolean;
    paused_execute: boolean;
    head_ring_offset: number;
  } | null;
  observed_slot: number;
  generated_ts_ms: number;
}

async function fetchV5QueueLiveState(
  connection: Connection,
  programId: PublicKey,
  group: PublicKey,
  marketIndex: number,
  commitment: Commitment,
): Promise<V5QueueLiveState | null> {
  const pda = deriveV5QueuePda(programId, group, marketIndex);
  const resp = await connection.getAccountInfoAndContext(pda, commitment);
  if (!resp.value?.data) return null;
  const data = Buffer.from(resp.value.data);
  if (data.length < 216) return null;
  // Header fields we surface.
  const layoutVersion = data.readUInt8(8 + 32 + 32 + 1 + 1 + 1); // layout_version
  const headerPausedIngress = data.readUInt8(8 + 32 + 32 + 1) === 1;
  const headerPausedExecute = data.readUInt8(8 + 32 + 32 + 1 + 1) === 1;
  const totalCount = data.readBigUInt64LE(8 + 32 + 32 + 1 + 1 + 1 + 1 + 1 + 3); // after 3-byte padding
  // SubQueueHeader[0] at offset 152.
  const sqBase = 152;
  const marketIdx = data.readUInt16LE(sqBase + 0);
  const active = data.readUInt8(sqBase + 2) === 1;
  const pausedIng = data.readUInt8(sqBase + 3) === 1;
  const pausedExe = data.readUInt8(sqBase + 4) === 1;
  const liveCount = data.readUInt32LE(sqBase + 8);
  const gapWaitSlots = data.readUInt16LE(sqBase + 12);
  const softLimit = data.readUInt16LE(sqBase + 14);
  const nextSeqToExec = data.readBigUInt64LE(sqBase + 16);
  const maxSeenSeq = data.readBigUInt64LE(sqBase + 24);
  const V5_PER_MARKET_CAPACITY = 1024n;
  const headRingOffset = Number(nextSeqToExec % V5_PER_MARKET_CAPACITY);
  return {
    pda: pda.toBase58(),
    layout_version: layoutVersion,
    paused_ingress: headerPausedIngress,
    paused_execute: headerPausedExecute,
    total_count: totalCount.toString(),
    sub_queue: active
      ? {
          market_index: marketIdx,
          active,
          live_count: liveCount,
          gap_wait_slots: gapWaitSlots,
          soft_limit: softLimit,
          next_sequence_to_execute: nextSeqToExec.toString(),
          max_seen_sequence: maxSeenSeq.toString(),
          paused_ingress: pausedIng,
          paused_execute: pausedExe,
          head_ring_offset: headRingOffset,
        }
      : null,
    observed_slot: resp.context.slot,
    generated_ts_ms: Date.now(),
  };
}

interface FreshBookLevel {
  price_lots: string;
  price_ui: string;
  base_lots: string;
  orders: number;
}

interface FreshBookSnapshot {
  market_index: number;
  observed_slot: number;
  generated_ts_ms: number;
  oracle_price_ui: string | null;
  bids: FreshBookLevel[];
  asks: FreshBookLevel[];
  best_bid: FreshBookLevel | null;
  best_ask: FreshBookLevel | null;
  mid_ui: string | null;
  spread_ui: string | null;
}

async function fetchFreshBook(
  onchain: OnchainContext,
  marketIndex: number,
  depth: number,
): Promise<FreshBookSnapshot | null> {
  const group = await getFreshGroup(onchain);
  if (!group) return null;
  const perpMarket = group.perpMarketsMapByMarketIndex.get(
    marketIndex as unknown as import('../../src/accounts/perp').PerpMarketIndex,
  );
  if (!perpMarket) return null;
  const [bidsBook, asksBook] = await Promise.all([
    perpMarket.loadBids(onchain.mangoClient, true),
    perpMarket.loadAsks(onchain.mangoClient, true),
  ]);
  const foldLevels = (book: typeof bidsBook): FreshBookLevel[] => {
    const levels = new Map<string, { baseLots: bigint; orders: number }>();
    for (const o of book.itemsValid()) {
      const priceKey = o.priceLots.toString();
      const cur = levels.get(priceKey) || { baseLots: 0n, orders: 0 };
      cur.baseLots += BigInt(o.sizeLots.toString());
      cur.orders += 1;
      levels.set(priceKey, cur);
    }
    const arr: FreshBookLevel[] = [];
    for (const [priceKey, v] of levels.entries()) {
      const priceUi = perpMarket.priceLotsToUi(new BN(priceKey));
      arr.push({
        price_lots: priceKey,
        price_ui: priceUi.toString(),
        base_lots: v.baseLots.toString(),
        orders: v.orders,
      });
    }
    return arr;
  };
  const bidsRaw = foldLevels(bidsBook).sort(
    (a, b) => Number(BigInt(b.price_lots) - BigInt(a.price_lots)),
  );
  const asksRaw = foldLevels(asksBook).sort(
    (a, b) => Number(BigInt(a.price_lots) - BigInt(b.price_lots)),
  );
  const bids = bidsRaw.slice(0, depth);
  const asks = asksRaw.slice(0, depth);
  const bestBid = bids[0] || null;
  const bestAsk = asks[0] || null;
  const midUi =
    bestBid && bestAsk
      ? ((parseFloat(bestBid.price_ui) + parseFloat(bestAsk.price_ui)) / 2).toString()
      : null;
  const spreadUi =
    bestBid && bestAsk
      ? (parseFloat(bestAsk.price_ui) - parseFloat(bestBid.price_ui)).toString()
      : null;
  return {
    market_index: marketIndex,
    observed_slot: 0, // loadBids/Asks don't return a context; slot left 0 — clients can still ts-check.
    generated_ts_ms: Date.now(),
    oracle_price_ui: perpMarket.uiPrice?.toString?.() ?? null,
    bids,
    asks,
    best_bid: bestBid,
    best_ask: bestAsk,
    mid_ui: midUi,
    spread_ui: spreadUi,
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
  const bestBidUi = priceLotsToUi(
    marketState?.bids?.[0]?.price_lots || null,
    metadata,
  );
  const bestAskUi = priceLotsToUi(
    marketState?.asks?.[0]?.price_lots || null,
    metadata,
  );
  const withDynamicOrderbookMetrics = (
    metrics: MarketRuntimeMetrics | null,
  ): MarketRuntimeMetrics | null => {
    if (!metrics) {
      return null;
    }
    return {
      ...metrics,
      best_bid_ui: bestBidUi,
      best_ask_ui: bestAskUi,
      mark_price_ui:
        bestBidUi !== null && bestAskUi !== null
          ? (bestBidUi + bestAskUi) / 2
          : metrics.oracle_price_ui,
      updated_ts_ms: Date.now(),
    };
  };
  const cached = marketRuntimeMetricsCache.get(market);
  if (
    cached &&
    Date.now() - cached.fetchedAtMs < HARNESS_ONCHAIN_CACHE_TTL_MS
  ) {
    return withDynamicOrderbookMetrics(cached.data);
  }

  const group = await getFreshGroup(onchain);
  if (!group) {
    marketRuntimeMetricsCache.set(market, {
      fetchedAtMs: Date.now(),
      data: cached?.data || null,
    });
    return withDynamicOrderbookMetrics(cached?.data || null);
  }

  const marketIndex = Number(market);
  if (!Number.isInteger(marketIndex)) {
    marketRuntimeMetricsCache.set(market, {
      fetchedAtMs: Date.now(),
      data: cached?.data || null,
    });
    return withDynamicOrderbookMetrics(cached?.data || null);
  }

  const perpMarket = group.perpMarketsMapByMarketIndex.get(
    marketIndex as never,
  );
  if (!perpMarket) {
    marketRuntimeMetricsCache.set(market, {
      fetchedAtMs: Date.now(),
      data: cached?.data || null,
    });
    return withDynamicOrderbookMetrics(cached?.data || null);
  }

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
  return withDynamicOrderbookMetrics(data);
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

function reconciliationMarketHasDrift(
  market: ReconciliationMarketDrift | undefined,
): boolean {
  if (!market) {
    return false;
  }
  return (
    market.replay_open_orders !== market.onchain_open_orders ||
    market.replay_best_bid !== market.onchain_best_bid ||
    market.replay_best_ask !== market.onchain_best_ask ||
    market.bid_base_lots_abs_diff !== '0' ||
    market.ask_base_lots_abs_diff !== '0'
  );
}

function suppressedMarketToDrift(
  market: LaneGuardMarketSuppression,
  current?: ReconciliationMarketDrift,
): ReconciliationMarketDrift {
  const onchainOpenOrders = current?.onchain_open_orders || 0;
  const replayOpenOrders = Math.max(
    current?.replay_open_orders || 0,
    onchainOpenOrders + Math.max(1, market.pendingCount),
  );
  return {
    replay_open_orders: replayOpenOrders,
    onchain_open_orders: onchainOpenOrders,
    replay_best_bid: current?.replay_best_bid || null,
    onchain_best_bid: current?.onchain_best_bid || null,
    replay_best_ask: current?.replay_best_ask || null,
    onchain_best_ask: current?.onchain_best_ask || null,
    bid_base_lots_abs_diff: current?.bid_base_lots_abs_diff || '0',
    ask_base_lots_abs_diff: current?.ask_base_lots_abs_diff || '0',
    synthetic_reason: market.reasons.join(',') || 'lane_guard',
    suppressed_lane_count: market.laneCount,
    suppressed_pending_count: market.pendingCount,
    suppressed_pending_sequences: market.pendingSequences,
    suppressed_until_ts_ms: market.suppressUntilTsMs,
  };
}

function buildEffectiveReconciliationSnapshot(
  base: ReconciliationSnapshot | null,
  nowMs = Date.now(),
): ReconciliationSnapshot | null {
  const suppressedMarkets = laneGuard.getSuppressedMarkets(nowMs);
  if (!base && suppressedMarkets.size === 0) {
    return null;
  }

  const snapshot: ReconciliationSnapshot = base
    ? {
        ts_ms: base.ts_ms,
        replay_generated_ts_ms: base.replay_generated_ts_ms,
        onchain_generated_ts_ms: base.onchain_generated_ts_ms,
        totals: { ...base.totals },
        markets: Object.fromEntries(
          Object.entries(base.markets).map(([market, drift]) => [
            market,
            { ...drift },
          ]),
        ),
      }
    : {
        ts_ms: nowMs,
        replay_generated_ts_ms: nowMs,
        onchain_generated_ts_ms: nowMs,
        totals: {
          replay_open_orders: 0,
          onchain_open_orders: 0,
          bid_base_lots_abs_diff: '0',
          ask_base_lots_abs_diff: '0',
          markets_with_drift: 0,
        },
        markets: {},
      };

  for (const [market, suppression] of suppressedMarkets.entries()) {
    snapshot.markets[market] = suppressedMarketToDrift(
      suppression,
      snapshot.markets[market],
    );
  }

  let replayOpenOrders = 0;
  let onchainOpenOrders = 0;
  let bidAbsDiff = 0n;
  let askAbsDiff = 0n;
  let marketsWithDrift = 0;
  for (const drift of Object.values(snapshot.markets)) {
    replayOpenOrders += drift.replay_open_orders;
    onchainOpenOrders += drift.onchain_open_orders;
    bidAbsDiff += decimalStringToBigintTrunc(drift.bid_base_lots_abs_diff);
    askAbsDiff += decimalStringToBigintTrunc(drift.ask_base_lots_abs_diff);
    if (reconciliationMarketHasDrift(drift)) {
      marketsWithDrift += 1;
    }
  }

  snapshot.totals = {
    replay_open_orders: replayOpenOrders,
    onchain_open_orders: onchainOpenOrders,
    bid_base_lots_abs_diff: bidAbsDiff.toString(),
    ask_base_lots_abs_diff: askAbsDiff.toString(),
    markets_with_drift: marketsWithDrift,
  };
  snapshot.ts_ms = nowMs;
  return snapshot;
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

// [redis-phase1a] module-level publisher
const redisPublisher: Publisher = createPublisher(publisherMetricsHooks); // [redis-phase6]
// [redis-phase1b] Closure captures the module-level `engine` lazily, so this is safe
// even though `engine` is assigned asynchronously during boot.
const stateMirror: Mirror = createMirror((view) => {
  const snap = engine?.getSnapshot(view);
  if (!snap) return snap;
  // [redis-phase5] enrich EngineSnapshot with rich identity + computed
  // metrics so v1:meta:market:<id> is a complete replacement for
  // /state/markets. Both reads are from module-level caches; zero I/O.
  const enrichedMarkets: typeof snap.markets = {};
  for (const [id, m] of Object.entries(snap.markets)) {
    enrichedMarkets[id] = {
      ...m,
      metadata: harnessMarketMetadataSnapshot[id] ?? null,
    } as (typeof snap.markets)[string];
  }
  const perp = (snap as { perp_markets?: Record<string, Record<string, unknown>> }).perp_markets ?? {};
  const enrichedPerp: Record<string, Record<string, unknown>> = {};
  for (const [id, pm] of Object.entries(perp)) {
    const marketKey = String((pm as { market_index?: unknown }).market_index ?? id);
    const metrics = marketRuntimeMetricsCache.get(marketKey)?.data ?? null;
    enrichedPerp[id] = {
      ...pm,
      mark_price: metrics?.mark_price_ui ?? null,
      funding_rate_daily: metrics?.funding_rate_daily_pct ?? null,
      funding_rate_hourly: metrics?.funding_rate_hourly_pct ?? null,
      open_interest: metrics?.open_interest_base_lots ?? null,
    };
  }
  return {
    ...snap,
    markets: enrichedMarkets,
    perp_markets: enrichedPerp,
  } as unknown as ReturnType<NonNullable<typeof engine>['getSnapshot']>;
});

// [redis-phase5] For queue_item_processed events, attach the fills the
// engine produced for this taker_sequence so the publisher's extractTrades()
// walker sees them and XADDs into v1:trades:<market>. Non-throwing; on any
// lookup failure we fall back to the original event shape.
function enrichProcessedWithFills(event: HarnessEvent): HarnessEvent {
  try {
    if (!engine || event.event_type !== 'queue_item_processed') return event;
    const ev = event as unknown as Record<string, unknown>;
    const market =
      (ev['market'] as string | number | undefined) ??
      (ev['market_index'] as string | number | undefined) ??
      null;
    const sequence = ev['sequence'] as string | number | undefined;
    if (market === null || market === undefined || sequence === undefined) return event;
    const filtered = engine.getTradesFiltered({
      view: 'confirmed',
      market: String(market),
      limit: 200,
    });
    const wanted = String(sequence);
    const matches = filtered.filter(
      (t: { taker_sequence?: string | number }) => String(t.taker_sequence) === wanted,
    );
    if (matches.length === 0) return event;
    return {
      ...event,
      trades: matches.map((t: Record<string, unknown>) => ({
        maker: t['maker_owner'],
        taker: t['taker_owner'],
        price: t['price_lots'],
        size: t['base_lots'],
        side: t['taker_side'],
        ts_ms: t['ts_ms'],
        sequence: t['taker_sequence'],
      })),
    } as HarnessEvent;
  } catch {
    return event;
  }
}

function broadcastEvent(event: HarnessEvent): void {
  // [redis-phase1a + phase5] fire-and-forget mirror; enrich processed events
  // with fill details so /v2/trades, /v2/trades/wallet and /v2/stream/trades
  // see them. Never blocks, never throws.
  redisPublisher.publishEvent(
    enrichProcessedWithFills(event) as unknown as HarnessEventLike,
  );
  // [redis-phase1b] debounced snapshot mirror; never blocks, never throws
  stateMirror.onEvent();
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
        queueBufferedLogLine(
          HARNESS_EVENT_LOG_PATH,
          `${JSON.stringify(event)}\n`,
          HARNESS_EVENT_LOG_MAX_BYTES,
          'append_event_log',
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

function appendTxnLog(event: HarnessEvent): void {
  if (!HARNESS_REPLAY_LOG) {
    return;
  }
  if (
    event.event_type !== 'relay_intent_accepted' &&
    event.event_type !== 'relay_intent_status'
  ) {
    return;
  }
  measureSync(
    'append_txn_log',
    () => {
      try {
        queueBufferedLogLine(
          HARNESS_TXN_LOG_PATH,
          `${JSON.stringify(event)}\n`,
          HARNESS_EVENT_LOG_MAX_BYTES,
          'append_txn_log',
        );
      } catch (err) {
        recordRuntimeError('append_txn_log', err, {
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
    request_id: String(
      raw.request_id || `${raw.group}:${raw.sequence}:${Number(raw.kind ?? 0)}`,
    ),
    accepted_source:
      raw.accepted_source === undefined || raw.accepted_source === null
        ? undefined
        : String(raw.accepted_source),
    harness_accept_received_ts_ms:
      raw.harness_accept_received_ts_ms === undefined ||
      raw.harness_accept_received_ts_ms === null
        ? undefined
        : Number(raw.harness_accept_received_ts_ms),
    harness_preconfirm_emit_ts_ms:
      raw.harness_preconfirm_emit_ts_ms === undefined ||
      raw.harness_preconfirm_emit_ts_ms === null
        ? undefined
        : Number(raw.harness_preconfirm_emit_ts_ms),
    fast_lane_preconfirm_emitted:
      raw.fast_lane_preconfirm_emitted === undefined ||
      raw.fast_lane_preconfirm_emitted === null
        ? undefined
        : !!raw.fast_lane_preconfirm_emitted,
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

function parseRelayIntentStatusEvent(raw: any): RelayIntentStatusEvent {
  if (!raw || raw.event_type !== 'relay_intent_status') {
    throw new Error('invalid relay intent status event_type');
  }
  if (raw.status_code === undefined || raw.status_code === null) {
    throw new Error('missing field: status_code');
  }
  return {
    event_type: 'relay_intent_status',
    ts_ms: Number(raw.ts_ms || Date.now()),
    request_id: String(raw.request_id || `status:${Date.now()}`),
    status_code: Number(raw.status_code),
    status_label: String(raw.status_label || 'unknown'),
    reason:
      raw.reason === undefined || raw.reason === null ? null : String(raw.reason),
    group: raw.group ? String(raw.group) : null,
    execution_queue: raw.execution_queue ? String(raw.execution_queue) : null,
    market: raw.market ? String(raw.market) : null,
    sequence:
      raw.sequence === undefined || raw.sequence === null
        ? null
        : String(raw.sequence),
    kind:
      raw.kind === undefined || raw.kind === null ? null : Number(raw.kind),
    user_owner: raw.user_owner ? String(raw.user_owner) : null,
    mango_account: raw.mango_account ? String(raw.mango_account) : null,
    tx_signature:
      raw.tx_signature === undefined || raw.tx_signature === null
        ? null
        : String(raw.tx_signature),
    grpc_code:
      raw.grpc_code === undefined || raw.grpc_code === null
        ? null
        : Number(raw.grpc_code),
    queue_process_status:
      raw.queue_process_status === undefined || raw.queue_process_status === null
        ? null
        : Number(raw.queue_process_status),
    queue_process_status_name:
      raw.queue_process_status_name === undefined ||
      raw.queue_process_status_name === null
        ? null
        : String(raw.queue_process_status_name),
  };
}

function wireBytesToBuffer(value: unknown): Buffer {
  if (Buffer.isBuffer(value)) {
    return value;
  }
  if (value instanceof Uint8Array) {
    return Buffer.from(value);
  }
  if (typeof value === 'string') {
    return Buffer.from(value, 'base64');
  }
  return Buffer.alloc(0);
}

function microsToMs(value: unknown): number {
  const numeric = Number(value);
  if (!Number.isFinite(numeric) || numeric <= 0) {
    return Date.now();
  }
  return Math.floor(numeric / 1_000);
}

function hasCanonicalIntentData(intent: any): boolean {
  return (
    !!intent &&
    typeof intent.payload_b64 === 'string' &&
    intent.payload_b64.length > 0 &&
    typeof intent.market === 'string' &&
    intent.market.length > 0 &&
    intent.market !== 'unknown' &&
    typeof intent.execution_queue === 'string' &&
    intent.execution_queue.length > 0 &&
    intent.execution_queue !== 'unknown'
  );
}

function resolveDefaultSequencerAcceptedStreamUrl(): string {
  return HARNESS_SEQUENCER_ACCEPTED_STREAM_URL;
}

function resolveDefaultSequencerTickStreamUrl(): string {
  if (HARNESS_SEQUENCER_TICK_STREAM_URL) {
    return HARNESS_SEQUENCER_TICK_STREAM_URL;
  }
  return resolveDefaultSequencerInternalStreamUrl('/tick-stream');
}

function resolveDefaultSequencerInternalStreamUrl(streamPath: string): string {
  if (!HARNESS_SEQUENCER_GRPC_ADDR) {
    return '';
  }
  if (!HARNESS_SEQUENCER_PREFER_INTERNAL_HTTP) {
    return '';
  }
  const runtimeInfo = loadContinuumSequencerRuntimeInfo(
    HARNESS_SEQUENCER_RUNTIME_INFO_PATH,
  );
  if (!runtimeInfo?.endpoints) {
    return '';
  }
  if (
    HARNESS_SEQUENCER_GRPC_ADDR &&
    !sequencerRuntimeInfoMatchesGrpcAddr(
      runtimeInfo,
      HARNESS_SEQUENCER_GRPC_ADDR,
    )
  ) {
    return '';
  }
  return buildSequencerInternalStreamUrl(
    runtimeInfo.endpoints.internal_api,
    streamPath,
  );
}

function loadContinuumSequencerRuntimeInfo(
  runtimeInfoPath: string,
): ContinuumSequencerRuntimeInfo {
  if (!runtimeInfoPath) {
    return null;
  }
  try {
    if (!fs.existsSync(runtimeInfoPath)) {
      return null;
    }
    return JSON.parse(
      fs.readFileSync(runtimeInfoPath, 'utf-8'),
    ) as ContinuumSequencerRuntimeInfo;
  } catch {
    return null;
  }
}

function sequencerRuntimeInfoMatchesGrpcAddr(
  runtimeInfo: ContinuumSequencerRuntimeInfo,
  grpcAddr: string,
): boolean {
  const normalizedGrpcAddr = normalizeSocketAddr(grpcAddr);
  if (!normalizedGrpcAddr) {
    return false;
  }
  const internalGrpc = normalizeSocketAddr(
    runtimeInfo?.endpoints?.internal_grpc || '',
  );
  const externalGrpc = normalizeSocketAddr(
    runtimeInfo?.endpoints?.external_grpc || '',
  );
  return (
    normalizedGrpcAddr === internalGrpc || normalizedGrpcAddr === externalGrpc
  );
}

function buildSequencerInternalStreamUrl(
  bindAddr: string | undefined,
  streamPath: string,
): string {
  const parsed = parseSocketAddr(bindAddr || '');
  if (!parsed || !isLocalSocketHost(parsed.host)) {
    return '';
  }
  const host = normalizeSocketHostForUrl(parsed.host);
  if (!host) {
    return '';
  }
  return `http://${host}:${parsed.port}${streamPath}`;
}

function parseSocketAddr(
  value: string,
): { host: string; port: string } | null {
  const trimmed = value.trim();
  if (!trimmed.length) {
    return null;
  }
  if (trimmed.startsWith('[')) {
    const bracketIndex = trimmed.indexOf(']');
    if (bracketIndex < 0 || trimmed.charAt(bracketIndex + 1) !== ':') {
      return null;
    }
    return {
      host: trimmed.slice(1, bracketIndex),
      port: trimmed.slice(bracketIndex + 2),
    };
  }
  const separatorIndex = trimmed.lastIndexOf(':');
  if (separatorIndex <= 0 || separatorIndex === trimmed.length - 1) {
    return null;
  }
  return {
    host: trimmed.slice(0, separatorIndex),
    port: trimmed.slice(separatorIndex + 1),
  };
}

function normalizeSocketAddr(value: string): string {
  const parsed = parseSocketAddr(value);
  if (!parsed) {
    return '';
  }
  return `${parsed.host}:${parsed.port}`;
}

function isLocalSocketHost(host: string): boolean {
  const normalized = host.trim().toLowerCase();
  return (
    normalized === '127.0.0.1' ||
    normalized === 'localhost' ||
    normalized === '::1' ||
    normalized === '0.0.0.0' ||
    normalized === '::'
  );
}

function normalizeSocketHostForUrl(host: string): string {
  const normalized = host.trim().toLowerCase();
  if (!normalized.length) {
    return '';
  }
  if (normalized === '0.0.0.0') {
    return '127.0.0.1';
  }
  if (normalized === '::') {
    return '[::1]';
  }
  if (normalized.includes(':')) {
    return `[${normalized}]`;
  }
  return normalized;
}

function loadContinuumSequencerProto(): any {
  const protoPath = path.resolve(__dirname, 'continuum_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  return grpc.loadPackageDefinition(pkgDef) as any;
}

function createContinuumSequencerClient(
  address: string,
): ContinuumSequencerClient {
  const proto = loadContinuumSequencerProto();
  return new proto.continuum.sequencer.v1.SequencerService(
    address,
    grpc.credentials.createInsecure(),
    {
      'grpc.keepalive_time_ms': 15000,
      'grpc.keepalive_timeout_ms': 5000,
      'grpc.keepalive_permit_without_calls': 1,
    },
  ) as ContinuumSequencerClient;
}

function sequencerWireToRelayIntentEvent(params: {
  transaction: ContinuumSequencerTransactionWire;
  sequenceNumber: string | number | null | undefined;
  txHash: string | null | undefined;
  ingestionTimestamp: string | number | null | undefined;
  requestIdPrefix: string;
  acceptedSource: string;
}): RelayIntentAcceptedEvent | null {
  const tx = params.transaction;
  const meta = tx?.intent_metadata;
  if (
    !tx ||
    !meta ||
    !meta.group ||
    !meta.execution_queue ||
    !meta.market ||
    !meta.user_owner ||
    !meta.mango_account ||
    params.sequenceNumber === undefined ||
    params.sequenceNumber === null
  ) {
    return null;
  }

  const payload = wireBytesToBuffer(tx.payload);
  return parseRelayIntentEvent({
    event_type: 'relay_intent_accepted',
    ts_ms: microsToMs(params.ingestionTimestamp),
    accepted_source: params.acceptedSource,
    request_id: `${params.requestIdPrefix}:${String(params.sequenceNumber)}:${String(
      params.txHash || tx.tx_id || 'unknown',
    )}`,
    group: String(meta.group),
    execution_queue: String(meta.execution_queue),
    market: String(meta.market),
    sequence: String(params.sequenceNumber),
    kind: Number(meta.kind ?? 0),
    payload_b64: payload.toString('base64'),
    remaining_accounts: Array.isArray(meta.remaining_accounts)
      ? meta.remaining_accounts.map((account) => ({
          pubkey: String(account?.pubkey || ''),
          is_signer: !!account?.is_signer,
          is_writable: !!account?.is_writable,
        }))
      : [],
    min_execute_slot: String(meta.min_execute_slot ?? '0'),
    expires_at_slot: String(meta.expires_at_slot ?? '0'),
    user_owner: String(meta.user_owner),
    mango_account: String(meta.mango_account),
    enqueue_tx_signature: String(
      params.txHash || tx.tx_id || `${meta.group}:${params.sequenceNumber}`,
    ),
  });
}

function acceptedTransactionToRelayIntentEvent(
  raw: ContinuumSequencerAcceptedTransactionWire,
): RelayIntentAcceptedEvent | null {
  return sequencerWireToRelayIntentEvent({
    transaction: raw.transaction || null,
    sequenceNumber: raw.sequence_number,
    txHash: raw.tx_hash,
    ingestionTimestamp: raw.ingestion_timestamp,
    requestIdPrefix: 'sequencer',
    acceptedSource: 'sequencer_accept_stream',
  });
}

function orderedTransactionToRelayIntentEvent(
  tickNumber: string | number | null | undefined,
  ordered: ContinuumSequencerOrderedTransactionWire,
): RelayIntentAcceptedEvent | null {
  return sequencerWireToRelayIntentEvent({
    transaction: ordered.transaction || null,
    sequenceNumber: ordered.sequence_number,
    txHash: ordered.tx_hash,
    ingestionTimestamp: ordered.ingestion_timestamp,
    requestIdPrefix: `sequencer-tick-${String(tickNumber ?? 'unknown')}`,
    acceptedSource: 'sequencer_tick_stream',
  });
}

function isSequencerLocalProcessedSignature(
  signature: string | null | undefined,
): boolean {
  return (
    typeof signature === 'string' &&
    signature.startsWith(SEQUENCER_TICK_PROCESSED_SIGNATURE_PREFIX)
  );
}

function orderedTransactionToQueueProcessedEvent(
  tick: ContinuumSequencerTickWire,
  ordered: ContinuumSequencerOrderedTransactionWire,
  tickReceivedTsMs: number | null = null,
): QueueItemProcessedEvent | null {
  const meta = ordered.transaction?.intent_metadata;
  if (
    !meta ||
    !meta.group ||
    ordered.sequence_number === undefined ||
    ordered.sequence_number === null
  ) {
    return null;
  }
  const tickNumber = String(tick.tick_number ?? 'unknown');
  const txHash = String(
    ordered.tx_hash ||
      ordered.transaction?.tx_id ||
      `${meta.group}:${String(ordered.sequence_number)}`,
  );
  const tsMs = microsToMs(tick.timestamp);
  return {
    event_type: 'queue_item_processed',
    ts_ms: tsMs,
    group: String(meta.group),
    sequence: String(ordered.sequence_number),
    kind: Number(meta.kind ?? 0),
    status: LOCAL_QUEUE_PROCESS_EXECUTED,
    slot: String(meta.min_execute_slot ?? '0'),
    tx_signature: `${SEQUENCER_TICK_PROCESSED_SIGNATURE_PREFIX}${tickNumber}:${txHash}`,
    market: String(meta.market || ''),
    user_owner: String(meta.user_owner || ''),
    mango_account: String(meta.mango_account || ''),
    processed_unix_ts: Math.floor(tsMs / 1_000),
    harness_tick_received_ts_ms: tickReceivedTsMs,
  };
}

function relayIntentTrackingKey(event: Pick<
  RelayIntentAcceptedEvent,
  'group' | 'sequence' | 'kind'
>): string {
  return buildRelayIntentTrackingKey(event);
}

function rememberFastPreconfirm(event: Pick<
  RelayIntentAcceptedEvent,
  'group' | 'sequence' | 'kind'
>): void {
  const key = relayIntentTrackingKey(event);
  if (fastPreconfirmKeys.has(key)) {
    return;
  }
  fastPreconfirmKeys.add(key);
  fastPreconfirmKeyOrder.push(key);
  while (fastPreconfirmKeyOrder.length > FAST_PRECONFIRM_KEY_LIMIT) {
    const evicted = fastPreconfirmKeyOrder.shift();
    if (evicted) {
      fastPreconfirmKeys.delete(evicted);
    }
  }
}

function hasFastPreconfirm(event: Pick<
  RelayIntentAcceptedEvent,
  'group' | 'sequence' | 'kind'
>): boolean {
  return fastPreconfirmKeys.has(relayIntentTrackingKey(event));
}

function logLaneGuardDecision(
  decision: LaneGuardDecision,
  source: string,
): void {
  console.warn(
    JSON.stringify({
      ts: new Date().toISOString(),
      msg: 'continuum-lane-guard',
      source,
      market: decision.market,
      lane_hash: decision.laneHash,
      tracking_key: decision.trackingKey,
      payload_hash: decision.payloadHash,
      pending_count: decision.pendingCount,
      suppression_reason: decision.suppressionReason,
      market_suppressed: decision.marketSuppressed,
    }),
  );
}

function applyLaneGuardAccepted(
  event: RelayIntentAcceptedEvent,
  opts: {
    source: string;
    persistSuppressedEvent?: boolean;
    allowFastPreconfirm?: boolean;
    deferEngineIngest?: boolean;
    onchain?: OnchainContext | null;
    onchainSync?: OnchainSyncState;
  },
): void {
  const decision = laneGuard.observeAccepted(
    event,
    event.harness_accept_received_ts_ms || event.ts_ms || Date.now(),
  );
  // FIFO fix: do NOT drop suppressed intents from replay — that breaks
  // strict sequence ordering (later intents would replay while earlier
  // suppressed ones are skipped). We still log the decision and persist
  // the event for divergence observability, but always continue to ingest.
  if (decision.shouldSuppressOptimisticReplay) {
    logLaneGuardDecision(decision, opts.source);
    if (opts.persistSuppressedEvent) {
      appendEventLog(event);
      appendTxnLog(event);
    }
    if (
      decision.marketSuppressionActivated &&
      opts.onchain &&
      opts.onchainSync
    ) {
      scheduleDirectOnchainRebase(opts.onchain, opts.onchainSync, {
        reason: 'lane_guard_suppressed_intent',
        market: event.market,
        lane_hash: decision.laneHash,
      });
    }
    // fall through to ingest — do not return
  }

  if (opts.allowFastPreconfirm) {
    event.fast_lane_preconfirm_emitted = true;
    event.harness_preconfirm_emit_ts_ms = Date.now();
    rememberFastPreconfirm(event);
    notifyFrontendPreconfirmSubscribers(event);
  }

  if (opts.deferEngineIngest) {
    scheduleDeferredAcceptedIntentIngest(event);
    return;
  }

  engine.ingestRelayIntent(event);
}

function applyLaneGuardStatus(
  event: RelayIntentStatusEvent,
  opts: {
    source: string;
    onchain?: OnchainContext | null;
    onchainSync?: OnchainSyncState;
  } = { source: 'status' },
): void {
  const decision = laneGuard.observeRelayIntentStatus(event, event.ts_ms || Date.now());
  if (!decision) {
    return;
  }
  if (decision.marketSuppressionActivated) {
    logLaneGuardDecision(decision, opts.source);
  }
  if (
    decision.marketSuppressionActivated &&
    opts.onchain &&
    opts.onchainSync
  ) {
    scheduleDirectOnchainRebase(opts.onchain, opts.onchainSync, {
      reason: 'lane_guard_status_failure',
      market: decision.market,
      lane_hash: decision.laneHash,
    });
  }
}

function applyLaneGuardProcessed(
  event: QueueItemProcessedEvent,
  opts: {
    source: string;
    onchain?: OnchainContext | null;
    onchainSync?: OnchainSyncState;
  } = { source: 'processed' },
): void {
  const decision = laneGuard.observeQueueItemProcessed(
    event,
    event.ts_ms || Date.now(),
  );
  if (!decision) {
    return;
  }
  if (decision.marketSuppressionActivated) {
    logLaneGuardDecision(decision, opts.source);
  }
  if (
    decision.marketSuppressionActivated &&
    opts.onchain &&
    opts.onchainSync
  ) {
    scheduleDirectOnchainRebase(opts.onchain, opts.onchainSync, {
      reason: 'lane_guard_processed_failure',
      market: decision.market,
      lane_hash: decision.laneHash,
    });
  }
}

function scheduleDeferredAcceptedIntentIngest(event: RelayIntentAcceptedEvent): void {
  deferredAcceptedIntentIngestQueue.push(event);
  if (deferredAcceptedIntentIngestScheduled) {
    return;
  }
  deferredAcceptedIntentIngestScheduled = true;
  setImmediate(() => {
    deferredAcceptedIntentIngestScheduled = false;
    while (deferredAcceptedIntentIngestQueue.length > 0) {
      const next = deferredAcceptedIntentIngestQueue.shift();
      if (!next) {
        break;
      }
      measureSync(
        'sequencer_accept_deferred_ingest',
        () => {
          engine.ingestRelayIntent(next);
        },
        {
          thresholdMs: HARNESS_SLOW_COMPONENT_MS,
          context: {
            sequence: next.sequence,
            market: next.market,
          },
        },
      );
    }
  });
}

function handleSequencerAcceptedTransactionWire(
  raw: ContinuumSequencerAcceptedTransactionWire,
): void {
  measureSync(
    'sequencer_accept_stream_message',
    () => {
      const event = acceptedTransactionToRelayIntentEvent(raw);
      if (!event) {
        return;
      }
      const eventMarketIdx = parseMarketIndexHint(event.market);
      const existing = engine.findIntent(
        event.group,
        event.sequence,
        event.kind,
        eventMarketIdx,
      );
      if (
        hasCanonicalIntentData(existing) ||
        laneGuard.findTrackedIntent(event.group, event.sequence, event.kind)
      ) {
        return;
      }
      event.harness_accept_received_ts_ms = Date.now();
      applyLaneGuardAccepted(event, {
        source: 'sequencer_accept_stream',
        allowFastPreconfirm: true,
        deferEngineIngest: true,
      });
    },
    {
      thresholdMs: HARNESS_SLOW_COMPONENT_MS,
      context: {
        sequence:
          raw.sequence_number === undefined || raw.sequence_number === null
            ? 'unknown'
            : String(raw.sequence_number),
        tx_hash: raw.tx_hash || 'unknown',
      },
    },
  );
}

function startContinuumSequencerAcceptedIngest(): void {
  const acceptedStreamUrl = resolveDefaultSequencerAcceptedStreamUrl();
  if (acceptedStreamUrl) {
    activeSequencerAcceptedTransport = 'http';
    activeSequencerAcceptedStreamUrl = acceptedStreamUrl;
    startContinuumSequencerAcceptedHttpIngest();
    return;
  }
  if (!HARNESS_SEQUENCER_GRPC_ADDR) {
    activeSequencerAcceptedTransport = 'none';
    activeSequencerAcceptedStreamUrl = null;
    return;
  }
  activeSequencerAcceptedTransport = 'grpc';
  activeSequencerAcceptedStreamUrl = null;

  const client = createContinuumSequencerClient(HARNESS_SEQUENCER_GRPC_ADDR);
  let reconnectTimer: NodeJS.Timeout | null = null;
  let activeCall: grpc.ClientReadableStream<ContinuumSequencerAcceptedTransactionWire> | null =
    null;

  const scheduleReconnect = (reason: string) => {
    if (reconnectTimer) {
      return;
    }
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, Math.max(100, HARNESS_SEQUENCER_GRPC_RECONNECT_MS));
    recordRuntimeError('sequencer_accept_stream_reconnect', reason, {
      addr: HARNESS_SEQUENCER_GRPC_ADDR,
    });
  };

  const clearActiveCall = () => {
    activeCall = null;
  };

  const connect = () => {
    if (activeCall) {
      return;
    }

    try {
      activeCall = client.streamAcceptedTransactions({});
    } catch (err) {
      recordRuntimeError('sequencer_accept_stream_connect', err, {
        addr: HARNESS_SEQUENCER_GRPC_ADDR,
      });
      scheduleReconnect('connect_exception');
      return;
    }

    activeCall.on('data', (raw) => {
      handleSequencerAcceptedTransactionWire(raw);
    });

    activeCall.on('error', (err) => {
      clearActiveCall();
      recordRuntimeError('sequencer_accept_stream', err, {
        addr: HARNESS_SEQUENCER_GRPC_ADDR,
      });
      scheduleReconnect('error');
    });

    activeCall.on('end', () => {
      clearActiveCall();
      scheduleReconnect('end');
    });

    activeCall.on('close', () => {
      clearActiveCall();
      scheduleReconnect('close');
    });
  };

  connect();
}

function startContinuumSequencerAcceptedHttpIngest(): void {
  const acceptedStreamUrl = activeSequencerAcceptedStreamUrl;
  if (!acceptedStreamUrl) {
    return;
  }

  let reconnectTimer: NodeJS.Timeout | null = null;
  let activeRequest: http.ClientRequest | null = null;
  let activeResponse: IncomingMessage | null = null;
  let pendingBuffer = '';

  const scheduleReconnect = (reason: string) => {
    if (reconnectTimer) {
      return;
    }
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, Math.max(100, HARNESS_SEQUENCER_GRPC_RECONNECT_MS));
    recordRuntimeError('sequencer_accept_http_stream_reconnect', reason, {
      url: acceptedStreamUrl,
    });
  };

  const clearActiveHandles = () => {
    activeRequest = null;
    activeResponse = null;
    pendingBuffer = '';
  };

  const connect = () => {
    if (activeRequest || activeResponse) {
      return;
    }

    let url: URL;
    try {
      url = new URL(acceptedStreamUrl);
    } catch (err) {
      recordRuntimeError('sequencer_accept_http_stream_url', err, {
        url: acceptedStreamUrl,
      });
      return;
    }

    const request = http.request({
      protocol: url.protocol,
      hostname: url.hostname,
      port: url.port,
      path: `${url.pathname}${url.search}`,
      method: 'GET',
      headers: {
        Accept: 'application/x-ndjson',
      },
    });

    activeRequest = request;

    request.on('response', (response) => {
      activeRequest = null;
      if (response.statusCode !== 200) {
        response.resume();
        clearActiveHandles();
        recordRuntimeError(
          'sequencer_accept_http_stream_status',
          `unexpected_status_${response.statusCode || 0}`,
          {
            url: acceptedStreamUrl,
            status_code: response.statusCode || 0,
          },
        );
        scheduleReconnect(`status_${response.statusCode || 0}`);
        return;
      }

      activeResponse = response;
      response.setEncoding('utf8');

      response.on('data', (chunk: string) => {
        pendingBuffer += chunk;
        while (true) {
          const newlineIndex = pendingBuffer.indexOf('\n');
          if (newlineIndex < 0) {
            break;
          }
          const line = pendingBuffer.slice(0, newlineIndex).trim();
          pendingBuffer = pendingBuffer.slice(newlineIndex + 1);
          if (!line) {
            continue;
          }
          try {
            const parsed = JSON.parse(
              line,
            ) as ContinuumSequencerAcceptedTransactionWire;
            handleSequencerAcceptedTransactionWire(parsed);
          } catch (err) {
            recordRuntimeError('sequencer_accept_http_stream_parse', err, {
              url: acceptedStreamUrl,
            });
          }
        }
      });

      response.on('error', (err) => {
        clearActiveHandles();
        recordRuntimeError('sequencer_accept_http_stream', err, {
          url: acceptedStreamUrl,
        });
        scheduleReconnect('error');
      });

      response.on('end', () => {
        clearActiveHandles();
        scheduleReconnect('end');
      });

      response.on('close', () => {
        clearActiveHandles();
        scheduleReconnect('close');
      });
    });

    request.on('error', (err) => {
      clearActiveHandles();
      recordRuntimeError('sequencer_accept_http_stream_connect', err, {
        url: acceptedStreamUrl,
      });
      scheduleReconnect('error');
    });

    request.end();
  };

  connect();
}

function processSequencerTickWire(tick: ContinuumSequencerTickWire): void {
  measureSync(
    'sequencer_tick_stream_message',
    () => {
      const tickReceivedTsMs = Date.now();
      for (const ordered of tick.transactions || []) {
        const acceptedEvent = orderedTransactionToRelayIntentEvent(
          tick.tick_number,
          ordered,
        );
        if (acceptedEvent) {
          const acceptedMarketIdx = parseMarketIndexHint(acceptedEvent.market);
          const existing = engine.findIntent(
            acceptedEvent.group,
            acceptedEvent.sequence,
            acceptedEvent.kind,
            acceptedMarketIdx,
          );
          if (
            !hasCanonicalIntentData(existing) &&
            !laneGuard.findTrackedIntent(
              acceptedEvent.group,
              acceptedEvent.sequence,
              acceptedEvent.kind,
            )
          ) {
            applyLaneGuardAccepted(acceptedEvent, {
              source: 'sequencer_tick',
            });
          }
        }

        const processedEvent = orderedTransactionToQueueProcessedEvent(
          tick,
          ordered,
          tickReceivedTsMs,
        );
        if (!processedEvent) {
          continue;
        }
        applyLaneGuardProcessed(processedEvent, {
          source: 'sequencer_tick',
        });
        engine.ingestQueueProcessed(processedEvent);
      }
    },
    {
      thresholdMs: HARNESS_SLOW_COMPONENT_MS,
      context: {
        tick_number:
          tick.tick_number === undefined || tick.tick_number === null
            ? 'unknown'
            : String(tick.tick_number),
        transactions: Array.isArray(tick.transactions)
          ? tick.transactions.length
          : 0,
      },
    },
  );
}

function startContinuumSequencerTickIngest(): void {
  const tickStreamUrl = resolveDefaultSequencerTickStreamUrl();
  if (tickStreamUrl) {
    activeSequencerTickTransport = 'http';
    activeSequencerTickStreamUrl = tickStreamUrl;
    startContinuumSequencerTickHttpIngest();
    return;
  }
  if (!HARNESS_SEQUENCER_GRPC_ADDR) {
    activeSequencerTickTransport = 'none';
    activeSequencerTickStreamUrl = null;
    return;
  }
  activeSequencerTickTransport = 'grpc';
  activeSequencerTickStreamUrl = null;

  const client = createContinuumSequencerClient(HARNESS_SEQUENCER_GRPC_ADDR);
  let reconnectTimer: NodeJS.Timeout | null = null;
  let activeCall: grpc.ClientReadableStream<ContinuumSequencerTickWire> | null =
    null;

  const scheduleReconnect = (reason: string) => {
    if (reconnectTimer) {
      return;
    }
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, Math.max(100, HARNESS_SEQUENCER_GRPC_RECONNECT_MS));
    recordRuntimeError('sequencer_tick_stream_reconnect', reason, {
      addr: HARNESS_SEQUENCER_GRPC_ADDR,
    });
  };

  const clearActiveCall = () => {
    activeCall = null;
  };

  const connect = () => {
    if (activeCall) {
      return;
    }

    try {
      activeCall = client.streamTicks({
        start_tick: 0,
        transactions_only: true,
      });
    } catch (err) {
      recordRuntimeError('sequencer_tick_stream_connect', err, {
        addr: HARNESS_SEQUENCER_GRPC_ADDR,
      });
      scheduleReconnect('connect_exception');
      return;
    }

    activeCall.on('data', (tick) => {
      processSequencerTickWire(tick);
    });

    activeCall.on('error', (err) => {
      clearActiveCall();
      recordRuntimeError('sequencer_tick_stream', err, {
        addr: HARNESS_SEQUENCER_GRPC_ADDR,
      });
      scheduleReconnect('error');
    });

    activeCall.on('end', () => {
      clearActiveCall();
      scheduleReconnect('end');
    });

    activeCall.on('close', () => {
      clearActiveCall();
      scheduleReconnect('close');
    });
  };

  connect();
}

function startContinuumSequencerTickHttpIngest(): void {
  const tickStreamUrl = activeSequencerTickStreamUrl;
  if (!tickStreamUrl) {
    return;
  }

  let reconnectTimer: NodeJS.Timeout | null = null;
  let activeRequest: http.ClientRequest | null = null;
  let activeResponse: IncomingMessage | null = null;
  let pendingBuffer = '';

  const scheduleReconnect = (reason: string) => {
    if (reconnectTimer) {
      return;
    }
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, Math.max(100, HARNESS_SEQUENCER_GRPC_RECONNECT_MS));
    recordRuntimeError('sequencer_tick_http_stream_reconnect', reason, {
      url: tickStreamUrl,
    });
  };

  const clearActiveHandles = () => {
    activeRequest = null;
    activeResponse = null;
    pendingBuffer = '';
  };

  const connect = () => {
    if (activeRequest || activeResponse) {
      return;
    }

    let url: URL;
    try {
      url = new URL(tickStreamUrl);
    } catch (err) {
      recordRuntimeError('sequencer_tick_http_stream_url', err, {
        url: tickStreamUrl,
      });
      return;
    }

    const request = http.request({
      protocol: url.protocol,
      hostname: url.hostname,
      port: url.port,
      path: `${url.pathname}${url.search}`,
      method: 'GET',
      headers: {
        Accept: 'application/x-ndjson',
      },
    });

    activeRequest = request;

    request.on('response', (response) => {
      activeRequest = null;
      if (response.statusCode !== 200) {
        response.resume();
        clearActiveHandles();
        recordRuntimeError(
          'sequencer_tick_http_stream_status',
          `unexpected_status_${response.statusCode || 0}`,
          {
            url: tickStreamUrl,
            status_code: response.statusCode || 0,
          },
        );
        scheduleReconnect(`status_${response.statusCode || 0}`);
        return;
      }

      activeResponse = response;
      response.setEncoding('utf8');

      response.on('data', (chunk: string) => {
        pendingBuffer += chunk;
        while (true) {
          const newlineIndex = pendingBuffer.indexOf('\n');
          if (newlineIndex < 0) {
            break;
          }
          const line = pendingBuffer.slice(0, newlineIndex).trim();
          pendingBuffer = pendingBuffer.slice(newlineIndex + 1);
          if (!line) {
            continue;
          }
          try {
            processSequencerTickWire(
              JSON.parse(line) as ContinuumSequencerTickWire,
            );
          } catch (err) {
            recordRuntimeError('sequencer_tick_http_stream_parse', err, {
              url: tickStreamUrl,
            });
          }
        }
      });

      response.on('error', (err) => {
        clearActiveHandles();
        recordRuntimeError('sequencer_tick_http_stream', err, {
          url: tickStreamUrl,
        });
        scheduleReconnect('error');
      });

      response.on('end', () => {
        clearActiveHandles();
        scheduleReconnect('end');
      });

      response.on('close', () => {
        clearActiveHandles();
        scheduleReconnect('close');
      });
    });

    request.on('error', (err) => {
      clearActiveHandles();
      recordRuntimeError('sequencer_tick_http_stream_connect', err, {
        url: tickStreamUrl,
      });
      scheduleReconnect('error');
    });

    request.end();
  };

  connect();
}

function parseRelayIngestEvent(
  raw: any,
): RelayIntentAcceptedEvent | RelayIntentStatusEvent {
  if (raw?.event_type === 'relay_intent_status') {
    return parseRelayIntentStatusEvent(raw);
  }
  return parseRelayIntentEvent(raw);
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
  const laneGuardStats = laneGuard.getStats();
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
    sequencer_grpc_addr: HARNESS_SEQUENCER_GRPC_ADDR || null,
    sequencer_runtime_info_path: HARNESS_SEQUENCER_RUNTIME_INFO_PATH || null,
    sequencer_prefer_internal_http: HARNESS_SEQUENCER_PREFER_INTERNAL_HTTP,
    sequencer_accepted_transport: activeSequencerAcceptedTransport,
    sequencer_accepted_stream_url: activeSequencerAcceptedStreamUrl,
    sequencer_tick_transport: activeSequencerTickTransport,
    sequencer_tick_stream_url: activeSequencerTickStreamUrl,
    intents_total: intentsTotal,
    divergences_total: divergencesTotal,
    lane_guard_active_lanes: laneGuardStats.activeLanes,
    lane_guard_tracked_intents: laneGuardStats.trackedIntents,
    lane_guard_pending_intents: laneGuardStats.pendingIntents,
    lane_guard_suppressed_markets: laneGuardStats.suppressedMarkets,
    lane_guard_suppressed_intents_total: laneGuardStats.totalSuppressedIntents,
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
    '# TYPE continuum_harness_lane_guard_active_lanes gauge',
    `continuum_harness_lane_guard_active_lanes ${stats.lane_guard_active_lanes}`,
    '# TYPE continuum_harness_lane_guard_tracked_intents gauge',
    `continuum_harness_lane_guard_tracked_intents ${stats.lane_guard_tracked_intents}`,
    '# TYPE continuum_harness_lane_guard_pending_intents gauge',
    `continuum_harness_lane_guard_pending_intents ${stats.lane_guard_pending_intents}`,
    '# TYPE continuum_harness_lane_guard_suppressed_markets gauge',
    `continuum_harness_lane_guard_suppressed_markets ${stats.lane_guard_suppressed_markets}`,
    '# TYPE continuum_harness_lane_guard_suppressed_intents_total gauge',
    `continuum_harness_lane_guard_suppressed_intents_total ${stats.lane_guard_suppressed_intents_total}`,
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

type MonitoringMarketItem = {
  market: string;
  market_name: string;
  last_collected_ms: number | null;
  volume_24h_base_lots: string;
  volume_24h_quote_lots: string;
  volume_24h_base_ui: number | null;
  volume_24h_quote_ui: number | null;
  trade_count_24h: number | null;
  change_24h_pct: number | null;
  last_price_ui: number | null;
  cumulative_volume_base_lots: string;
  cumulative_volume_quote_lots: string;
  cumulative_volume_base_ui: number | null;
  cumulative_volume_quote_ui: number | null;
  open_interest_base_lots: string | null;
  open_interest_base_ui: number | null;
  fees_accrued_native: string | null;
  fees_settled_native: string | null;
  active_unique_users: number;
  all_time_unique_users: number;
  current_funding_hourly_pct: number | null;
  current_funding_daily_pct: number | null;
  current_oracle_price_ui: number | null;
  funding_rate_history: any[];
};

async function buildMonitoringItems(
  onchain: OnchainContext | null,
  fullHistory: boolean,
): Promise<MonitoringMarketItem[]> {
  const monitoringMetadata = await getMarketMetadataMapSafe(onchain);
  const monitoringSnapshot = engine.getSnapshot('confirmed');

  const monitoringMarketKeys = new Set([
    ...Object.keys(monitoringSnapshot.markets),
    ...Object.keys(monitoringMetadata),
    ...Object.keys(marketStatsStore.markets),
  ]);

  return Array.from(monitoringMarketKeys).map((marketKey) => {
    const stats = marketStatsStore.markets[marketKey] ?? null;
    const meta = monitoringMetadata[marketKey] ?? null;
    const marketName = meta?.name ?? stats?.market_name ?? marketKey;

    const tradesFor24h = engine.getTrades(marketKey, 'confirmed', 50_000);
    const tradeSummary24h = buildTradeSummary(
      marketKey,
      'confirmed',
      tradesFor24h,
      meta,
      TRADE_SUMMARY_WINDOW_MS,
    );

    let activeUniqueUsers = 0;
    for (const user of Object.values(monitoringSnapshot.users)) {
      if (user.per_market.some((p) => p.market === marketKey)) {
        activeUniqueUsers += 1;
      }
    }

    const latestFunding =
      stats?.funding_rate_history?.length
        ? stats.funding_rate_history[stats.funding_rate_history.length - 1]
        : null;

    const fundingHistory = fullHistory
      ? (stats?.funding_rate_history ?? [])
      : (stats?.funding_rate_history ?? []).slice(-48);

    return {
      market: marketKey,
      market_name: marketName,
      last_collected_ms: stats?.last_collected_ms ?? null,
      volume_24h_base_lots: tradeSummary24h.volume_base_lots,
      volume_24h_quote_lots: tradeSummary24h.volume_quote_lots,
      volume_24h_base_ui: tradeSummary24h.volume_base_ui,
      volume_24h_quote_ui: tradeSummary24h.volume_quote_ui,
      trade_count_24h: tradeSummary24h.trade_count,
      change_24h_pct: tradeSummary24h.change_24h_pct,
      last_price_ui: tradeSummary24h.last_price_ui,
      cumulative_volume_base_lots: stats?.cumulative_volume_base_lots ?? '0',
      cumulative_volume_quote_lots: stats?.cumulative_volume_quote_lots ?? '0',
      cumulative_volume_base_ui: baseLotsToUi(
        stats?.cumulative_volume_base_lots ?? '0',
        meta,
      ),
      cumulative_volume_quote_ui: quoteLotsToUi(
        stats?.cumulative_volume_quote_lots ?? '0',
        meta,
      ),
      open_interest_base_lots: stats?.open_interest_base_lots ?? null,
      open_interest_base_ui: stats?.open_interest_base_ui ?? null,
      fees_accrued_native: stats?.fees_accrued_native ?? null,
      fees_settled_native: stats?.fees_settled_native ?? null,
      active_unique_users: activeUniqueUsers,
      all_time_unique_users: stats?.all_time_unique_users?.length ?? 0,
      current_funding_hourly_pct: latestFunding?.hourly_pct ?? null,
      current_funding_daily_pct: latestFunding?.daily_pct ?? null,
      current_oracle_price_ui: latestFunding?.oracle_price_ui ?? null,
      funding_rate_history: fundingHistory,
    };
  });
}

async function buildMonitoringResponse(
  onchain: OnchainContext | null,
  fullHistory: boolean,
): Promise<{
  generated_ts_ms: number;
  collection_interval_ms: number;
  stats_file: string;
  last_collection_ms: number | null;
  markets: MonitoringMarketItem[];
}> {
  return {
    generated_ts_ms: Date.now(),
    collection_interval_ms: HARNESS_MARKET_STATS_INTERVAL_MS,
    stats_file: HARNESS_MARKET_STATS_PATH,
    last_collection_ms: marketStatsStore.written_ts_ms,
    markets: await buildMonitoringItems(onchain, fullHistory),
  };
}

function escapePrometheusLabelValue(value: string): string {
  return value
    .replace(/\\/g, '\\\\')
    .replace(/\n/g, '\\n')
    .replace(/"/g, '\\"');
}

function pushPrometheusGauge(
  lines: string[],
  name: string,
  value: unknown,
  labels?: Record<string, string>,
): void {
  const numeric =
    typeof value === 'number'
      ? value
      : typeof value === 'string'
        ? Number(value)
        : NaN;
  if (!Number.isFinite(numeric)) return;
  const renderedLabels =
    labels && Object.keys(labels).length
      ? `{${Object.entries(labels)
          .map(
            ([key, labelValue]) =>
              `${key}="${escapePrometheusLabelValue(labelValue)}"`,
          )
          .join(',')}}`
      : '';
  lines.push(`# TYPE ${name} gauge`);
  lines.push(`${name}${renderedLabels} ${numeric}`);
}

async function monitoringPrometheusText(
  onchain: OnchainContext | null,
): Promise<string> {
  const snapshot = await buildMonitoringResponse(onchain, false);
  const lines: string[] = [];

  pushPrometheusGauge(
    lines,
    'continuum_harness_monitoring_generated_timestamp_seconds',
    snapshot.generated_ts_ms / 1000,
  );
  pushPrometheusGauge(
    lines,
    'continuum_harness_monitoring_collection_interval_seconds',
    snapshot.collection_interval_ms / 1000,
  );
  if (snapshot.last_collection_ms != null) {
    pushPrometheusGauge(
      lines,
      'continuum_harness_monitoring_last_collection_timestamp_seconds',
      snapshot.last_collection_ms / 1000,
    );
  }

  for (const item of snapshot.markets) {
    const labels = {
      market: item.market,
      market_name: item.market_name,
    };

    pushPrometheusGauge(
      lines,
      'continuum_harness_monitoring_market_info',
      1,
      labels,
    );
    if (item.last_collected_ms != null) {
      pushPrometheusGauge(
        lines,
        'continuum_harness_monitoring_market_last_collected_timestamp_seconds',
        item.last_collected_ms / 1000,
        labels,
      );
    }
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_volume_24h_base_ui', item.volume_24h_base_ui, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_volume_24h_quote_ui', item.volume_24h_quote_ui, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_trade_count_24h', item.trade_count_24h, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_change_24h_pct', item.change_24h_pct, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_last_price_ui', item.last_price_ui, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_cumulative_volume_base_ui', item.cumulative_volume_base_ui, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_cumulative_volume_quote_ui', item.cumulative_volume_quote_ui, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_open_interest_base_ui', item.open_interest_base_ui, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_fees_accrued_native', item.fees_accrued_native, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_fees_settled_native', item.fees_settled_native, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_active_unique_users', item.active_unique_users, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_all_time_unique_users', item.all_time_unique_users, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_current_funding_hourly_pct', item.current_funding_hourly_pct, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_current_funding_daily_pct', item.current_funding_daily_pct, labels);
    pushPrometheusGauge(lines, 'continuum_harness_monitoring_current_oracle_price_ui', item.current_oracle_price_ui, labels);
  }

  return `${lines.join('\n')}\n`;
}

async function runOnchainBalanceSanityCheck(
  onchain: OnchainContext | null,
): Promise<void> {
  await withOnchainRead(onchain, 'run_onchain_balance_sanity_check', async () => {
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
      const onchainRow = ownerToOnchain.get(owner) || {
        mangoAccounts: [],
        usdcUiBalance: 0,
      };

      const harnessAccounts = [...harnessUser.mango_accounts].sort();
      const onchainAccounts = [...onchainRow.mangoAccounts].sort();
      const harnessUsdcUiBalance = 0; // queue replay does not yet model token collateral.
      const usdcDelta = onchainRow.usdcUiBalance - harnessUsdcUiBalance;
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
        onchainUsdcUiBalance: onchainRow.usdcUiBalance,
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
            onchain_usdc_ui_balance: onchainRow.usdcUiBalance.toString(),
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
  });
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
            applyLaneGuardAccepted(parseRelayIntentEvent(event), {
              source: 'replay_event_log',
            });
          } else if (event.event_type === 'relay_intent_status') {
            const parsed = parseRelayIntentStatusEvent(event);
            applyLaneGuardStatus(parsed, {
              source: 'replay_event_log',
            });
            engine.ingestRelayIntentStatus(parsed);
          } else if (event.event_type === 'queue_item_enqueued') {
            engine.ingestQueueEnqueued(event);
          } else if (event.event_type === 'queue_item_processed') {
            applyLaneGuardProcessed(event, {
              source: 'replay_event_log',
            });
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
  try {
    return await withOnchainRead(
      onchain,
      'enrich_owner_state_with_onchain',
      async () => {
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
      },
    );
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
  if (HARNESS_USES_V3_QUEUE) {
    return null;
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
  return await withOnchainRead(onchain, 'build_onchain_queue_snapshot', async () => {
    if (!onchain) {
      return null;
    }
    const executionQueuePk = await resolveExecutionQueuePk(onchain);
    if (!executionQueuePk) {
      return null;
    }
    if (!HARNESS_USES_V3_QUEUE && !onchain.groupPk) {
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
    if (HARNESS_USES_V3_QUEUE) {
      const root = decodeExecutionQueueV3MarketRoot(data);
      return {
        generated_ts_ms: Date.now(),
        observed_slot: queueAccount.context.slot,
        next_sequence: executionQueueV3NextEnqueueSequence(root).toString(),
        max_seen_sequence: root.maxSeenSequence.toString(),
        total_count: root.liveCount,
        // The harness only needs authoritative queue depth and sequence
        // watermarks here. Decoding per-page pending items for v3/v4 routes
        // is separate from the legacy flat-queue layout.
        items: [],
      };
    }
    const header = decodeExecutionQueueHeader(data);
    const group = onchain.groupPk!.toBase58();
    const items = decodeExecutionQueuePendingItems(data).map((item) => {
      // v2 sub-queue: items decoded from a per-market window carry the
      // market_index of their owning sub-queue. Pass it through to the engine
      // so the lookup hits the right (group, market, sequence, kind) key.
      const intent =
        item.section === 'ctm'
          ? engine.findIntent(
              group,
              item.sequence.toString(),
              item.kind,
              item.marketIndex ?? null,
            )
          : null;
      return {
        market:
          intent?.market ||
          (item.section === 'liquidity'
            ? 'liquidity'
            : item.marketIndex !== null
              ? String(item.marketIndex)
              : 'unknown'),
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
  });
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
  return await withOnchainRead(onchain, 'build_onchain_confirmed_snapshot', async () => {
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
  });
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
    await engine.bootstrapFromOnchainSnapshot(onchainSnapshot);
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
    await engine.bootstrapFromOnchainSnapshot(onchainState.snapshot);
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
  if (onchainSync.queue) {
    sweepLaneGuardFromOnchain(onchainSync.queue);
  }
}

// Sweep the lane guard after each periodic queue refresh: any tracked
// intent whose sequence is below the on-chain next-to-execute watermark
// and is no longer in the pending item list has been crankered on-chain.
// Mark it executed so the lane guard releases its pending slot immediately
// rather than waiting for the 8 s stale timeout.
function sweepLaneGuardFromOnchain(queue: OnchainQueueSnapshot): void {
  const nextSequence = maybeToBigInt(queue.next_sequence) || 0n;
  if (nextSequence === 0n) return;
  const pendingSeqKinds = new Set<string>();
  for (const item of queue.items) {
    if (item.section === 'ctm') {
      pendingSeqKinds.add(`${item.sequence}:${item.kind}`);
    }
  }
  const swept = laneGuard.sweepProcessedBelow(nextSequence, pendingSeqKinds);
  if (swept.length === 0) return;
  const nowMs = Date.now();
  for (const intent of swept) {
    const processedEvent: QueueItemProcessedEvent = {
      event_type: 'queue_item_processed',
      ts_ms: nowMs,
      group: intent.group,
      sequence: intent.sequence,
      kind: intent.kind,
      status: LOCAL_QUEUE_PROCESS_EXECUTED,
      slot: '0',
      tx_signature: `onchain_queue_sweep:${intent.sequence}:${intent.kind}`,
      market: intent.market || null,
      user_owner: intent.owner || null,
      mango_account: intent.mangoAccount || null,
    };
    if (HARNESS_BACKEND === 'rust-backend') {
      engine.ingestQueueProcessed(processedEvent);
    }
  }
}

function scheduleDirectOnchainRebase(
  onchain: OnchainContext | null,
  onchainSync: OnchainSyncState,
  context: Record<string, string>,
): void {
  if (!onchain?.groupPk) {
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
  try {
    return await withOnchainRead(onchain, 'build_airdrop_context', async () => {
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
    });
  } catch (err) {
    console.warn(`failed to build airdrop context: ${err}`);
    return null;
  }
}

async function buildOnchainContext(
  connection: Connection,
  programId: PublicKey,
): Promise<OnchainContext> {
  const groupPk = HARNESS_GROUP_PK.length
    ? new PublicKey(HARNESS_GROUP_PK)
    : null;
  const fallbackConnection =
    TRITON_RPC_URL.length > 0
      ? createReadConnection(TRITON_RPC_URL, TRITON_WS_URL || null)
      : null;
  let mangoClient: MangoClient | null = null;
  let cachedGroup: HarnessGroup | null = null;
  let usdcMint: PublicKey | null = HARNESS_USDC_MINT.length
    ? new PublicKey(HARNESS_USDC_MINT)
    : null;
  let executionQueuePk: PublicKey | null =
    process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
    process.env.EXECUTION_QUEUE_PK ||
    process.env.V4_QUEUE_ROOT
      ? new PublicKey(
          process.env.CONTINUUM_HARNESS_EXECUTION_QUEUE_PK ||
            process.env.EXECUTION_QUEUE_PK ||
            process.env.V4_QUEUE_ROOT ||
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
      if (!executionQueuePk && !HARNESS_USES_V3_QUEUE) {
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
    primaryConnection: connection,
    primaryRpcUrl: CLUSTER_URL || '',
    primaryWsUrl: CLUSTER_WS_URL || null,
    fallbackConnection,
    fallbackRpcUrl: TRITON_RPC_URL || null,
    fallbackWsUrl: TRITON_WS_URL || null,
    activeReadProvider: 'primary',
    primaryReadOutcomes: [],
    primaryWsUnexpectedResponses: [],
    lastReadProviderSwitchTsMs: null,
    lastReadProviderSwitchReason: null,
    lastReadError: null,
    lastReadErrorTsMs: null,
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
          market_index: decoded.market_index,
          sequence: decoded.sequence.toString(),
          kind: decoded.kind,
          min_execute_slot: decoded.min_execute_slot.toString(),
          slot: slot.toString(),
          tx_signature: logs.signature,
        });
      } else {
        const processedEvent = {
          event_type: 'queue_item_processed',
          ts_ms: Date.now(),
          group: decoded.group,
          market_index: decoded.market_index,
          sequence: decoded.sequence.toString(),
          kind: decoded.kind,
          status: decoded.status,
          slot: slot.toString(),
          tx_signature: logs.signature,
          processed_unix_ts: processedUnixTs,
        } as QueueItemProcessedEvent;
        applyLaneGuardProcessed(processedEvent, {
          source: 'program_logs',
          onchain,
          onchainSync,
        });
        engine.ingestQueueProcessed(processedEvent);
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
  if (!onchain?.groupPk) {
    throw new Error('deposit context unavailable: group/client not configured');
  }
  return await withOnchainRead(onchain, 'process_deposit_context_request', async () => {
    const ownerRaw = decodeURIComponent(
      url.pathname.split('/').pop() || '',
    ).trim();
    if (!ownerRaw.length) {
      throw new Error('owner is required');
    }
    const owner = new PublicKey(ownerRaw);
    const group = await getFreshGroup(onchain);
    if (!group || !onchain.mangoClient) {
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
  });
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
  if (!onchain?.groupPk) {
    throw new Error(
      'lane registration unavailable: group/client not configured',
    );
  }
  return await withOnchainRead(onchain, 'build_lane_for_owner', async () => {
    const owner = new PublicKey(ownerRaw);
    const group = await getFreshGroup(onchain);
    if (!group || !onchain.mangoClient) {
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
    const remainingAccounts =
      await onchain.mangoClient.buildExecutionQueueCanonicalPerpRemainingAccounts(
        group,
        mangoAccount,
        perpMarket.perpMarketIndex,
        owner,
      );

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
  });
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
  if (event.event_type === 'relay_intent_status') {
    const statusMarketIdx = parseMarketIndexHint(event.market);
    const intent =
      event.group && event.sequence !== null && event.kind !== null
        ? engine.findIntent(
            event.group,
            event.sequence,
            event.kind,
            statusMarketIdx,
          )
        : null;
    const tracked =
      event.group && event.sequence !== null && event.kind !== null
        ? laneGuard.findTrackedIntent(event.group, event.sequence, event.kind)
        : null;
    return {
      market: event.market || intent?.market || tracked?.market || null,
      owner: event.user_owner || intent?.user_owner || tracked?.owner || null,
      mangoAccount:
        event.mango_account || intent?.mango_account || tracked?.mangoAccount || null,
    };
  }
  if (
    event.event_type === 'queue_item_enqueued' ||
    event.event_type === 'queue_item_processed'
  ) {
    // v2 sub-queue events carry market_index directly; for processed events
    // the optional `market` string may also be set by the sequencer tick
    // builder. Either is enough to keep the lookup on the O(1) path.
    const queueMarketIdx =
      event.market_index ??
      ('market' in event ? parseMarketIndexHint(event.market) : null);
    const intent = engine.findIntent(
      event.group,
      event.sequence,
      event.kind,
      queueMarketIdx,
    );
    const tracked = laneGuard.findTrackedIntent(
      event.group,
      event.sequence,
      event.kind,
    );
    return {
      market: intent?.market || tracked?.market || null,
      owner: intent?.user_owner || tracked?.owner || null,
      mangoAccount: intent?.mango_account || tracked?.mangoAccount || null,
    };
  }
  return {
    market: null,
    owner: null,
    mangoAccount: null,
  };
}

function frontendSubscriberMatchesOptions(
  subscriber: FrontendStreamSubscriber,
  options: {
    owner?: string | null;
    mangoAccount?: string | null;
    market?: string | null;
    forceAll?: boolean;
  },
): boolean {
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
  if (!options.forceAll && !shouldConsiderOwner && !shouldConsiderMarket) {
    return false;
  }
  return true;
}

function sideFromDiscriminant(side: number): 'bid' | 'ask' {
  return side === 0 ? 'bid' : 'ask';
}

function buildFrontendPreconfirmIntent(
  event: RelayIntentAcceptedEvent,
): FrontendPreconfirmIntent {
  const payload = Buffer.from(event.payload_b64, 'base64');
  try {
    const decoded = decodeQueuePayload(payload);
    switch (decoded.variant) {
      case QueuePayloadVariantHarness.PerpPlaceOrderV2: {
        const side = sideFromDiscriminant(decoded.side);
        return {
          action: 'place_order',
          side,
          price_lots: decoded.price_lots.toString(),
          max_base_lots: decoded.max_base_lots.toString(),
          max_quote_lots: decoded.max_quote_lots.toString(),
          client_order_id: decoded.client_order_id.toString(),
          order_type: decoded.order_type,
          self_trade_behavior: decoded.self_trade_behavior,
          reduce_only: decoded.reduce_only,
          expiry_timestamp: decoded.expiry_timestamp.toString(),
          limit: decoded.limit,
          reserve_estimate: {
            base_lots: side === 'ask' ? decoded.max_base_lots.toString() : null,
            quote_lots: side === 'bid' ? decoded.max_quote_lots.toString() : null,
          },
        };
      }
      case QueuePayloadVariantHarness.PerpCancelOrder:
        return {
          action: 'cancel_order',
          order_id: decoded.order_id.toString(),
        };
      case QueuePayloadVariantHarness.PerpCancelOrderByClientOrderId:
        return {
          action: 'cancel_order_by_client_order_id',
          client_order_id: decoded.client_order_id.toString(),
        };
      case QueuePayloadVariantHarness.PerpCancelAllOrders:
        return {
          action: 'cancel_all_orders',
          limit: decoded.limit,
        };
      case QueuePayloadVariantHarness.PerpCancelAllOrdersBySide:
        return {
          action: 'cancel_all_orders_by_side',
          side:
            decoded.side_option === null
              ? 'all'
              : sideFromDiscriminant(decoded.side_option),
          limit: decoded.limit,
        };
      case QueuePayloadVariantHarness.LiquidityDeposit:
        return {
          action: 'liquidity_deposit',
          amount: decoded.amount.toString(),
          reduce_only: decoded.reduce_only,
        };
      case QueuePayloadVariantHarness.LiquidityWithdraw:
        return {
          action: 'liquidity_withdraw',
          amount: decoded.amount.toString(),
          allow_borrow: decoded.allow_borrow,
        };
      default:
        return {
          action: 'unknown',
          payload_bytes: payload.length,
        };
    }
  } catch {
    return {
      action: 'unknown',
      payload_bytes: payload.length,
    };
  }
}

function buildFrontendPreconfirmEvent(
  event: RelayIntentAcceptedEvent,
): FrontendPreconfirmEvent {
  const emittedTsMs = event.harness_preconfirm_emit_ts_ms || Date.now();
  return {
    phase: 'pre_confirmed',
    source: 'sequencer_ack',
    ts_ms: emittedTsMs,
    view: 'optimistic',
    accepted_source: event.accepted_source || null,
    fast_lane: !!event.fast_lane_preconfirm_emitted,
    sequencer_ingest_ts_ms: event.ts_ms || null,
    harness_accept_received_ts_ms: event.harness_accept_received_ts_ms || null,
    harness_preconfirm_emit_ts_ms: emittedTsMs,
    tracking_key: `${event.group}:${event.sequence}:${event.kind}`,
    request_id: event.request_id || null,
    group: event.group,
    execution_queue: event.execution_queue,
    market: event.market,
    sequence: event.sequence,
    kind: event.kind,
    owner: event.user_owner,
    mango_account: event.mango_account,
    enqueue_tx_signature: event.enqueue_tx_signature,
    min_execute_slot: event.min_execute_slot,
    expires_at_slot: event.expires_at_slot,
    intent: buildFrontendPreconfirmIntent(event),
  };
}

function subscriberShouldReceivePreconfirm(
  subscriber: FrontendStreamSubscriber,
  event: RelayIntentAcceptedEvent,
): boolean {
  if (subscriber.view !== 'optimistic' || !subscriber.include.has('pre_confirm')) {
    return false;
  }
  if (subscriber.owner && subscriber.owner !== event.user_owner) {
    return false;
  }
  if (subscriber.mangoAccount && subscriber.mangoAccount !== event.mango_account) {
    return false;
  }
  if (subscriber.market && subscriber.market !== event.market) {
    return false;
  }
  return true;
}

function hasLegacyAcceptedSnapshotSubscribers(options: {
  owner?: string | null;
  mangoAccount?: string | null;
  market?: string | null;
}): boolean {
  for (const subscriber of frontendStreamSubscribers.values()) {
    if (
      subscriber.view === 'optimistic' &&
      !subscriber.include.has('pre_confirm') &&
      frontendSubscriberMatchesOptions(subscriber, options)
    ) {
      return true;
    }
  }
  return false;
}

function notifyFrontendPreconfirmSubscribers(
  event: RelayIntentAcceptedEvent,
): void {
  if (!frontendStreamSubscribers.size) {
    return;
  }
  measureSync(
    'notify_frontend_preconfirm_subscribers',
    () => {
      const payload = buildFrontendPreconfirmEvent(event);
      for (const subscriber of Array.from(frontendStreamSubscribers.values())) {
        if (!subscriberShouldReceivePreconfirm(subscriber, event)) {
          continue;
        }
        if (
          !writeSseEvent(
            subscriber.res,
            'pre_confirm',
            payload,
            'frontend_stream_pre_confirm',
          )
        ) {
          frontendStreamSubscribers.delete(subscriber.id);
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_SSE_NOTIFY_MS,
      context: {
        subscribers: frontendStreamSubscribers.size,
        owner: event.user_owner,
        mango_account: event.mango_account,
        market: event.market,
      },
    },
  );
}

function buildFrontendValidatedLocalEvent(
  event: QueueItemProcessedEvent,
  opts: {
    includeOwnerState: boolean;
    includeMarketState: boolean;
    includeMarketOpenOrders: boolean;
  },
): FrontendValidatedLocalEvent | null {
  // v2 sub-queue: pass the market index hint so the rust-harness lookup
  // hits the per-market direct key instead of falling back to the O(N)
  // intent map scan.
  const marketIdx =
    event.market_index ?? parseMarketIndexHint(event.market ?? null);
  const payload = engine.getValidatedLocalPayload(
    event.group,
    event.sequence,
    event.kind,
    { ...opts, marketIndex: marketIdx },
  );
  if (!payload) {
    return null;
  }
  const emittedTsMs = Date.now();
  event.harness_validated_local_emit_ts_ms = emittedTsMs;
  return {
    phase: 'validated_local',
    source: 'sequencer_tick',
    ts_ms: event.ts_ms || Date.now(),
    harness_tick_received_ts_ms: event.harness_tick_received_ts_ms || null,
    harness_validated_local_emit_ts_ms: emittedTsMs,
    view: 'confirmed',
    tracking_key: payload.tracking_key,
    request_id: payload.request_id || null,
    group: payload.group,
    execution_queue: payload.execution_queue,
    market: payload.market,
    sequence: payload.sequence,
    kind: payload.kind,
    owner: payload.owner,
    mango_account: payload.mango_account,
    queue_process_status: event.status,
    queue_process_status_name: 'executed',
    validation_status: payload.validation_status,
    validation_error: payload.validation_error,
    tx_signature: event.tx_signature,
    processed_slot: event.slot,
    owner_state: payload.owner_state,
    market_state: payload.market_state,
  };
}

function subscriberShouldReceiveValidatedLocal(
  subscriber: FrontendStreamSubscriber,
  payload: FrontendValidatedLocalEvent,
): boolean {
  if (!subscriber.include.has('validated_local')) {
    return false;
  }
  if (subscriber.owner && subscriber.owner !== payload.owner) {
    return false;
  }
  if (
    subscriber.mangoAccount &&
    subscriber.mangoAccount !== payload.mango_account
  ) {
    return false;
  }
  if (subscriber.market && subscriber.market !== payload.market) {
    return false;
  }
  return true;
}

function hasLegacyValidatedLocalSnapshotSubscribers(options: {
  owner?: string | null;
  mangoAccount?: string | null;
  market?: string | null;
}): boolean {
  for (const subscriber of frontendStreamSubscribers.values()) {
    if (
      !subscriber.include.has('validated_local') &&
      frontendSubscriberMatchesOptions(subscriber, options)
    ) {
      return true;
    }
  }
  return false;
}

function notifyFrontendValidatedLocalSubscribers(
  event: QueueItemProcessedEvent,
): void {
  if (!frontendStreamSubscribers.size) {
    return;
  }
  measureSync(
    'notify_frontend_validated_local_subscribers',
    () => {
      const matchingSubscribers: FrontendStreamSubscriber[] = [];
      let includeOwnerState = false;
      let includeMarketState = false;
      let includeMarketOpenOrders = false;
      for (const subscriber of frontendStreamSubscribers.values()) {
        if (!subscriber.include.has('validated_local')) {
          continue;
        }
        if (subscriber.owner && subscriber.owner !== event.user_owner) {
          continue;
        }
        if (
          subscriber.mangoAccount &&
          subscriber.mangoAccount !== event.mango_account
        ) {
          continue;
        }
        if (subscriber.market && subscriber.market !== event.market) {
          continue;
        }
        matchingSubscribers.push(subscriber);
        includeOwnerState ||= !!subscriber.owner || !!subscriber.mangoAccount;
        includeMarketState ||= !!subscriber.market;
        includeMarketOpenOrders ||=
          !!subscriber.market && (!!subscriber.owner || !!subscriber.mangoAccount);
      }
      if (!matchingSubscribers.length) {
        return;
      }
      const payload = buildFrontendValidatedLocalEvent(event, {
        includeOwnerState,
        includeMarketState,
        includeMarketOpenOrders,
      });
      if (!payload) {
        return;
      }
      for (const subscriber of matchingSubscribers) {
        if (
          !writeSseEvent(
            subscriber.res,
            'validated_local',
            payload,
            'frontend_stream_validated_local',
          )
        ) {
          frontendStreamSubscribers.delete(subscriber.id);
        }
      }
    },
    {
      thresholdMs: HARNESS_SLOW_SSE_NOTIFY_MS,
      context: {
        subscribers: frontendStreamSubscribers.size,
        tx_signature: event.tx_signature,
        sequence: event.sequence,
      },
    },
  );
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
        if (!frontendSubscriberMatchesOptions(subscriber, options)) {
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
  if (intervalMs <= 0) {
    console.info(`periodic:${name} disabled (interval_ms=${intervalMs})`);
    return;
  }
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

// ── Market stats persistence helpers ────────────────────────────────────────

function loadMarketStats(): void {
  try {
    if (fs.existsSync(HARNESS_MARKET_STATS_PATH)) {
      const raw = fs.readFileSync(HARNESS_MARKET_STATS_PATH, 'utf-8');
      const parsed: unknown = JSON.parse(raw);
      if (
        parsed !== null &&
        typeof parsed === 'object' &&
        (parsed as MarketStatsStore).version === 1 &&
        (parsed as MarketStatsStore).markets
      ) {
        marketStatsStore = parsed as MarketStatsStore;
        console.log(
          `market_stats: loaded from ${HARNESS_MARKET_STATS_PATH} ` +
            `(${Object.keys(marketStatsStore.markets).length} markets)`,
        );
      }
    }
  } catch (err) {
    console.warn(`market_stats: failed to load ${HARNESS_MARKET_STATS_PATH}: ${err}`);
  }
}

function saveMarketStats(): void {
  try {
    marketStatsStore.written_ts_ms = Date.now();
    fs.mkdirSync(
      path.dirname(path.resolve(HARNESS_MARKET_STATS_PATH)),
      { recursive: true },
    );
    fs.writeFileSync(
      HARNESS_MARKET_STATS_PATH,
      JSON.stringify(marketStatsStore, null, 2),
      'utf-8',
    );
  } catch (err) {
    recordRuntimeError('market_stats_save', err);
  }
}

async function pollMarketStats(onchain: OnchainContext | null): Promise<void> {
  const now = Date.now();

  // Fetch fresh on-chain group (uses the shared cache; may trigger an RPC call).
  let group: HarnessGroup | null = null;
  try {
    group = await getFreshGroup(onchain);
  } catch (err) {
    recordRuntimeError('market_stats_fetch_group', err);
  }

  // Engine confirmed snapshot for OB state and user positions.
  const snapshot = engine.getSnapshot('confirmed');

  // Market metadata for lot-size conversions.
  const metadata = await getMarketMetadataMapSafe(onchain);

  // Build a market-index → perpMarket lookup from the on-chain group.
  const perpByIndex = new Map<number, (typeof group extends null ? never : InstanceType<any>)>();
  if (group) {
    for (const [idx, pm] of (group as HarnessGroup).perpMarketsMapByMarketIndex.entries()) {
      perpByIndex.set(Number(idx), pm);
    }
  }

  // Union of all known market keys (numeric index strings).
  const allMarketKeys = new Set([
    ...Object.keys(snapshot.markets),
    ...Array.from(perpByIndex.keys()).map(String),
  ]);

  for (const marketKey of allMarketKeys) {
    const marketIndexNum = Number(marketKey);
    const meta = metadata[marketKey] ?? null;
    const marketName = meta?.name ?? marketKey;

    // Get or initialise the per-market entry.
    const entry: PerMarketStats = marketStatsStore.markets[marketKey] ?? {
      market: marketKey,
      market_name: marketName,
      last_collected_ms: 0,
      cumulative_volume_base_lots: '0',
      cumulative_volume_quote_lots: '0',
      cumulative_volume_seq_hwm: '0',
      fees_accrued_native: null,
      fees_settled_native: null,
      open_interest_base_lots: null,
      open_interest_base_ui: null,
      funding_rate_history: [],
      all_time_unique_users: [],
    };
    // Back-fill the new field for entries loaded from an older stats file.
    if (!entry.cumulative_volume_seq_hwm) {
      entry.cumulative_volume_seq_hwm = '0';
    }

    if (meta?.name) {
      entry.market_name = meta.name;
    }

    // ── On-chain: OI, fees, funding rate ──────────────────────────────────
    const perpMarket = perpByIndex.get(marketIndexNum);
    if (perpMarket) {
      entry.open_interest_base_lots = perpMarket.openInterest.toString();
      entry.open_interest_base_ui = perpMarket.baseLotsToUi(perpMarket.openInterest);
      entry.fees_accrued_native = perpMarket.feesAccrued.toString();
      entry.fees_settled_native = perpMarket.feesSettled.toString();

      // Instantaneous funding rate derived from engine order-book + oracle.
      const engineMarketState = snapshot.markets[marketKey] ?? null;
      const bidImpactUi = getImpactPriceUiFromLevels(
        engineMarketState?.bids ?? [],
        perpMarket.impactQuantity,
        meta,
      );
      const askImpactUi = getImpactPriceUiFromLevels(
        engineMarketState?.asks ?? [],
        perpMarket.impactQuantity,
        meta,
      );
      let oraclePriceUi: number | null = null;
      try {
        oraclePriceUi = Number.isFinite(perpMarket.uiPrice) ? perpMarket.uiPrice : null;
      } catch { /* ignore */ }

      const dailyPct = computeFundingRateDailyPct(
        oraclePriceUi,
        bidImpactUi,
        askImpactUi,
        perpMarket.minFunding.toNumber(),
        perpMarket.maxFunding.toNumber(),
      );

      entry.funding_rate_history.push({
        ts_ms: now,
        hourly_pct: dailyPct !== null ? dailyPct / 24 : null,
        daily_pct: dailyPct,
        long_funding: perpMarket.longFunding.toString(),
        short_funding: perpMarket.shortFunding.toString(),
        oracle_price_ui: oraclePriceUi,
      });
      if (entry.funding_rate_history.length > HARNESS_FUNDING_HISTORY_MAX_ENTRIES) {
        entry.funding_rate_history = entry.funding_rate_history.slice(
          -HARNESS_FUNDING_HISTORY_MAX_ENTRIES,
        );
      }
    }

    // ── Cumulative volume accumulation ────────────────────────────────────
    // Use a taker_sequence high-water mark (persisted in the stats file) so
    // each fill is counted exactly once regardless of:
    //  - harness restarts (checkpoint loaded from file, no re-counting)
    //  - the 5000-trade ring buffer rotating old fills out (we track by
    //    sequence, not by summing the buffer, so buffer roll-over no longer
    //    stalls the accumulator)
    const trades = engine.getTrades(marketKey, 'confirmed', 100_000);
    const seqHwm = BigInt(entry.cumulative_volume_seq_hwm ?? '0');
    let newSeqHwm = seqHwm;
    let deltaBase = 0n;
    let deltaQuote = 0n;
    for (const t of trades) {
      const seq = BigInt(t.taker_sequence);
      if (seq > seqHwm) {
        deltaBase += BigInt(t.base_lots);
        deltaQuote += BigInt(t.quote_lots);
        if (seq > newSeqHwm) {
          newSeqHwm = seq;
        }
      }
    }
    if (deltaBase > 0n) {
      entry.cumulative_volume_base_lots = (
        BigInt(entry.cumulative_volume_base_lots) + deltaBase
      ).toString();
    }
    if (deltaQuote > 0n) {
      entry.cumulative_volume_quote_lots = (
        BigInt(entry.cumulative_volume_quote_lots) + deltaQuote
      ).toString();
    }
    entry.cumulative_volume_seq_hwm = newSeqHwm.toString();

    // ── Cumulative unique users ───────────────────────────────────────────
    const userSet = new Set(entry.all_time_unique_users);
    for (const [owner, user] of Object.entries(snapshot.users)) {
      if (user.per_market.some((p) => p.market === marketKey)) {
        userSet.add(owner);
      }
    }
    entry.all_time_unique_users = Array.from(userSet);

    entry.last_collected_ms = now;
    marketStatsStore.markets[marketKey] = entry;
  }

  saveMarketStats();
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
        const effectiveDrift = buildEffectiveReconciliationSnapshot(
          onchainSync.drift,
        );
        const cachedMetadataCount = onchain?.cachedMarketMetadata
          ? Object.keys(onchain.cachedMarketMetadata).length
          : 0;
        writeJson(res, 200, {
          ok: true,
          ...stats,
          onchain_read_enabled: !!onchain?.groupPk && !!onchain?.mangoClient,
          read_provider: getReadProviderDiagnostics(onchain),
          market_metadata_total: cachedMetadataCount,
          airdrop_enabled: !!airdrop,
          airdrop_deposit_enabled: !!airdrop?.groupPk && !!airdrop?.mangoClient,
          reconcile_interval_ms: Math.max(1000, HARNESS_RECONCILE_INTERVAL_MS),
          last_reconcile_ts_ms: effectiveDrift?.ts_ms || onchainSync.drift?.ts_ms || null,
          reconcile_markets_with_drift:
            effectiveDrift?.totals.markets_with_drift || 0,
          lane_guard: laneGuard.getStats(),
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
          data: buildEffectiveReconciliationSnapshot(onchainSync.drift),
          last_error: onchainSync.last_error,
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/diagnostics/lane-guard') {
        writeJson(res, 200, {
          stats: laneGuard.getStats(),
          markets: Array.from(laneGuard.getSuppressedMarkets().values()),
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/diagnostics/read-provider') {
        writeJson(res, 200, {
          data: getReadProviderDiagnostics(onchain),
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
        const payload = parseRelayIngestEvent(JSON.parse(body));
        if (payload.event_type === 'relay_intent_status') {
          applyLaneGuardStatus(payload, {
            source: 'relay_ingest_http',
            onchain,
            onchainSync,
          });
          engine.ingestRelayIntentStatus(payload);
        } else {
          applyLaneGuardAccepted(payload, {
            source: 'relay_ingest_http',
            persistSuppressedEvent: true,
            onchain,
            onchainSync,
          });
        }
        writeJson(res, 202, {
          ok: true,
          key:
            payload.event_type === 'relay_intent_status'
              ? `${payload.request_id}:${payload.status_code}`
              : `${payload.group}:${payload.sequence}:${payload.kind}`,
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
            ? [
                'positions',
                'open_orders',
                'trades',
                'account_metrics',
                'pre_confirm',
                'validated_local',
              ]
            : []),
          ...(market
            ? [
                'market_metrics',
                'trade_summary',
                'orderbook_summary',
                'pre_confirm',
                'validated_local',
              ]
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
          supported_events:
            [
              'snapshot',
              'account_update',
              'market_update',
              ...(view === 'optimistic' && include.has('pre_confirm')
                ? ['pre_confirm']
                : []),
              ...(include.has('validated_local') ? ['validated_local'] : []),
            ],
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
        // Reject empty/undefined/non-numeric market index with a clear 400.
        // This prevents callers (relayer margin precheck, frontends) from
        // wasting work on a malformed request.
        if (!market || market === 'undefined' || market === 'null') {
          writeJson(res, 400, {
            error: 'invalid_market_index',
            message: `market index missing or undefined: '${market}'`,
          });
          return;
        }
        if (!/^[0-9]+$/.test(market) || Number(market) > 65535) {
          writeJson(res, 400, {
            error: 'invalid_market_index',
            message: `market index must be a non-negative integer <= 65535, got '${market}'`,
          });
          return;
        }
        const view = parseView(url);
        const marketMetadata = await getMarketMetadataMapSafe(onchain);
        const metadataOnly =
          (
            url.searchParams.get('metadata_only') || 'false'
          ).toLowerCase() === 'true';
        if (metadataOnly) {
          writeJson(res, 200, {
            view,
            metadata: marketMetadata[market] || null,
          });
          return;
        }
        const snapshot = getSnapshotForView(view, onchainSync);
        const responseView = snapshot.view;
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
        // v5 per-market PDA: opt-in live read via ?source=onchain_v5.
        // Default keeps the legacy replay-engine shape so existing callers
        // don't regress. `onchain_v5` bypasses the replay model and reads
        // the per-market queue PDA directly — the correct truth after the
        // per-market-queue migration.
        const source = (url.searchParams.get('source') || 'engine').toLowerCase();
        if (source === 'onchain_v5') {
          const marketIndex = Number(market);
          if (
            !Number.isFinite(marketIndex) ||
            marketIndex < 0 ||
            marketIndex > 65535
          ) {
            writeJson(res, 400, {
              error: 'invalid_market_index',
              message: `market index must be 0..65535, got '${market}'`,
            });
            return;
          }
          if (!onchain || !onchain.groupPk || !onchain.programId) {
            writeJson(res, 503, {
              error: 'onchain_unavailable',
              message: 'onchain context not initialised',
            });
            return;
          }
          try {
            const live = await fetchV5QueueLiveState(
              onchain.connection,
              onchain.programId,
              onchain.groupPk,
              marketIndex,
              HARNESS_COMMITMENT,
            );
            if (!live) {
              writeJson(res, 404, {
                source: 'onchain_v5',
                market,
                message: 'per-market v5 queue PDA not found on-chain',
                pda: deriveV5QueuePda(
                  onchain.programId,
                  onchain.groupPk,
                  marketIndex,
                ).toBase58(),
              });
              return;
            }
            writeJson(res, 200, {
              source: 'onchain_v5',
              market,
              data: live,
            });
            return;
          } catch (err) {
            writeJson(res, 502, {
              source: 'onchain_v5',
              market,
              error: 'rpc_read_failed',
              message: normalizeError(err).message,
            });
            return;
          }
        }
        writeJson(res, 200, {
          view,
          market,
          data: snapshot.queue[market] || emptyQueueState(market),
        });
        return;
      }

      // Fresh per-market orderbook — always live-reads from on-chain,
      // bypassing the replay engine. Use when the reconciled snapshot is
      // too stale (e.g. between HARNESS_RECONCILE_INTERVAL_MS ticks).
      // Query params: depth=N (default 20, max 500).
      if (method === 'GET' && url.pathname.startsWith('/state/book/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/book/'.length),
        );
        const marketIndex = Number(market);
        if (
          !Number.isFinite(marketIndex) ||
          marketIndex < 0 ||
          marketIndex > 65535
        ) {
          writeJson(res, 400, {
            error: 'invalid_market_index',
            message: `market index must be 0..65535, got '${market}'`,
          });
          return;
        }
        if (!onchain) {
          writeJson(res, 503, {
            error: 'onchain_unavailable',
            message: 'onchain context not initialised',
          });
          return;
        }
        const depth = parseNonNegativeInteger(
          url.searchParams.get('depth'),
          20,
          { min: 1, max: 500 },
        );
        try {
          const book = await fetchFreshBook(onchain, marketIndex, depth);
          if (!book) {
            writeJson(res, 404, {
              market,
              message:
                'market not found in mango group (check V5_M<idx>_PERP_MARKET config)',
            });
            return;
          }
          writeJson(res, 200, { market, data: book });
          return;
        } catch (err) {
          writeJson(res, 502, {
            market,
            error: 'rpc_read_failed',
            message: normalizeError(err).message,
          });
          return;
        }
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

      if (method === 'GET' && url.pathname === '/monitoring') {
        const fullHistory = url.searchParams.get('full_history') === 'true';
        writeJson(res, 200, await buildMonitoringResponse(onchain, fullHistory));
        return;
      }

      if (method === 'GET' && url.pathname === '/monitoring-prometheus') {
        writeText(res, 200, await monitoringPrometheusText(onchain));
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

  const connection = createReadConnection(
    CLUSTER_URL,
    CLUSTER_WS_URL || null,
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];

  loadMarketStats();
  replayEventLogIfPresent();
  await measureAsync('startup.backfill_program_logs', async () => {
    await maybeBackfillProgramLogs(connection, programId);
  });
  const onchain = await measureAsync(
    'startup.build_onchain_context',
    async () => await buildOnchainContext(connection, programId),
  );
  attachRpcWebSocketDiagnostics(
    onchain.primaryConnection,
    inferRpcProviderLabel(CLUSTER_URL, 'primary'),
    CLUSTER_URL,
    CLUSTER_WS_URL || null,
    (details) => {
      notePrimaryWsUnexpectedResponse(
        onchain,
        details.statusCode,
        details.statusMessage,
      );
    },
  );
  if (onchain.fallbackConnection) {
    attachRpcWebSocketDiagnostics(
      onchain.fallbackConnection,
      inferRpcProviderLabel(onchain.fallbackRpcUrl || '', 'fallback'),
      onchain.fallbackRpcUrl || '',
      onchain.fallbackWsUrl,
    );
  }
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
        const snapshot = bootstrappedOnchainState.snapshot;
        await measureAsync('startup.bootstrap_from_onchain_snapshot', async () => {
          await engine.bootstrapFromOnchainSnapshot(snapshot);
        });
        const userCount = Object.keys(snapshot.users).length;
        const marketCount = Object.keys(snapshot.markets).length;
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
      const isLocalValidatedEvent =
        event.event_type === 'queue_item_processed' &&
        event.status === LOCAL_QUEUE_PROCESS_EXECUTED &&
        isSequencerLocalProcessedSignature(event.tx_signature);
      if (
        event.event_type === 'relay_intent_accepted' &&
        !hasFastPreconfirm(event)
      ) {
        notifyFrontendPreconfirmSubscribers(event);
      }
      if (isLocalValidatedEvent) {
        notifyFrontendValidatedLocalSubscribers(event);
      }
      appendEventLog(event);
      appendTxnLog(event);
      broadcastEvent(event);
      const context = deriveEventContext(event);
      if (event.event_type !== 'relay_intent_accepted') {
        scheduleTradeStreamNotification(event);
      }
      if (
        (event.event_type !== 'relay_intent_accepted' ||
          hasLegacyAcceptedSnapshotSubscribers({
            owner: context.owner,
            mangoAccount: context.mangoAccount,
            market: context.market,
          })) &&
        (!isLocalValidatedEvent ||
          hasLegacyValidatedLocalSnapshotSubscribers({
            owner: context.owner,
            mangoAccount: context.mangoAccount,
            market: context.market,
          }))
      ) {
        scheduleFrontendStreamNotification(onchain, onchainSync, {
          owner: context.owner,
          mangoAccount: context.mangoAccount,
          market: context.market,
        });
      }
    } catch (err) {
      recordRuntimeError('engine_subscriber', err, {
        event_type: event.event_type,
      });
    }
  });

  startContinuumSequencerAcceptedIngest();
  startContinuumSequencerTickIngest();
  console.info(
    `Continuum sequencer ingest transports: accepted=${activeSequencerAcceptedTransport}` +
      `${activeSequencerAcceptedStreamUrl ? `(${activeSequencerAcceptedStreamUrl})` : ''}, ` +
      `tick=${activeSequencerTickTransport}` +
      `${activeSequencerTickStreamUrl ? `(${activeSequencerTickStreamUrl})` : ''}`,
  );

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
    'market_stats',
    HARNESS_MARKET_STATS_INTERVAL_MS,
    async () => {
      await pollMarketStats(onchain);
    },
  );

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

  // Lightweight queue-state refresh — keeps last_processed_sequence current
  // even when the WebSocket log subscription misses QueueItemEnqueued events.
  // Reads one account only; no getAllMangoAccounts().
  if (HARNESS_QUEUE_REFRESH_INTERVAL_MS > 0 && onchain) {
    startPeriodicTask(
      'onchain_queue_refresh',
      HARNESS_QUEUE_REFRESH_INTERVAL_MS,
      async () => {
        await refreshOnchainQueueState(onchain, onchainSync);
      },
      { runImmediately: false },
    );
  }

  startPeriodicTask(
    'frontend_market_refresh',
    HARNESS_FRONTEND_REFRESH_INTERVAL_MS,
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
