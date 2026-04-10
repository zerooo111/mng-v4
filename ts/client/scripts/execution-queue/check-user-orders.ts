import { Connection, PublicKey, Keypair } from '@solana/web3.js';
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

  const [bids, asks] = await Promise.all([perp.loadBids(client, true), perp.loadAsks(client, true)]);
  const myBids: number[] = [];
  for (const o of bids.items()) {
    if (o.owner && o.owner.equals(target)) myBids.push(Number(o.uiPrice));
  }
  const myAsks: number[] = [];
  for (const o of asks.items()) {
    if (o.owner && o.owner.equals(target)) myAsks.push(Number(o.uiPrice));
  }
  myBids.sort((a, b) => b - a);
  myAsks.sort((a, b) => a - b);
  console.log('CMFfQAEX72 bids on book: ' + myBids.length);
  console.log('CMFfQAEX72 asks on book: ' + myAsks.length);
  if (myBids.length > 0) {
    console.log('bid range: max=' + myBids[0].toFixed(3) + ' min=' + myBids[myBids.length-1].toFixed(3));
    console.log('all bids: ' + myBids.map(p => p.toFixed(3)).join(', '));
  }
  if (myAsks.length > 0) {
    console.log('ask range: min=' + myAsks[0].toFixed(3) + ' max=' + myAsks[myAsks.length-1].toFixed(3));
  }
})().catch(e => { console.error(e.message); process.exit(1); });
