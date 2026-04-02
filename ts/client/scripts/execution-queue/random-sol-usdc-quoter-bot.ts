import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import {
  createAssociatedTokenAccountIdempotent,
  mintTo,
} from '@solana/spl-token';
import {
  AccountMeta,
  Cluster,
  Commitment,
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import {
  PerpMarketIndex,
  PerpMarket,
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import {
  buildExecutionQueueUserIntent,
  encodePerpCancelOrderByClientOrderIdQueuePayload,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { decodeExecutionQueueCount } from '../../src/executionQueueLayout';
import { runtimeConfigPath } from './scriptEnv';

dotenv.config();

const WS_HANDSHAKE_405 = 'Unexpected server response: 405';

function shouldIgnoreBackgroundWsHandshakeError(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : `${err}`;
  return msg.includes(WS_HANDSHAKE_405);
}

process.on('uncaughtException', (err) => {
  if (shouldIgnoreBackgroundWsHandshakeError(err)) {
    console.error(
      `ignoring background websocket handshake error: ${err instanceof Error ? err.message : err}`,
    );
    return;
  }
  console.error(err);
  process.exit(1);
});

process.on('unhandledRejection', (err) => {
  if (shouldIgnoreBackgroundWsHandshakeError(err)) {
    console.error(
      `ignoring background websocket rejection: ${err instanceof Error ? err.message : err}`,
    );
    return;
  }
  console.error('unhandled rejection in quoter:', err);
});

type BotSpec = {
  name: string;
  keypairPath: string;
  mangoAccount?: string;
  side: PerpOrderSide;
};

type BotSpecWire = {
  name: string;
  keypairPath: string;
  mangoAccount?: string;
  side: 'bid' | 'ask' | 'buy' | 'sell';
};

type BotRuntime = {
  name: string;
  keypair: Keypair;
  mangoAccountPk: PublicKey;
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
  side: PerpOrderSide;
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  lastPlacedClientOrderId: number | null;
};

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  perpMarketIndex: number;
  solMint?: string;
  usdcMint?: string;
  maker?: {
    keypairPath?: string;
    mangoAccount?: string;
  };
  taker?: {
    keypairPath?: string;
    mangoAccount?: string;
  };
  relayer?: {
    bindAddr?: string;
  };
};

const CONFIG_PATH =
  process.env.QUOTER_CONFIG_PATH ||
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  runtimeConfigPath('execution-queue-e2e-9101.json');
const CLUSTER_URL_OVERRIDE = process.env.CLUSTER_URL_OVERRIDE;
const CLUSTER_WS_URL_OVERRIDE =
  process.env.CLUSTER_WS_URL_OVERRIDE || process.env.MB_CLUSTER_WS_URL || '';
const RELAYER_ADDR_OVERRIDE = process.env.CTM_RELAYER_ADDR;
const COMMITMENT: Commitment =
  (process.env.QUOTER_COMMITMENT as Commitment) || 'confirmed';
const INTERVAL_MS = Number(process.env.QUOTER_INTERVAL_MS || '2000');
const PRICE_RANGE_BPS = Number(process.env.QUOTER_PRICE_RANGE_BPS || '200');
const QUOTER_BID_MIN_BPS = Number(process.env.QUOTER_BID_MIN_BPS || '-200');
const QUOTER_BID_MAX_BPS = Number(process.env.QUOTER_BID_MAX_BPS || '-50');
const QUOTER_ASK_MIN_BPS = Number(process.env.QUOTER_ASK_MIN_BPS || '50');
const QUOTER_ASK_MAX_BPS = Number(process.env.QUOTER_ASK_MAX_BPS || '200');
const SIZE_MIN_SOL = Number(process.env.QUOTER_SIZE_MIN_SOL || '1');
const SIZE_MAX_SOL = Number(process.env.QUOTER_SIZE_MAX_SOL || '3');
const ORDER_EXPIRY_SECS = Number(process.env.QUOTER_ORDER_EXPIRY_SECS || '60');
const CLOSE_POSITION_PROBABILITY_BPS = Number(
  process.env.QUOTER_CLOSE_POSITION_PROBABILITY_BPS || '1000',
);
const MIN_EXECUTE_SLOT_OFFSET = BigInt(
  process.env.QUOTER_MIN_EXECUTE_SLOT_OFFSET || '1',
);
const CANCEL_BEFORE_PLACE =
  (process.env.QUOTER_CANCEL_BEFORE_PLACE || 'false') === 'true';
const CANCEL_MODE = (process.env.QUOTER_CANCEL_MODE || 'all').toLowerCase();
const CANCEL_EVERY_TICKS = Number(process.env.QUOTER_CANCEL_EVERY_TICKS || '1');
const CANCEL_LIMIT = Number(process.env.QUOTER_CANCEL_LIMIT || '255');
const ORDER_LIMIT = Number(process.env.QUOTER_ORDER_LIMIT || '20');
const QUOTE_BUDGET_MULTIPLIER = Number(
  process.env.QUOTER_QUOTE_BUDGET_MULTIPLIER || '1.25',
);
const COINGECKO_API_BASE =
  process.env.QUOTER_COINGECKO_API_BASE || 'https://api.coingecko.com/api/v3';
const COINGECKO_ASSET_ID =
  process.env.QUOTER_COINGECKO_ASSET_ID || 'solana';
const COINGECKO_VS_CURRENCY =
  process.env.QUOTER_COINGECKO_VS_CURRENCY || 'usd';
const COINGECKO_API_KEY = process.env.QUOTER_COINGECKO_API_KEY || '';
const COINGECKO_TIMEOUT_MS = Number(
  process.env.QUOTER_COINGECKO_TIMEOUT_MS || '1500',
);
const COINGECKO_REFRESH_MS = Number(
  process.env.QUOTER_COINGECKO_REFRESH_MS || '10000',
);
const FIXED_REFERENCE_PRICE = process.env.QUOTER_FIXED_REFERENCE_PRICE
  ? Number(process.env.QUOTER_FIXED_REFERENCE_PRICE)
  : 0;
const USE_LIMIT_ORDER = (process.env.QUOTER_USE_LIMIT_ORDER || 'false') === 'true';
const BOTS_JSON_PATH = process.env.QUOTER_BOTS_JSON_PATH || '';
const BOTS_JSON = process.env.QUOTER_BOTS_JSON || '';
const PARALLEL_BOT_EXECUTION =
  (process.env.QUOTER_PARALLEL_BOT_EXECUTION || 'true') === 'true';
const LOG_TPS_EVERY_TICKS = Number(process.env.QUOTER_LOG_TPS_EVERY_TICKS || '5');
const LOG_EACH_ORDER = (process.env.QUOTER_LOG_EACH_ORDER || 'true') === 'true';
const NONBLOCKING_SUBMIT =
  (process.env.QUOTER_NONBLOCKING_SUBMIT || 'false') === 'true';
const MAX_INFLIGHT = Number(process.env.QUOTER_MAX_INFLIGHT || '200');
const BOT_DISPATCH_MODE = (
  process.env.QUOTER_BOT_DISPATCH_MODE || 'all'
).toLowerCase();
const RELAYER_RPC_TIMEOUT_MS = Number(
  process.env.QUOTER_RELAYER_RPC_TIMEOUT_MS || '5000',
);
const GROUP_RELOAD_EVERY_TICKS = Number(
  process.env.QUOTER_GROUP_RELOAD_EVERY_TICKS || '0',
);
const ACCOUNT_RELOAD_EVERY_TICKS = Number(
  process.env.QUOTER_ACCOUNT_RELOAD_EVERY_TICKS || '0',
);
const MIN_INTERVAL_MS = Number(process.env.QUOTER_MIN_INTERVAL_MS || '100');
const BOT_SIDES = (process.env.QUOTER_BOT_SIDES || 'bid,ask')
  .split(',')
  .map((v) => v.trim().toLowerCase())
  .filter((v) => v.length > 0);
const DEBUG_STARTUP = (process.env.QUOTER_DEBUG_STARTUP || 'false') === 'true';
const REPORT_PATH = process.env.QUOTER_REPORT_PATH || '';
const RELAYER_METRICS_URL =
  process.env.QUOTER_RELAYER_METRICS_URL || 'http://127.0.0.1:9093/metrics';
const MAX_TICKS = Number(process.env.QUOTER_MAX_TICKS || '0');
const MAX_RUNTIME_MS = Number(process.env.QUOTER_MAX_RUNTIME_MS || '0');
const HARNESS_URL =
  process.env.QUOTER_HARNESS_URL ||
  process.env.CONTINUUM_HARNESS_URL ||
  'http://127.0.0.1:9091';
const MIN_SOL_BALANCE_LAMPORTS = Number(
  process.env.QUOTER_MIN_SOL_BALANCE_LAMPORTS || `${0.05 * 1e9}`,
);
const STARTUP_ENSURE_FUNDED =
  (process.env.QUOTER_STARTUP_ENSURE_FUNDED || 'true') === 'true';
const BOT_ACCOUNT_NUM_START = Number(
  process.env.QUOTER_BOT_ACCOUNT_NUM_START || '100',
);
const STARTUP_DEPOSIT_UI_AMOUNT = Number(
  process.env.QUOTER_STARTUP_DEPOSIT_UI_AMOUNT || '1000',
);
const STARTUP_FUNDING_RPC_URL =
  process.env.QUOTER_STARTUP_FUNDING_RPC_URL ||
  'https://api.devnet.solana.com';

function startupDebug(msg: string): void {
  if (!DEBUG_STARTUP) {
    return;
  }
  console.error(`[quoter-startup] ${msg}`);
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

/**
 * Ensure a bot has sufficient SOL for tx fees and USDC margin deposited.
 * On startup:
 *  1. Check SOL balance — if below threshold, transfer from payer (not faucet)
 *  2. Check mango account exists on-chain — if missing, request creation via harness
 *  3. Request USDC airdrop-deposit via harness if account has no margin
 *
 * This makes bots self-bootstrapping on fresh deployments.
 */
async function ensureBotFunded(
  connection: Connection,
  botKeypair: Keypair,
  mangoAccountPk: PublicKey,
  botName: string,
  payerKeypair: Keypair | null,
  client: MangoClient,
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  usdcMintPk: PublicKey | null,
  cluster: Cluster,
  programId: PublicKey,
): Promise<void> {
  if (!STARTUP_ENSURE_FUNDED) return;

  // ── 1. SOL balance check — transfer from payer only if shortage ──────
  const solBalance = await connection.getBalance(botKeypair.publicKey);
  console.log(`[${botName}] SOL balance: ${(solBalance / 1e9).toFixed(4)} SOL`);
  if (solBalance < MIN_SOL_BALANCE_LAMPORTS) {
    if (payerKeypair && !payerKeypair.publicKey.equals(botKeypair.publicKey)) {
      const transferAmount = 0.5 * 1e9; // 0.5 SOL
      console.log(
        `[${botName}] SOL below threshold (${(MIN_SOL_BALANCE_LAMPORTS / 1e9).toFixed(3)} SOL), transferring ${transferAmount / 1e9} SOL from payer...`,
      );
      try {
        const { SystemProgram, Transaction, sendAndConfirmTransaction } =
          await import('@solana/web3.js');
        const tx = new Transaction().add(
          SystemProgram.transfer({
            fromPubkey: payerKeypair.publicKey,
            toPubkey: botKeypair.publicKey,
            lamports: transferAmount,
          }),
        );
        const sig = await sendAndConfirmTransaction(connection, tx, [payerKeypair], {
          commitment: 'confirmed',
        });
        console.log(`[${botName}] SOL transfer confirmed: ${sig}`);
      } catch (err: any) {
        console.warn(`[${botName}] SOL transfer from payer failed: ${err.message || err}`);
        // Fallback: try devnet faucet
        try {
          const devnetConn = new Connection('https://api.devnet.solana.com', 'confirmed');
          const sig = await devnetConn.requestAirdrop(botKeypair.publicKey, 1e9);
          for (let i = 0; i < 20; i++) {
            const st = await devnetConn.getSignatureStatus(sig);
            if (st?.value?.confirmationStatus === 'confirmed' || st?.value?.confirmationStatus === 'finalized') break;
            await new Promise((r) => setTimeout(r, 1000));
          }
          console.log(`[${botName}] devnet faucet airdrop confirmed: ${sig}`);
        } catch (e2: any) {
          console.warn(`[${botName}] devnet faucet also failed: ${e2.message || e2}`);
        }
      }
    } else {
      console.warn(`[${botName}] SOL below threshold but no payer keypair available for transfer`);
    }
  }

  // ── 2. Mango account existence check ─────────────────────────────────
  let mangoAccountExists = false;
  try {
    const acctInfo = await connection.getAccountInfo(mangoAccountPk);
    mangoAccountExists = acctInfo !== null && acctInfo.data.length > 0;
    if (!mangoAccountExists) {
      console.log(`[${botName}] mango account ${mangoAccountPk.toBase58()} not found on-chain`);
    } else {
      console.log(`[${botName}] mango account verified on-chain (${acctInfo!.data.length} bytes)`);
    }
  } catch (err: any) {
    console.warn(`[${botName}] mango account check failed: ${err.message || err}`);
  }

  const loadEquity = async (): Promise<number> => {
    try {
      const mangoAccount = await client.getMangoAccount(mangoAccountPk);
      return mangoAccount.getEquity(group).toNumber();
    } catch (err: any) {
      console.warn(`[${botName}] equity check failed: ${err.message || err}`);
      return 0;
    }
  };

  const directMintAndDeposit = async (): Promise<void> => {
    if (!payerKeypair || !usdcMintPk) {
      throw new Error('direct mint/deposit requires payer keypair and usdc mint');
    }
    const fundingConnection = new Connection(
      STARTUP_FUNDING_RPC_URL,
      AnchorProvider.defaultOptions(),
    );
    const fundingProvider = new AnchorProvider(
      fundingConnection,
      new Wallet(botKeypair),
      AnchorProvider.defaultOptions(),
    );
    const fundingClient = await MangoClient.connect(
      fundingProvider,
      cluster,
      programId,
      { idsSource: 'get-program-accounts' },
    );
    const fundingGroup = await fundingClient.getGroup(group.publicKey);
    const fundingAccount = await fundingClient.getMangoAccount(mangoAccountPk);
    console.log(
      `[${botName}] funding via direct mint + tokenDeposit ui_amount=${STARTUP_DEPOSIT_UI_AMOUNT}`,
    );
    const botUsdcAta = await createAssociatedTokenAccountIdempotent(
      fundingConnection,
      payerKeypair,
      usdcMintPk,
      botKeypair.publicKey,
    );
    await mintTo(
      fundingConnection,
      payerKeypair,
      usdcMintPk,
      botUsdcAta,
      payerKeypair,
      BigInt(Math.round(STARTUP_DEPOSIT_UI_AMOUNT * 1_000_000)),
    );
    await fundingClient.tokenDeposit(
      fundingGroup,
      fundingAccount,
      usdcMintPk,
      STARTUP_DEPOSIT_UI_AMOUNT,
    );
  };

  const waitForPositiveEquity = async (timeoutMs: number): Promise<number> => {
    const startedAt = Date.now();
    let lastEquity = 0;
    while (Date.now() - startedAt < timeoutMs) {
      lastEquity = await loadEquity();
      if (lastEquity > 0) {
        return lastEquity;
      }
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
    return lastEquity;
  };

  const currentEquity = await loadEquity();
  console.log(`[${botName}] mango equity before funding: ${currentEquity}`);

  // ── 3. Harness airdrop-deposit (creates account + mints USDC + deposits) ──
  // Treat funding as required if equity is still zero.
  if (!mangoAccountExists || currentEquity <= 0) {
    let funded = false;
    if (payerKeypair && usdcMintPk) {
      try {
        await directMintAndDeposit();
        funded = true;
      } catch (err: any) {
        console.warn(`[${botName}] direct mint/deposit failed, falling back to harness: ${err.message || err}`);
      }
    }
    if (!funded) {
      console.log(`[${botName}] requesting harness airdrop-deposit for ${mangoAccountPk.toBase58()}...`);
      await callHarnessAirdropDeposit(botKeypair.publicKey, botName, 30_000, mangoAccountPk);
    }
    const fundedEquity = await waitForPositiveEquity(30_000);
    if (fundedEquity <= 0) {
      throw new Error(
        `[${botName}] funding did not land on-chain for ${mangoAccountPk.toBase58()} after harness airdrop-deposit`,
      );
    }
    console.log(`[${botName}] mango equity after funding: ${fundedEquity}`);
  }
}

async function callHarnessAirdropDeposit(
  ownerPk: PublicKey,
  botName: string,
  timeoutMs = 30_000,
  mangoAccountPk?: PublicKey,
): Promise<void> {
  const url = `${HARNESS_URL}/airdrop-deposit`;
  const body = JSON.stringify({
    owner: ownerPk.toBase58(),
    ...(mangoAccountPk ? { mango_account: mangoAccountPk.toBase58() } : {}),
  });
  console.log(`[${botName}] POST ${url} (timeout=${timeoutMs}ms)`);

  for (let attempt = 1; attempt <= 2; attempt++) {
    try {
      const resp = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body,
        signal: AbortSignal.timeout(timeoutMs),
      });
      if (!resp.ok) {
        const text = await resp.text().catch(() => '');
        throw new Error(`harness returned ${resp.status}: ${text}`);
      }
      const result = await resp.json().catch(() => ({}));
      console.log(`[${botName}] airdrop-deposit result:`, JSON.stringify(result));
      return;
    } catch (err: any) {
      if (attempt < 2) {
        console.warn(`[${botName}] airdrop-deposit attempt ${attempt} failed, retrying: ${err.message || err}`);
        await new Promise((r) => setTimeout(r, 2000));
      } else {
        throw err;
      }
    }
  }
}

async function resolveOrCreateBotMangoAccount(params: {
  spec: BotSpec;
  botIndex: number;
  keypair: Keypair;
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  executionQueuePk: PublicKey;
  payerKeypair: Keypair | null;
  cluster: Cluster;
  connection: Connection;
  programId: PublicKey;
}): Promise<Awaited<ReturnType<MangoClient['getMangoAccount']>>> {
  const accountNum = BOT_ACCOUNT_NUM_START + params.botIndex;

  const loadConfiguredAccount = async () => {
    if (!params.spec.mangoAccount) {
      return null;
    }
    try {
      const account = await params.client.getMangoAccount(
        new PublicKey(params.spec.mangoAccount),
      );
      if (!account.owner.equals(params.keypair.publicKey)) {
        throw new Error('owner mismatch');
      }
      if (!account.group.equals(params.group.publicKey)) {
        throw new Error('group mismatch');
      }
      console.log(
        `[${params.spec.name}] using configured mango account ${account.publicKey.toBase58()} (account_num=${account.accountNum})`,
      );
      return account;
    } catch (err: any) {
      console.warn(
        `[${params.spec.name}] configured mango account ${params.spec.mangoAccount} unusable, falling back: ${err.message || err}`,
      );
      return null;
    }
  };

  const configured = await loadConfiguredAccount();
  if (configured) {
    await params.client.editMangoAccount(
      params.group,
      configured,
      undefined,
      params.executionQueuePk,
    );
    return configured;
  }

  const existing = await params.client.getMangoAccountForOwner(
    params.group,
    params.keypair.publicKey,
    accountNum,
  );
  if (existing) {
    console.log(
      `[${params.spec.name}] found existing mango account ${existing.publicKey.toBase58()} for account_num=${accountNum}`,
    );
    await params.client.editMangoAccount(
      params.group,
      existing,
      undefined,
      params.executionQueuePk,
    );
    return existing;
  }

  if (!params.payerKeypair) {
    throw new Error(
      `[${params.spec.name}] missing payer keypair; cannot auto-create mango account ${accountNum}`,
    );
  }

  console.log(
    `[${params.spec.name}] creating mango account for owner ${params.keypair.publicKey.toBase58()} with account_num=${accountNum}`,
  );
  const payerProvider = new AnchorProvider(
    params.connection,
    new Wallet(params.payerKeypair),
    AnchorProvider.defaultOptions(),
  );
  const payerClient = await MangoClient.connect(
    payerProvider,
    params.cluster,
    params.programId,
    { idsSource: 'get-program-accounts' },
  );
  const ix = await payerClient.program.methods
    .accountCreate(accountNum, 8, 4, 4, 32, params.spec.name.slice(0, 32))
    .accounts({
      group: params.group.publicKey,
      owner: params.keypair.publicKey,
      payer: payerClient.walletPk,
    })
    .instruction();
  await payerClient.sendAndConfirmTransactionForGroup(params.group, [ix], {
    additionalSigners: [params.keypair],
  });
  const created = await params.client.getMangoAccountForOwner(
    params.group,
    params.keypair.publicKey,
    accountNum,
  );
  if (!created) {
    throw new Error(
      `[${params.spec.name}] mango account create transaction confirmed but account not found for account_num=${accountNum}`,
    );
  }
  await params.client.editMangoAccount(
    params.group,
    created,
    undefined,
    params.executionQueuePk,
  );
  console.log(
    `[${params.spec.name}] created mango account ${created.publicKey.toBase58()} for account_num=${accountNum}`,
  );
  return created;
}

function sideFromString(input: string): PerpOrderSide {
  if (input === 'bid' || input === 'buy') {
    return PerpOrderSide.bid;
  }
  if (input === 'ask' || input === 'sell') {
    return PerpOrderSide.ask;
  }
  throw new Error(`invalid side ${input}`);
}

function sideToString(side: PerpOrderSide): 'bid' | 'ask' {
  return side === PerpOrderSide.bid ? 'bid' : 'ask';
}

function selectBotsForTick(
  bots: BotRuntime[],
  currentTick: number,
): BotRuntime[] {
  if (BOT_DISPATCH_MODE === 'all' || bots.length <= 1) {
    return bots;
  }
  if (BOT_DISPATCH_MODE === 'round-robin') {
    return [bots[(currentTick - 1) % bots.length]];
  }
  if (BOT_DISPATCH_MODE === 'random-one') {
    return [bots[Math.floor(Math.random() * bots.length)]];
  }
  throw new Error('QUOTER_BOT_DISPATCH_MODE must be all, round-robin, or random-one');
}

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
  marketIndex: PerpMarketIndex;
  userOwner: PublicKey;
}): Promise<AccountMeta[]> {
  const perpMarket = params.group.getPerpMarketByMarketIndex(params.marketIndex);
  const healthRemainingAccounts = await params.client.buildHealthRemainingAccounts(
    params.group,
    [params.mangoAccount],
    [params.group.getFirstBankForPerpSettlement()],
    [perpMarket],
  );

  return [
    { pubkey: params.group.publicKey, isSigner: false, isWritable: false },
    { pubkey: params.mangoAccount.publicKey, isSigner: false, isWritable: true },
    { pubkey: params.userOwner, isSigner: false, isWritable: false },
    { pubkey: perpMarket.publicKey, isSigner: false, isWritable: true },
    { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
    { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
    { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
    { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
    ...healthRemainingAccounts.map((pubkey) => ({
      pubkey,
      isSigner: false,
      isWritable: false,
    })),
  ];
}

function randomFloat(min: number, max: number): number {
  return min + Math.random() * (max - min);
}

function randomSizeSol(): number {
  return Number(randomFloat(SIZE_MIN_SOL, SIZE_MAX_SOL).toFixed(3));
}

function randomQuotePrice(
  referencePrice: number,
  side: PerpOrderSide,
): number {
  const offsetBps =
    side === PerpOrderSide.bid
      ? randomFloat(QUOTER_BID_MIN_BPS, QUOTER_BID_MAX_BPS)
      : randomFloat(QUOTER_ASK_MIN_BPS, QUOTER_ASK_MAX_BPS);
  const multiplier = 1 + offsetBps / 10_000;
  return Number((referencePrice * multiplier).toFixed(4));
}

function aggressiveClosePrice(
  referencePrice: number,
  side: PerpOrderSide,
): number {
  const fraction = PRICE_RANGE_BPS / 10_000;
  const multiplier =
    side === PerpOrderSide.bid ? 1 + fraction : 1 - fraction;
  return Number((referencePrice * multiplier).toFixed(4));
}

function shouldAttemptClosePosition(): boolean {
  return Math.random() < CLOSE_POSITION_PROBABILITY_BPS / 10_000;
}

function orderExpiryTimestampSec(nowMs: number): number {
  // 0 means no expiry — pass 0 to the on-chain program
  if (ORDER_EXPIRY_SECS === 0) return 0;
  return Math.floor(nowMs / 1000) + ORDER_EXPIRY_SECS;
}

function getCloseOrderPlan(params: {
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
  marketIndex: PerpMarketIndex;
  perpMarket: PerpMarket;
  referencePrice: number;
}):
  | {
      side: PerpOrderSide;
      sizeSol: number;
      quotePrice: number;
      maxQuoteQty: number;
    }
  | null {
  const position = params.mangoAccount.getPerpPosition(params.marketIndex);
  if (!position || position.basePositionLots.isZero()) {
    return null;
  }

  const basePositionUi = Math.abs(
    params.perpMarket.baseLotsToUi(position.basePositionLots),
  );
  if (!Number.isFinite(basePositionUi) || basePositionUi <= 0) {
    return null;
  }

  const side =
    position.basePositionLots.gt(new BN(0))
      ? PerpOrderSide.ask
      : PerpOrderSide.bid;
  const quotePrice = aggressiveClosePrice(params.referencePrice, side);
  const maxQuoteQty = Number(
    (quotePrice * basePositionUi * QUOTE_BUDGET_MULTIPLIER).toFixed(6),
  );

  return {
    side,
    sizeSol: basePositionUi,
    quotePrice,
    maxQuoteQty,
  };
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function parsePrometheusMetrics(text: string): Record<string, number> {
  const out: Record<string, number> = {};
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) {
      continue;
    }
    const [name, value] = trimmed.split(/\s+/, 2);
    const parsed = Number(value);
    if (!name || Number.isNaN(parsed)) {
      continue;
    }
    out[name] = parsed;
  }
  return out;
}

async function fetchRelayerMetricsSnapshot(): Promise<Record<string, number> | null> {
  try {
    const response = await fetch(RELAYER_METRICS_URL);
    if (!response.ok) {
      return null;
    }
    return parsePrometheusMetrics(await response.text());
  } catch {
    return null;
  }
}

function deriveWsEndpoint(httpUrl: string): string | null {
  try {
    const parsed = new URL(httpUrl);
    const protocol = parsed.protocol === 'https:' ? 'wss:' : 'ws:';
    let port = parsed.port;
    if (
      (parsed.hostname === '127.0.0.1' || parsed.hostname === 'localhost') &&
      parsed.port === '8899'
    ) {
      port = '8900';
    }
    const host = port.length ? `${parsed.hostname}:${port}` : parsed.hostname;
    return `${protocol}//${host}${parsed.pathname || ''}`;
  } catch {
    return null;
  }
}

async function submitIntentViaRelayer(params: {
  relayerClient: any;
  group: PublicKey;
  executionQueue: PublicKey;
  market: number;
  payload: Buffer;
  remainingAccounts: AccountMeta[];
  userOwner: PublicKey;
  userSecretKey: Uint8Array;
  mangoAccount: PublicKey;
  minExecuteSlot: bigint;
}): Promise<{ sequence: string; tx_signature: string }> {
  const intent = await buildExecutionQueueUserIntent({
    group: params.group,
    executionQueue: params.executionQueue,
    mangoAccount: params.mangoAccount,
    userOwner: params.userOwner,
    payload: params.payload,
    remainingAccounts: params.remainingAccounts,
  });
  const userSignature = signExecutionQueueIntentMessage(
    params.userSecretKey,
    intent.userIntentMessage,
  );

  return await new Promise<{ sequence: string; tx_signature: string }>(
    (resolve, reject) => {
      params.relayerClient.submitIntent(
        {
          group: params.group.toBase58(),
          execution_queue: params.executionQueue.toBase58(),
          market: new BN(params.market).toString(),
          payload: params.payload,
          remaining_accounts: params.remainingAccounts.map((a) => ({
            pubkey: a.pubkey.toBase58(),
            is_signer: !!a.isSigner,
            is_writable: !!a.isWritable,
          })),
          min_execute_slot: params.minExecuteSlot.toString(),
          expires_at_slot: '0',
          user_owner: params.userOwner.toBase58(),
          mango_account: params.mangoAccount.toBase58(),
          user_signature: Buffer.from(userSignature),
        },
        {
          deadline: Date.now() + RELAYER_RPC_TIMEOUT_MS,
        },
        (
          err: Error | null,
          res: { sequence: string; tx_signature: string },
        ) => {
          if (err) {
            reject(err);
            return;
          }
          resolve(res);
        },
      );
    },
  );
}

function loadBotSpecs(config: E2EConfig): BotSpec[] {
  if (BOTS_JSON_PATH.length || BOTS_JSON.length) {
    const raw = BOTS_JSON_PATH.length
      ? fs.readFileSync(path.resolve(BOTS_JSON_PATH), 'utf-8')
      : BOTS_JSON;
    const parsed = JSON.parse(raw) as BotSpecWire[];
    if (!Array.isArray(parsed) || !parsed.length) {
      throw new Error('QUOTER_BOTS_JSON_PATH/QUOTER_BOTS_JSON must contain a non-empty array');
    }
    return parsed.map((bot, i) => ({
      name: bot.name || `bot-${i}`,
      keypairPath: bot.keypairPath,
      mangoAccount: bot.mangoAccount,
      side: sideFromString(bot.side),
    }));
  }

  const specs: BotSpec[] = [];
  if (config.maker?.keypairPath && config.maker?.mangoAccount) {
    specs.push({
      name: 'maker-bot',
      keypairPath: config.maker.keypairPath,
      mangoAccount: config.maker.mangoAccount,
      side: sideFromString(BOT_SIDES[0] || 'bid'),
    });
  }
  if (config.taker?.keypairPath && config.taker?.mangoAccount) {
    specs.push({
      name: 'taker-bot',
      keypairPath: config.taker.keypairPath,
      mangoAccount: config.taker.mangoAccount,
      side: sideFromString(BOT_SIDES[1] || 'ask'),
    });
  }
  if (specs.length === 0) {
    throw new Error(
      'no bots found in config; expected maker/taker keypair + mangoAccount',
    );
  }
  return specs;
}

function isRelayerTransportError(errText: string): boolean {
  const msg = errText.toLowerCase();
  return (
    msg.includes('unavailable') ||
    msg.includes('channel has been shut down') ||
    msg.includes('connection dropped') ||
    msg.includes('no connection established')
  );
}

function getOnchainReferencePriceUi(params: {
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  marketIndex: PerpMarketIndex;
  solMint: PublicKey | null;
}): number {
  if (params.solMint) {
    try {
      const solBank = params.group.getFirstBankByMint(params.solMint);
      return solBank.uiPrice;
    } catch {
      // Fallback to perp oracle below.
    }
  }
  const perpMarket = params.group.getPerpMarketByMarketIndex(params.marketIndex);
  return perpMarket.uiPrice;
}

async function fetchCoinGeckoReferencePriceUi(): Promise<number> {
  const base = COINGECKO_API_BASE.endsWith('/')
    ? COINGECKO_API_BASE.slice(0, -1)
    : COINGECKO_API_BASE;
  const query = new URLSearchParams({
    ids: COINGECKO_ASSET_ID,
    vs_currencies: COINGECKO_VS_CURRENCY,
  });
  const url = `${base}/simple/price?${query.toString()}`;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), COINGECKO_TIMEOUT_MS);
  try {
    const headers: Record<string, string> = {};
    if (COINGECKO_API_KEY) {
      if (base.includes('pro-api.coingecko.com')) {
        headers['x-cg-pro-api-key'] = COINGECKO_API_KEY;
      } else {
        headers['x-cg-demo-api-key'] = COINGECKO_API_KEY;
      }
    }
    const res = await fetch(url, {
      signal: controller.signal,
      headers,
    });
    if (!res.ok) {
      throw new Error(`coingecko non-200 status ${res.status}`);
    }
    const parsed = (await res.json()) as Record<string, Record<string, number>>;
    const value = parsed?.[COINGECKO_ASSET_ID]?.[COINGECKO_VS_CURRENCY];
    if (!Number.isFinite(value) || value <= 0) {
      throw new Error('coingecko returned invalid price');
    }
    return value;
  } finally {
    clearTimeout(timer);
  }
}

async function main(): Promise<void> {
  startupDebug('main-enter');
  if (!Number.isFinite(MIN_INTERVAL_MS) || MIN_INTERVAL_MS <= 0) {
    throw new Error('QUOTER_MIN_INTERVAL_MS must be > 0');
  }
  if (INTERVAL_MS < MIN_INTERVAL_MS) {
    throw new Error(`QUOTER_INTERVAL_MS must be >= ${MIN_INTERVAL_MS}`);
  }
  if (PRICE_RANGE_BPS <= 0 || PRICE_RANGE_BPS > 1000) {
    throw new Error('QUOTER_PRICE_RANGE_BPS must be in (0, 1000]');
  }
  if (
    !Number.isFinite(QUOTER_BID_MIN_BPS) ||
    !Number.isFinite(QUOTER_BID_MAX_BPS) ||
    !Number.isFinite(QUOTER_ASK_MIN_BPS) ||
    !Number.isFinite(QUOTER_ASK_MAX_BPS)
  ) {
    throw new Error('quote band bps values must be finite');
  }
  if (QUOTER_BID_MIN_BPS >= QUOTER_BID_MAX_BPS) {
    throw new Error('QUOTER_BID_MIN_BPS must be < QUOTER_BID_MAX_BPS');
  }
  if (QUOTER_ASK_MIN_BPS >= QUOTER_ASK_MAX_BPS) {
    throw new Error('QUOTER_ASK_MIN_BPS must be < QUOTER_ASK_MAX_BPS');
  }
  if (SIZE_MIN_SOL <= 0 || SIZE_MAX_SOL < SIZE_MIN_SOL) {
    throw new Error('invalid QUOTER_SIZE_MIN_SOL / QUOTER_SIZE_MAX_SOL');
  }
  if (!Number.isInteger(ORDER_EXPIRY_SECS) || ORDER_EXPIRY_SECS < 0) {
    throw new Error('QUOTER_ORDER_EXPIRY_SECS must be an integer >= 0 (0 = no expiry)');
  }
  if (
    !Number.isFinite(CLOSE_POSITION_PROBABILITY_BPS) ||
    CLOSE_POSITION_PROBABILITY_BPS < 0 ||
    CLOSE_POSITION_PROBABILITY_BPS > 10_000
  ) {
    throw new Error(
      'QUOTER_CLOSE_POSITION_PROBABILITY_BPS must be in [0, 10000]',
    );
  }
  if (!Number.isFinite(COINGECKO_REFRESH_MS) || COINGECKO_REFRESH_MS <= 0) {
    throw new Error('QUOTER_COINGECKO_REFRESH_MS must be > 0');
  }
  if (!Number.isInteger(MAX_INFLIGHT) || MAX_INFLIGHT <= 0) {
    throw new Error('QUOTER_MAX_INFLIGHT must be an integer > 0');
  }
  if (
    BOT_DISPATCH_MODE !== 'all' &&
    BOT_DISPATCH_MODE !== 'round-robin' &&
    BOT_DISPATCH_MODE !== 'random-one'
  ) {
    throw new Error(
      'QUOTER_BOT_DISPATCH_MODE must be all, round-robin, or random-one',
    );
  }
  if (!Number.isInteger(CANCEL_EVERY_TICKS) || CANCEL_EVERY_TICKS <= 0) {
    throw new Error('QUOTER_CANCEL_EVERY_TICKS must be an integer > 0');
  }
  if (CANCEL_MODE !== 'all' && CANCEL_MODE !== 'client-id') {
    throw new Error('QUOTER_CANCEL_MODE must be all or client-id');
  }
  if (!Number.isInteger(GROUP_RELOAD_EVERY_TICKS) || GROUP_RELOAD_EVERY_TICKS < 0) {
    throw new Error('QUOTER_GROUP_RELOAD_EVERY_TICKS must be an integer >= 0');
  }
  if (
    !Number.isInteger(ACCOUNT_RELOAD_EVERY_TICKS) ||
    ACCOUNT_RELOAD_EVERY_TICKS < 0
  ) {
    throw new Error('QUOTER_ACCOUNT_RELOAD_EVERY_TICKS must be an integer >= 0');
  }

  const config = JSON.parse(
    fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8'),
  ) as E2EConfig;
  startupDebug('config-loaded');
  const cluster = config.cluster;
  const clusterUrl = CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const relayerAddr = RELAYER_ADDR_OVERRIDE || config.relayer?.bindAddr || '127.0.0.1:9090';
  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const marketIndex = config.perpMarketIndex as PerpMarketIndex;
  const solMintPk = config.solMint ? new PublicKey(config.solMint) : null;
  const usdcMintPk = config.usdcMint ? new PublicKey(config.usdcMint) : null;

  const wsEndpoint = CLUSTER_WS_URL_OVERRIDE || undefined;
  const connection = new Connection(clusterUrl, {
    ...AnchorProvider.defaultOptions(),
    commitment: COMMITMENT,
    wsEndpoint,
  });
  startupDebug(`connection-created ws=${wsEndpoint || 'default'}`);
  const botSpecs = loadBotSpecs(config);
  startupDebug(`bot-specs-loaded count=${botSpecs.length}`);

  // Load payer keypair for SOL transfers to underfunded bots
  const payerKeypairPath =
    process.env.MB_PAYER_KEYPAIR ||
    process.env.CTM_RELAYER_PAYER_KEYPAIR ||
    '';
  let payerKeypair: Keypair | null = null;
  if (payerKeypairPath) {
    try {
      payerKeypair = readKeypair(payerKeypairPath);
      startupDebug(`payer-keypair-loaded pk=${payerKeypair.publicKey.toBase58()}`);
    } catch (err: any) {
      console.warn(`failed to load payer keypair: ${err.message}`);
    }
  }

  const bots: BotRuntime[] = [];
  let sharedGroup: Awaited<ReturnType<MangoClient['getGroup']>> | null = null;
  for (const [botIndex, spec] of botSpecs.entries()) {
    // Stagger bot init to avoid RPC rate limits (429)
    if (botIndex > 0) {
      await new Promise((r) => setTimeout(r, 2000));
    }
    const keypair = readKeypair(spec.keypairPath);
    const provider = new AnchorProvider(
      connection,
      new Wallet(keypair),
      AnchorProvider.defaultOptions(),
    );
    const client = await MangoClient.connect(provider, cluster, new PublicKey(config.programId), {
      idsSource: 'get-program-accounts',
    });
    startupDebug(`client-connected bot=${spec.name}`);
    if (!sharedGroup) {
      sharedGroup = await client.getGroup(groupPk);
      startupDebug('group-loaded');
    }
    const mangoAccount = await resolveOrCreateBotMangoAccount({
      spec,
      botIndex,
      keypair,
      client,
      group: sharedGroup,
      executionQueuePk,
      payerKeypair,
      cluster,
      connection,
      programId: new PublicKey(config.programId),
    });
    const mangoAccountPk = mangoAccount.publicKey;

    // Ensure the bot has SOL for fees and USDC margin deposited
    await ensureBotFunded(
      connection,
      keypair,
      mangoAccountPk,
      spec.name,
      payerKeypair,
      client,
      sharedGroup,
      usdcMintPk,
      cluster,
      new PublicKey(config.programId),
    );

    startupDebug(`mango-account-loaded bot=${spec.name}`);
    bots.push({
      name: spec.name,
      keypair,
      mangoAccountPk,
      mangoAccount,
      side: spec.side,
      client,
      group: sharedGroup,
      lastPlacedClientOrderId: null,
    });
  }

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  startupDebug('grpc-proto-loaded');
  const buildRelayerClient = () =>
    new proto.ctmsequencer.CtmSequencerRelayer(
      relayerAddr,
      grpc.credentials.createInsecure(),
    );
  let relayerClient = buildRelayerClient();

  let running = true;
  const shutdown = () => {
    running = false;
    relayerClient.close();
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);

  console.log(
    JSON.stringify({
      msg: 'random-sol-usdc-quoter-bot started',
      clusterUrl,
      relayerAddr,
      intervalMs: INTERVAL_MS,
      priceRangeBps: PRICE_RANGE_BPS,
      bidBandBps: [QUOTER_BID_MIN_BPS, QUOTER_BID_MAX_BPS],
      askBandBps: [QUOTER_ASK_MIN_BPS, QUOTER_ASK_MAX_BPS],
      orderExpirySecs: ORDER_EXPIRY_SECS,
      closePositionProbabilityBps: CLOSE_POSITION_PROBABILITY_BPS,
      sizeMinSol: SIZE_MIN_SOL,
      sizeMaxSol: SIZE_MAX_SOL,
      referencePrice: {
        provider: 'coingecko',
        assetId: COINGECKO_ASSET_ID,
        vsCurrency: COINGECKO_VS_CURRENCY,
        apiBase: COINGECKO_API_BASE,
        refreshMs: COINGECKO_REFRESH_MS,
      },
      parallelBotExecution: PARALLEL_BOT_EXECUTION,
      botDispatchMode: BOT_DISPATCH_MODE,
      logEachOrder: LOG_EACH_ORDER,
      nonblockingSubmit: NONBLOCKING_SUBMIT,
      maxInFlight: MAX_INFLIGHT,
      groupReloadEveryTicks: GROUP_RELOAD_EVERY_TICKS,
      accountReloadEveryTicks: ACCOUNT_RELOAD_EVERY_TICKS,
      bots: bots.map((b) => ({
        name: b.name,
        owner: b.keypair.publicKey.toBase58(),
        mangoAccount: b.mangoAccountPk.toBase58(),
        side: sideToString(b.side),
      })),
    }),
  );

  let totalPlaceIntents = 0;
  let totalCancelIntents = 0;
  let ticks = 0;
  const startedAtMs = Date.now();
  let peakAvgPlaceTps = 0;
  let peakAvgIntentTps = 0;
  let peakWindowPlaceTps = 0;
  let peakWindowIntentTps = 0;
  let maxInFlightObserved = 0;
  let tickErrorCount = 0;
  let lastStatsAtMs = startedAtMs;
  let lastStatsPlaceIntents = 0;
  let lastStatsIntentCount = 0;
  let stopReason = 'signal';
  let cachedCoinGeckoPriceUi: number | null = null;
  let cachedCoinGeckoTsMs = 0;
  const inFlight = new Set<Promise<void>>();

  let backoffUntilMs = 0;

  const handleTickError = (err: unknown) => {
    tickErrorCount += 1;
    const errText = err instanceof Error ? err.message : `${err}`;
    if (isRelayerTransportError(errText)) {
      try {
        relayerClient.close();
      } catch {
        // no-op
      }
      relayerClient = buildRelayerClient();
    }
    // Only back off on actual backpressure (queue full).
    // Connection dropped / UNAVAILABLE are transient relayer issues
    // (usually from harness health gate) — retry immediately.
    if (
      errText.includes('RESOURCE_EXHAUSTED') ||
      errText.includes('backpressure')
    ) {
      backoffUntilMs = Date.now() + 3_000;
      console.error(
        JSON.stringify({
          ts: new Date().toISOString(),
          msg: 'queue backpressure, backing off 3s',
          error: errText.slice(0, 200),
        }),
      );
      return;
    }
    if (errText.includes('429') || errText.includes('Too Many')) {
      backoffUntilMs = Date.now() + 10_000;
      console.error(
        JSON.stringify({
          ts: new Date().toISOString(),
          msg: 'RPC 429, backing off 10s',
          error: errText.slice(0, 200),
        }),
      );
      return;
    }
    console.error(
      JSON.stringify({
        ts: new Date().toISOString(),
        msg: 'quote tick failed',
        error: errText,
      }),
    );
  };

  const writeReport = async () => {
    if (!REPORT_PATH) {
      return;
    }
    const elapsedMs = Date.now() - startedAtMs;
    const elapsedSec = Math.max(1, elapsedMs / 1000);
    const queueAccountInfo = await connection
      .getAccountInfo(executionQueuePk, COMMITMENT)
      .catch(() => null);
    const relayerMetrics = await fetchRelayerMetricsSnapshot();
    const report = {
      ts: new Date().toISOString(),
      stopReason,
      ticks,
      bots: bots.length,
      elapsedMs,
      totalPlaceIntents,
      totalCancelIntents,
      avgPlaceTps: totalPlaceIntents / elapsedSec,
      avgIntentTps: (totalPlaceIntents + totalCancelIntents) / elapsedSec,
      peakAvgPlaceTps,
      peakAvgIntentTps,
      peakWindowPlaceTps,
      peakWindowIntentTps,
      maxInFlightObserved,
      tickErrorCount,
      queueCount: queueAccountInfo?.data
        ? decodeExecutionQueueCount(queueAccountInfo.data)
        : null,
      relayerMetrics,
      config: {
        intervalMs: INTERVAL_MS,
        nonblockingSubmit: NONBLOCKING_SUBMIT,
        maxInFlight: MAX_INFLIGHT,
        cancelBeforePlace: CANCEL_BEFORE_PLACE,
        cancelMode: CANCEL_MODE,
        cancelEveryTicks: CANCEL_EVERY_TICKS,
        orderLimit: ORDER_LIMIT,
        botDispatchMode: BOT_DISPATCH_MODE,
        priceRangeBps: PRICE_RANGE_BPS,
        bidBandBps: [QUOTER_BID_MIN_BPS, QUOTER_BID_MAX_BPS],
        askBandBps: [QUOTER_ASK_MIN_BPS, QUOTER_ASK_MAX_BPS],
        minExecuteSlotOffset: MIN_EXECUTE_SLOT_OFFSET.toString(),
        orderExpirySecs: ORDER_EXPIRY_SECS,
        closePositionProbabilityBps: CLOSE_POSITION_PROBABILITY_BPS,
      },
    };
    const reportPath = path.resolve(REPORT_PATH);
    fs.mkdirSync(path.dirname(reportPath), { recursive: true });
    fs.writeFileSync(reportPath, JSON.stringify(report, null, 2));
    console.log(
      JSON.stringify({
        ts: new Date().toISOString(),
        msg: 'quoter-report-written',
        reportPath,
        stopReason,
        peakWindowPlaceTps,
        peakWindowIntentTps,
      }),
    );
  };

  while (running) {
    // Rate-limit backoff: skip tick if recently rate-limited
    if (backoffUntilMs > Date.now()) {
      await new Promise((r) => setTimeout(r, Math.min(1000, backoffUntilMs - Date.now())));
      continue;
    }
    if (MAX_TICKS > 0 && ticks >= MAX_TICKS) {
      stopReason = 'max_ticks';
      break;
    }
    if (MAX_RUNTIME_MS > 0 && Date.now() - startedAtMs >= MAX_RUNTIME_MS) {
      stopReason = 'max_runtime_ms';
      break;
    }
    const tickStart = Date.now();
    try {
      const currentTick = ticks + 1;
      if (
        GROUP_RELOAD_EVERY_TICKS > 0 &&
        currentTick % GROUP_RELOAD_EVERY_TICKS === 0
      ) {
        await bots[0].group.reloadAll(bots[0].client);
      }
      if (
        ACCOUNT_RELOAD_EVERY_TICKS > 0 &&
        currentTick % ACCOUNT_RELOAD_EVERY_TICKS === 0
      ) {
        await Promise.all(
          bots.map(async (bot) => {
            bot.mangoAccount = await bot.client.getMangoAccount(bot.mangoAccountPk);
          }),
        );
      }
      let referencePrice: number;
      let referenceSource: 'coingecko' | 'onchain-fallback' | 'fixed' = 'coingecko';
      if (FIXED_REFERENCE_PRICE > 0) {
        referencePrice = FIXED_REFERENCE_PRICE;
        referenceSource = 'fixed';
      } else {
      const nowMs = Date.now();
      const shouldRefreshCoinGecko =
        cachedCoinGeckoPriceUi === null ||
        nowMs - cachedCoinGeckoTsMs >= COINGECKO_REFRESH_MS;
      if (shouldRefreshCoinGecko) {
        try {
          const fetched = await fetchCoinGeckoReferencePriceUi();
          cachedCoinGeckoPriceUi = fetched;
          cachedCoinGeckoTsMs = nowMs;
          referencePrice = fetched;
        } catch (e) {
          if (cachedCoinGeckoPriceUi !== null) {
            referencePrice = cachedCoinGeckoPriceUi;
            console.warn(
              JSON.stringify({
                ts: new Date().toISOString(),
                msg: 'coingecko price fetch failed; using cached coingecko price',
                error: e instanceof Error ? e.message : `${e}`,
              }),
            );
          } else {
            referencePrice = getOnchainReferencePriceUi({
              group: bots[0].group,
              marketIndex,
              solMint: solMintPk,
            });
            referenceSource = 'onchain-fallback';
            console.warn(
              JSON.stringify({
                ts: new Date().toISOString(),
                msg: 'coingecko price fetch failed; using onchain fallback',
                error: e instanceof Error ? e.message : `${e}`,
              }),
            );
          }
        }
      } else {
        if (cachedCoinGeckoPriceUi === null) {
          referencePrice = getOnchainReferencePriceUi({
            group: bots[0].group,
            marketIndex,
            solMint: solMintPk,
          });
          referenceSource = 'onchain-fallback';
        } else {
          referencePrice = cachedCoinGeckoPriceUi;
        }
      }
      } // end FIXED_REFERENCE_PRICE else
      const minExecuteSlot =
        BigInt(await connection.getSlot(COMMITMENT)) + MIN_EXECUTE_SLOT_OFFSET;
      const botsForTick = selectBotsForTick(bots, currentTick);

      const runBotTick = async (bot: BotRuntime) => {
        const nowMs = Date.now();
        const shouldClosePosition = shouldAttemptClosePosition();
        const mangoAccount = shouldClosePosition
          ? await bot.client.getMangoAccount(bot.mangoAccountPk)
          : bot.mangoAccount;
        if (shouldClosePosition) {
          bot.mangoAccount = mangoAccount;
        }
        const perpMarket = bot.group.getPerpMarketByMarketIndex(marketIndex);
        const canonicalRemainingAccounts =
          await executionQueueCanonicalPerpRemainingAccounts({
            client: bot.client,
            group: bot.group,
            mangoAccount,
            marketIndex,
            userOwner: bot.keypair.publicKey,
          });

        const shouldCancelThisTick =
          CANCEL_BEFORE_PLACE && currentTick % CANCEL_EVERY_TICKS === 0;
        if (shouldCancelThisTick) {
          if (CANCEL_MODE === 'client-id') {
            if (bot.lastPlacedClientOrderId !== null) {
              const cancelPayload = encodePerpCancelOrderByClientOrderIdQueuePayload({
                clientOrderId: BigInt(bot.lastPlacedClientOrderId),
              });
              await submitIntentViaRelayer({
                relayerClient,
                group: bot.group.publicKey,
                executionQueue: executionQueuePk,
                market: marketIndex,
                payload: cancelPayload,
                remainingAccounts: canonicalRemainingAccounts,
                userOwner: bot.keypair.publicKey,
                userSecretKey: bot.keypair.secretKey,
                mangoAccount: mangoAccount.publicKey,
                minExecuteSlot,
              });
              totalCancelIntents += 1;
            }
          } else {
            const cancelPayload = encodePerpCancelAllOrdersQueuePayload({
              limit: CANCEL_LIMIT,
            });
            await submitIntentViaRelayer({
              relayerClient,
              group: bot.group.publicKey,
              executionQueue: executionQueuePk,
              market: marketIndex,
              payload: cancelPayload,
              remainingAccounts: canonicalRemainingAccounts,
              userOwner: bot.keypair.publicKey,
              userSecretKey: bot.keypair.secretKey,
              mangoAccount: mangoAccount.publicKey,
              minExecuteSlot,
            });
            totalCancelIntents += 1;
          }
        }

        const closePlan = shouldClosePosition
          ? getCloseOrderPlan({
              mangoAccount,
              marketIndex,
              perpMarket,
              referencePrice,
            })
          : null;
        const action = closePlan ? 'close-position' : 'quote';
        const side = closePlan ? closePlan.side : bot.side;
        const sizeSol = closePlan ? closePlan.sizeSol : randomSizeSol();
        const quotePrice = closePlan
          ? closePlan.quotePrice
          : randomQuotePrice(referencePrice, side);
        const maxQuoteQty = closePlan
          ? closePlan.maxQuoteQty
          : Number((quotePrice * sizeSol * QUOTE_BUDGET_MULTIPLIER).toFixed(6));
        const clientOrderId = Date.now() * 1000 + Math.floor(Math.random() * 1000);
        // Record the intended client id before submit so pipelined ticks can issue
        // a targeted cancel on the next cycle without waiting for the place RPC to return.
        bot.lastPlacedClientOrderId = clientOrderId;

        const payload = encodePerpPlaceOrderV2QueuePayload({
          side,
          priceLots: BigInt(perpMarket.uiPriceToLots(quotePrice).toString()),
          maxBaseLots: BigInt(perpMarket.uiBaseToLots(sizeSol).toString()),
          maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(maxQuoteQty).toString()),
          clientOrderId,
          orderType: closePlan
            ? PerpOrderType.immediateOrCancel
            : USE_LIMIT_ORDER
              ? PerpOrderType.limit
              : PerpOrderType.postOnlySlide,
          selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
          reduceOnly: !!closePlan,
          expiryTimestamp: orderExpiryTimestampSec(nowMs),
          limit: ORDER_LIMIT,
        });

        const resp = await submitIntentViaRelayer({
          relayerClient,
          group: bot.group.publicKey,
          executionQueue: executionQueuePk,
          market: marketIndex,
          payload,
          remainingAccounts: canonicalRemainingAccounts,
          userOwner: bot.keypair.publicKey,
          userSecretKey: bot.keypair.secretKey,
          mangoAccount: mangoAccount.publicKey,
          minExecuteSlot,
        });
        totalPlaceIntents += 1;

        if (LOG_EACH_ORDER) {
          console.log(
            JSON.stringify({
              ts: new Date().toISOString(),
              bot: bot.name,
              action,
              side: sideToString(side),
              referencePrice,
              referenceSource,
              quotePrice,
              sizeSol,
              maxQuoteQty,
              sequence: resp.sequence,
              txSignature: resp.tx_signature,
            }),
          );
        }
      };

      if (NONBLOCKING_SUBMIT) {
        if (PARALLEL_BOT_EXECUTION) {
          for (const bot of botsForTick) {
            while (inFlight.size >= MAX_INFLIGHT) {
              await Promise.race(inFlight);
            }
            const promise = runBotTick(bot).catch((err) => {
              handleTickError(err);
            });
            inFlight.add(promise);
            void promise.finally(() => {
              inFlight.delete(promise);
            });
          }
        } else {
          while (inFlight.size >= MAX_INFLIGHT) {
            await Promise.race(inFlight);
          }
          const sequence = (async () => {
            for (const bot of botsForTick) {
              await runBotTick(bot);
            }
          })().catch((err) => {
            handleTickError(err);
          });
          inFlight.add(sequence);
          void sequence.finally(() => {
            inFlight.delete(sequence);
          });
        }
      } else if (PARALLEL_BOT_EXECUTION) {
        await Promise.all(botsForTick.map((bot) => runBotTick(bot)));
      } else {
        for (const bot of botsForTick) {
          await runBotTick(bot);
        }
      }

      ticks += 1;
      maxInFlightObserved = Math.max(maxInFlightObserved, inFlight.size);
      if (LOG_TPS_EVERY_TICKS > 0 && ticks % LOG_TPS_EVERY_TICKS === 0) {
        const nowMs = Date.now();
        const elapsedSec = Math.max(1, (nowMs - startedAtMs) / 1000);
        const avgPlaceTps = totalPlaceIntents / elapsedSec;
        const avgIntentTps = (totalPlaceIntents + totalCancelIntents) / elapsedSec;
        const totalIntentCount = totalPlaceIntents + totalCancelIntents;
        const windowElapsedSec = Math.max(1, (nowMs - lastStatsAtMs) / 1000);
        const windowPlaceTps =
          (totalPlaceIntents - lastStatsPlaceIntents) / windowElapsedSec;
        const windowIntentTps =
          (totalIntentCount - lastStatsIntentCount) / windowElapsedSec;
        peakAvgPlaceTps = Math.max(peakAvgPlaceTps, avgPlaceTps);
        peakAvgIntentTps = Math.max(peakAvgIntentTps, avgIntentTps);
        peakWindowPlaceTps = Math.max(peakWindowPlaceTps, windowPlaceTps);
        peakWindowIntentTps = Math.max(peakWindowIntentTps, windowIntentTps);
        lastStatsAtMs = nowMs;
        lastStatsPlaceIntents = totalPlaceIntents;
        lastStatsIntentCount = totalIntentCount;
        console.log(
          JSON.stringify({
            ts: new Date().toISOString(),
            msg: 'quoter-stats',
            ticks,
            bots: bots.length,
            totalPlaceIntents,
            totalCancelIntents,
            avgPlaceTps,
            avgIntentTps,
            windowPlaceTps,
            windowIntentTps,
            peakAvgPlaceTps,
            peakAvgIntentTps,
            peakWindowPlaceTps,
            peakWindowIntentTps,
            cancelBeforePlace: CANCEL_BEFORE_PLACE,
            cancelEveryTicks: CANCEL_EVERY_TICKS,
            inFlight: inFlight.size,
          }),
        );
      }
    } catch (err) {
      handleTickError(err);
    }

    const elapsed = Date.now() - tickStart;
    const waitMs = Math.max(0, INTERVAL_MS - elapsed);
    await sleep(waitMs);
  }

  if (inFlight.size > 0) {
    await Promise.allSettled(Array.from(inFlight));
  }
  await writeReport();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
