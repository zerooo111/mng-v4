import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import { AccountMeta, Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { performance } from 'perf_hooks';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import {
  PerpMarketIndex,
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import { ContinuumHarnessClient } from '../../src/continuumHarnessClient';
import {
  buildExecutionQueueUserIntent,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { runtimeConfigPath } from './scriptEnv';

dotenv.config();

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  perpMarketIndex: number;
  maker: {
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
  sequence: string | number;
  tx_signature: string;
};

type BenchmarkRecord = {
  iteration: number;
  client_order_id: string;
  sequence: string;
  submit_to_ack_ms: number;
  submit_to_visible_ms: number;
  ack_to_visible_ms: number;
  visible_query_rtt_ms: number;
  visible_query_polls: number;
  visible_before_ack: boolean;
  submit_to_cleared_ms: number;
  clear_query_rtt_ms: number;
  clear_query_polls: number;
};

type QueryWaitResult = {
  total_ms: number;
  query_rtt_ms: number;
  polls: number;
};

const CONFIG_PATH =
  process.env.E2E_OUTPUT_CONFIG_PATH || runtimeConfigPath('execution-queue-e2e-9120.json');
const HARNESS_URL =
  process.env.CONTINUUM_HARNESS_BASE_URL ||
  process.env.HARNESS_URL ||
  'http://127.0.0.1:9292';
const RELAYER_ADDR = process.env.CTM_RELAYER_ADDR || '127.0.0.1:9090';
const ITERATIONS = Number(process.env.BENCH_QUERY_ITERATIONS || '25');
const WARMUP_ITERATIONS = Number(process.env.BENCH_QUERY_WARMUP || '5');
const TIMEOUT_MS = Number(process.env.BENCH_QUERY_TIMEOUT_MS || '5000');
const POLL_INTERVAL_MS = Number(process.env.BENCH_QUERY_POLL_INTERVAL_MS || '2');
const ORDER_PRICE_UI = Number(process.env.BENCH_QUERY_PRICE_UI || '99');
const ORDER_QTY_UI = Number(process.env.BENCH_QUERY_QTY_UI || '1');
const MAX_QUOTE_UI = Number(process.env.BENCH_QUERY_MAX_QUOTE_UI || '1000');
const ORDER_LIMIT = Number(process.env.BENCH_QUERY_LIMIT || '20');
const CANCEL_LIMIT = Number(process.env.BENCH_QUERY_CANCEL_LIMIT || '20');

function percentile(sorted: number[], p: number): number {
  if (!sorted.length) {
    return 0;
  }
  const idx = Math.min(
    sorted.length - 1,
    Math.max(0, Math.ceil((p / 100) * sorted.length) - 1),
  );
  return sorted[idx];
}

function summarize(values: number[]) {
  const sorted = [...values].sort((a, b) => a - b);
  const total = sorted.reduce((sum, value) => sum + value, 0);
  return {
    count: sorted.length,
    min_ms: sorted[0] || 0,
    p50_ms: percentile(sorted, 50),
    p95_ms: percentile(sorted, 95),
    p99_ms: percentile(sorted, 99),
    max_ms: sorted[sorted.length - 1] || 0,
    mean_ms: sorted.length ? total / sorted.length : 0,
  };
}

function formatSummary(
  label: string,
  summary: ReturnType<typeof summarize>,
): string {
  return `${label}: count=${summary.count} min=${summary.min_ms.toFixed(
    3,
  )}ms p50=${summary.p50_ms.toFixed(3)}ms p95=${summary.p95_ms.toFixed(
    3,
  )}ms p99=${summary.p99_ms.toFixed(3)}ms max=${summary.max_ms.toFixed(
    3,
  )}ms mean=${summary.mean_ms.toFixed(3)}ms`;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

async function waitForHealth(client: ContinuumHarnessClient): Promise<void> {
  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    try {
      const health = await client.healthz();
      if (health.ok) {
        return;
      }
    } catch {
      // retry until deadline
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for harness health at ${HARNESS_URL}`);
}

async function waitForRelayerReady(client: grpc.Client): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    client.waitForReady(Date.now() + TIMEOUT_MS, (err) => {
      if (err) {
        reject(err);
        return;
      }
      resolve();
    });
  });
}

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: any;
  mangoAccount: any;
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
    ...healthRemainingAccounts.map((pubkey: PublicKey) => ({
      pubkey,
      isSigner: false,
      isWritable: false,
    })),
  ];
}

function createRelayerClient(relayerAddr: string): any {
  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  return new proto.ctmsequencer.CtmSequencerRelayer(
    relayerAddr,
    grpc.credentials.createInsecure(),
  );
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
  clientOrderId: number;
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
        min_execute_slot: '0',
        expires_at_slot: '0',
        user_owner: params.userOwner.toBase58(),
        mango_account: params.mangoAccount.toBase58(),
        user_signature: Buffer.from(userSignature),
        intent_version: 2,
        target_kind: 0,
        target_index: params.market,
        client_order_id: params.clientOrderId,
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

async function waitForOrderVisible(params: {
  harnessClient: ContinuumHarnessClient;
  market: number;
  owner: PublicKey;
  clientOrderId: string;
  startedAt: number;
  timeoutMs: number;
  pollIntervalMs: number;
  cancelRef: { cancelled: boolean };
}): Promise<QueryWaitResult> {
  const deadline = Date.now() + params.timeoutMs;
  let polls = 0;
  while (Date.now() < deadline) {
    if (params.cancelRef.cancelled) {
      throw new Error('order visibility wait cancelled');
    }
    const queryStartedAt = performance.now();
    const orders = await params.harnessClient.getOrders({
      market: params.market,
      owner: params.owner,
      view: 'optimistic',
    });
    const queryRttMs = performance.now() - queryStartedAt;
    polls += 1;
    if (orders.some((order) => order.client_order_id === params.clientOrderId)) {
      return {
        total_ms: performance.now() - params.startedAt,
        query_rtt_ms: queryRttMs,
        polls,
      };
    }
    await sleep(params.pollIntervalMs);
  }
  throw new Error(
    `timed out waiting for order visibility client_order_id=${params.clientOrderId}`,
  );
}

async function waitForOrderCleared(params: {
  harnessClient: ContinuumHarnessClient;
  market: number;
  owner: PublicKey;
  clientOrderId: string;
  startedAt: number;
  timeoutMs: number;
  pollIntervalMs: number;
  cancelRef: { cancelled: boolean };
}): Promise<QueryWaitResult> {
  const deadline = Date.now() + params.timeoutMs;
  let polls = 0;
  while (Date.now() < deadline) {
    if (params.cancelRef.cancelled) {
      throw new Error('order clear wait cancelled');
    }
    const queryStartedAt = performance.now();
    const orders = await params.harnessClient.getOrders({
      market: params.market,
      owner: params.owner,
      view: 'optimistic',
    });
    const queryRttMs = performance.now() - queryStartedAt;
    polls += 1;
    if (!orders.some((order) => order.client_order_id === params.clientOrderId)) {
      return {
        total_ms: performance.now() - params.startedAt,
        query_rtt_ms: queryRttMs,
        polls,
      };
    }
    await sleep(params.pollIntervalMs);
  }
  throw new Error(
    `timed out waiting for order clear client_order_id=${params.clientOrderId}`,
  );
}

async function clearExistingOrders(params: {
  harnessClient: ContinuumHarnessClient;
  relayerClient: any;
  group: PublicKey;
  executionQueue: PublicKey;
  market: number;
  remainingAccounts: AccountMeta[];
  owner: Keypair;
  mangoAccount: PublicKey;
}): Promise<void> {
  const existing = await params.harnessClient.getOrders({
    market: params.market,
    owner: params.owner.publicKey,
    view: 'optimistic',
  });
  if (!existing.length) {
    return;
  }

  const payload = encodePerpCancelAllOrdersQueuePayload({ limit: CANCEL_LIMIT });
  const cancelRef = { cancelled: false };
  const startedAt = performance.now();
  const clearedPromise = waitForOrderCleared({
    harnessClient: params.harnessClient,
    market: params.market,
    owner: params.owner.publicKey,
    clientOrderId: existing[0].client_order_id,
    startedAt,
    timeoutMs: TIMEOUT_MS,
    pollIntervalMs: POLL_INTERVAL_MS,
    cancelRef,
  });

  try {
    await submitIntentViaRelayer({
      relayerClient: params.relayerClient,
      group: params.group,
      executionQueue: params.executionQueue,
      market: params.market,
      payload,
      remainingAccounts: params.remainingAccounts,
      userOwner: params.owner.publicKey,
      userSecretKey: params.owner.secretKey,
      mangoAccount: params.mangoAccount,
      clientOrderId: Number(existing[0].client_order_id),
    });
    await clearedPromise;
  } catch (err) {
    cancelRef.cancelled = true;
    throw err;
  }
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
  const marketIndex = config.perpMarketIndex as PerpMarketIndex;

  const owner = readKeypair(config.maker.keypairPath);
  const provider = new AnchorProvider(
    connection,
    new Wallet(owner),
    AnchorProvider.defaultOptions(),
  );
  const mangoClient = await MangoClient.connect(provider, config.cluster, programId, {
    idsSource: 'get-program-accounts',
  });
  const group = await mangoClient.getGroup(groupPk);
  const mangoAccount = await mangoClient.getMangoAccount(new PublicKey(config.maker.mangoAccount));
  const perpMarket = group.getPerpMarketByMarketIndex(marketIndex);
  const remainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
    client: mangoClient,
    group,
    mangoAccount,
    marketIndex,
    userOwner: owner.publicKey,
  });

  const harnessClient = new ContinuumHarnessClient(HARNESS_URL);
  await waitForHealth(harnessClient);
  const relayerClient = createRelayerClient(RELAYER_ADDR);
  await waitForRelayerReady(relayerClient as grpc.Client);

  await harnessClient.getOrders({
    market: marketIndex,
    owner: owner.publicKey,
    view: 'optimistic',
  });
  await clearExistingOrders({
    harnessClient,
    relayerClient,
    group: groupPk,
    executionQueue: executionQueuePk,
    market: marketIndex,
    remainingAccounts,
    owner,
    mangoAccount: mangoAccount.publicKey,
  });

  const records: BenchmarkRecord[] = [];
  const totalIterations = WARMUP_ITERATIONS + ITERATIONS;

  for (let iter = 0; iter < totalIterations; iter += 1) {
    const clientOrderId = 1_000_000 + iter;
    const payload = encodePerpPlaceOrderV2QueuePayload({
      side: PerpOrderSide.bid,
      priceLots: BigInt(perpMarket.uiPriceToLots(ORDER_PRICE_UI).toString()),
      maxBaseLots: BigInt(perpMarket.uiBaseToLots(ORDER_QTY_UI).toString()),
      maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(MAX_QUOTE_UI).toString()),
      clientOrderId,
      orderType: PerpOrderType.limit,
      selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: ORDER_LIMIT,
    });

    const submitStartedAt = performance.now();
    const visibleCancelRef = { cancelled: false };
    const visiblePromise = waitForOrderVisible({
      harnessClient,
      market: marketIndex,
      owner: owner.publicKey,
      clientOrderId: clientOrderId.toString(),
      startedAt: submitStartedAt,
      timeoutMs: TIMEOUT_MS,
      pollIntervalMs: POLL_INTERVAL_MS,
      cancelRef: visibleCancelRef,
    });

    let ackResponse: SubmitIntentResponse;
    let visible: QueryWaitResult;
    let submitToAckMs: number;
    try {
      ackResponse = await submitIntentViaRelayer({
        relayerClient,
        group: groupPk,
        executionQueue: executionQueuePk,
        market: marketIndex,
        payload,
        remainingAccounts,
        userOwner: owner.publicKey,
        userSecretKey: owner.secretKey,
        mangoAccount: mangoAccount.publicKey,
        clientOrderId,
      });
      submitToAckMs = performance.now() - submitStartedAt;
      visible = await visiblePromise;
    } catch (err) {
      visibleCancelRef.cancelled = true;
      throw err;
    }

    const cancelPayload = encodePerpCancelAllOrdersQueuePayload({ limit: CANCEL_LIMIT });
    const clearedStartedAt = performance.now();
    const clearedCancelRef = { cancelled: false };
    const clearedPromise = waitForOrderCleared({
      harnessClient,
      market: marketIndex,
      owner: owner.publicKey,
      clientOrderId: clientOrderId.toString(),
      startedAt: clearedStartedAt,
      timeoutMs: TIMEOUT_MS,
      pollIntervalMs: POLL_INTERVAL_MS,
      cancelRef: clearedCancelRef,
    });

    let cleared: QueryWaitResult;
    try {
      await submitIntentViaRelayer({
        relayerClient,
        group: groupPk,
        executionQueue: executionQueuePk,
        market: marketIndex,
        payload: cancelPayload,
        remainingAccounts,
        userOwner: owner.publicKey,
        userSecretKey: owner.secretKey,
        mangoAccount: mangoAccount.publicKey,
        clientOrderId,
      });
      cleared = await clearedPromise;
    } catch (err) {
      clearedCancelRef.cancelled = true;
      throw err;
    }

    const sequence = String(ackResponse.sequence);
    const visibleBeforeAck = visible.total_ms < submitToAckMs;
    if (iter >= WARMUP_ITERATIONS) {
      records.push({
        iteration: iter - WARMUP_ITERATIONS,
        client_order_id: clientOrderId.toString(),
        sequence,
        submit_to_ack_ms: submitToAckMs,
        submit_to_visible_ms: visible.total_ms,
        ack_to_visible_ms: Math.max(0, visible.total_ms - submitToAckMs),
        visible_query_rtt_ms: visible.query_rtt_ms,
        visible_query_polls: visible.polls,
        visible_before_ack: visibleBeforeAck,
        submit_to_cleared_ms: cleared.total_ms,
        clear_query_rtt_ms: cleared.query_rtt_ms,
        clear_query_polls: cleared.polls,
      });
    }
  }

  const output = {
    config: {
      harness_url: HARNESS_URL,
      relayer_addr: RELAYER_ADDR,
      config_path: path.resolve(CONFIG_PATH),
      iterations: ITERATIONS,
      warmup_iterations: WARMUP_ITERATIONS,
      timeout_ms: TIMEOUT_MS,
      poll_interval_ms: POLL_INTERVAL_MS,
      market: marketIndex,
      owner: owner.publicKey.toBase58(),
      mango_account: mangoAccount.publicKey.toBase58(),
      endpoint: `/state/orders/${marketIndex}?owner=${owner.publicKey.toBase58()}&view=optimistic`,
      price_ui: ORDER_PRICE_UI,
      qty_ui: ORDER_QTY_UI,
    },
    counts: {
      measured_iterations: records.length,
      visible_before_ack: records.filter((record) => record.visible_before_ack).length,
    },
    summaries: {
      submit_to_ack_ms: summarize(records.map((record) => record.submit_to_ack_ms)),
      submit_to_visible_query_ms: summarize(
        records.map((record) => record.submit_to_visible_ms),
      ),
      ack_to_visible_query_ms: summarize(
        records.map((record) => record.ack_to_visible_ms),
      ),
      visible_query_rtt_ms: summarize(
        records.map((record) => record.visible_query_rtt_ms),
      ),
      visible_query_polls: summarize(
        records.map((record) => record.visible_query_polls),
      ),
      submit_to_cleared_query_ms: summarize(
        records.map((record) => record.submit_to_cleared_ms),
      ),
      cleared_query_rtt_ms: summarize(
        records.map((record) => record.clear_query_rtt_ms),
      ),
      cleared_query_polls: summarize(
        records.map((record) => record.clear_query_polls),
      ),
    },
    records,
  };

  console.log(JSON.stringify(output, null, 2));
  console.log(formatSummary('submit->ack', output.summaries.submit_to_ack_ms));
  console.log(
    formatSummary(
      'submit->orders_visible',
      output.summaries.submit_to_visible_query_ms,
    ),
  );
  console.log(
    formatSummary(
      'ack->orders_visible',
      output.summaries.ack_to_visible_query_ms,
    ),
  );
  console.log(
    formatSummary(
      'orders_query_rtt',
      output.summaries.visible_query_rtt_ms,
    ),
  );
  console.log(
    formatSummary(
      'submit->orders_cleared',
      output.summaries.submit_to_cleared_query_ms,
    ),
  );
  console.log(
    formatSummary(
      'orders_clear_query_rtt',
      output.summaries.cleared_query_rtt_ms,
    ),
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
