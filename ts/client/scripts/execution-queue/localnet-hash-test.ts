/**
 * localnet-hash-test.ts — Localnet end-to-end hash matching diagnostic.
 *
 * 1. Connects to a running local validator (expects program already deployed)
 * 2. Bootstraps group, queue, maker account
 * 3. Enqueues a PerpPlaceOrderV2 intent
 * 4. Reads back the stored accounts_hash from the queue
 * 5. Computes the hash client-side using the same remaining_accounts
 * 6. Sends execute_multi and verifies the item is processed
 *
 * Usage:
 *   # Start validator first:
 *   solana-test-validator --bpf-program Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt target/deploy/mango_v4.so --reset
 *
 *   # Then run:
 *   CLUSTER_URL_OVERRIDE=http://127.0.0.1:8899 \
 *   npx ts-node ts/client/scripts/execution-queue/localnet-hash-test.ts
 */
import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Cluster,
  Connection,
  Keypair,
  PublicKey,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import {
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';
import {
  buildExecutionQueueEnqueueCtmIx,
  encodePerpPlaceOrderV2QueuePayload,
  buildExecutionQueueUserIntent,
  signExecutionQueueIntentMessage,
  anchorInstructionDiscriminator,
  hashExecutionQueueAccounts,
} from '../../src/executionQueue';
import {
  decodeExecutionQueueHeadItem,
  decodeExecutionQueueHeader,
  EXECUTION_QUEUE_ACCOUNT_SPACE,
} from '../../src/executionQueueLayout';

dotenv.config();

const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || 'http://127.0.0.1:8899';
const PROGRAM_ID = new PublicKey(
  process.env.CTM_RELAYER_PROGRAM_ID || 'Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt',
);

function hashAccountsLocal(accounts: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[]): string {
  const buf = Buffer.alloc(accounts.length * 34);
  let off = 0;
  for (const a of accounts) {
    a.pubkey.toBuffer().copy(buf, off); off += 32;
    buf[off++] = a.isSigner ? 1 : 0;
    buf[off++] = a.isWritable ? 1 : 0;
  }
  return createHash('sha256').update(buf).digest('hex');
}

function mergeRuntimeFlags(
  remaining: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[],
  fixed: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[],
) {
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const a of [...fixed, ...remaining]) {
    const key = a.pubkey.toBase58();
    const e = merged.get(key);
    if (e) { e.isSigner ||= a.isSigner; e.isWritable ||= a.isWritable; }
    else merged.set(key, { isSigner: a.isSigner, isWritable: a.isWritable });
  }
  return remaining.map(a => ({
    pubkey: a.pubkey,
    ...merged.get(a.pubkey.toBase58())!,
  }));
}

async function main() {
  const connection = new Connection(CLUSTER_URL, 'confirmed');

  // Use the e2e config if it exists (from a prior bootstrap), otherwise bootstrap
  const e2eConfigPath = process.env.E2E_OUTPUT_CONFIG_PATH || '';
  let config: any;

  if (e2eConfigPath && fs.existsSync(e2eConfigPath)) {
    config = JSON.parse(fs.readFileSync(e2eConfigPath, 'utf-8'));
    console.log(`Using existing e2e config from ${e2eConfigPath}`);
  } else {
    console.log('No e2e config found. Run local-perp-e2e-bootstrap.ts first.');
    process.exit(1);
  }

  const groupPk = new PublicKey(config.group);
  const queuePk = new PublicKey(config.executionQueue);
  const makerKp = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(config.maker.keypairPath, 'utf-8'))),
  );
  const makerMangoAccount = new PublicKey(config.maker.mangoAccount);
  const ctmKp = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(config.relayer?.ctmKeypairPath || config.relayer?.payerKeypairPath, 'utf-8'))),
  );
  const adminKp = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(config.relayer?.payerKeypairPath, 'utf-8'))),
  );

  // Read the lane config
  const laneConfigPath = config.cranker?.laneConfigPath || '';
  if (!laneConfigPath || !fs.existsSync(laneConfigPath)) {
    console.error(`No lane config at ${laneConfigPath}`);
    process.exit(1);
  }
  const lanes = JSON.parse(fs.readFileSync(laneConfigPath, 'utf-8'));
  const makerLane = lanes.find((l: any) => l.name?.includes('maker') && l.name?.includes('place')) || lanes[0];

  console.log(`\nGroup: ${groupPk.toBase58()}`);
  console.log(`Queue: ${queuePk.toBase58()}`);
  console.log(`Maker: ${makerKp.publicKey.toBase58()}`);
  console.log(`MangoAccount: ${makerMangoAccount.toBase58()}`);
  console.log(`Lane: ${makerLane.name} (${makerLane.remainingAccounts.length} accounts)`);

  // Read queue state
  const queueInfo = await connection.getAccountInfo(queuePk);
  if (!queueInfo) { console.error('Queue not found'); process.exit(1); }
  const header = decodeExecutionQueueHeader(queueInfo.data);
  console.log(`\nQueue state: count=${header.count} next=${header.nextSequence} max=${header.maxSeenSequence}`);

  // Build remaining_accounts from the lane config
  const remainingAccounts: AccountMeta[] = makerLane.remainingAccounts.map((a: any) => ({
    pubkey: new PublicKey(a.pubkey),
    isSigner: !!a.isSigner,
    isWritable: !!a.isWritable,
  }));

  // Compute hash: raw
  const rawHash = hashAccountsLocal(remainingAccounts.map(a => ({
    pubkey: a.pubkey, isSigner: a.isSigner, isWritable: a.isWritable,
  })));

  // Compute hash: with enqueue-style merge (group+queue+sysvar as fixed)
  const enqueueFixed = [
    { pubkey: groupPk, isSigner: false, isWritable: true },
    { pubkey: queuePk, isSigner: false, isWritable: true },
    { pubkey: SYSVAR_INSTRUCTIONS_PUBKEY, isSigner: false, isWritable: false },
  ];
  const enqueueAccounts = mergeRuntimeFlags(
    remainingAccounts.map(a => ({ pubkey: a.pubkey, isSigner: a.isSigner, isWritable: a.isWritable })),
    enqueueFixed,
  );
  const enqueueHash = hashAccountsLocal(enqueueAccounts);

  // Compute hash: with execute-style merge (group+queue only, no sysvar)
  const executeFixed = [
    { pubkey: groupPk, isSigner: false, isWritable: true },
    { pubkey: queuePk, isSigner: false, isWritable: true },
  ];
  const executeAccounts = mergeRuntimeFlags(
    remainingAccounts.map(a => ({ pubkey: a.pubkey, isSigner: a.isSigner, isWritable: a.isWritable })),
    executeFixed,
  );
  const executeHash = hashAccountsLocal(executeAccounts);

  console.log(`\nHash computations:`);
  console.log(`  raw:     ${rawHash.slice(0, 16)}...`);
  console.log(`  enqueue: ${enqueueHash.slice(0, 16)}... (merge w/ group+queue+sysvar)`);
  console.log(`  execute: ${executeHash.slice(0, 16)}... (merge w/ group+queue)`);
  console.log(`  differ?  enqueue vs execute: ${enqueueHash === executeHash ? 'SAME' : 'DIFFERENT !!!'}`);

  // Show per-account flag diff
  for (let i = 0; i < remainingAccounts.length; i++) {
    const eq = enqueueAccounts[i];
    const ex = executeAccounts[i];
    if (eq.isWritable !== ex.isWritable || eq.isSigner !== ex.isSigner) {
      console.log(`  [${i}] ${eq.pubkey.toBase58().slice(0, 12)}... enqueue(w=${eq.isWritable},s=${eq.isSigner}) != execute(w=${ex.isWritable},s=${ex.isSigner})`);
    }
  }

  // Now check what the on-chain enqueue actually stores
  if (header.count > 0) {
    const headItem = decodeExecutionQueueHeadItem(queueInfo.data);
    if (headItem) {
      const storedHash = headItem.accountsHash.toString('hex');
      console.log(`\nStored head hash: ${storedHash.slice(0, 16)}...`);
      console.log(`  matches enqueue? ${storedHash === enqueueHash ? 'YES' : 'NO'}`);
      console.log(`  matches execute? ${storedHash === executeHash ? 'YES' : 'NO'}`);
      console.log(`  matches raw?     ${storedHash === rawHash ? 'YES' : 'NO'}`);
    }
  } else {
    console.log('\nQueue empty — enqueue an item to compare stored hash.');
    console.log('Run the quoter or local-perp-e2e-run.ts to populate.');
  }

  // Now try the execute path: send execute_multi and see if items advance
  if (header.count > 0) {
    console.log('\n--- Testing execute_multi ---');

    const apl = remainingAccounts.length;
    const laneHashes = [Buffer.alloc(32)]; // deprecated, ignored by C-1 fix
    const discriminator = anchorInstructionDiscriminator('execution_queue_execute_multi');

    // Build instruction data
    const maxItems = 8;
    const laneCount = 1;
    const ixData = Buffer.concat([
      discriminator,
      Buffer.from(new Uint16Array([maxItems]).buffer),    // max_items: u16
      Buffer.from([laneCount]),                            // lane_count: u8
      Buffer.from(new Uint16Array([apl]).buffer),          // accounts_per_lane: u16
      Buffer.from(new Uint32Array([laneCount]).buffer),    // vec length prefix
      ...laneHashes.map(h => Buffer.from(h)),              // lane hashes
    ]);

    const executeIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      accounts: [
        { pubkey: groupPk, isSigner: false, isWritable: true },
        { pubkey: queuePk, isSigner: false, isWritable: true },
        ...remainingAccounts,
      ],
      data: ixData,
    });

    const tx = new Transaction().add(executeIx);
    try {
      const sig = await sendAndConfirmTransaction(connection, tx, [adminKp], {
        commitment: 'confirmed',
        skipPreflight: true,
      });
      console.log(`Execute tx confirmed: ${sig}`);

      // Check if head advanced
      const afterInfo = await connection.getAccountInfo(queuePk);
      if (afterInfo) {
        const afterHeader = decodeExecutionQueueHeader(afterInfo.data);
        const advanced = BigInt(afterHeader.nextSequence.toString()) - BigInt(header.nextSequence.toString());
        console.log(`Queue after: count=${afterHeader.count} next=${afterHeader.nextSequence}`);
        console.log(`Head advanced by: ${advanced} items`);
        if (Number(advanced) > 0) {
          console.log('*** SUCCESS: Execute processed items! Hash matching works. ***');
        } else {
          console.log('*** FAIL: Head did not advance. Hash mismatch on-chain. ***');

          // Get tx logs
          const txInfo = await connection.getTransaction(sig, { commitment: 'confirmed', maxSupportedTransactionVersion: 0 });
          if (txInfo?.meta?.logMessages) {
            console.log('\nTx logs:');
            for (const log of txInfo.meta.logMessages) {
              if (log.includes('HLT') || log.includes('Program log')) {
                console.log(`  ${log}`);
              }
            }
          }
        }
      }
    } catch (err: any) {
      console.error(`Execute failed: ${err.message || err}`);
      // Try to get logs from failed tx
      if (err.signature) {
        const txInfo = await connection.getTransaction(err.signature, { commitment: 'confirmed' });
        if (txInfo?.meta?.logMessages) {
          console.log('\nFailed tx logs:');
          for (const log of txInfo.meta.logMessages) {
            console.log(`  ${log}`);
          }
        }
      }
    }
  }
}

main().catch((e) => { console.error(e); process.exit(1); });
