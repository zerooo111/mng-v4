/**
 * v2-multi-market-test.ts
 *
 * Submit interleaved orders to two perp markets via the gRPC relayer.
 * Verifies multi-market parallel cranking on the v2 sub-queue layout.
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import fs from 'fs';
import path from 'path';
import nacl from 'tweetnacl';
import {
  PerpMarketIndex,
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import {
  buildExecutionQueueUserIntent,
  encodePerpPlaceOrderV2QueuePayload,
} from '../../src/executionQueue';
import { I64_MAX_BN } from '../../src/utils';

type Cfg = {
  clusterUrl: string;
  programId: string;
  group: string;
  executionQueue: string;
  maker: { keypairPath: string; mangoAccount: string };
  taker: { keypairPath: string; mangoAccount: string };
};

const CFG_PATH = process.env.CFG_PATH || '/tmp/v2-localnet/run/cfg-99.json';
const RELAYER_ADDR = process.env.RELAYER_ADDR || '127.0.0.1:29090';
const ORDERS_PER_MARKET = Number(process.env.ORDERS_PER_MARKET || '10');

async function buildCanonicalRemainingAccounts(
  client: MangoClient,
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  mangoAccount: Awaited<ReturnType<MangoClient['getMangoAccount']>>,
  marketIndex: PerpMarketIndex,
  userOwner: PublicKey,
): Promise<AccountMeta[]> {
  const perpMarket = group.getPerpMarketByMarketIndex(marketIndex);
  const healthRA = await client.buildHealthRemainingAccounts(
    group,
    [mangoAccount],
    [group.getFirstBankForPerpSettlement()],
    [perpMarket],
  );
  return [
    { pubkey: group.publicKey, isSigner: false, isWritable: false },
    { pubkey: mangoAccount.publicKey, isSigner: false, isWritable: true },
    { pubkey: userOwner, isSigner: false, isWritable: false },
    { pubkey: perpMarket.publicKey, isSigner: false, isWritable: true },
    { pubkey: perpMarket.bids, isSigner: false, isWritable: true },
    { pubkey: perpMarket.asks, isSigner: false, isWritable: true },
    { pubkey: perpMarket.eventQueue, isSigner: false, isWritable: true },
    { pubkey: perpMarket.oracle, isSigner: false, isWritable: false },
    ...healthRA.map((pubkey) => ({ pubkey, isSigner: false, isWritable: false })),
  ];
}

async function main() {
  const cfg: Cfg = JSON.parse(fs.readFileSync(CFG_PATH, 'utf-8'));
  const conn = new Connection(cfg.clusterUrl, 'confirmed');
  const programId = new PublicKey(cfg.programId);
  const groupPk = new PublicKey(cfg.group);
  const executionQueuePk = new PublicKey(cfg.executionQueue);

  const taker = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(cfg.taker.keypairPath, 'utf-8'))),
  );
  const provider = new AnchorProvider(conn, new Wallet(taker), AnchorProvider.defaultOptions());
  const client = await MangoClient.connect(provider, 'devnet', programId, {
    idsSource: 'get-program-accounts',
  });
  const group = await client.getGroup(groupPk);
  const takerAccount = await client.getMangoAccount(new PublicKey(cfg.taker.mangoAccount));

  // gRPC relayer client
  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  const relayer = new proto.ctmsequencer.CtmSequencerRelayer(
    RELAYER_ADDR,
    grpc.credentials.createInsecure(),
  );

  const submit = (req: any) =>
    new Promise<{ sequence: string; tx_signature?: string }>((resolve, reject) => {
      relayer.submitIntent(req, (err: Error | null, resp: any) => {
        if (err) reject(err);
        else resolve(resp);
      });
    });

  // Build remaining_accounts for both markets
  const ra0 = await buildCanonicalRemainingAccounts(
    client,
    group,
    takerAccount,
    0 as PerpMarketIndex,
    taker.publicKey,
  );
  const ra1 = await buildCanonicalRemainingAccounts(
    client,
    group,
    takerAccount,
    1 as PerpMarketIndex,
    taker.publicKey,
  );

  console.log(JSON.stringify({ msg: 'submitting', orders_per_market: ORDERS_PER_MARKET }));

  const results: Array<{ market: number; tick: number; sequence?: string; err?: string }> = [];

  for (let i = 0; i < ORDERS_PER_MARKET; i++) {
    for (const marketIndex of [0, 1] as const) {
      const ra = marketIndex === 0 ? ra0 : ra1;
      const perpMarket = group.getPerpMarketByMarketIndex(marketIndex as PerpMarketIndex);
      const oraclePrice = perpMarket.uiPrice;
      // post-only buy 5% below oracle so we don't cross
      const priceUi = oraclePrice * 0.95;
      const payload = encodePerpPlaceOrderV2QueuePayload({
        side: PerpOrderSide.bid,
        priceLots: BigInt(perpMarket.uiPriceToLots(priceUi).toString()),
        maxBaseLots: BigInt(perpMarket.uiBaseToLots(0.1).toString()),
        maxQuoteLots: BigInt(I64_MAX_BN.toString()),
        clientOrderId: Number(`${Date.now()}${marketIndex}${i}`),
        orderType: PerpOrderType.postOnly,
        selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
        reduceOnly: false,
        expiryTimestamp: 0,
        limit: 10,
      });

      // Build user intent envelope and sign
      const intent = await buildExecutionQueueUserIntent({
        group: groupPk,
        executionQueue: executionQueuePk,
        mangoAccount: takerAccount.publicKey,
        userOwner: taker.publicKey,
        payload,
        target: { kind: 0, index: marketIndex },
        remainingAccounts: ra,
      });
      const userSig = nacl.sign.detached(
        new Uint8Array(intent.userIntentMessage),
        taker.secretKey,
      );

      try {
        const resp = await submit({
          group: groupPk.toBase58(),
          execution_queue: executionQueuePk.toBase58(),
          market: String(marketIndex),
          payload,
          remaining_accounts: ra.map((a) => ({
            pubkey: a.pubkey.toBase58(),
            is_signer: !!a.isSigner,
            is_writable: !!a.isWritable,
          })),
          min_execute_slot: '0',
          expires_at_slot: '0',
          user_owner: taker.publicKey.toBase58(),
          mango_account: takerAccount.publicKey.toBase58(),
          user_signature: Buffer.from(userSig),
          intent_version: 2,
          target_kind: 0,
          target_index: marketIndex,
        });
        results.push({
          market: marketIndex,
          tick: i,
          sequence: resp.sequence,
        });
        console.log(JSON.stringify({
          msg: 'submit_ok',
          market: marketIndex,
          tick: i,
          sequence: resp.sequence,
        }));
      } catch (err: any) {
        results.push({ market: marketIndex, tick: i, err: err.message?.slice(0, 200) });
        console.log(JSON.stringify({
          msg: 'submit_err',
          market: marketIndex,
          tick: i,
          err: err.message?.slice(0, 200),
        }));
      }
    }
    await new Promise((r) => setTimeout(r, 200));
  }

  // Summary
  const m0 = results.filter((r) => r.market === 0);
  const m1 = results.filter((r) => r.market === 1);
  console.log(JSON.stringify({
    msg: 'summary',
    market_0: { total: m0.length, ok: m0.filter((r) => r.sequence).length, err: m0.filter((r) => r.err).length },
    market_1: { total: m1.length, ok: m1.filter((r) => r.sequence).length, err: m1.filter((r) => r.err).length },
  }));
}

main().then(() => process.exit(0)).catch((err) => {
  console.error(err);
  process.exit(1);
});
