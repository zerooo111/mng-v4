import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { AccountMeta, Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { MangoClient } from '../../src/client';
import {
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

type TakerBotRuntime = {
  name: string;
  user: Keypair;
  client: MangoClient;
  mangoAccountPk: PublicKey;
  ownerPk: PublicKey;
  cachedBasePositionLots: bigint;
  lastHarnessPollMs: number;
};

const RUNTIME_ROOT = path.resolve(__dirname, '../../../..');
const DEFAULT_CONFIG_PATH = path.resolve(
  RUNTIME_ROOT,
  '.devnet/run/execution-queue-e2e-9126.json',
);
const DEFAULT_BOTS_CONFIG_PATH = path.resolve(
  RUNTIME_ROOT,
  '.devnet/run/taker-bots-9126.json',
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
const PERP_MARKET_INDEX_OVERRIDE = process.env.PERP_MARKET_INDEX;
const INTERVAL_MS = Number(process.env.TAKER_INTERVAL_MS || '5000');
const MAX_TICKS = Number(process.env.TAKER_MAX_TICKS || '0');
const SIZE_UI = Number(process.env.TAKER_SIZE_UI || '0.1');
const SLIPPAGE_BPS = Number(process.env.TAKER_SLIPPAGE_BPS || '2000');
const ORDER_LIMIT = Number(process.env.TAKER_ORDER_LIMIT || '20');

// Harness position-tracking / skew config
const HARNESS_URL = process.env.TAKER_HARNESS_URL || 'http://127.0.0.1:9091';
const HARNESS_POLL_MS = Number(process.env.TAKER_HARNESS_POLL_MS || '20000');
const SKEW_SOFT_LIMIT_UI = Number(process.env.TAKER_SKEW_SOFT_LIMIT_UI || '5');
const SKEW_HARD_LIMIT_UI = Number(process.env.TAKER_SKEW_HARD_LIMIT_UI || '50');

// Shared lot size cache. Market is the same for every taker account.
let cachedBaseLotSize = 0.01;
let dispatchQueue: TakerBotRuntime[] = [];

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

function shuffleBots(bots: TakerBotRuntime[]): TakerBotRuntime[] {
  const out = [...bots];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

function refillDispatchQueue(bots: TakerBotRuntime[]): void {
  dispatchQueue = shuffleBots(bots);
  console.log(
    JSON.stringify({
      event: 'taker_bot_v2_cycle_refresh',
      dispatch_mode: 'random-cycle',
      order: dispatchQueue.map((bot) => bot.name),
    }),
  );
}

function selectBotForTick(bots: TakerBotRuntime[]): TakerBotRuntime {
  if (dispatchQueue.length === 0) {
    refillDispatchQueue(bots);
  }
  const next = dispatchQueue.shift();
  if (!next) {
    throw new Error('dispatch queue unexpectedly empty');
  }
  return next;
}

function currentPositionUi(bot: TakerBotRuntime): number {
  return Number(bot.cachedBasePositionLots) * cachedBaseLotSize;
}

function chooseSide(bot: TakerBotRuntime): PerpOrderSide {
  const positionUi = currentPositionUi(bot);
  const absPos = Math.abs(positionUi);

  // Hard limit: only quote reducing side
  if (absPos >= SKEW_HARD_LIMIT_UI) {
    return positionUi > 0 ? PerpOrderSide.ask : PerpOrderSide.bid;
  }

  // Soft limit: bias 75% toward reducing side
  if (absPos >= SKEW_SOFT_LIMIT_UI) {
    const reducingSide =
      positionUi > 0 ? PerpOrderSide.ask : PerpOrderSide.bid;
    const addingSide =
      positionUi > 0 ? PerpOrderSide.bid : PerpOrderSide.ask;
    return Math.random() < 0.75 ? reducingSide : addingSide;
  }

  // Within limits: random 50/50
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

  // For IOC taker orders: use book price when available (handles stale oracle),
  // apply slippage from the book price so we cross the spread aggressively.
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

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
  marketIndex: number;
  userOwner: PublicKey;
}): Promise<AccountMeta[]> {
  const perpMarket = params.group.getPerpMarketByMarketIndex(
    params.marketIndex as PerpMarketIndex,
  );
  return params.client
    .buildHealthRemainingAccounts(
      params.group,
      [params.mangoAccount],
      [params.group.getFirstBankForPerpSettlement()],
      [perpMarket],
    )
    .then((healthRemainingAccounts) => [
      { pubkey: params.group.publicKey, isSigner: false, isWritable: false },
      { pubkey: params.mangoAccount.publicKey, isSigner: false, isWritable: true },
      { pubkey: params.userOwner, isSigner: false, isWritable: false },
      { pubkey: perpMarket.publicKey, isSigner: false, isWritable: true },
      { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
      { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
      { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
      { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
      ...healthRemainingAccounts.map((pubkey) => ({
        pubkey,
        isSigner: false,
        isWritable: false,
      })),
    ]);
}

async function submitIntentViaRelayer(params: {
  group: PublicKey;
  executionQueue: PublicKey;
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
        market: '0',
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

async function pollHarnessPosition(bot: TakerBotRuntime): Promise<void> {
  try {
    const data = await httpGetJson(
      `${HARNESS_URL}/state/users/${bot.ownerPk.toBase58()}?view=confirmed`,
    );

    const perMarket = data?.data?.per_market;
    if (Array.isArray(perMarket) && perMarket.length > 0) {
      bot.cachedBasePositionLots = BigInt(perMarket[0].base_position_lots ?? '0');
    }

    // Refresh lot size from market metadata once
    if (cachedBaseLotSize === 0.01) {
      try {
        const mkt = await httpGetJson(
          `${HARNESS_URL}/state/markets/0?view=confirmed`,
        );
        const rawLotSize = mkt?.metadata?.base_lot_size;
        const baseDecimals = Number(mkt?.metadata?.base_decimals ?? 4);
        if (rawLotSize) {
          cachedBaseLotSize = Number(rawLotSize) / 10 ** baseDecimals;
        }
      } catch {
        // keep default
      }
    }

    bot.lastHarnessPollMs = Date.now();
    console.log(
      JSON.stringify({
        event: 'harness_position_poll',
        bot_name: bot.name,
        owner: bot.ownerPk.toBase58(),
        base_position_lots: bot.cachedBasePositionLots.toString(),
        base_position_ui: currentPositionUi(bot),
        lot_size: cachedBaseLotSize,
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

  return {
    name: params.spec.name,
    user,
    client,
    mangoAccountPk: new PublicKey(params.spec.mangoAccount),
    ownerPk,
    cachedBasePositionLots: 0n,
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
  const perpMarketIndex = Number(
    PERP_MARKET_INDEX_OVERRIDE ?? String(config.perpMarketIndex),
  );
  const connection = new Connection(
    clusterUrl,
    AnchorProvider.defaultOptions(),
  );

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

  console.log(
    JSON.stringify({
      event: 'taker_bot_v2_start',
      config_path: CONFIG_PATH,
      bots_config_path: BOTS_CONFIG_PATH,
      cluster,
      cluster_url: clusterUrl,
      program_id: programId.toBase58(),
      execution_queue: executionQueue.toBase58(),
      bot_count: bots.length,
      owners: bots.map((bot) => bot.ownerPk.toBase58()),
      interval_ms: INTERVAL_MS,
      size_ui: SIZE_UI,
      slippage_bps: SLIPPAGE_BPS,
      max_ticks: MAX_TICKS,
      harness_url: HARNESS_URL,
      harness_poll_ms: HARNESS_POLL_MS,
      skew_soft_limit_ui: SKEW_SOFT_LIMIT_UI,
      skew_hard_limit_ui: SKEW_HARD_LIMIT_UI,
      dispatch_mode: 'random-cycle',
    }),
  );

  for (const bot of bots) {
    startHarnessPoller(bot);
  }

  let tick = 0;
  while (MAX_TICKS === 0 || tick < MAX_TICKS) {
    tick += 1;

    const bot = selectBotForTick(bots);
    const mangoAccount = await bot.client.getMangoAccount(bot.mangoAccountPk);
    const group = await bot.client.getGroup(mangoAccount.group);
    const perpMarket = group.getPerpMarketByMarketIndex(
      perpMarketIndex as PerpMarketIndex,
    );
    const [bids, asks] = await Promise.all([
      perpMarket.loadBids(bot.client, true),
      perpMarket.loadAsks(bot.client, true),
    ]);
    const bestBidUi = bids.best()?.uiPrice;
    const bestAskUi = asks.best()?.uiPrice;
    const side = chooseSide(bot);
    const sideLabel = side === PerpOrderSide.bid ? 'buy' : 'sell';
    const capPriceUi = computeCapPriceUi({
      side,
      oraclePriceUi: perpMarket.uiPrice,
      bestBidUi,
      bestAskUi,
      slippageBps: SLIPPAGE_BPS,
    });
    const clientOrderId = Date.now() * 100 + (tick % 100);

    const remainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
      client: bot.client,
      group,
      mangoAccount,
      marketIndex: perpMarketIndex,
      userOwner: bot.user.publicKey,
    });

    const priceLotsNum = Number(perpMarket.uiPriceToLots(capPriceUi).toString());
    if (!Number.isFinite(priceLotsNum) || priceLotsNum <= 0) {
      console.log(
        JSON.stringify({
          event: 'taker_bot_skip',
          tick,
          bot_name: bot.name,
          owner: bot.ownerPk.toBase58(),
          mango_account: mangoAccount.publicKey.toBase58(),
          reason: 'invalid_price_lots',
          cap_price_ui: capPriceUi,
        }),
      );
      await sleep(INTERVAL_MS);
      continue;
    }

    const payload = encodePerpPlaceOrderV2QueuePayload({
      side,
      priceLots: BigInt(perpMarket.uiPriceToLots(capPriceUi).toString()),
      maxBaseLots: BigInt(perpMarket.uiBaseToLots(SIZE_UI).toString()),
      maxQuoteLots: BigInt(I64_MAX_BN.toString()),
      clientOrderId,
      orderType: PerpOrderType.immediateOrCancel,
      selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: ORDER_LIMIT,
    });

    const currentPosUi = currentPositionUi(bot);
    console.log(
      JSON.stringify({
        event: 'taker_bot_submit',
        tick,
        bot_name: bot.name,
        owner: bot.ownerPk.toBase58(),
        mango_account: mangoAccount.publicKey.toBase58(),
        side: sideLabel,
        size_ui: SIZE_UI,
        slippage_bps: SLIPPAGE_BPS,
        oracle_price_ui: perpMarket.uiPrice,
        best_bid_ui: bestBidUi ?? null,
        best_ask_ui: bestAskUi ?? null,
        cap_price_ui: capPriceUi,
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
