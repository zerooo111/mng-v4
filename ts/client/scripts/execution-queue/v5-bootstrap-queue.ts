/**
 * v5-bootstrap-queue.ts
 *
 * One-shot bootstrap for execution_queue_v5. Creates (or completes) the
 * single per-group queue account and configures the sub-queue for one
 * market_index. Idempotent: re-running after a partial failure resumes
 * the resize loop from the current data_len.
 *
 * Steps:
 *   1. Derive the queue PDA (seeds = ["execution-queue-v5", group]).
 *   2. If the account is missing, call `execution_queue_v5_create` (8 bytes).
 *   3. Loop `execution_queue_v5_resize` until data_len == full v5 size
 *      (~394 KB, ~39 calls at Solana's 10 KB realloc cap).
 *   4. Call `execution_queue_v5_init` to zero-copy the header layout.
 *   5. If the requested market_index has no sub-queue yet, call
 *      `execution_queue_v5_configure_market`.
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE       — RPC url
 *   CTM_RELAYER_PROGRAM_ID     — mango-v4 program id (devnet: 9rpAcg1...)
 *   MB_PAYER_KEYPAIR           — admin + fee payer keypair path
 *   EXECUTION_QUEUE_GROUP_NUM  — group num the queue belongs to
 *   V5_MARKET_INDEX            — market_index to configure a sub-queue for (e.g. 2)
 *
 * Optional env:
 *   V5_SOFT_LIMIT              — per-market soft cap; 0 = full 256
 *   V5_GAP_WAIT_SLOTS          — gap tolerance; 0 = default (4)
 *   V5_ENV_OUTPUT_PATH         — if set, append V5_QUEUE=<pubkey> to this file
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
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

const V5_N_MAX_MARKETS = 16;
const V5_PER_MARKET_CAPACITY = 256;
// Header: group(32) + authority_state(32) + bump(1) + paused_in(1) + paused_ex(1) +
//         layout_version(1) + _padding0(4) + total_count(8) + reserved(64) = 144
const V5_HEADER_SIZE = 144;
const V5_SUB_QUEUE_HEADER_STRIDE = 64;
const V5_ITEM_STRIDE = 96;
const V5_ACCOUNT_SPACE =
  8 /* anchor disc */ +
  V5_HEADER_SIZE +
  V5_N_MAX_MARKETS * V5_SUB_QUEUE_HEADER_STRIDE +
  V5_N_MAX_MARKETS * V5_PER_MARKET_CAPACITY * V5_ITEM_STRIDE;
// Solana's single-call realloc ceiling. Must match
// solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE (10_240).
const MAX_PERMITTED_DATA_INCREASE = 10_240;

// ── Env ────────────────────────────────────────────────────────────────

const CLUSTER_URL = requireEnv('CLUSTER_URL_OVERRIDE');
const PROGRAM_ID = new PublicKey(requireEnv('CTM_RELAYER_PROGRAM_ID'));
const ADMIN_KEYPAIR = requireEnv('MB_PAYER_KEYPAIR');
const GROUP_NUM = Number(requireEnv('EXECUTION_QUEUE_GROUP_NUM'));
const MARKET_INDEX = Number(requireEnv('V5_MARKET_INDEX'));
const SOFT_LIMIT = Number(process.env.V5_SOFT_LIMIT ?? '0');
const GAP_WAIT_SLOTS = Number(process.env.V5_GAP_WAIT_SLOTS ?? '0');
const ENV_OUTPUT_PATH = process.env.V5_ENV_OUTPUT_PATH || '';

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`missing required env: ${name}`);
  return v;
}

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

function deriveQueueV5(group: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('execution-queue-v5'), group.toBuffer()],
    PROGRAM_ID,
  );
}

// ── Instruction builders (match lib.rs dispatch + accounts_ix/execution_queue_v5.rs) ──

function ixCreate(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  payer: PublicKey,
  admin: PublicKey,
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
    data: anchorDiscriminator('execution_queue_v5_create'),
  });
}

function ixResize(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  payer: PublicKey,
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
    data: anchorDiscriminator('execution_queue_v5_resize'),
  });
}

function ixInit(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  admin: PublicKey,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: group, isSigner: false, isWritable: false },
      { pubkey: authorityState, isSigner: false, isWritable: false },
      { pubkey: queue, isSigner: false, isWritable: true },
      { pubkey: admin, isSigner: true, isWritable: false },
    ],
    data: anchorDiscriminator('execution_queue_v5_init'),
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
  // Args: ExecutionQueueV5ConfigureMarketParams { market_index: u16, shard_id: u8,
  //        soft_limit: u16, gap_wait_slots: u16 }
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

// ── Sub-queue inspector (raw bytes) ────────────────────────────────────

function isSubQueueConfigured(data: Buffer, marketIndex: number): boolean {
  const subHeadersStart = 8 + V5_HEADER_SIZE;
  for (let i = 0; i < V5_N_MAX_MARKETS; i++) {
    const base = subHeadersStart + i * V5_SUB_QUEUE_HEADER_STRIDE;
    const mi = data.readUInt16LE(base + 0);
    const active = data.readUInt8(base + 2);
    if (active === 1 && mi === marketIndex) return true;
  }
  return false;
}

function isQueueInitialized(data: Buffer): boolean {
  if (data.length < 8 + V5_HEADER_SIZE) return false;
  // layout_version lives at header offset 3 (bump, paused_in, paused_ex, layout_version)
  // within the ExecutionQueueV5Header. After the 8-byte anchor disc that's
  // 8 + 32 + 32 + 1 + 1 + 1 = 75 for the layout_version byte.
  const layoutVersion = data.readUInt8(8 + 32 + 32 + 3);
  return layoutVersion === 5;
}

// ── Main ───────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, {
    commitment: 'confirmed',
  });
  const provider = new AnchorProvider(
    connection,
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );

  const groupPk = deriveGroup(admin.publicKey, GROUP_NUM);
  const authorityState = deriveQueueAuthority(groupPk);
  const [queuePk] = deriveQueueV5(groupPk);

  log({
    msg: 'v5_bootstrap_start',
    group: groupPk.toBase58(),
    authority_state: authorityState.toBase58(),
    queue: queuePk.toBase58(),
    market_index: MARKET_INDEX,
    target_len: V5_ACCOUNT_SPACE,
  });

  const adminBalance = await connection.getBalance(admin.publicKey, 'confirmed');
  const rent = await connection.getMinimumBalanceForRentExemption(V5_ACCOUNT_SPACE);
  log({
    msg: 'admin_balance',
    lamports: adminBalance,
    required_rent_lamports: rent,
    approx_sol: adminBalance / 1e9,
    required_sol: rent / 1e9,
  });
  if (adminBalance < rent + 10_000_000 /* ~0.01 SOL fee buffer */) {
    throw new Error(
      `admin balance ${adminBalance / 1e9} SOL insufficient for queue rent ${
        rent / 1e9
      } SOL + fees`,
    );
  }

  // 1. Create if missing
  let info = await connection.getAccountInfo(queuePk, 'confirmed');
  if (!info) {
    log({ msg: 'queue_create' });
    await sendIx(
      connection,
      admin,
      ixCreate(groupPk, authorityState, queuePk, admin.publicKey, admin.publicKey),
      'create',
    );
    info = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!info) throw new Error('queue still missing after create');
  } else {
    log({ msg: 'queue_exists', data_len: info.data.length });
  }

  // 2. Resize loop — each call grows by ≤ MAX_PERMITTED_DATA_INCREASE.
  let loops = 0;
  while (info!.data.length < V5_ACCOUNT_SPACE) {
    const before = info!.data.length;
    const predicted = Math.min(before + MAX_PERMITTED_DATA_INCREASE, V5_ACCOUNT_SPACE);
    await sendIx(
      connection,
      admin,
      ixResize(groupPk, authorityState, queuePk, admin.publicKey),
      `resize ${before}→${predicted}`,
    );
    info = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!info) throw new Error('queue disappeared mid-resize');
    if (info.data.length <= before) {
      throw new Error(
        `resize did not grow: ${before} → ${info.data.length} (target ${V5_ACCOUNT_SPACE})`,
      );
    }
    loops++;
    if (loops % 5 === 0 || info.data.length >= V5_ACCOUNT_SPACE) {
      log({
        msg: 'resize_progress',
        loops,
        data_len: info.data.length,
        target: V5_ACCOUNT_SPACE,
        pct: ((info.data.length / V5_ACCOUNT_SPACE) * 100).toFixed(1),
      });
    }
  }
  log({ msg: 'resize_complete', loops, data_len: info!.data.length });

  // 3. Init (zero-copy the header + sub-queues + items)
  if (!isQueueInitialized(info!.data)) {
    log({ msg: 'queue_init' });
    await sendIx(
      connection,
      admin,
      ixInit(groupPk, authorityState, queuePk, admin.publicKey),
      'init',
    );
    info = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!info || !isQueueInitialized(info.data)) {
      throw new Error('queue layout_version != 5 after init');
    }
  } else {
    log({ msg: 'queue_already_initialized' });
  }

  // 4. Configure sub-queue for market_index
  if (isSubQueueConfigured(info!.data, MARKET_INDEX)) {
    log({ msg: 'sub_queue_already_configured', market_index: MARKET_INDEX });
  } else {
    log({
      msg: 'configure_market',
      market_index: MARKET_INDEX,
      soft_limit: SOFT_LIMIT,
      gap_wait_slots: GAP_WAIT_SLOTS,
    });
    await sendIx(
      connection,
      admin,
      ixConfigureMarket(
        groupPk,
        authorityState,
        queuePk,
        admin.publicKey,
        MARKET_INDEX,
        0,
        SOFT_LIMIT,
        GAP_WAIT_SLOTS,
      ),
      'configure_market',
    );
    info = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!info || !isSubQueueConfigured(info.data, MARKET_INDEX)) {
      throw new Error(
        `sub-queue for market_index=${MARKET_INDEX} not visible after configure_market`,
      );
    }
  }

  log({
    msg: 'v5_bootstrap_done',
    queue: queuePk.toBase58(),
    market_index: MARKET_INDEX,
  });

  if (ENV_OUTPUT_PATH) {
    const line = `V5_QUEUE=${queuePk.toBase58()}\nV5_MARKET_INDEX=${MARKET_INDEX}\n`;
    fs.appendFileSync(ENV_OUTPUT_PATH, line);
    log({ msg: 'env_written', path: ENV_OUTPUT_PATH });
  } else {
    console.log(`# append to devnet-stack.env:`);
    console.log(`V5_QUEUE=${queuePk.toBase58()}`);
    console.log(`V5_MARKET_INDEX=${MARKET_INDEX}`);
  }
}

async function sendIx(
  connection: Connection,
  payer: Keypair,
  ix: TransactionInstruction,
  label: string,
): Promise<string> {
  const tx = new Transaction().add(ix);
  const sig = await sendAndConfirmTransaction(connection, tx, [payer], {
    commitment: 'confirmed',
    skipPreflight: false,
  });
  log({ msg: 'tx', label, sig });
  return sig;
}

function log(obj: Record<string, unknown>): void {
  console.log(JSON.stringify({ ts: new Date().toISOString(), ...obj }));
}

main().catch((err) => {
  console.error(
    JSON.stringify({
      ts: new Date().toISOString(),
      level: 'error',
      error: String(err?.stack ?? err),
    }),
  );
  process.exit(1);
});
