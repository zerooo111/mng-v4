import http, { IncomingMessage, ServerResponse } from 'http';
import path from 'path';
import fs from 'fs';
import * as dotenv from 'dotenv';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { Pool } from 'pg';
import { runtimeConfigPath } from './scriptEnv';

dotenv.config();

type LaneAccountMeta = {
  pubkey: string;
  isWritable: boolean;
  isSigner?: boolean;
};

type LaneConfig = {
  name: string;
  remainingAccounts: LaneAccountMeta[];
};

type E2EConfig = {
  group?: string;
  executionQueue?: string;
  perpMarketIndex?: number;
  maker?: { owner?: string; mangoAccount?: string };
  taker?: { owner?: string; mangoAccount?: string };
  cranker?: { laneConfigPath?: string };
  relayer?: { bindAddr?: string };
};

type SubmitIntentHttpBody = {
  group: string;
  execution_queue: string;
  market: string;
  payload_b64: string;
  remaining_accounts: Array<{
    pubkey: string;
    is_signer: boolean;
    is_writable: boolean;
  }>;
  min_execute_slot?: string;
  expires_at_slot?: string;
  user_owner: string;
  mango_account: string;
  user_signature_b64: string;
};

const BRIDGE_BIND_ADDR = process.env.CTM_RELAYER_HTTP_BIND_ADDR || '127.0.0.1:9092';
const RELAYER_GRPC_ADDR = process.env.CTM_RELAYER_ADDR || '127.0.0.1:9090';
const E2E_CONFIG_PATH =
  process.env.E2E_OUTPUT_CONFIG_PATH ||
  (process.env.EXECUTION_QUEUE_GROUP_NUM
    ? runtimeConfigPath(`execution-queue-e2e-${process.env.EXECUTION_QUEUE_GROUP_NUM}.json`)
    : runtimeConfigPath('execution-queue-e2e-9101.json'));
const LANE_CONFIG_PATH_OVERRIDE = process.env.EXECUTION_QUEUE_CRANK_LANES_JSON_PATH || '';

const TSDB_ENABLED = (process.env.FERMI_TSDB_ENABLED || 'true').toLowerCase() !== 'false';
const TSDB_URL =
  process.env.FERMI_TSDB_URL || process.env.DATABASE_URL || 'postgres://postgres:postgres@127.0.0.1:5432/postgres';
const TSDB_TABLE = process.env.FERMI_TSDB_TABLE || 'fermi_trade_ticks';

function parseBind(bindAddr: string): { host: string; port: number } {
  const [hostPart, portPart] = bindAddr.split(':');
  const host = hostPart || '127.0.0.1';
  const port = Number(portPart || '9092');
  if (!Number.isFinite(port) || port <= 0) {
    throw new Error(`invalid bind port in ${bindAddr}`);
  }
  return { host, port };
}

function json(res: ServerResponse, status: number, payload: unknown): void {
  res.statusCode = status;
  res.setHeader('Content-Type', 'application/json');
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.end(JSON.stringify(payload));
}

async function readBody(req: IncomingMessage): Promise<string> {
  return await new Promise((resolve, reject) => {
    let raw = '';
    req.on('data', (chunk) => {
      raw += chunk.toString('utf-8');
      if (raw.length > 1024 * 1024) {
        reject(new Error('request body too large'));
      }
    });
    req.on('end', () => resolve(raw));
    req.on('error', reject);
  });
}

function readJsonFile<T>(filePath: string): T | null {
  try {
    if (!filePath || !fs.existsSync(filePath)) {
      return null;
    }
    return JSON.parse(fs.readFileSync(filePath, 'utf-8')) as T;
  } catch {
    return null;
  }
}

function loadRuntimeConfig(): {
  e2eConfig: E2EConfig | null;
  lanes: LaneConfig[];
  ownerToMangoAccount: Record<string, string>;
  defaults: {
    group: string | null;
    executionQueue: string | null;
    market: string;
    mangoAccount: string | null;
  };
} {
  const e2eConfig = readJsonFile<E2EConfig>(path.resolve(E2E_CONFIG_PATH));
  const lanePathFromE2E = e2eConfig?.cranker?.laneConfigPath || '';
  const laneConfigPath = LANE_CONFIG_PATH_OVERRIDE || lanePathFromE2E;
  const lanes = readJsonFile<LaneConfig[]>(path.resolve(laneConfigPath)) || [];

  const ownerToMangoAccount: Record<string, string> = {};
  if (e2eConfig?.maker?.owner && e2eConfig?.maker?.mangoAccount) {
    ownerToMangoAccount[e2eConfig.maker.owner] = e2eConfig.maker.mangoAccount;
  }
  if (e2eConfig?.taker?.owner && e2eConfig?.taker?.mangoAccount) {
    ownerToMangoAccount[e2eConfig.taker.owner] = e2eConfig.taker.mangoAccount;
  }

  const defaults = {
    group: e2eConfig?.group || process.env.EXECUTION_QUEUE_GROUP_PK || null,
    executionQueue:
      e2eConfig?.executionQueue || process.env.EXECUTION_QUEUE_PK || null,
    market: String(
      e2eConfig?.perpMarketIndex ??
        process.env.PERP_MARKET_INDEX ??
        process.env.MARKET_OVERRIDE ??
        '0',
    ),
    mangoAccount:
      process.env.RELAYER_BRIDGE_DEFAULT_MANGO_ACCOUNT ||
      e2eConfig?.maker?.mangoAccount ||
      null,
  };

  return { e2eConfig, lanes, ownerToMangoAccount, defaults };
}

function createRelayerClient() {
  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  return new proto.ctmsequencer.CtmSequencerRelayer(
    RELAYER_GRPC_ADDR,
    grpc.credentials.createInsecure(),
  );
}

async function submitIntentViaGrpc(
  relayerClient: any,
  body: SubmitIntentHttpBody,
): Promise<any> {
  const payload = Buffer.from(body.payload_b64, 'base64');
  const signature = Buffer.from(body.user_signature_b64, 'base64');

  return await new Promise<any>((resolve, reject) => {
    relayerClient.submitIntent(
      {
        group: body.group,
        execution_queue: body.execution_queue,
        market: body.market,
        payload,
        remaining_accounts: body.remaining_accounts,
        min_execute_slot: body.min_execute_slot || '0',
        expires_at_slot: body.expires_at_slot || '0',
        user_owner: body.user_owner,
        mango_account: body.mango_account,
        user_signature: signature,
      },
      (err: Error | null, res: any) => {
        if (err) {
          reject(err);
          return;
        }
        resolve(res);
      },
    );
  });
}

function tfToInterval(tf: string): string {
  switch (tf) {
    case '1m':
      return '1 minute';
    case '5m':
      return '5 minutes';
    case '15m':
      return '15 minutes';
    case '1h':
      return '1 hour';
    case '4h':
      return '4 hours';
    case '1d':
      return '1 day';
    default:
      return '1 hour';
  }
}

async function ensureTimescale(pool: Pool): Promise<void> {
  await pool.query(
    `CREATE TABLE IF NOT EXISTS ${TSDB_TABLE} (
      ts TIMESTAMPTZ NOT NULL,
      market TEXT NOT NULL,
      price DOUBLE PRECISION NOT NULL,
      size DOUBLE PRECISION NULL,
      source TEXT NOT NULL DEFAULT 'frontend'
    )`,
  );
  try {
    await pool.query(`CREATE EXTENSION IF NOT EXISTS timescaledb`);
  } catch {
    // Extension creation can fail on managed/local setups without superuser.
  }
  try {
    await pool.query(
      `SELECT create_hypertable('${TSDB_TABLE}', 'ts', if_not_exists => TRUE)`,
    );
  } catch {
    // If timescaledb extension is missing, the table remains a regular table.
  }
  await pool.query(
    `CREATE INDEX IF NOT EXISTS ${TSDB_TABLE}_market_ts_idx ON ${TSDB_TABLE} (market, ts DESC)`,
  );
}

async function ingestTick(
  pool: Pool,
  market: string,
  price: number,
  size: number | null,
  tsMs: number,
  source: string,
): Promise<void> {
  await pool.query(
    `INSERT INTO ${TSDB_TABLE} (ts, market, price, size, source)
     VALUES (to_timestamp($1::double precision / 1000.0), $2, $3, $4, $5)`,
    [tsMs, market, price, size, source],
  );
}

async function fetchCandles(
  pool: Pool,
  market: string,
  tf: string,
  fromMs: number,
  toMs: number,
  limit: number,
): Promise<Array<[number, number, number, number, number]>> {
  const interval = tfToInterval(tf);
  const result = await pool.query(
    `WITH bucketed AS (
      SELECT
        time_bucket($1::interval, ts) AS bucket,
        ts,
        price
      FROM ${TSDB_TABLE}
      WHERE market = $2
        AND source <> 'frontend-intent'
        AND ts >= to_timestamp($3::double precision / 1000.0)
        AND ts <= to_timestamp($4::double precision / 1000.0)
    ),
    stats AS (
      SELECT
        bucket,
        max(price) AS high,
        min(price) AS low
      FROM bucketed
      GROUP BY bucket
    ),
    opened AS (
      SELECT DISTINCT ON (bucket)
        bucket,
        price AS open
      FROM bucketed
      ORDER BY bucket, ts ASC
    ),
    closed AS (
      SELECT DISTINCT ON (bucket)
        bucket,
        price AS close
      FROM bucketed
      ORDER BY bucket, ts DESC
    ),
    merged AS (
      SELECT
        s.bucket,
        o.open,
        s.high,
        s.low,
        c.close
      FROM stats s
      JOIN opened o USING (bucket)
      JOIN closed c USING (bucket)
      ORDER BY s.bucket DESC
      LIMIT $5
    )
    SELECT
      (EXTRACT(EPOCH FROM bucket) * 1000)::bigint AS ts_ms,
      open,
      high,
      low,
      close
    FROM merged
    ORDER BY ts_ms ASC`,
    [interval, market, fromMs, toMs, limit],
  );

  return result.rows.map((row) => [
    Number(row.ts_ms),
    Number(row.open),
    Number(row.high),
    Number(row.low),
    Number(row.close),
  ]);
}

async function main(): Promise<void> {
  const relayerClient = createRelayerClient();
  const runtime = loadRuntimeConfig();
  let tsdbPool: Pool | null =
    TSDB_ENABLED
      ? new Pool({
          connectionString: TSDB_URL,
        })
      : null;

  if (tsdbPool) {
    try {
      await ensureTimescale(tsdbPool);
    } catch (err: any) {
      console.warn(
        JSON.stringify(
          {
            msg: 'timescaledb unavailable, running bridge without candle storage',
            error: err?.message || String(err),
            tsdb_url: TSDB_URL,
          },
          null,
          2,
        ),
      );
      try {
        await tsdbPool.end();
      } catch {
        // ignore cleanup failure when startup already failed
      }
      tsdbPool = null;
    }
  }

  const server = http.createServer(async (req, res) => {
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization');
    res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
    if (req.method === 'OPTIONS') {
      res.statusCode = 204;
      res.end();
      return;
    }

    if (!req.url) {
      json(res, 400, { error: 'invalid request url' });
      return;
    }

    const url = new URL(req.url, 'http://localhost');
    const pathname = url.pathname;

    if (req.method === 'GET' && pathname === '/healthz') {
      json(res, 200, {
        ok: true,
        relayer_grpc_addr: RELAYER_GRPC_ADDR,
        e2e_config_path: E2E_CONFIG_PATH,
        lanes: runtime.lanes.length,
        tsdb_enabled: !!tsdbPool,
      });
      return;
    }

    if (req.method === 'GET' && pathname === '/relay/config') {
      const owner = url.searchParams.get('owner') || '';
      const mangoAccount =
        (owner && runtime.ownerToMangoAccount[owner]) || runtime.defaults.mangoAccount;

      json(res, 200, {
        group: runtime.defaults.group,
        execution_queue: runtime.defaults.executionQueue,
        market: runtime.defaults.market,
        mango_account: mangoAccount,
        owner_to_mango_account: runtime.ownerToMangoAccount,
        lanes: runtime.lanes.map((lane) => ({
          name: lane.name,
          remaining_accounts: lane.remainingAccounts.map((m) => ({
            pubkey: m.pubkey,
            is_signer: !!m.isSigner,
            is_writable: !!m.isWritable,
          })),
        })),
      });
      return;
    }

    if (req.method === 'POST' && pathname === '/relay/submit-intent') {
      try {
        const raw = await readBody(req);
        const body = JSON.parse(raw) as SubmitIntentHttpBody;
        if (
          !body.group ||
          !body.execution_queue ||
          !body.market ||
          !body.payload_b64 ||
          !body.user_owner ||
          !body.mango_account ||
          !body.user_signature_b64
        ) {
          json(res, 400, { error: 'missing required fields' });
          return;
        }

        const relayResponse = await submitIntentViaGrpc(relayerClient, body);
        json(res, 200, {
          sequence: String(relayResponse.sequence),
          tx_signature: relayResponse.tx_signature,
          user_intent_message_b64: Buffer.from(relayResponse.user_intent_message || []).toString(
            'base64',
          ),
          ctm_envelope_message_b64: Buffer.from(
            relayResponse.ctm_envelope_message || [],
          ).toString('base64'),
        });
      } catch (err: any) {
        json(res, 500, {
          error: err?.message || 'relay submission failed',
        });
      }
      return;
    }

    if (req.method === 'POST' && pathname === '/candles/ingest') {
      if (!tsdbPool) {
        json(res, 503, { error: 'timescaledb disabled' });
        return;
      }
      try {
        const raw = await readBody(req);
        const body = JSON.parse(raw) as {
          market: string;
          price: number;
          size?: number;
          timestamp_ms?: number;
          source?: string;
        };
        if (!body.market || !Number.isFinite(body.price)) {
          json(res, 400, { error: 'market and price are required' });
          return;
        }
        await ingestTick(
          tsdbPool,
          body.market,
          Number(body.price),
          Number.isFinite(body.size) ? Number(body.size) : null,
          Number.isFinite(body.timestamp_ms) ? Number(body.timestamp_ms) : Date.now(),
          body.source || 'frontend',
        );
        json(res, 202, { ok: true });
      } catch (err: any) {
        json(res, 500, { error: err?.message || 'failed to ingest candle tick' });
      }
      return;
    }

    if (req.method === 'GET' && pathname.startsWith('/candles/')) {
      if (!tsdbPool) {
        json(res, 503, { error: 'timescaledb disabled' });
        return;
      }
      try {
        const market = decodeURIComponent(pathname.replace('/candles/', '')).trim();
        if (!market) {
          json(res, 400, { error: 'market is required' });
          return;
        }
        const tf = url.searchParams.get('tf') || '1h';
        const limit = Math.min(Number(url.searchParams.get('limit') || '500'), 2000);
        const toMs = Number(url.searchParams.get('to') || `${Date.now()}`);
        const fromMs =
          Number(url.searchParams.get('from') || `${toMs - 30 * 24 * 60 * 60 * 1000}`);
        const candles = await fetchCandles(tsdbPool, market, tf, fromMs, toMs, limit);
        json(res, 200, candles);
      } catch (err: any) {
        json(res, 500, { error: err?.message || 'failed to fetch candles' });
      }
      return;
    }

    json(res, 404, { error: 'not found' });
  });

  const { host, port } = parseBind(BRIDGE_BIND_ADDR);
  server.listen(port, host, () => {
    console.log(
      JSON.stringify(
        {
          msg: 'ctm-relayer-http-bridge listening',
          bind: `${host}:${port}`,
          relayer_grpc: RELAYER_GRPC_ADDR,
          tsdb_enabled: !!tsdbPool,
          tsdb_table: TSDB_TABLE,
        },
        null,
        2,
      ),
    );
  });
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
