import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Cluster,
  Commitment,
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import {
  PerpMarketIndex,
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import {
  buildExecutionQueueUserIntent,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';

dotenv.config();

type BotSpec = {
  name: string;
  keypairPath: string;
  mangoAccount: string;
  side: PerpOrderSide;
};

type BotRuntime = {
  name: string;
  keypair: Keypair;
  mangoAccountPk: PublicKey;
  side: PerpOrderSide;
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
};

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  perpMarketIndex: number;
  solMint?: string;
  maker?: {
    keypairPath?: string;
    mangoAccount?: string;
  };
  taker?: {
    keypairPath?: string;
    mangoAccount?: string;
  };
  relayer?: {
    bindAddr?: string;
  };
};

const CONFIG_PATH =
  process.env.QUOTER_CONFIG_PATH ||
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  '/tmp/execution-queue-e2e-9101.json';
const CLUSTER_URL_OVERRIDE = process.env.CLUSTER_URL_OVERRIDE;
const RELAYER_ADDR_OVERRIDE = process.env.CTM_RELAYER_ADDR;
const COMMITMENT: Commitment =
  (process.env.QUOTER_COMMITMENT as Commitment) || 'confirmed';
const INTERVAL_MS = Number(process.env.QUOTER_INTERVAL_MS || '5000');
const PRICE_RANGE_BPS = Number(process.env.QUOTER_PRICE_RANGE_BPS || '200');
const SIZE_MIN_SOL = Number(process.env.QUOTER_SIZE_MIN_SOL || '1');
const SIZE_MAX_SOL = Number(process.env.QUOTER_SIZE_MAX_SOL || '3');
const MIN_EXECUTE_SLOT_OFFSET = BigInt(
  process.env.QUOTER_MIN_EXECUTE_SLOT_OFFSET || '1',
);
const CANCEL_BEFORE_PLACE =
  (process.env.QUOTER_CANCEL_BEFORE_PLACE || 'true') === 'true';
const CANCEL_LIMIT = Number(process.env.QUOTER_CANCEL_LIMIT || '255');
const ORDER_LIMIT = Number(process.env.QUOTER_ORDER_LIMIT || '20');
const QUOTE_BUDGET_MULTIPLIER = Number(
  process.env.QUOTER_QUOTE_BUDGET_MULTIPLIER || '1.25',
);
const COINGECKO_API_BASE =
  process.env.QUOTER_COINGECKO_API_BASE || 'https://api.coingecko.com/api/v3';
const COINGECKO_ASSET_ID =
  process.env.QUOTER_COINGECKO_ASSET_ID || 'solana';
const COINGECKO_VS_CURRENCY =
  process.env.QUOTER_COINGECKO_VS_CURRENCY || 'usd';
const COINGECKO_TIMEOUT_MS = Number(
  process.env.QUOTER_COINGECKO_TIMEOUT_MS || '1500',
);
const BOT_SIDES = (process.env.QUOTER_BOT_SIDES || 'bid,ask')
  .split(',')
  .map((v) => v.trim().toLowerCase())
  .filter((v) => v.length > 0);

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function sideFromString(input: string): PerpOrderSide {
  if (input === 'bid' || input === 'buy') {
    return PerpOrderSide.bid;
  }
  if (input === 'ask' || input === 'sell') {
    return PerpOrderSide.ask;
  }
  throw new Error(`invalid side ${input}`);
}

function sideToString(side: PerpOrderSide): 'bid' | 'ask' {
  return side === PerpOrderSide.bid ? 'bid' : 'ask';
}

function executionQueueRemainingAccountsFromMangoIx(
  executionQueue: PublicKey,
  keys: { pubkey: PublicKey; isWritable: boolean; isSigner: boolean }[],
): AccountMeta[] {
  if (keys.length < 3) {
    throw new Error('expected at least 3 metas in mango instruction');
  }
  const remaining = keys.map((k) => ({
    pubkey: k.pubkey,
    isWritable: k.isWritable,
    isSigner: k.isSigner,
  }));
  remaining[2] = {
    pubkey: executionQueue,
    isWritable: remaining[2].isWritable,
    isSigner: false,
  };
  return remaining;
}

function randomFloat(min: number, max: number): number {
  return min + Math.random() * (max - min);
}

function randomSizeSol(): number {
  return Number(randomFloat(SIZE_MIN_SOL, SIZE_MAX_SOL).toFixed(3));
}

function randomQuotePrice(
  referencePrice: number,
  side: PerpOrderSide,
): number {
  const fraction = randomFloat(0, PRICE_RANGE_BPS / 10_000);
  const multiplier =
    side === PerpOrderSide.bid ? 1 - fraction : 1 + fraction;
  return Number((referencePrice * multiplier).toFixed(4));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function submitIntentViaRelayer(params: {
  relayerClient: any;
  group: PublicKey;
  executionQueue: PublicKey;
  market: number;
  payload: Buffer;
  remainingAccounts: AccountMeta[];
  userOwner: PublicKey;
  userSecretKey: Uint8Array;
  mangoAccount: PublicKey;
  minExecuteSlot: bigint;
}): Promise<{ sequence: string; tx_signature: string }> {
  const intent = await buildExecutionQueueUserIntent({
    group: params.group,
    executionQueue: params.executionQueue,
    mangoAccount: params.mangoAccount,
    userOwner: params.userOwner,
    payload: params.payload,
    remainingAccounts: params.remainingAccounts,
  });
  const userSignature = signExecutionQueueIntentMessage(
    params.userSecretKey,
    intent.userIntentMessage,
  );

  return await new Promise<{ sequence: string; tx_signature: string }>(
    (resolve, reject) => {
      params.relayerClient.submitIntent(
        {
          group: params.group.toBase58(),
          execution_queue: params.executionQueue.toBase58(),
          market: new BN(params.market).toString(),
          payload: params.payload,
          remaining_accounts: params.remainingAccounts.map((a) => ({
            pubkey: a.pubkey.toBase58(),
            is_signer: !!a.isSigner,
            is_writable: !!a.isWritable,
          })),
          min_execute_slot: params.minExecuteSlot.toString(),
          expires_at_slot: '0',
          user_owner: params.userOwner.toBase58(),
          mango_account: params.mangoAccount.toBase58(),
          user_signature: Buffer.from(userSignature),
        },
        (
          err: Error | null,
          res: { sequence: string; tx_signature: string },
        ) => {
          if (err) {
            reject(err);
            return;
          }
          resolve(res);
        },
      );
    },
  );
}

function loadBotSpecs(config: E2EConfig): BotSpec[] {
  const specs: BotSpec[] = [];
  if (config.maker?.keypairPath && config.maker?.mangoAccount) {
    specs.push({
      name: 'maker-bot',
      keypairPath: config.maker.keypairPath,
      mangoAccount: config.maker.mangoAccount,
      side: sideFromString(BOT_SIDES[0] || 'bid'),
    });
  }
  if (config.taker?.keypairPath && config.taker?.mangoAccount) {
    specs.push({
      name: 'taker-bot',
      keypairPath: config.taker.keypairPath,
      mangoAccount: config.taker.mangoAccount,
      side: sideFromString(BOT_SIDES[1] || 'ask'),
    });
  }
  if (specs.length === 0) {
    throw new Error(
      'no bots found in config; expected maker/taker keypair + mangoAccount',
    );
  }
  return specs;
}

function getOnchainReferencePriceUi(params: {
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  marketIndex: PerpMarketIndex;
  solMint: PublicKey | null;
}): number {
  if (params.solMint) {
    try {
      const solBank = params.group.getFirstBankByMint(params.solMint);
      return solBank.uiPrice;
    } catch {
      // Fallback to perp oracle below.
    }
  }
  const perpMarket = params.group.getPerpMarketByMarketIndex(params.marketIndex);
  return perpMarket.uiPrice;
}

async function fetchCoinGeckoReferencePriceUi(): Promise<number> {
  const base = COINGECKO_API_BASE.endsWith('/')
    ? COINGECKO_API_BASE.slice(0, -1)
    : COINGECKO_API_BASE;
  const query = new URLSearchParams({
    ids: COINGECKO_ASSET_ID,
    vs_currencies: COINGECKO_VS_CURRENCY,
  });
  const url = `${base}/simple/price?${query.toString()}`;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), COINGECKO_TIMEOUT_MS);
  try {
    const res = await fetch(url, { signal: controller.signal });
    if (!res.ok) {
      throw new Error(`coingecko non-200 status ${res.status}`);
    }
    const parsed = (await res.json()) as Record<string, Record<string, number>>;
    const value = parsed?.[COINGECKO_ASSET_ID]?.[COINGECKO_VS_CURRENCY];
    if (!Number.isFinite(value) || value <= 0) {
      throw new Error('coingecko returned invalid price');
    }
    return value;
  } finally {
    clearTimeout(timer);
  }
}

async function main(): Promise<void> {
  if (INTERVAL_MS < 1000) {
    throw new Error('QUOTER_INTERVAL_MS must be >= 1000');
  }
  if (PRICE_RANGE_BPS <= 0 || PRICE_RANGE_BPS > 1000) {
    throw new Error('QUOTER_PRICE_RANGE_BPS must be in (0, 1000]');
  }
  if (SIZE_MIN_SOL <= 0 || SIZE_MAX_SOL < SIZE_MIN_SOL) {
    throw new Error('invalid QUOTER_SIZE_MIN_SOL / QUOTER_SIZE_MAX_SOL');
  }

  const config = JSON.parse(
    fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8'),
  ) as E2EConfig;
  const cluster = config.cluster;
  const clusterUrl = CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const relayerAddr = RELAYER_ADDR_OVERRIDE || config.relayer?.bindAddr || '127.0.0.1:9090';
  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const marketIndex = config.perpMarketIndex as PerpMarketIndex;
  const solMintPk = config.solMint ? new PublicKey(config.solMint) : null;

  const connection = new Connection(clusterUrl, COMMITMENT);
  const botSpecs = loadBotSpecs(config);

  const bots: BotRuntime[] = [];
  for (const spec of botSpecs) {
    const keypair = readKeypair(spec.keypairPath);
    const provider = new AnchorProvider(
      connection,
      new Wallet(keypair),
      AnchorProvider.defaultOptions(),
    );
    const client = await MangoClient.connect(provider, cluster, new PublicKey(config.programId), {
      idsSource: 'get-program-accounts',
    });
    const group = await client.getGroup(groupPk);
    bots.push({
      name: spec.name,
      keypair,
      mangoAccountPk: new PublicKey(spec.mangoAccount),
      side: spec.side,
      client,
      group,
    });
  }

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  const buildRelayerClient = () =>
    new proto.ctmsequencer.CtmSequencerRelayer(
      relayerAddr,
      grpc.credentials.createInsecure(),
    );
  let relayerClient = buildRelayerClient();

  let running = true;
  const shutdown = () => {
    running = false;
    relayerClient.close();
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);

  console.log(
    JSON.stringify({
      msg: 'random-sol-usdc-quoter-bot started',
      clusterUrl,
      relayerAddr,
      intervalMs: INTERVAL_MS,
      priceRangeBps: PRICE_RANGE_BPS,
      sizeMinSol: SIZE_MIN_SOL,
      sizeMaxSol: SIZE_MAX_SOL,
      referencePrice: {
        provider: 'coingecko',
        assetId: COINGECKO_ASSET_ID,
        vsCurrency: COINGECKO_VS_CURRENCY,
        apiBase: COINGECKO_API_BASE,
      },
      bots: bots.map((b) => ({
        name: b.name,
        owner: b.keypair.publicKey.toBase58(),
        mangoAccount: b.mangoAccountPk.toBase58(),
        side: sideToString(b.side),
      })),
    }),
  );

  while (running) {
    const tickStart = Date.now();
    try {
      await bots[0].group.reloadAll(bots[0].client);
      let referencePrice: number;
      let referenceSource: 'coingecko' | 'onchain-fallback' = 'coingecko';
      try {
        referencePrice = await fetchCoinGeckoReferencePriceUi();
      } catch (e) {
        referencePrice = getOnchainReferencePriceUi({
          group: bots[0].group,
          marketIndex,
          solMint: solMintPk,
        });
        referenceSource = 'onchain-fallback';
        console.warn(
          JSON.stringify({
            ts: new Date().toISOString(),
            msg: 'coingecko price fetch failed; using onchain fallback',
            error: e instanceof Error ? e.message : `${e}`,
          }),
        );
      }
      const minExecuteSlot =
        BigInt(await connection.getSlot(COMMITMENT)) + MIN_EXECUTE_SLOT_OFFSET;

      for (const bot of bots) {
        const mangoAccount = await bot.client.getMangoAccount(bot.mangoAccountPk);
        const perpMarket = bot.group.getPerpMarketByMarketIndex(marketIndex);

        if (CANCEL_BEFORE_PLACE) {
          const cancelIx = await bot.client.perpCancelAllOrdersIx(
            bot.group,
            mangoAccount,
            marketIndex,
            CANCEL_LIMIT,
          );
          const cancelRemaining = executionQueueRemainingAccountsFromMangoIx(
            executionQueuePk,
            cancelIx.keys,
          );
          const cancelPayload = encodePerpCancelAllOrdersQueuePayload({
            limit: CANCEL_LIMIT,
          });
          await submitIntentViaRelayer({
            relayerClient,
            group: bot.group.publicKey,
            executionQueue: executionQueuePk,
            market: marketIndex,
            payload: cancelPayload,
            remainingAccounts: cancelRemaining,
            userOwner: bot.keypair.publicKey,
            userSecretKey: bot.keypair.secretKey,
            mangoAccount: mangoAccount.publicKey,
            minExecuteSlot,
          });
        }

        const sizeSol = randomSizeSol();
        const quotePrice = randomQuotePrice(referencePrice, bot.side);
        const maxQuoteQty = Number((quotePrice * sizeSol * QUOTE_BUDGET_MULTIPLIER).toFixed(6));
        const clientOrderId = Date.now() * 1000 + Math.floor(Math.random() * 1000);

        const placeIx = await bot.client.perpPlaceOrderV2Ix(
          bot.group,
          mangoAccount,
          marketIndex,
          bot.side,
          quotePrice,
          sizeSol,
          maxQuoteQty,
          clientOrderId,
          PerpOrderType.limit,
          PerpSelfTradeBehavior.decrementTake,
          false,
          0,
          ORDER_LIMIT,
        );
        const remainingAccounts = executionQueueRemainingAccountsFromMangoIx(
          executionQueuePk,
          placeIx.keys,
        );
        const payload = encodePerpPlaceOrderV2QueuePayload({
          side: bot.side,
          priceLots: BigInt(perpMarket.uiPriceToLots(quotePrice).toString()),
          maxBaseLots: BigInt(perpMarket.uiBaseToLots(sizeSol).toString()),
          maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(maxQuoteQty).toString()),
          clientOrderId,
          orderType: PerpOrderType.limit,
          selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
          reduceOnly: false,
          expiryTimestamp: 0,
          limit: ORDER_LIMIT,
        });

        const resp = await submitIntentViaRelayer({
          relayerClient,
          group: bot.group.publicKey,
          executionQueue: executionQueuePk,
          market: marketIndex,
          payload,
          remainingAccounts,
          userOwner: bot.keypair.publicKey,
          userSecretKey: bot.keypair.secretKey,
          mangoAccount: mangoAccount.publicKey,
          minExecuteSlot,
        });

        console.log(
          JSON.stringify({
            ts: new Date().toISOString(),
            bot: bot.name,
            side: sideToString(bot.side),
            referencePrice,
            referenceSource,
            quotePrice,
            sizeSol,
            maxQuoteQty,
            sequence: resp.sequence,
            txSignature: resp.tx_signature,
          }),
        );
      }
    } catch (err) {
      const errText = err instanceof Error ? err.message : `${err}`;
      if (errText.includes('UNAVAILABLE')) {
        try {
          relayerClient.close();
        } catch {
          // no-op
        }
        relayerClient = buildRelayerClient();
      }
      console.error(
        JSON.stringify({
          ts: new Date().toISOString(),
          msg: 'quote tick failed',
          error: errText,
        }),
      );
    }

    const elapsed = Date.now() - tickStart;
    const waitMs = Math.max(0, INTERVAL_MS - elapsed);
    await sleep(waitMs);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
