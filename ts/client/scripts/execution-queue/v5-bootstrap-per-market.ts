/**
 * v5-bootstrap-per-market.ts
 *
 * Bootstrap one execution_queue_v5 PDA PER MARKET. Supersedes
 * v5-bootstrap-queue.ts (which created a single shared queue). Each market
 * gets an independent PDA — seeds now include market_index — so Solana's
 * per-account lock no longer serializes reveals across markets.
 *
 * Account sizing (updated):
 *   N_MAX_MARKETS = 1        (one sub-queue per PDA)
 *   PER_MARKET_CAPACITY = 1024  (4× prior)
 *   ≈ 99 KB per PDA (fits in ~10 resize calls)
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE       — RPC URL
 *   CTM_RELAYER_PROGRAM_ID     — mango-v4 program id
 *   MB_PAYER_KEYPAIR           — admin + fee payer
 *   EXECUTION_QUEUE_GROUP_NUM  — group num
 *   V5_MARKETS                 — CSV of market_index values, e.g. "2,3,4"
 *
 * Optional:
 *   V5_SOFT_LIMIT              — per-market soft cap; 0 = full 1024
 *   V5_GAP_WAIT_SLOTS          — gap tolerance; 0 = default (4)
 *
 * Idempotent: re-running after partial progress resumes from the current
 * account data_len / sub-queue-configured state for each market.
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

// ── Constants (must match programs/mango-v4/src/state/execution_queue_v5.rs) ──

const V5_N_MAX_MARKETS = 1;
const V5_PER_MARKET_CAPACITY = 1024;
const V5_HEADER_SIZE = 144;
const V5_SUB_QUEUE_HEADER_STRIDE = 64;
const V5_ITEM_STRIDE = 96;
const V5_ACCOUNT_SPACE =
  8 +
  V5_HEADER_SIZE +
  V5_N_MAX_MARKETS * V5_SUB_QUEUE_HEADER_STRIDE +
  V5_N_MAX_MARKETS * V5_PER_MARKET_CAPACITY * V5_ITEM_STRIDE;
const MAX_PERMITTED_DATA_INCREASE = 10_240;

// ── Env ────────────────────────────────────────────────────────────────

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
const SOFT_LIMIT = Number(process.env.V5_SOFT_LIMIT ?? '0');
const GAP_WAIT_SLOTS = Number(process.env.V5_GAP_WAIT_SLOTS ?? '0');

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

function anchorDiscriminator(name: string): Buffer {
  return createHash('sha256')
    .update(`global:${name}`)
    .digest()
    .subarray(0, 8);
}

function u16le(n: number): Buffer {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n, 0);
  return b;
}

function u8(n: number): Buffer {
  return Buffer.from([n & 0xff]);
}

function log(o: Record<string, unknown>): void {
  console.log(JSON.stringify({ ts: new Date().toISOString(), ...o }));
}

// ── PDA derivations ────────────────────────────────────────────────────

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

/**
 * Per-market queue PDA. The `market_index.to_le_bytes()` seed is what
 * gives each market its own account — and thus its own Solana write lock.
 */
function deriveQueueV5(
  group: PublicKey,
  marketIndex: number,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('execution-queue-v5'), group.toBuffer(), u16le(marketIndex)],
    PROGRAM_ID,
  );
}

// ── Instruction builders ───────────────────────────────────────────────
//
// All three lifecycle ixs now take `market_index: u16` after the ctx so
// the PDA seeds resolve. Anchor serializes args in declaration order
// after the discriminator.

function ixCreate(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  payer: PublicKey,
  admin: PublicKey,
  marketIndex: number,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: false },
      { pubkey: authorityState, isSigner: false, isWritable: false },
      { pubkey: queue, isSigner: false, isWritable: true },
      { pubkey: payer, isSigner: true, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: Buffer.concat([
      anchorDiscriminator('execution_queue_v5_create'),
      u16le(marketIndex),
    ]),
  });
}

function ixResize(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  payer: PublicKey,
  marketIndex: number,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: false },
      { pubkey: authorityState, isSigner: false, isWritable: false },
      { pubkey: queue, isSigner: false, isWritable: true },
      { pubkey: payer, isSigner: true, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: Buffer.concat([
      anchorDiscriminator('execution_queue_v5_resize'),
      u16le(marketIndex),
    ]),
  });
}

function ixInit(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  admin: PublicKey,
  marketIndex: number,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: false },
      { pubkey: authorityState, isSigner: false, isWritable: false },
      { pubkey: queue, isSigner: false, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
    ],
    data: Buffer.concat([
      anchorDiscriminator('execution_queue_v5_init'),
      u16le(marketIndex),
    ]),
  });
}

function ixConfigureMarket(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  admin: PublicKey,
  marketIndex: number,
  shardId: number,
  softLimit: number,
  gapWaitSlots: number,
): TransactionInstruction {
  const data = Buffer.concat([
    anchorDiscriminator('execution_queue_v5_configure_market'),
    u16le(marketIndex),
    u8(shardId),
    u16le(softLimit),
    u16le(gapWaitSlots),
  ]);
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: false },
      { pubkey: authorityState, isSigner: false, isWritable: false },
      { pubkey: queue, isSigner: false, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
    ],
    data,
  });
}

// ── Inspectors ─────────────────────────────────────────────────────────

function isQueueInitialized(data: Buffer): boolean {
  if (data.length < 8 + V5_HEADER_SIZE) return false;
  const layoutVersion = data.readUInt8(8 + 32 + 32 + 3);
  return layoutVersion === 5 || layoutVersion === 6;
}

function isSubQueueConfigured(data: Buffer, marketIndex: number): boolean {
  if (data.length < 8 + V5_HEADER_SIZE + V5_SUB_QUEUE_HEADER_STRIDE)
    return false;
  const subHeadersStart = 8 + V5_HEADER_SIZE;
  for (let i = 0; i < V5_N_MAX_MARKETS; i++) {
    const base = subHeadersStart + i * V5_SUB_QUEUE_HEADER_STRIDE;
    const mi = data.readUInt16LE(base + 0);
    const active = data.readUInt8(base + 2);
    if (active === 1 && mi === marketIndex) return true;
  }
  return false;
}

// ── Per-market bootstrap step ─────────────────────────────────────────

async function bootstrapMarket(
  connection: Connection,
  admin: Keypair,
  group: PublicKey,
  authorityState: PublicKey,
  marketIndex: number,
): Promise<PublicKey> {
  const [queue] = deriveQueueV5(group, marketIndex);
  log({ market: marketIndex, queue: queue.toBase58(), step: 'start' });

  // 1. Create if absent.
  const acct0 = await connection.getAccountInfo(queue, 'confirmed');
  if (acct0 === null) {
    const tx = new Transaction().add(
      ixCreate(group, authorityState, queue, admin.publicKey, admin.publicKey, marketIndex),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
      skipPreflight: false,
    });
    log({ market: marketIndex, step: 'created', sig });
  } else {
    log({
      market: marketIndex,
      step: 'create_skip',
      current_len: acct0.data.length,
    });
  }

  // 2. Resize loop until target.
  let currentLen =
    (await connection.getAccountInfo(queue, 'confirmed'))?.data.length ?? 0;
  while (currentLen < V5_ACCOUNT_SPACE) {
    const nextLen = Math.min(
      currentLen + MAX_PERMITTED_DATA_INCREASE,
      V5_ACCOUNT_SPACE,
    );
    const tx = new Transaction().add(
      ixResize(group, authorityState, queue, admin.publicKey, marketIndex),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
      skipPreflight: false,
    });
    log({
      market: marketIndex,
      step: 'resized',
      from: currentLen,
      to: nextLen,
      sig,
    });
    currentLen = nextLen;
  }

  // 3. Init if not yet initialized.
  const acct1 = await connection.getAccountInfo(queue, 'confirmed');
  if (acct1 === null) throw new Error('queue vanished mid-bootstrap');
  if (!isQueueInitialized(acct1.data)) {
    const tx = new Transaction().add(
      ixInit(group, authorityState, queue, admin.publicKey, marketIndex),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
      skipPreflight: false,
    });
    log({ market: marketIndex, step: 'inited', sig });
  } else {
    log({ market: marketIndex, step: 'init_skip' });
  }

  // 4. Configure market if sub-queue[0] not bound yet.
  const acct2 = await connection.getAccountInfo(queue, 'confirmed');
  if (acct2 === null) throw new Error('queue vanished post-init');
  if (!isSubQueueConfigured(acct2.data, marketIndex)) {
    const tx = new Transaction().add(
      ixConfigureMarket(
        group,
        authorityState,
        queue,
        admin.publicKey,
        marketIndex,
        /* shard_id */ 0,
        SOFT_LIMIT,
        GAP_WAIT_SLOTS,
      ),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
      skipPreflight: false,
    });
    log({ market: marketIndex, step: 'configured', sig });
  } else {
    log({ market: marketIndex, step: 'configure_skip' });
  }

  return queue;
}

// ── Main ───────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, { commitment: 'confirmed' });
  const group = deriveGroup(admin.publicKey, GROUP_NUM);
  const authorityState = deriveQueueAuthority(group);

  log({
    msg: 'bootstrap_start',
    group: group.toBase58(),
    authority_state: authorityState.toBase58(),
    markets: MARKET_INDICES,
    target_len: V5_ACCOUNT_SPACE,
    n_max_markets: V5_N_MAX_MARKETS,
    per_market_capacity: V5_PER_MARKET_CAPACITY,
  });

  const adminBalance = await connection.getBalance(
    admin.publicKey,
    'confirmed',
  );
  const perQueueRent = await connection.getMinimumBalanceForRentExemption(
    V5_ACCOUNT_SPACE,
  );
  const totalRent = perQueueRent * MARKET_INDICES.length;
  log({
    msg: 'admin_balance',
    lamports: adminBalance,
    required_rent_lamports: totalRent,
    required_sol: totalRent / 1e9,
    sol: adminBalance / 1e9,
  });
  if (adminBalance < totalRent + 50_000_000) {
    throw new Error(
      `admin balance ${adminBalance / 1e9} SOL insufficient for ${
        MARKET_INDICES.length
      } × ${perQueueRent / 1e9} SOL rent`,
    );
  }

  const queues: Record<number, string> = {};
  for (const mi of MARKET_INDICES) {
    const pk = await bootstrapMarket(
      connection,
      admin,
      group,
      authorityState,
      mi,
    );
    queues[mi] = pk.toBase58();
  }

  log({ msg: 'bootstrap_done', queues });
  console.log('\n# Derived queue PDAs (env-ready):');
  for (const [mi, pk] of Object.entries(queues)) {
    console.log(`V5_M${mi}_QUEUE=${pk}  # market ${mi}`);
  }
  console.log(
    '\n# The relayer derives these itself from (program_id, group, market_index) — no env needed.',
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
