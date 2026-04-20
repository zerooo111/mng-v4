// Bootstrap a single Address Lookup Table for v5 reveal txs across all
// configured markets. The relayer loads this ALT at startup so reveal txs
// can reference shared pubkeys (group, queue, per-market dispatch accounts)
// by 1-byte indices instead of inlining 32-byte pubkeys — letting us pack
// 3-4 reveals per tx instead of legacy's ~2.
//
// Usage:
//   ts-node ts/client/scripts/execution-queue/v5-bootstrap-alt.ts
//
// Env used:
//   CLUSTER_URL_OVERRIDE    RPC endpoint
//   CTM_RELAYER_PAYER_KEYPAIR   funding + authority keypair
//   PROGRAM_ID              mango-v4 program
//   V4_GROUP, V4_AUTHORITY_STATE
//   V5_QUEUE                v5 queue PDA
//   V4_PERP_*, V4_USDC_*    SOL (market 2) accounts
//   V5_M3_PERP_*, V5_M4_PERP_*  ETH/BTC per-market accounts
//
// Emits the ALT address to stdout. Set V5_REVEAL_ALT_ADDRESS=<addr> in the
// relayer env to activate.

import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
  AddressLookupTableProgram,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  ComputeBudgetProgram,
} from '@solana/web3.js';
import * as fs from 'fs';

function env(name: string): string {
  const v = process.env[name];
  if (!v) {
    throw new Error(`missing env ${name}`);
  }
  return v;
}

function optEnv(name: string): string | undefined {
  const v = process.env[name];
  return v && v.length > 0 ? v : undefined;
}

function loadKeypair(path: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path, 'utf-8'));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

async function main(): Promise<void> {
  const rpcUrl = env('CLUSTER_URL_OVERRIDE');
  const payer = loadKeypair(env('CTM_RELAYER_PAYER_KEYPAIR'));
  const connection = new Connection(rpcUrl, 'confirmed');

  const programId = new PublicKey(env('PROGRAM_ID'));
  const group = new PublicKey(env('V4_GROUP'));
  const authState = new PublicKey(env('V4_AUTHORITY_STATE'));
  // Per-market queue PDAs (post-split from a single shared queue). The
  // relayer derives each from (program_id, group, market_index); we mirror
  // that here so the ALT picks up all of them in one pass.
  const usdcBank = new PublicKey(env('V4_USDC_BANK'));
  const usdcOracle = new PublicKey(env('V4_USDC_ORACLE'));
  const u16le = (n: number): Buffer => {
    const b = Buffer.alloc(2);
    b.writeUInt16LE(n, 0);
    return b;
  };
  const deriveMarketQueue = (mi: number): PublicKey => {
    const [pk] = PublicKey.findProgramAddressSync(
      [Buffer.from('execution-queue-v5'), group.toBuffer(), u16le(mi)],
      programId,
    );
    return pk;
  };

  // Market-specific account sets. Market 2 uses the legacy V4_PERP_* env
  // names (SOL was bootstrapped first); markets 3 and 4 use the V5_M*_ names.
  type PerMarket = {
    idx: number;
    perp_market: PublicKey;
    bids: PublicKey;
    asks: PublicKey;
    event_queue: PublicKey;
    oracle: PublicKey;
  };
  const markets: PerMarket[] = [
    {
      idx: 2,
      perp_market: new PublicKey(env('V4_PERP_MARKET')),
      bids: new PublicKey(env('V4_PERP_BIDS')),
      asks: new PublicKey(env('V4_PERP_ASKS')),
      event_queue: new PublicKey(env('V4_PERP_EVENT_QUEUE')),
      oracle: new PublicKey(env('V4_PERP_ORACLE')),
    },
  ];
  for (const m of ['3', '4']) {
    const pm = optEnv(`V5_M${m}_PERP_MARKET`);
    if (!pm) continue;
    markets.push({
      idx: parseInt(m, 10),
      perp_market: new PublicKey(pm),
      bids: new PublicKey(env(`V5_M${m}_PERP_BIDS`)),
      asks: new PublicKey(env(`V5_M${m}_PERP_ASKS`)),
      event_queue: new PublicKey(env(`V5_M${m}_PERP_EVENT_QUEUE`)),
      oracle: new PublicKey(env(`V5_M${m}_PERP_ORACLE`)),
    });
  }

  // De-dupe the full address set. Fixed accounts (group, authority_state,
  // queue, instructions_sysvar, program_id, payer, usdc_bank, usdc_oracle)
  // appear once each; each market adds 5 more; repeated pubkeys (e.g. the
  // payer and program_id referenced by every reveal) are filtered below.
  const fixed: PublicKey[] = [
    group,
    authState,
    SYSVAR_INSTRUCTIONS_PUBKEY,
    programId,
    payer.publicKey,
    usdcBank,
    usdcOracle,
  ];
  const perMarketQueues: PublicKey[] = markets.map((m) => deriveMarketQueue(m.idx));
  const perMarket: PublicKey[] = markets.flatMap((m) => [
    m.perp_market,
    m.bids,
    m.asks,
    m.event_queue,
    m.oracle,
  ]);
  const seen = new Set<string>();
  const addresses: PublicKey[] = [];
  for (const k of [...fixed, ...perMarketQueues, ...perMarket]) {
    const s = k.toBase58();
    if (!seen.has(s)) {
      seen.add(s);
      addresses.push(k);
    }
  }
  console.log(`ALT will hold ${addresses.length} unique addresses:`);
  for (const a of addresses) console.log(`  - ${a.toBase58()}`);

  // Create.
  const recentSlot = await connection.getSlot('finalized');
  const [createIx, altAddress] = AddressLookupTableProgram.createLookupTable({
    authority: payer.publicKey,
    payer: payer.publicKey,
    recentSlot,
  });
  const createTx = new Transaction().add(
    ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 10 }),
    createIx,
  );
  const sig1 = await sendAndConfirmTransaction(connection, createTx, [payer], {
    commitment: 'confirmed',
    skipPreflight: false,
  });
  console.log(`\ncreated ALT ${altAddress.toBase58()} (sig=${sig1})`);

  // Extend. ALT extend has an address-count-per-tx limit (~30 fits safely
  // under tx size). With our current ~23 addresses one tx is enough; guard
  // anyway.
  const EXTEND_CHUNK = 20;
  for (let off = 0; off < addresses.length; off += EXTEND_CHUNK) {
    const chunk = addresses.slice(off, off + EXTEND_CHUNK);
    const extendIx = AddressLookupTableProgram.extendLookupTable({
      payer: payer.publicKey,
      authority: payer.publicKey,
      lookupTable: altAddress,
      addresses: chunk,
    });
    const extendTx = new Transaction().add(
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 10 }),
      extendIx,
    );
    const sig = await sendAndConfirmTransaction(connection, extendTx, [payer], {
      commitment: 'confirmed',
      skipPreflight: false,
    });
    console.log(
      `extended ALT with ${chunk.length} addrs [${off}..${off + chunk.length}) sig=${sig}`,
    );
  }

  console.log(`\nALT ready. Set:\n  V5_REVEAL_ALT_ADDRESS=${altAddress.toBase58()}`);
  console.log(
    `\nNote: the ALT is fully usable only after ~1 slot; give the relayer a few seconds before first send.`,
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
