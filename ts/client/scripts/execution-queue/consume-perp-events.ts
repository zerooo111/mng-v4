import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { MangoClient } from '../../src/client';
import { PerpMarketIndex } from '../../src/accounts/perp';
import { runtimeConfigPath } from './scriptEnv';

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

const CONFIG_PATH =
  process.env.QUOTER_CONFIG_PATH ||
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  runtimeConfigPath('execution-queue-e2e-9120.json');
const PAYER_KEYPAIR =
  process.env.USER_KEYPAIR_OVERRIDE ||
  process.env.MB_PAYER_KEYPAIR ||
  '/home/ec2-user/.config/solana/id.json';
const MARKET_INDEX_OVERRIDE = process.env.PERP_MARKET_INDEX
  ? Number(process.env.PERP_MARKET_INDEX)
  : undefined;
const MAX_PASSES = Number(process.env.PERP_CONSUME_MAX_PASSES || '200');

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

async function main(): Promise<void> {
  const config = JSON.parse(fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8')) as E2EConfig;
  const clusterUrl = process.env.CLUSTER_URL_OVERRIDE || config.clusterUrl;
  const kp = readKeypair(PAYER_KEYPAIR);
  const provider = new AnchorProvider(
    new Connection(clusterUrl, AnchorProvider.defaultOptions()),
    new Wallet(kp),
    AnchorProvider.defaultOptions(),
  );

  const programId = new PublicKey(config.programId);
  const groupPk = new PublicKey(config.group);
  const marketIndex = (MARKET_INDEX_OVERRIDE ?? config.perpMarketIndex) as PerpMarketIndex;

  const client = await MangoClient.connect(provider, config.cluster, programId, {
    idsSource: 'get-program-accounts',
  });
  const group = await client.getGroup(groupPk);
  const perpMarket = group.getPerpMarketByMarketIndex(marketIndex);

  for (let pass = 0; pass < MAX_PASSES; pass++) {
    const eq = await perpMarket.loadEventQueue(client);
    const before = eq.getUnconsumedEvents().length;
    if (before === 0) {
      console.log(JSON.stringify({ msg: 'consume-perp-events:done', pass, remaining: 0 }));
      return;
    }

    await client.perpConsumeAllEvents(group, marketIndex);

    const afterEq = await perpMarket.loadEventQueue(client);
    const after = afterEq.getUnconsumedEvents().length;
    console.log(
      JSON.stringify({
        msg: 'consume-perp-events:pass',
        pass,
        before,
        after,
      }),
    );

    if (after >= before) {
      console.log(
        JSON.stringify({
          msg: 'consume-perp-events:stalled',
          pass,
          before,
          after,
        }),
      );
      return;
    }
  }

  const eq = await perpMarket.loadEventQueue(client);
  console.log(
    JSON.stringify({
      msg: 'consume-perp-events:max-passes-reached',
      maxPasses: MAX_PASSES,
      remaining: eq.getUnconsumedEvents().length,
    }),
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
