import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { AccountMeta, Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { Group } from '../../src/accounts/group';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const CRANKER_KEYPAIR =
  process.env.EXECUTION_QUEUE_CRANKER_KEYPAIR ||
  process.env.USER_KEYPAIR_OVERRIDE ||
  process.env.MB_PAYER_KEYPAIR;
const GROUP_PK = process.env.EXECUTION_QUEUE_GROUP_PK;
const EXECUTION_QUEUE_PK = process.env.EXECUTION_QUEUE_PK;
const CRANK_INTERVAL_MS = Number(
  process.env.EXECUTION_QUEUE_CRANK_INTERVAL_MS ?? '1500',
);
const CRANK_MAX_ITEMS = Number(process.env.EXECUTION_QUEUE_CRANK_MAX_ITEMS ?? '4');
const CRANK_PRIORITIZATION_FEE = Number(
  process.env.EXECUTION_QUEUE_CRANK_PRIORITIZATION_FEE ?? '0',
);
const CRANK_LANES_JSON = process.env.EXECUTION_QUEUE_CRANK_LANES_JSON || '[]';
const CRANK_LANES_JSON_PATH = process.env.EXECUTION_QUEUE_CRANK_LANES_JSON_PATH || '';
const PROGRAM_ID_OVERRIDE = process.env.EXECUTION_QUEUE_PROGRAM_ID;

type LaneConfig = {
  name?: string;
  remainingAccounts: Array<{
    pubkey: string;
    isWritable: boolean;
    isSigner?: boolean;
  }>;
};

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parseLaneConfigs(): LaneConfig[] {
  const raw = CRANK_LANES_JSON_PATH
    ? fs.readFileSync(path.resolve(CRANK_LANES_JSON_PATH), 'utf-8')
    : CRANK_LANES_JSON;
  return JSON.parse(raw) as LaneConfig[];
}

function toAccountMetas(config: LaneConfig): AccountMeta[] {
  return config.remainingAccounts.map((a) => ({
    pubkey: new PublicKey(a.pubkey),
    isWritable: !!a.isWritable,
    isSigner: !!a.isSigner,
  }));
}

function decodeQueueCount(data: Buffer): number {
  if (data.length < 160) {
    return 0;
  }
  return data.readUInt32LE(156);
}

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }
  if (!GROUP_PK || !EXECUTION_QUEUE_PK) {
    throw new Error('EXECUTION_QUEUE_GROUP_PK and EXECUTION_QUEUE_PK are required');
  }
  if (!CRANKER_KEYPAIR) {
    throw new Error('EXECUTION_QUEUE_CRANKER_KEYPAIR (or MB_PAYER_KEYPAIR) is required');
  }

  const lanes = parseLaneConfigs();
  if (!lanes.length) {
    throw new Error(
      'At least one lane is required in EXECUTION_QUEUE_CRANK_LANES_JSON or *_JSON_PATH',
    );
  }

  const connection = new Connection(CLUSTER_URL, AnchorProvider.defaultOptions());
  const cranker = readKeypair(CRANKER_KEYPAIR);
  const provider = new AnchorProvider(
    connection,
    new Wallet(cranker),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(
    provider,
    CLUSTER,
    PROGRAM_ID_OVERRIDE
      ? new PublicKey(PROGRAM_ID_OVERRIDE)
      : MANGO_V4_ID[CLUSTER],
    { idsSource: 'get-program-accounts' },
  );
  const group: Group = await client.getGroup(new PublicKey(GROUP_PK));
  const executionQueue = new PublicKey(EXECUTION_QUEUE_PK);

  console.log(
    `Execution queue cranker started, queue=${executionQueue.toBase58()}, lanes=${lanes.length}`,
  );

  // eslint-disable-next-line no-constant-condition
  while (true) {
    try {
      const queueAccountInfo = await client.connection.getAccountInfo(executionQueue);
      if (!queueAccountInfo?.data || decodeQueueCount(queueAccountInfo.data) === 0) {
        await new Promise((resolve) => setTimeout(resolve, CRANK_INTERVAL_MS));
        continue;
      }

      for (const lane of lanes) {
        const laneName = lane.name ?? 'lane';
        try {
          const status = await client.executionQueueExecute(
            group,
            executionQueue,
            toAccountMetas(lane),
            CRANK_MAX_ITEMS,
            { prioritizationFee: CRANK_PRIORITIZATION_FEE },
          );
          console.log(
            `[${laneName}] execute sent: https://explorer.solana.com/tx/${status.signature}`,
          );
        } catch (err) {
          console.error(`[${laneName}] execute failed:`, err);
        }
      }
    } catch (err) {
      console.error('queue cranker loop error:', err);
    }

    await new Promise((resolve) => setTimeout(resolve, CRANK_INTERVAL_MS));
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
