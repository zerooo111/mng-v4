/**
 * v5-unstick-health-region.ts
 *
 * One-shot recovery: finds every MangoAccount in the group that has
 * `in_health_region = 1` stuck set, and sends a standalone
 * `health_region_end` tx for each to clear the flag.
 *
 * Root cause of the stuck state: a prior reveal_execute_market called
 * `queue_health_region_begin` successfully, then errored before the
 * paired `queue_health_region_end` could run. The flag stays set, and
 * every future reveal for that account fails silently with status=Failed
 * (20-24k CU, no log) because `queue_health_region_begin` requires
 * `!is_in_health_region`.
 *
 * The standalone `health_region_end` handler:
 *   - requires `is_in_health_region()` (our stuck accounts satisfy)
 *   - computes health_cache from remaining_accounts
 *   - check_health_post(current_post >= stored_pre_init_health)
 *   - clears the flag + resets stored pre_init_health
 *
 * Stuck accounts have been idle (no reveals succeeded for them since
 * the begin), so their health hasn't drifted — check_health_post passes.
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE       RPC url
 *   CTM_RELAYER_PROGRAM_ID     mango-v4 program id
 *   MB_PAYER_KEYPAIR           admin + fee payer keypair path
 *   EXECUTION_QUEUE_GROUP_NUM  group num
 *
 * Optional env:
 *   V5_UNSTICK_DRY_RUN=1       scan only, don't send txs
 *   V5_UNSTICK_LIMIT=N         cap number of unstick txs in one run
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import fs from 'fs';

import { MangoClient } from '../../src/client';

const CLUSTER_URL = requireEnv('CLUSTER_URL_OVERRIDE');
const PROGRAM_ID = new PublicKey(requireEnv('CTM_RELAYER_PROGRAM_ID'));
const ADMIN_KEYPAIR = requireEnv('MB_PAYER_KEYPAIR');
const GROUP_NUM = Number(requireEnv('EXECUTION_QUEUE_GROUP_NUM'));
const DRY_RUN = process.env.V5_UNSTICK_DRY_RUN === '1';
const LIMIT = Number(process.env.V5_UNSTICK_LIMIT ?? '1000');

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`missing required env: ${name}`);
  return v;
}

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

function log(obj: Record<string, unknown>): void {
  console.log(JSON.stringify({ ts: new Date().toISOString(), ...obj }));
}

function deriveGroup(admin: PublicKey, groupNum: number): PublicKey {
  const buf = Buffer.alloc(4);
  buf.writeUInt32LE(groupNum);
  return PublicKey.findProgramAddressSync(
    [Buffer.from('Group'), admin.toBuffer(), buf],
    PROGRAM_ID,
  )[0];
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, { commitment: 'confirmed' });
  const provider = new AnchorProvider(
    connection,
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(provider, 'devnet', PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });

  const groupPk = deriveGroup(admin.publicKey, GROUP_NUM);
  const group = await client.getGroup(groupPk);
  log({ msg: 'scanning_group', group: groupPk.toBase58() });

  // Load every mango account in the group.
  const all = await client.getAllMangoAccounts(group, true);
  log({ msg: 'loaded_mango_accounts', count: all.length });

  // Filter to stuck ones. The client wraps the on-chain MangoAccountFixed;
  // the flag is `beingLiquidated` vs `inHealthRegion` — exact property name
  // depends on the TS client's casing. Read raw bytes as the canonical source
  // to avoid any naming drift.
  const stuck: typeof all = [];
  for (const acct of all) {
    const ai = await connection.getAccountInfo(acct.publicKey, 'confirmed');
    if (!ai) continue;
    const data = ai.data;
    // Layout: disc(8) + group(32) + owner(32) + name(32) + delegate(32) +
    //         account_num(u32) + being_liquidated(u8) + in_health_region(u8)
    const offset = 8 + 32 + 32 + 32 + 32 + 4 + 1;
    if (data[offset] === 1) {
      stuck.push(acct);
    }
  }
  log({ msg: 'stuck_found', count: stuck.length });
  if (stuck.length === 0) {
    log({ msg: 'no_work', note: 'no mango accounts have in_health_region=1' });
    return;
  }

  if (DRY_RUN) {
    for (const a of stuck) {
      log({
        msg: 'would_unstick',
        mango_account: a.publicKey.toBase58(),
        owner: a.owner.toBase58(),
      });
    }
    return;
  }

  // Process stuck accounts.
  let fixed = 0;
  let failed = 0;
  const perpMarkets = Array.from(group.perpMarketsMapByMarketIndex.values());
  const banks = [group.getFirstBankForPerpSettlement()];

  for (const acct of stuck.slice(0, LIMIT)) {
    try {
      // Build health remaining accounts for this account's positions.
      // The client helper computes which (bank, oracle, perp_market, perp_oracle)
      // tuples the account needs based on its holdings + the markets passed in.
      const remaining = await client.buildHealthRemainingAccounts(
        group,
        [acct],
        banks,
        perpMarkets,
      );
      const remainingMeta = remaining.map((pk) => ({
        pubkey: pk,
        isSigner: false,
        isWritable: false,
      }));

      const ix = await client.program.methods
        .healthRegionEnd()
        .accounts({ account: acct.publicKey })
        .remainingAccounts(remainingMeta)
        .instruction();

      const sig = await client.sendAndConfirmTransactionForGroup(group, [ix]);
      log({
        msg: 'unstuck',
        mango_account: acct.publicKey.toBase58(),
        owner: acct.owner.toBase58(),
        sig,
      });
      fixed += 1;
    } catch (e: any) {
      log({
        msg: 'unstick_failed',
        mango_account: acct.publicKey.toBase58(),
        error: String(e?.message ?? e).slice(0, 300),
      });
      failed += 1;
    }
  }

  log({
    msg: 'done',
    stuck_total: stuck.length,
    fixed,
    failed,
    skipped_due_to_limit: Math.max(0, stuck.length - LIMIT),
  });

  // Verification: re-scan a few of the fixed accounts.
  if (fixed > 0) {
    let still_stuck = 0;
    for (const acct of stuck.slice(0, Math.min(fixed, 5))) {
      const ai = await connection.getAccountInfo(acct.publicKey, 'confirmed');
      if (!ai) continue;
      const offset = 8 + 32 + 32 + 32 + 32 + 4 + 1;
      if (ai.data[offset] === 1) still_stuck += 1;
    }
    log({ msg: 'verification', resampled: Math.min(fixed, 5), still_stuck });
  }
}

main().catch((err) => {
  console.error(
    JSON.stringify({
      ts: new Date().toISOString(),
      level: 'error',
      error: String(err?.stack ?? err),
    }),
  );
  process.exit(1);
});
