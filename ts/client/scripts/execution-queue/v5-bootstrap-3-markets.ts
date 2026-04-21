/**
 * v5-bootstrap-3-markets.ts
 *
 * Full one-shot bootstrap for a v5-era mango group running three perp
 * markets (SOL, ETH, BTC) against real sponsored Pyth feeds.
 *
 * Every step is idempotent — re-running after a partial failure resumes
 * from the first unfinished step.
 *
 *   1. Ensure Group (create + USDC mint if missing)
 *   2. Ensure USDC bank + Pyth USDC/USD sponsored oracle
 *   3. Ensure v5 queue account (create → resize loop → init)
 *   4. For each perp market (SOL/ETH/BTC):
 *       - perpCreateMarket with Pyth-sponsored oracle + precision below
 *   5. For each market_index: configure v5 sub-queue
 *
 * ── Precision rationale ──────────────────────────────────────────────
 *
 * Target tick = min(5-sig-fig step at current price, $0.01).
 *
 * mango-v4 tick in quote UI per 1 base UI:
 *     tick = quoteLotSize * 10^(baseDecimals - quoteDecimals) / baseLotSize
 *
 * With quoteDecimals = 6 (USDC):
 *
 *   SOL @ ~$85  → 5 sig figs = $0.001. min(0.001, 0.01) = $0.001
 *                 baseDecimals=5, baseLotSize=100, quoteLotSize=1
 *                 → tick = 1 * 10^-1 / 100 = $0.001       ✓
 *                 → min size = 100 / 10^5 = 0.001 SOL (~$0.085)
 *
 *   ETH @ ~$3000 → 5 sig figs = $0.1. min(0.1, 0.01)    = $0.01
 *                 baseDecimals=6, baseLotSize=100, quoteLotSize=1
 *                 → tick = 1 * 10^0 / 100 = $0.01         ✓
 *                 → min size = 100 / 10^6 = 0.0001 ETH (~$0.30)
 *
 *   BTC @ ~$67k  → 5 sig figs = $1.    min(1, 0.01)     = $0.01
 *                 baseDecimals=6, baseLotSize=100, quoteLotSize=1
 *                 → tick = $0.01                          ✓
 *                 → min size = 0.0001 BTC (~$6.70)
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE       RPC url
 *   CTM_RELAYER_PROGRAM_ID     mango-v4 program id (devnet: 9rpAcg1…)
 *   MB_PAYER_KEYPAIR           admin + fee payer keypair path
 *   EXECUTION_QUEUE_GROUP_NUM  group num
 *
 * Optional env:
 *   V5_ENV_OUTPUT_PATH         if set, append V5_QUEUE / per-market pubkeys here
 *   V5_MARKET_INDEX_SOL        default 0
 *   V5_MARKET_INDEX_ETH        default 1
 *   V5_MARKET_INDEX_BTC        default 2
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  createInitializeAccount3Instruction,
  createMint,
  TOKEN_PROGRAM_ID,
} from '@solana/spl-token';
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

import { PerpMarketIndex } from '../../src/accounts/perp';
import { TokenIndex } from '../../src/accounts/bank';
import { MangoClient } from '../../src/client';

// ── V5 queue constants (must match state/execution_queue_v5.rs) ──────

const V5_N_MAX_MARKETS = 16;
const V5_PER_MARKET_CAPACITY = 256;
const V5_HEADER_SIZE = 144;
const V5_SUB_QUEUE_HEADER_STRIDE = 64;
const V5_ITEM_STRIDE = 96;
const V5_ACCOUNT_SPACE =
  8 +
  V5_HEADER_SIZE +
  V5_N_MAX_MARKETS * V5_SUB_QUEUE_HEADER_STRIDE +
  V5_N_MAX_MARKETS * V5_PER_MARKET_CAPACITY * V5_ITEM_STRIDE;
const MAX_PERMITTED_DATA_INCREASE = 10_240;

// ── Pyth sponsored PriceUpdateV2 feeds (shard 0, identical on mainnet/devnet) ──

const PYTH_SPONSORED = {
  USDC: new PublicKey('Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX'),
  SOL: new PublicKey('7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE'),
  BTC: new PublicKey('4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo'),
  ETH: new PublicKey('42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC'),
  ZEC: new PublicKey('HzdKMXqocYWqy7mh8AKDoZFJinjeGMfBKmGAxGbasc28'),
  FARTCOIN: new PublicKey('2t8eUbYKjidMs3uSeYM9jXM9uudYZwGkSeTB4TKjmvnC'),
};

const PYTH_ORACLE_CONFIG = {
  confFilter: 0.1,
  maxStalenessSlots: 600, // ~5 min on devnet
} as const;

// ── Market definitions (single source of truth for precision + oracle) ──

type MarketDef = {
  name: string;
  marketIndex: PerpMarketIndex;
  oraclePk: PublicKey;
  /// Used only to seed stub oracles if ever needed; Pyth feeds ignore this.
  initialPrice: number;
  baseDecimals: number;
  baseLotSize: number;
  quoteLotSize: number;
};

const MARKETS: MarketDef[] = [
  {
    name: 'SOL-PERP',
    marketIndex: Number(process.env.V5_MARKET_INDEX_SOL ?? '0') as PerpMarketIndex,
    oraclePk: PYTH_SPONSORED.SOL,
    initialPrice: 85,
    baseDecimals: 5,
    baseLotSize: 100,
    quoteLotSize: 1,
  },
  {
    name: 'ETH-PERP',
    marketIndex: Number(process.env.V5_MARKET_INDEX_ETH ?? '1') as PerpMarketIndex,
    oraclePk: PYTH_SPONSORED.ETH,
    initialPrice: 3_000,
    baseDecimals: 6,
    baseLotSize: 100,
    quoteLotSize: 1,
  },
  {
    name: 'BTC-PERP',
    marketIndex: Number(process.env.V5_MARKET_INDEX_BTC ?? '2') as PerpMarketIndex,
    oraclePk: PYTH_SPONSORED.BTC,
    initialPrice: 67_000,
    baseDecimals: 6,
    baseLotSize: 100,
    quoteLotSize: 1,
  },
  {
    // ZEC ~$50 → 5 sig figs = $0.001 tick. Same shape as SOL.
    name: 'ZEC-PERP',
    marketIndex: Number(process.env.V5_MARKET_INDEX_ZEC ?? '3') as PerpMarketIndex,
    oraclePk: PYTH_SPONSORED.ZEC,
    initialPrice: 50,
    baseDecimals: 5,
    baseLotSize: 100,
    quoteLotSize: 1,
  },
  {
    // FARTCOIN ~$1 → 5 sig figs = $0.0001. baseLotSize=10000 with
    // baseDecimals=6 gives min size 0.01 FARTCOIN (~$0.01) and tick $0.0001.
    name: 'FARTCOIN-PERP',
    marketIndex: Number(process.env.V5_MARKET_INDEX_FARTCOIN ?? '4') as PerpMarketIndex,
    oraclePk: PYTH_SPONSORED.FARTCOIN,
    initialPrice: 1,
    baseDecimals: 6,
    baseLotSize: 10000,
    quoteLotSize: 1,
  },
];

// ── Env ──────────────────────────────────────────────────────────────

const CLUSTER_URL = requireEnv('CLUSTER_URL_OVERRIDE');
const PROGRAM_ID = new PublicKey(requireEnv('CTM_RELAYER_PROGRAM_ID'));
const ADMIN_KEYPAIR = requireEnv('MB_PAYER_KEYPAIR');
const GROUP_NUM = Number(requireEnv('EXECUTION_QUEUE_GROUP_NUM'));
const ENV_OUTPUT_PATH = process.env.V5_ENV_OUTPUT_PATH || '';

const USDC_MINT_DECIMALS = 6;
const USDC_TOKEN_INDEX = 0 as TokenIndex;
const BANK_ACCOUNT_SPACE = 8 + 3072;
const MINT_INFO_ACCOUNT_SPACE = 8 + 3056;
const FULL_COLLATERAL_DEPOSIT_SCALE_START_QUOTE = Number.MAX_VALUE;

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
  return createHash('sha256').update(`global:${name}`).digest().subarray(0, 8);
}

function u16le(n: number): Buffer {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n, 0);
  return b;
}

function u8(n: number): Buffer {
  return Buffer.from([n & 0xff]);
}

function log(obj: Record<string, unknown>): void {
  console.log(JSON.stringify({ ts: new Date().toISOString(), ...obj }));
}

// ── PDA derivations ──────────────────────────────────────────────────

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

// ── V5 queue ix builders (raw; independent of IDL freshness) ─────────

function ixV5Create(
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

function ixV5Resize(
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

function ixV5Init(
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

function ixV5ConfigureMarket(
  group: PublicKey,
  authorityState: PublicKey,
  queue: PublicKey,
  admin: PublicKey,
  marketIndex: number,
  softLimit: number,
  gapWaitSlots: number,
): TransactionInstruction {
  const data = Buffer.concat([
    anchorDiscriminator('execution_queue_v5_configure_market'),
    u16le(marketIndex),
    u8(0), // shard_id
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

// ── V5 queue inspectors ──────────────────────────────────────────────

function isQueueInitialized(data: Buffer): boolean {
  if (data.length < 8 + V5_HEADER_SIZE) return false;
  // layout_version lives at group(32) + authority_state(32) + bump(1) +
  // paused_in(1) + paused_ex(1) → offset 67 within the header.
  const layoutVersion = data.readUInt8(8 + 32 + 32 + 3);
  return layoutVersion === 5;
}

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

// ── USDC bank bootstrap (copied from v2-multi-market-bootstrap for
//    self-containedness; uses the pyth USDC sponsored feed). ──────────

async function bootstrapTokenRegister(
  connection: Connection,
  adminClient: MangoClient,
  programId: PublicKey,
  group: PublicKey,
  admin: Keypair,
  mint: PublicKey,
  oracle: PublicKey,
  tokenIndex: TokenIndex,
  name: string,
): Promise<void> {
  const bank = Keypair.generate();
  const mintInfo = Keypair.generate();
  const vault = Keypair.generate();
  const bankLamports = await connection.getMinimumBalanceForRentExemption(
    BANK_ACCOUNT_SPACE,
  );
  const mintInfoLamports = await connection.getMinimumBalanceForRentExemption(
    MINT_INFO_ACCOUNT_SPACE,
  );
  const vaultLamports = await connection.getMinimumBalanceForRentExemption(165);

  await adminClient.sendAndConfirmTransaction(
    [
      SystemProgram.createAccount({
        fromPubkey: admin.publicKey,
        newAccountPubkey: bank.publicKey,
        lamports: bankLamports,
        space: BANK_ACCOUNT_SPACE,
        programId,
      }),
      SystemProgram.createAccount({
        fromPubkey: admin.publicKey,
        newAccountPubkey: mintInfo.publicKey,
        lamports: mintInfoLamports,
        space: MINT_INFO_ACCOUNT_SPACE,
        programId,
      }),
      SystemProgram.createAccount({
        fromPubkey: admin.publicKey,
        newAccountPubkey: vault.publicKey,
        lamports: vaultLamports,
        space: 165,
        programId: TOKEN_PROGRAM_ID,
      }),
      createInitializeAccount3Instruction(
        vault.publicKey,
        mint,
        group,
        TOKEN_PROGRAM_ID,
      ),
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: group, isSigner: false, isWritable: false },
          { pubkey: admin.publicKey, isSigner: true, isWritable: false },
          { pubkey: mint, isSigner: false, isWritable: false },
          { pubkey: bank.publicKey, isSigner: false, isWritable: true },
          { pubkey: vault.publicKey, isSigner: false, isWritable: true },
          { pubkey: mintInfo.publicKey, isSigner: false, isWritable: true },
          { pubkey: oracle, isSigner: false, isWritable: false },
          { pubkey: PublicKey.default, isSigner: false, isWritable: false },
        ],
        data: Buffer.concat([
          anchorDiscriminator('token_register_bootstrap'),
          Buffer.from(Uint8Array.of(tokenIndex & 0xff, (tokenIndex >> 8) & 0xff)),
          (() => {
            const nameBuf = Buffer.from(name, 'utf8');
            const lenBuf = Buffer.alloc(4);
            lenBuf.writeUInt32LE(nameBuf.length, 0);
            return Buffer.concat([lenBuf, nameBuf, Buffer.from([0])]);
          })(),
        ]),
      }),
    ],
    { additionalSigners: [bank, mintInfo, vault] },
  );
}

// ── Main ─────────────────────────────────────────────────────────────

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
  const adminClient = await MangoClient.connect(provider, 'devnet', PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });

  const groupPk = deriveGroup(admin.publicKey, GROUP_NUM);
  const authorityState = deriveQueueAuthority(groupPk);
  const [queuePk] = deriveQueueV5(groupPk);

  log({
    msg: 'bootstrap_start',
    group: groupPk.toBase58(),
    authority_state: authorityState.toBase58(),
    queue: queuePk.toBase58(),
    markets: MARKETS.map((m) => ({
      name: m.name,
      market_index: m.marketIndex,
      oracle: m.oraclePk.toBase58(),
      baseDecimals: m.baseDecimals,
      baseLotSize: m.baseLotSize,
      quoteLotSize: m.quoteLotSize,
    })),
  });

  // Pre-flight: admin balance.
  const adminBalance = await connection.getBalance(admin.publicKey, 'confirmed');
  const queueRent = await connection.getMinimumBalanceForRentExemption(
    V5_ACCOUNT_SPACE,
  );
  log({
    msg: 'admin_balance',
    sol: adminBalance / 1e9,
    queue_rent_sol: queueRent / 1e9,
  });
  // Queue (~2.74) + 3 perp markets (~1.5 total) + USDC bank (~0.03) + fees.
  const ESTIMATED_TOTAL_LAMPORTS = queueRent + 2_500_000_000;
  if (adminBalance < ESTIMATED_TOTAL_LAMPORTS) {
    throw new Error(
      `admin balance ${adminBalance / 1e9} SOL insufficient; estimated need ${
        ESTIMATED_TOTAL_LAMPORTS / 1e9
      } SOL (queue rent + markets + fees)`,
    );
  }

  // Pyth oracle presence check — refuse to create markets pointing at
  // feeds that don't exist on this cluster.
  for (const [name, pk] of Object.entries(PYTH_SPONSORED)) {
    const info = await connection.getAccountInfo(pk, 'confirmed');
    if (!info) {
      throw new Error(
        `Pyth sponsored feed ${name} (${pk.toBase58()}) missing on the cluster. ` +
          `Verify the Pyth pusher is live before bootstrapping markets.`,
      );
    }
  }
  log({ msg: 'pyth_feeds_verified' });

  // 1. Group + USDC mint.
  let usdcMint: PublicKey;
  const groupInfo = await connection.getAccountInfo(groupPk);
  if (!groupInfo) {
    log({ msg: 'group_create' });
    usdcMint = await createMint(
      connection,
      admin,
      admin.publicKey,
      null,
      USDC_MINT_DECIMALS,
    );
    await adminClient.groupCreate(GROUP_NUM, true, 0, usdcMint);
    log({
      msg: 'group_created',
      group: groupPk.toBase58(),
      usdc_mint: usdcMint.toBase58(),
    });
  } else {
    const existingGroup = await adminClient.getGroup(groupPk);
    const existingBank = existingGroup.banksMapByTokenIndex.get(USDC_TOKEN_INDEX)?.[0];
    if (existingBank) {
      usdcMint = existingBank.mint;
      log({ msg: 'group_exists_with_usdc', usdc_mint: usdcMint.toBase58() });
    } else if (process.env.USDC_MINT_OVERRIDE) {
      // Resuming a partial prior run: caller passed the mint the previous
      // run created (log line `group_created` / `group_existed_new_usdc_mint`
      // records it). Reusing avoids orphan mints and a second bank creation
      // that'd collide with the first once it lands.
      usdcMint = new PublicKey(process.env.USDC_MINT_OVERRIDE);
      log({ msg: 'group_existed_usdc_mint_from_env', usdc_mint: usdcMint.toBase58() });
    } else {
      // Group exists but no USDC bank yet and no override — mint fresh.
      usdcMint = await createMint(
        connection,
        admin,
        admin.publicKey,
        null,
        USDC_MINT_DECIMALS,
      );
      log({ msg: 'group_existed_new_usdc_mint', usdc_mint: usdcMint.toBase58() });
    }
  }
  let group = await adminClient.getGroup(groupPk);

  // 2. USDC bank + Pyth USDC oracle.
  if (!group.banksMapByTokenIndex.get(USDC_TOKEN_INDEX)?.length) {
    log({ msg: 'usdc_bank_bootstrap' });
    await bootstrapTokenRegister(
      connection,
      adminClient,
      PROGRAM_ID,
      groupPk,
      admin,
      usdcMint,
      PYTH_SPONSORED.USDC,
      USDC_TOKEN_INDEX,
      'USDC',
    );
    group = await adminClient.getGroup(groupPk);
  }

  // Make USDC fully collateralized (maint/init weights = 1, unlimited scale).
  const usdcBank = group.getFirstBankByMint(usdcMint);
  if (
    usdcBank.maintAssetWeight.toNumber() !== 1 ||
    usdcBank.initAssetWeight.toNumber() !== 1
  ) {
    log({ msg: 'usdc_bank_set_full_collateral' });
    await adminClient.tokenEdit(group, usdcMint, {
      maintAssetWeight: 1,
      initAssetWeight: 1,
      depositWeightScaleStartQuote: FULL_COLLATERAL_DEPOSIT_SCALE_START_QUOTE,
    } as any);
    group = await adminClient.getGroup(groupPk);
  }

  // 3. v5 queue: create → resize → init.
  //    Skipped when V5_SKIP_SHARED_QUEUE=true — the program now requires
  //    per-market PDA seeds (["execution-queue-v5", group, market_index]),
  //    so the shared-seed derivation here is rejected by program checks.
  //    Use v5-bootstrap-per-market.ts to create per-market queues after
  //    this script finishes group + bank + perp markets.
  if (process.env.V5_SKIP_SHARED_QUEUE === 'true') {
    log({ msg: 'queue_step_skipped_per_market_mode' });
  } else {
  let queueInfo = await connection.getAccountInfo(queuePk, 'confirmed');
  if (!queueInfo) {
    log({ msg: 'queue_create' });
    const tx = new Transaction().add(
      ixV5Create(groupPk, authorityState, queuePk, admin.publicKey, admin.publicKey),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
    });
    log({ msg: 'tx', label: 'queue_create', sig });
    queueInfo = await connection.getAccountInfo(queuePk, 'confirmed');
  }
  let loops = 0;
  while (queueInfo!.data.length < V5_ACCOUNT_SPACE) {
    const before = queueInfo!.data.length;
    const tx = new Transaction().add(
      ixV5Resize(groupPk, authorityState, queuePk, admin.publicKey),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
    });
    queueInfo = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!queueInfo || queueInfo.data.length <= before) {
      throw new Error(
        `queue resize did not grow: ${before} → ${queueInfo?.data.length ?? 'missing'}`,
      );
    }
    loops++;
    if (loops % 5 === 0 || queueInfo.data.length >= V5_ACCOUNT_SPACE) {
      log({
        msg: 'queue_resize',
        loops,
        data_len: queueInfo.data.length,
        target: V5_ACCOUNT_SPACE,
        pct: ((queueInfo.data.length / V5_ACCOUNT_SPACE) * 100).toFixed(1),
        last_sig: sig,
      });
    }
  }
  if (!isQueueInitialized(queueInfo!.data)) {
    log({ msg: 'queue_init' });
    const tx = new Transaction().add(
      ixV5Init(groupPk, authorityState, queuePk, admin.publicKey),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
    });
    log({ msg: 'tx', label: 'queue_init', sig });
    queueInfo = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!queueInfo || !isQueueInitialized(queueInfo.data)) {
      throw new Error('queue layout_version != 5 after init');
    }
  }
  } // end V5_SKIP_SHARED_QUEUE else

  // 4. Perp markets.
  for (const mkt of MARKETS) {
    const existing = group.perpMarketsMapByMarketIndex.get(mkt.marketIndex);
    if (existing) {
      log({
        msg: 'perp_market_exists',
        name: mkt.name,
        market_index: mkt.marketIndex,
        pubkey: existing.publicKey.toBase58(),
        oracle: existing.oracle.toBase58(),
      });
      continue;
    }
    log({
      msg: 'perp_create_market',
      name: mkt.name,
      market_index: mkt.marketIndex,
      oracle: mkt.oraclePk.toBase58(),
      tick_quote_per_base:
        (mkt.quoteLotSize * Math.pow(10, mkt.baseDecimals - USDC_MINT_DECIMALS)) /
        mkt.baseLotSize,
      min_base_size_ui: mkt.baseLotSize / Math.pow(10, mkt.baseDecimals),
    });
    await adminClient.perpCreateMarket(
      group,
      mkt.oraclePk,
      mkt.marketIndex,
      mkt.name,
      { ...PYTH_ORACLE_CONFIG },
      mkt.baseDecimals,
      mkt.quoteLotSize,
      mkt.baseLotSize,
      0.9,    // maint_base_asset_weight
      0.8,    // init_base_asset_weight
      1.1,    // maint_base_liab_weight
      1.2,    // init_base_liab_weight
      0.0,    // base_liquidation_fee
      0.0,    // maker_fee
      0.05,   // taker_fee
      -0.001, // min_funding
      0.002,  // max_funding
      0,      // impact_quantity
      -0.1,   // group_insurance_fund
      0.1,    // trusted_market
      10,     // settle_fee_flat
      false,  // reduce_only
      0, 0, 0, 0, // padding / future fields
      -1.0,   // negative_pnl_liquidation_fee
      2 * 60 * 60, // settle_pnl_limit_window_size_ts
      0.025,  // settle_pnl_limit_factor
      0.0,    // platform_liquidation_fee
    );
    group = await adminClient.getGroup(groupPk);
  }

  // 5. v5 sub-queues — one per market_index.
  //    Skipped in per-market mode; v5-bootstrap-per-market.ts both creates
  //    the queue PDA AND configures its sub-queue in a single pass.
  if (process.env.V5_SKIP_SHARED_QUEUE === 'true') {
    log({ msg: 'sub_queue_step_skipped_per_market_mode' });
  } else {
  let queueInfo = await connection.getAccountInfo(queuePk, 'confirmed');
  for (const mkt of MARKETS) {
    if (isSubQueueConfigured(queueInfo!.data, mkt.marketIndex)) {
      log({
        msg: 'sub_queue_exists',
        name: mkt.name,
        market_index: mkt.marketIndex,
      });
      continue;
    }
    log({
      msg: 'configure_sub_queue',
      name: mkt.name,
      market_index: mkt.marketIndex,
    });
    const tx = new Transaction().add(
      ixV5ConfigureMarket(
        groupPk,
        authorityState,
        queuePk,
        admin.publicKey,
        mkt.marketIndex,
        0, // soft_limit: 0 = full capacity
        0, // gap_wait_slots: 0 = default (4)
      ),
    );
    const sig = await sendAndConfirmTransaction(connection, tx, [admin], {
      commitment: 'confirmed',
    });
    log({ msg: 'tx', label: `configure_sub_queue_${mkt.name}`, sig });
    queueInfo = await connection.getAccountInfo(queuePk, 'confirmed');
    if (!queueInfo || !isSubQueueConfigured(queueInfo.data, mkt.marketIndex)) {
      throw new Error(
        `sub-queue for market_index=${mkt.marketIndex} (${mkt.name}) not visible after configure`,
      );
    }
  }
  } // end V5_SKIP_SHARED_QUEUE else (step 5)

  // Summary.
  const finalGroup = await adminClient.getGroup(groupPk);
  const summary = {
    msg: 'bootstrap_done',
    group: groupPk.toBase58(),
    authority_state: authorityState.toBase58(),
    queue: queuePk.toBase58(),
    usdc_mint: usdcMint.toBase58(),
    markets: MARKETS.map((m) => {
      const pm = finalGroup.perpMarketsMapByMarketIndex.get(m.marketIndex);
      return {
        name: m.name,
        market_index: m.marketIndex,
        perp_market: pm?.publicKey.toBase58(),
        perp_bids: pm?.bids.toBase58(),
        perp_asks: pm?.asks.toBase58(),
        perp_event_queue: pm?.eventQueue.toBase58(),
        oracle: m.oraclePk.toBase58(),
      };
    }),
  };
  log(summary);

  if (ENV_OUTPUT_PATH) {
    const lines: string[] = [
      '',
      `# --- generated by v5-bootstrap-3-markets.ts ---`,
      `V5_QUEUE=${queuePk.toBase58()}`,
      `V5_USDC_MINT=${usdcMint.toBase58()}`,
    ];
    for (const m of MARKETS) {
      const pm = finalGroup.perpMarketsMapByMarketIndex.get(m.marketIndex);
      if (!pm) continue;
      const sym = m.name.replace('-PERP', '');
      lines.push(`V5_${sym}_MARKET_INDEX=${m.marketIndex}`);
      lines.push(`V5_${sym}_PERP_MARKET=${pm.publicKey.toBase58()}`);
      lines.push(`V5_${sym}_PERP_BIDS=${pm.bids.toBase58()}`);
      lines.push(`V5_${sym}_PERP_ASKS=${pm.asks.toBase58()}`);
      lines.push(`V5_${sym}_PERP_EVENT_QUEUE=${pm.eventQueue.toBase58()}`);
      lines.push(`V5_${sym}_PERP_ORACLE=${m.oraclePk.toBase58()}`);
    }
    fs.appendFileSync(ENV_OUTPUT_PATH, lines.join('\n') + '\n');
    log({ msg: 'env_written', path: ENV_OUTPUT_PATH });
  }
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
