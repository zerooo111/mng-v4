/**
 * swap-perp-oracle.ts
 *
 * Admin: change the oracle pubkey on a perp market. Used when a Pyth feed
 * stalls on devnet and we need to drop in a stub oracle (see
 * stub_oracle_create) to unblock health accounts.
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE
 *   CTM_RELAYER_PROGRAM_ID      program id
 *   MB_PAYER_KEYPAIR            admin keypair path
 *   V4_GROUP                    group pubkey
 *   PERP_MARKET_INDEX           market index to edit
 *   NEW_ORACLE_PK               new oracle account
 */
import { Keypair, PublicKey } from '@solana/web3.js';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Connection } from '@solana/web3.js';
import * as fs from 'fs';

import { MangoClient } from '../../src/client';
import { NullPerpEditParams } from '../../src/clientIxParamBuilder';

function reqEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`missing env ${name}`);
  return v;
}

async function main(): Promise<void> {
  const CLUSTER_URL = reqEnv('CLUSTER_URL_OVERRIDE');
  const PROGRAM_ID = new PublicKey(reqEnv('CTM_RELAYER_PROGRAM_ID'));
  const GROUP_PK = new PublicKey(reqEnv('V4_GROUP'));
  const MARKET_INDEX = Number(reqEnv('PERP_MARKET_INDEX'));
  const NEW_ORACLE = new PublicKey(reqEnv('NEW_ORACLE_PK'));
  const admin = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(reqEnv('MB_PAYER_KEYPAIR'), 'utf-8'))),
  );

  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const provider = new AnchorProvider(connection, new Wallet(admin), { commitment: 'confirmed' });
  const client = await MangoClient.connect(provider, 'devnet', PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });
  const group = await client.getGroup(GROUP_PK);
  const perpMarket = group.perpMarketsMapByMarketIndex.get(MARKET_INDEX as any);
  if (!perpMarket) {
    throw new Error(`perp market ${MARKET_INDEX} not found in group ${GROUP_PK.toBase58()}`);
  }

  console.log(JSON.stringify({
    msg: 'before',
    market_index: MARKET_INDEX,
    perp_market: perpMarket.publicKey.toBase58(),
    current_oracle: perpMarket.oracle.toBase58(),
    new_oracle: NEW_ORACLE.toBase58(),
  }));

  // PerpEditParams requires every field, not Partial. Start from the null
  // template (all None) and set only `oracle`.
  const params = { ...NullPerpEditParams, oracle: NEW_ORACLE };
  const sig = await client.perpEditMarket(group, MARKET_INDEX as any, params);
  console.log(JSON.stringify({ msg: 'edit_tx', sig }));

  await new Promise((r) => setTimeout(r, 3000));
  const group2 = await client.getGroup(GROUP_PK);
  const pm2 = group2.perpMarketsMapByMarketIndex.get(MARKET_INDEX as any)!;
  console.log(JSON.stringify({
    msg: 'after',
    market_index: MARKET_INDEX,
    perp_market: pm2.publicKey.toBase58(),
    oracle: pm2.oracle.toBase58(),
    match: pm2.oracle.equals(NEW_ORACLE),
  }));
}

main().catch((e) => { console.error(e); process.exit(1); });
