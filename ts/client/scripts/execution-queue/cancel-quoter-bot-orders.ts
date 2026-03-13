import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { MangoClient } from '../../src/client';

dotenv.config();

type E2EConfig = {
  cluster: Cluster;
  clusterUrl: string;
  programId: string;
  group: string;
  perpMarketIndex: number;
  relayer?: {
    payerKeypairPath?: string;
  };
};

type QuoterBotSpec = {
  name: string;
  keypairPath: string;
  owner: string;
  mangoAccount: string;
};

const CONFIG_PATH =
  process.env.QUOTER_CONFIG_PATH ||
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  '/home/ec2-user/stagin4/mng-v4/.localnet/run/execution-queue-e2e-9120.json';
const BOTS_PATH =
  process.env.QUOTER_BOTS_OUTPUT_PATH ||
  process.env.QUOTER_BOTS_JSON_PATH ||
  '/home/ec2-user/stagin4/mng-v4/.localnet/run/quoter-bots-9120.json';
const LIMIT_PER_TX = Number(process.env.QUOTER_CANCEL_LIMIT_PER_TX || '10');
const MAX_ROUNDS = Number(process.env.QUOTER_CANCEL_MAX_ROUNDS || '100');
const MARKET_INDEX_OVERRIDE = process.env.PERP_MARKET_INDEX
  ? Number(process.env.PERP_MARKET_INDEX)
  : undefined;
const BOT_NAME_FILTER = (process.env.QUOTER_CANCEL_BOT_NAME || '').trim();
const BOT_OWNER_FILTER = (process.env.QUOTER_CANCEL_OWNER || '').trim();
const CONSUME_EVENTS_EACH_ROUND =
  (process.env.QUOTER_CANCEL_CONSUME_EVENTS_EACH_ROUND || 'true') === 'true';

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

async function main(): Promise<void> {
  const config = JSON.parse(fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8')) as E2EConfig;
  const bots = JSON.parse(fs.readFileSync(path.resolve(BOTS_PATH), 'utf-8')) as QuoterBotSpec[];
  const clusterUrl = process.env.CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const programId = new PublicKey(config.programId);
  const groupPk = new PublicKey(config.group);
  const marketIndex = MARKET_INDEX_OVERRIDE ?? config.perpMarketIndex;

  if (!Number.isFinite(marketIndex)) {
    throw new Error('invalid perpMarketIndex in config/env');
  }

  console.log(
    JSON.stringify({
      msg: 'cancel-bot-orders:start',
      configPath: path.resolve(CONFIG_PATH),
      botsPath: path.resolve(BOTS_PATH),
      botCount: bots.length,
      clusterUrl,
      group: groupPk.toBase58(),
      marketIndex,
      limitPerTx: LIMIT_PER_TX,
      maxRounds: MAX_ROUNDS,
    }),
  );

  const selectedBots = bots.filter((bot) => {
    if (BOT_NAME_FILTER.length && bot.name !== BOT_NAME_FILTER) {
      return false;
    }
    if (BOT_OWNER_FILTER.length && bot.owner !== BOT_OWNER_FILTER) {
      return false;
    }
    return true;
  });
  if (!selectedBots.length) {
    throw new Error('no bots matched QUOTER_CANCEL_BOT_NAME/QUOTER_CANCEL_OWNER filter');
  }

  for (const bot of selectedBots) {
    const kp = readKeypair(bot.keypairPath);
    const provider = new AnchorProvider(
      new Connection(clusterUrl, AnchorProvider.defaultOptions()),
      new Wallet(kp),
      AnchorProvider.defaultOptions(),
    );
    const client = await MangoClient.connect(provider, config.cluster, programId, {
      idsSource: 'get-program-accounts',
    });
    const group = await client.getGroup(groupPk);
    const mangoPk = new PublicKey(bot.mangoAccount);
    let mangoAccount = await client.getMangoAccount(mangoPk);

    let canceledTx = 0;
    for (let round = 0; round < MAX_ROUNDS; round++) {
      const openOrders = await mangoAccount.loadPerpOpenOrdersForMarket(
        client,
        group,
        marketIndex,
      );
      if (openOrders.length === 0) {
        break;
      }
      const sig = await client.perpCancelAllOrders(group, mangoAccount, marketIndex, LIMIT_PER_TX);
      canceledTx += 1;
      if (CONSUME_EVENTS_EACH_ROUND) {
        await client.perpConsumeAllEvents(group, marketIndex);
      }
      console.log(
        JSON.stringify({
          msg: 'cancel-bot-orders:tx',
          bot: bot.name,
          owner: kp.publicKey.toBase58(),
          mangoAccount: mangoPk.toBase58(),
          round,
          openOrdersBefore: openOrders.length,
          sig,
        }),
      );
      mangoAccount = await client.getMangoAccount(mangoPk);
    }

    const remaining = (
      await mangoAccount.loadPerpOpenOrdersForMarket(client, group, marketIndex)
    ).length;
    console.log(
      JSON.stringify({
        msg: 'cancel-bot-orders:done',
        bot: bot.name,
        owner: kp.publicKey.toBase58(),
        mangoAccount: mangoPk.toBase58(),
        canceledTx,
        remaining,
      }),
    );
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
