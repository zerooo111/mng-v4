import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  Cluster,
} from '@solana/web3.js';
import { createHash } from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import os from 'os';
import path from 'path';
import {
  findExecutionQueueAuthorityStatePda,
  findPerpMarketQueueRootV3Pda,
} from '../../src/executionQueue';

dotenv.config();

const CLUSTER: Cluster = (process.env.CLUSTER_OVERRIDE as Cluster) || 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const PROGRAM_ID = process.env.CTM_RELAYER_PROGRAM_ID;
const GROUP_PK = process.env.EXECUTION_QUEUE_GROUP_PK;
const MARKET_INDEX = Number(process.env.PERP_MARKET_INDEX || '0');
const SHARD_ID = Number(process.env.EXECUTION_QUEUE_V3_SHARD_ID || '0');
const PAGE_SIZE = Number(process.env.EXECUTION_QUEUE_V3_PAGE_SIZE || '128');
const NUM_PAGES = Number(process.env.EXECUTION_QUEUE_V3_NUM_PAGES || '16');
const SOFT_LIMIT = Number(process.env.EXECUTION_QUEUE_V3_SOFT_LIMIT || '256');
const RECIPE_VERSION = Number(process.env.EXECUTION_QUEUE_V3_RECIPE_VERSION || '1');
const GAP_WAIT_SLOTS = Number(process.env.EXECUTION_QUEUE_V3_GAP_WAIT_SLOTS || '4');
const MAX_COMPACTION_DISTANCE = Number(
  process.env.EXECUTION_QUEUE_V3_MAX_COMPACTION_DISTANCE || '16',
);
const MIN_EXPIRY_BUFFER_SLOTS = Number(
  process.env.EXECUTION_QUEUE_V3_MIN_EXPIRY_BUFFER_SLOTS || '4',
);
const PAYER_KEYPAIR =
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  process.env.MB_PAYER_KEYPAIR ||
  path.join(os.homedir(), '.config/solana/id.json');
const CTM_KEYPAIR = process.env.CTM_RELAYER_CTM_KEYPAIR || PAYER_KEYPAIR;

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function anchorInstructionDiscriminator(ixName: string): Buffer {
  return createHash('sha256')
    .update(`global:${ixName}`)
    .digest()
    .subarray(0, 8);
}

function u8(value: number): Buffer {
  return Buffer.from([value & 0xff]);
}

function u16ToLe(value: number): Buffer {
  const buf = Buffer.alloc(2);
  buf.writeUInt16LE(value, 0);
  return buf;
}

async function main(): Promise<void> {
  if (!CLUSTER_URL || !PROGRAM_ID || !GROUP_PK) {
    throw new Error(
      'CLUSTER_URL_OVERRIDE/MB_CLUSTER_URL, CTM_RELAYER_PROGRAM_ID, and EXECUTION_QUEUE_GROUP_PK are required',
    );
  }

  const payer = readKeypair(PAYER_KEYPAIR);
  const ctm = readKeypair(CTM_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, AnchorProvider.defaultOptions());
  const provider = new AnchorProvider(
    connection,
    new Wallet(payer),
    AnchorProvider.defaultOptions(),
  );
  const programId = new PublicKey(PROGRAM_ID);
  const group = new PublicKey(GROUP_PK);
  const authorityState = findExecutionQueueAuthorityStatePda(programId, group);
  const queueRoot = findPerpMarketQueueRootV3Pda(programId, group, MARKET_INDEX, SHARD_ID);

  const instructions: TransactionInstruction[] = [];

  const authorityInfo = await connection.getAccountInfo(authorityState);
  if (!authorityInfo) {
    instructions.push(
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: group, isSigner: false, isWritable: true },
          { pubkey: authorityState, isSigner: false, isWritable: true },
          { pubkey: payer.publicKey, isSigner: true, isWritable: true },
          { pubkey: payer.publicKey, isSigner: true, isWritable: false },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: Buffer.concat([
          anchorInstructionDiscriminator('execution_queue_v3_init_authority_state'),
          ctm.publicKey.toBuffer(),
        ]),
      }),
    );
  }

  const queueRootInfo = await connection.getAccountInfo(queueRoot);
  if (!queueRootInfo) {
    instructions.push(
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: group, isSigner: false, isWritable: true },
          { pubkey: authorityState, isSigner: false, isWritable: true },
          { pubkey: queueRoot, isSigner: false, isWritable: true },
          { pubkey: payer.publicKey, isSigner: true, isWritable: true },
          { pubkey: payer.publicKey, isSigner: true, isWritable: false },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: Buffer.concat([
          anchorInstructionDiscriminator('execution_queue_v3_init_market_root'),
          u16ToLe(MARKET_INDEX),
          u8(SHARD_ID),
          u16ToLe(PAGE_SIZE),
          u16ToLe(NUM_PAGES),
          u16ToLe(SOFT_LIMIT),
          u16ToLe(RECIPE_VERSION),
          u16ToLe(GAP_WAIT_SLOTS),
          u8(MAX_COMPACTION_DISTANCE),
          u8(MIN_EXPIRY_BUFFER_SLOTS),
        ]),
      }),
    );
  }

  if (instructions.length > 0) {
    const tx = new Transaction().add(...instructions);
    const sig = await provider.sendAndConfirm(tx, [payer], {
      skipPreflight: false,
    });
    console.log(
      JSON.stringify(
        {
          cluster: CLUSTER,
          clusterUrl: CLUSTER_URL,
          programId: programId.toBase58(),
          group: group.toBase58(),
          authorityState: authorityState.toBase58(),
          queueRoot: queueRoot.toBase58(),
          marketIndex: MARKET_INDEX,
          shardId: SHARD_ID,
          signature: sig,
          createdAuthorityState: !authorityInfo,
          createdQueueRoot: !queueRootInfo,
        },
        null,
        2,
      ),
    );
    return;
  }

  console.log(
    JSON.stringify(
      {
        cluster: CLUSTER,
        clusterUrl: CLUSTER_URL,
        programId: programId.toBase58(),
        group: group.toBase58(),
        authorityState: authorityState.toBase58(),
        queueRoot: queueRoot.toBase58(),
        marketIndex: MARKET_INDEX,
        shardId: SHARD_ID,
        createdAuthorityState: false,
        createdQueueRoot: false,
        status: 'already_initialized',
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
