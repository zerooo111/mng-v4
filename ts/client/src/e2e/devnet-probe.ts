/**
 * Devnet probe script — reads execution queue and group state.
 * Run: npx ts-node --compiler-options '{"module":"commonjs"}' ts/client/src/e2e/devnet-probe.ts
 */
import { Connection, PublicKey, Keypair } from '@solana/web3.js';
import {
  decodeExecutionQueueHeader,
  decodeExecutionQueueHeadItem,
} from '../executionQueueLayout';

const RPC = 'https://api.devnet.solana.com';
const PROGRAM_ID = new PublicKey('7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37');
const GROUP = new PublicKey('42ZYDpNAUM8NZgmuQHwrUXsEWb98AkjJfJHYFFoCZpCi');
const EXEC_QUEUE = new PublicKey('2Bs3tGdMs4PQV98qFLXaafSi8AJWH55eT2WwN8ZKJMkn');

async function main() {
  const conn = new Connection(RPC, 'confirmed');

  // Read execution queue
  const eqAi = await conn.getAccountInfo(EXEC_QUEUE);
  if (!eqAi) {
    console.log('Execution queue account not found');
    return;
  }
  const data = Buffer.from(eqAi.data);

  const header = decodeExecutionQueueHeader(data);
  console.log('=== Execution Queue Header ===');
  console.log('  totalCount:', header.totalCount);
  console.log('  ctmCount:', header.ctmCount);
  console.log('  liquidityCount:', header.liquidityCount);
  console.log('  nextSequence:', header.nextSequence.toString());
  console.log('  maxSeenSequence:', header.maxSeenSequence.toString());
  console.log('  gapSpan:', header.gapSpan.toString());
  console.log('  headerInvariantOk:', header.headerInvariantOk);

  const head = decodeExecutionQueueHeadItem(data);
  if (head) {
    console.log('=== Head Item ===');
    console.log('  section:', head.section);
    console.log('  sequence:', head.sequence.toString());
    console.log('  minExecuteSlot:', head.minExecuteSlot.toString());
  } else {
    console.log('=== Head Item: null (queue empty or gap) ===');
  }

  // Raw fields
  const groupPk = new PublicKey(data.slice(8, 40));
  const adminPk = new PublicKey(data.slice(40, 72));
  const ctmSigner = new PublicKey(data.slice(72, 104));
  const pausedIngress = data[145];
  const pausedExecute = data[146];
  console.log('=== Queue Config ===');
  console.log('  group:', groupPk.toBase58());
  console.log('  admin:', adminPk.toBase58());
  console.log('  ctmSigner:', ctmSigner.toBase58());
  console.log('  pausedIngress:', pausedIngress);
  console.log('  pausedExecute:', pausedExecute);

  // Find perp markets owned by this group
  console.log('\n=== Searching for perp markets ===');
  const perpAccounts = await conn.getProgramAccounts(PROGRAM_ID, {
    filters: [
      { dataSize: 1080 }, // approximate PerpMarket size — may need adjustment
      { memcmp: { offset: 8, bytes: GROUP.toBase58() } },
    ],
  });
  console.log('  Found', perpAccounts.length, 'potential perp markets (size=1080)');

  // Try other common sizes
  for (const size of [584, 672, 776, 904, 1048, 1080, 1112, 1240, 1368, 1496]) {
    const accts = await conn.getProgramAccounts(PROGRAM_ID, {
      filters: [
        { dataSize: size },
        { memcmp: { offset: 8, bytes: GROUP.toBase58() } },
      ],
    });
    if (accts.length > 0) {
      console.log(`  Found ${accts.length} accounts with size=${size} for this group`);
    }
  }

  // Find mango accounts (MangoAccountFixed size varies, try known sizes)
  console.log('\n=== Searching for mango accounts ===');
  const mangoAccounts = await conn.getProgramAccounts(PROGRAM_ID, {
    filters: [
      { memcmp: { offset: 8, bytes: GROUP.toBase58() } },
    ],
  });
  console.log('  Total program accounts for this group:', mangoAccounts.length);
  const sizeMap = new Map<number, number>();
  for (const a of mangoAccounts) {
    const s = a.account.data.length;
    sizeMap.set(s, (sizeMap.get(s) || 0) + 1);
  }
  console.log('  Account sizes:', Object.fromEntries([...sizeMap.entries()].sort((a,b) => a[0]-b[0])));

  // Find token banks (check for common bank sizes)
  console.log('\n=== Token banks ===');
  for (const [size, count] of [...sizeMap.entries()].sort((a,b) => a[0]-b[0])) {
    if (count <= 10 && size > 200 && size < 100000) {
      const accts = await conn.getProgramAccounts(PROGRAM_ID, {
        filters: [
          { dataSize: size },
          { memcmp: { offset: 8, bytes: GROUP.toBase58() } },
        ],
      });
      for (const a of accts) {
        console.log(`  Account ${a.pubkey.toBase58()} size=${size}`);
      }
    }
  }

  // Check if we have a keypair to use
  const defaultKeypath = `${process.env.HOME}/.config/solana/id.json`;
  try {
    const fs = require('fs');
    const raw = fs.readFileSync(defaultKeypath, 'utf-8');
    const kp = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
    const balance = await conn.getBalance(kp.publicKey);
    console.log('\n=== Default keypair ===');
    console.log('  Pubkey:', kp.publicKey.toBase58());
    console.log('  Balance:', balance / 1e9, 'SOL');
  } catch (e) {
    console.log('\n  No default keypair at', defaultKeypath);
  }

  const slot = await conn.getSlot();
  console.log('\n  Current slot:', slot);
}

main().catch(console.error);
