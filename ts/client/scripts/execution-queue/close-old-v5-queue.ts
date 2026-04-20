// One-shot: close the abandoned shared-queue PDA `7a2zzAA...` to reclaim
// its ~2.75 SOL rent. The new program's ExecutionQueueV5 type is a
// different (smaller) layout, but Anchor's `close = receiver` does not
// touch the items array — it just zeroes the discriminator and transfers
// lamports — so a size mismatch doesn't block the close.

import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import fs from 'fs';

const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE!;
const PROGRAM_ID = new PublicKey(process.env.CTM_RELAYER_PROGRAM_ID!);
const ADMIN_KEYPAIR = process.env.MB_PAYER_KEYPAIR!;
const QUEUE = new PublicKey('7a2zzAAczhycRH6vK6kMyRzkVV3mm2nSJCowPqGin5Yp');
const GROUP = new PublicKey(process.env.V4_GROUP!);
const AUTHORITY_STATE = new PublicKey(process.env.V4_AUTHORITY_STATE!);

const admin = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(fs.readFileSync(ADMIN_KEYPAIR, 'utf-8'))),
);

const connection = new Connection(CLUSTER_URL, 'confirmed');

function disc(name: string): Buffer {
  return createHash('sha256').update(`global:${name}`).digest().subarray(0, 8);
}

async function main() {
  const beforeBal = await connection.getBalance(admin.publicKey);
  console.log('payer balance before:', beforeBal / 1e9);

  const ix = new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: GROUP, isSigner: false, isWritable: false },
      { pubkey: AUTHORITY_STATE, isSigner: false, isWritable: false },
      { pubkey: QUEUE, isSigner: false, isWritable: true },
      { pubkey: admin.publicKey, isSigner: true, isWritable: true }, // receiver
      { pubkey: admin.publicKey, isSigner: true, isWritable: false }, // admin
    ],
    data: disc('execution_queue_v5_close'),
  });

  const tx = new Transaction().add(ix);
  const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
    commitment: 'confirmed',
  });
  console.log('close tx:', sig);

  const afterBal = await connection.getBalance(admin.publicKey);
  console.log(
    'payer balance after:',
    afterBal / 1e9,
    '(delta:',
    (afterBal - beforeBal) / 1e9,
    'SOL)',
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
