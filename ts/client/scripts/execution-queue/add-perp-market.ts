import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { createMint } from '@solana/spl-token';
import {
  Cluster,
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import os from 'os';
import path from 'path';
import { PerpMarketIndex } from '../../src/accounts/perp';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';
import {
  defaultClusterUrl,
  runtimeConfigPath,
} from './scriptEnv';

dotenv.config();

const CLUSTER: Cluster = (process.env.CLUSTER_OVERRIDE as Cluster) || 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || defaultClusterUrl();
const ADMIN_KEYPAIR =
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  process.env.MB_PAYER_KEYPAIR ||
  path.join(os.homedir(), '.config/solana/id.json');
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;
const GROUP_NUM = Number(
  process.env.EXECUTION_QUEUE_GROUP_NUM || process.env.GROUP_NUM || '9101',
);
const DEFAULT_PERP_BASE_DECIMALS = 6;
const DEFAULT_PERP_BASE_LOT_SIZE = 100;
const DEFAULT_PERP_QUOTE_LOT_SIZE = 1;
type OracleMode = 'stub' | 'pyth';
type PerpMarketPrecision = {
  baseDecimals: number;
  baseLotSize: number;
  quoteLotSize: number;
};

const PYTH_SPONSORED_FEED: Record<string, PublicKey> = {
  SOL: new PublicKey('7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE'),
  BTC: new PublicKey('4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo'),
  ETH: new PublicKey('42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC'),
};

const PYTH_ORACLE_CONFIG = {
  confFilter: 0.1,
  maxStalenessSlots: 600,
} as const;

type Cli = {
  symbol: string;
  price: number;
  index: number | null;
  baseDecimals: number;
  baseLotSize: number;
  quoteLotSize: number;
  mintDecimals: number;
  oracleMode: OracleMode | null;
  pythFeed: string | null;
};

function defaultPrecisionForSymbol(symbol: string): PerpMarketPrecision {
  switch (symbol) {
    case 'SOL':
      return {
        baseDecimals: 5,
        baseLotSize: 100,
        quoteLotSize: 1,
      };
    case 'BTC':
      return {
        baseDecimals: 6,
        baseLotSize: 100,
        quoteLotSize: 1,
      };
    default:
      return {
        baseDecimals: DEFAULT_PERP_BASE_DECIMALS,
        baseLotSize: DEFAULT_PERP_BASE_LOT_SIZE,
        quoteLotSize: DEFAULT_PERP_QUOTE_LOT_SIZE,
      };
  }
}

function parseCli(): Cli {
  const args = process.argv.slice(2);
  const out: Cli = {
    symbol: '',
    price: Number.NaN,
    index: null,
    baseDecimals: DEFAULT_PERP_BASE_DECIMALS,
    baseLotSize: DEFAULT_PERP_BASE_LOT_SIZE,
    quoteLotSize: DEFAULT_PERP_QUOTE_LOT_SIZE,
    mintDecimals: 9,
    oracleMode: null,
    pythFeed: null,
  };
  let baseDecimalsOverridden = false;
  let baseLotSizeOverridden = false;
  let quoteLotSizeOverridden = false;
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    const take = (): string => {
      const v = args[i + 1];
      if (v === undefined) {
        throw new Error(`missing value for ${a}`);
      }
      i++;
      return v;
    };
    switch (a) {
      case '--symbol':
        out.symbol = take().trim().toUpperCase();
        break;
      case '--price':
        out.price = Number(take());
        break;
      case '--index':
        out.index = Number(take());
        break;
      case '--base-decimals':
        out.baseDecimals = Number(take());
        baseDecimalsOverridden = true;
        break;
      case '--base-lot-size':
        out.baseLotSize = Number(take());
        baseLotSizeOverridden = true;
        break;
      case '--quote-lot-size':
        out.quoteLotSize = Number(take());
        quoteLotSizeOverridden = true;
        break;
      case '--mint-decimals':
        out.mintDecimals = Number(take());
        break;
      case '--oracle-mode': {
        const mode = take().trim().toLowerCase();
        if (mode !== 'stub' && mode !== 'pyth') {
          throw new Error(`invalid --oracle-mode ${mode}`);
        }
        out.oracleMode = mode as OracleMode;
        break;
      }
      case '--pyth-feed':
        out.pythFeed = take().trim();
        break;
      default:
        throw new Error(`unknown arg ${a}`);
    }
  }
  if (!out.symbol) throw new Error('--symbol is required');
  if (!Number.isFinite(out.price) || out.price <= 0) {
    throw new Error('--price must be a positive number');
  }
  const precision = defaultPrecisionForSymbol(out.symbol);
  if (!baseDecimalsOverridden) {
    out.baseDecimals = precision.baseDecimals;
  }
  if (!baseLotSizeOverridden) {
    out.baseLotSize = precision.baseLotSize;
  }
  if (!quoteLotSizeOverridden) {
    out.quoteLotSize = precision.quoteLotSize;
  }
  if (out.oracleMode === null) {
    out.oracleMode = PYTH_SPONSORED_FEED[out.symbol] ? 'pyth' : 'stub';
  }
  if (out.oracleMode === 'pyth' && !out.pythFeed) {
    const sponsored = PYTH_SPONSORED_FEED[out.symbol];
    if (!sponsored) {
      throw new Error(
        `no default Pyth sponsored feed for ${out.symbol}; pass --pyth-feed or --oracle-mode stub`,
      );
    }
    out.pythFeed = sponsored.toBase58();
  }
  return out;
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function resolveMarketIndex(
  group: Awaited<ReturnType<MangoClient['getGroup']>>,
  requested: number | null,
): PerpMarketIndex {
  const taken = new Set<number>();
  for (const [idx] of group.perpMarketsMapByMarketIndex.entries()) {
    taken.add(Number(idx));
  }
  if (requested !== null) {
    if (taken.has(requested)) {
      throw new Error(
        `market index ${requested} already taken by ${group.perpMarketsMapByMarketIndex
          .get(requested as PerpMarketIndex)
          ?.name}`,
      );
    }
    return requested as PerpMarketIndex;
  }
  let next = 0;
  while (taken.has(next)) next++;
  return next as PerpMarketIndex;
}

function perpMarketsAuditPath(): string {
  return runtimeConfigPath(`perp-markets-${GROUP_NUM}.json`);
}

function appendAuditEntry(entry: Record<string, unknown>): void {
  const p = perpMarketsAuditPath();
  fs.mkdirSync(path.dirname(p), { recursive: true });
  let existing: Record<string, unknown>[] = [];
  if (fs.existsSync(p)) {
    try {
      const parsed = JSON.parse(fs.readFileSync(p, 'utf-8'));
      if (Array.isArray(parsed)) existing = parsed;
    } catch {
      existing = [];
    }
  }
  existing.push(entry);
  fs.writeFileSync(p, JSON.stringify(existing, null, 2));
}

async function main(): Promise<void> {
  const cli = parseCli();
  const admin = readKeypair(ADMIN_KEYPAIR);

  const connection = new Connection(CLUSTER_URL, AnchorProvider.defaultOptions());
  const provider = new AnchorProvider(
    connection,
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];

  const adminClient = await MangoClient.connect(provider, CLUSTER, programId, {
    idsSource: 'get-program-accounts',
  });

  const groupNumBuf = Buffer.alloc(4);
  groupNumBuf.writeUInt32LE(GROUP_NUM);
  const [groupPk] = PublicKey.findProgramAddressSync(
    [Buffer.from('Group'), admin.publicKey.toBuffer(), groupNumBuf],
    programId,
  );

  const group = await adminClient.getGroup(groupPk);
  const marketIndex = resolveMarketIndex(group, cli.index);
  const marketName = `${cli.symbol}-PERP`;

  console.error(
    `[add-perp-market] group=${groupPk.toBase58()} index=${marketIndex} name=${marketName} price=${cli.price}`,
  );

  // 1) Create synthetic base mint.
  const baseMint = await createMint(
    connection,
    admin,
    admin.publicKey,
    null,
    cli.mintDecimals,
  );
  console.error(`[add-perp-market] created base mint ${baseMint.toBase58()}`);

  // 2) Select the market oracle.
  let oraclePk: PublicKey;
  let oracleConfig: { confFilter: number; maxStalenessSlots: number | null };
  if (cli.oracleMode === 'pyth') {
    oraclePk = new PublicKey(cli.pythFeed!);
    oracleConfig = { ...PYTH_ORACLE_CONFIG };
    console.error(
      `[add-perp-market] using sponsored pyth oracle ${oraclePk.toBase58()}`,
    );
  } else {
    const existingOracles = await adminClient.getStubOracle(group, baseMint);
    if (existingOracles.length > 0) {
      oraclePk = existingOracles[0].publicKey;
      console.error(
        `[add-perp-market] reusing existing stub oracle ${oraclePk.toBase58()}`,
      );
    } else {
      await adminClient.stubOracleCreate(group, baseMint, cli.price);
      const oracles = await adminClient.getStubOracle(group, baseMint);
      if (oracles.length === 0) {
        throw new Error('stub oracle creation did not yield an account');
      }
      oraclePk = oracles[0].publicKey;
      console.error(
        `[add-perp-market] created stub oracle ${oraclePk.toBase58()}`,
      );
    }
    oracleConfig = { confFilter: 0.1, maxStalenessSlots: null };
  }

  // 3) Create the perp market. Risk params mirror SOL-PERP bootstrap.
  const sig = await adminClient.perpCreateMarket(
    group,
    oraclePk,
    marketIndex,
    marketName,
    oracleConfig,
    cli.baseDecimals,
    cli.quoteLotSize,
    cli.baseLotSize,
    0.9, // maintBaseAssetWeight
    0.8, // initBaseAssetWeight
    1.1, // maintBaseLiabWeight
    1.2, // initBaseLiabWeight
    0.0, // maintOverallAssetWeight
    0.0, // initOverallAssetWeight
    0.05, // baseLiquidationFee
    -0.001, // makerFee
    0.002, // takerFee
    0, // feePenalty
    -0.1, // minFunding
    0.1, // maxFunding
    10, // impactQuantity
    false, // groupInsuranceFund
    0, // settleFeeFlat
    0, // settleFeeAmountThreshold
    0, // settleFeeFractionLowHealth
    0, // settleTokenIndex (USDC)
    -1.0, // settlePnlLimitFactor
    2 * 60 * 60, // settlePnlLimitWindowSize
    0.025, // positivePnlLiquidationFee
    0.0, // platformLiquidationFee
  );

  // Reload and report the PDAs anchor assigned to the new market.
  await group.reloadAll(adminClient);
  const perpMarket = group.perpMarketsMapByMarketIndex.get(marketIndex);
  if (!perpMarket) {
    throw new Error(
      `perpCreateMarket returned but market index ${marketIndex} not found after reload`,
    );
  }

  const result = {
    cluster: CLUSTER,
    groupNum: GROUP_NUM,
    group: groupPk.toBase58(),
    programId: programId.toBase58(),
    marketIndex,
    name: marketName,
    baseMint: baseMint.toBase58(),
    oracle: oraclePk.toBase58(),
    oracleMode: cli.oracleMode,
    pythFeed: cli.oracleMode === 'pyth' ? cli.pythFeed : null,
    perpMarket: perpMarket.publicKey.toBase58(),
    bids: perpMarket.bids.toBase58(),
    asks: perpMarket.asks.toBase58(),
    eventQueue: perpMarket.eventQueue.toBase58(),
    baseDecimals: cli.baseDecimals,
    baseLotSize: cli.baseLotSize,
    quoteLotSize: cli.quoteLotSize,
    priceTickUi:
      (cli.quoteLotSize * Math.pow(10, cli.baseDecimals - 6)) /
      cli.baseLotSize,
    initialPrice: cli.price,
    signature: sig?.signature ?? null,
    createdAtMs: Date.now(),
  };

  appendAuditEntry(result);

  // stdout is the machine-readable blob
  console.log(JSON.stringify(result, null, 2));
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(err);
    process.exit(1);
  });
