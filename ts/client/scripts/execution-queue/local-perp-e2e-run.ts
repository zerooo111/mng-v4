import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
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
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { decodeExecutionQueueCount } from '../../src/executionQueueLayout';
import { runtimeConfigPath } from './scriptEnv';

dotenv.config();

type LaneMeta = {
  pubkey: string;
  isWritable: boolean;
  isSigner?: boolean;
};

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
    payerKeypairPath?: string;
  };
};

type SubmitIntentResponse = {
  sequence: string;
  tx_signature: string;
};

const CONFIG_PATH =
  process.env.E2E_OUTPUT_CONFIG_PATH || runtimeConfigPath('execution-queue-e2e-9101.json');
const RELAYER_ADDR_OVERRIDE = process.env.CTM_RELAYER_ADDR;
const MAKER_PRICE = Number(process.env.E2E_MAKER_PRICE || '99');
const MAKER_QTY = Number(process.env.E2E_MAKER_QTY || '2');
const TAKER_PRICE = Number(process.env.E2E_TAKER_PRICE || '98');
const TAKER_QTY = Number(process.env.E2E_TAKER_QTY || '1');
const MAKER_MAX_QUOTE_QTY = Number(process.env.E2E_MAKER_MAX_QUOTE_QTY || '1000');
const TAKER_MAX_QUOTE_QTY = Number(process.env.E2E_TAKER_MAX_QUOTE_QTY || '1000');
const QUEUE_EMPTY_TIMEOUT_MS = Number(process.env.E2E_QUEUE_EMPTY_TIMEOUT_MS || '90000');
const POLL_MS = Number(process.env.E2E_POLL_MS || '1000');
const POSITION_SETTLE_TIMEOUT_MS = Number(
  process.env.E2E_POSITION_SETTLE_TIMEOUT_MS || '15000',
);

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
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

async function snapshotAccount(params: {
  client: MangoClient;
  group: Group;
  accountPk: PublicKey;
  owner: PublicKey;
  usdcMint: PublicKey;
  marketIndex: PerpMarketIndex;
}): Promise<{
  account: MangoAccount;
  baseLots: BN;
  usdcBalanceUi: number;
  openOrdersCount: number;
}> {
  const account = await params.client.getMangoAccount(params.accountPk);
  await account.reload(params.client);
  const perpPosition = account.getPerpPosition(params.marketIndex);
  const usdcBank = params.group.getFirstBankByMint(params.usdcMint);
  const openOrdersCount = await loadOpenOrdersCount(
    params.client,
    params.group,
    account,
    params.marketIndex,
  );

  return {
    account,
    baseLots: perpPosition ? perpPosition.basePositionLots : new BN(0),
    usdcBalanceUi: account.getTokenBalanceUi(usdcBank),
    openOrdersCount,
  };
}

async function waitForMatchedPositions(params: {
  adminClient: MangoClient;
  adminGroup: Group;
  marketIndex: PerpMarketIndex;
  makerClient: MangoClient;
  makerGroup: Group;
  makerAccountPk: PublicKey;
  makerOwner: PublicKey;
  takerClient: MangoClient;
  takerGroup: Group;
  takerAccountPk: PublicKey;
  takerOwner: PublicKey;
  usdcMint: PublicKey;
}): Promise<{
  makerState: Awaited<ReturnType<typeof snapshotAccount>>;
  takerState: Awaited<ReturnType<typeof snapshotAccount>>;
}> {
  const deadline = Date.now() + POSITION_SETTLE_TIMEOUT_MS;
  while (Date.now() < deadline) {
    await params.adminClient.perpConsumeAllEvents(params.adminGroup, params.marketIndex);
    await params.adminGroup.reloadAll(params.adminClient);
    await params.makerGroup.reloadAll(params.makerClient);
    await params.takerGroup.reloadAll(params.takerClient);

    const makerState = await snapshotAccount({
      client: params.makerClient,
      group: params.makerGroup,
      accountPk: params.makerAccountPk,
      owner: params.makerOwner,
      usdcMint: params.usdcMint,
      marketIndex: params.marketIndex,
    });
    const takerState = await snapshotAccount({
      client: params.takerClient,
      group: params.takerGroup,
      accountPk: params.takerAccountPk,
      owner: params.takerOwner,
      usdcMint: params.usdcMint,
      marketIndex: params.marketIndex,
    });

    if (makerState.baseLots.gt(new BN(0)) && takerState.baseLots.lt(new BN(0))) {
      return { makerState, takerState };
    }

    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }

  const makerState = await snapshotAccount({
    client: params.makerClient,
    group: params.makerGroup,
    accountPk: params.makerAccountPk,
    owner: params.makerOwner,
    usdcMint: params.usdcMint,
    marketIndex: params.marketIndex,
  });
  const takerState = await snapshotAccount({
    client: params.takerClient,
    group: params.takerGroup,
    accountPk: params.takerAccountPk,
    owner: params.takerOwner,
    usdcMint: params.usdcMint,
    marketIndex: params.marketIndex,
  });
  return { makerState, takerState };
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
    target: { kind: 0, index: params.market },
    remainingAccounts: params.remainingAccounts,
  });
  const userSignature = signExecutionQueueIntentMessage(
    params.userSecretKey,
    intent.userIntentMessage,
  );

  return await new Promise<SubmitIntentResponse>((resolve, reject) => {
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
        intent_version: 2,
        target_kind: 0,
        target_index: params.market,
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
  const adminKp = readKeypair(
    config.relayer.payerKeypairPath || '/home/ec2-user/.config/solana/id.json',
  );

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
  const adminProvider = new AnchorProvider(
    connection,
    new Wallet(adminKp),
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
  const adminClient = await MangoClient.connect(
    adminProvider,
    config.cluster,
    programId,
    { idsSource: 'get-program-accounts' },
  );

  const adminGroup = await adminClient.getGroup(groupPk);
  const makerGroup = await makerClient.getGroup(groupPk);
  const takerGroup = await takerClient.getGroup(groupPk);
  const makerAccount = await makerClient.getMangoAccount(new PublicKey(config.maker.mangoAccount));
  const takerAccount = await takerClient.getMangoAccount(new PublicKey(config.taker.mangoAccount));

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

  const makerPerpMarket = makerGroup.getPerpMarketByMarketIndex(marketIndex);
  const makerClientOrderId = Date.now();
  const takerClientOrderId = Date.now() + 1;

  const makerRemainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
    client: makerClient,
    group: makerGroup,
    mangoAccount: makerAccount,
    marketIndex,
    userOwner: makerKp.publicKey,
  });
  const takerRemainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
    client: takerClient,
    group: takerGroup,
    mangoAccount: takerAccount,
    marketIndex,
    userOwner: takerKp.publicKey,
  });

  const makerPayload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.bid,
    priceLots: BigInt(makerPerpMarket.uiPriceToLots(MAKER_PRICE).toString()),
    maxBaseLots: BigInt(makerPerpMarket.uiBaseToLots(MAKER_QTY).toString()),
    maxQuoteLots: BigInt(
      makerPerpMarket.uiQuoteToLots(MAKER_MAX_QUOTE_QTY).toString(),
    ),
    clientOrderId: makerClientOrderId,
    orderType: PerpOrderType.limit,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: 0,
    limit: 20,
  });

  const takerPayload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.ask,
    priceLots: BigInt(makerPerpMarket.uiPriceToLots(TAKER_PRICE).toString()),
    maxBaseLots: BigInt(makerPerpMarket.uiBaseToLots(TAKER_QTY).toString()),
    maxQuoteLots: BigInt(
      makerPerpMarket.uiQuoteToLots(TAKER_MAX_QUOTE_QTY).toString(),
    ),
    clientOrderId: takerClientOrderId,
    orderType: PerpOrderType.limit,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: 0,
    limit: 20,
  });

  const currentSlot = BigInt(await connection.getSlot('confirmed'));

  const makerResp = await submitIntentViaRelayer({
    relayerClient,
    group: groupPk,
    executionQueue: executionQueuePk,
    market: marketIndex,
    payload: makerPayload,
    remainingAccounts: makerRemainingAccounts,
    userOwner: makerKp.publicKey,
    userSecretKey: makerKp.secretKey,
    mangoAccount: makerAccount.publicKey,
    minExecuteSlot: currentSlot + 1n,
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
    mangoAccount: takerAccount.publicKey,
    minExecuteSlot: currentSlot + 1n,
  });

  await waitForQueueToDrain(connection, executionQueuePk);
  const { makerState, takerState } = await waitForMatchedPositions({
    adminClient,
    adminGroup,
    marketIndex,
    makerClient,
    makerGroup,
    makerAccountPk: makerAccount.publicKey,
    makerOwner: makerKp.publicKey,
    takerClient,
    takerGroup,
    takerAccountPk: takerAccount.publicKey,
    takerOwner: takerKp.publicKey,
    usdcMint: usdcMintPk,
  });

  if (!makerState.baseLots.gt(new BN(0)) || !takerState.baseLots.lt(new BN(0))) {
    throw new Error(
      `expected maker long and taker short after match, got maker=${makerState.baseLots.toString()} taker=${takerState.baseLots.toString()}`,
    );
  }

  const loadMakerOpenOrders = async () => {
    const freshMakerAccount = await makerClient.getMangoAccount(makerAccount.publicKey);
    return await freshMakerAccount.loadPerpOpenOrdersForMarket(
      makerClient,
      makerGroup,
      marketIndex,
      true,
    );
  };

  const makerOpenBeforeCancel = await loadMakerOpenOrders();
  if (makerOpenBeforeCancel.length === 0) {
    throw new Error('expected maker to retain partially-filled open order before cancel');
  }

  const cancelRemainingAccounts = makerRemainingAccounts;
  const cancelPayload = encodePerpCancelAllOrdersQueuePayload({ limit: 255 });

  let cancelResp = await submitIntentViaRelayer({
    relayerClient,
    group: groupPk,
    executionQueue: executionQueuePk,
    market: marketIndex,
    payload: cancelPayload,
    remainingAccounts: cancelRemainingAccounts,
    userOwner: makerKp.publicKey,
    userSecretKey: makerKp.secretKey,
    mangoAccount: makerAccount.publicKey,
    minExecuteSlot: BigInt(await connection.getSlot('confirmed')) + 1n,
  });
  try {
    await makerClient.executionQueueExecute(
      makerGroup,
      executionQueuePk,
      executionQueueBufferPk,
      0, // marketIndex (single-market localnet test)
      cancelRemainingAccounts,
      1,
    );
  } catch {
    // Best effort: cranker may process this first.
  }

  await waitForQueueToDrain(connection, executionQueuePk);

  let makerOpenAfterCancel = await loadMakerOpenOrders();
  const cancelDeadline = Date.now() + 180000;
  while (makerOpenAfterCancel.length !== 0 && Date.now() < cancelDeadline) {
    try {
      await makerClient.executionQueueExecute(
        makerGroup,
        executionQueuePk,
        executionQueueBufferPk,
        0, // marketIndex (single-market localnet test)
        cancelRemainingAccounts,
        1,
      );
    } catch {
      // Best effort: cranker may process this first.
    }
    await new Promise((resolve) => setTimeout(resolve, 2000));
    makerOpenAfterCancel = await loadMakerOpenOrders();
  }
  if (makerOpenAfterCancel.length !== 0) {
    throw new Error('expected maker open orders to be empty after cancel-all');
  }

  await makerGroup.reloadAll(makerClient);
  const makerFinalState = await snapshotAccount({
    client: makerClient,
    group: makerGroup,
    accountPk: makerAccount.publicKey,
    owner: makerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });
  const takerFinalState = await snapshotAccount({
    client: takerClient,
    group: takerGroup,
    accountPk: takerAccount.publicKey,
    owner: takerKp.publicKey,
    usdcMint: usdcMintPk,
    marketIndex,
  });
  const makerUsdcBalance = makerFinalState.usdcBalanceUi;
  const takerUsdcBalance = takerFinalState.usdcBalanceUi;

  if (makerUsdcBalance <= 0 || takerUsdcBalance <= 0) {
    throw new Error('expected positive USDC deposits for maker and taker');
  }

  console.log(
    JSON.stringify(
      {
        status: 'ok',
        relayerSubmissions: {
          maker: makerResp,
          taker: takerResp,
          cancel: cancelResp,
        },
        positions: {
          makerBaseLots: makerFinalState.baseLots.toString(),
          takerBaseLots: takerFinalState.baseLots.toString(),
        },
        balances: {
          makerUsdc: makerUsdcBalance,
          takerUsdc: takerUsdcBalance,
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
