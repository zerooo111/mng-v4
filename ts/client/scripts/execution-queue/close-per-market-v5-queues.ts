/**
 * close-per-market-v5-queues.ts
 *
 * Closes each per-market ExecutionQueueV5 PDA listed in V5_MARKETS. Uses
 * the admin-gated `execution_queue_v5_close` ix — Anchor's `close =
 * receiver` zero-s the discriminator and refunds rent, so the PDA can be
 * re-created fresh at the same seed by v5-bootstrap-per-market.ts.
 *
 * Required env (shares the bootstrap script's env):
 *   CLUSTER_URL_OVERRIDE
 *   CTM_RELAYER_PROGRAM_ID
 *   MB_PAYER_KEYPAIR             — admin + fee-payer + receiver
 *   EXECUTION_QUEUE_GROUP_NUM
 *   V5_MARKETS                   — CSV of market_index values, e.g. "2,3,4"
 *
 * Idempotent: skips markets whose PDA is already closed (owner !=
 * program_id or account missing).
 */
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

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`missing required env: ${name}`);
  return v;
}

const CLUSTER_URL = requireEnv('CLUSTER_URL_OVERRIDE');
const PROGRAM_ID = new PublicKey(requireEnv('CTM_RELAYER_PROGRAM_ID'));
const ADMIN_KEYPAIR = requireEnv('MB_PAYER_KEYPAIR');
const GROUP_NUM = Number(requireEnv('EXECUTION_QUEUE_GROUP_NUM'));
const MARKETS_CSV = requireEnv('V5_MARKETS');
const MARKET_INDICES = MARKETS_CSV.split(',')
  .map((s) => s.trim())
  .filter((s) => s.length)
  .map((s) => Number(s));

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

function disc(name: string): Buffer {
  return createHash('sha256').update(`global:${name}`).digest().subarray(0, 8);
}

function u16le(n: number): Buffer {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n, 0);
  return b;
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

function deriveQueueAuthority(group: PublicKey): PublicKey {
  const [pk] = PublicKey.findProgramAddressSync(
    [Buffer.from('queue-authority'), group.toBuffer()],
    PROGRAM_ID,
  );
  return pk;
}

function deriveQueueV5(
  group: PublicKey,
  marketIndex: number,
): PublicKey {
  const [pk] = PublicKey.findProgramAddressSync(
    [Buffer.from('execution-queue-v5'), group.toBuffer(), u16le(marketIndex)],
    PROGRAM_ID,
  );
  return pk;
}

function ixClose(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  receiver: PublicKey,
  admin: PublicKey,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: false },
      { pubkey: authorityState, isSigner: false, isWritable: false },
      { pubkey: queue, isSigner: false, isWritable: true },
      { pubkey: receiver, isSigner: true, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
    ],
    data: disc('execution_queue_v5_close'),
  });
}

function log(o: Record<string, unknown>): void {
  console.log(JSON.stringify({ ts: new Date().toISOString(), ...o }));
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, { commitment: 'confirmed' });
  const group = deriveGroup(admin.publicKey, GROUP_NUM);
  const authorityState = deriveQueueAuthority(group);

  log({
    msg: 'close_start',
    group: group.toBase58(),
    authority_state: authorityState.toBase58(),
    markets: MARKET_INDICES,
  });

  const startBal = await connection.getBalance(admin.publicKey, 'confirmed');
  log({ msg: 'admin_balance_before', sol: startBal / 1e9 });

  for (const mi of MARKET_INDICES) {
    const queue = deriveQueueV5(group, mi);
    const acct = await connection.getAccountInfo(queue, 'confirmed');
    if (acct === null) {
      log({ market: mi, queue: queue.toBase58(), step: 'skip_missing' });
      continue;
    }
    if (!acct.owner.equals(PROGRAM_ID)) {
      log({
        market: mi,
        queue: queue.toBase58(),
        step: 'skip_wrong_owner',
        owner: acct.owner.toBase58(),
      });
      continue;
    }
    log({
      market: mi,
      queue: queue.toBase58(),
      step: 'closing',
      lamports: acct.lamports,
      size: acct.data.length,
    });
    const tx = new Transaction().add(
      ixClose(group, authorityState, queue, admin.publicKey, admin.publicKey),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
      skipPreflight: false,
    });
    log({ market: mi, queue: queue.toBase58(), step: 'closed', sig });

    const after = await connection.getAccountInfo(queue, 'confirmed');
    log({
      market: mi,
      queue: queue.toBase58(),
      step: 'post_close',
      still_exists: after !== null,
      owner_after: after ? after.owner.toBase58() : null,
    });
  }

  const endBal = await connection.getBalance(admin.publicKey, 'confirmed');
  log({
    msg: 'close_done',
    sol_before: startBal / 1e9,
    sol_after: endBal / 1e9,
    delta_sol: (endBal - startBal) / 1e9,
  });
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
