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
import { MANGO_V4_ID } from '../../src/constants';
import {
  ContinuumStateEngine,
  HarnessEvent,
  QueueView,
  RelayIntentAcceptedEvent,
  decodeQueueAnchorEvent,
  parseProgramDataLogLine,
} from '../../src/continuumHarness';
import { MangoClient } from '../../src/client';
import { HealthType } from '../../src/accounts/mangoAccount';
import { ZERO_I80F48 } from '../../src/numbers/I80F48';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const PROGRAM_ID_OVERRIDE = process.env.CONTINUUM_HARNESS_PROGRAM_ID;
const HARNESS_BIND_ADDR = process.env.CONTINUUM_HARNESS_BIND_ADDR || '0.0.0.0:9091';
const HARNESS_MODE = process.env.CONTINUUM_HARNESS_MODE || 'local';
const HARNESS_EVENT_LOG_PATH =
  process.env.CONTINUUM_HARNESS_EVENT_LOG_PATH || '/tmp/continuum-harness-events.jsonl';
const HARNESS_RELAY_INGEST_TOKEN = process.env.CONTINUUM_HARNESS_RELAY_INGEST_TOKEN || '';
const HARNESS_REPLAY_LOG = (process.env.CONTINUUM_HARNESS_REPLAY_LOG || 'true') === 'true';
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
  process.env.CONTINUUM_HARNESS_AIRDROP_KEYPAIR || process.env.MB_PAYER_KEYPAIR || '';
const HARNESS_AIRDROP_DEFAULT_UI_AMOUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_DEFAULT_UI_AMOUNT || '1000',
);
const HARNESS_AIRDROP_MAX_UI_AMOUNT = Number(
  process.env.CONTINUUM_HARNESS_AIRDROP_MAX_UI_AMOUNT || '100000',
);
const HARNESS_GROUP_PK =
  process.env.CONTINUUM_HARNESS_GROUP_PK || process.env.EXECUTION_QUEUE_GROUP_PK || '';
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

const engine = new ContinuumStateEngine();
const sseClients = new Set<ServerResponse>();
const sanityStateByOwner = new Map<string, string>();

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

function ensureDirForFile(filePath: string): void {
  fs.mkdirSync(path.dirname(path.resolve(filePath)), { recursive: true });
}

function writeJson(res: ServerResponse, status: number, body: unknown): void {
  res.statusCode = status;
  res.setHeader('Content-Type', 'application/json');
  res.end(JSON.stringify(body));
}

function writeText(res: ServerResponse, status: number, body: string): void {
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

function symbolToCanonicalMint(symbol: string, usdcMint: PublicKey | null): string {
  switch (symbol.trim().toUpperCase()) {
    case 'SOL':
      return 'So11111111111111111111111111111111111111112';
    case 'USDC':
      return usdcMint?.toBase58() || '';
    default:
      return '';
  }
}

async function getFreshGroup(onchain: OnchainContext | null): Promise<HarnessGroup | null> {
  if (!onchain?.mangoClient || !onchain.groupPk) {
    return null;
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
    Date.now() - onchain.cachedMarketMetadataFetchedAtMs < HARNESS_ONCHAIN_CACHE_TTL_MS
  ) {
    return onchain.cachedMarketMetadata;
  }

  const group = await getFreshGroup(onchain);
  if (!group) {
    return {};
  }

  const metadata: Record<string, HarnessMarketMetadata> = {};
  for (const [marketIndex, perpMarket] of group.perpMarketsMapByMarketIndex.entries()) {
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

function writeSseEvent(res: ServerResponse, eventName: string, data: unknown): void {
  res.write(`event: ${eventName}\n`);
  res.write(`data: ${JSON.stringify(data)}\n\n`);
}

function broadcastEvent(event: HarnessEvent): void {
  const name = event.event_type;
  for (const client of sseClients) {
    writeSseEvent(client, name, event);
  }
}

function appendEventLog(event: HarnessEvent): void {
  ensureDirForFile(HARNESS_EVENT_LOG_PATH);
  fs.appendFileSync(HARNESS_EVENT_LOG_PATH, `${JSON.stringify(event)}\n`);
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
  const intents = engine.listIntents();
  const divergences = engine.listDivergences(1000000).length;
  const optimistic = engine.getSnapshot('optimistic');
  return {
    mode: HARNESS_MODE,
    intents_total: intents.length,
    divergences_total: divergences,
    markets_total: Object.keys(optimistic.markets).length,
    users_total: Object.keys(optimistic.users).length,
    queue_views_total: Object.keys(optimistic.queue).length,
    sse_clients: sseClients.size,
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
      engine.reportExternalDivergence('onchain_balance_sanity_mismatch', `owner:${owner}`, {
        owner,
        harness_mango_accounts: harnessAccounts.join(','),
        onchain_mango_accounts: onchainAccounts.join(','),
        harness_usdc_ui_balance: harnessUsdcUiBalance.toString(),
        onchain_usdc_ui_balance: onchain.usdcUiBalance.toString(),
        usdc_delta: usdcDelta.toString(),
      });
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

  const content = fs.readFileSync(logPath, 'utf-8');
  if (!content.trim().length) {
    return;
  }

  for (const line of content.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed.length) {
      continue;
    }
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
      console.error('failed to replay event log line:', err);
    }
  }
}

function parseHeaderSingle(req: IncomingMessage, key: string): string | undefined {
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
    payload?.owner || payload?.wallet || payload?.wallet_pubkey || payload?.user_owner;
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
    const ownerAccounts = await onchain.mangoClient.getMangoAccountsForOwner(group, owner);
    if (!ownerAccounts.length) {
      return {
        ...baseState,
        margin_summary: {
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
          equity_native_quote: '0',
          pnl_native_quote: '0',
          assets_native_quote: '0',
          liabs_native_quote: '0',
          init_health_native_quote: '0',
          maint_health_native_quote: '0',
          margin_usage_fraction: 0,
          accounts: [],
        },
      };
    }

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

    const tokens = Array.from(tokenTotals.values()).sort(
      (a, b) => a.token_index - b.token_index,
    );
    const usdcMint = onchain.usdcMint?.toBase58() || tokens[0]?.mint || '';
    const usdcToken = tokens.find((t) => t.mint === usdcMint);
    const aggregate = {
      equity: ZERO_I80F48(),
      pnl: ZERO_I80F48(),
      assets: ZERO_I80F48(),
      liabs: ZERO_I80F48(),
      initHealth: ZERO_I80F48(),
      maintHealth: ZERO_I80F48(),
    };
    const marginAccounts = ownerAccounts.map((account) => {
      const equity = account.getEquity(group);
      const pnl = account.getPnl(group);
      const assets = account.getAssetsValue(group, HealthType.init);
      const liabs = account.getLiabsValue(group);
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

    const totalMarginUsage = aggregate.assets.toNumber() > 0
      ? aggregate.liabs.div(aggregate.assets).toNumber()
      : 0;

    const collateralPayload = {
      source:
        view === 'optimistic'
          ? 'onchain-baseline'
          : 'onchain',
      usdc_mint: usdcMint,
      usdc_ui_balance: usdcToken?.ui_balance || 0,
      tokens,
    };

    return {
      ...baseState,
      mango_accounts:
        baseState.mango_accounts?.length
          ? baseState.mango_accounts
          : ownerAccounts.map((a) => a.publicKey.toBase58()),
      margin_summary: {
        status: 'ok',
        source: 'onchain-mango-health',
        account_count: marginAccounts.length,
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
        accounts: marginAccounts,
      },
      ...(view === 'confirmed'
        ? { confirmed_collateral: collateralPayload }
        : { optimistic_collateral: collateralPayload }),
    };
  } catch (err) {
    console.warn(`failed to enrich ${view} balances for ${ownerRaw}: ${err}`);
    return baseState;
  }
}

async function buildAirdropContext(
  onchain: OnchainContext,
): Promise<AirdropContext | null> {
  if (!HARNESS_ENABLE_AIRDROP) {
    return null;
  }
  if (!onchain.usdcMint || !HARNESS_AIRDROP_KEYPAIR.length) {
    console.warn(
      'airdrop endpoint disabled: unable to resolve USDC mint or faucet keypair',
    );
    return null;
  }

  const usdcMint = onchain.usdcMint;
  const faucet = readKeypair(HARNESS_AIRDROP_KEYPAIR);
  const mintInfo = await getMint(onchain.connection, usdcMint);

  if (!mintInfo.mintAuthority || !mintInfo.mintAuthority.equals(faucet.publicKey)) {
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
  const groupPk = HARNESS_GROUP_PK.length ? new PublicKey(HARNESS_GROUP_PK) : null;
  let mangoClient: MangoClient | null = null;
  let cachedGroup: HarnessGroup | null = null;
  let usdcMint: PublicKey | null =
    HARNESS_USDC_MINT.length ? new PublicKey(HARNESS_USDC_MINT) : null;

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
    } catch (err) {
      console.warn(
        `onchain read context disabled: failed to initialize group/mango client (${err})`,
      );
      mangoClient = null;
      cachedGroup = null;
      usdcMint = HARNESS_USDC_MINT.length ? new PublicKey(HARNESS_USDC_MINT) : null;
    }
  }

  return {
    connection,
    groupPk,
    mangoClient,
    usdcMint,
    programId,
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
  if (!HARNESS_BACKFILL_SIGNATURE_LIMIT || HARNESS_BACKFILL_SIGNATURE_LIMIT <= 0) {
    return;
  }

  const signatures = await connection.getSignaturesForAddress(programId, {
    limit: HARNESS_BACKFILL_SIGNATURE_LIMIT,
  });
  const txCommitment = HARNESS_COMMITMENT === 'finalized' ? 'finalized' : 'confirmed';

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
    handleProgramLogs(
      {
        err: tx.meta.err,
        logs: tx.meta.logMessages,
        signature: sig.signature,
      },
      tx.slot,
    );
  }
}

function handleProgramLogs(logs: Logs, slot: number): void {
  for (const line of logs.logs) {
    const raw = parseProgramDataLogLine(line);
    if (!raw) {
      continue;
    }
    const decoded = decodeQueueAnchorEvent(raw);
    if (!decoded) {
      continue;
    }

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

  const uiAmount = toPositiveUiAmount(payload?.ui_amount, airdrop.defaultUiAmount);
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
    throw new Error('airdrop-deposit endpoint is disabled: group/client not configured');
  }

  const body = await readBody(req);
  const payload = body.trim().length ? JSON.parse(body) : {};
  if (payload?.ui_amount !== undefined && Number(payload.ui_amount) !== airdrop.depositUiAmount) {
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
    const ownerAccounts = await airdrop.mangoClient.getMangoAccountsForOwner(group, owner);
    if (!ownerAccounts.length) {
      if (!HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT) {
        throw new Error('no mango account found for owner in configured group');
      }
      const requestedAccountNum = payload?.account_num;
      const accountNum = requestedAccountNum === undefined
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
      mangoAccount = await airdrop.mangoClient.getMangoAccount(createResult.mangoAccountPk);
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
      await airdrop.mangoClient.sendAndConfirmTransactionForGroup(group, [unsafeDepositIx]);
    unsafeDepositSignature = unsafeDepositStatus.signature;
  } catch (err: any) {
    const asText = err?.message || `${err}`;
    const missingUnsafeDepositIx =
      asText.includes('"Custom":101') || asText.includes('InstructionFallbackNotFound');
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

  const ownerRaw = decodeURIComponent(url.pathname.split('/').pop() || '').trim();
  if (!ownerRaw.length) {
    throw new Error('owner is required');
  }
  const owner = new PublicKey(ownerRaw);
  const group = await getFreshGroup(onchain);
  if (!group) {
    throw new Error('deposit context unavailable: group could not be loaded');
  }

  const quoteMint = onchain.usdcMint ?? group.getFirstBankForPerpSettlement().mint;
  const quoteBank = group.getFirstBankByMint(quoteMint);
  const requestedAccountNumRaw = url.searchParams.get('account_num');
  const requestedAccountNum = requestedAccountNumRaw ? Number(requestedAccountNumRaw) : 0;
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
    healthRemainingAccounts = await onchain.mangoClient.buildHealthRemainingAccounts(
      group,
      [mangoAccount],
      [quoteBank],
      [],
    );
  } else {
    const ownerAccounts = await onchain.mangoClient.getMangoAccountsForOwner(group, owner);
    const requestedAccount = ownerAccounts.find(
      (account) => account.accountNum === requestedAccountNum,
    );
    const mangoAccount = requestedAccount ?? ownerAccounts[0] ?? null;
    if (mangoAccount) {
      mangoAccountPk = mangoAccount.publicKey;
      mangoAccountExists = true;
      healthRemainingAccounts = await onchain.mangoClient.buildHealthRemainingAccounts(
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
      const fallbackMap = await onchain.mangoClient.deriveFallbackOracleContexts(group);
      const fallbackAccounts = fallbackMap.get(quoteBank.oracle.toBase58()) || [];
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
    health_remaining_accounts: healthRemainingAccounts.map((pk) => pk.toBase58()),
    default_ui_amount: HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT,
  };
}

function buildHttpServer(
  onchain: OnchainContext | null,
  airdrop: AirdropContext | null,
): http.Server {
  return http.createServer(async (req, res) => {
    try {
      const url = parsePathAndQuery(req);
      const method = req.method || 'GET';

      if (method === 'GET' && url.pathname === '/healthz') {
        const marketMetadata = await getMarketMetadataMap(onchain);
        writeJson(res, 200, {
          ok: true,
          ...statsSnapshot(),
          onchain_read_enabled: !!onchain?.groupPk && !!onchain?.mangoClient,
          market_metadata_total: Object.keys(marketMetadata).length,
          airdrop_enabled: !!airdrop,
          airdrop_deposit_enabled: !!airdrop?.groupPk && !!airdrop?.mangoClient,
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

      if (method === 'GET' && url.pathname === '/diagnostics/divergence') {
        const limit = Number(url.searchParams.get('limit') || '200');
        writeJson(res, 200, {
          items: engine.listDivergences(limit),
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

      if (method === 'GET' && url.pathname.startsWith('/state/deposit-context/')) {
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

      if (method === 'GET' && url.pathname === '/state/stream') {
        res.statusCode = 200;
        res.setHeader('Content-Type', 'text/event-stream');
        res.setHeader('Cache-Control', 'no-cache');
        res.setHeader('Connection', 'keep-alive');
        res.flushHeaders?.();

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

        req.on('close', () => {
          sseClients.delete(res);
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/markets/')) {
        const market = decodeURIComponent(url.pathname.slice('/state/markets/'.length));
        const view = parseView(url);
        const marketMetadata = await getMarketMetadataMap(onchain);
        writeJson(res, 200, {
          view,
          metadata: marketMetadata[market] || null,
          data: engine.getMarketState(market, view),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/users/')) {
        const owner = decodeURIComponent(url.pathname.slice('/state/users/'.length));
        const view = parseView(url);
        const baseUserState = engine.getUserState(owner, view);
        const includeOnchain =
          (url.searchParams.get('onchain') || 'true').toLowerCase() !== 'false';
        const data = includeOnchain
          ? await enrichOwnerStateWithOnchain(
              owner,
              baseUserState,
              onchain,
              view,
            )
          : baseUserState;
        writeJson(res, 200, {
          view,
          data,
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/balances/')) {
        const owner = decodeURIComponent(
          url.pathname.slice('/state/balances/'.length),
        );
        const view = parseView(url);
        const baseBalances = engine.getBalances(owner, view);
        const includeOnchain =
          (url.searchParams.get('onchain') || 'true').toLowerCase() !== 'false';
        const data = includeOnchain
          ? await enrichOwnerStateWithOnchain(
              owner,
              baseBalances,
              onchain,
              view,
            )
          : baseBalances;
        writeJson(res, 200, {
          view,
          data,
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/orders/')) {
        const market = decodeURIComponent(url.pathname.slice('/state/orders/'.length));
        const view = parseView(url);
        const owner = url.searchParams.get('owner');
        writeJson(res, 200, {
          view,
          market,
          owner,
          data: engine.getOrders(market, owner, view),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/trades/')) {
        const market = decodeURIComponent(url.pathname.slice('/state/trades/'.length));
        const view = parseView(url);
        const limit = Number(url.searchParams.get('limit') || '200');
        writeJson(res, 200, {
          view,
          market,
          data: engine.getTrades(
            market,
            view,
            Number.isFinite(limit) ? Math.max(0, Math.floor(limit)) : 200,
          ),
        });
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/state/candles/')) {
        const market = decodeURIComponent(
          url.pathname.slice('/state/candles/'.length),
        );
        const view = parseView(url);
        const limit = Number(url.searchParams.get('limit') || '200');
        const resolutionSec = Number(url.searchParams.get('resolution_sec') || '60');
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
        const market = decodeURIComponent(url.pathname.slice('/state/queue/'.length));
        writeJson(res, 200, {
          market,
          data: engine.getQueueState(market),
        });
        return;
      }

      if (method === 'GET' && url.pathname === '/state/full') {
        const view = parseView(url);
        const market = url.searchParams.get('market');
        const snapshot = engine.getSnapshot(view);
        const marketMetadata = await getMarketMetadataMap(onchain);
        if (!market) {
          writeJson(res, 200, {
            ...snapshot,
            market_metadata: marketMetadata,
          });
          return;
        }
        const marketState = snapshot.markets[market] || null;
        const queueState = snapshot.queue[market] || null;
        const users = Object.fromEntries(
          Object.entries(snapshot.users).filter(([, user]) => {
            return user.open_orders.some((order) => order.market === market);
          }),
        );
        writeJson(res, 200, {
          view,
          generated_ts_ms: snapshot.generated_ts_ms,
          market: marketState,
          market_metadata: marketMetadata[market] || null,
          queue: queueState,
          users,
        });
        return;
      }

      writeJson(res, 404, {
        error: 'not_found',
        path: url.pathname,
      });
    } catch (err: any) {
      writeJson(res, 500, {
        error: err?.message || `${err}`,
      });
    }
  });
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

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }

  const connection = new Connection(
    CLUSTER_URL,
    HARNESS_COMMITMENT === 'processed' ? 'confirmed' : HARNESS_COMMITMENT,
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];

  replayEventLogIfPresent();
  await maybeBackfillProgramLogs(connection, programId);
  const onchain = await buildOnchainContext(connection, programId);
  const airdrop = await buildAirdropContext(onchain);

  engine.subscribe((event) => {
    appendEventLog(event);
    broadcastEvent(event);
  });

  await connection.onLogs(
    programId,
    (logs, ctx) => {
      handleProgramLogs(logs, ctx.slot);
    },
    HARNESS_COMMITMENT,
  );

  const server = buildHttpServer(onchain, airdrop);
  const { host, port } = parseBindAddress(HARNESS_BIND_ADDR);

  setInterval(() => {
    for (const client of sseClients) {
      client.write(`: heartbeat ${Date.now()}\n\n`);
    }
  }, 15000);

  setInterval(() => {
    runOnchainBalanceSanityCheck(onchain).catch((err) => {
      console.warn(`onchain sanity check failed: ${err}`);
    });
  }, Math.max(1000, HARNESS_SANITY_INTERVAL_MS));
  runOnchainBalanceSanityCheck(onchain).catch((err) => {
    console.warn(`initial onchain sanity check failed: ${err}`);
  });

  server.listen(port, host, () => {
    console.log(
      `Continuum state harness listening on http://${host}:${port}, mode=${HARNESS_MODE}, program=${programId.toBase58()}`,
    );
    if (onchain?.groupPk && onchain.mangoClient) {
      console.log(
        `Onchain read context enabled: group=${onchain.groupPk.toBase58()}, usdc_mint=${onchain.usdcMint?.toBase58() || 'unresolved'}`,
      );
    }
    if (airdrop) {
      console.log(
        `USDC airdrop enabled: mint=${airdrop.usdcMint.toBase58()}, default_ui_amount=${airdrop.defaultUiAmount}`,
      );
      if (airdrop.groupPk && airdrop.mangoClient) {
        console.log(
          `USDC airdrop-deposit enabled: group=${airdrop.groupPk.toBase58()}, ui_amount=${airdrop.depositUiAmount}`,
        );
      }
    }
  });
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
