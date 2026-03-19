/**
 * Devnet E2E Happy Path Tests
 *
 * Tests against the live devnet deployment:
 *   Program: 7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37
 *   Group:   42ZYDpNAUM8NZgmuQHwrUXsEWb98AkjJfJHYFFoCZpCi
 *   Queue:   2Bs3tGdMs4PQV98qFLXaafSi8AJWH55eT2WwN8ZKJMkn
 *
 * Run:
 *   npx ts-node --compiler-options '{"module":"commonjs"}' ts/client/src/e2e/devnet-e2e-happy.ts
 */
import {
  Connection,
  PublicKey,
  Keypair,
  Transaction,
  sendAndConfirmTransaction,
} from '@solana/web3.js';
import * as fs from 'fs';
import {
  decodeExecutionQueueHeader,
  decodeExecutionQueueHeadItem,
} from '../executionQueueLayout';
import {
  encodePerpCancelAllOrdersQueuePayload,
  buildExecutionQueueEnqueueCtmWithIntentIxs,
  buildExecutionQueueExecuteIx,
} from '../executionQueue';

// ── Devnet constants ──
const RPC = 'https://api.devnet.solana.com';
const PROGRAM_ID = new PublicKey('7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37');
const GROUP = new PublicKey('42ZYDpNAUM8NZgmuQHwrUXsEWb98AkjJfJHYFFoCZpCi');
const EXEC_QUEUE = new PublicKey('2Bs3tGdMs4PQV98qFLXaafSi8AJWH55eT2WwN8ZKJMkn');
const PERP_MARKET = new PublicKey('2K6J5ACxtxh6zjKagTi3AzJpde1CHmWHQa2gyWwN2Aaf');
const BIDS = new PublicKey('4xM6BxBjL5Ec13A6A4zTxNivvXV8GuBykxSYjhoQVdFc');
const ASKS = new PublicKey('HgRts1YfoTEYNdAjBhDVMD1YFqowN6Q2wpxVkydzTHmw');
// Maker mango account (owner: 9p74BNtmhvNiJuat8QLTPXbjMjcxaTZAaGPu3FNx26yg)
const MAKER_MANGO_ACCOUNT = new PublicKey('ePaFExiRQsj7z5CrXCBJMKbu8KwkmwYg5uv4gXbGPey');

function loadKeypair(path: string): Keypair {
  const raw = fs.readFileSync(path, 'utf-8');
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

async function readQueueHeader(conn: Connection) {
  const ai = await conn.getAccountInfo(EXEC_QUEUE);
  if (!ai) throw new Error('Queue account not found');
  return decodeExecutionQueueHeader(Buffer.from(ai.data));
}

interface TestResult {
  name: string;
  passed: boolean;
  error?: string;
  duration_ms: number;
}

const results: TestResult[] = [];

async function runTest(name: string, fn: () => Promise<void>) {
  const start = Date.now();
  try {
    await fn();
    const duration = Date.now() - start;
    console.log(`  ✓ ${name} (${duration}ms)`);
    results.push({ name, passed: true, duration_ms: duration });
  } catch (e: any) {
    const duration = Date.now() - start;
    const msg = e.message?.slice(0, 200) ?? String(e);
    console.log(`  ✗ ${name} (${duration}ms): ${msg}`);
    results.push({ name, passed: false, error: msg, duration_ms: duration });
  }
}

/** Build remaining_accounts for PerpCancelAllOrders dispatch */
function cancelAllRemainingAccounts(
  mangoAccount: PublicKey,
  ownerPk: PublicKey,
) {
  return [
    { pubkey: GROUP, isWritable: true, isSigner: false },
    { pubkey: mangoAccount, isWritable: true, isSigner: false },
    { pubkey: ownerPk, isWritable: false, isSigner: false },
    { pubkey: PERP_MARKET, isWritable: true, isSigner: false },
    { pubkey: BIDS, isWritable: true, isSigner: false },
    { pubkey: ASKS, isWritable: true, isSigner: false },
  ];
}

async function main() {
  const conn = new Connection(RPC, 'confirmed');

  // Load keypairs
  const payer = loadKeypair(`${process.env.HOME}/.config/solana/id.json`);
  const ctmSigner = payer; // CTM signer is the payer (4zDJ8...)
  let maker: Keypair;
  try {
    maker = loadKeypair('keypairs/execution-queue-maker.json');
  } catch {
    maker = payer;
  }

  console.log('Payer/CTM:', payer.publicKey.toBase58());
  console.log('Maker:', maker.publicKey.toBase58());
  console.log('Maker mango account:', MAKER_MANGO_ACCOUNT.toBase58());
  console.log();

  const remaining = cancelAllRemainingAccounts(MAKER_MANGO_ACCOUNT, maker.publicKey);

  console.log('=== Running E2E Happy Path Tests ===\n');

  // ── Test 1: Read queue state ──
  await runTest('read_queue_state', async () => {
    const header = await readQueueHeader(conn);
    if (header.totalCount < 0) throw new Error('negative count');
    if (!header.headerInvariantOk) throw new Error('header invariant violated');
    console.log(
      `    Queue: total=${header.totalCount} ctm=${header.ctmCount} liq=${header.liquidityCount} nextSeq=${header.nextSequence}`,
    );
  });

  // ── Test 2: Queue is unpaused ──
  await runTest('queue_is_unpaused', async () => {
    const ai = await conn.getAccountInfo(EXEC_QUEUE);
    if (!ai) throw new Error('no account');
    const data = Buffer.from(ai.data);
    if (data[145] !== 0) throw new Error(`ingress paused`);
    if (data[146] !== 0) throw new Error(`execute paused`);
  });

  // ── Test 3: Enqueue CTM cancel-all ──
  await runTest('enqueue_ctm_cancel_all', async () => {
    const headerBefore = await readQueueHeader(conn);
    const seq = Number(headerBefore.maxSeenSequence) + 1;
    const slot = await conn.getSlot();

    const payload = encodePerpCancelAllOrdersQueuePayload({ limit: 10 });

    const built = await buildExecutionQueueEnqueueCtmWithIntentIxs({
      programId: PROGRAM_ID,
      group: GROUP,
      executionQueue: EXEC_QUEUE,
      executionQueueBuffer: EXEC_QUEUE,
      remainingAccounts: remaining,
      payload,
      sequence: seq,
      minExecuteSlot: slot + 2,
      mangoAccount: MAKER_MANGO_ACCOUNT,
      userOwner: maker.publicKey,
      userSigner: { kind: 'keypair', privateKey: maker.secretKey },
      ctmSigner: { kind: 'keypair', privateKey: ctmSigner.secretKey },
    });

    const tx = new Transaction();
    tx.add(...built.instructions);
    tx.feePayer = payer.publicKey;
    const { blockhash } = await conn.getLatestBlockhash();
    tx.recentBlockhash = blockhash;

    // Only payer signs the tx — maker's intent is proven via ed25519 pre-instruction
    const signers = [payer];
    const sig = await sendAndConfirmTransaction(conn, tx, signers, {
      commitment: 'confirmed',
    });
    console.log(`    Enqueued seq=${seq}, tx=${sig}`);

    await sleep(2000);
    const headerAfter = await readQueueHeader(conn);
    if (headerAfter.ctmCount <= headerBefore.ctmCount) {
      throw new Error(
        `CTM count didn't increase: ${headerBefore.ctmCount} → ${headerAfter.ctmCount}`,
      );
    }
    console.log(`    Queue after: total=${headerAfter.totalCount} ctm=${headerAfter.ctmCount}`);
  });

  // ── Test 4: Execute drains the enqueued item ──
  await runTest('execute_drains_enqueued_item', async () => {
    const headerBefore = await readQueueHeader(conn);
    if (headerBefore.totalCount === 0) {
      throw new Error('Queue is empty — nothing to execute');
    }

    // Wait for min_execute_slot
    await sleep(3000);

    const executeIx = await buildExecutionQueueExecuteIx({
      programId: PROGRAM_ID,
      group: GROUP,
      executionQueue: EXEC_QUEUE,
      executionQueueBuffer: EXEC_QUEUE,
      maxItems: 10,
      remainingAccounts: remaining,
    });

    const tx = new Transaction();
    tx.add(executeIx);
    tx.feePayer = payer.publicKey;
    const { blockhash } = await conn.getLatestBlockhash();
    tx.recentBlockhash = blockhash;

    const sig = await sendAndConfirmTransaction(conn, tx, [payer], {
      commitment: 'confirmed',
    });
    console.log(`    Execute tx=${sig}`);

    await sleep(2000);
    const headerAfter = await readQueueHeader(conn);
    console.log(`    Queue after: total=${headerAfter.totalCount} ctm=${headerAfter.ctmCount}`);
    if (headerAfter.totalCount >= headerBefore.totalCount) {
      throw new Error(
        `Queue didn't drain: ${headerBefore.totalCount} → ${headerAfter.totalCount}`,
      );
    }
  });

  // ── Test 5: Full enqueue+execute roundtrip ──
  await runTest('enqueue_execute_roundtrip', async () => {
    const headerBefore = await readQueueHeader(conn);
    const seq = Number(headerBefore.maxSeenSequence) + 1;
    const slot = await conn.getSlot();

    const payload = encodePerpCancelAllOrdersQueuePayload({ limit: 5 });

    // Enqueue
    const built = await buildExecutionQueueEnqueueCtmWithIntentIxs({
      programId: PROGRAM_ID,
      group: GROUP,
      executionQueue: EXEC_QUEUE,
      executionQueueBuffer: EXEC_QUEUE,
      remainingAccounts: remaining,
      payload,
      sequence: seq,
      minExecuteSlot: slot + 1,
      mangoAccount: MAKER_MANGO_ACCOUNT,
      userOwner: maker.publicKey,
      userSigner: { kind: 'keypair', privateKey: maker.secretKey },
      ctmSigner: { kind: 'keypair', privateKey: ctmSigner.secretKey },
    });

    const enqueueTx = new Transaction();
    enqueueTx.add(...built.instructions);
    enqueueTx.feePayer = payer.publicKey;
    const { blockhash: bh1 } = await conn.getLatestBlockhash();
    enqueueTx.recentBlockhash = bh1;

    // Only payer signs — maker's intent is proven via ed25519 pre-instruction
    const enqueueSigners = [payer];
    const enqueueSig = await sendAndConfirmTransaction(conn, enqueueTx, enqueueSigners, {
      commitment: 'confirmed',
    });
    console.log(`    Enqueued seq=${seq}, tx=${enqueueSig}`);

    // Wait then execute
    await sleep(3000);

    const executeIx = await buildExecutionQueueExecuteIx({
      programId: PROGRAM_ID,
      group: GROUP,
      executionQueue: EXEC_QUEUE,
      executionQueueBuffer: EXEC_QUEUE,
      maxItems: 10,
      remainingAccounts: remaining,
    });

    const executeTx = new Transaction();
    executeTx.add(executeIx);
    executeTx.feePayer = payer.publicKey;
    const { blockhash: bh2 } = await conn.getLatestBlockhash();
    executeTx.recentBlockhash = bh2;

    const executeSig = await sendAndConfirmTransaction(conn, executeTx, [payer], {
      commitment: 'confirmed',
    });
    console.log(`    Executed tx=${executeSig}`);

    await sleep(2000);
    const headerAfter = await readQueueHeader(conn);
    console.log(
      `    Queue: before=${headerBefore.totalCount} after=${headerAfter.totalCount}`,
    );
    if (headerAfter.totalCount > headerBefore.totalCount) {
      throw new Error(
        `Queue grew: ${headerBefore.totalCount} → ${headerAfter.totalCount}`,
      );
    }
  });

  // ── Summary ──
  console.log('\n=== E2E Test Summary ===');
  const passed = results.filter((r) => r.passed).length;
  const failed = results.filter((r) => !r.passed).length;
  console.log(`${passed} passed, ${failed} failed\n`);
  for (const r of results) {
    console.log(`  ${r.passed ? '✓' : '✗'} ${r.name} (${r.duration_ms}ms)${r.error ? ' — ' + r.error : ''}`);
  }

  if (failed > 0) process.exit(1);
}

main().catch((e) => {
  console.error('Fatal:', e.message ?? e);
  process.exit(1);
});
