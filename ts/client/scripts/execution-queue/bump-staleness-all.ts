/**
 * bump-staleness-all.ts
 *
 * Bump oracle staleness tolerance on USDC bank AND perp markets
 * (2/3/4) so reveals don't terminalize on the devnet Pyth feed's
 * occasional 600+ slot publish_time gaps. Sets max_staleness_slots=1800
 * (~12 min at 400 ms/slot), a temporary devnet value.
 *
 * Mainnet MUST NOT use this. See exec_q_debugging.md WARN section and
 * mainnet_plan_17apr.md §P0-30.
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import fs from 'fs';
import { MangoClient } from '../../src/client';
import {
  NullPerpEditParams,
  NullTokenEditParams,
} from '../../src/clientIxParamBuilder';
import { PerpMarketIndex } from '../../src/accounts/perp';
import { TokenIndex } from '../../src/accounts/bank';

const CLUSTER: Cluster = 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE!;
const PROGRAM_ID = new PublicKey(process.env.CTM_RELAYER_PROGRAM_ID!);
const ADMIN_KEYPAIR = process.env.MB_PAYER_KEYPAIR!;
const GROUP_PK = new PublicKey(process.env.GROUP_PK || process.env.V4_GROUP!);
const PERP_INDICES: PerpMarketIndex[] = (
  process.env.PERP_MARKET_INDICES || '2,3,4'
)
  .split(',')
  .map((s) => Number(s.trim()) as PerpMarketIndex);
const USDC_TOKEN_INDEX = Number(
  process.env.USDC_TOKEN_INDEX || '0',
) as TokenIndex;
const MAX_STALE = Number(process.env.ORACLE_MAX_STALENESS_SLOTS || '1800');
const CONF_FILTER = Number(process.env.ORACLE_CONF_FILTER || '0.1');

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

async function main() {
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

  const oracleConfig = {
    confFilter: CONF_FILTER,
    maxStalenessSlots: MAX_STALE,
  };

  // USDC bank
  const usdcBank = group.getFirstBankByTokenIndex(USDC_TOKEN_INDEX);
  console.log(
    JSON.stringify({
      msg: 'tokenEdit_usdc_before',
      token_index: USDC_TOKEN_INDEX,
      mint: usdcBank.mint.toBase58(),
      oracle: usdcBank.oracle.toBase58(),
      current_max_stale: usdcBank.oracleConfig.maxStalenessSlots.toString(),
      current_conf: usdcBank.oracleConfig.confFilter.toNumber(),
    }),
  );
  const usdcSig = await client.tokenEdit(group, usdcBank.mint, {
    ...NullTokenEditParams,
    oracleConfig,
    resetStablePrice: false,
  });
  console.log(
    JSON.stringify({
      msg: 'tokenEdit_usdc_sig',
      new_max_stale: MAX_STALE,
      sig: usdcSig.signature,
    }),
  );

  // Perp markets
  for (const idx of PERP_INDICES) {
    const pm = group.getPerpMarketByMarketIndex(idx);
    console.log(
      JSON.stringify({
        msg: 'perpEdit_before',
        perp_market_index: idx,
        oracle: pm.oracle.toBase58(),
        current_max_stale: pm.oracleConfig.maxStalenessSlots.toString(),
      }),
    );
    const sig = await client.perpEditMarket(group, idx, {
      ...NullPerpEditParams,
      oracleConfig,
      resetStablePrice: false,
    });
    console.log(
      JSON.stringify({
        msg: 'perpEdit_sig',
        perp_market_index: idx,
        new_max_stale: MAX_STALE,
        sig: sig.signature,
      }),
    );
  }

  await group.reloadAll(client);
  console.log(
    JSON.stringify({
      msg: 'verify_usdc_bank',
      max_stale: group
        .getFirstBankByTokenIndex(USDC_TOKEN_INDEX)
        .oracleConfig.maxStalenessSlots.toString(),
    }),
  );
  for (const idx of PERP_INDICES) {
    console.log(
      JSON.stringify({
        msg: 'verify_perp',
        perp_market_index: idx,
        max_stale: group
          .getPerpMarketByMarketIndex(idx)
          .oracleConfig.maxStalenessSlots.toString(),
      }),
    );
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
