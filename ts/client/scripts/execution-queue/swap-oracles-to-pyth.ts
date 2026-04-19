/**
 * swap-oracles-to-pyth.ts
 *
 * Edits an existing v4 group to swap stub oracles for the Pyth Data
 * Association sponsored PriceUpdateV2 feeds (shard 0). Idempotent — if a
 * bank/perp_market already points at the target Pyth feed it is skipped.
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE       — RPC url
 *   CTM_RELAYER_PROGRAM_ID     — mango program id (5KaJhG2... on the v4 deploy)
 *   MB_PAYER_KEYPAIR           — admin keypair (= group admin)
 *   GROUP_PK                   — v4 group pubkey
 *
 * Optional env:
 *   PERP_MARKET_INDICES        — CSV of perp market indices to retarget
 *                                (default: 0 → SOL-PERP)
 *   USDC_TOKEN_INDEX           — quote token index (default: 0)
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import fs from 'fs';
import { PerpMarketIndex } from '../../src/accounts/perp';
import { TokenIndex } from '../../src/accounts/bank';
import { MangoClient } from '../../src/client';
import {
  NullPerpEditParams,
  NullTokenEditParams,
} from '../../src/clientIxParamBuilder';

const CLUSTER: Cluster = 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE!;
const PROGRAM_ID = new PublicKey(process.env.CTM_RELAYER_PROGRAM_ID!);
const ADMIN_KEYPAIR = process.env.MB_PAYER_KEYPAIR!;
const GROUP_PK = new PublicKey(process.env.GROUP_PK!);
const USDC_TOKEN_INDEX = Number(
  process.env.USDC_TOKEN_INDEX || '0',
) as TokenIndex;
const PERP_MARKET_INDICES: PerpMarketIndex[] = (
  process.env.PERP_MARKET_INDICES || '0'
)
  .split(',')
  .map((s) => Number(s.trim()) as PerpMarketIndex);

// Sponsored PriceUpdateV2 feed accounts (shard 0). Identical on mainnet/devnet.
const PYTH_SPONSORED_FEED = {
  USDC: new PublicKey('Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX'),
  SOL: new PublicKey('7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE'),
  BTC: new PublicKey('4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo'),
  ETH: new PublicKey('42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC'),
};

// Per-perp-market-index quote-asset Pyth feed.
const PERP_INDEX_TO_PYTH: Record<number, PublicKey> = {
  0: PYTH_SPONSORED_FEED.SOL,
  1: PYTH_SPONSORED_FEED.BTC,
  2: PYTH_SPONSORED_FEED.ETH,
};

const PYTH_ORACLE_CONFIG = {
  confFilter: 0.1,
  maxStalenessSlots: 600,
};

function readKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(p, 'utf-8'))),
  );
}

async function assertSponsoredFeedExists(
  connection: Connection,
  name: string,
  pk: PublicKey,
): Promise<void> {
  const info = await connection.getAccountInfo(pk, 'confirmed');
  if (!info) {
    throw new Error(
      `Pyth sponsored feed ${name} (${pk.toBase58()}) missing on ${CLUSTER}.`,
    );
  }
}

async function main(): Promise<void> {
  const admin = readKeypair(ADMIN_KEYPAIR);
  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const adminClient = await MangoClient.connect(provider, CLUSTER, PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });

  for (const [name, pk] of Object.entries(PYTH_SPONSORED_FEED)) {
    await assertSponsoredFeedExists(provider.connection, name, pk);
  }

  const group = await adminClient.getGroup(GROUP_PK);
  await group.reloadAll(adminClient);

  // 1) USDC bank → Pyth USDC feed.
  const usdcBank = group.getFirstBankByTokenIndex(USDC_TOKEN_INDEX);
  if (!usdcBank) {
    throw new Error(`USDC bank (token_index=${USDC_TOKEN_INDEX}) not found`);
  }
  if (usdcBank.oracle.equals(PYTH_SPONSORED_FEED.USDC)) {
    console.log(
      JSON.stringify({
        msg: 'usdc_oracle_already_pyth',
        token_index: USDC_TOKEN_INDEX,
        oracle: usdcBank.oracle.toBase58(),
      }),
    );
  } else {
    console.log(
      JSON.stringify({
        msg: 'tokenEdit_usdc_oracle',
        old_oracle: usdcBank.oracle.toBase58(),
        new_oracle: PYTH_SPONSORED_FEED.USDC.toBase58(),
      }),
    );
    const sig = await adminClient.tokenEdit(group, usdcBank.mint, {
      ...NullTokenEditParams,
      oracle: PYTH_SPONSORED_FEED.USDC,
      oracleConfig: PYTH_ORACLE_CONFIG,
      resetStablePrice: true,
    });
    console.log(
      JSON.stringify({ msg: 'tokenEdit_usdc_oracle_sig', sig: sig.signature }),
    );
  }

  // 2) Each perp market → its Pyth feed.
  for (const idx of PERP_MARKET_INDICES) {
    const target = PERP_INDEX_TO_PYTH[idx];
    if (!target) {
      console.warn(
        JSON.stringify({
          msg: 'no_pyth_mapping_for_perp_index',
          perp_market_index: idx,
        }),
      );
      continue;
    }
    const perpMarket = group.getPerpMarketByMarketIndex(idx);
    if (!perpMarket) {
      console.warn(
        JSON.stringify({
          msg: 'perp_market_not_found',
          perp_market_index: idx,
        }),
      );
      continue;
    }
    if (perpMarket.oracle.equals(target)) {
      console.log(
        JSON.stringify({
          msg: 'perp_oracle_already_pyth',
          perp_market_index: idx,
          oracle: perpMarket.oracle.toBase58(),
        }),
      );
      continue;
    }
    console.log(
      JSON.stringify({
        msg: 'perpEditMarket_oracle',
        perp_market_index: idx,
        old_oracle: perpMarket.oracle.toBase58(),
        new_oracle: target.toBase58(),
      }),
    );
    const sig = await adminClient.perpEditMarket(group, idx, {
      ...NullPerpEditParams,
      oracle: target,
      oracleConfig: PYTH_ORACLE_CONFIG,
      resetStablePrice: true,
    });
    console.log(
      JSON.stringify({
        msg: 'perpEditMarket_oracle_sig',
        perp_market_index: idx,
        sig: sig.signature,
      }),
    );
  }

  // Verify
  await group.reloadAll(adminClient);
  const verifiedUsdcBank = group.getFirstBankByTokenIndex(USDC_TOKEN_INDEX);
  console.log(
    JSON.stringify({
      msg: 'verify_usdc_oracle',
      token_index: USDC_TOKEN_INDEX,
      oracle: verifiedUsdcBank.oracle.toBase58(),
      expected: PYTH_SPONSORED_FEED.USDC.toBase58(),
      match: verifiedUsdcBank.oracle.equals(PYTH_SPONSORED_FEED.USDC),
    }),
  );
  for (const idx of PERP_MARKET_INDICES) {
    const target = PERP_INDEX_TO_PYTH[idx];
    if (!target) continue;
    const verifiedPerp = group.getPerpMarketByMarketIndex(idx);
    console.log(
      JSON.stringify({
        msg: 'verify_perp_oracle',
        perp_market_index: idx,
        oracle: verifiedPerp.oracle.toBase58(),
        expected: target.toBase58(),
        match: verifiedPerp.oracle.equals(target),
      }),
    );
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
