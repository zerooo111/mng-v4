/**
 * init-queue-authority-state.ts
 *
 * One-shot: initialize the ExecutionQueueAuthorityState PDA for the group.
 * Run this after v5-bootstrap-3-markets.ts creates the group but before
 * v5-bootstrap-per-market.ts creates per-market queues.
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE
 *   CTM_RELAYER_PROGRAM_ID
 *   MB_PAYER_KEYPAIR              — admin + payer
 *   EXECUTION_QUEUE_GROUP_NUM
 *
 * Optional:
 *   CTM_SIGNER                    — pubkey; defaults to admin pubkey
 */
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import fs from 'fs';

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`missing required env: ${name}`);
  return v;
}

const CLUSTER_URL = requireEnv('CLUSTER_URL_OVERRIDE');
const PROGRAM_ID = new PublicKey(requireEnv('CTM_RELAYER_PROGRAM_ID'));
const ADMIN_KEYPAIR = requireEnv('MB_PAYER_KEYPAIR');
const GROUP_NUM = Number(requireEnv('EXECUTION_QUEUE_GROUP_NUM'));

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

function disc(name: string): Buffer {
  return createHash('sha256').update(`global:${name}`).digest().subarray(0, 8);
}

function deriveGroup(admin: PublicKey, groupNum: number): PublicKey {
  const buf = Buffer.alloc(4);
  buf.writeUInt32LE(groupNum);
  const [pk] = PublicKey.findProgramAddressSync(
    [Buffer.from('Group'), admin.toBuffer(), buf],
    PROGRAM_ID,
  );
  return pk;
}

function deriveAuthorityState(group: PublicKey): PublicKey {
  const [pk] = PublicKey.findProgramAddressSync(
    [Buffer.from('queue-authority'), group.toBuffer()],
    PROGRAM_ID,
  );
  return pk;
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, { commitment: 'confirmed' });
  const group = deriveGroup(admin.publicKey, GROUP_NUM);
  const authority = deriveAuthorityState(group);
  const ctmSigner = process.env.CTM_SIGNER
    ? new PublicKey(process.env.CTM_SIGNER)
    : admin.publicKey;

  const existing = await connection.getAccountInfo(authority, 'confirmed');
  if (existing) {
    console.log(JSON.stringify({
      msg: 'already_initialized',
      authority: authority.toBase58(),
      size: existing.data.length,
      owner: existing.owner.toBase58(),
    }));
    return;
  }

  const data = Buffer.concat([
    disc('execution_queue_v3_init_authority_state'),
    ctmSigner.toBuffer(),
  ]);
  const ix = new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: true },
      { pubkey: authority, isSigner: false, isWritable: true },
      { pubkey: admin.publicKey, isSigner: true, isWritable: true }, // payer
      { pubkey: admin.publicKey, isSigner: true, isWritable: false }, // admin
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data,
  });
  const tx = new Transaction().add(ix);
  const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
    commitment: 'confirmed',
    skipPreflight: false,
  });
  console.log(JSON.stringify({
    msg: 'initialized',
    authority: authority.toBase58(),
    ctm_signer: ctmSigner.toBase58(),
    sig,
  }));
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
