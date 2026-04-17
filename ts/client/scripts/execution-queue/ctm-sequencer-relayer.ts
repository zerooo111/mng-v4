import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Cluster,
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import crypto from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import nacl from 'tweetnacl';
import { MANGO_V4_ID } from '../../src/constants';
import {
  buildIntentEd25519Instruction,
  buildExecutionQueueEnqueueCtmWithIntentIxs,
  IntentSigner,
} from '../../src/executionQueue';
import {
  fetchLatestBlockHash,
  LatestBlockhash,
  sendTransaction,
} from '../../src/utils/rpc';

dotenv.config();

const WS_HANDSHAKE_405 = 'Unexpected server response: 405';

function shouldIgnoreBackgroundWsHandshakeError(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : `${err}`;
  return msg.includes(WS_HANDSHAKE_405);
}

process.on('uncaughtException', (err) => {
  if (shouldIgnoreBackgroundWsHandshakeError(err)) {
    console.error(
      `ignoring background websocket handshake error: ${err instanceof Error ? err.message : err}`,
    );
    return;
  }
  console.error(err);
  process.exit(1);
});

process.on('unhandledRejection', (err) => {
  if (shouldIgnoreBackgroundWsHandshakeError(err)) {
    console.error(
      `ignoring background websocket rejection: ${err instanceof Error ? err.message : err}`,
    );
    return;
  }
  console.error('unhandled rejection in relayer:', err);
});

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const CLUSTER_WS_URL =
  process.env.CLUSTER_WS_URL_OVERRIDE || process.env.MB_CLUSTER_WS_URL || '';
const RELAYER_BIND_ADDR = process.env.CTM_RELAYER_BIND_ADDR || '0.0.0.0:9090';
const RELAYER_PAYER_KEYPAIR =
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  process.env.USER_KEYPAIR_OVERRIDE ||
  process.env.MB_PAYER_KEYPAIR;
const RELAYER_CTM_KEYPAIR =
  process.env.CTM_RELAYER_CTM_KEYPAIR || RELAYER_PAYER_KEYPAIR;
const RELAYER_SEQUENCE_STATE_PATH =
  process.env.CTM_RELAYER_SEQUENCE_STATE_PATH || '/tmp/ctm-sequences.json';
const RELAYER_DEFAULT_MIN_EXECUTE_SLOT_OFFSET = BigInt(
  process.env.CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET ?? '20',
);
const RELAYER_DEFAULT_EXPIRES_AT_SLOT = BigInt(
  process.env.CTM_RELAYER_DEFAULT_EXPIRES_AT_SLOT ?? '0',
);
const EXECUTION_QUEUE_BUFFER_PK = process.env.EXECUTION_QUEUE_BUFFER_PK;
const RELAYER_HARNESS_BASE_URL = (
  process.env.CTM_RELAYER_HARNESS_BASE_URL || ''
).trim().replace(/\/+$/, '');

function parseBoolEnv(raw: string | undefined, defaultValue: boolean): boolean {
  if (raw === undefined) {
    return defaultValue;
  }
  return raw === '1' || raw.toLowerCase() === 'true';
}

function normalizeBaseUrl(raw: string | undefined): string {
  return (raw || '').trim().replace(/\/+$/, '');
}

function resolveRelayerEventSinkUrl(): string {
  const explicit = normalizeBaseUrl(process.env.CTM_RELAYER_EVENT_SINK_URL);
  if (explicit) {
    return explicit;
  }

  const fanoutMode = (
    process.env.CTM_FANOUT_MODE ||
    (parseBoolEnv(process.env.CTM_FANOUT_ENABLED, false) ? 'local' : 'disabled')
  )
    .trim()
    .toLowerCase();

  if (
    fanoutMode === 'local' ||
    fanoutMode === 'external' ||
    fanoutMode === 'gateway'
  ) {
    const fanoutBaseUrl =
      normalizeBaseUrl(process.env.CTM_FANOUT_BASE_URL) ||
      'http://127.0.0.1:9094';
    return `${fanoutBaseUrl}/ingest`;
  }

  return RELAYER_HARNESS_BASE_URL
    ? `${RELAYER_HARNESS_BASE_URL}/ingest/relay-intent`
    : '';
}

const RELAYER_EVENT_SINK_URL = resolveRelayerEventSinkUrl();
const RELAYER_EVENT_SINK_AUTH_TOKEN =
  process.env.CTM_RELAYER_EVENT_SINK_AUTH_TOKEN || '';
const RELAYER_PRIORITIZATION_FEE = Number(
  process.env.CTM_RELAYER_PRIORITIZATION_FEE ?? '0',
);
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;
// Default to serialized sequence assignment so failed submits do not burn
// sequence numbers and create artificial queue gaps.
const RELAYER_SERIALIZE_SUBMITS =
  (process.env.CTM_RELAYER_SERIALIZE_SUBMITS || 'true') === 'true';
const RELAYER_CONFIRM_IN_BACKGROUND =
  (process.env.CTM_RELAYER_CONFIRM_IN_BACKGROUND || 'false') === 'true';
const RELAYER_BLOCKHASH_CACHE_MS = Number(
  process.env.CTM_RELAYER_BLOCKHASH_CACHE_MS ?? '500',
);
const RELAYER_POST_SEND_STATUS_TIMEOUT_MS = Number(
  process.env.CTM_RELAYER_POST_SEND_STATUS_TIMEOUT_MS ?? '15000',
);
const RELAYER_POST_SEND_STATUS_POLL_MS = Number(
  process.env.CTM_RELAYER_POST_SEND_STATUS_POLL_MS ?? '200',
);
const RELAYER_SUBMIT_MODE_RAW = (
  process.env.CTM_RELAYER_SUBMIT_MODE || 'strict'
).toLowerCase();
const RELAYER_SUBMIT_MODE =
  RELAYER_SUBMIT_MODE_RAW === 'fast' ? 'fast' : 'strict';
const RELAYER_MAX_INFLIGHT = Number(process.env.CTM_RELAYER_MAX_INFLIGHT ?? '24');
const RELAYER_MAX_QUEUED = Number(process.env.CTM_RELAYER_MAX_QUEUED ?? '96');
const RELAYER_QUEUE_WAIT_TIMEOUT_MS = Number(
  process.env.CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS ?? '500',
);
const RELAYER_QUEUE_FULL_WATERMARK = Number(
  process.env.CTM_RELAYER_QUEUE_FULL_WATERMARK ?? '990',
);
const RELAYER_QUEUE_COUNT_CACHE_MS = Number(
  process.env.CTM_RELAYER_QUEUE_COUNT_CACHE_MS ?? '200',
);
const RELAYER_VERIFY_USER_SIGNATURE =
  (process.env.CTM_RELAYER_VERIFY_USER_SIGNATURE || 'true') === 'true';
const RELAYER_PROFILE_LOG_EVERY = Number(
  process.env.CTM_RELAYER_PROFILE_LOG_EVERY ?? '100',
);

type RelayerProfileTotals = {
  requests: number;
  totalMs: number;
  gateMs: number;
  queueCheckMs: number;
  blockhashMs: number;
  buildMs: number;
  sendMs: number;
  maxTotalMs: number;
};

type SequenceState = Record<string, string>;

type SubmitIntentRequest = {
  group: string;
  execution_queue: string;
  market: string;
  payload: Buffer;
  remaining_accounts: Array<{
    pubkey: string;
    is_signer: boolean;
    is_writable: boolean;
  }>;
  min_execute_slot: string;
  expires_at_slot: string;
  user_owner: string;
  mango_account: string;
  user_signature: Buffer;
  base_fee?: string;
  intent_version?: number;
  target_kind?: number;
  target_index?: number;
};

type SubmitIntentResponse = {
  sequence: string;
  tx_signature: string;
  user_intent_message: Buffer;
  ctm_envelope_message: Buffer;
};

class RelayerError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

class InflightGate {
  private active = 0;
  private readonly waiters: Array<() => void> = [];

  constructor(
    private readonly maxInflight: number,
    private readonly maxQueued: number,
  ) {}

  private tryAcquire(): boolean {
    if (this.active >= this.maxInflight) {
      return false;
    }
    this.active += 1;
    return true;
  }

  release(): void {
    this.active = Math.max(0, this.active - 1);
    const next = this.waiters.shift();
    if (next) {
      next();
    }
  }

  async acquireWithTimeout(timeoutMs: number): Promise<void> {
    if (this.tryAcquire()) {
      return;
    }
    if (this.waiters.length >= this.maxQueued) {
      throw new RelayerError(
        grpc.status.RESOURCE_EXHAUSTED,
        `relayer saturated: active=${this.active}, queued=${this.waiters.length}, maxQueued=${this.maxQueued}`,
      );
    }

    await new Promise<void>((resolve, reject) => {
      let done = false;
      const wake = () => {
        if (done) {
          return;
        }
        if (this.tryAcquire()) {
          done = true;
          clearTimeout(timer);
          resolve();
          return;
        }
        this.waiters.push(wake);
      };
      const timer = setTimeout(() => {
        if (done) {
          return;
        }
        done = true;
        const idx = this.waiters.indexOf(wake);
        if (idx >= 0) {
          this.waiters.splice(idx, 1);
        }
        reject(
          new RelayerError(
            grpc.status.RESOURCE_EXHAUSTED,
            `relayer queue timeout after ${timeoutMs}ms: active=${this.active}, queued=${this.waiters.length}`,
          ),
        );
      }, timeoutMs);
      this.waiters.push(wake);
    });
  }
}

class SequenceStore {
  private readonly sequences: Map<string, bigint>;
  private pending: Promise<void> = Promise.resolve();
  private flushTimer: NodeJS.Timeout | null = null;
  private dirty = false;

  constructor(private readonly statePath: string) {
    this.sequences = this.load();
  }

  private load(): Map<string, bigint> {
    if (!fs.existsSync(this.statePath)) {
      return new Map();
    }
    const data = JSON.parse(fs.readFileSync(this.statePath, 'utf-8')) as SequenceState;
    const out = new Map<string, bigint>();
    for (const [k, v] of Object.entries(data)) {
      out.set(k, BigInt(v));
    }
    return out;
  }

  async withNextSequence<T>(
    key: string,
    submit: (sequence: bigint) => Promise<T>,
  ): Promise<{ sequence: bigint; value: T }> {
    const waitFor = this.pending;
    let releasePending!: () => void;
    this.pending = new Promise<void>((resolve) => {
      releasePending = resolve;
    });
    await waitFor;

    try {
      const sequence = this.sequences.get(key) ?? 0n;
      const value = await submit(sequence);
      this.sequences.set(key, sequence + 1n);
      this.schedulePersist();
      return { sequence, value };
    } finally {
      releasePending();
    }
  }

  reserveNextSequenceSync(key: string): bigint {
    const sequence = this.sequences.get(key) ?? 0n;
    this.sequences.set(key, sequence + 1n);
    this.schedulePersist();
    return sequence;
  }

  flushNow(): void {
    if (!this.dirty) {
      return;
    }
    if (this.flushTimer) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    this.persist();
  }

  private schedulePersist(): void {
    this.dirty = true;
    if (this.flushTimer) {
      return;
    }
    this.flushTimer = setTimeout(() => {
      this.flushTimer = null;
      this.persist();
    }, 250);
  }

  private persist(): void {
    this.dirty = false;
    const dir = path.dirname(this.statePath);
    fs.mkdirSync(dir, { recursive: true });
    const serialized: SequenceState = {};
    for (const [k, v] of this.sequences.entries()) {
      serialized[k] = v.toString();
    }
    fs.writeFileSync(this.statePath, JSON.stringify(serialized, null, 2));
  }
}

const relayerProfile: RelayerProfileTotals = {
  requests: 0,
  totalMs: 0,
  gateMs: 0,
  queueCheckMs: 0,
  blockhashMs: 0,
  buildMs: 0,
  sendMs: 0,
  maxTotalMs: 0,
};

function nowMs(): number {
  return Number(process.hrtime.bigint()) / 1_000_000;
}

function recordProfile(sample: {
  totalMs: number;
  gateMs: number;
  queueCheckMs: number;
  blockhashMs: number;
  buildMs: number;
  sendMs: number;
}): void {
  if (!Number.isFinite(RELAYER_PROFILE_LOG_EVERY) || RELAYER_PROFILE_LOG_EVERY <= 0) {
    return;
  }
  relayerProfile.requests += 1;
  relayerProfile.totalMs += sample.totalMs;
  relayerProfile.gateMs += sample.gateMs;
  relayerProfile.queueCheckMs += sample.queueCheckMs;
  relayerProfile.blockhashMs += sample.blockhashMs;
  relayerProfile.buildMs += sample.buildMs;
  relayerProfile.sendMs += sample.sendMs;
  relayerProfile.maxTotalMs = Math.max(relayerProfile.maxTotalMs, sample.totalMs);

  if (relayerProfile.requests % RELAYER_PROFILE_LOG_EVERY !== 0) {
    return;
  }

  const count = relayerProfile.requests;
  console.error(
    JSON.stringify({
      msg: 'relayer-profile',
      requests: count,
      avgTotalMs: relayerProfile.totalMs / count,
      avgGateMs: relayerProfile.gateMs / count,
      avgQueueCheckMs: relayerProfile.queueCheckMs / count,
      avgBlockhashMs: relayerProfile.blockhashMs / count,
      avgBuildMs: relayerProfile.buildMs / count,
      avgSendMs: relayerProfile.sendMs / count,
      maxTotalMs: relayerProfile.maxTotalMs,
    }),
  );
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parseU64(input: string | number | undefined | null): bigint {
  if (input === undefined || input === null) {
    return 0n;
  }
  if (typeof input === 'number') {
    return BigInt(input);
  }
  return input.length ? BigInt(input) : 0n;
}

function parseRemainingAccounts(
  remainingAccounts: SubmitIntentRequest['remaining_accounts'],
): AccountMeta[] {
  return (remainingAccounts ?? []).map((a) => ({
    pubkey: cachedPublicKey(a.pubkey),
    isSigner: !!a.is_signer,
    isWritable: !!a.is_writable,
  }));
}

const publicKeyCache = new Map<string, PublicKey>();
function cachedPublicKey(value: string): PublicKey {
  const cached = publicKeyCache.get(value);
  if (cached) {
    return cached;
  }
  const parsed = new PublicKey(value);
  publicKeyCache.set(value, parsed);
  return parsed;
}

const remainingAccountsCache = new Map<string, AccountMeta[]>();
function cachedRemainingAccounts(
  remainingAccounts: SubmitIntentRequest['remaining_accounts'],
): AccountMeta[] {
  const key = JSON.stringify(remainingAccounts ?? []);
  const cached = remainingAccountsCache.get(key);
  if (cached) {
    return cached;
  }
  const parsed = parseRemainingAccounts(remainingAccounts);
  if (remainingAccountsCache.size >= 256) {
    const firstKey = remainingAccountsCache.keys().next().value;
    if (firstKey) {
      remainingAccountsCache.delete(firstKey);
    }
  }
  remainingAccountsCache.set(key, parsed);
  return parsed;
}

function toHexUtf8IntentMessage(intentMessage: Uint8Array): Buffer {
  return Buffer.from(Buffer.from(intentMessage).toString('hex'), 'utf-8');
}

function deriveWsEndpoint(httpUrl: string): string | null {
  try {
    const parsed = new URL(httpUrl);
    const protocol = parsed.protocol === 'https:' ? 'wss:' : 'ws:';
    let port = parsed.port;
    if (
      (parsed.hostname === '127.0.0.1' || parsed.hostname === 'localhost') &&
      parsed.port === '8899'
    ) {
      port = '8900';
    }
    const host = port.length ? `${parsed.hostname}:${port}` : parsed.hostname;
    return `${protocol}//${host}${parsed.pathname || ''}`;
  } catch {
    return null;
  }
}

async function maybeEmitRelayIntentAccepted(event: {
  ts_ms: number;
  request_id: string;
  group: string;
  execution_queue: string;
  market: string;
  sequence: string;
  kind: number;
  payload_b64: string;
  remaining_accounts: SubmitIntentRequest['remaining_accounts'];
  min_execute_slot: string;
  expires_at_slot: string;
  user_owner: string;
  mango_account: string;
  enqueue_tx_signature: string;
}): Promise<void> {
  await maybeEmitRelayEvent({
    event_type: 'relay_intent_accepted',
    ...event,
  });
}

async function maybeEmitRelayIntentStatus(event: {
  ts_ms: number;
  request_id: string;
  status_code: number;
  status_label: string;
  reason: string | null;
  group: string | null;
  execution_queue: string | null;
  market: string | null;
  sequence: string | null;
  kind: number | null;
  user_owner: string | null;
  mango_account: string | null;
  tx_signature: string | null;
  grpc_code: number | null;
  queue_process_status: number | null;
  queue_process_status_name: string | null;
}): Promise<void> {
  const payload = {
    event_type: 'relay_intent_status',
    ...event,
  };
  console.log(JSON.stringify(payload));
  await maybeEmitRelayEvent(payload);
}

async function maybeEmitRelayEvent(event: Record<string, unknown>): Promise<void> {
  if (!RELAYER_EVENT_SINK_URL) {
    return;
  }

  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };
  if (RELAYER_EVENT_SINK_AUTH_TOKEN.length) {
    headers.Authorization = `Bearer ${RELAYER_EVENT_SINK_AUTH_TOKEN}`;
  }

  const body = JSON.stringify(event);

  try {
    const response = await fetch(RELAYER_EVENT_SINK_URL, {
      method: 'POST',
      headers,
      body,
    });
    if (!response.ok) {
      const text = await response.text();
      console.error(
        `relay event sink failed: status=${response.status}, body=${text.slice(0, 256)}`,
      );
    }
  } catch (err) {
    console.error('relay event sink request failed:', err);
  }
}

async function waitForProcessedSignature(params: {
  connection: Connection;
  signature: string;
  timeoutMs: number;
  pollMs: number;
}): Promise<void> {
  const started = Date.now();
  while (Date.now() - started < params.timeoutMs) {
    const statuses = await params.connection.getSignatureStatuses([params.signature]);
    const status = statuses.value[0];
    if (!status) {
      await new Promise((resolve) => setTimeout(resolve, params.pollMs));
      continue;
    }
    if (status.err) {
      throw new Error(
        `relay submit landed with error for ${params.signature}: ${JSON.stringify(status.err)}`,
      );
    }
    return;
  }
  throw new Error(
    `relay submit status timeout for ${params.signature} after ${params.timeoutMs}ms`,
  );
}

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }
  if (!RELAYER_PAYER_KEYPAIR) {
    throw new Error('CTM_RELAYER_PAYER_KEYPAIR (or MB_PAYER_KEYPAIR) is required');
  }
  if (!RELAYER_CTM_KEYPAIR) {
    throw new Error('CTM_RELAYER_CTM_KEYPAIR is required');
  }
  if (!Number.isFinite(RELAYER_BLOCKHASH_CACHE_MS) || RELAYER_BLOCKHASH_CACHE_MS < 0) {
    throw new Error('CTM_RELAYER_BLOCKHASH_CACHE_MS must be >= 0');
  }
  if (!Number.isFinite(RELAYER_MAX_INFLIGHT) || RELAYER_MAX_INFLIGHT <= 0) {
    throw new Error('CTM_RELAYER_MAX_INFLIGHT must be > 0');
  }
  if (!Number.isFinite(RELAYER_MAX_QUEUED) || RELAYER_MAX_QUEUED < 0) {
    throw new Error('CTM_RELAYER_MAX_QUEUED must be >= 0');
  }
  if (
    !Number.isFinite(RELAYER_QUEUE_WAIT_TIMEOUT_MS) ||
    RELAYER_QUEUE_WAIT_TIMEOUT_MS < 0
  ) {
    throw new Error('CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS must be >= 0');
  }
  if (
    !Number.isFinite(RELAYER_QUEUE_FULL_WATERMARK) ||
    RELAYER_QUEUE_FULL_WATERMARK < 0
  ) {
    throw new Error('CTM_RELAYER_QUEUE_FULL_WATERMARK must be >= 0');
  }
  if (!Number.isFinite(RELAYER_QUEUE_COUNT_CACHE_MS) || RELAYER_QUEUE_COUNT_CACHE_MS < 0) {
    throw new Error('CTM_RELAYER_QUEUE_COUNT_CACHE_MS must be >= 0');
  }

  const payer = readKeypair(RELAYER_PAYER_KEYPAIR);
  const ctm = readKeypair(RELAYER_CTM_KEYPAIR);
  const configuredExecutionQueueBuffer = EXECUTION_QUEUE_BUFFER_PK
    ? new PublicKey(EXECUTION_QUEUE_BUFFER_PK)
    : null;
  const wsEndpoint = CLUSTER_WS_URL || deriveWsEndpoint(CLUSTER_URL) || undefined;
  const connection = new Connection(CLUSTER_URL, {
    ...AnchorProvider.defaultOptions(),
    wsEndpoint,
  });
  const provider = new AnchorProvider(
    connection,
    new Wallet(payer),
    AnchorProvider.defaultOptions(),
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];
  const sequenceStore = new SequenceStore(RELAYER_SEQUENCE_STATE_PATH);
  const flushSequenceStore = () => {
    try {
      sequenceStore.flushNow();
    } catch (err) {
      console.error('failed to flush relayer sequence store:', err);
    }
  };
  process.on('SIGINT', flushSequenceStore);
  process.on('SIGTERM', flushSequenceStore);
  process.on('exit', flushSequenceStore);
  const inflightGate = new InflightGate(RELAYER_MAX_INFLIGHT, RELAYER_MAX_QUEUED);
  const ctmSigner: IntentSigner = {
    kind: 'keypair',
    privateKey: ctm.secretKey,
    publicKey: ctm.publicKey.toBytes(),
  };
  let cachedBlockhash: { fetchedAtMs: number; value: LatestBlockhash } | null =
    null;
  let blockhashInflight: Promise<LatestBlockhash> | null = null;
  const queueCountCache = new Map<string, { fetchedAtMs: number; count: number }>();
  const queueCountInflight = new Map<string, Promise<number>>();
  const getCachedLatestBlockhash = async (): Promise<LatestBlockhash> => {
    const now = Date.now();
    if (
      cachedBlockhash &&
      now - cachedBlockhash.fetchedAtMs <= RELAYER_BLOCKHASH_CACHE_MS
    ) {
      return cachedBlockhash.value;
    }
    if (blockhashInflight) {
      return await blockhashInflight;
    }
    blockhashInflight = (async () => {
      const fetched = await fetchLatestBlockHash(provider, {});
      cachedBlockhash = { fetchedAtMs: Date.now(), value: fetched };
      return fetched;
    })();
    try {
      return await blockhashInflight;
    } finally {
      blockhashInflight = null;
    }
  };
  const getCachedQueueCount = async (queuePk: PublicKey): Promise<number> => {
    const key = queuePk.toBase58();
    const now = Date.now();
    const cached = queueCountCache.get(key);
    if (cached && now - cached.fetchedAtMs <= RELAYER_QUEUE_COUNT_CACHE_MS) {
      return cached.count;
    }
    const inflight = queueCountInflight.get(key);
    if (inflight) {
      return await inflight;
    }
    const fetchPromise = (async () => {
      const ai = await connection.getAccountInfo(queuePk, 'processed');
      const count = ai?.data && ai.data.length >= 160 ? ai.data.readUInt32LE(156) : 0;
      queueCountCache.set(key, { fetchedAtMs: Date.now(), count });
      return count;
    })();
    queueCountInflight.set(key, fetchPromise);
    try {
      return await fetchPromise;
    } finally {
      queueCountInflight.delete(key);
    }
  };

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;

  const serviceImpl = {
    submitIntent: async (
      call: grpc.ServerUnaryCall<SubmitIntentRequest, SubmitIntentResponse>,
      callback: grpc.sendUnaryData<SubmitIntentResponse>,
    ) => {
      let gateHeld = false;
      const startedAtMs = nowMs();
      let gateMs = 0;
      let queueCheckMs = 0;
      let blockhashMs = 0;
      let buildMs = 0;
      let sendMs = 0;
      const requestId = crypto.randomUUID();
      const rawReq = call.request;
      let sequence: bigint | null = null;
      let builtKind: number | null = null;
      try {
        const gateStartMs = nowMs();
        await inflightGate.acquireWithTimeout(RELAYER_QUEUE_WAIT_TIMEOUT_MS);
        gateMs = nowMs() - gateStartMs;
        gateHeld = true;

        const req = rawReq;
        const group = cachedPublicKey(req.group);
        const executionQueue = cachedPublicKey(req.execution_queue);
        const userOwner = cachedPublicKey(req.user_owner);
        const mangoAccount = cachedPublicKey(req.mango_account);
        const payload = Buffer.from(req.payload ?? []);
        const remainingAccounts = cachedRemainingAccounts(req.remaining_accounts);

        const queueCheckStartMs = nowMs();
        const queueCount = await getCachedQueueCount(executionQueue);
        queueCheckMs = nowMs() - queueCheckStartMs;
        if (queueCount >= RELAYER_QUEUE_FULL_WATERMARK) {
          throw new RelayerError(
            grpc.status.RESOURCE_EXHAUSTED,
            `execution queue near/full: count=${queueCount}, watermark=${RELAYER_QUEUE_FULL_WATERMARK}`,
          );
        }

        const blockhashStartMs = nowMs();
        const latestForSlot = await getCachedLatestBlockhash();
        blockhashMs = nowMs() - blockhashStartMs;
        const nowSlot = BigInt(latestForSlot.slot);
        const requestedMinSlot = parseU64(req.min_execute_slot);
        const minExecuteSlot =
          requestedMinSlot > 0n
            ? requestedMinSlot
            : nowSlot + RELAYER_DEFAULT_MIN_EXECUTE_SLOT_OFFSET;
        const requestedExpiresSlot = parseU64(req.expires_at_slot);
        const expiresAtSlot =
          requestedExpiresSlot > 0n
            ? requestedExpiresSlot
            : RELAYER_DEFAULT_EXPIRES_AT_SLOT;

        const sequenceKey = `${group.toBase58()}:${req.market}`;
        const marketIndex = Number.parseInt(String(req.market ?? '0'), 10);
        if (Number.isNaN(marketIndex) || marketIndex < 0 || marketIndex > 0xffff) {
          throw new RelayerError(
            grpc.status.INVALID_ARGUMENT,
            `invalid market_index: '${req.market}'`,
          );
        }
        const userSigner: IntentSigner = {
          kind: 'presigned',
          publicKey: userOwner,
          signature: Buffer.from(req.user_signature ?? []),
        };

        const submitWithSequence = async (nextSequence: bigint) => {
          sequence = nextSequence;
          const buildStartMs = nowMs();
          const built = await buildExecutionQueueEnqueueCtmWithIntentIxs({
            programId,
            group,
            executionQueue,
            executionQueueBuffer:
              configuredExecutionQueueBuffer || executionQueue,
            marketIndex,
            remainingAccounts,
            payload,
            sequence: nextSequence,
            minExecuteSlot,
            expiresAtSlot,
            userOwner,
            mangoAccount,
            userSigner,
            ctmSigner,
          });

          let userIntentPreInstruction = built.userIntentPreInstruction;
          if (RELAYER_VERIFY_USER_SIGNATURE) {
            const userSigRawOk = nacl.sign.detached.verify(
              new Uint8Array(built.userIntentMessage),
              new Uint8Array(userSigner.signature),
              userOwner.toBytes(),
            );
            const userIntentMessageHexUtf8 = toHexUtf8IntentMessage(
              built.userIntentMessage,
            );
            const userSigHexUtf8Ok = nacl.sign.detached.verify(
              new Uint8Array(userIntentMessageHexUtf8),
              new Uint8Array(userSigner.signature),
              userOwner.toBytes(),
            );

            if (!userSigRawOk && !userSigHexUtf8Ok) {
              throw new Error('user intent signature verification failed');
            }

            userIntentPreInstruction = userSigRawOk
              ? built.userIntentPreInstruction
              : buildIntentEd25519Instruction(userIntentMessageHexUtf8, userSigner);
          }
          const instructions = [
            userIntentPreInstruction,
            built.ctmEnvelopePreInstruction,
            built.enqueueInstruction,
          ];
          builtKind = built.envelope.kind;
          buildMs += nowMs() - buildStartMs;

          void maybeEmitRelayIntentStatus({
            ts_ms: Date.now(),
            request_id: requestId,
            status_code: 1,
            status_label: 'accepted',
            reason: null,
            group: group.toBase58(),
            execution_queue: executionQueue.toBase58(),
            market: req.market,
            sequence: nextSequence.toString(),
            kind: built.envelope.kind,
            user_owner: userOwner.toBase58(),
            mango_account: mangoAccount.toBase58(),
            tx_signature: null,
            grpc_code: null,
            queue_process_status: null,
            queue_process_status_name: null,
          }).catch((sinkErr) => {
            console.error('relay status sink async emit failed:', sinkErr);
          });

          const latestBlockhash = latestForSlot;
          const sendStartMs = nowMs();
          const status = await sendTransaction(provider, instructions, [], {
            prioritizationFee: RELAYER_PRIORITIZATION_FEE,
            confirmInBackground: RELAYER_SUBMIT_MODE === 'fast',
            latestBlockhash,
            skipPreflight: RELAYER_SUBMIT_MODE === 'fast',
          });
          sendMs += nowMs() - sendStartMs;
          if (RELAYER_SUBMIT_MODE === 'strict') {
            await waitForProcessedSignature({
              connection,
              signature: status.signature,
              timeoutMs: RELAYER_POST_SEND_STATUS_TIMEOUT_MS,
              pollMs: RELAYER_POST_SEND_STATUS_POLL_MS,
            });
          }
          void maybeEmitRelayIntentStatus({
            ts_ms: Date.now(),
            request_id: requestId,
            status_code: 2,
            status_label: 'submitted',
            reason: null,
            group: group.toBase58(),
            execution_queue: executionQueue.toBase58(),
            market: req.market,
            sequence: nextSequence.toString(),
            kind: built.envelope.kind,
            user_owner: userOwner.toBase58(),
            mango_account: mangoAccount.toBase58(),
            tx_signature: status.signature,
            grpc_code: null,
            queue_process_status: null,
            queue_process_status_name: null,
          }).catch((sinkErr) => {
            console.error('relay status sink async emit failed:', sinkErr);
          });
          return { built, status };
        };

        let value: Awaited<ReturnType<typeof submitWithSequence>>;
        if (RELAYER_SERIALIZE_SUBMITS) {
          const res = await sequenceStore.withNextSequence(
            sequenceKey,
            submitWithSequence,
          );
          sequence = res.sequence;
          value = res.value;
        } else {
          sequence = sequenceStore.reserveNextSequenceSync(sequenceKey);
          value = await submitWithSequence(sequence);
        }

        void maybeEmitRelayIntentAccepted({
          ts_ms: Date.now(),
          request_id: requestId,
          group: group.toBase58(),
          execution_queue: executionQueue.toBase58(),
          market: req.market,
          sequence: sequence!.toString(),
          kind: value.built.envelope.kind,
          payload_b64: payload.toString('base64'),
          remaining_accounts: req.remaining_accounts ?? [],
          min_execute_slot: minExecuteSlot.toString(),
          expires_at_slot: expiresAtSlot.toString(),
          user_owner: userOwner.toBase58(),
          mango_account: mangoAccount.toBase58(),
          enqueue_tx_signature: value.status.signature,
        }).catch((sinkErr) => {
          console.error('relay event sink async emit failed:', sinkErr);
        });

        callback(null, {
          sequence: sequence.toString(),
          tx_signature: value.status.signature,
          user_intent_message: value.built.userIntentMessage,
          ctm_envelope_message: value.built.ctmEnvelopeMessage,
        });
      } catch (err: any) {
        const code =
          err instanceof RelayerError
            ? err.code
            : err?.message?.includes('timed out') ||
                err?.message?.includes('DEADLINE_EXCEEDED')
              ? grpc.status.DEADLINE_EXCEEDED
              : grpc.status.INVALID_ARGUMENT;
        void maybeEmitRelayIntentStatus({
          ts_ms: Date.now(),
          request_id: requestId,
          status_code: 0,
          status_label: 'rejected',
          reason: err?.message || `${err}`,
          group: rawReq.group ? String(rawReq.group) : null,
          execution_queue: rawReq.execution_queue
            ? String(rawReq.execution_queue)
            : null,
          market: rawReq.market ? String(rawReq.market) : null,
          sequence: sequence ? sequence.toString() : null,
          kind: builtKind,
          user_owner: rawReq.user_owner ? String(rawReq.user_owner) : null,
          mango_account: rawReq.mango_account ? String(rawReq.mango_account) : null,
          tx_signature: null,
          grpc_code: code,
          queue_process_status: null,
          queue_process_status_name: null,
        }).catch((sinkErr) => {
          console.error('relay status sink async emit failed:', sinkErr);
        });
        callback(
          {
            code,
            message: err?.message || `${err}`,
          },
          null,
        );
      } finally {
        recordProfile({
          totalMs: nowMs() - startedAtMs,
          gateMs,
          queueCheckMs,
          blockhashMs,
          buildMs,
          sendMs,
        });
        if (gateHeld) {
          inflightGate.release();
        }
      }
    },
  };

  const server = new grpc.Server();
  server.addService(proto.ctmsequencer.CtmSequencerRelayer.service, serviceImpl);

  server.bindAsync(
    RELAYER_BIND_ADDR,
    grpc.ServerCredentials.createInsecure(),
    (err) => {
      if (err) {
        throw err;
      }
      console.log(
        `CTM relayer listening on ${RELAYER_BIND_ADDR}, ctm=${ctm.publicKey.toBase58()}, wsEndpoint=${wsEndpoint || 'default'}, submitMode=${RELAYER_SUBMIT_MODE}, serializeSubmits=${RELAYER_SERIALIZE_SUBMITS}, confirmInBackground=${RELAYER_CONFIRM_IN_BACKGROUND}, blockhashCacheMs=${RELAYER_BLOCKHASH_CACHE_MS}, maxInflight=${RELAYER_MAX_INFLIGHT}, maxQueued=${RELAYER_MAX_QUEUED}, queueWaitMs=${RELAYER_QUEUE_WAIT_TIMEOUT_MS}, queueWatermark=${RELAYER_QUEUE_FULL_WATERMARK}, queueCountCacheMs=${RELAYER_QUEUE_COUNT_CACHE_MS}`,
      );
      server.start();
    },
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
