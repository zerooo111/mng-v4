import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { AccountMeta, Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
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
import { MangoAccount } from '../../src/accounts/mangoAccount';
import { Group } from '../../src/accounts/group';
import { MangoClient } from '../../src/client';
import {
  buildExecutionQueueUserIntent,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { decodeExecutionQueueCount } from '../../src/executionQueueLayout';
import { runtimeConfigPath } from './scriptEnv';

dotenv.config();

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  executionQueueBuffer?: string;
  usdcMint: string;
  perpMarketIndex: number;
  maker: {
    keypairPath: string;
    owner: string;
    mangoAccount: string;
  };
  taker: {
    keypairPath: string;
    owner: string;
    mangoAccount: string;
  };
  relayer: {
    bindAddr: string;
  };
};

type SubmitIntentResponse = {
  sequence: string;
  tx_signature: string;
};

type AccountSnapshot = {
  owner: string;
  mangoAccount: string;
  baseLots: string;
  quotePositionNative: string;
  unsettledPnlUi: number;
  unsettledFundingUi: number;
  settleablePnlUi: number;
  usdcBalanceUi: number;
  openOrdersCount: number;
};

type TradeSubmissionSummary =
  | {
      mode: 'relayer';
      maker: SubmitIntentResponse;
      taker: SubmitIntentResponse;
    }
  | {
      mode: 'direct';
      maker: { signature: string };
      taker: { signature: string };
      fallbackReason: string;
    };

const CONFIG_PATH =
  process.env.E2E_OUTPUT_CONFIG_PATH || runtimeConfigPath('execution-queue-e2e-9101.json');
const RELAYER_ADDR_OVERRIDE = process.env.CTM_RELAYER_ADDR;
const POLL_MS = Number(process.env.E2E_POLL_MS || '1000');
const QUEUE_EMPTY_TIMEOUT_MS = Number(process.env.E2E_QUEUE_EMPTY_TIMEOUT_MS || '90000');
const PRICE_DISCOUNT_BPS = Number(process.env.E2E_PNL_PRICE_DISCOUNT_BPS || '500');
const TRADE_QTY = Number(process.env.E2E_PNL_TRADE_QTY || '1');
const MAX_QUOTE_QTY = Number(process.env.E2E_PNL_MAX_QUOTE_QTY || '1000');
const ORDER_LIMIT = Number(process.env.E2E_PNL_ORDER_LIMIT || '20');
const MIN_PNL_UI = Number(process.env.E2E_PNL_MIN_UI || '0.01');
const RELAYER_RETRY_ATTEMPTS = Number(process.env.E2E_PNL_RELAYER_RETRY_ATTEMPTS || '3');
const RELAYER_RETRY_DELAY_MS = Number(process.env.E2E_PNL_RELAYER_RETRY_DELAY_MS || '500');

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: Group;
  mangoAccount: MangoAccount;
  marketIndex: PerpMarketIndex;
  userOwner: PublicKey;
}): Promise<AccountMeta[]> {
  const perpMarket = params.group.getPerpMarketByMarketIndex(params.marketIndex);
  const healthRemainingAccounts = await params.client.buildHealthRemainingAccounts(
    params.group,
    [params.mangoAccount],
    [params.group.getFirstBankForPerpSettlement()],
    [perpMarket],
  );

  return [
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
  ];
}

async function waitForQueueToDrain(
  connection: Connection,
  executionQueue: PublicKey,
): Promise<void> {
  const deadline = Date.now() + QUEUE_EMPTY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const ai = await connection.getAccountInfo(executionQueue, 'confirmed');
    const count = ai?.data ? decodeExecutionQueueCount(ai.data) : 0;
    if (count === 0) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
  throw new Error('timed out waiting for execution queue to drain');
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
}): Promise<SubmitIntentResponse> {
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

  let attempt = 0;
  while (true) {
    attempt += 1;
    try {
      return await new Promise<SubmitIntentResponse>((resolve, reject) => {
        params.relayerClient.submitIntent(
          {
            group: params.group.toBase58(),
            execution_queue: params.executionQueue.toBase58(),
            market: params.market.toString(),
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
          (err: Error | null, res: SubmitIntentResponse) => {
            if (err) {
              reject(err);
              return;
            }
            resolve(res);
          },
        );
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : `${err}`;
      if (
        msg.includes('duplicate sequence') &&
        attempt < RELAYER_RETRY_ATTEMPTS
      ) {
        await new Promise((resolve) => setTimeout(resolve, RELAYER_RETRY_DELAY_MS));
        continue;
      }
      throw err;
    }
  }
}

async function loadOpenOrdersCount(
  client: MangoClient,
  group: Group,
  account: MangoAccount,
  marketIndex: PerpMarketIndex,
): Promise<number> {
  const fresh = await client.getMangoAccount(account.publicKey);
  const openOrders = await fresh.loadPerpOpenOrdersForMarket(client, group, marketIndex, true);
  return openOrders.length;
}

async function cancelAllIfNeeded(
  client: MangoClient,
  group: Group,
  account: MangoAccount,
  marketIndex: PerpMarketIndex,
): Promise<void> {
  const openCount = await loadOpenOrdersCount(client, group, account, marketIndex);
  if (openCount === 0) {
    return;
  }
  await client.perpCancelAllOrders(group, account, marketIndex, 255);
  await new Promise((resolve) => setTimeout(resolve, 2000));
}

async function snapshotAccount(params: {
  client: MangoClient;
  group: Group;
  accountPk: PublicKey;
  owner: PublicKey;
  usdcMint: PublicKey;
  marketIndex: PerpMarketIndex;
}): Promise<{ account: MangoAccount; snapshot: AccountSnapshot }> {
  const account = await params.client.getMangoAccount(params.accountPk);
  await account.reload(params.client);
  const perpMarket = params.group.getPerpMarketByMarketIndex(params.marketIndex);
  const perpPosition = account.getPerpPosition(params.marketIndex);
  const usdcBank = params.group.getFirstBankByMint(params.usdcMint);
  const openOrdersCount = await loadOpenOrdersCount(
    params.client,
    params.group,
    account,
    params.marketIndex,
  );
  const snapshot: AccountSnapshot = {
    owner: params.owner.toBase58(),
    mangoAccount: account.publicKey.toBase58(),
    baseLots: perpPosition ? perpPosition.basePositionLots.toString() : '0',
    quotePositionNative: perpPosition ? perpPosition.quotePositionNative.toString() : '0',
    unsettledPnlUi: perpPosition ? perpPosition.getUnsettledPnlUi(perpMarket) : 0,
    unsettledFundingUi: perpPosition ? perpPosition.getUnsettledFundingUi(perpMarket) : 0,
    settleablePnlUi: perpPosition
      ? perpPosition.getSettleablePnlUi(params.group, perpMarket, account)
      : 0,
    usdcBalanceUi: account.getTokenBalanceUi(usdcBank),
    openOrdersCount,
  };
  return { account, snapshot };
}

function requireOpposingPnl(
  maker: AccountSnapshot,
  taker: AccountSnapshot,
): { profitable: 'maker' | 'taker'; unprofitable: 'maker' | 'taker' } {
  if (Math.abs(maker.unsettledPnlUi) < MIN_PNL_UI || Math.abs(taker.unsettledPnlUi) < MIN_PNL_UI) {
    throw new Error(
      `expected both accounts to have meaningful unsettled pnl, got maker=${maker.unsettledPnlUi} taker=${taker.unsettledPnlUi}`,
    );
  }
  if (maker.unsettledPnlUi > 0 && taker.unsettledPnlUi < 0) {
    return { profitable: 'maker', unprofitable: 'taker' };
  }
  if (maker.unsettledPnlUi < 0 && taker.unsettledPnlUi > 0) {
    return { profitable: 'taker', unprofitable: 'maker' };
  }
  throw new Error(
    `expected opposite pnl signs, got maker=${maker.unsettledPnlUi} taker=${taker.unsettledPnlUi}`,
  );
}

async function main(): Promise<void> {
  const config = JSON.parse(fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8')) as E2EConfig;

  const connection = new Connection(
    process.env.CLUSTER_URL_OVERRIDE || config.clusterUrl,
    AnchorProvider.defaultOptions(),
  );
  const programId = new PublicKey(config.programId);
  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const executionQueueBufferPk = new PublicKey(
    config.executionQueueBuffer || config.executionQueue,
  );
  const usdcMintPk = new PublicKey(config.usdcMint);
  const marketIndex = config.perpMarketIndex as PerpMarketIndex;

  const makerKp = readKeypair(config.maker.keypairPath);
  const takerKp = readKeypair(config.taker.keypairPath);

  const makerProvider = new AnchorProvider(
    connection,
    new Wallet(makerKp),
    AnchorProvider.defaultOptions(),
  );
  const takerProvider = new AnchorProvider(
    connection,
    new Wallet(takerKp),
    AnchorProvider.defaultOptions(),
  );
  const makerClient = await MangoClient.connect(
    makerProvider,
    config.cluster,
    programId,
    { idsSource: 'get-program-accounts' },
  );
  const takerClient = await MangoClient.connect(
    takerProvider,
    config.cluster,
    programId,
    { idsSource: 'get-program-accounts' },
  );

  const group = await makerClient.getGroup(groupPk);
  const makerAccountPk = new PublicKey(config.maker.mangoAccount);
  const takerAccountPk = new PublicKey(config.taker.mangoAccount);

  let makerState = await snapshotAccount({
    client: makerClient,
    group,
    accountPk: makerAccountPk,
    owner: makerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });
  let takerState = await snapshotAccount({
    client: takerClient,
    group,
    accountPk: takerAccountPk,
    owner: takerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });

  await cancelAllIfNeeded(makerClient, group, makerState.account, marketIndex);
  await cancelAllIfNeeded(takerClient, group, takerState.account, marketIndex);

  const beforeTrade = {
    maker: makerState.snapshot,
    taker: takerState.snapshot,
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
  const relayerClient = new proto.ctmsequencer.CtmSequencerRelayer(
    RELAYER_ADDR_OVERRIDE || config.relayer.bindAddr,
    grpc.credentials.createInsecure(),
  );

  await group.reloadAll(makerClient);
  const perpMarket = group.getPerpMarketByMarketIndex(marketIndex);
  const oracleUiPrice = perpMarket.uiPrice;
  const fillUiPrice = Number((oracleUiPrice * (1 - PRICE_DISCOUNT_BPS / 10_000)).toFixed(4));

  const makerRemainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
    client: makerClient,
    group,
    mangoAccount: makerState.account,
    marketIndex,
    userOwner: makerKp.publicKey,
  });
  const takerRemainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
    client: takerClient,
    group,
    mangoAccount: takerState.account,
    marketIndex,
    userOwner: takerKp.publicKey,
  });

  const clientOrderIdBase = Date.now();
  const makerPayload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.bid,
    priceLots: BigInt(perpMarket.uiPriceToLots(fillUiPrice).toString()),
    maxBaseLots: BigInt(perpMarket.uiBaseToLots(TRADE_QTY).toString()),
    maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(MAX_QUOTE_QTY).toString()),
    clientOrderId: clientOrderIdBase,
    orderType: PerpOrderType.limit,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: 0,
    limit: ORDER_LIMIT,
  });
  const takerPayload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.ask,
    priceLots: BigInt(perpMarket.uiPriceToLots(fillUiPrice).toString()),
    maxBaseLots: BigInt(perpMarket.uiBaseToLots(TRADE_QTY).toString()),
    maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(MAX_QUOTE_QTY).toString()),
    clientOrderId: clientOrderIdBase + 1,
    orderType: PerpOrderType.limit,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: 0,
    limit: ORDER_LIMIT,
  });

  const currentSlot = await connection.getSlot('confirmed');
  let tradeSubmissions: TradeSubmissionSummary;
  try {
    const makerResp = await submitIntentViaRelayer({
      relayerClient,
      group: groupPk,
      executionQueue: executionQueuePk,
      market: marketIndex,
      payload: makerPayload,
      remainingAccounts: makerRemainingAccounts,
      userOwner: makerKp.publicKey,
      userSecretKey: makerKp.secretKey,
      mangoAccount: makerAccountPk,
      minExecuteSlot: BigInt(currentSlot) + 1n,
    });
    const takerResp = await submitIntentViaRelayer({
      relayerClient,
      group: groupPk,
      executionQueue: executionQueuePk,
      market: marketIndex,
      payload: takerPayload,
      remainingAccounts: takerRemainingAccounts,
      userOwner: takerKp.publicKey,
      userSecretKey: takerKp.secretKey,
      mangoAccount: takerAccountPk,
      minExecuteSlot: BigInt(currentSlot) + 1n,
    });
    tradeSubmissions = {
      mode: 'relayer',
      maker: makerResp,
      taker: takerResp,
    };

    try {
      await makerClient.executionQueueExecute(
        group,
        executionQueuePk,
        executionQueueBufferPk,
        makerRemainingAccounts,
        1,
      );
    } catch {
      // Best effort: the embedded cranker may process the queue first.
    }

    await waitForQueueToDrain(connection, executionQueuePk);
  } catch (err) {
    const msg = err instanceof Error ? err.message : `${err}`;
    if (!msg.includes('duplicate sequence')) {
      throw err;
    }
    const makerDirect = await makerClient.perpPlaceOrder(
      group,
      makerState.account,
      marketIndex,
      PerpOrderSide.bid,
      fillUiPrice,
      TRADE_QTY,
      MAX_QUOTE_QTY,
      clientOrderIdBase,
      PerpOrderType.limit,
      false,
      0,
      ORDER_LIMIT,
    );
    const takerDirect = await takerClient.perpPlaceOrder(
      group,
      takerState.account,
      marketIndex,
      PerpOrderSide.ask,
      fillUiPrice,
      TRADE_QTY,
      MAX_QUOTE_QTY,
      clientOrderIdBase + 1,
      PerpOrderType.limit,
      false,
      0,
      ORDER_LIMIT,
    );
    tradeSubmissions = {
      mode: 'direct',
      maker: { signature: makerDirect.signature },
      taker: { signature: takerDirect.signature },
      fallbackReason: msg,
    };
  }

  await makerClient.perpConsumeAllEvents(group, marketIndex);

  await group.reloadAll(makerClient);
  makerState = await snapshotAccount({
    client: makerClient,
    group,
    accountPk: makerAccountPk,
    owner: makerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });
  takerState = await snapshotAccount({
    client: takerClient,
    group,
    accountPk: takerAccountPk,
    owner: takerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });

  const pnlSides = requireOpposingPnl(makerState.snapshot, takerState.snapshot);
  const profitableState = pnlSides.profitable === 'maker' ? makerState : takerState;
  const unprofitableState = pnlSides.unprofitable === 'maker' ? makerState : takerState;
  const profitableClient = pnlSides.profitable === 'maker' ? makerClient : takerClient;

  const settleSig = await profitableClient.perpSettlePnl(
    group,
    profitableState.account,
    unprofitableState.account,
    profitableState.account,
    marketIndex,
  );

  await group.reloadAll(makerClient);
  const makerAfterSettle = await snapshotAccount({
    client: makerClient,
    group,
    accountPk: makerAccountPk,
    owner: makerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });
  const takerAfterSettle = await snapshotAccount({
    client: takerClient,
    group,
    accountPk: takerAccountPk,
    owner: takerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });

  const profitableAfter =
    pnlSides.profitable === 'maker' ? makerAfterSettle.snapshot : takerAfterSettle.snapshot;
  const unprofitableAfter =
    pnlSides.unprofitable === 'maker' ? makerAfterSettle.snapshot : takerAfterSettle.snapshot;
  const profitableBefore = profitableState.snapshot;
  const unprofitableBefore = unprofitableState.snapshot;

  if (
    Math.abs(profitableAfter.unsettledPnlUi) >= Math.abs(profitableBefore.unsettledPnlUi) &&
    Math.abs(unprofitableAfter.unsettledPnlUi) >= Math.abs(unprofitableBefore.unsettledPnlUi)
  ) {
    throw new Error(
      `expected settle to reduce unsettled pnl magnitude; before=${profitableBefore.unsettledPnlUi}/${unprofitableBefore.unsettledPnlUi} after=${profitableAfter.unsettledPnlUi}/${unprofitableAfter.unsettledPnlUi}`,
    );
  }

  console.log(
    JSON.stringify(
      {
        status: 'ok',
        tradeSubmissions,
        pricing: {
          oracleUiPrice,
          fillUiPrice,
          tradeQty: TRADE_QTY,
          priceDiscountBps: PRICE_DISCOUNT_BPS,
        },
        beforeTrade,
        afterTrade: {
          maker: makerState.snapshot,
          taker: takerState.snapshot,
        },
        settle: {
          profitable: pnlSides.profitable,
          unprofitable: pnlSides.unprofitable,
          signature: settleSig.signature,
        },
        afterSettle: {
          maker: makerAfterSettle.snapshot,
          taker: takerAfterSettle.snapshot,
        },
      },
      null,
      2,
    ),
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
