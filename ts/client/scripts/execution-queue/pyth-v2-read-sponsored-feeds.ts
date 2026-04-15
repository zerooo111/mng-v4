// Reads the Pyth sponsored PriceUpdateV2 feed accounts (shard 0) for SOL/USD,
// BTC/USD, and ETH/USD on Solana devnet and decodes their current price,
// confidence, exponent, publish time, and posted slot.
//
// No transaction is sent — the Pyth Data Association pusher keeps these
// accounts continuously fresh, so on-chain consumers (including mango-v4's
// updated OracleType::Pyth decoder in programs/mango-v4/src/state/oracle.rs)
// can simply pass these pubkeys as the oracle account.
//
// Run with:
//   npx ts-node ts/client/scripts/execution-queue/pyth-v2-read-sponsored-feeds.ts
//
// The sponsored feed account addresses are the same on mainnet and devnet.

import { Connection, PublicKey } from '@solana/web3.js';

const DEVNET_RPC = 'https://api.devnet.solana.com';

const PYTH_RECEIVER_PROGRAM = new PublicKey(
  'rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ',
);

// Anchor discriminator for PriceUpdateV2: sha256("account:PriceUpdateV2")[..8]
const PRICE_UPDATE_V2_DISCRIMINATOR = Buffer.from([
  0x22, 0xf1, 0x23, 0x63, 0x9d, 0x7e, 0xf4, 0xcd,
]);

// Sponsored PriceUpdateV2 feed accounts (shard 0). Identical on mainnet and devnet.
const FEEDS: { name: string; account: PublicKey; feedId: string }[] = [
  {
    name: 'SOL/USD',
    account: new PublicKey('7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE'),
    feedId:
      'ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d',
  },
  {
    name: 'BTC/USD',
    account: new PublicKey('4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo'),
    feedId:
      'e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43',
  },
  {
    name: 'ETH/USD',
    account: new PublicKey('42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC'),
    feedId:
      'ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace',
  },
];

interface DecodedPriceUpdateV2 {
  writeAuthority: PublicKey;
  verificationLevel: 'Partial' | 'Full';
  numSignatures?: number;
  feedIdHex: string;
  price: bigint;
  conf: bigint;
  exponent: number;
  publishTime: bigint;
  prevPublishTime: bigint;
  emaPrice: bigint;
  emaConf: bigint;
  postedSlot: bigint;
}

function decodePriceUpdateV2(data: Buffer): DecodedPriceUpdateV2 {
  if (!data.slice(0, 8).equals(PRICE_UPDATE_V2_DISCRIMINATOR)) {
    throw new Error('not a PriceUpdateV2 account');
  }
  const writeAuthority = new PublicKey(data.slice(8, 40));
  const tag = data[40];
  let off: number;
  let verificationLevel: 'Partial' | 'Full';
  let numSignatures: number | undefined;
  if (tag === 1) {
    verificationLevel = 'Full';
    off = 41;
  } else if (tag === 0) {
    verificationLevel = 'Partial';
    numSignatures = data[41];
    off = 42;
  } else {
    throw new Error(`unknown verification_level tag ${tag}`);
  }
  const feedIdHex = data.slice(off, off + 32).toString('hex');
  off += 32;
  const price = data.readBigInt64LE(off);
  off += 8;
  const conf = data.readBigUInt64LE(off);
  off += 8;
  const exponent = data.readInt32LE(off);
  off += 4;
  const publishTime = data.readBigInt64LE(off);
  off += 8;
  const prevPublishTime = data.readBigInt64LE(off);
  off += 8;
  const emaPrice = data.readBigInt64LE(off);
  off += 8;
  const emaConf = data.readBigUInt64LE(off);
  off += 8;
  const postedSlot = data.readBigUInt64LE(off);
  return {
    writeAuthority,
    verificationLevel,
    numSignatures,
    feedIdHex,
    price,
    conf,
    exponent,
    publishTime,
    prevPublishTime,
    emaPrice,
    emaConf,
    postedSlot,
  };
}

function scaledString(price: bigint, exponent: number): string {
  const expo = BigInt(exponent);
  if (expo >= 0n) {
    return (price * 10n ** expo).toString();
  }
  const decimals = Number(-expo);
  const sign = price < 0n ? '-' : '';
  const abs = price < 0n ? -price : price;
  const s = abs.toString().padStart(decimals + 1, '0');
  const intPart = s.slice(0, s.length - decimals);
  const fracPart = s.slice(s.length - decimals);
  return `${sign}${intPart}.${fracPart}`;
}

async function main(): Promise<void> {
  const connection = new Connection(DEVNET_RPC, 'confirmed');
  console.log(`devnet rpc: ${DEVNET_RPC}`);
  console.log(`pyth receiver: ${PYTH_RECEIVER_PROGRAM.toBase58()}`);
  console.log();

  for (const feed of FEEDS) {
    const info = await connection.getAccountInfo(feed.account, 'confirmed');
    if (!info) {
      console.error(`${feed.name}: account ${feed.account.toBase58()} not found on devnet`);
      continue;
    }
    if (!info.owner.equals(PYTH_RECEIVER_PROGRAM)) {
      console.error(
        `${feed.name}: wrong owner ${info.owner.toBase58()} (expected ${PYTH_RECEIVER_PROGRAM.toBase58()})`,
      );
      continue;
    }
    const decoded = decodePriceUpdateV2(info.data);
    if (decoded.feedIdHex !== feed.feedId) {
      console.error(
        `${feed.name}: feed_id mismatch — on-chain 0x${decoded.feedIdHex} vs expected 0x${feed.feedId}`,
      );
      continue;
    }
    const priceFmt = scaledString(decoded.price, decoded.exponent);
    const confFmt = scaledString(
      BigInt.asIntN(64, decoded.conf),
      decoded.exponent,
    );
    console.log(`${feed.name}  account=${feed.account.toBase58()}`);
    console.log(`  feed_id       0x${decoded.feedIdHex}`);
    console.log(
      `  price         ${priceFmt}  (raw=${decoded.price.toString()}, expo=${decoded.exponent})`,
    );
    console.log(`  confidence    ${confFmt}  (raw=${decoded.conf.toString()})`);
    console.log(`  publish_time  ${decoded.publishTime.toString()}`);
    console.log(`  posted_slot   ${decoded.postedSlot.toString()}`);
    console.log(
      `  verification  ${decoded.verificationLevel}${
        decoded.numSignatures !== undefined
          ? ` (n=${decoded.numSignatures})`
          : ''
      }`,
    );
    console.log();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
