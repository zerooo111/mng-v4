/**
 * v2-multi-market-bootstrap.ts
 *
 * Minimal bootstrap for testing the v2 sub-queue with multiple markets.
 * Creates: group + USDC bank + 2 perp markets (SOL-PERP, BTC-PERP) +
 * execution queue + maker/taker mango accounts + lanes config + run config.
 *
 * Skips editMangoAccount (broken in test-validator due to ws confirmation
 * timeout) and skips harness wiring (not needed for the multi-market test).
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE       — RPC url
 *   CTM_RELAYER_PROGRAM_ID     — mango program id
 *   MB_PAYER_KEYPAIR           — admin/deployer keypair path
 *   EXECUTION_QUEUE_GROUP_NUM  — group num
 *   E2E_LANE_CONFIG_PATH       — output lanes json
 *   E2E_OUTPUT_CONFIG_PATH     — output run config json
 *   MAKER_KEYPAIR              — maker keypair path
 *   TAKER_KEYPAIR              — taker keypair path
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  createAssociatedTokenAccountIdempotent,
  createInitializeAccount3Instruction,
  createMint,
  mintTo,
  TOKEN_PROGRAM_ID,
} from '@solana/spl-token';
import {
  AccountMeta,
  Cluster,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import fs from 'fs';
import path from 'path';
import { PerpMarketIndex } from '../../src/accounts/perp';
import { TokenIndex } from '../../src/accounts/bank';
import { MangoClient } from '../../src/client';
import { NullTokenEditParams } from '../../src/clientIxParamBuilder';
import { EXECUTION_QUEUE_ACCOUNT_SPACE } from '../../src/executionQueueLayout';

const CLUSTER: Cluster = 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE!;
const PROGRAM_ID = new PublicKey(process.env.CTM_RELAYER_PROGRAM_ID!);
const ADMIN_KEYPAIR = process.env.MB_PAYER_KEYPAIR!;
const GROUP_NUM = Number(process.env.EXECUTION_QUEUE_GROUP_NUM || '99');
const MAKER_KEYPAIR_PATH = process.env.MAKER_KEYPAIR!;
const TAKER_KEYPAIR_PATH = process.env.TAKER_KEYPAIR!;
const LANE_CONFIG_PATH = process.env.E2E_LANE_CONFIG_PATH!;
const OUTPUT_CONFIG_PATH = process.env.E2E_OUTPUT_CONFIG_PATH!;

const USDC_MINT_DECIMALS = 6;
const ASSET_MINT_DECIMALS = 9;
const BANK_ACCOUNT_SPACE = 8 + 3072;
const MINT_INFO_ACCOUNT_SPACE = 8 + 3056;
const USDC_TOKEN_INDEX = 0 as TokenIndex;
type PerpMarketPrecision = {
  baseDecimals: number;
  baseLotSize: number;
  quoteLotSize: number;
};

const DEFAULT_PERP_MARKET_PRECISION: Record<number, PerpMarketPrecision> = {
  // SOL-PERP: 0.001 USDC ticks at the current contract size scale.
  0: {
    baseDecimals: 5,
    baseLotSize: 100,
    quoteLotSize: 1,
  },
  // BTC-PERP: 0.01 USDC ticks.
  1: {
    baseDecimals: 6,
    baseLotSize: 100,
    quoteLotSize: 1,
  },
};
const FULL_COLLATERAL_DEPOSIT_SCALE_START_QUOTE = Number.MAX_VALUE;

// Oracle wiring. `stub` (default) keeps the existing stub-oracle bootstrap flow;
// `pyth` uses the Pyth Data Association sponsored PriceUpdateV2 feed accounts
// (shard 0) which the Pyth pusher keeps fresh on mainnet and devnet.
//
// Program-side decoder lives in programs/mango-v4/src/state/oracle.rs
// (detection: owner == pyth_solana_receiver_program::ID + PriceUpdateV2 disc).
type OracleMode = 'stub' | 'pyth';
const ORACLE_MODE: OracleMode =
  (process.env.ORACLE_MODE as OracleMode) || 'pyth';
if (ORACLE_MODE !== 'stub' && ORACLE_MODE !== 'pyth') {
  throw new Error(`invalid ORACLE_MODE=${ORACLE_MODE} (expected 'stub' or 'pyth')`);
}

// Sponsored PriceUpdateV2 feed accounts (shard 0). Identical on mainnet/devnet.
const PYTH_SPONSORED_FEED = {
  USDC: new PublicKey('Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX'),
  SOL:  new PublicKey('7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE'),
  BTC:  new PublicKey('4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo'),
  ETH:  new PublicKey('42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC'),
};

// OracleConfigParams consumed by both tokenRegister and perpCreateMarket when
// backed by a sponsored Pyth feed. The pusher refreshes shard-0 accounts every
// few slots; 600 slots (~5 min on devnet) leaves room for transient lag.
const PYTH_ORACLE_CONFIG = {
  confFilter: 0.1,
  maxStalenessSlots: 600,
} as const;

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))));
}

function anchorDiscriminator(name: string): Buffer {
  return createHash('sha256').update(`global:${name}`).digest().subarray(0, 8);
}

function queueNeedsInit(data: Buffer): boolean {
  return data.length < 8 || data.subarray(0, 8).every((b) => b === 0);
}

async function ensureExecutionQueue(
  connection: Connection,
  send: (ixs: TransactionInstruction[]) => Promise<unknown>,
  programId: PublicKey,
  group: PublicKey,
  executionQueue: PublicKey,
  admin: PublicKey,
  ctmSigner: PublicKey,
): Promise<void> {
  let info = await connection.getAccountInfo(executionQueue);
  if (!info) {
    await send([
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: group, isSigner: false, isWritable: true },
          { pubkey: executionQueue, isSigner: false, isWritable: true },
          { pubkey: admin, isSigner: true, isWritable: true },
          { pubkey: admin, isSigner: true, isWritable: false },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: anchorDiscriminator('execution_queue_create'),
      }),
    ]);
    info = await connection.getAccountInfo(executionQueue);
  }
  if (!info) throw new Error('queue create failed');

  while (info.data.length < EXECUTION_QUEUE_ACCOUNT_SPACE) {
    await send([
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: group, isSigner: false, isWritable: true },
          { pubkey: executionQueue, isSigner: false, isWritable: true },
          { pubkey: admin, isSigner: true, isWritable: true },
          { pubkey: admin, isSigner: true, isWritable: false },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: anchorDiscriminator('execution_queue_resize'),
      }),
    ]);
    info = await connection.getAccountInfo(executionQueue);
    if (!info) throw new Error('queue resize lost account');
  }

  if (queueNeedsInit(info.data)) {
    await send([
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: group, isSigner: false, isWritable: true },
          { pubkey: executionQueue, isSigner: false, isWritable: true },
          { pubkey: admin, isSigner: true, isWritable: false },
        ],
        data: Buffer.concat([
          anchorDiscriminator('execution_queue_init'),
          ctmSigner.toBuffer(),
        ]),
      }),
    ]);
  }
}

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
  const bankLamports = await connection.getMinimumBalanceForRentExemption(BANK_ACCOUNT_SPACE);
  const mintInfoLamports = await connection.getMinimumBalanceForRentExemption(MINT_INFO_ACCOUNT_SPACE);
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
      createInitializeAccount3Instruction(vault.publicKey, mint, group, TOKEN_PROGRAM_ID),
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

async function resolveMarketOracle(
  adminClient: MangoClient,
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  oracleMint: PublicKey,
  stubInitialPrice: number,
  pythSponsored: PublicKey,
): Promise<{ oracle: PublicKey; config: { confFilter: number; maxStalenessSlots: number | null } }> {
  if (ORACLE_MODE === 'pyth') {
    return {
      oracle: pythSponsored,
      config: { ...PYTH_ORACLE_CONFIG },
    };
  }
  const existing = await adminClient.getStubOracle(group, oracleMint);
  let stub = existing[0];
  if (!stub) {
    await adminClient.stubOracleCreate(group, oracleMint, stubInitialPrice);
    stub = (await adminClient.getStubOracle(group, oracleMint))[0];
  }
  return {
    oracle: stub.publicKey,
    config: { confFilter: 0.1, maxStalenessSlots: null },
  };
}

async function createPerpMarket(
  adminClient: MangoClient,
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  oracleMint: PublicKey,
  marketIndex: PerpMarketIndex,
  name: string,
  initialPrice: number,
  pythSponsored: PublicKey,
): Promise<void> {
  const precision =
    DEFAULT_PERP_MARKET_PRECISION[Number(marketIndex)] ?? {
      baseDecimals: 6,
      baseLotSize: 100,
      quoteLotSize: 1,
    };
  const { oracle, config } = await resolveMarketOracle(
    adminClient,
    group,
    oracleMint,
    initialPrice,
    pythSponsored,
  );

  await adminClient.perpCreateMarket(
    group,
    oracle,
    marketIndex,
    name,
    config,
    precision.baseDecimals,
    precision.quoteLotSize,
    precision.baseLotSize,
    0.9,      // maint_base_asset_weight
    0.8,      // init_base_asset_weight
    1.1,      // maint_base_liab_weight
    1.2,      // init_base_liab_weight
    0.0,      // base_liquidation_fee
    0.0,      // maker_fee
    0.05,     // taker_fee
    -0.001,   // min_funding
    0.002,    // max_funding
    0,        // impact_quantity
    -0.1,     // group_insurance_fund
    0.1,      // trusted_market
    10,       // settle_fee_flat
    false,    // reduce_only
    0,        // reset_last_funding_slot? (varies)
    0,        //
    0,        //
    0,        //
    -1.0,     // negative_pnl_liquidation_fee
    2 * 60 * 60, // settle_pnl_limit_window_size_ts
    0.025,    // settle_pnl_limit_factor
    0.0,      // platform_liquidation_fee
  );
}

async function ensureFullCollateralQuoteBank(
  adminClient: MangoClient,
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  quoteMint: PublicKey,
): Promise<boolean> {
  const quoteBank = group.getFirstBankByMint(quoteMint);
  const needsUpdate =
    quoteBank.maintAssetWeight.toNumber() !== 1 ||
    quoteBank.initAssetWeight.toNumber() !== 1 ||
    quoteBank.depositWeightScaleStartQuote !==
      FULL_COLLATERAL_DEPOSIT_SCALE_START_QUOTE;
  if (!needsUpdate) {
    return false;
  }

  await adminClient.tokenEdit(group, quoteMint, {
    ...NullTokenEditParams,
    maintAssetWeight: 1,
    initAssetWeight: 1,
    depositWeightScaleStartQuote: FULL_COLLATERAL_DEPOSIT_SCALE_START_QUOTE,
  });
  return true;
}

async function getOrCreateMangoAccount(
  ownerClient: MangoClient,
  payerClient: MangoClient,
  groupPk: PublicKey,
  owner: Keypair,
  accountNum: number,
  name: string,
) {
  const group = await ownerClient.getGroup(groupPk);
  let acct = await ownerClient.getMangoAccountForOwner(group, owner.publicKey, accountNum);
  if (!acct) {
    const ix = await payerClient.program.methods
      .accountCreate(accountNum, 8, 4, 4, 32, name)
      .accounts({
        group: groupPk,
        owner: owner.publicKey,
        payer: payerClient.walletPk,
      })
      .instruction();
    await payerClient.sendAndConfirmTransactionForGroup(group, [ix], {
      additionalSigners: [owner],
    });
    acct = await ownerClient.getMangoAccountForOwner(group, owner.publicKey, accountNum);
  }
  if (!acct) throw new Error(`failed to create mango account for ${owner.publicKey.toBase58()}`);
  return acct;
}

async function canonicalRemainingAccounts(
  client: MangoClient,
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>,
  marketIndex: PerpMarketIndex,
  userOwner: PublicKey,
): Promise<AccountMeta[]> {
  const perpMarket = group.getPerpMarketByMarketIndex(marketIndex);
  const healthRemainingAccounts = await client.buildHealthRemainingAccounts(
    group,
    [mangoAccount],
    [group.getFirstBankForPerpSettlement()],
    [perpMarket],
  );
  return [
    { pubkey: group.publicKey, isSigner: false, isWritable: false },
    { pubkey: mangoAccount.publicKey, isSigner: false, isWritable: true },
    { pubkey: userOwner, isSigner: false, isWritable: false },
    { pubkey: perpMarket.publicKey, isSigner: false, isWritable: true },
    { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
    { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
    { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
    { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
    ...healthRemainingAccounts.map((pubkey) => ({ pubkey, isSigner: false, isWritable: false })),
  ];
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const maker = readKeypair(MAKER_KEYPAIR_PATH);
  const taker = readKeypair(TAKER_KEYPAIR_PATH);

  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const adminClient = await MangoClient.connect(provider, CLUSTER, PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });
  console.log(JSON.stringify({ msg: 'oracle_mode', mode: ORACLE_MODE }));
  if (ORACLE_MODE === 'pyth') {
    for (const [name, pk] of Object.entries(PYTH_SPONSORED_FEED)) {
      const info = await provider.connection.getAccountInfo(pk, 'confirmed');
      if (!info) {
        throw new Error(
          `Pyth sponsored feed ${name} (${pk.toBase58()}) missing on ${CLUSTER}. ` +
            `Verify the pusher is live before bootstrapping markets.`,
        );
      }
    }
  }

  // Group PDA
  const groupNumBuf = Buffer.alloc(4);
  groupNumBuf.writeUInt32LE(GROUP_NUM);
  const [groupPk] = PublicKey.findProgramAddressSync(
    [Buffer.from('Group'), admin.publicKey.toBuffer(), groupNumBuf],
    PROGRAM_ID,
  );

  // Group create + USDC mint
  const groupInfo = await provider.connection.getAccountInfo(groupPk);
  let usdcMint: PublicKey;
  if (!groupInfo) {
    usdcMint = await createMint(provider.connection, admin, admin.publicKey, null, USDC_MINT_DECIMALS);
    await adminClient.groupCreate(GROUP_NUM, true, 0, usdcMint);
    console.log(JSON.stringify({ msg: 'group_created', group: groupPk.toBase58(), usdcMint: usdcMint.toBase58() }));
  } else {
    const existingGroup = await adminClient.getGroup(groupPk);
    const existingUsdcBank = existingGroup.banksMapByTokenIndex.get(USDC_TOKEN_INDEX)?.[0];
    if (existingUsdcBank) {
      usdcMint = existingUsdcBank.mint;
    } else {
      usdcMint = await createMint(provider.connection, admin, admin.publicKey, null, USDC_MINT_DECIMALS);
    }
  }
  let group = await adminClient.getGroup(groupPk);

  // USDC bank oracle: sponsored Pyth USDC/USD feed in pyth mode, stub in stub mode.
  let usdcOraclePk: PublicKey;
  if (ORACLE_MODE === 'pyth') {
    usdcOraclePk = PYTH_SPONSORED_FEED.USDC;
  } else {
    const usdcOracles = await adminClient.getStubOracle(group, usdcMint);
    let usdcStub = usdcOracles[0];
    if (!usdcStub) {
      await adminClient.stubOracleCreate(group, usdcMint, 1.0);
      usdcStub = (await adminClient.getStubOracle(group, usdcMint))[0];
    }
    usdcOraclePk = usdcStub.publicKey;
  }
  let hasUsdcBank = true;
  try {
    group.getFirstBankByMint(usdcMint);
  } catch {
    hasUsdcBank = false;
  }
  if (!hasUsdcBank) {
    await bootstrapTokenRegister(
      provider.connection,
      adminClient,
      PROGRAM_ID,
      group.publicKey,
      admin,
      usdcMint,
      usdcOraclePk,
      USDC_TOKEN_INDEX,
      'USDC',
    );
    console.log(JSON.stringify({ msg: 'usdc_bank_created' }));
    await group.reloadAll(adminClient);
  }
  if (await ensureFullCollateralQuoteBank(adminClient, group, usdcMint)) {
    console.log(JSON.stringify({ msg: 'usdc_bank_full_collateral_enabled' }));
    await group.reloadAll(adminClient);
  }

  // Create 2 mints + 2 perp markets
  const solMint = await createMint(provider.connection, admin, admin.publicKey, null, ASSET_MINT_DECIMALS);
  const btcMint = await createMint(provider.connection, admin, admin.publicKey, null, ASSET_MINT_DECIMALS);

  let pm0 = group.perpMarketsMapByMarketIndex.get(0 as PerpMarketIndex);
  if (!pm0) {
    await createPerpMarket(
      adminClient,
      group,
      solMint,
      0 as PerpMarketIndex,
      'SOL-PERP',
      100,
      PYTH_SPONSORED_FEED.SOL,
    );
    console.log(JSON.stringify({ msg: 'perp_market_created', market_index: 0, name: 'SOL-PERP' }));
    await group.reloadAll(adminClient);
  }

  let pm1 = group.perpMarketsMapByMarketIndex.get(1 as PerpMarketIndex);
  if (!pm1) {
    await createPerpMarket(
      adminClient,
      group,
      btcMint,
      1 as PerpMarketIndex,
      'BTC-PERP',
      60000,
      PYTH_SPONSORED_FEED.BTC,
    );
    console.log(JSON.stringify({ msg: 'perp_market_created', market_index: 1, name: 'BTC-PERP' }));
    await group.reloadAll(adminClient);
  }

  // Execution queue
  const [executionQueue] = PublicKey.findProgramAddressSync(
    [Buffer.from('ExecutionQueue'), group.publicKey.toBuffer()],
    PROGRAM_ID,
  );
  await ensureExecutionQueue(
    provider.connection,
    (ixs) => adminClient.sendAndConfirmTransaction(ixs),
    PROGRAM_ID,
    group.publicKey,
    executionQueue,
    admin.publicKey,
    admin.publicKey, // ctm_signer = admin for the localnet test
  );
  console.log(JSON.stringify({ msg: 'execution_queue_ready', pubkey: executionQueue.toBase58() }));

  // Maker / taker mango accounts
  const makerProvider = new AnchorProvider(provider.connection, new Wallet(maker), AnchorProvider.defaultOptions());
  const takerProvider = new AnchorProvider(provider.connection, new Wallet(taker), AnchorProvider.defaultOptions());
  const makerClient = await MangoClient.connect(makerProvider, CLUSTER, PROGRAM_ID, { idsSource: 'get-program-accounts' });
  const takerClient = await MangoClient.connect(takerProvider, CLUSTER, PROGRAM_ID, { idsSource: 'get-program-accounts' });

  // Fund the SOL accounts of maker/taker so they can sign txs
  const adminConn = provider.connection;
  for (const wallet of [maker, taker]) {
    const balance = await adminConn.getBalance(wallet.publicKey);
    if (balance < 0.5e9) {
      const sig = await adminConn.requestAirdrop(wallet.publicKey, 5e9);
      await adminConn.confirmTransaction(sig, 'confirmed');
    }
  }

  // USDC ATA + mint to maker/taker
  const makerUsdcAta = await createAssociatedTokenAccountIdempotent(provider.connection, admin, usdcMint, maker.publicKey);
  const takerUsdcAta = await createAssociatedTokenAccountIdempotent(provider.connection, admin, usdcMint, taker.publicKey);
  await mintTo(provider.connection, admin, usdcMint, makerUsdcAta, admin, 5_000_000_000_000);
  await mintTo(provider.connection, admin, usdcMint, takerUsdcAta, admin, 5_000_000_000_000);

  const makerAccount = await getOrCreateMangoAccount(makerClient, adminClient, group.publicKey, maker, 0, 'eq-maker');
  const takerAccount = await getOrCreateMangoAccount(takerClient, adminClient, group.publicKey, taker, 0, 'eq-taker');

  const makerGroup = await makerClient.getGroup(group.publicKey);
  const takerGroup = await takerClient.getGroup(group.publicKey);

  await makerClient.tokenDeposit(makerGroup, makerAccount, usdcMint, 50000);
  await takerClient.tokenDeposit(takerGroup, takerAccount, usdcMint, 50000);

  console.log(JSON.stringify({
    msg: 'mango_accounts_funded',
    maker: makerAccount.publicKey.toBase58(),
    taker: takerAccount.publicKey.toBase58(),
  }));

  // Build canonical remaining_accounts for both markets, both maker + taker
  const reloadedMaker = await makerClient.getMangoAccount(makerAccount.publicKey);
  const reloadedTaker = await takerClient.getMangoAccount(takerAccount.publicKey);
  const reloadedMakerGroup = await makerClient.getGroup(group.publicKey);
  const reloadedTakerGroup = await takerClient.getGroup(group.publicKey);

  const lanes: Array<{ name: string; remainingAccounts: Array<{ pubkey: string; isWritable: boolean; isSigner: boolean }> }> = [];
  for (const [side, client, grp, acct, owner] of [
    ['maker', makerClient, reloadedMakerGroup, reloadedMaker, maker.publicKey],
    ['taker', takerClient, reloadedTakerGroup, reloadedTaker, taker.publicKey],
  ] as const) {
    for (const marketIndex of [0, 1] as const) {
      const ra = await canonicalRemainingAccounts(
        client,
        grp,
        acct,
        marketIndex as PerpMarketIndex,
        owner,
      );
      lanes.push({
        name: `${side}-place-m${marketIndex}`,
        remainingAccounts: ra.map((a) => ({
          pubkey: a.pubkey.toBase58(),
          isWritable: !!a.isWritable,
          isSigner: !!a.isSigner,
        })),
      });
    }
  }

  fs.mkdirSync(path.dirname(LANE_CONFIG_PATH), { recursive: true });
  fs.writeFileSync(LANE_CONFIG_PATH, JSON.stringify(lanes, null, 2));

  const out = {
    cluster: CLUSTER,
    clusterUrl: CLUSTER_URL,
    programId: PROGRAM_ID.toBase58(),
    groupNum: GROUP_NUM,
    group: group.publicKey.toBase58(),
    executionQueue: executionQueue.toBase58(),
    executionQueueBuffer: executionQueue.toBase58(),
    ctmSigner: admin.publicKey.toBase58(),
    usdcMint: usdcMint.toBase58(),
    solMint: solMint.toBase58(),
    btcMint: btcMint.toBase58(),
    perpMarkets: [0, 1],
    oracleMode: ORACLE_MODE,
    oracles:
      ORACLE_MODE === 'pyth'
        ? {
            usdc: PYTH_SPONSORED_FEED.USDC.toBase58(),
            sol: PYTH_SPONSORED_FEED.SOL.toBase58(),
            btc: PYTH_SPONSORED_FEED.BTC.toBase58(),
          }
        : { note: 'stub oracles created per-mint; query via getStubOracle' },
    // Default sub-queue / relayer config consumed by the taker bot v3 and
    // other clients that read this file directly. Override via env at
    // runtime as needed.
    perpMarketIndex: 0,
    relayer: {
      bindAddr: process.env.E2E_RELAYER_BIND_ADDR || '127.0.0.1:9090',
    },
    maker: {
      keypairPath: path.resolve(MAKER_KEYPAIR_PATH),
      owner: maker.publicKey.toBase58(),
      mangoAccount: makerAccount.publicKey.toBase58(),
      usdcAta: makerUsdcAta.toBase58(),
    },
    taker: {
      keypairPath: path.resolve(TAKER_KEYPAIR_PATH),
      owner: taker.publicKey.toBase58(),
      mangoAccount: takerAccount.publicKey.toBase58(),
      usdcAta: takerUsdcAta.toBase58(),
    },
  };
  fs.mkdirSync(path.dirname(OUTPUT_CONFIG_PATH), { recursive: true });
  fs.writeFileSync(OUTPUT_CONFIG_PATH, JSON.stringify(out, null, 2));
  console.log(JSON.stringify({ msg: 'bootstrap_complete', config_path: OUTPUT_CONFIG_PATH, lanes_path: LANE_CONFIG_PATH }));
}

main().then(() => process.exit(0)).catch((err) => {
  console.error(err);
  process.exit(1);
});
