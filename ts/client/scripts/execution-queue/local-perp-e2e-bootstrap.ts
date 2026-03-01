import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  createAssociatedTokenAccountIdempotent,
  createMint,
  mintTo,
} from '@solana/spl-token';
import {
  AccountMeta,
  Cluster,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import os from 'os';
import path from 'path';
import {
  PerpMarketIndex,
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import { DefaultTokenRegisterParams } from '../../src/clientIxParamBuilder';
import { MANGO_V4_ID } from '../../src/constants';

dotenv.config();

const CLUSTER: Cluster = (process.env.CLUSTER_OVERRIDE as Cluster) || 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || 'http://127.0.0.1:8899';
const ADMIN_KEYPAIR =
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  process.env.MB_PAYER_KEYPAIR ||
  path.join(os.homedir(), '.config/solana/id.json');
const CTM_KEYPAIR = process.env.CTM_RELAYER_CTM_KEYPAIR || ADMIN_KEYPAIR;
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;
const GROUP_NUM = Number(process.env.EXECUTION_QUEUE_GROUP_NUM || '9101');
const EXECUTION_QUEUE_BUFFER_KEYPAIR =
  process.env.EXECUTION_QUEUE_BUFFER_KEYPAIR ||
  `/tmp/execution-queue-buffer-${GROUP_NUM}.json`;
const PERP_MARKET_INDEX = Number(process.env.PERP_MARKET_INDEX || '0');
const MAKER_KEYPAIR_PATH =
  process.env.E2E_MAKER_KEYPAIR_PATH || `/tmp/execution-queue-maker-${GROUP_NUM}.json`;
const TAKER_KEYPAIR_PATH =
  process.env.E2E_TAKER_KEYPAIR_PATH || `/tmp/execution-queue-taker-${GROUP_NUM}.json`;
const LANE_CONFIG_PATH =
  process.env.E2E_LANE_CONFIG_PATH || `/tmp/execution-queue-lanes-${GROUP_NUM}.json`;
const OUTPUT_CONFIG_PATH =
  process.env.E2E_OUTPUT_CONFIG_PATH || `/tmp/execution-queue-e2e-${GROUP_NUM}.json`;

const EXECUTION_QUEUE_BUFFER_SPACE = 8 + 32 + 4 + 4 + 1000 * 368;
const USDC_MINT_DECIMALS = 6;
const SOL_MINT_DECIMALS = 9;
const PERP_MARKET_INDEX_TYPED = PERP_MARKET_INDEX as PerpMarketIndex;
const MAKER_MAX_QUOTE_QTY = Number(process.env.E2E_MAKER_MAX_QUOTE_QTY || '1000');
const TAKER_MAX_QUOTE_QTY = Number(process.env.E2E_TAKER_MAX_QUOTE_QTY || '1000');

function readOrCreateKeypair(filePath: string): Keypair {
  const resolved = path.resolve(filePath);
  if (fs.existsSync(resolved)) {
    const raw = fs.readFileSync(resolved, 'utf-8');
    return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
  }
  const kp = Keypair.generate();
  fs.writeFileSync(resolved, JSON.stringify(Array.from(kp.secretKey)));
  return kp;
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function anchorDiscriminator(ixName: string): Buffer {
  return createHash('sha256')
    .update(`global:${ixName}`)
    .digest()
    .subarray(0, 8);
}

function executionQueueRemainingAccountsFromMangoIx(
  executionQueue: PublicKey,
  keys: { pubkey: PublicKey; isWritable: boolean; isSigner: boolean }[],
): AccountMeta[] {
  if (keys.length < 3) {
    throw new Error('expected at least 3 metas in mango instruction');
  }
  const remaining = keys.map((k) => ({
    pubkey: k.pubkey,
    isWritable: k.isWritable,
    isSigner: k.isSigner,
  }));
  remaining[2] = {
    pubkey: executionQueue,
    isWritable: remaining[2].isWritable,
    isSigner: false,
  };
  return remaining;
}

async function airdropIfNeeded(connection: Connection, pubkey: PublicKey): Promise<void> {
  const balance = await connection.getBalance(pubkey, 'confirmed');
  if (balance > 2e9) {
    return;
  }
  const sig = await connection.requestAirdrop(pubkey, 5e9);
  await connection.confirmTransaction(sig, 'confirmed');
}

async function getOrCreateMangoAccount(
  client: MangoClient,
  groupPk: PublicKey,
  owner: PublicKey,
  accountNum: number,
  name: string,
) {
  const group = await client.getGroup(groupPk);
  let account = await client.getMangoAccountForOwner(group, owner, accountNum);
  if (!account) {
    await client.createMangoAccount(group, accountNum, name, 8, 4, 4, 32);
    account = await client.getMangoAccountForOwner(group, owner, accountNum);
  }
  if (!account) {
    throw new Error(`failed to create mango account for ${owner.toBase58()}`);
  }
  return account;
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const ctm = readKeypair(CTM_KEYPAIR);
  const maker = readOrCreateKeypair(MAKER_KEYPAIR_PATH);
  const taker = readOrCreateKeypair(TAKER_KEYPAIR_PATH);

  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];

  const adminClient = await MangoClient.connect(provider, CLUSTER, programId, {
    idsSource: 'get-program-accounts',
  });

  await airdropIfNeeded(provider.connection, admin.publicKey);
  await airdropIfNeeded(provider.connection, maker.publicKey);
  await airdropIfNeeded(provider.connection, taker.publicKey);

  const usdcMint = await createMint(
    provider.connection,
    admin,
    admin.publicKey,
    null,
    USDC_MINT_DECIMALS,
  );
  const solMint = await createMint(
    provider.connection,
    admin,
    admin.publicKey,
    null,
    SOL_MINT_DECIMALS,
  );

  const groupNumBuf = Buffer.alloc(4);
  groupNumBuf.writeUInt32LE(GROUP_NUM);
  const [groupPk] = PublicKey.findProgramAddressSync(
    [Buffer.from('Group'), admin.publicKey.toBuffer(), groupNumBuf],
    programId,
  );

  const groupInfo = await provider.connection.getAccountInfo(groupPk);
  if (!groupInfo) {
    await adminClient.groupCreate(GROUP_NUM, true, 0, usdcMint);
  }
  let group = await adminClient.getGroup(groupPk);

  const usdcOracles = await adminClient.getStubOracle(group, usdcMint);
  const usdcOracle = usdcOracles[0]
    ? usdcOracles[0]
    : (await (async () => {
        await adminClient.stubOracleCreate(group, usdcMint, 1.0);
        return (await adminClient.getStubOracle(group, usdcMint))[0];
      })());

  const solOracles = await adminClient.getStubOracle(group, solMint);
  const solOracle = solOracles[0]
    ? solOracles[0]
    : (await (async () => {
        await adminClient.stubOracleCreate(group, solMint, 100.0);
        return (await adminClient.getStubOracle(group, solMint))[0];
      })());

  let hasUsdcBank = true;
  try {
    group.getFirstBankByMint(usdcMint);
  } catch {
    hasUsdcBank = false;
  }

  if (!hasUsdcBank) {
    await adminClient.tokenRegister(
      group,
      usdcMint,
      usdcOracle.publicKey,
      PublicKey.default,
      0,
      'USDC',
      {
        ...DefaultTokenRegisterParams,
        loanFeeRate: 0,
        loanOriginationFeeRate: 0,
        maintAssetWeight: 1,
        initAssetWeight: 1,
        maintLiabWeight: 1,
        initLiabWeight: 1,
        liquidationFee: 0,
      },
    );
    await group.reloadAll(adminClient);
  }

  if (!group.perpMarketsMapByMarketIndex.get(PERP_MARKET_INDEX_TYPED)) {
    await adminClient.perpCreateMarket(
      group,
      solOracle.publicKey,
      PERP_MARKET_INDEX_TYPED,
      'SOL-PERP',
      {
        confFilter: 0.1,
        maxStalenessSlots: null,
      },
      6,
      10,
      100,
      0.9,
      0.8,
      1.1,
      1.2,
      0.0,
      0.0,
      0.05,
      -0.001,
      0.002,
      0,
      -0.1,
      0.1,
      10,
      false,
      0,
      0,
      0,
      0,
      -1.0,
      2 * 60 * 60,
      0.025,
      0.0,
    );
    await group.reloadAll(adminClient);
  }

  const [executionQueue] = PublicKey.findProgramAddressSync(
    [Buffer.from('ExecutionQueue'), group.publicKey.toBuffer()],
    programId,
  );

  const queueInfo = await provider.connection.getAccountInfo(executionQueue);
  const queueBuffer = readOrCreateKeypair(EXECUTION_QUEUE_BUFFER_KEYPAIR);
  let executionQueueBuffer = queueBuffer.publicKey;

  if (!queueInfo) {
    const queueBufferInfo = await provider.connection.getAccountInfo(queueBuffer.publicKey);
    if (!queueBufferInfo) {
      const lamports = await provider.connection.getMinimumBalanceForRentExemption(
        EXECUTION_QUEUE_BUFFER_SPACE,
      );
      const createBufferIx = SystemProgram.createAccount({
        fromPubkey: admin.publicKey,
        newAccountPubkey: queueBuffer.publicKey,
        lamports,
        space: EXECUTION_QUEUE_BUFFER_SPACE,
        programId,
      });
      await provider.sendAndConfirm(new Transaction().add(createBufferIx), [queueBuffer]);
    }

    const initIx = new TransactionInstruction({
      programId,
      keys: [
        { pubkey: group.publicKey, isSigner: false, isWritable: true },
        { pubkey: executionQueue, isSigner: false, isWritable: true },
        { pubkey: admin.publicKey, isSigner: true, isWritable: true },
        { pubkey: admin.publicKey, isSigner: true, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        { pubkey: queueBuffer.publicKey, isSigner: false, isWritable: true },
      ],
      data: Buffer.concat([
        anchorDiscriminator('execution_queue_init'),
        ctm.publicKey.toBuffer(),
      ]),
    });
    await adminClient.sendAndConfirmTransaction([initIx]);
  } else if (queueInfo.data.length >= 136) {
    executionQueueBuffer = new PublicKey(queueInfo.data.subarray(104, 136));
  }

  await group.reloadAll(adminClient);

  const makerProvider = new AnchorProvider(
    provider.connection,
    new Wallet(maker),
    AnchorProvider.defaultOptions(),
  );
  const takerProvider = new AnchorProvider(
    provider.connection,
    new Wallet(taker),
    AnchorProvider.defaultOptions(),
  );
  const makerClient = await MangoClient.connect(makerProvider, CLUSTER, programId, {
    idsSource: 'get-program-accounts',
  });
  const takerClient = await MangoClient.connect(takerProvider, CLUSTER, programId, {
    idsSource: 'get-program-accounts',
  });

  const makerUsdcAta = await createAssociatedTokenAccountIdempotent(
    provider.connection,
    admin,
    usdcMint,
    maker.publicKey,
  );
  const takerUsdcAta = await createAssociatedTokenAccountIdempotent(
    provider.connection,
    admin,
    usdcMint,
    taker.publicKey,
  );
  await mintTo(provider.connection, admin, usdcMint, makerUsdcAta, admin, 2_000_000_000_000);
  await mintTo(provider.connection, admin, usdcMint, takerUsdcAta, admin, 2_000_000_000_000);

  const makerAccount = await getOrCreateMangoAccount(
    makerClient,
    group.publicKey,
    maker.publicKey,
    0,
    'eq-maker',
  );
  const takerAccount = await getOrCreateMangoAccount(
    takerClient,
    group.publicKey,
    taker.publicKey,
    0,
    'eq-taker',
  );
  const makerGroup = await makerClient.getGroup(group.publicKey);
  const takerGroup = await takerClient.getGroup(group.publicKey);

  await makerClient.editMangoAccount(
    makerGroup,
    makerAccount,
    undefined,
    executionQueue,
  );
  await takerClient.editMangoAccount(
    takerGroup,
    takerAccount,
    undefined,
    executionQueue,
  );

  await makerClient.tokenDeposit(makerGroup, makerAccount, usdcMint, 10000);
  await takerClient.tokenDeposit(takerGroup, takerAccount, usdcMint, 10000);

  const makerPlaceIx = await makerClient.perpPlaceOrderV2Ix(
    makerGroup,
    makerAccount,
    PERP_MARKET_INDEX_TYPED,
    PerpOrderSide.bid,
    99,
    2,
    MAKER_MAX_QUOTE_QTY,
    Date.now(),
    PerpOrderType.limit,
    PerpSelfTradeBehavior.decrementTake,
    false,
    0,
    20,
  );
  const takerPlaceIx = await takerClient.perpPlaceOrderV2Ix(
    takerGroup,
    takerAccount,
    PERP_MARKET_INDEX_TYPED,
    PerpOrderSide.ask,
    98,
    1,
    TAKER_MAX_QUOTE_QTY,
    Date.now() + 1,
    PerpOrderType.limit,
    PerpSelfTradeBehavior.decrementTake,
    false,
    0,
    20,
  );
  const makerCancelAllIx = await makerClient.perpCancelAllOrdersIx(
    makerGroup,
    makerAccount,
    PERP_MARKET_INDEX_TYPED,
    255,
  );

  const lanes = [
    {
      name: 'maker-place',
      remainingAccounts: executionQueueRemainingAccountsFromMangoIx(
        executionQueue,
        makerPlaceIx.keys,
      ).map((a) => ({
        pubkey: a.pubkey.toBase58(),
        isWritable: a.isWritable,
        isSigner: !!a.isSigner,
      })),
    },
    {
      name: 'taker-place',
      remainingAccounts: executionQueueRemainingAccountsFromMangoIx(
        executionQueue,
        takerPlaceIx.keys,
      ).map((a) => ({
        pubkey: a.pubkey.toBase58(),
        isWritable: a.isWritable,
        isSigner: !!a.isSigner,
      })),
    },
    {
      name: 'maker-cancel-all',
      remainingAccounts: executionQueueRemainingAccountsFromMangoIx(
        executionQueue,
        makerCancelAllIx.keys,
      ).map((a) => ({
        pubkey: a.pubkey.toBase58(),
        isWritable: a.isWritable,
        isSigner: !!a.isSigner,
      })),
    },
  ];
  fs.writeFileSync(LANE_CONFIG_PATH, JSON.stringify(lanes, null, 2));

  const out = {
    cluster: CLUSTER,
    clusterUrl: CLUSTER_URL,
    programId: programId.toBase58(),
    groupNum: GROUP_NUM,
    group: group.publicKey.toBase58(),
    executionQueue: executionQueue.toBase58(),
    executionQueueBuffer: executionQueueBuffer.toBase58(),
    ctmSigner: ctm.publicKey.toBase58(),
    usdcMint: usdcMint.toBase58(),
    solMint: solMint.toBase58(),
    usdcOracle: usdcOracle.publicKey.toBase58(),
    solOracle: solOracle.publicKey.toBase58(),
    perpMarketIndex: PERP_MARKET_INDEX,
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
    relayer: {
      bindAddr: '127.0.0.1:9090',
      payerKeypairPath: path.resolve(ADMIN_KEYPAIR),
      ctmKeypairPath: path.resolve(CTM_KEYPAIR),
      sequenceStatePath: `/tmp/ctm-sequences-${GROUP_NUM}.json`,
    },
    cranker: {
      keypairPath: path.resolve(ADMIN_KEYPAIR),
      laneConfigPath: path.resolve(LANE_CONFIG_PATH),
      maxItems: 8,
      intervalMs: 1000,
    },
  };

  fs.writeFileSync(OUTPUT_CONFIG_PATH, JSON.stringify(out, null, 2));
  console.log(JSON.stringify({ configPath: OUTPUT_CONFIG_PATH, ...out }, null, 2));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
