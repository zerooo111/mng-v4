import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Connection,
  Keypair,
  PublicKey,
  TransactionInstruction,
} from '@solana/web3.js';
import fs from 'fs';
import path from 'path';
import { MangoClient } from '../../src/client';
import { PerpPosition } from '../../src/accounts/mangoAccount';
import {
  buildExecutionQueueEnqueueCtmWithIntentIxs,
  anchorInstructionDiscriminator,
  encodePerpCancelAllOrdersQueuePayload,
} from '../../src/executionQueue';
import {
  EXECUTION_QUEUE_LAYOUT,
  decodeExecutionQueueHeadItem,
  decodeExecutionQueueHeader,
} from '../../src/executionQueueLayout';
import { runtimeConfigPath } from './scriptEnv';

const INTERVAL_MS = Number(process.env.DIVERGENCE_MONITOR_INTERVAL_MS || '5000');
const HARNESS_URL =
  process.env.DIVERGENCE_MONITOR_HARNESS_URL ||
  process.env.CONTINUUM_HARNESS_URL ||
  'http://127.0.0.1:9091';
const CONFIG_PATH =
  process.env.E2E_CONFIG_PATH ||
  process.env.QUOTER_CONFIG_PATH ||
  runtimeConfigPath('execution-queue-e2e-9125.json');
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || 'https://api.devnet.solana.com';
const STALL_TICKS = Number(process.env.DIVERGENCE_MONITOR_STALL_TICKS || '3');
const AUTO_DRAIN = (process.env.DIVERGENCE_MONITOR_AUTO_DRAIN || 'true') === 'true';
const DRAIN_MAX_STEPS = Number(process.env.DIVERGENCE_MONITOR_DRAIN_MAX_STEPS || '2048');
const DRAIN_SLEEP_MS = Number(process.env.DIVERGENCE_MONITOR_DRAIN_SLEEP_MS || '1200');
const DRAIN_BATCH_SIZE = Number(process.env.DIVERGENCE_MONITOR_DRAIN_BATCH_SIZE || '12');
const INCLUDE_BOOTSTRAP_ACCOUNTS =
  (process.env.DIVERGENCE_MONITOR_INCLUDE_BOOTSTRAP_ACCOUNTS || 'false') ===
  'true';

const EXECUTION_QUEUE_GAP_WAIT_SLOTS_OFFSET = 184;
const EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET = 192;
const EXECUTION_QUEUE_PAUSED_INGRESS_OFFSET = 145;
const EXECUTION_QUEUE_PAUSED_EXECUTE_OFFSET = 146;

interface E2EConfig {
  programId: string;
  group: string;
  executionQueue: string;
  cluster: string;
  clusterUrl: string;
  perpMarketIndex: number;
  maker?: { mangoAccount: string; owner: string };
  taker?: { mangoAccount: string; owner: string };
  cranker?: { keypairPath?: string };
  relayer?: { payerKeypairPath?: string };
}

interface QuoterBotSpec {
  name: string;
  keypairPath: string;
  owner: string;
  mangoAccount: string;
  side?: string;
}

interface HarnessUserState {
  owner: string;
  mango_accounts: string[];
  open_orders: any[];
  per_market: {
    market: string;
    base_position_lots: string;
    quote_position_native: string;
    open_order_base_lots_bid: string;
    open_order_base_lots_ask: string;
  }[];
}

interface HarnessMarketState {
  bids: any[];
  asks: any[];
  open_orders: any[];
  watermarks: {
    optimistic_seq: string;
    confirmed_seq: string;
    last_slot: string;
  };
}

interface HarnessFullState {
  markets: Record<string, HarnessMarketState>;
  users: Record<string, HarnessUserState>;
  queue?: Record<
    string,
    {
      market: string;
      pending_count: number;
      processed_count: number;
      failed_count: number;
      skipped_count: number;
      last_processed_sequence: string;
      lag_slots: string;
      unmatched_processed_count?: number;
    }
  >;
  generated_ts_ms: number;
}

interface Divergence {
  field: string;
  account: string;
  harness: string | number;
  onchain: string | number;
  delta?: string | number;
}

interface QueueAdminState {
  gapWaitSlots: bigint;
  liquidityDelaySlots: bigint;
  pauseIngress: boolean;
  pauseExecute: boolean;
}

function ts(): string {
  return new Date().toISOString();
}

function u8(value: number): Buffer {
  const out = Buffer.alloc(1);
  out.writeUInt8(value, 0);
  return out;
}

function u16ToLe(value: number): Buffer {
  const out = Buffer.alloc(2);
  out.writeUInt16LE(value, 0);
  return out;
}

function u64ToLe(value: bigint): Buffer {
  const out = Buffer.alloc(8);
  out.writeBigUInt64LE(value, 0);
  return out;
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

async function fetchHarnessState(): Promise<HarnessFullState> {
  const resp = await fetch(`${HARNESS_URL}/state/full`, {
    signal: AbortSignal.timeout(4000),
  });
  if (!resp.ok) {
    throw new Error(`harness ${resp.status}`);
  }
  return resp.json() as Promise<HarnessFullState>;
}

async function fetchHarnessHealth(): Promise<any> {
  const resp = await fetch(`${HARNESS_URL}/healthz`, {
    signal: AbortSignal.timeout(4000),
  });
  if (!resp.ok) {
    throw new Error(`harness health ${resp.status}`);
  }
  return resp.json();
}

function decodeQueueAdminState(data: Buffer): QueueAdminState {
  return {
    gapWaitSlots: data.readBigUInt64LE(EXECUTION_QUEUE_GAP_WAIT_SLOTS_OFFSET),
    liquidityDelaySlots: data.readBigUInt64LE(
      EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET,
    ),
    pauseIngress: data.readUInt8(EXECUTION_QUEUE_PAUSED_INGRESS_OFFSET) !== 0,
    pauseExecute: data.readUInt8(EXECUTION_QUEUE_PAUSED_EXECUTE_OFFSET) !== 0,
  };
}

function buildExecutionQueueConfigureIx(params: {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  admin: PublicKey;
  state: QueueAdminState;
  pauseExecute: boolean;
}): TransactionInstruction {
  const discriminator = anchorInstructionDiscriminator('execution_queue_configure');
  const data = Buffer.concat([
    discriminator,
    u64ToLe(params.state.gapWaitSlots),
    u64ToLe(params.state.liquidityDelaySlots),
    u8(params.state.pauseIngress ? 1 : 0),
    u8(params.pauseExecute ? 1 : 0),
  ]);
  return new TransactionInstruction({
    programId: params.programId,
    keys: [
      { pubkey: params.group, isSigner: false, isWritable: false },
      { pubkey: params.executionQueue, isSigner: false, isWritable: true },
      { pubkey: params.admin, isSigner: true, isWritable: false },
    ],
    data,
  });
}

function buildExecutionQueueDropCtmIx(params: {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  admin: PublicKey;
  sequence: bigint;
}): TransactionInstruction {
  const discriminator = anchorInstructionDiscriminator('execution_queue_drop_ctm');
  const data = Buffer.concat([discriminator, u64ToLe(params.sequence)]);
  return new TransactionInstruction({
    programId: params.programId,
    keys: [
      { pubkey: params.group, isSigner: false, isWritable: false },
      { pubkey: params.executionQueue, isSigner: false, isWritable: true },
      { pubkey: params.admin, isSigner: true, isWritable: false },
    ],
    data,
  });
}

async function sendIxs(
  client: MangoClient,
  ixs: TransactionInstruction[],
): Promise<string> {
  const status = await client.sendAndConfirmTransaction(ixs, {
    prioritizationFee: 1,
    txConfirmationCommitment: 'confirmed',
  });
  return status.signature;
}

async function inspectQueue(
  connection: Connection,
  executionQueuePk: PublicKey,
): Promise<{
  data: Buffer;
  header: ReturnType<typeof decodeExecutionQueueHeader>;
  head: ReturnType<typeof decodeExecutionQueueHeadItem>;
  admin: QueueAdminState;
  nextCtmSlot: {
    sequence: bigint;
    kind: number;
    status: number;
  } | null;
}> {
  const ai = await connection.getAccountInfo(executionQueuePk, 'confirmed');
  if (!ai) {
    throw new Error(`missing queue account ${executionQueuePk.toBase58()}`);
  }
  const data = Buffer.from(ai.data);
  const header = decodeExecutionQueueHeader(data);
  let nextCtmSlot: { sequence: bigint; kind: number; status: number } | null = null;
  if (header.ctmCount > 0) {
    const physicalIndex =
      Number(header.nextSequence % BigInt(EXECUTION_QUEUE_LAYOUT.ctmCapacity));
    const offset =
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
      physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
    if (offset + EXECUTION_QUEUE_LAYOUT.itemSize <= data.length) {
      nextCtmSlot = {
        sequence: data.readBigUInt64LE(
          offset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset,
        ),
        kind: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemKindOffset),
        status: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset),
      };
    }
  }
  return {
    data,
    header,
    head: decodeExecutionQueueHeadItem(data),
    admin: decodeQueueAdminState(data),
    nextCtmSlot,
  };
}

function isPendingCtmAtSequence(data: Buffer, sequence: bigint): boolean {
  const physicalIndex =
    Number(sequence % BigInt(EXECUTION_QUEUE_LAYOUT.ctmCapacity));
  const offset =
    EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
    physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
  if (offset + EXECUTION_QUEUE_LAYOUT.itemSize > data.length) {
    return false;
  }
  const itemSequence = data.readBigUInt64LE(
    offset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset,
  );
  const itemKind = data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemKindOffset);
  const itemStatus = data.readUInt8(
    offset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset,
  );
  return (
    itemSequence === sequence &&
    itemKind === EXECUTION_QUEUE_LAYOUT.ctmWrappedKind &&
    itemStatus === EXECUTION_QUEUE_LAYOUT.pendingStatus
  );
}

function findNextPendingCtmSequence(
  data: Buffer,
  nextSequence: bigint,
  maxSeenSequence: bigint,
): bigint | null {
  for (let sequence = nextSequence; sequence <= maxSeenSequence; sequence += 1n) {
    if (isPendingCtmAtSequence(data, sequence)) {
      return sequence;
    }
  }
  return null;
}

async function enqueueGapFillers(params: {
  client: MangoClient;
  group: ReturnType<MangoClient['getGroup']> extends Promise<infer T> ? T : never;
  programId: PublicKey;
  groupPk: PublicKey;
  executionQueuePk: PublicKey;
  perpMarketIndex: number;
  ctmSigner: Keypair;
  bot: QuoterBotSpec;
  startSequence: bigint;
  endSequenceInclusive: bigint;
}): Promise<number> {
  const {
    client,
    group,
    programId,
    groupPk,
    executionQueuePk,
    perpMarketIndex,
    ctmSigner,
    bot,
    startSequence,
    endSequenceInclusive,
  } = params;
  if (endSequenceInclusive < startSequence) {
    return 0;
  }

  const botOwner = readKeypair(bot.keypairPath);
  const mangoAccount = await client.getMangoAccount(new PublicKey(bot.mangoAccount));
  const perpMarket = group.getPerpMarketByMarketIndex(perpMarketIndex as any);
  const remainingAccounts: AccountMeta[] = [
    { pubkey: groupPk, isSigner: false, isWritable: false },
    { pubkey: mangoAccount.publicKey, isSigner: false, isWritable: true },
    { pubkey: botOwner.publicKey, isSigner: false, isWritable: false },
    { pubkey: perpMarket.publicKey, isSigner: false, isWritable: true },
    { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
    { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
    { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
    { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
  ];

  let sent = 0;
  const payload = encodePerpCancelAllOrdersQueuePayload({ limit: 1 });
  let currentSlot = BigInt(await client.connection.getSlot('confirmed'));
  for (
    let sequence = startSequence;
    sequence <= endSequenceInclusive;
    sequence += 1n, sent += 1
  ) {
    const built = buildExecutionQueueEnqueueCtmWithIntentIxs({
      programId,
      group: groupPk,
      executionQueue: executionQueuePk,
      executionQueueBuffer: executionQueuePk,
      remainingAccounts,
      payload,
      sequence,
      minExecuteSlot: currentSlot + 1n,
      mangoAccount: mangoAccount.publicKey,
      userOwner: botOwner.publicKey,
      userSigner: { kind: 'keypair', privateKey: botOwner.secretKey },
      ctmSigner: { kind: 'keypair', privateKey: ctmSigner.secretKey },
    });
    const signature = await sendIxs(client, built.instructions);
    console.log(
      JSON.stringify({
        ts: ts(),
        msg: 'queue-gap-filler-enqueued',
        bot: bot.name,
        sequence: sequence.toString(),
        signature,
      }),
    );
    if (sent % 8 === 7) {
      currentSlot = BigInt(await client.connection.getSlot('confirmed'));
    }
  }
  return sent;
}

async function drainQueue(
  client: MangoClient,
  group: ReturnType<MangoClient['getGroup']> extends Promise<infer T> ? T : never,
  groupPk: PublicKey,
  executionQueuePk: PublicKey,
  programId: PublicKey,
  perpMarketIndex: number,
  ctmSigner: Keypair,
  fallbackBot: QuoterBotSpec | null,
): Promise<{ drained: boolean; steps: number; finalCount: number }> {
  let steps = 0;

  while (steps < DRAIN_MAX_STEPS) {
    const inspected = await inspectQueue(client.connection, executionQueuePk);
    const { header, head, admin } = inspected;

    if (header.count === 0) {
      if (admin.pauseExecute) {
        await sendIxs(client, [
          buildExecutionQueueConfigureIx({
            programId,
            group: groupPk,
            executionQueue: executionQueuePk,
            admin: client.walletPk,
            state: admin,
            pauseExecute: false,
          }),
        ]);
      }
      return { drained: true, steps, finalCount: 0 };
    }

    const isGap =
      header.ctmCount > 0 &&
      header.maxSeenSequence >= header.nextSequence &&
      (!inspected.nextCtmSlot ||
        inspected.nextCtmSlot.status !== EXECUTION_QUEUE_LAYOUT.pendingStatus ||
        inspected.nextCtmSlot.kind !== EXECUTION_QUEUE_LAYOUT.ctmWrappedKind ||
        inspected.nextCtmSlot.sequence !== header.nextSequence);
    if (isGap) {
      if (!fallbackBot) {
        throw new Error('gap detected but no quoter bot spec available for filler');
      }
      const nextRealSequence = findNextPendingCtmSequence(
        inspected.data,
        header.nextSequence,
        header.maxSeenSequence,
      );
      const fillerEnd =
        nextRealSequence !== null ? nextRealSequence - 1n : header.maxSeenSequence;
      if (fillerEnd < header.nextSequence) {
        throw new Error(
          `gap detected at ${header.nextSequence.toString()} but no filler span found`,
        );
      }
      if (admin.pauseExecute) {
        await sendIxs(client, [
          buildExecutionQueueConfigureIx({
            programId,
            group: groupPk,
            executionQueue: executionQueuePk,
            admin: client.walletPk,
            state: admin,
            pauseExecute: false,
          }),
        ]);
        await sleep(DRAIN_SLEEP_MS);
      }
      const filled = await enqueueGapFillers({
        client,
        group,
        programId,
        groupPk,
        executionQueuePk,
        perpMarketIndex,
        ctmSigner,
        bot: fallbackBot,
        startSequence: header.nextSequence,
        endSequenceInclusive: fillerEnd,
      });
      console.log(
        JSON.stringify({
          ts: ts(),
          msg: 'queue-drain-gap-fill',
          next_sequence_before: header.nextSequence.toString(),
          next_real_sequence:
            nextRealSequence !== null ? nextRealSequence.toString() : null,
          filled_until: fillerEnd.toString(),
          filled,
          queue_count_before: header.count,
        }),
      );
      steps += filled;
      await sleep(DRAIN_SLEEP_MS);
      continue;
    }

    if (head && head.section === 'ctm') {
      const batch = collectContiguousPendingCtmSequences(
        inspected.data,
        head.sequence,
        DRAIN_BATCH_SIZE,
      );
      const ixs: TransactionInstruction[] = [];
      if (!admin.pauseExecute) {
        ixs.push(
          buildExecutionQueueConfigureIx({
            programId,
            group: groupPk,
            executionQueue: executionQueuePk,
            admin: client.walletPk,
            state: admin,
            pauseExecute: true,
          }),
        );
      }
      for (const sequence of batch) {
        ixs.push(
          buildExecutionQueueDropCtmIx({
            programId,
            group: groupPk,
            executionQueue: executionQueuePk,
            admin: client.walletPk,
            sequence,
          }),
        );
      }

      const sig = await sendIxs(client, ixs);
      console.log(
        JSON.stringify({
          ts: ts(),
          msg: 'queue-drain-drop-batch',
          first_sequence: batch[0].toString(),
          last_sequence: batch[batch.length - 1].toString(),
          dropped: batch.length,
          queue_count_before: header.count,
          signature: sig,
        }),
      );
      steps += batch.length;
      await sleep(DRAIN_SLEEP_MS);
      continue;
    }

    throw new Error(
      `unsupported drain state count=${header.count} ctm_count=${header.ctmCount} liquidity_count=${header.liquidityCount} head_section=${head?.section ?? 'none'}`,
    );
  }

  const finalState = await inspectQueue(client.connection, executionQueuePk);
  return {
    drained: finalState.header.count === 0,
    steps,
    finalCount: finalState.header.count,
  };
}

function collectContiguousPendingCtmSequences(
  data: Buffer,
  startSequence: bigint,
  maxCount: number,
): bigint[] {
  const out: bigint[] = [];
  for (let i = 0; i < maxCount; i += 1) {
    const sequence = startSequence + BigInt(i);
    const physicalIndex =
      Number(sequence % BigInt(EXECUTION_QUEUE_LAYOUT.ctmCapacity));
    const offset =
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
      physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
    const itemSequence = data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset,
    );
    const itemKind = data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemKindOffset);
    const itemStatus = data.readUInt8(
      offset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset,
    );
    if (
      itemSequence !== sequence ||
      itemKind !== EXECUTION_QUEUE_LAYOUT.ctmWrappedKind ||
      itemStatus !== EXECUTION_QUEUE_LAYOUT.pendingStatus
    ) {
      break;
    }
    out.push(sequence);
  }
  return out.length > 0 ? out : [startSequence];
}

async function main() {
  const config: E2EConfig = JSON.parse(
    fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8'),
  );
  const clusterUrl = CLUSTER_URL || config.clusterUrl;
  const adminKeypairPath =
    process.env.DIVERGENCE_MONITOR_ADMIN_KEYPAIR ||
    config.cranker?.keypairPath ||
    config.relayer?.payerKeypairPath ||
    path.resolve(process.env.HOME || '', '.config/solana/id.json');
  const admin = readKeypair(adminKeypairPath);

  const connection = new Connection(clusterUrl, 'confirmed');
  const provider = new AnchorProvider(
    connection,
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(
    provider,
    config.cluster as any,
    new PublicKey(config.programId),
    { idsSource: 'get-program-accounts' },
  );

  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const programId = new PublicKey(config.programId);
  const perpMarketIndex = config.perpMarketIndex;

  const mangoAccountPks: { label: string; owner: string; pk: PublicKey }[] = [];
  if (
    INCLUDE_BOOTSTRAP_ACCOUNTS &&
    config.maker?.mangoAccount &&
    config.maker?.owner
  ) {
    mangoAccountPks.push({
      label: 'maker',
      owner: config.maker.owner,
      pk: new PublicKey(config.maker.mangoAccount),
    });
  }
  if (
    INCLUDE_BOOTSTRAP_ACCOUNTS &&
    config.taker?.mangoAccount &&
    config.taker?.owner
  ) {
    mangoAccountPks.push({
      label: 'taker',
      owner: config.taker.owner,
      pk: new PublicKey(config.taker.mangoAccount),
    });
  }

  const botsPath =
    process.env.QUOTER_BOTS_JSON_PATH ||
    runtimeConfigPath('quoter-bots-active-9125.json');
  let fallbackBot: QuoterBotSpec | null = null;
  if (fs.existsSync(botsPath)) {
    const bots = JSON.parse(fs.readFileSync(botsPath, 'utf-8')) as QuoterBotSpec[];
    fallbackBot = bots[0] ?? null;
    for (const bot of bots) {
      mangoAccountPks.push({
        label: bot.name,
        owner: bot.owner,
        pk: new PublicKey(bot.mangoAccount),
      });
    }
  }

  console.log(
    JSON.stringify({
      ts: ts(),
      msg: 'divergence-guard started',
      harness: HARNESS_URL,
      rpc: clusterUrl,
      interval_ms: INTERVAL_MS,
      auto_drain: AUTO_DRAIN,
      config_path: path.resolve(CONFIG_PATH),
      program: config.programId,
      group: config.group,
      execution_queue: config.executionQueue,
      monitored_accounts: mangoAccountPks.map((a) => ({
        label: a.label,
        owner: a.owner,
        mango_account: a.pk.toBase58(),
      })),
    }),
  );

  let group = await client.getGroup(groupPk);
  let tickCount = 0;
  let stalledTicks = 0;
  let lastNextSequence = '-1';
  let lastQueueCount = -1;
  let lastAcceptedTotal = 0;
  let lastProcessedTotal = 0;
  let lastSkippedTotal = 0;
  let lastFailedTotal = 0;
  let gapRecoveryEventsTotal = 0;
  let gapFilledSequencesTotal = 0;

  const maybeRecover = async (reason: string) => {
    if (!AUTO_DRAIN) {
      return;
    }
    const before = await inspectQueue(connection, executionQueuePk);
    if (before.header.count === 0) {
      return;
    }
    const isGapBefore =
      before.header.ctmCount > 0 &&
      before.header.maxSeenSequence >= before.header.nextSequence &&
      (!before.nextCtmSlot ||
        before.nextCtmSlot.status !== EXECUTION_QUEUE_LAYOUT.pendingStatus ||
        before.nextCtmSlot.kind !== EXECUTION_QUEUE_LAYOUT.ctmWrappedKind ||
        before.nextCtmSlot.sequence !== before.header.nextSequence);
    if (isGapBefore) {
      const nextRealSequence = findNextPendingCtmSequence(
        before.data,
        before.header.nextSequence,
        before.header.maxSeenSequence,
      );
      const filledUntil =
        nextRealSequence !== null
          ? nextRealSequence - 1n
          : before.header.maxSeenSequence;
      const missingSpan =
        filledUntil >= before.header.nextSequence
          ? Number(filledUntil - before.header.nextSequence + 1n)
          : 0;
      gapRecoveryEventsTotal += 1;
      gapFilledSequencesTotal += missingSpan;
    }
    console.log(
      JSON.stringify({
        ts: ts(),
        msg: 'queue-recovery-start',
        reason,
        queue_count: before.header.count,
        next_sequence: before.header.nextSequence.toString(),
        max_seen_sequence: before.header.maxSeenSequence.toString(),
        ctm_count: before.header.ctmCount,
        liquidity_count: before.header.liquidityCount,
      }),
    );
    const result = await drainQueue(
      client,
      group,
      groupPk,
      executionQueuePk,
      programId,
      perpMarketIndex,
      admin,
      fallbackBot,
    );
    console.log(
      JSON.stringify({
        ts: ts(),
        msg: 'queue-recovery-finish',
        reason,
        ...result,
      }),
    );
  };

  await maybeRecover('startup');

  let loopInFlight = false;

  const loop = async () => {
    tickCount += 1;
    const divergences: Divergence[] = [];
    let harnessState: HarnessFullState;
    let harnessHealth: any;

    const queueState = await inspectQueue(connection, executionQueuePk);
    const nextSequence = queueState.header.nextSequence.toString();
    if (
      queueState.header.count > 0 &&
      nextSequence === lastNextSequence &&
      queueState.header.count === lastQueueCount
    ) {
      stalledTicks += 1;
    } else {
      stalledTicks = 0;
    }
    lastNextSequence = nextSequence;
    lastQueueCount = queueState.header.count;

    try {
      [harnessState, harnessHealth] = await Promise.all([
        fetchHarnessState(),
        fetchHarnessHealth(),
      ]);
    } catch (err: any) {
      console.log(
        JSON.stringify({
          ts: ts(),
          tick: tickCount,
          queue_count: queueState.header.count,
          next_sequence: nextSequence,
          stalled_ticks: stalledTicks,
          error: `harness fetch failed: ${err.message}`,
        }),
      );
      if (stalledTicks >= STALL_TICKS) {
        await maybeRecover('harness_unavailable_and_queue_stalled');
      }
      return;
    }

    if (tickCount % 10 === 0) {
      try {
        group = await client.getGroup(groupPk);
      } catch {}
    }

    for (const entry of mangoAccountPks) {
      try {
        const onchainAccount = await client.getMangoAccount(entry.pk);
        const pp: PerpPosition | undefined = onchainAccount.getPerpPosition(
          perpMarketIndex as any,
        );
        const onchainBaseLots = pp ? pp.basePositionLots.toString() : '0';
        const onchainQuoteNative = Math.trunc(
          pp ? pp.quotePositionNative.toNumber() : 0,
        ).toString();

        const harnessUser = harnessState.users[entry.owner];
        if (!harnessUser) {
          divergences.push({
            field: 'user_missing_in_harness',
            account: entry.label,
            harness: 'missing',
            onchain: entry.owner,
          });
          continue;
        }

        const harnessPerp = harnessUser.per_market?.find(
          (pm) => pm.market === String(perpMarketIndex),
        );
        const harnessBaseLots = harnessPerp?.base_position_lots || '0';
        const harnessQuoteNative = harnessPerp?.quote_position_native || '0';

        if (onchainBaseLots !== harnessBaseLots) {
          divergences.push({
            field: 'base_position_lots',
            account: entry.label,
            harness: harnessBaseLots,
            onchain: onchainBaseLots,
            delta: (BigInt(harnessBaseLots) - BigInt(onchainBaseLots)).toString(),
          });
        }

        if (onchainQuoteNative !== harnessQuoteNative) {
          divergences.push({
            field: 'quote_position_native',
            account: entry.label,
            harness: harnessQuoteNative,
            onchain: onchainQuoteNative,
            delta: (
              BigInt(harnessQuoteNative) - BigInt(onchainQuoteNative)
            ).toString(),
          });
        }

        const harnessOOCount = harnessUser.open_orders?.length || 0;
        const onchainOOCount = onchainAccount.perpActive()
          .filter((p) => p.marketIndex === perpMarketIndex)
          .reduce(
            (sum, p) =>
              sum + (p.bidsBaseLots.toNumber() > 0 ? 1 : 0) +
              (p.asksBaseLots.toNumber() > 0 ? 1 : 0),
            0,
          );
        if ((harnessOOCount === 0) !== (onchainOOCount === 0)) {
          divergences.push({
            field: 'open_orders_presence',
            account: entry.label,
            harness: harnessOOCount,
            onchain: onchainOOCount,
          });
        }
      } catch (err: any) {
        divergences.push({
          field: 'onchain_fetch_error',
          account: entry.label,
          harness: 'n/a',
          onchain: err.message?.slice(0, 120) || String(err),
        });
      }
    }

    const market0 = harnessState.markets['0'];
    const queueStats = Object.values(harnessState.queue || {});
    const processedTotal = queueStats.reduce(
      (sum, q) => sum + (q.processed_count || 0),
      0,
    );
    const skippedTotal = queueStats.reduce(
      (sum, q) => sum + (q.skipped_count || 0),
      0,
    );
    const failedTotal = queueStats.reduce(
      (sum, q) => sum + (q.failed_count || 0),
      0,
    );
    const acceptedTotal = Number(harnessHealth.intents_total || 0);
    const acceptedDelta = Math.max(0, acceptedTotal - lastAcceptedTotal);
    const processedDelta = Math.max(0, processedTotal - lastProcessedTotal);
    const skippedDelta = Math.max(0, skippedTotal - lastSkippedTotal);
    const failedDelta = Math.max(0, failedTotal - lastFailedTotal);
    const nonSkippedTerminalDelta = Math.max(0, processedDelta - skippedDelta);
    const pNonSkippedInclusion =
      acceptedDelta > 0 ? nonSkippedTerminalDelta / acceptedDelta : null;
    const pSkippedGap = acceptedDelta > 0 ? skippedDelta / acceptedDelta : null;
    const pAnyTerminal = acceptedDelta > 0 ? processedDelta / acceptedDelta : null;
    lastAcceptedTotal = acceptedTotal;
    lastProcessedTotal = processedTotal;
    lastSkippedTotal = skippedTotal;
    lastFailedTotal = failedTotal;

    if (market0) {
      const optimisticSeq = Number(market0.watermarks.optimistic_seq);
      const confirmedSeq = Number(market0.watermarks.confirmed_seq);
      const seqGap = optimisticSeq - confirmedSeq;
      if (seqGap > 100) {
        divergences.push({
          field: 'sequence_gap',
          account: 'execution_queue',
          harness: optimisticSeq,
          onchain: confirmedSeq,
          delta: seqGap,
        });
      }
    }

    const report: any = {
      ts: ts(),
      tick: tickCount,
      harness_intents: harnessHealth.intents_total,
      harness_divergences: harnessHealth.divergences_total,
      harness_sse_clients: harnessHealth.sse_clients,
      optimistic_seq: market0?.watermarks?.optimistic_seq,
      confirmed_seq: market0?.watermarks?.confirmed_seq,
      ob_bids: market0?.bids?.length || 0,
      ob_asks: market0?.asks?.length || 0,
      monitored_accounts: mangoAccountPks.length,
      divergence_count: divergences.length,
      accepted_total: acceptedTotal,
      accepted_delta: acceptedDelta,
      onchain_terminal_total: processedTotal,
      onchain_terminal_delta: processedDelta,
      skipped_gap_total: skippedTotal,
      skipped_gap_delta: skippedDelta,
      gap_recovery_events_total: gapRecoveryEventsTotal,
      gap_filled_sequences_total: gapFilledSequencesTotal,
      failed_total: failedTotal,
      failed_delta: failedDelta,
      non_skipped_terminal_delta: nonSkippedTerminalDelta,
      p_non_skipped_inclusion:
        pNonSkippedInclusion === null ? null : Number(pNonSkippedInclusion.toFixed(4)),
      p_skipped_gap: pSkippedGap === null ? null : Number(pSkippedGap.toFixed(4)),
      p_any_terminal:
        pAnyTerminal === null ? null : Number(pAnyTerminal.toFixed(4)),
      queue_count: queueState.header.count,
      ctm_count: queueState.header.ctmCount,
      liquidity_count: queueState.header.liquidityCount,
      queue_next_sequence: nextSequence,
      queue_max_seen_sequence: queueState.header.maxSeenSequence.toString(),
      stalled_ticks: stalledTicks,
    };

    if (divergences.length > 0) {
      report.divergences = divergences;
    } else {
      report.status = 'in_sync';
    }
    console.log(JSON.stringify(report));

    const shouldRecover =
      queueState.header.count > 0 && stalledTicks >= STALL_TICKS;
    if (shouldRecover) {
      await maybeRecover('queue_stalled');
      stalledTicks = 0;
    }
  };

  await loop();
  setInterval(() => {
    if (loopInFlight) {
      return;
    }
    loopInFlight = true;
    loop()
      .catch((err) => {
        console.error(
          JSON.stringify({
            ts: ts(),
            msg: 'divergence-guard-loop-fatal',
            error: err.message || String(err),
          }),
        );
      })
      .finally(() => {
        loopInFlight = false;
      });
  }, INTERVAL_MS);
}

main().catch((err) => {
  console.error(
    JSON.stringify({
      ts: ts(),
      msg: 'divergence-guard fatal',
      error: err.message || String(err),
    }),
  );
  process.exit(1);
});
