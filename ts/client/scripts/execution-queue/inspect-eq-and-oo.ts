import { Connection, Keypair, PublicKey } from '@solana/web3.js';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { MangoClient } from '../../src/client';
import { PerpMarketIndex } from '../../src/accounts/perp';

(async () => {
  const conn = new Connection('https://devnet.helius-rpc.com/?api-key=61e8475f-abea-4774-bc59-9b8ba4df20a0', 'confirmed');
  const kp = Keypair.generate();
  const prov = new AnchorProvider(conn, new Wallet(kp), { commitment: 'confirmed' });
  const client = await MangoClient.connect(prov, 'devnet', new PublicKey('Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt'), { idsSource: 'get-program-accounts' });
  const group = await client.getGroup(new PublicKey('Cj8vUC2nWbREhofnD3iWk4j8CD9Fo6j9c33M5ZFKLVPB'));
  const perp = group.getPerpMarketByMarketIndex(0 as PerpMarketIndex);
  const target = new PublicKey('CMFfQAEX72fJhTr8rBJAStfrm65v94dPovNdS6MJDyyq');

  // Event queue stats
  const eq = await perp.loadEventQueue(client);
  const unconsumed = eq.getUnconsumedEvents();
  console.log('=== event queue ===');
  console.log('total unconsumed events:', unconsumed.length);
  // Count events that affect CMFfQAEX72
  let mineMaker = 0, mineTaker = 0, mineOut = 0;
  for (const ev of unconsumed) {
    const e = ev as any;
    if (e.maker && e.maker.equals && e.maker.equals(target)) mineMaker++;
    if (e.taker && e.taker.equals && e.taker.equals(target)) mineTaker++;
    if (e.owner && e.owner.equals && e.owner.equals(target)) mineOut++;
  }
  console.log('events for CMFfQAEX72: maker=' + mineMaker + ' taker=' + mineTaker + ' out=' + mineOut);

  // OO array detail with raw fields
  console.log('\n=== CMFfQAEX72 perp_open_orders raw ===');
  const acc = await client.getMangoAccount(target);
  let occ = 0;
  for (let i = 0; i < (acc.perpOpenOrders?.length || 0); i++) {
    const oo = acc.perpOpenOrders[i] as any;
    const idStr = oo?.id?.toString?.() || '0';
    if (idStr !== '0') {
      occ++;
      console.log(`  slot[${i}]: market=${oo.marketIndex} side_tree=${oo.sideAndTree} id=${idStr.slice(0,18)} client=${oo.clientId?.toString?.() || '0'} qty=${oo.quantity?.toString?.() || '?'}`);
    }
  }
  console.log('total occupied:', occ);

  // Compare to actual book
  console.log('\n=== orderbook IDs ===');
  const [bids, asks] = await Promise.all([perp.loadBids(client, true), perp.loadAsks(client, true)]);
  const bookIds = new Set<string>();
  for (const o of bids.items()) {
    if (o.owner && o.owner.equals(target)) bookIds.add((o as any).orderId?.toString?.() || (o as any).key?.toString?.() || '?');
  }
  for (const o of asks.items()) {
    if (o.owner && o.owner.equals(target)) bookIds.add((o as any).orderId?.toString?.() || (o as any).key?.toString?.() || '?');
  }
  console.log('CMFfQAEX72 distinct order ids on book:', bookIds.size);
})().catch(e => { console.error(e.message); process.exit(1); });
