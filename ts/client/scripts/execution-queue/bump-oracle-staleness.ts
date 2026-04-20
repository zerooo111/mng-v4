/**
 * bump-oracle-staleness.ts
 *
 * One-shot admin fix: raise `max_staleness_slots` on each configured perp
 * market from 0 (strict — any older-than-current-slot price is stale) to
 * 600 slots (~4 minutes, tolerates the lag of devnet's sponsored Pyth
 * feeds which update on a long cadence).
 *
 * Devnet-only workaround. Mainnet must run its own Pyth publisher +
 * tighter max_staleness_slots (see debug.md §oracle-staleness-prod).
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import fs from 'fs';
import { PerpMarketIndex } from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import { NullPerpEditParams } from '../../src/clientIxParamBuilder';

const CLUSTER: Cluster = 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE!;
const PROGRAM_ID = new PublicKey(process.env.CTM_RELAYER_PROGRAM_ID!);
const ADMIN_KEYPAIR = process.env.MB_PAYER_KEYPAIR!;
const GROUP_PK = new PublicKey(process.env.GROUP_PK || process.env.V4_GROUP!);
const PERP_MARKET_INDICES: PerpMarketIndex[] = (
  process.env.PERP_MARKET_INDICES || '2,3,4'
)
  .split(',')
  .map((s) => Number(s.trim()) as PerpMarketIndex);
const NEW_MAX_STALENESS_SLOTS = Number(
  process.env.ORACLE_MAX_STALENESS_SLOTS || '600',
);
const CONF_FILTER = Number(process.env.ORACLE_CONF_FILTER || '0.1');

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const provider = new AnchorProvider(
    connection,
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const client = MangoClient.connect(provider, CLUSTER, PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });
  const group = await client.getGroup(GROUP_PK);

  for (const idx of PERP_MARKET_INDICES) {
    const perpMarket = group.getPerpMarketByMarketIndex(idx);
    console.log(
      JSON.stringify({
        msg: 'perpEditMarket_staleness_before',
        perp_market_index: idx,
        oracle: perpMarket.oracle.toBase58(),
        current_max_staleness_slots: perpMarket.oracleConfig.maxStalenessSlots.toString(),
        current_conf_filter: perpMarket.oracleConfig.confFilter.toNumber(),
      }),
    );
    const sig = await client.perpEditMarket(group, idx, {
      ...NullPerpEditParams,
      oracleConfig: {
        confFilter: CONF_FILTER,
        maxStalenessSlots: NEW_MAX_STALENESS_SLOTS,
      },
      resetStablePrice: false,
    });
    console.log(
      JSON.stringify({
        msg: 'perpEditMarket_staleness_sig',
        perp_market_index: idx,
        new_max_staleness_slots: NEW_MAX_STALENESS_SLOTS,
        sig: sig.signature,
      }),
    );
  }

  await group.reloadAll(client);
  for (const idx of PERP_MARKET_INDICES) {
    const pm = group.getPerpMarketByMarketIndex(idx);
    console.log(
      JSON.stringify({
        msg: 'perpEditMarket_staleness_after',
        perp_market_index: idx,
        max_staleness_slots: pm.oracleConfig.maxStalenessSlots.toString(),
        conf_filter: pm.oracleConfig.confFilter.toNumber(),
      }),
    );
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
