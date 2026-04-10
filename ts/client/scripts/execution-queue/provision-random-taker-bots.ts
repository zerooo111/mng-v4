import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  Cluster,
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  sendAndConfirmTransaction,
  SystemProgram,
  Transaction,
} from '@solana/web3.js';
import { getAssociatedTokenAddressSync } from '@solana/spl-token';
import axios from 'axios';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { MangoClient } from '../../src/client';
import { keypairsDir, runtimeConfigPath } from './scriptEnv';

dotenv.config();

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  usdcMint: string;
  groupNum?: number;
  relayer?: {
    payerKeypairPath?: string;
  };
};

type TakerBotManifestItem = {
  name: string;
  keypairPath: string;
  owner: string;
  mangoAccount: string;
  usdcAta: string;
  accountNum: number;
};

const CONFIG_PATH =
  process.env.TAKER_BOT_CONFIG_PATH ||
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  runtimeConfigPath('execution-queue-e2e-9126.json');
const BOT_COUNT = Number(process.env.TAKER_BOT_COUNT || '10');
const ACCOUNT_NUM_START = Number(process.env.TAKER_BOT_ACCOUNT_NUM_START || '200');
const SOL_TARGET_UI = Number(process.env.TAKER_BOT_SOL_UI || '0.1');
const USDC_TARGET_UI = Number(process.env.TAKER_BOT_USDC_UI || '1000');
const NAME_PREFIX = process.env.TAKER_BOT_NAME_PREFIX || 'taker-bot';
const KEYPAIR_DIR =
  process.env.TAKER_BOT_KEYPAIR_DIR ||
  path.resolve(keypairsDir(), 'taker-bots');
const OUTPUT_PATH =
  process.env.TAKER_BOTS_OUTPUT_PATH ||
  runtimeConfigPath('taker-bots-9126.json');
const HARNESS_URL =
  process.env.HARNESS_URL ||
  process.env.TAKER_HARNESS_URL ||
  'http://127.0.0.1:9091';
const FALLBACK_ADMIN =
  process.env.MB_PAYER_KEYPAIR || '/home/hetalkenaudekar/.config/solana/id.json';
const RETRY_MAX_ATTEMPTS = Number(process.env.TAKER_BOT_RETRY_MAX_ATTEMPTS || '6');
const RETRY_DELAY_MS = Number(process.env.TAKER_BOT_RETRY_DELAY_MS || '1500');

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function readKeypair(filePath: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path.resolve(filePath), 'utf-8'));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

function writeKeypair(filePath: string, keypair: Keypair): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(
    filePath,
    JSON.stringify(Array.from(keypair.secretKey), null, 2),
    'utf-8',
  );
  fs.chmodSync(filePath, 0o600);
}

function readOrCreateKeypair(filePath: string): Keypair {
  const resolved = path.resolve(filePath);
  if (fs.existsSync(resolved)) {
    return readKeypair(resolved);
  }
  const generated = Keypair.generate();
  writeKeypair(resolved, generated);
  return generated;
}

function shouldRetry(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : `${err}`;
  return (
    msg.includes('429') ||
    msg.includes('502') ||
    msg.includes('503') ||
    msg.includes('504') ||
    msg.includes('Blockhash not found') ||
    msg.includes('Timed out') ||
    msg.includes('ECONNRESET') ||
    msg.includes('socket hang up') ||
    msg.includes('Too Many Requests') ||
    msg.includes('Transaction was not confirmed') ||
    msg.includes('failed to get recent blockhash')
  );
}

async function withRetry<T>(label: string, fn: () => Promise<T>): Promise<T> {
  let attempt = 0;
  let lastErr: unknown;
  while (attempt < RETRY_MAX_ATTEMPTS) {
    attempt += 1;
    try {
      return await fn();
    } catch (err) {
      lastErr = err;
      if (!shouldRetry(err) || attempt >= RETRY_MAX_ATTEMPTS) {
        throw err;
      }
      const msg = err instanceof Error ? err.message : `${err}`;
      console.warn(
        `[retry] ${label} failed (attempt ${attempt}/${RETRY_MAX_ATTEMPTS}): ${msg}`,
      );
      await sleep(RETRY_DELAY_MS);
    }
  }
  throw lastErr;
}

async function getSolBalanceUi(
  connection: Connection,
  owner: PublicKey,
): Promise<number> {
  return (await connection.getBalance(owner, 'confirmed')) / LAMPORTS_PER_SOL;
}

async function getTokenBalanceUi(
  connection: Connection,
  tokenAccount: PublicKey,
): Promise<number> {
  try {
    const balance = await connection.getTokenAccountBalance(tokenAccount, 'confirmed');
    return Number(balance.value.uiAmountString || '0');
  } catch {
    return 0;
  }
}

async function ensureHarnessHealthy(): Promise<void> {
  const response = await axios.get(`${HARNESS_URL}/healthz`, {
    timeout: 5000,
  });
  if (!response.data?.ok) {
    throw new Error(`harness unhealthy at ${HARNESS_URL}`);
  }
  if (!response.data?.airdrop_enabled) {
    throw new Error(`harness airdrop disabled at ${HARNESS_URL}`);
  }
}

async function ensureSolFunding(
  connection: Connection,
  admin: Keypair,
  owner: PublicKey,
): Promise<void> {
  const currentUi = await getSolBalanceUi(connection, owner);
  if (currentUi + 1e-9 >= SOL_TARGET_UI) {
    return;
  }
  const lamportsNeeded = Math.ceil((SOL_TARGET_UI - currentUi) * LAMPORTS_PER_SOL);
  const tx = new Transaction().add(
    SystemProgram.transfer({
      fromPubkey: admin.publicKey,
      toPubkey: owner,
      lamports: lamportsNeeded,
    }),
  );
  await sendAndConfirmTransaction(connection, tx, [admin], {
    commitment: 'confirmed',
  });
}

async function airdropUsdc(owner: PublicKey, uiAmount: number): Promise<void> {
  const response = await axios.post(
    `${HARNESS_URL}/airdrop`,
    {
      owner: owner.toBase58(),
      ui_amount: uiAmount,
    },
    {
      headers: { 'Content-Type': 'application/json' },
      timeout: 20000,
    },
  );
  if (!response.data?.ok) {
    throw new Error(`airdrop failed for ${owner.toBase58()}`);
  }
}

async function main(): Promise<void> {
  if (BOT_COUNT < 1) {
    throw new Error('TAKER_BOT_COUNT must be >= 1');
  }

  const config = JSON.parse(
    fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8'),
  ) as E2EConfig;
  const clusterUrl = process.env.CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const adminKeypairPath = config.relayer?.payerKeypairPath || FALLBACK_ADMIN;

  const connection = new Connection(clusterUrl, AnchorProvider.defaultOptions());
  const admin = readKeypair(adminKeypairPath);
  const adminProvider = new AnchorProvider(
    connection,
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const adminClient = await MangoClient.connect(
    adminProvider,
    config.cluster,
    new PublicKey(config.programId),
    { idsSource: 'get-program-accounts' },
  );

  await ensureHarnessHealthy();

  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const usdcMint = new PublicKey(config.usdcMint);
  const adminGroup = await adminClient.getGroup(groupPk);
  const usdcBank = adminGroup.getFirstBankByMint(usdcMint);

  fs.mkdirSync(path.resolve(KEYPAIR_DIR), { recursive: true });
  fs.mkdirSync(path.dirname(path.resolve(OUTPUT_PATH)), { recursive: true });

  const manifest: TakerBotManifestItem[] = [];
  for (let i = 0; i < BOT_COUNT; i++) {
    const name = `${NAME_PREFIX}-${i}`;
    const accountNum = ACCOUNT_NUM_START + i;
    const keypairPath = path.resolve(KEYPAIR_DIR, `${name}.json`);
    const ownerKeypair = readOrCreateKeypair(keypairPath);
    const ownerPk = ownerKeypair.publicKey;
    const usdcAta = getAssociatedTokenAddressSync(usdcMint, ownerPk);

    console.log(`\n[${name}] owner=${ownerPk.toBase58()} account_num=${accountNum}`);

    await withRetry(`ensureSolFunding(${name})`, async () => {
      await ensureSolFunding(connection, admin, ownerPk);
    });
    const solBalanceUi = await getSolBalanceUi(connection, ownerPk);
    console.log(`[${name}] SOL balance ${solBalanceUi.toFixed(4)}`);

    const ownerProvider = new AnchorProvider(
      connection,
      new Wallet(ownerKeypair),
      AnchorProvider.defaultOptions(),
    );
    const ownerClient = await MangoClient.connect(
      ownerProvider,
      config.cluster,
      new PublicKey(config.programId),
      { idsSource: 'get-program-accounts' },
    );
    const ownerGroup = await ownerClient.getGroup(groupPk);

    let mangoAccount = await withRetry(`getMangoAccountForOwner(${name})`, async () => {
      return await ownerClient.getMangoAccountForOwner(ownerGroup, ownerPk, accountNum);
    });

    let mangoDepositUi = mangoAccount
      ? mangoAccount.getTokenDepositsUi(usdcBank)
      : 0;
    let walletUsdcUi = await getTokenBalanceUi(connection, usdcAta);

    const requiredWalletUsdcUi = Math.max(0, USDC_TARGET_UI - mangoDepositUi);
    const airdropUi = Math.max(0, requiredWalletUsdcUi - walletUsdcUi);
    if (airdropUi > 0.000001) {
      console.log(`[${name}] airdropping ${airdropUi.toFixed(6)} USDC to wallet`);
      await withRetry(`airdropUsdc(${name})`, async () => {
        await airdropUsdc(ownerPk, airdropUi);
      });
      walletUsdcUi = await getTokenBalanceUi(connection, usdcAta);
    }
    console.log(`[${name}] wallet USDC ${walletUsdcUi.toFixed(6)}`);

    if (!mangoAccount) {
      console.log(`[${name}] creating Mango account`);
      await withRetry(`createMangoAccount(${name})`, async () => {
        await ownerClient.createMangoAccount(
          ownerGroup,
          accountNum,
          name.slice(0, 32),
          8,
          4,
          4,
          32,
        );
      });
      mangoAccount = await withRetry(`reloadMangoAccount(${name})`, async () => {
        return await ownerClient.getMangoAccountForOwner(
          ownerGroup,
          ownerPk,
          accountNum,
        );
      });
    }

    if (!mangoAccount) {
      throw new Error(`failed to load Mango account for ${name}`);
    }

    if (!mangoAccount.delegate.equals(executionQueuePk)) {
      console.log(`[${name}] setting execution-queue delegate`);
      await withRetry(`editMangoAccount(${name})`, async () => {
        await ownerClient.editMangoAccount(
          ownerGroup,
          mangoAccount!,
          undefined,
          executionQueuePk,
        );
      });
      mangoAccount = await withRetry(`reloadEditedMangoAccount(${name})`, async () => {
        return await ownerClient.getMangoAccount(mangoAccount!.publicKey);
      });
    }

    mangoDepositUi = mangoAccount.getTokenDepositsUi(usdcBank);
    walletUsdcUi = await getTokenBalanceUi(connection, usdcAta);
    const depositUi = Math.min(walletUsdcUi, Math.max(0, USDC_TARGET_UI - mangoDepositUi));
    if (depositUi > 0.000001) {
      console.log(`[${name}] depositing ${depositUi.toFixed(6)} USDC into Mango`);
      await withRetry(`tokenDeposit(${name})`, async () => {
        await ownerClient.tokenDeposit(
          ownerGroup,
          mangoAccount!,
          usdcMint,
          depositUi,
        );
      });
      mangoAccount = await withRetry(`reloadDepositedMangoAccount(${name})`, async () => {
        return await ownerClient.getMangoAccount(mangoAccount!.publicKey);
      });
    }

    mangoDepositUi = mangoAccount.getTokenDepositsUi(usdcBank);
    walletUsdcUi = await getTokenBalanceUi(connection, usdcAta);
    console.log(
      `[${name}] ready mango_account=${mangoAccount.publicKey.toBase58()} wallet_usdc=${walletUsdcUi.toFixed(6)} mango_usdc=${mangoDepositUi.toFixed(6)}`,
    );

    manifest.push({
      name,
      keypairPath,
      owner: ownerPk.toBase58(),
      mangoAccount: mangoAccount.publicKey.toBase58(),
      usdcAta: usdcAta.toBase58(),
      accountNum,
    });
  }

  fs.writeFileSync(path.resolve(OUTPUT_PATH), JSON.stringify(manifest, null, 2));
  console.log(`\nWrote ${manifest.length} taker bots to ${path.resolve(OUTPUT_PATH)}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
