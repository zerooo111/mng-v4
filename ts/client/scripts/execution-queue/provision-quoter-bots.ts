import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  createAssociatedTokenAccountIdempotent,
  mintTo,
} from '@solana/spl-token';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { PerpOrderSide } from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import {
  keypairsDir,
  runtimeConfigPath,
} from './scriptEnv';

dotenv.config();

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  usdcMint: string;
  perpMarketIndex: number;
  relayer?: {
    payerKeypairPath?: string;
  };
};

type QuoterBotSpec = {
  name: string;
  keypairPath: string;
  owner: string;
  mangoAccount: string;
  side: 'bid' | 'ask';
};

const CONFIG_PATH =
  process.env.QUOTER_CONFIG_PATH ||
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  runtimeConfigPath('execution-queue-e2e-9101.json');
const BOT_COUNT = Number(process.env.QUOTER_BOT_COUNT || '5');
const ACCOUNT_NUM_START = Number(process.env.QUOTER_BOT_ACCOUNT_NUM_START || '100');
const DEPOSIT_UI_AMOUNT = Number(process.env.QUOTER_BOT_DEPOSIT_UI_AMOUNT || '10000');
const KEYPAIR_DIR =
  process.env.QUOTER_BOT_KEYPAIR_DIR ||
  path.resolve(keypairsDir(), 'quoter-bots');
const OUTPUT_PATH =
  process.env.QUOTER_BOTS_OUTPUT_PATH ||
  runtimeConfigPath('quoter-bots-9120.json');
const FALLBACK_ADMIN =
  process.env.MB_PAYER_KEYPAIR || '/home/ec2-user/.config/solana/id.json';
const RETRY_MAX_ATTEMPTS = Number(process.env.QUOTER_BOT_RETRY_MAX_ATTEMPTS || '6');
const RETRY_DELAY_MS = Number(process.env.QUOTER_BOT_RETRY_DELAY_MS || '1500');

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function readRequiredKeypair(filePath: string): Keypair {
  const resolved = path.resolve(filePath);
  if (!fs.existsSync(resolved)) {
    throw new Error(`missing required quoter bot keypair: ${resolved}`);
  }
  return readKeypair(resolved);
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

function sideForBot(index: number): PerpOrderSide {
  return index % 2 === 0 ? PerpOrderSide.bid : PerpOrderSide.ask;
}

function shouldRetry(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : `${err}`;
  return (
    msg.includes('ECONNRESET') ||
    msg.includes('429') ||
    msg.includes('Blockhash not found') ||
    msg.includes('Timed out') ||
    msg.includes('Transaction was not confirmed')
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
      await new Promise((resolve) => setTimeout(resolve, RETRY_DELAY_MS));
    }
  }
  throw lastErr;
}

async function main(): Promise<void> {
  if (BOT_COUNT < 1) {
    throw new Error('QUOTER_BOT_COUNT must be >= 1');
  }

  const config = JSON.parse(fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8')) as E2EConfig;
  const clusterUrl = process.env.CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const adminKeypairPath = config.relayer?.payerKeypairPath || FALLBACK_ADMIN;

  const admin = readKeypair(adminKeypairPath);
  const provider = new AnchorProvider(
    new Connection(clusterUrl, AnchorProvider.defaultOptions()),
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );

  const programId = new PublicKey(config.programId);
  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const usdcMint = new PublicKey(config.usdcMint);

  const adminClient = await MangoClient.connect(provider, config.cluster, programId, {
    idsSource: 'get-program-accounts',
  });

  fs.mkdirSync(path.resolve(KEYPAIR_DIR), { recursive: true });
  fs.mkdirSync(path.dirname(path.resolve(OUTPUT_PATH)), { recursive: true });

  const bots: QuoterBotSpec[] = [];
  for (let i = 0; i < BOT_COUNT; i++) {
    const keypairPath = path.resolve(KEYPAIR_DIR, `quoter-bot-${i}.json`);
    const botKp = readRequiredKeypair(keypairPath);

    const botProvider = new AnchorProvider(
      provider.connection,
      new Wallet(botKp),
      AnchorProvider.defaultOptions(),
    );
    const botClient = await MangoClient.connect(botProvider, config.cluster, programId, {
      idsSource: 'get-program-accounts',
    });
    const botGroup = await botClient.getGroup(groupPk);
    const accountNum = ACCOUNT_NUM_START + i;
    const botAccount = await withRetry(
      `getOrCreateMangoAccount(${i})`,
      async () =>
        await getOrCreateMangoAccount(
          botClient,
          groupPk,
          botKp.publicKey,
          accountNum,
          `qbot-${i}`,
        ),
    );

    await withRetry(
      `editMangoAccount(${i})`,
      async () =>
        await botClient.editMangoAccount(
          botGroup,
          botAccount,
          undefined,
          executionQueuePk,
        ),
    );

    const botUsdcAta = await withRetry(
      `createAssociatedTokenAccountIdempotent(${i})`,
      async () =>
        await createAssociatedTokenAccountIdempotent(
          provider.connection,
          admin,
          usdcMint,
          botKp.publicKey,
        ),
    );
    await withRetry(
      `mintTo(${i})`,
      async () =>
        await mintTo(
          provider.connection,
          admin,
          usdcMint,
          botUsdcAta,
          admin,
          BigInt(Math.round(DEPOSIT_UI_AMOUNT * 1_000_000)),
        ),
    );
    await withRetry(
      `tokenDeposit(${i})`,
      async () =>
        await botClient.tokenDeposit(
          botGroup,
          botAccount,
          usdcMint,
          DEPOSIT_UI_AMOUNT,
        ),
    );

    const side = sideForBot(i);
    bots.push({
      name: `quoter-bot-${i}`,
      keypairPath,
      owner: botKp.publicKey.toBase58(),
      mangoAccount: botAccount.publicKey.toBase58(),
      side: side === PerpOrderSide.bid ? 'bid' : 'ask',
    });
  }

  fs.writeFileSync(path.resolve(OUTPUT_PATH), JSON.stringify(bots, null, 2));
  console.log(
    JSON.stringify(
      {
        status: 'ok',
        botCount: bots.length,
        outputPath: path.resolve(OUTPUT_PATH),
        bots,
      },
      null,
      2,
    ),
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
