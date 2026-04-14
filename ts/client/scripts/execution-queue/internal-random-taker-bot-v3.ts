import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { AccountMeta, Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { MangoClient } from '../../src/client';
import {
  PerpMarket,
  PerpMarketIndex,
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import {
  buildExecutionQueueUserIntent,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { I64_MAX_BN } from '../../src/utils';
import http from 'http';

dotenv.config();

type StackConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  perpMarketIndex: number;
  relayer: {
    bindAddr: string;
  };
};

type TakerBotSpec = {
  name: string;
  keypairPath: string;
  owner?: string;
  mangoAccount: string;
  accountNum?: number;
};

type MarketRuntimeState = {
  marketIndex: number;
  name: string;
  baseLotSizeUi: number;
};

type TakerBotRuntime = {
  name: string;
  user: Keypair;
  client: MangoClient;
  mangoAccountPk: PublicKey;
  ownerPk: PublicKey;
  cachedBasePositionLotsByMarket: Map<number, bigint>;
  lastHarnessPollMs: number;
};

type DispatchTarget = {
  bot: TakerBotRuntime;
  marketIndex: number;
};

const RUNTIME_ROOT = path.resolve(__dirname, '../../../..');
// v2 sub-queue deploy: defaults point at the 9200 group bootstrapped by
// `v2-multi-market-bootstrap.ts`. Override via TAKER_CONFIG_PATH /
// TAKER_BOTS_CONFIG_PATH for other groups.
const DEFAULT_CONFIG_PATH = path.resolve(
  RUNTIME_ROOT,
  '.devnet/run/execution-queue-e2e-9200.json',
);
const DEFAULT_BOTS_CONFIG_PATH = path.resolve(
  RUNTIME_ROOT,
  '.devnet/run/taker-bots-9200.json',
);
const CONFIG_PATH = path.resolve(
  process.env.TAKER_CONFIG_PATH || DEFAULT_CONFIG_PATH,
);
const BOTS_CONFIG_PATH = path.resolve(
  process.env.TAKER_BOTS_CONFIG_PATH || DEFAULT_BOTS_CONFIG_PATH,
);
const CLUSTER_OVERRIDE = process.env.CLUSTER_OVERRIDE as Cluster | undefined;
const CLUSTER_URL_OVERRIDE = process.env.CLUSTER_URL_OVERRIDE;
const RELAYER_ADDR_OVERRIDE = process.env.CTM_RELAYER_ADDR;
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;
const EXECUTION_QUEUE_OVERRIDE = process.env.EXECUTION_QUEUE_PK;
const INTERVAL_MS = Number(process.env.TAKER_INTERVAL_MS || '5000');
const MAX_TICKS = Number(process.env.TAKER_MAX_TICKS || '0');
const SIZE_UI = Number(process.env.TAKER_SIZE_UI || '0.1');
const TARGET_NOTIONAL_USD = Number(process.env.TAKER_TARGET_NOTIONAL_USD || '0');
const SLIPPAGE_BPS = Number(process.env.TAKER_SLIPPAGE_BPS || '2000');
const ORDER_LIMIT = Number(process.env.TAKER_ORDER_LIMIT || '20');

// Harness position-tracking / skew config
const HARNESS_URL = process.env.TAKER_HARNESS_URL || 'http://127.0.0.1:9091';
const HARNESS_POLL_MS = Number(process.env.TAKER_HARNESS_POLL_MS || '20000');
const SKEW_SOFT_LIMIT_UI = Number(process.env.TAKER_SKEW_SOFT_LIMIT_UI || '5');
const SKEW_HARD_LIMIT_UI = Number(process.env.TAKER_SKEW_HARD_LIMIT_UI || '50');
// v2 sub-queue deploy bootstraps SOL-PERP (idx 0) and BTC-PERP (idx 1).
// Override via TAKER_MARKET_INDEXES for other deploys.
const DEFAULT_MARKET_INDEXES = '0,1';
const MARKET_INDEXES = parseMarketIndexes(
  process.env.TAKER_MARKET_INDEXES || DEFAULT_MARKET_INDEXES,
);

const marketRuntimeByIndex = new Map<number, MarketRuntimeState>();
let dispatchQueue: DispatchTarget[] = [];

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function readJsonFile<T>(filePath: string): T {
  return JSON.parse(fs.readFileSync(filePath, 'utf-8')) as T;
}

function resolveKeypairPath(configuredPath: string): string {
  const directPath = path.resolve(configuredPath);
  if (fs.existsSync(directPath)) {
    return directPath;
  }

  const fallback = path.resolve(RUNTIME_ROOT, 'keypairs', path.basename(configuredPath));
  if (fs.existsSync(fallback)) {
    return fallback;
  }

  throw new Error(`missing keypair path: ${configuredPath}`);
}

function readKeypair(filePath: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(filePath, 'utf-8'))),
  );
}

function parseMarketIndexes(raw: string): number[] {
  const parsed = Array.from(
    new Set(
      raw
        .split(',')
        .map((value) => Number(value.trim()))
        .filter((value) => Number.isInteger(value) && value >= 0),
    ),
  );
  if (parsed.length === 0) {
    throw new Error(`no valid market indexes in TAKER_MARKET_INDEXES=${raw}`);
  }
  return parsed;
}

function ensureMarketRuntime(marketIndex: number): MarketRuntimeState {
  const existing = marketRuntimeByIndex.get(marketIndex);
  if (existing) {
    return existing;
  }
  const created: MarketRuntimeState = {
    marketIndex,
    name: `market-${marketIndex}`,
    baseLotSizeUi: 0.01,
  };
  marketRuntimeByIndex.set(marketIndex, created);
  return created;
}

function hydrateMarketRuntimeFromPerpMarket(perpMarket: PerpMarket): void {
  const runtime = ensureMarketRuntime(Number(perpMarket.perpMarketIndex));
  runtime.name = perpMarket.name;
  runtime.baseLotSizeUi =
    Number(perpMarket.baseLotSize.toString()) / 10 ** perpMarket.baseDecimals;
}

function hydrateMarketMetadataFromGroup(group: Awaited<ReturnType<MangoClient['getGroup']>>): void {
  for (const marketIndex of MARKET_INDEXES) {
    const perpMarket = group.perpMarketsMapByMarketIndex.get(
      marketIndex as PerpMarketIndex,
    );
    if (!perpMarket) {
      continue;
    }
    hydrateMarketRuntimeFromPerpMarket(perpMarket);
  }
}

function currentPositionUi(bot: TakerBotRuntime, marketIndex: number): number {
  const marketRuntime = ensureMarketRuntime(marketIndex);
  const lots = bot.cachedBasePositionLotsByMarket.get(marketIndex) ?? 0n;
  return Number(lots) * marketRuntime.baseLotSizeUi;
}

function chooseSide(bot: TakerBotRuntime, marketIndex: number): PerpOrderSide {
  const positionUi = currentPositionUi(bot, marketIndex);
  const absPos = Math.abs(positionUi);

  if (absPos >= SKEW_HARD_LIMIT_UI) {
    return positionUi > 0 ? PerpOrderSide.ask : PerpOrderSide.bid;
  }

  if (absPos >= SKEW_SOFT_LIMIT_UI) {
    const reducingSide =
      positionUi > 0 ? PerpOrderSide.ask : PerpOrderSide.bid;
    const addingSide =
      positionUi > 0 ? PerpOrderSide.bid : PerpOrderSide.ask;
    return Math.random() < 0.75 ? reducingSide : addingSide;
  }

  return Math.random() < 0.5 ? PerpOrderSide.bid : PerpOrderSide.ask;
}

function computeCapPriceUi(params: {
  side: PerpOrderSide;
  oraclePriceUi: number;
  bestBidUi?: number;
  bestAskUi?: number;
  slippageBps: number;
}): number {
  const slip = params.slippageBps / 10_000;

  if (params.side === PerpOrderSide.bid) {
    const base = params.bestAskUi ?? params.oraclePriceUi;
    const price = base * (1 + slip);
    return price > 0 && Number.isFinite(price) ? price : params.oraclePriceUi * (1 + slip);
  } else {
    const base = params.bestBidUi ?? params.oraclePriceUi;
    const price = base * (1 - slip);
    return price > 0 && Number.isFinite(price) ? price : params.oraclePriceUi * (1 - slip);
  }
}

function clampCapPriceToInsideLimit(params: {
  perpMarket: {
    maintBaseAssetWeight: { toNumber(): number };
    maintBaseLiabWeight: { toNumber(): number };
    uiPrice: number;
  };
  side: PerpOrderSide;
  capPriceUi: number;
}): number {
  const lowerBoundUi =
    params.perpMarket.maintBaseAssetWeight.toNumber() * params.perpMarket.uiPrice;
  const upperBoundUi =
    params.perpMarket.maintBaseLiabWeight.toNumber() * params.perpMarket.uiPrice;

  if (params.side === PerpOrderSide.bid) {
    return Math.min(params.capPriceUi, upperBoundUi);
  }

  return Math.max(params.capPriceUi, lowerBoundUi);
}

function computeOrderBaseLots(params: {
  perpUiPrice: number;
  perpUiBaseToLots: (baseUi: number) => any;
  marketIndex: number;
}): { baseLots: bigint; sizeUi: number; targetNotionalUsd: number | null } {
  const marketRuntime = ensureMarketRuntime(params.marketIndex);
  const desiredSizeUi =
    TARGET_NOTIONAL_USD > 0 && Number.isFinite(params.perpUiPrice) && params.perpUiPrice > 0
      ? TARGET_NOTIONAL_USD / params.perpUiPrice
      : SIZE_UI;

  const baseLots = BigInt(params.perpUiBaseToLots(desiredSizeUi).toString());
  if (baseLots <= 0n) {
    return {
      baseLots,
      sizeUi: 0,
      targetNotionalUsd: TARGET_NOTIONAL_USD > 0 ? TARGET_NOTIONAL_USD : null,
    };
  }

  return {
    baseLots,
    sizeUi: Number(baseLots) * marketRuntime.baseLotSizeUi,
    targetNotionalUsd: TARGET_NOTIONAL_USD > 0 ? TARGET_NOTIONAL_USD : null,
  };
}

function shuffleTargets(targets: DispatchTarget[]): DispatchTarget[] {
  const out = [...targets];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

function refillDispatchQueue(bots: TakerBotRuntime[]): void {
  const targets: DispatchTarget[] = [];
  for (const bot of bots) {
    for (const marketIndex of MARKET_INDEXES) {
      targets.push({ bot, marketIndex });
    }
  }
  dispatchQueue = shuffleTargets(targets);
  console.log(
    JSON.stringify({
      event: 'taker_bot_v3_cycle_refresh',
      dispatch_mode: 'random-cycle-bot-market',
      order: dispatchQueue.map((target) => ({
        bot_name: target.bot.name,
        market_index: target.marketIndex,
        market_name: ensureMarketRuntime(target.marketIndex).name,
      })),
    }),
  );
}

function selectDispatchTarget(bots: TakerBotRuntime[]): DispatchTarget {
  if (dispatchQueue.length === 0) {
    refillDispatchQueue(bots);
  }
  const next = dispatchQueue.shift();
  if (!next) {
    throw new Error('dispatch queue unexpectedly empty');
  }
  return next;
}

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
  marketIndex: number;
  userOwner: PublicKey;
}): Promise<AccountMeta[]> {
  return params.client.buildExecutionQueueCanonicalPerpRemainingAccounts(
    params.group,
    params.mangoAccount,
    params.marketIndex as PerpMarketIndex,
    params.userOwner,
  );
}

async function submitIntentViaRelayer(params: {
  group: PublicKey;
  executionQueue: PublicKey;
  marketIndex: number;
  mangoAccount: PublicKey;
  user: Keypair;
  payload: Buffer;
  remainingAccounts: AccountMeta[];
  relayerAddr: string;
}): Promise<{ sequence: string; tx_signature?: string }> {
  const intent = await buildExecutionQueueUserIntent({
    group: params.group,
    executionQueue: params.executionQueue,
    mangoAccount: params.mangoAccount,
    userOwner: params.user.publicKey,
    payload: params.payload,
    target: { kind: 0, index: params.marketIndex },
    remainingAccounts: params.remainingAccounts,
  });
  const userSignature = signExecutionQueueIntentMessage(
    params.user.secretKey,
    intent.userIntentMessage,
  );

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  const relayerClient = new proto.ctmsequencer.CtmSequencerRelayer(
    params.relayerAddr,
    grpc.credentials.createInsecure(),
  );

  return await new Promise((resolve, reject) => {
    relayerClient.submitIntent(
      {
        group: params.group.toBase58(),
        execution_queue: params.executionQueue.toBase58(),
        market: String(params.marketIndex),
        payload: params.payload,
        remaining_accounts: params.remainingAccounts.map((a) => ({
          pubkey: a.pubkey.toBase58(),
          is_signer: !!a.isSigner,
          is_writable: !!a.isWritable,
        })),
        min_execute_slot: '0',
        expires_at_slot: '0',
        user_owner: params.user.publicKey.toBase58(),
        mango_account: params.mangoAccount.toBase58(),
        user_signature: Buffer.from(userSignature),
        intent_version: 2,
        target_kind: 0,
        target_index: params.marketIndex,
      },
      (err: Error | null, response: { sequence: string; tx_signature?: string }) => {
        if (err) {
          reject(err);
          return;
        }
        resolve(response);
      },
    );
  });
}

function httpGetJson(url: string): Promise<any> {
  return new Promise((resolve, reject) => {
    const req = http.get(url, { timeout: 5000 }, (res) => {
      let body = '';
      res.on('data', (chunk: string) => (body += chunk));
      res.on('end', () => {
        try {
          resolve(JSON.parse(body));
        } catch {
          reject(new Error(`invalid JSON from ${url}: ${body.slice(0, 200)}`));
        }
      });
    });
    req.on('error', reject);
    req.on('timeout', () => {
      req.destroy();
      reject(new Error(`timeout fetching ${url}`));
    });
  });
}

async function refreshMarketMetadata(marketIndex: number): Promise<void> {
  try {
    const marketRuntime = ensureMarketRuntime(marketIndex);
    const data = await httpGetJson(
      `${HARNESS_URL}/state/markets/${marketIndex}?view=confirmed`,
    );
    const metadata = data?.metadata ?? data?.data?.metadata ?? {};
    const rawLotSize = metadata?.base_lot_size;
    const baseDecimals = Number(metadata?.base_decimals ?? 4);
    if (rawLotSize) {
      marketRuntime.baseLotSizeUi = Number(rawLotSize) / 10 ** baseDecimals;
    }
    if (metadata?.name) {
      marketRuntime.name = metadata.name;
    }
  } catch (err) {
    console.log(
      JSON.stringify({
        event: 'market_metadata_refresh_error',
        market_index: marketIndex,
        error: err instanceof Error ? err.message : String(err),
      }),
    );
  }
}

async function pollHarnessPosition(bot: TakerBotRuntime): Promise<void> {
  try {
    const data = await httpGetJson(
      `${HARNESS_URL}/state/users/${bot.ownerPk.toBase58()}?view=confirmed`,
    );

    const nextPositions = new Map<number, bigint>();
    for (const marketIndex of MARKET_INDEXES) {
      nextPositions.set(marketIndex, 0n);
    }

    const perMarket = data?.data?.per_market;
    if (Array.isArray(perMarket) && perMarket.length > 0) {
      for (const entry of perMarket) {
        const marketIndex = Number(entry?.market);
        if (nextPositions.has(marketIndex)) {
          nextPositions.set(marketIndex, BigInt(entry?.base_position_lots ?? '0'));
        }
      }
    }

    bot.cachedBasePositionLotsByMarket = nextPositions;
    bot.lastHarnessPollMs = Date.now();
    console.log(
      JSON.stringify({
        event: 'harness_position_poll',
        bot_name: bot.name,
        owner: bot.ownerPk.toBase58(),
        positions: MARKET_INDEXES.map((marketIndex) => ({
          market_index: marketIndex,
          market_name: ensureMarketRuntime(marketIndex).name,
          base_position_lots: (bot.cachedBasePositionLotsByMarket.get(marketIndex) ?? 0n).toString(),
          base_position_ui: currentPositionUi(bot, marketIndex),
          lot_size_ui: ensureMarketRuntime(marketIndex).baseLotSizeUi,
        })),
        skew_soft_limit_ui: SKEW_SOFT_LIMIT_UI,
        skew_hard_limit_ui: SKEW_HARD_LIMIT_UI,
      }),
    );
  } catch (err) {
    console.log(
      JSON.stringify({
        event: 'harness_position_poll_error',
        bot_name: bot.name,
        owner: bot.ownerPk.toBase58(),
        error: err instanceof Error ? err.message : String(err),
      }),
    );
  }
}

function startHarnessPoller(bot: TakerBotRuntime): void {
  pollHarnessPosition(bot);
  setInterval(() => pollHarnessPosition(bot), HARNESS_POLL_MS);
}

async function buildBotRuntime(params: {
  spec: TakerBotSpec;
  cluster: Cluster;
  clusterUrl: string;
  programId: PublicKey;
  connection: Connection;
}): Promise<TakerBotRuntime> {
  const keypairPath = resolveKeypairPath(params.spec.keypairPath);
  const user = readKeypair(keypairPath);
  const ownerPk = user.publicKey;
  if (
    params.spec.owner &&
    params.spec.owner !== ownerPk.toBase58()
  ) {
    throw new Error(
      `[${params.spec.name}] keypair owner mismatch: manifest=${params.spec.owner} keypair=${ownerPk.toBase58()}`,
    );
  }

  const provider = new AnchorProvider(
    params.connection,
    new Wallet(user),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(provider, params.cluster, params.programId, {
    idsSource: 'get-program-accounts',
  });

  const cachedBasePositionLotsByMarket = new Map<number, bigint>();
  for (const marketIndex of MARKET_INDEXES) {
    cachedBasePositionLotsByMarket.set(marketIndex, 0n);
  }

  return {
    name: params.spec.name,
    user,
    client,
    mangoAccountPk: new PublicKey(params.spec.mangoAccount),
    ownerPk,
    cachedBasePositionLotsByMarket,
    lastHarnessPollMs: 0,
  };
}

async function main(): Promise<void> {
  const config = readJsonFile<StackConfig>(CONFIG_PATH);
  const botSpecs = readJsonFile<TakerBotSpec[]>(BOTS_CONFIG_PATH);
  if (!Array.isArray(botSpecs) || botSpecs.length === 0) {
    throw new Error(`no taker bot specs found in ${BOTS_CONFIG_PATH}`);
  }

  const cluster = CLUSTER_OVERRIDE || config.cluster;
  const clusterUrl = CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const programId = new PublicKey(PROGRAM_ID_OVERRIDE || config.programId);
  const executionQueue = new PublicKey(
    EXECUTION_QUEUE_OVERRIDE || config.executionQueue,
  );
  const relayerAddr = RELAYER_ADDR_OVERRIDE || config.relayer.bindAddr;
  const connection = new Connection(
    clusterUrl,
    AnchorProvider.defaultOptions(),
  );

  await Promise.all(MARKET_INDEXES.map((marketIndex) => refreshMarketMetadata(marketIndex)));

  const bots = await Promise.all(
    botSpecs.map(async (spec) => {
      return await buildBotRuntime({
        spec,
        cluster,
        clusterUrl,
        programId,
        connection,
      });
    }),
  );

  // Prime startup metadata from the on-chain group/perp definitions so the
  // first startup banner does not depend on the harness refresh path.
  const startupMangoAccount = await bots[0].client.getMangoAccount(bots[0].mangoAccountPk);
  const startupGroup = await bots[0].client.getGroup(startupMangoAccount.group);
  hydrateMarketMetadataFromGroup(startupGroup);

  console.log(
    JSON.stringify({
      event: 'taker_bot_v3_start',
      config_path: CONFIG_PATH,
      bots_config_path: BOTS_CONFIG_PATH,
      cluster,
      cluster_url: clusterUrl,
      program_id: programId.toBase58(),
      execution_queue: executionQueue.toBase58(),
      bot_count: bots.length,
      owners: bots.map((bot) => bot.ownerPk.toBase58()),
      market_indexes: MARKET_INDEXES,
      markets: MARKET_INDEXES.map((marketIndex) => ({
        market_index: marketIndex,
        market_name: ensureMarketRuntime(marketIndex).name,
        base_lot_size_ui: ensureMarketRuntime(marketIndex).baseLotSizeUi,
      })),
      interval_ms: INTERVAL_MS,
      size_ui: SIZE_UI,
      target_notional_usd: TARGET_NOTIONAL_USD > 0 ? TARGET_NOTIONAL_USD : null,
      slippage_bps: SLIPPAGE_BPS,
      max_ticks: MAX_TICKS,
      harness_url: HARNESS_URL,
      harness_poll_ms: HARNESS_POLL_MS,
      skew_soft_limit_ui: SKEW_SOFT_LIMIT_UI,
      skew_hard_limit_ui: SKEW_HARD_LIMIT_UI,
      dispatch_mode: 'random-cycle-bot-market',
    }),
  );

  for (const bot of bots) {
    startHarnessPoller(bot);
  }

  let tick = 0;
  while (MAX_TICKS === 0 || tick < MAX_TICKS) {
    tick += 1;

    const target = selectDispatchTarget(bots);
    const bot = target.bot;
    const marketIndex = target.marketIndex;
    const mangoAccount = await bot.client.getMangoAccount(bot.mangoAccountPk);
    const group = await bot.client.getGroup(mangoAccount.group);
    const perpMarket = group.getPerpMarketByMarketIndex(
      marketIndex as PerpMarketIndex,
    );
    // Refresh runtime metadata from the live market definition before any
    // position math or logging so stale harness/default metadata cannot skew
    // size or labels.
    hydrateMarketRuntimeFromPerpMarket(perpMarket);
    const marketRuntime = ensureMarketRuntime(marketIndex);
    const [bids, asks] = await Promise.all([
      perpMarket.loadBids(bot.client, true),
      perpMarket.loadAsks(bot.client, true),
    ]);
    const bestBidUi = bids.best()?.uiPrice;
    const bestAskUi = asks.best()?.uiPrice;
    const side = chooseSide(bot, marketIndex);
    const sideLabel = side === PerpOrderSide.bid ? 'buy' : 'sell';
    const rawCapPriceUi = computeCapPriceUi({
      side,
      oraclePriceUi: perpMarket.uiPrice,
      bestBidUi,
      bestAskUi,
      slippageBps: SLIPPAGE_BPS,
    });
    const capPriceUi = clampCapPriceToInsideLimit({
      perpMarket,
      side,
      capPriceUi: rawCapPriceUi,
    });
    const orderSizing = computeOrderBaseLots({
      perpUiPrice: perpMarket.uiPrice,
      perpUiBaseToLots: (baseUi: number) => perpMarket.uiBaseToLots(baseUi),
      marketIndex,
    });
    const clientOrderId = Date.now() * 100 + (tick % 100);

    const remainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
      client: bot.client,
      group,
      mangoAccount,
      marketIndex,
      userOwner: bot.user.publicKey,
    });

    const priceLots = perpMarket.uiPriceToLotsForSide(capPriceUi, side);
    const priceLotsNum = Number(priceLots.toString());
    if (!Number.isFinite(priceLotsNum) || priceLotsNum <= 0) {
      console.log(
        JSON.stringify({
          event: 'taker_bot_skip',
          tick,
          bot_name: bot.name,
          owner: bot.ownerPk.toBase58(),
          mango_account: mangoAccount.publicKey.toBase58(),
          market_index: marketIndex,
          market_name: marketRuntime.name,
          reason: 'invalid_price_lots',
          oracle_price_ui: perpMarket.uiPrice,
          best_bid_ui: bestBidUi ?? null,
          best_ask_ui: bestAskUi ?? null,
          cap_price_ui: capPriceUi,
        }),
      );
      await sleep(INTERVAL_MS);
      continue;
    }

    if (orderSizing.baseLots <= 0n || orderSizing.sizeUi <= 0) {
      console.log(
        JSON.stringify({
          event: 'taker_bot_skip',
          tick,
          bot_name: bot.name,
          owner: bot.ownerPk.toBase58(),
          mango_account: mangoAccount.publicKey.toBase58(),
          market_index: marketIndex,
          market_name: marketRuntime.name,
          reason: 'invalid_order_size',
          target_notional_usd: orderSizing.targetNotionalUsd,
          oracle_price_ui: perpMarket.uiPrice,
          computed_size_ui: orderSizing.sizeUi,
          computed_base_lots: orderSizing.baseLots.toString(),
        }),
      );
      await sleep(INTERVAL_MS);
      continue;
    }

    const payload = encodePerpPlaceOrderV2QueuePayload({
      side,
      priceLots: BigInt(priceLots.toString()),
      maxBaseLots: orderSizing.baseLots,
      maxQuoteLots: BigInt(I64_MAX_BN.toString()),
      clientOrderId,
      orderType: PerpOrderType.immediateOrCancel,
      selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: ORDER_LIMIT,
    });

    const currentPosUi = currentPositionUi(bot, marketIndex);
    console.log(
      JSON.stringify({
        event: 'taker_bot_submit',
        tick,
        bot_name: bot.name,
        owner: bot.ownerPk.toBase58(),
        mango_account: mangoAccount.publicKey.toBase58(),
        market_index: marketIndex,
        market_name: marketRuntime.name,
        side: sideLabel,
        size_ui: orderSizing.sizeUi,
        target_notional_usd: orderSizing.targetNotionalUsd,
        computed_base_lots: orderSizing.baseLots.toString(),
        slippage_bps: SLIPPAGE_BPS,
        oracle_price_ui: perpMarket.uiPrice,
        best_bid_ui: bestBidUi ?? null,
        best_ask_ui: bestAskUi ?? null,
        raw_cap_price_ui: rawCapPriceUi,
        cap_price_ui: capPriceUi,
        price_limit_lower_ui:
          perpMarket.maintBaseAssetWeight.toNumber() * perpMarket.uiPrice,
        price_limit_upper_ui:
          perpMarket.maintBaseLiabWeight.toNumber() * perpMarket.uiPrice,
        inside_price_limit: perpMarket.insidePriceLimit(side, capPriceUi),
        client_order_id: clientOrderId,
        base_position_ui: currentPosUi,
        dispatch_queue_remaining: dispatchQueue.length,
        harness_position_age_ms:
          bot.lastHarnessPollMs > 0 ? Date.now() - bot.lastHarnessPollMs : null,
        skew_mode:
          Math.abs(currentPosUi) >= SKEW_HARD_LIMIT_UI
            ? 'hard_reduce'
            : Math.abs(currentPosUi) >= SKEW_SOFT_LIMIT_UI
              ? 'soft_reduce'
              : 'balanced',
      }),
    );

    try {
      const response = await submitIntentViaRelayer({
        group: group.publicKey,
        executionQueue,
        marketIndex,
        mangoAccount: mangoAccount.publicKey,
        user: bot.user,
        payload,
        remainingAccounts,
        relayerAddr,
      });

      console.log(
        JSON.stringify({
          event: 'taker_bot_submit_result',
          tick,
          bot_name: bot.name,
          owner: bot.ownerPk.toBase58(),
          mango_account: mangoAccount.publicKey.toBase58(),
          market_index: marketIndex,
          market_name: marketRuntime.name,
          side: sideLabel,
          client_order_id: clientOrderId,
          sequence: response.sequence,
          enqueue_tx_signature: response.tx_signature ?? null,
        }),
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.log(
        JSON.stringify({
          event: 'taker_bot_submit_error',
          tick,
          bot_name: bot.name,
          owner: bot.ownerPk.toBase58(),
          mango_account: mangoAccount.publicKey.toBase58(),
          market_index: marketIndex,
          market_name: marketRuntime.name,
          side: sideLabel,
          client_order_id: clientOrderId,
          error: message,
        }),
      );
    }

    if (MAX_TICKS === 0 || tick < MAX_TICKS) {
      await sleep(INTERVAL_MS);
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
