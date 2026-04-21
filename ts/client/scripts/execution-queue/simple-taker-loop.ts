/**
 * simple-taker-loop.ts
 *
 * Minimal 0.5 TPS taker that rotates through all configured markets,
 * alternating buy/sell, at a price close to the live oracle (IOC).
 * Intended for devnet smoke-testing post-bootstrap, not production.
 *
 * Required env:
 *   CLUSTER_URL_OVERRIDE
 *   CTM_RELAYER_PROGRAM_ID
 *   CTM_RELAYER_ADDR             e.g. 127.0.0.1:9090
 *   USER_KEYPAIR_OVERRIDE        path to signing keypair
 *   MANGO_ACCOUNT_PK             taker's mango account (created for the group)
 *
 * Optional:
 *   TAKER_MARKET_INDICES         CSV, default "0,1,2,3,4"
 *   TAKER_INTERVAL_MS            default 2000 (0.5 tps across all markets)
 *   TAKER_SIZE_NOTIONAL_USD      default 10
 *   TAKER_HARNESS_URL            default http://127.0.0.1:9091 (for oracle lookups)
 */
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  VersionedTransaction,
  TransactionMessage,
  ComputeBudgetProgram,
} from '@solana/web3.js';
import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { MangoClient } from '../../src/client';
import { toNative } from '../../src/utils';
import { PerpOrderSide, PerpOrderType, PerpSelfTradeBehavior, PerpMarketIndex } from '../../src/accounts/perp';
import {
  buildExecutionQueueUserIntent,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { I64_MAX_BN } from '../../src/utils';
import { AccountMeta } from '@solana/web3.js';
import BN from 'bn.js';

function reqEnv(n: string): string {
  const v = process.env[n];
  if (!v) throw new Error(`missing env ${n}`);
  return v;
}
const CLUSTER_URL = reqEnv('CLUSTER_URL_OVERRIDE');
const PROGRAM_ID = new PublicKey(reqEnv('CTM_RELAYER_PROGRAM_ID'));
const RELAYER_ADDR = reqEnv('CTM_RELAYER_ADDR');
const USER_KEYPAIR = reqEnv('USER_KEYPAIR_OVERRIDE');
const MANGO_ACCOUNT = new PublicKey(reqEnv('MANGO_ACCOUNT_PK'));
const MARKETS = (process.env.TAKER_MARKET_INDICES || '0,1,2,3,4')
  .split(',').map((s) => Number(s.trim())).filter((n) => Number.isFinite(n));
// Per-market v5 execution queue PDA. The relayer's SubmitIntent proto
// field `execution_queue` must be a valid 32-byte Pubkey string; empty
// fails server-side with "String is the wrong size" during Pubkey parse.
const QUEUE_BY_MARKET: Record<number, string> = {
  0: process.env.V5_M0_QUEUE || 'GGUVjU3iEkREsWBrbYAXUUChrshZNjqxbtSJbC5eyBNu',
  1: process.env.V5_M1_QUEUE || '8kVA5nsogRgNJFXMfdgbuYE5G3sJUmjBuZmkjkWwc5Yj',
  2: process.env.V5_M2_QUEUE || 'GBJWFY8VuMiNieVM4L6bwBszeeA8VFZeyohpfF3D94DH',
  3: process.env.V5_M3_QUEUE || 'GFiPdSaJP6eAPpDLuze7CaL1FsAr3TpitiHQcqmS84cq',
  4: process.env.V5_M4_QUEUE || 'AXUarLCHmA4AewXzVBz72fwgSQsiQoNBTUSh6ZCFs6Ct',
};
const INTERVAL_MS = Number(process.env.TAKER_INTERVAL_MS || '2000');
const SIZE_USD = Number(process.env.TAKER_SIZE_NOTIONAL_USD || '10');
const HARNESS_URL = process.env.TAKER_HARNESS_URL || 'http://127.0.0.1:9091';

type MarketSpec = {
  index: number;
  perpMarket: PublicKey;
  oracle: PublicKey;
  bids: PublicKey;
  asks: PublicKey;
  eventQueue: PublicKey;
  baseDecimals: number;
  baseLotSize: number;
  quoteLotSize: number;
  name: string;
};

async function loadProto() {
  // Proto lives alongside this script, not in the relayer tree. The relayer
  // references this same file via `../../ts/client/scripts/execution-queue/
  // ctm_sequencer.proto` from its build.rs.
  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkg = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const root = grpc.loadPackageDefinition(pkg) as any;
  // Service name is CtmSequencerRelayer per ctm_sequencer.proto line 41.
  return root.ctmsequencer.CtmSequencerRelayer;
}

async function pickOracleUi(m: number): Promise<number> {
  const res = await fetch(`${HARNESS_URL}/state/book/${m}?depth=1`);
  const j = (await res.json()) as any;
  return Number(j?.data?.oracle_price_ui || 0);
}

async function main() {
  const user = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(USER_KEYPAIR, 'utf-8'))),
  );
  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const provider = new AnchorProvider(connection, new Wallet(user), {
    commitment: 'confirmed',
  });
  const client = await MangoClient.connect(provider, 'devnet', PROGRAM_ID, {
    idsSource: 'get-program-accounts',
  });
  const groupPk = new PublicKey(reqEnv('V4_GROUP'));
  const group = await client.getGroup(groupPk);

  const specs: MarketSpec[] = MARKETS.map((i) => {
    const pm = group.perpMarketsMapByMarketIndex.get(i)!;
    if (!pm) throw new Error(`perp market ${i} not found in group`);
    return {
      index: i,
      name: pm.name,
      perpMarket: pm.publicKey,
      oracle: pm.oracle,
      bids: pm.bids,
      asks: pm.asks,
      eventQueue: pm.eventQueue,
      baseDecimals: pm.baseDecimals,
      baseLotSize: pm.baseLotSize.toNumber(),
      quoteLotSize: pm.quoteLotSize.toNumber(),
    };
  });

  const Svc = await loadProto();
  const stub = new Svc(RELAYER_ADDR, grpc.credentials.createInsecure());

  let sideFlip = 0;
  let marketCursor = 0;
  let tick = 0;

  while (true) {
    const spec = specs[marketCursor % specs.length];
    marketCursor++;
    tick++;
    sideFlip ^= 1;
    const side = sideFlip ? 'buy' : 'sell';

    try {
      const oracle = await pickOracleUi(spec.index);
      if (!oracle || !Number.isFinite(oracle)) throw new Error('no oracle');
      // ±0.5% off oracle (crossing for taker IOC)
      const priceUi = side === 'buy' ? oracle * 1.005 : oracle * 0.995;
      const qtyUi = Math.max(
        spec.baseLotSize / 10 ** spec.baseDecimals,
        SIZE_USD / oracle,
      );

      // Build the v2 intent: encode payload → derive remaining accounts →
      // build canonical intent message → ed25519-sign it with the owner
      // keypair. Mirrors send-perp-order-via-relayer.ts (the script the
      // legacy per-tick loop uses), but stays in a single long-lived
      // process so we can hit the 0.5 TPS/market target without paying
      // ts-node startup cost on every submit.
      const mangoAccount = await client.getMangoAccount(MANGO_ACCOUNT, true);
      const perpMarket = group.getPerpMarketByMarketIndex(
        spec.index as PerpMarketIndex,
      );
      const perpSide =
        side === 'buy' ? PerpOrderSide.bid : PerpOrderSide.ask;
      const payload = encodePerpPlaceOrderV2QueuePayload({
        side: perpSide,
        priceLots: BigInt(
          perpMarket.uiPriceToLotsForSide(priceUi, perpSide).toString(),
        ),
        maxBaseLots: BigInt(perpMarket.uiBaseToLots(qtyUi).toString()),
        maxQuoteLots: BigInt(I64_MAX_BN.toString()),
        clientOrderId: Date.now() & 0x7fffffff,
        orderType: PerpOrderType.immediateOrCancel,
        selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
        reduceOnly: false,
        expiryTimestamp: 0,
        limit: 20,
      });
      // Include ALL configured perp markets in the health walk, not just
      // the one being traded. Once the taker fills on one market, a perp
      // position is opened and the on-chain health check will iterate
      // EVERY active perp position and demand its (perp_market, oracle)
      // pair be present in remaining_accounts. Missing any one fails
      // with ExecutionQueuePerpHealthAccountsInvalid (6101). Passing the
      // full set is always safe (no-op when no position).
      const allPerpMarkets = MARKETS.map((idx) =>
        group.getPerpMarketByMarketIndex(idx as PerpMarketIndex),
      );
      const healthRemaining = await client.buildHealthRemainingAccounts(
        group,
        [mangoAccount],
        [group.getFirstBankForPerpSettlement()],
        allPerpMarkets,
      );
      const remainingAccounts: AccountMeta[] = [
        {
          pubkey: perpMarket.publicKey,
          isSigner: false,
          isWritable: true,
        },
        { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
        { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
        { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
        { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
        ...healthRemaining.map((pubkey) => ({
          pubkey,
          isSigner: false,
          isWritable: false,
        })),
      ];
      const executionQueue = new PublicKey(
        QUEUE_BY_MARKET[spec.index] ||
          (() => {
            throw new Error(`no queue configured for market ${spec.index}`);
          })(),
      );
      const intent = await buildExecutionQueueUserIntent({
        group: groupPk,
        executionQueue,
        mangoAccount: mangoAccount.publicKey,
        userOwner: user.publicKey,
        payload,
        target: { kind: 0, index: spec.index },
        remainingAccounts,
      });
      const userSignature = signExecutionQueueIntentMessage(
        user.secretKey,
        intent.userIntentMessage,
      );

      const request = {
        group: groupPk.toBase58(),
        execution_queue: executionQueue.toBase58(),
        market: String(spec.index),
        payload,
        remaining_accounts: remainingAccounts.map((a) => ({
          pubkey: a.pubkey.toBase58(),
          is_signer: !!a.isSigner,
          is_writable: !!a.isWritable,
        })),
        min_execute_slot: '0',
        expires_at_slot: '0',
        user_owner: user.publicKey.toBase58(),
        mango_account: MANGO_ACCOUNT.toBase58(),
        user_signature: Buffer.from(userSignature),
        intent_version: 2,
        target_kind: 0,
        target_index: spec.index,
        client_order_id: Date.now() & 0x7fffffff,
      };

      await new Promise<void>((resolve, reject) => {
        stub.submitIntent(request, (err: any, resp: any) => {
          if (err) return reject(err);
          console.log(
            JSON.stringify({
              ts: new Date().toISOString(),
              tick,
              m: spec.index,
              name: spec.name,
              side,
              price_ui: priceUi.toFixed(6),
              qty_ui: qtyUi.toFixed(6),
              oracle_ui: oracle.toFixed(6),
              sequence: resp?.sequence,
              tx: (resp?.tx_signature || '').slice(0, 16),
            }),
          );
          resolve();
        });
      });
    } catch (e: any) {
      console.log(
        JSON.stringify({
          ts: new Date().toISOString(),
          tick,
          m: spec.index,
          name: spec.name,
          side,
          err: `${e?.details || e?.message || e}`.slice(0, 200),
        }),
      );
    }

    await new Promise((r) => setTimeout(r, INTERVAL_MS));
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
