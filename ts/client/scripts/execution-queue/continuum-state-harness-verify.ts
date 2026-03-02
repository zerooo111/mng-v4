import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';
import { ContinuumHarnessClient } from '../../src/continuumHarnessClient';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const PROGRAM_ID_OVERRIDE = process.env.CONTINUUM_HARNESS_PROGRAM_ID;
const VERIFY_GROUP_PK = process.env.VERIFY_GROUP_PK || '';
const VERIFY_PAYER_KEYPAIR =
  process.env.VERIFY_PAYER_KEYPAIR ||
  process.env.USER_KEYPAIR_OVERRIDE ||
  process.env.MB_PAYER_KEYPAIR ||
  '';
const HARNESS_BASE_URL =
  process.env.CONTINUUM_HARNESS_BASE_URL || 'http://127.0.0.1:9091';
const VERIFY_VIEW = (process.env.VERIFY_VIEW || 'confirmed').toLowerCase();
const VERIFY_OWNERS = process.env.VERIFY_OWNERS || '';
const VERIFY_MARKETS = process.env.VERIFY_MARKETS || '';
const VERIFY_COMPARE_CLIENT_IDS =
  (process.env.VERIFY_COMPARE_CLIENT_IDS || 'true') === 'true';
const VERIFY_FAIL_ON_MISMATCH =
  (process.env.VERIFY_FAIL_ON_MISMATCH || 'true') === 'true';

type CountMap = Map<string, number>;

type VerifyMismatch = {
  owner: string;
  market: string;
  type:
    | 'count_mismatch'
    | 'client_id_mismatch'
    | 'invalid_owner'
    | 'invalid_mango_account'
    | 'fetch_mango_account_failed';
  harness_count?: number;
  onchain_count?: number;
  harness_client_ids?: Record<string, number>;
  onchain_client_ids?: Record<string, number>;
  detail?: string;
};

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parseCsvValues(raw: string): string[] {
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function isValidPubkey(value: string): boolean {
  try {
    // eslint-disable-next-line no-new
    new PublicKey(value);
    return true;
  } catch {
    return false;
  }
}

function incrementCounter(map: CountMap, key: string): void {
  map.set(key, (map.get(key) || 0) + 1);
}

function toSortedRecord(map: CountMap): Record<string, number> {
  return Object.fromEntries(
    Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b)),
  );
}

function compareCountMaps(a: CountMap, b: CountMap): boolean {
  const keys = new Set([...a.keys(), ...b.keys()]);
  for (const key of keys) {
    if ((a.get(key) || 0) !== (b.get(key) || 0)) {
      return false;
    }
  }
  return true;
}

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }
  if (!VERIFY_PAYER_KEYPAIR) {
    throw new Error('VERIFY_PAYER_KEYPAIR (or MB_PAYER_KEYPAIR) is required');
  }
  if (!VERIFY_GROUP_PK) {
    throw new Error('VERIFY_GROUP_PK is required');
  }

  const payer = readKeypair(VERIFY_PAYER_KEYPAIR);
  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(payer),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(
    provider,
    CLUSTER,
    PROGRAM_ID_OVERRIDE
      ? new PublicKey(PROGRAM_ID_OVERRIDE)
      : MANGO_V4_ID[CLUSTER],
    { idsSource: 'get-program-accounts' },
  );

  await client.getGroup(new PublicKey(VERIFY_GROUP_PK));

  const harness = new ContinuumHarnessClient(HARNESS_BASE_URL);
  const health = await harness.healthz();
  const snapshot = await harness.getFullState(
    VERIFY_VIEW === 'confirmed' ? 'confirmed' : 'optimistic',
  );

  const ownerFilter = new Set(parseCsvValues(VERIFY_OWNERS));
  const marketFilter = new Set(parseCsvValues(VERIFY_MARKETS));

  const ownersToCheck = (ownerFilter.size
    ? Array.from(ownerFilter)
    : Object.keys(snapshot.users)
  ).sort((a, b) => a.localeCompare(b));

  const mismatches: VerifyMismatch[] = [];
  let checkedOwners = 0;
  let checkedMarkets = 0;

  for (const owner of ownersToCheck) {
    const userState = snapshot.users[owner];
    if (!userState) {
      continue;
    }

    if (!isValidPubkey(owner)) {
      mismatches.push({
        owner,
        market: '*',
        type: 'invalid_owner',
        detail: 'owner in harness snapshot is not a valid pubkey',
      });
      continue;
    }

    checkedOwners += 1;

    const harnessClientIdsByMarket = new Map<string, CountMap>();
    for (const order of userState.open_orders) {
      if (marketFilter.size && !marketFilter.has(order.market)) {
        continue;
      }
      const byMarket =
        harnessClientIdsByMarket.get(order.market) || new Map<string, number>();
      incrementCounter(byMarket, order.client_order_id);
      harnessClientIdsByMarket.set(order.market, byMarket);
    }

    const onchainClientIdsByMarket = new Map<string, CountMap>();
    for (const mangoAccountPk of userState.mango_accounts) {
      if (!isValidPubkey(mangoAccountPk)) {
        mismatches.push({
          owner,
          market: '*',
          type: 'invalid_mango_account',
          detail: `invalid mango account pubkey in harness snapshot: ${mangoAccountPk}`,
        });
        continue;
      }

      try {
        const mangoAccount = await client.getMangoAccount(new PublicKey(mangoAccountPk));
        for (const oo of mangoAccount.perpOrdersActive()) {
          const market = String(oo.orderMarket);
          if (marketFilter.size && !marketFilter.has(market)) {
            continue;
          }
          const byMarket =
            onchainClientIdsByMarket.get(market) || new Map<string, number>();
          incrementCounter(byMarket, oo.clientId.toString());
          onchainClientIdsByMarket.set(market, byMarket);
        }
      } catch (err: any) {
        mismatches.push({
          owner,
          market: '*',
          type: 'fetch_mango_account_failed',
          detail: `failed to fetch mango account ${mangoAccountPk}: ${
            err?.message || `${err}`
          }`,
        });
      }
    }

    const markets = new Set([
      ...harnessClientIdsByMarket.keys(),
      ...onchainClientIdsByMarket.keys(),
    ]);
    for (const market of markets) {
      checkedMarkets += 1;
      const harnessIds = harnessClientIdsByMarket.get(market) || new Map();
      const onchainIds = onchainClientIdsByMarket.get(market) || new Map();
      const harnessCount = Array.from(harnessIds.values()).reduce(
        (a, b) => a + b,
        0,
      );
      const onchainCount = Array.from(onchainIds.values()).reduce(
        (a, b) => a + b,
        0,
      );

      if (harnessCount !== onchainCount) {
        mismatches.push({
          owner,
          market,
          type: 'count_mismatch',
          harness_count: harnessCount,
          onchain_count: onchainCount,
          harness_client_ids: toSortedRecord(harnessIds),
          onchain_client_ids: toSortedRecord(onchainIds),
        });
        continue;
      }

      if (VERIFY_COMPARE_CLIENT_IDS && !compareCountMaps(harnessIds, onchainIds)) {
        mismatches.push({
          owner,
          market,
          type: 'client_id_mismatch',
          harness_count: harnessCount,
          onchain_count: onchainCount,
          harness_client_ids: toSortedRecord(harnessIds),
          onchain_client_ids: toSortedRecord(onchainIds),
        });
      }
    }
  }

  const output = {
    ok: mismatches.length === 0,
    mode: VERIFY_VIEW === 'confirmed' ? 'confirmed' : 'optimistic',
    harness: {
      base_url: HARNESS_BASE_URL,
      health,
    },
    checked_owners: checkedOwners,
    checked_markets: checkedMarkets,
    mismatch_count: mismatches.length,
    mismatches,
    generated_ts_ms: Date.now(),
  };

  console.log(JSON.stringify(output, null, 2));

  if (mismatches.length && VERIFY_FAIL_ON_MISMATCH) {
    process.exit(2);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
