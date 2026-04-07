/**
 * admin-drop-head.ts — Admin-drop the current CTM head item(s) via pause → drop → unpause.
 *
 * Usage:
 *   npx ts-node ts/client/scripts/execution-queue/admin-drop-head.ts [count]
 *   npx ts-node ts/client/scripts/execution-queue/admin-drop-head.ts --sequences 123,124,125
 *
 * Reads config from environment (CLUSTER_URL_OVERRIDE, EXECUTION_QUEUE_ADMIN_KEYPAIR, etc.)
 * or falls back to devnet-stack.env defaults.
 *
 * The optional [count] argument specifies how many sequential head items to drop (default 1).
 * Use --sequences to drop an explicit set of pending CTM sequences, which is useful
 * when the queue is stuck behind a front gap and there is no decodable current head item.
 */
import {
  Connection,
  Keypair,
  PublicKey,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import fs from 'fs';
import { anchorInstructionDiscriminator } from '../../src/executionQueue';
import {
  decodeExecutionQueueHeader,
  decodeExecutionQueueHeadItem,
} from '../../src/executionQueueLayout';
import { decodeQueuePayload } from '../../src/continuumHarness';

const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE ||
  'https://fermila-develope-edb9.devnet.rpcpool.com/c3f2c0dd-6bb9-46bb-bd56-e178ed73783e';
const ADMIN_KEYPAIR_PATH =
  process.env.EXECUTION_QUEUE_ADMIN_KEYPAIR ||
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  '/home/hetalkenaudekar/.config/solana/id.json';
const GROUP = new PublicKey(
  process.env.EXECUTION_QUEUE_GROUP_PK || '7SqdQ3xMta4EhztpX1VRtBPitX1cfkyXtWhq29nva9mi',
);
const EXECUTION_QUEUE = new PublicKey(
  process.env.EXECUTION_QUEUE_PK || '2gnDrvJgs3MuMVFte8Xx7KKTyeoqqLGGmcEgRpPVvCpL',
);
const PROGRAM_ID = new PublicKey(
  process.env.PROGRAM_ID || '4CGsiGHZXSnweudEcN235xkLz4twT2DJB35hS7t89cUm',
);

function u64ToLe(v: bigint): Buffer {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(v);
  return buf;
}

function buildConfigureIx(
  admin: PublicKey,
  gapWaitSlots: bigint,
  liquidityDelaySlots: bigint,
  pauseIngress: boolean,
  pauseExecute: boolean,
): TransactionInstruction {
  const discriminator = anchorInstructionDiscriminator('execution_queue_configure');
  const ixData = Buffer.concat([
    discriminator,
    u64ToLe(gapWaitSlots),
    u64ToLe(liquidityDelaySlots),
    Buffer.from([pauseIngress ? 1 : 0]),
    Buffer.from([pauseExecute ? 1 : 0]),
  ]);
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: GROUP, isSigner: false, isWritable: false },
      { pubkey: EXECUTION_QUEUE, isSigner: false, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
    ],
    data: ixData,
  });
}

function buildDropCtmIx(admin: PublicKey, sequence: bigint): TransactionInstruction {
  const discriminator = anchorInstructionDiscriminator('execution_queue_drop_ctm');
  const ixData = Buffer.concat([discriminator, u64ToLe(sequence)]);
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: GROUP, isSigner: false, isWritable: false },
      { pubkey: EXECUTION_QUEUE, isSigner: false, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
    ],
    data: ixData,
  });
}

async function main() {
  const args = process.argv.slice(2);
  let dropCount = 1;
  let explicitSequences: bigint[] = [];
  if (args[0] === '--sequences' || args[0] === '--seq') {
    const raw = args[1] || '';
    explicitSequences = raw
      .split(',')
      .map((value) => value.trim())
      .filter((value) => value.length > 0)
      .map((value) => BigInt(value));
    if (explicitSequences.length === 0) {
      throw new Error('expected comma-separated sequences after --sequences');
    }
  } else {
    dropCount = Math.max(1, Number(args[0] || '1'));
  }
  const adminKey = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(ADMIN_KEYPAIR_PATH, 'utf-8'))),
  );
  const connection = new Connection(CLUSTER_URL, 'confirmed');

  // Read current queue state
  const queueAccount = await connection.getAccountInfo(EXECUTION_QUEUE);
  if (!queueAccount) throw new Error('queue account not found');
  const data = queueAccount.data;

  const header = decodeExecutionQueueHeader(data);
  const headItem = decodeExecutionQueueHeadItem(data);

  console.log(
    `Queue: count=${header.count} next=${header.nextSequence} max=${header.maxSeenSequence} ctm=${header.ctmCount}`,
  );

  if (!headItem && explicitSequences.length === 0) {
    console.log('No head item found — nothing to drop.');
    return;
  }

  let decoded: unknown = null;
  try {
    decoded = decodeQueuePayload(headItem.payload);
  } catch {
    decoded = null;
  }

  if (headItem) {
    console.log(
      `Head: sequence=${headItem.sequence} kind=${headItem.kind} status=${headItem.status} ` +
        `accountsHash=${headItem.accountsHash.toString('hex')} ` +
        `payloadHash=${headItem.payloadHash.toString('hex')}`,
    );
    if (decoded) {
      console.log(`Decoded payload: ${JSON.stringify(decoded, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))}`);
    }
  }

  const gapWaitSlots = data.readBigUInt64LE(184);
  const liquidityDelaySlots = data.readBigUInt64LE(192);
  const pauseIngress = data.readUInt8(145) !== 0;
  const currentPauseExecute = data.readUInt8(146) !== 0;

  const sequencesToDrop: bigint[] =
    explicitSequences.length > 0
      ? explicitSequences
      : (() => {
          const headSequence = BigInt(headItem!.sequence.toString());
          const sequences: bigint[] = [];
          for (let i = 0; i < dropCount; i++) {
            sequences.push(headSequence + BigInt(i));
          }
          return sequences;
        })();

  console.log(
    `Dropping ${sequencesToDrop.length} item(s): sequences=[${sequencesToDrop.join(', ')}]`,
  );

  // Build atomic tx: pause → drop(s) → unpause
  const instructions: TransactionInstruction[] = [
    buildConfigureIx(adminKey.publicKey, gapWaitSlots, liquidityDelaySlots, pauseIngress, true),
  ];
  for (const seq of sequencesToDrop) {
    instructions.push(buildDropCtmIx(adminKey.publicKey, seq));
  }
  instructions.push(
    buildConfigureIx(
      adminKey.publicKey,
      gapWaitSlots,
      liquidityDelaySlots,
      pauseIngress,
      currentPauseExecute,
    ),
  );

  const { blockhash } = await connection.getLatestBlockhash();
  const message = new TransactionMessage({
    payerKey: adminKey.publicKey,
    recentBlockhash: blockhash,
    instructions,
  }).compileToV0Message();

  const tx = new VersionedTransaction(message);
  tx.sign([adminKey]);
  const sig = await connection.sendTransaction(tx);
  console.log(`Admin drop tx sent: ${sig}`);
  await connection.confirmTransaction(sig, 'confirmed');
  console.log('Confirmed.');

  // Verify
  const after = await connection.getAccountInfo(EXECUTION_QUEUE);
  if (after) {
    const afterHeader = decodeExecutionQueueHeader(after.data);
    console.log(
      `After: count=${afterHeader.count} next=${afterHeader.nextSequence} max=${afterHeader.maxSeenSequence}`,
    );
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
