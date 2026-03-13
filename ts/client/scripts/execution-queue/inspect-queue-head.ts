import { Connection, PublicKey } from '@solana/web3.js';
import { decodeQueuePayload } from '../../src/continuumHarness';
import {
  decodeExecutionQueueHeadItem,
  decodeExecutionQueueHeader,
} from '../../src/executionQueueLayout';

const RPC_URL = process.env.CLUSTER_URL_OVERRIDE || 'http://127.0.0.1:8899';
const QUEUE_PK =
  process.env.EXECUTION_QUEUE_PK || 'HfaFVCt5FnLQfLETidHYopgQ2RqpW5JhR66yfQdtYrFP';

function json(value: unknown): string {
  return JSON.stringify(value, (_k, v) => (typeof v === 'bigint' ? v.toString() : v));
}

async function main(): Promise<void> {
  const conn = new Connection(RPC_URL, 'confirmed');
  const qPk = new PublicKey(QUEUE_PK);
  const qi = await conn.getAccountInfo(qPk, 'confirmed');
  if (!qi) {
    throw new Error('missing queue account');
  }

  const qd = qi.data;
  const header = decodeExecutionQueueHeader(qd);
  console.log(
    json({
      msg: 'queue',
      capacity: header.capacity,
      head: header.head,
      count: header.count,
      next: header.nextSequence,
      max: header.maxSeenSequence,
    }),
  );

  const headItem = decodeExecutionQueueHeadItem(qd);
  if (!headItem) {
    console.log(json({ msg: 'head-not-found', next: header.nextSequence }));
    return;
  }

  let decoded: unknown = null;
  try {
    decoded = decodeQueuePayload(headItem.payload);
  } catch {
    decoded = null;
  }

  console.log(
    json({
      msg: 'head',
      logicalIdx: headItem.logicalIndex,
      physicalIdx: headItem.physicalIndex,
      sequence: headItem.sequence,
      kind: headItem.kind,
      status: headItem.status,
      minExecuteSlot: headItem.minExecuteSlot,
      retries: headItem.retries,
      payloadLen: headItem.payloadLen,
      payloadHash: headItem.payloadHash.toString('hex'),
      accountsHash: headItem.accountsHash.toString('hex'),
      decoded,
    }),
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
