import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import { AccountMeta, Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';
import { PerpMarketIndex } from '../../src/accounts/perp';
import {
  buildExecutionQueueUserIntent,
  encodePerpCancelAllOrdersQueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { defaultClusterUrl } from './scriptEnv';

dotenv.config();

type BootstrapUser = {
  keypairPath: string;
  owner: string;
  mangoAccount: string;
};

type BootstrapConfig = {
  clusterUrl?: string;
  group?: string;
  perpMarketIndex?: number;
  maker?: BootstrapUser;
  taker?: BootstrapUser;
};

type BurstActor = {
  label: string;
  user: Keypair;
  userOwner: PublicKey;
  mangoAccountPk: PublicKey;
  remainingAccounts: AccountMeta[];
};

const CLUSTER: Cluster = (process.env.CLUSTER_OVERRIDE as Cluster) || 'devnet';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL || defaultClusterUrl();
const RELAYER_ADDR = process.env.CTM_RELAYER_ADDR || '127.0.0.1:9090';
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;
const CONFIG_PATH =
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  process.env.BOOTSTRAP_CONFIG_PATH ||
  '';
const GROUP_PK = process.env.EXECUTION_QUEUE_GROUP_PK || '';
const EXECUTION_QUEUE_PK = process.env.EXECUTION_QUEUE_PK || '';
const PERP_MARKET_INDEX = Number(process.env.PERP_MARKET_INDEX ?? '0');
const MIN_EXECUTE_SLOT = process.env.MIN_EXECUTE_SLOT || '0';
const EXPIRES_AT_SLOT = process.env.EXPIRES_AT_SLOT || '0';
const LIMIT = Number(process.env.PERP_CANCEL_ALL_LIMIT || '10');
const BURST_COUNT = Number(process.env.BURST_COUNT || '200');
const BURST_CONCURRENCY = Number(process.env.BURST_CONCURRENCY || '16');
const BURST_RETRY_LIMIT = Number(process.env.BURST_RETRY_LIMIT || '20');
const BURST_RETRY_DELAY_MS = Number(process.env.BURST_RETRY_DELAY_MS || '500');
const MAKER_KEYPAIR_OVERRIDE = process.env.E2E_MAKER_KEYPAIR_PATH || '';
const MAKER_MANGO_ACCOUNT_OVERRIDE = process.env.E2E_MAKER_MANGO_ACCOUNT || '';
const TAKER_KEYPAIR_OVERRIDE = process.env.E2E_TAKER_KEYPAIR_PATH || '';
const TAKER_MANGO_ACCOUNT_OVERRIDE = process.env.E2E_TAKER_MANGO_ACCOUNT || '';

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function loadBootstrapConfig(): BootstrapConfig | null {
  if (!CONFIG_PATH) {
    return null;
  }
  const resolved = path.resolve(CONFIG_PATH);
  if (!fs.existsSync(resolved)) {
    return null;
  }
  return JSON.parse(fs.readFileSync(resolved, 'utf-8')) as BootstrapConfig;
}

function requirePubkey(raw: string, label: string): PublicKey {
  if (!raw) {
    throw new Error(`${label} is required`);
  }
  return new PublicKey(raw);
}

async function executionQueueCanonicalPerpRemainingAccounts(params: {
  client: MangoClient;
  group: Awaited<ReturnType<MangoClient['getGroup']>>;
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>;
  marketIndex: number;
  userOwner: PublicKey;
}): Promise<AccountMeta[]> {
  const perpMarket = params.group.getPerpMarketByMarketIndex(params.marketIndex as PerpMarketIndex);
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

async function prepareActor(params: {
  label: string;
  connection: Connection;
  programId: PublicKey;
  keypairPath: string;
  mangoAccountPk: PublicKey;
  marketIndex: number;
}): Promise<BurstActor> {
  const user = readKeypair(params.keypairPath);
  const provider = new AnchorProvider(
    params.connection,
    new Wallet(user),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(provider, CLUSTER, params.programId, {
    idsSource: 'get-program-accounts',
  });
  const mangoAccount = await client.getMangoAccount(params.mangoAccountPk);
  const group = await client.getGroup(mangoAccount.group);
  const remainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
    client,
    group,
    mangoAccount,
    marketIndex: params.marketIndex,
    userOwner: user.publicKey,
  });
  return {
    label: params.label,
    user,
    userOwner: user.publicKey,
    mangoAccountPk: mangoAccount.publicKey,
    remainingAccounts,
  };
}

function submitIntent(
  relayerClient: any,
  request: Record<string, unknown>,
): Promise<{ sequence: string; tx_signature: string }> {
  return new Promise((resolve, reject) => {
    relayerClient.submitIntent(request, (err: Error | null, res: any) => {
      if (err) {
        reject(err);
        return;
      }
      resolve(res);
    });
  });
}

async function main(): Promise<void> {
  const bootstrap = loadBootstrapConfig();
  const groupPk = requirePubkey(GROUP_PK || bootstrap?.group || '', 'EXECUTION_QUEUE_GROUP_PK');
  const executionQueuePk = requirePubkey(
    EXECUTION_QUEUE_PK,
    'EXECUTION_QUEUE_PK',
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];
  const marketIndex = PERP_MARKET_INDEX || bootstrap?.perpMarketIndex || 0;

  const makerKeypairPath = MAKER_KEYPAIR_OVERRIDE || bootstrap?.maker?.keypairPath || '';
  const makerMangoAccount = requirePubkey(
    MAKER_MANGO_ACCOUNT_OVERRIDE || bootstrap?.maker?.mangoAccount || '',
    'maker mango account',
  );
  const takerKeypairPath = TAKER_KEYPAIR_OVERRIDE || bootstrap?.taker?.keypairPath || '';
  const takerMangoAccount = requirePubkey(
    TAKER_MANGO_ACCOUNT_OVERRIDE || bootstrap?.taker?.mangoAccount || '',
    'taker mango account',
  );

  if (!makerKeypairPath || !takerKeypairPath) {
    throw new Error('maker/taker keypair paths are required');
  }

  const connection = new Connection(CLUSTER_URL, AnchorProvider.defaultOptions());
  const [maker, taker] = await Promise.all([
    prepareActor({
      label: 'maker',
      connection,
      programId,
      keypairPath: makerKeypairPath,
      mangoAccountPk: makerMangoAccount,
      marketIndex,
    }),
    prepareActor({
      label: 'taker',
      connection,
      programId,
      keypairPath: takerKeypairPath,
      mangoAccountPk: takerMangoAccount,
      marketIndex,
    }),
  ]);
  const actors = [maker, taker];

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  const relayerClient = new proto.ctmsequencer.CtmSequencerRelayer(
    RELAYER_ADDR,
    grpc.credentials.createInsecure(),
  );

  const payload = encodePerpCancelAllOrdersQueuePayload({ limit: LIMIT });
  const startedAt = Date.now();
  const accepted: Array<{ actor: string; sequence: bigint; txSignature: string }> = [];
  const failures: Array<{ index: number; actor: string; error: string }> = [];
  let nextIndex = 0;

  const worker = async (): Promise<void> => {
    while (true) {
      const index = nextIndex;
      nextIndex += 1;
      if (index >= BURST_COUNT) {
        return;
      }
      const actor = actors[index % actors.length];
      let lastError: unknown = null;
      for (let attempt = 0; attempt < BURST_RETRY_LIMIT; attempt += 1) {
        try {
          const intent = await buildExecutionQueueUserIntent({
            group: groupPk,
            executionQueue: executionQueuePk,
            mangoAccount: actor.mangoAccountPk,
            userOwner: actor.userOwner,
            payload,
            target: { kind: 0, index: marketIndex },
            remainingAccounts: actor.remainingAccounts,
          });
          const userSignature = signExecutionQueueIntentMessage(
            actor.user.secretKey,
            intent.userIntentMessage,
          );
          const response = await submitIntent(relayerClient, {
            group: groupPk.toBase58(),
            execution_queue: executionQueuePk.toBase58(),
            market: new BN(marketIndex).toString(),
            payload,
            remaining_accounts: actor.remainingAccounts.map((a) => ({
              pubkey: a.pubkey.toBase58(),
              is_signer: !!a.isSigner,
              is_writable: !!a.isWritable,
            })),
            min_execute_slot: MIN_EXECUTE_SLOT,
            expires_at_slot: EXPIRES_AT_SLOT,
            user_owner: actor.userOwner.toBase58(),
            mango_account: actor.mangoAccountPk.toBase58(),
            user_signature: Buffer.from(userSignature),
            intent_version: 2,
            target_kind: 0,
            target_index: marketIndex,
          });
          accepted.push({
            actor: actor.label,
            sequence: BigInt(response.sequence),
            txSignature: response.tx_signature,
          });
          lastError = null;
          break;
        } catch (err) {
          lastError = err;
          if (attempt + 1 < BURST_RETRY_LIMIT) {
            await sleep(BURST_RETRY_DELAY_MS);
          }
        }
      }
      if (lastError) {
        failures.push({
          index,
          actor: actor.label,
          error: String(lastError),
        });
      }
    }
  };

  const workers = Array.from(
    { length: Math.max(1, BURST_CONCURRENCY) },
    () => worker(),
  );
  await Promise.all(workers);
  relayerClient.close();

  const elapsedMs = Date.now() - startedAt;
  const acceptedSequences = accepted.map((entry) => entry.sequence).sort((a, b) =>
    a < b ? -1 : a > b ? 1 : 0,
  );

  console.log(
    JSON.stringify(
      {
        cluster: CLUSTER,
        clusterUrl: CLUSTER_URL,
        relayerAddr: RELAYER_ADDR,
        group: groupPk.toBase58(),
        executionQueue: executionQueuePk.toBase58(),
        marketIndex,
        payloadVariant: 'PerpCancelAllOrders',
        countRequested: BURST_COUNT,
        concurrency: BURST_CONCURRENCY,
        retryLimit: BURST_RETRY_LIMIT,
        acceptedCount: accepted.length,
        failedCount: failures.length,
        elapsedMs,
        acceptedPerSecond:
          elapsedMs > 0 ? Number(((accepted.length * 1000) / elapsedMs).toFixed(2)) : 0,
        firstSequence:
          acceptedSequences.length > 0 ? acceptedSequences[0].toString() : null,
        lastSequence:
          acceptedSequences.length > 0
            ? acceptedSequences[acceptedSequences.length - 1].toString()
            : null,
        failures,
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
