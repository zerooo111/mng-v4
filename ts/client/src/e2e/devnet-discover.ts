/**
 * Devnet discovery — find perp markets, token banks, and mango accounts for the group.
 * Run: npx ts-node --compiler-options '{"module":"commonjs"}' ts/client/src/e2e/devnet-discover.ts
 */
import { Connection, PublicKey, Keypair } from '@solana/web3.js';

const RPC = 'https://api.devnet.solana.com';
const PROGRAM_ID = new PublicKey('4CGsiGHZXSnweudEcN235xkLz4twT2DJB35hS7t89cUm');
const GROUP = new PublicKey('42ZYDpNAUM8NZgmuQHwrUXsEWb98AkjJfJHYFFoCZpCi');

async function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

async function main() {
  const conn = new Connection(RPC, 'confirmed');

  // Get ALL accounts for the group in one call
  console.log('Fetching all program accounts for group...');
  let allAccounts;
  try {
    allAccounts = await conn.getProgramAccounts(PROGRAM_ID, {
      filters: [
        { memcmp: { offset: 8, bytes: GROUP.toBase58() } },
      ],
    });
  } catch (e: any) {
    console.log('getProgramAccounts failed, trying with dataSlice...');
    // Try fetching with smaller data
    allAccounts = await conn.getProgramAccounts(PROGRAM_ID, {
      filters: [
        { memcmp: { offset: 8, bytes: GROUP.toBase58() } },
      ],
      dataSlice: { offset: 0, length: 200 },
    });
  }

  console.log(`Found ${allAccounts.length} accounts for this group`);

  const sizeMap = new Map<number, { count: number; pubkeys: string[] }>();
  for (const a of allAccounts) {
    const s = a.account.data.length;
    if (!sizeMap.has(s)) sizeMap.set(s, { count: 0, pubkeys: [] });
    const entry = sizeMap.get(s)!;
    entry.count++;
    if (entry.pubkeys.length < 5) entry.pubkeys.push(a.pubkey.toBase58());
  }

  console.log('\nAccount sizes:');
  for (const [size, info] of [...sizeMap.entries()].sort((a, b) => a[0] - b[0])) {
    console.log(`  size=${size}: ${info.count} account(s)`);
    for (const pk of info.pubkeys) {
      console.log(`    - ${pk}`);
    }
  }

  // Now read the group account itself to find useful info
  console.log('\n=== Group Account ===');
  const groupAi = await conn.getAccountInfo(GROUP);
  if (groupAi) {
    const data = groupAi.data;
    console.log('  Data length:', data.length);
    // Group struct: discriminator(8) + creator(32) + group_num(4) + admin(32) + ...
    const admin = new PublicKey(data.slice(40, 72));
    console.log('  Admin (from group):', admin.toBase58());
  }

  // Check default keypair
  const defaultKeypath = `${process.env.HOME}/.config/solana/id.json`;
  try {
    const fs = require('fs');
    const raw = fs.readFileSync(defaultKeypath, 'utf-8');
    const kp = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
    const balance = await conn.getBalance(kp.publicKey);
    console.log('\n=== Default Keypair ===');
    console.log('  Pubkey:', kp.publicKey.toBase58());
    console.log('  Balance:', balance / 1e9, 'SOL');

    // Check if this keypair is the admin
    const adminPk = new PublicKey('4zDJ8FiJu7A3ZHjCwdKF2JPyLYheFRpGVvCvHS32XnhM');
    console.log('  Is admin?', kp.publicKey.equals(adminPk));
  } catch (e) {
    console.log('\n  No default keypair found');
  }

  // Check keypairs directory
  const fs = require('fs');
  const keypairsDir = '/home/ec2-user/stagin4/mng-v4/keypairs';
  if (fs.existsSync(keypairsDir)) {
    console.log('\n=== Keypairs directory ===');
    const files = fs.readdirSync(keypairsDir).filter((f: string) => f.endsWith('.json'));
    for (const file of files) {
      try {
        const raw = fs.readFileSync(`${keypairsDir}/${file}`, 'utf-8');
        const kp = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
        console.log(`  ${file}: ${kp.publicKey.toBase58()}`);
      } catch {
        console.log(`  ${file}: (invalid)`);
      }
    }
  }

  console.log('\nDone.');
}

main().catch(console.error);
