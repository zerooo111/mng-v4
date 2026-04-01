/**
 * generate_bot_lanes.ts — Generate lane configs for all provisioned bot keypairs.
 * Reads the existing lane config as a template, and the deposit-context for each
 * bot to get their Mango account + owner, then produces a lane file with entries
 * for every bot.
 */
import fs from 'fs';
import path from 'path';
import { Keypair, PublicKey } from '@solana/web3.js';
import axios from 'axios';

const HARNESS_URL = process.env.HARNESS_URL || 'http://127.0.0.1:9091';
const LANE_TEMPLATE_PATH = process.env.LANE_TEMPLATE_PATH || '';
const OUTPUT_PATH = process.env.OUTPUT_PATH || '';
const KEYS_DIRS = (process.env.KEYS_DIRS || '').split(',').filter(Boolean);

interface LaneAccount {
  pubkey: string;
  isWritable: boolean;
  isSigner: boolean;
}

interface Lane {
  name: string;
  remainingAccounts: LaneAccount[];
}

interface DepositContext {
  owner: string;
  mango_account: string;
  mango_account_exists: boolean;
}

async function main() {
  if (!LANE_TEMPLATE_PATH || !OUTPUT_PATH || KEYS_DIRS.length === 0) {
    console.error(
      'Usage: LANE_TEMPLATE_PATH=... OUTPUT_PATH=... KEYS_DIRS=dir1,dir2 npx ts-node generate_bot_lanes.ts',
    );
    process.exit(1);
  }

  const templateLanes: Lane[] = JSON.parse(
    fs.readFileSync(LANE_TEMPLATE_PATH, 'utf-8'),
  );

  // Use the first "place" lane as template
  const placeLane = templateLanes.find((l) => l.name.includes('place')) || templateLanes[0];
  const cancelLane = templateLanes.find((l) => l.name.includes('cancel'));

  // Identify the user-specific account indices by comparing place lanes
  // Slot [1] = mango account (writable), Slot [2] = owner (read-only)
  const templateMangoAccount = placeLane.remainingAccounts[1].pubkey;
  const templateOwner = placeLane.remainingAccounts[2].pubkey;

  console.log(`Template mango_account: ${templateMangoAccount}`);
  console.log(`Template owner: ${templateOwner}`);

  // Collect all keypair files
  const keypairFiles: string[] = [];
  for (const dir of KEYS_DIRS) {
    if (!fs.existsSync(dir)) continue;
    for (const f of fs.readdirSync(dir).sort()) {
      if (f.endsWith('.json')) {
        keypairFiles.push(path.join(dir, f));
      }
    }
  }

  console.log(`Found ${keypairFiles.length} keypairs`);

  const allLanes: Lane[] = [];

  for (const kpPath of keypairFiles) {
    const raw = JSON.parse(fs.readFileSync(kpPath, 'utf-8'));
    const kp = Keypair.fromSecretKey(Uint8Array.from(raw));
    const owner = kp.publicKey.toBase58();
    const name = path.basename(kpPath, '.json');

    // Get their mango account from the harness
    let mangoAccount: string;
    try {
      const { data } = await axios.get<DepositContext>(
        `${HARNESS_URL}/state/deposit-context/${owner}`,
      );
      mangoAccount = data.mango_account;
      if (!data.mango_account_exists) {
        console.warn(`  ${name}: mango account does not exist, skipping`);
        continue;
      }
    } catch (err: any) {
      console.warn(`  ${name}: failed to get deposit context: ${err.message}`);
      continue;
    }

    console.log(`  ${name}: owner=${owner.slice(0, 12)}... mango=${mangoAccount.slice(0, 12)}...`);

    // Generate place lane for this bot
    const botPlaceLane: Lane = {
      name: `${name}-place`,
      remainingAccounts: placeLane.remainingAccounts.map((a, i) => {
        if (a.pubkey === templateMangoAccount) {
          return { ...a, pubkey: mangoAccount };
        }
        if (a.pubkey === templateOwner) {
          return { ...a, pubkey: owner };
        }
        return { ...a };
      }),
    };
    allLanes.push(botPlaceLane);

    // Generate cancel lane if template exists
    if (cancelLane) {
      const botCancelLane: Lane = {
        name: `${name}-cancel`,
        remainingAccounts: cancelLane.remainingAccounts.map((a) => {
          if (a.pubkey === templateMangoAccount) {
            return { ...a, pubkey: mangoAccount };
          }
          if (a.pubkey === templateOwner) {
            return { ...a, pubkey: owner };
          }
          return { ...a };
        }),
      };
      allLanes.push(botCancelLane);
    }
  }

  fs.writeFileSync(OUTPUT_PATH, JSON.stringify(allLanes, null, 2));
  console.log(`\nWrote ${allLanes.length} lanes to ${OUTPUT_PATH}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
