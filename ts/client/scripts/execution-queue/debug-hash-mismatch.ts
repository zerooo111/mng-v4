/**
 * debug-hash-mismatch.ts — Compare the hash stored in a queue item against
 * what the engine would compute for each lane, byte by byte.
 */
import { Connection, PublicKey, SYSVAR_INSTRUCTIONS_PUBKEY } from '@solana/web3.js';
import { createHash } from 'crypto';
import fs from 'fs';
import {
  decodeExecutionQueueHeader,
  decodeExecutionQueueHeadItem,
} from '../../src/executionQueueLayout';

const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || '';
const EXECUTION_QUEUE_PK = process.env.EXECUTION_QUEUE_PK || '';
const LANE_CONFIG_PATH = process.env.EXECUTION_QUEUE_CRANK_LANES_JSON_PATH || '';
const GROUP_PK = process.env.EXECUTION_QUEUE_GROUP_PK || '';

interface LaneAccount {
  pubkey: string;
  isWritable: boolean;
  isSigner: boolean;
}
interface Lane {
  name: string;
  remainingAccounts: LaneAccount[];
}

function hashAccounts(accounts: { pubkey: Buffer; isSigner: boolean; isWritable: boolean }[]): Buffer {
  const bytes = Buffer.alloc(accounts.length * 34);
  let offset = 0;
  for (const a of accounts) {
    a.pubkey.copy(bytes, offset);
    offset += 32;
    bytes[offset] = a.isSigner ? 1 : 0;
    offset += 1;
    bytes[offset] = a.isWritable ? 1 : 0;
    offset += 1;
  }
  return createHash('sha256').update(bytes).digest();
}

function mergeEffectiveRuntimeFlags(
  remaining: { pubkey: string; isSigner: boolean; isWritable: boolean }[],
  fixed: { pubkey: string; isSigner: boolean; isWritable: boolean }[],
): { pubkey: string; isSigner: boolean; isWritable: boolean }[] {
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const a of [...fixed, ...remaining]) {
    const existing = merged.get(a.pubkey);
    if (existing) {
      existing.isSigner = existing.isSigner || a.isSigner;
      existing.isWritable = existing.isWritable || a.isWritable;
    } else {
      merged.set(a.pubkey, { isSigner: a.isSigner, isWritable: a.isWritable });
    }
  }
  return remaining.map((a) => ({
    pubkey: a.pubkey,
    ...merged.get(a.pubkey)!,
  }));
}

async function main() {
  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const queuePk = new PublicKey(EXECUTION_QUEUE_PK);
  const groupPk = new PublicKey(GROUP_PK);

  const qi = await connection.getAccountInfo(queuePk, 'confirmed');
  if (!qi) throw new Error('queue account not found');

  const headItem = decodeExecutionQueueHeadItem(qi.data);
  if (!headItem) {
    console.log('No head item');
    return;
  }

  const storedHash = headItem.accountsHash.toString('hex');
  console.log(`Head sequence=${headItem.sequence} storedHash=${storedHash}`);

  const lanes: Lane[] = JSON.parse(fs.readFileSync(LANE_CONFIG_PATH, 'utf-8'));

  // Fixed accounts for enqueue context
  const enqueueFixed = [
    { pubkey: groupPk.toBase58(), isSigner: false, isWritable: true },
    { pubkey: queuePk.toBase58(), isSigner: false, isWritable: true },
    { pubkey: SYSVAR_INSTRUCTIONS_PUBKEY.toBase58(), isSigner: false, isWritable: false },
  ];

  // Fixed accounts for execute context (no sysvar)
  const executeFixed = [
    { pubkey: groupPk.toBase58(), isSigner: false, isWritable: true },
    { pubkey: queuePk.toBase58(), isSigner: false, isWritable: true },
  ];

  for (const lane of lanes) {
    const raw = lane.remainingAccounts.map((a) => ({
      pubkey: a.pubkey,
      isSigner: a.isSigner,
      isWritable: a.isWritable,
    }));

    // Hash 1: "enqueue-style" (merge with group+queue+sysvar)
    const enqueueAccounts = mergeEffectiveRuntimeFlags(raw, enqueueFixed);
    const enqueueHash = hashAccounts(
      enqueueAccounts.map((a) => ({
        pubkey: new PublicKey(a.pubkey).toBuffer(),
        isSigner: a.isSigner,
        isWritable: a.isWritable,
      })),
    );

    // Hash 2: "execute-style" (merge with group+queue only, no sysvar)
    const executeAccounts = mergeEffectiveRuntimeFlags(raw, executeFixed);
    const executeHash = hashAccounts(
      executeAccounts.map((a) => ({
        pubkey: new PublicKey(a.pubkey).toBuffer(),
        isSigner: a.isSigner,
        isWritable: a.isWritable,
      })),
    );

    // Hash 3: raw (no merge at all)
    const rawHash = hashAccounts(
      raw.map((a) => ({
        pubkey: new PublicKey(a.pubkey).toBuffer(),
        isSigner: a.isSigner,
        isWritable: a.isWritable,
      })),
    );

    // Hash 4: "execute runtime" — simulate what Solana does: OR flags from ALL named accounts
    // In execute tx, group and queue are named (mut), so they're writable.
    // Any remaining_account that matches group or queue gets ORed to writable.
    const runtimeAccounts = raw.map((a) => {
      let isWritable = a.isWritable;
      let isSigner = a.isSigner;
      // Solana ORs flags when same account appears in both named and remaining
      if (a.pubkey === groupPk.toBase58()) isWritable = true;
      if (a.pubkey === queuePk.toBase58()) isWritable = true;
      // Also OR within remaining accounts (same pubkey appears multiple times)
      for (const other of raw) {
        if (other.pubkey === a.pubkey) {
          isWritable = isWritable || other.isWritable;
          isSigner = isSigner || other.isSigner;
        }
      }
      return { pubkey: a.pubkey, isSigner, isWritable };
    });
    const runtimeHash = hashAccounts(
      runtimeAccounts.map((a) => ({
        pubkey: new PublicKey(a.pubkey).toBuffer(),
        isSigner: a.isSigner,
        isWritable: a.isWritable,
      })),
    );

    const eH = enqueueHash.toString('hex');
    const xH = executeHash.toString('hex');
    const rH = rawHash.toString('hex');
    const rtH = runtimeHash.toString('hex');

    const matchesStored = eH === storedHash ? '✓' : (xH === storedHash ? '✓(exec)' : (rH === storedHash ? '✓(raw)' : (rtH === storedHash ? '✓(rt)' : '✗')));

    if (matchesStored !== '✗' || lane.name.includes('place')) {
      console.log(`\n${lane.name}: ${matchesStored}`);
      if (matchesStored === '✗') {
        console.log(`  enqueue=${eH.slice(0, 16)}... execute=${xH.slice(0, 16)}... raw=${rH.slice(0, 16)}... runtime=${rtH.slice(0, 16)}...`);
        console.log(`  stored =${storedHash.slice(0, 16)}...`);

        // Show per-account flag diff between enqueue and runtime
        for (let i = 0; i < raw.length; i++) {
          const e = enqueueAccounts[i];
          const r = runtimeAccounts[i];
          if (e.isWritable !== r.isWritable || e.isSigner !== r.isSigner) {
            console.log(`  [${i}] ${e.pubkey.slice(0, 12)}... enqueue w=${e.isWritable} s=${e.isSigner} | runtime w=${r.isWritable} s=${r.isSigner}`);
          }
        }
      }
    }
  }
}

main().catch((e) => { console.error(e); process.exit(1); });
