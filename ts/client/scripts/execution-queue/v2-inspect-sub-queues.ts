import { Connection, PublicKey } from '@solana/web3.js';
import fs from 'fs';

const CFG_PATH = process.env.CFG_PATH || '/tmp/v2-localnet/run/cfg-99.json';

// Layout offsets (with 8-byte discriminator)
const SUB_QUEUE_HEADERS_OFFSET = 264;
const SUB_QUEUE_HEADER_STRIDE = 64;
const N_MAX_MARKETS = 16;

const SUB_HDR_MARKET_INDEX_OFFSET = 0;
const SUB_HDR_ACTIVE_OFFSET = 2;
const SUB_HDR_PAUSED_INGRESS_OFFSET = 3;
const SUB_HDR_PAUSED_EXECUTE_OFFSET = 4;
const SUB_HDR_CTM_COUNT_OFFSET = 8;
const SUB_HDR_NEXT_SEQ_OFFSET = 16;
const SUB_HDR_MAX_SEEN_OFFSET = 24;
const SUB_HDR_GAP_OBSERVED_OFFSET = 32;

const LAYOUT_VERSION_OFFSET = 147;
const TOTAL_COUNT_OFFSET = 152 + 24; // global_header + 24

async function main() {
  const cfg = JSON.parse(fs.readFileSync(CFG_PATH, 'utf-8'));
  const conn = new Connection(cfg.clusterUrl, 'confirmed');
  const queuePk = new PublicKey(cfg.executionQueue);
  const acct = await conn.getAccountInfo(queuePk);
  if (!acct) {
    console.error('queue account not found');
    process.exit(1);
  }
  const data = acct.data;
  console.log(JSON.stringify({
    msg: 'queue_account',
    pubkey: queuePk.toBase58(),
    data_length: data.length,
    layout_version: data[LAYOUT_VERSION_OFFSET],
    paused_ingress: data[145],
    paused_execute: data[146],
    total_count: data.readUInt32LE(TOTAL_COUNT_OFFSET),
  }));

  for (let i = 0; i < N_MAX_MARKETS; i++) {
    const off = SUB_QUEUE_HEADERS_OFFSET + i * SUB_QUEUE_HEADER_STRIDE;
    const active = data[off + SUB_HDR_ACTIVE_OFFSET];
    if (active === 0) continue;
    const marketIndex = data.readUInt16LE(off + SUB_HDR_MARKET_INDEX_OFFSET);
    const ctmCount = data.readUInt32LE(off + SUB_HDR_CTM_COUNT_OFFSET);
    const nextSeq = Number(data.readBigUInt64LE(off + SUB_HDR_NEXT_SEQ_OFFSET));
    const maxSeen = Number(data.readBigUInt64LE(off + SUB_HDR_MAX_SEEN_OFFSET));
    const gapObs = Number(data.readBigUInt64LE(off + SUB_HDR_GAP_OBSERVED_OFFSET));
    console.log(JSON.stringify({
      msg: 'sub_queue',
      slot_index: i,
      market_index: marketIndex,
      active,
      paused_ingress: data[off + SUB_HDR_PAUSED_INGRESS_OFFSET],
      paused_execute: data[off + SUB_HDR_PAUSED_EXECUTE_OFFSET],
      ctm_count: ctmCount,
      next_sequence_to_execute: nextSeq,
      max_seen_sequence: maxSeen,
      gap_observed_slot: gapObs,
    }));
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
