import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Cluster,
  Connection,
  Keypair,
  PublicKey,
  SYSVAR_INSTRUCTIONS_PUBKEY,
} from '@solana/web3.js';
import crypto from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import { Group } from '../../src/accounts/group';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';
import {
  decodeExecutionQueueCount,
  decodeExecutionQueueHeadItem,
  decodeExecutionQueueNextSequence,
} from '../../src/executionQueueLayout';

dotenv.config();

const WS_HANDSHAKE_405 = 'Unexpected server response: 405';

function shouldIgnoreBackgroundWsHandshakeError(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : `${err}`;
  return msg.includes(WS_HANDSHAKE_405);
}

process.on('uncaughtException', (err) => {
  if (shouldIgnoreBackgroundWsHandshakeError(err)) {
    console.error(
      `ignoring background websocket handshake error: ${err instanceof Error ? err.message : err}`,
    );
    return;
  }
  console.error(err);
  process.exit(1);
});

process.on('unhandledRejection', (err) => {
  if (shouldIgnoreBackgroundWsHandshakeError(err)) {
    console.error(
      `ignoring background websocket rejection: ${err instanceof Error ? err.message : err}`,
    );
    return;
  }
  console.error('unhandled rejection in cranker:', err);
});

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const CLUSTER_WS_URL =
  process.env.CLUSTER_WS_URL_OVERRIDE || process.env.MB_CLUSTER_WS_URL || '';
const CRANKER_KEYPAIR =
  process.env.EXECUTION_QUEUE_CRANKER_KEYPAIR ||
  process.env.USER_KEYPAIR_OVERRIDE ||
  process.env.MB_PAYER_KEYPAIR;
const GROUP_PK = process.env.EXECUTION_QUEUE_GROUP_PK;
const EXECUTION_QUEUE_PK = process.env.EXECUTION_QUEUE_PK;
const EXECUTION_QUEUE_BUFFER_PK = process.env.EXECUTION_QUEUE_BUFFER_PK;
const CRANK_INTERVAL_MS = Number(
  process.env.EXECUTION_QUEUE_CRANK_INTERVAL_MS ?? '1500',
);
const CRANK_MAX_ITEMS = Number(process.env.EXECUTION_QUEUE_CRANK_MAX_ITEMS ?? '4');
const CRANK_PRIORITIZATION_FEE = Number(
  process.env.EXECUTION_QUEUE_CRANK_PRIORITIZATION_FEE ?? '0',
);
const CRANK_CONFIRM_IN_BACKGROUND =
  (process.env.EXECUTION_QUEUE_CRANK_CONFIRM_IN_BACKGROUND || 'true') === 'true';
const CRANK_SKIP_PREFLIGHT =
  (process.env.EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT || 'true') === 'true';
const CRANK_SKIP_CONFIRMATION =
  (process.env.EXECUTION_QUEUE_CRANK_SKIP_CONFIRMATION || 'true') === 'true';
const CRANK_LANES_JSON = process.env.EXECUTION_QUEUE_CRANK_LANES_JSON || '[]';
const CRANK_LANES_JSON_PATH = process.env.EXECUTION_QUEUE_CRANK_LANES_JSON_PATH || '';
const CRANK_RELAY_EVENT_LOG_PATH =
  process.env.EXECUTION_QUEUE_CRANK_RELAY_EVENT_LOG_PATH || '';
const CRANK_DYNAMIC_LANES_REFRESH_MS = Number(
  process.env.EXECUTION_QUEUE_CRANK_DYNAMIC_LANES_REFRESH_MS ?? '5000',
);
const CRANK_DYNAMIC_LANES_MAX_EVENTS = Number(
  process.env.EXECUTION_QUEUE_CRANK_DYNAMIC_LANES_MAX_EVENTS ?? '5000',
);
const CRANK_STALL_WARN_LOOPS = Number(
  process.env.EXECUTION_QUEUE_CRANK_STALL_WARN_LOOPS ?? '10',
);
const CRANK_PARALLEL_LANES =
  (process.env.EXECUTION_QUEUE_CRANK_PARALLEL_LANES || 'true') === 'true';
const CRANK_LANE_FAILURE_THRESHOLD = Number(
  process.env.EXECUTION_QUEUE_CRANK_LANE_FAILURE_THRESHOLD ?? '3',
);
const CRANK_LANE_FAILURE_BACKOFF_MS = Number(
  process.env.EXECUTION_QUEUE_CRANK_LANE_FAILURE_BACKOFF_MS ?? '10000',
);
const CRANK_INCLUDE_LEGACY_FIXED_HASH =
  (process.env.EXECUTION_QUEUE_CRANK_INCLUDE_LEGACY_FIXED_HASH || 'true') === 'true';
const CRANK_MATCH_HEAD_ONLY =
  (process.env.EXECUTION_QUEUE_CRANK_MATCH_HEAD_ONLY || 'true') === 'true';
const PROGRAM_ID_OVERRIDE = process.env.EXECUTION_QUEUE_PROGRAM_ID;
const CRANK_DEBUG_HEAD_HASH = (
  process.env.EXECUTION_QUEUE_CRANK_DEBUG_HEAD_HASH || ''
).toLowerCase();

type LaneConfig = {
  name?: string;
  remainingAccounts: Array<{
    pubkey: string;
    isWritable: boolean;
    isSigner?: boolean;
  }>;
};

type RelayIntentAcceptedEvent = {
  event_type: 'relay_intent_accepted';
  group: string;
  execution_queue: string;
  market?: string;
  sequence?: string;
  user_owner?: string;
  remaining_accounts?: Array<{
    pubkey: string;
    is_writable: boolean;
    is_signer: boolean;
  }>;
};

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parseLaneConfigs(): LaneConfig[] {
  const raw = CRANK_LANES_JSON_PATH
    ? fs.readFileSync(path.resolve(CRANK_LANES_JSON_PATH), 'utf-8')
    : CRANK_LANES_JSON;
  return JSON.parse(raw) as LaneConfig[];
}

function toAccountMetas(config: LaneConfig): AccountMeta[] {
  return config.remainingAccounts.map((a) => ({
    pubkey: new PublicKey(a.pubkey),
    isWritable: !!a.isWritable,
    isSigner: !!a.isSigner,
  }));
}

function laneHash(config: LaneConfig): string {
  const encoded: Buffer[] = [];
  for (const account of config.remainingAccounts) {
    encoded.push(new PublicKey(account.pubkey).toBuffer());
    encoded.push(Buffer.from([account.isSigner ? 1 : 0]));
    encoded.push(Buffer.from([account.isWritable ? 1 : 0]));
  }
  return crypto.createHash('sha256').update(Buffer.concat(encoded)).digest('hex');
}

function normalizeLaneRuntimeFlags(
  lane: LaneConfig,
  _groupPk: PublicKey,
  _executionQueuePk: PublicKey,
): LaneConfig {
  // ExecutionQueue accounts_hash is computed from dispatch remaining accounts only.
  // Duplicate metas in that list are OR-merged by the runtime before instruction dispatch.
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const a of lane.remainingAccounts) {
    const key = a.pubkey;
    const prev = merged.get(key);
    if (!prev) {
      merged.set(key, {
        isSigner: !!a.isSigner,
        isWritable: !!a.isWritable,
      });
      continue;
    }
    prev.isSigner = prev.isSigner || !!a.isSigner;
    prev.isWritable = prev.isWritable || !!a.isWritable;
  }

  return {
    ...lane,
    remainingAccounts: lane.remainingAccounts.map((a) => {
      const effective = merged.get(a.pubkey);
      return {
        ...a,
        isSigner: effective?.isSigner ?? !!a.isSigner,
        isWritable: effective?.isWritable ?? !!a.isWritable,
      };
    }),
  };
}

function normalizeLaneRuntimeFlagsLegacyWithFixedAccounts(
  lane: LaneConfig,
  groupPk: PublicKey,
  executionQueuePk: PublicKey,
): LaneConfig {
  const fixed = [
    { pubkey: groupPk.toBase58(), isSigner: false, isWritable: true },
    { pubkey: executionQueuePk.toBase58(), isSigner: false, isWritable: true },
    {
      pubkey: SYSVAR_INSTRUCTIONS_PUBKEY.toBase58(),
      isSigner: false,
      isWritable: false,
    },
  ];
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const a of [...fixed, ...lane.remainingAccounts]) {
    const key = a.pubkey;
    const prev = merged.get(key);
    if (!prev) {
      merged.set(key, { isSigner: !!a.isSigner, isWritable: !!a.isWritable });
      continue;
    }
    prev.isSigner = prev.isSigner || !!a.isSigner;
    prev.isWritable = prev.isWritable || !!a.isWritable;
  }
  return {
    ...lane,
    name: `${lane.name || 'lane'}-legacy`,
    remainingAccounts: lane.remainingAccounts.map((a) => {
      const effective = merged.get(a.pubkey);
      return {
        ...a,
        isSigner: effective?.isSigner ?? !!a.isSigner,
        isWritable: effective?.isWritable ?? !!a.isWritable,
      };
    }),
  };
}

function expandLaneVariants(
  lane: LaneConfig,
  groupPk: PublicKey,
  executionQueuePk: PublicKey,
): LaneConfig[] {
  const current = normalizeLaneRuntimeFlags(lane, groupPk, executionQueuePk);
  if (!CRANK_INCLUDE_LEGACY_FIXED_HASH) {
    return [current];
  }
  const legacy = normalizeLaneRuntimeFlagsLegacyWithFixedAccounts(
    lane,
    groupPk,
    executionQueuePk,
  );
  return [current, legacy];
}

function dedupeLanes(lanes: LaneConfig[]): LaneConfig[] {
  const byHash = new Map<string, LaneConfig>();
  for (const lane of lanes) {
    if (!lane.remainingAccounts.length) {
      continue;
    }
    byHash.set(laneHash(lane), lane);
  }
  return Array.from(byHash.values());
}

function findLaneByHash(lanes: LaneConfig[], wantedHash: string): LaneConfig | null {
  if (!wantedHash.length) {
    return null;
  }
  for (const lane of lanes) {
    if (laneHash(lane) === wantedHash) {
      return lane;
    }
  }
  return null;
}

function parseDynamicLanesFromRelayLog(
  eventLogPath: string,
  groupPk: PublicKey,
  executionQueuePk: PublicKey,
): LaneConfig[] {
  const resolved = path.resolve(eventLogPath);
  if (!resolved.length || !fs.existsSync(resolved)) {
    return [];
  }

  const content = fs.readFileSync(resolved, 'utf-8');
  if (!content.trim().length) {
    return [];
  }

  const lines = content
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .slice(-Math.max(1, CRANK_DYNAMIC_LANES_MAX_EVENTS));

  const dynamic: LaneConfig[] = [];
  for (const line of lines) {
    let parsed: RelayIntentAcceptedEvent;
    try {
      parsed = JSON.parse(line) as RelayIntentAcceptedEvent;
    } catch {
      continue;
    }

    if (parsed.event_type !== 'relay_intent_accepted') {
      continue;
    }
    if (parsed.group !== groupPk.toBase58()) {
      continue;
    }
    if (parsed.execution_queue !== executionQueuePk.toBase58()) {
      continue;
    }
    if (!parsed.remaining_accounts?.length) {
      continue;
    }

    const lane: LaneConfig = {
      name: `dynamic-${parsed.market || 'market'}-${parsed.sequence || 'seq'}-${
        parsed.user_owner?.slice(0, 8) || 'owner'
      }`,
      remainingAccounts: parsed.remaining_accounts.map((a) => ({
        pubkey: a.pubkey,
        isWritable: !!a.is_writable,
        isSigner: !!a.is_signer,
      })),
    };
    dynamic.push(...expandLaneVariants(lane, groupPk, executionQueuePk));
  }

  return dedupeLanes(dynamic);
}

function inspectQueueHead(params: {
  queueData: Buffer;
}): {
  count: number;
  nextSequence: bigint;
  headAccountsHash: string | null;
} {
  const count = decodeExecutionQueueCount(params.queueData);
  const nextSequence = decodeExecutionQueueNextSequence(params.queueData);
  const headItem = decodeExecutionQueueHeadItem(params.queueData);
  if (!headItem) {
    return {
      count,
      nextSequence,
      headAccountsHash: null,
    };
  }
  return {
    count,
    nextSequence,
    headAccountsHash: headItem.accountsHash.toString('hex'),
  };
}

function deriveWsEndpoint(httpUrl: string): string | null {
  try {
    const parsed = new URL(httpUrl);
    const protocol = parsed.protocol === 'https:' ? 'wss:' : 'ws:';
    let port = parsed.port;
    if (
      (parsed.hostname === '127.0.0.1' || parsed.hostname === 'localhost') &&
      parsed.port === '8899'
    ) {
      port = '8900';
    }
    const host = port.length ? `${parsed.hostname}:${port}` : parsed.hostname;
    return `${protocol}//${host}${parsed.pathname || ''}`;
  } catch {
    return null;
  }
}

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }
  if (!GROUP_PK || !EXECUTION_QUEUE_PK) {
    throw new Error('EXECUTION_QUEUE_GROUP_PK and EXECUTION_QUEUE_PK are required');
  }
  if (!CRANKER_KEYPAIR) {
    throw new Error('EXECUTION_QUEUE_CRANKER_KEYPAIR (or MB_PAYER_KEYPAIR) is required');
  }

  const executionQueue = new PublicKey(EXECUTION_QUEUE_PK);
  const groupPk = new PublicKey(GROUP_PK);
  const staticLanes = parseLaneConfigs().flatMap((lane) =>
    expandLaneVariants(lane, groupPk, executionQueue),
  );
  if (!staticLanes.length) {
    throw new Error(
      'At least one lane is required in EXECUTION_QUEUE_CRANK_LANES_JSON or *_JSON_PATH',
    );
  }
  let lanes = dedupeLanes(staticLanes);

  const wsEndpoint = CLUSTER_WS_URL || deriveWsEndpoint(CLUSTER_URL) || undefined;
  const connection = new Connection(CLUSTER_URL, {
    ...AnchorProvider.defaultOptions(),
    wsEndpoint,
  });
  const cranker = readKeypair(CRANKER_KEYPAIR);
  const provider = new AnchorProvider(
    connection,
    new Wallet(cranker),
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
  const group: Group = await client.getGroup(groupPk);
  const executionQueueBuffer = new PublicKey(EXECUTION_QUEUE_BUFFER_PK || EXECUTION_QUEUE_PK);
  let lastDynamicRefreshMs = 0;
  let unchangedLoops = 0;
  let previousQueueCount = -1;
  const laneFailureCounts = new Map<string, number>();
  const laneBackoffUntilMs = new Map<string, number>();

  console.log(
    `Execution queue cranker started, queue=${executionQueue.toBase58()}, static_lanes=${lanes.length}, parallel_lanes=${CRANK_PARALLEL_LANES}, confirmInBackground=${CRANK_CONFIRM_IN_BACKGROUND}, skipPreflight=${CRANK_SKIP_PREFLIGHT}, skipConfirmation=${CRANK_SKIP_CONFIRMATION}`,
  );

  // eslint-disable-next-line no-constant-condition
  while (true) {
    try {
      const queueAccountInfo = await client.connection.getAccountInfo(executionQueue);
      const head = queueAccountInfo?.data
        ? inspectQueueHead({
          queueData: queueAccountInfo.data,
          })
        : { count: 0, nextSequence: 0n, headAccountsHash: null };
      if (!queueAccountInfo?.data || head.count === 0) {
        previousQueueCount = 0;
        unchangedLoops = 0;
        await new Promise((resolve) => setTimeout(resolve, CRANK_INTERVAL_MS));
        continue;
      }

      const now = Date.now();
      if (
        CRANK_RELAY_EVENT_LOG_PATH.length &&
        now - lastDynamicRefreshMs >= CRANK_DYNAMIC_LANES_REFRESH_MS
      ) {
        const dynamicLanes = parseDynamicLanesFromRelayLog(
          CRANK_RELAY_EVENT_LOG_PATH,
          groupPk,
          executionQueue,
        );
        const merged = dedupeLanes([...staticLanes, ...dynamicLanes]);
        if (merged.length !== lanes.length) {
          console.log(
            `Lane set refreshed: ${lanes.length} -> ${merged.length} (dynamic=${dynamicLanes.length})`,
          );
        }
        if (CRANK_DEBUG_HEAD_HASH.length) {
          const hit = findLaneByHash(merged, CRANK_DEBUG_HEAD_HASH);
          console.log(
            `Lane debug hash ${CRANK_DEBUG_HEAD_HASH}: ${
              hit ? `present (${hit.name || 'lane'})` : 'missing'
            }`,
          );
        }
        lanes = merged;
        lastDynamicRefreshMs = now;
      }

      const runnableLanes =
        CRANK_MATCH_HEAD_ONLY && head.headAccountsHash
          ? lanes.filter((lane) => laneHash(lane) === head.headAccountsHash)
          : lanes;
      const lanesToRun = runnableLanes.length ? runnableLanes : lanes;
      if (
        CRANK_MATCH_HEAD_ONLY &&
        head.headAccountsHash &&
        !runnableLanes.length &&
        CRANK_RELAY_EVENT_LOG_PATH.length
      ) {
        console.warn(
          `No lane matched head hash ${head.headAccountsHash} at seq=${head.nextSequence.toString()}, falling back to all lanes`,
        );
        lastDynamicRefreshMs = 0;
      }

      const runLane = async (lane: LaneConfig) => {
        const laneName = lane.name ?? 'lane';
        const laneKey = laneHash(lane);
        const blockedUntil = laneBackoffUntilMs.get(laneKey) ?? 0;
        if (blockedUntil > Date.now()) {
          return;
        }
        try {
          const status = await client.executionQueueExecute(
            group,
            executionQueue,
            executionQueueBuffer,
            toAccountMetas(lane),
            CRANK_MAX_ITEMS,
            {
              prioritizationFee: CRANK_PRIORITIZATION_FEE,
              confirmInBackground: CRANK_CONFIRM_IN_BACKGROUND,
              skipPreflight: CRANK_SKIP_PREFLIGHT,
              skipConfirmation: CRANK_SKIP_CONFIRMATION,
            },
          );
          console.log(
            `[${laneName}] execute sent: https://explorer.solana.com/tx/${status.signature}`,
          );
          laneFailureCounts.set(laneKey, 0);
          laneBackoffUntilMs.delete(laneKey);
        } catch (err) {
          const errText = err instanceof Error ? err.message : `${err}`;
          const nextFails = (laneFailureCounts.get(laneKey) ?? 0) + 1;
          laneFailureCounts.set(laneKey, nextFails);
          if (
            errText.includes('"Custom":6000') &&
            nextFails >= CRANK_LANE_FAILURE_THRESHOLD
          ) {
            laneBackoffUntilMs.set(laneKey, Date.now() + CRANK_LANE_FAILURE_BACKOFF_MS);
            console.warn(
              `[${laneName}] backoff ${CRANK_LANE_FAILURE_BACKOFF_MS}ms after ${nextFails} failures (${errText.slice(0, 128)})`,
            );
          }
          console.error(`[${laneName}] execute failed:`, err);
        }
      };

      if (CRANK_PARALLEL_LANES) {
        await Promise.all(lanesToRun.map((lane) => runLane(lane)));
      } else {
        for (const lane of lanesToRun) {
          await runLane(lane);
        }
      }

      const queueAccountAfter = await client.connection.getAccountInfo(executionQueue);
      const queueCountAfter = queueAccountAfter?.data
        ? decodeExecutionQueueCount(queueAccountAfter.data)
        : 0;
      if (queueCountAfter === previousQueueCount) {
        unchangedLoops += 1;
      } else {
        unchangedLoops = 0;
      }
      previousQueueCount = queueCountAfter;

      if (queueCountAfter > 0 && unchangedLoops >= CRANK_STALL_WARN_LOOPS) {
        console.warn(
          `Queue stall detected: count=${queueCountAfter}, lanes=${lanes.length}, unchanged_loops=${unchangedLoops}`,
        );
        if (CRANK_DEBUG_HEAD_HASH.length) {
          const hit = findLaneByHash(lanes, CRANK_DEBUG_HEAD_HASH);
          console.warn(
            `Queue stall lane debug ${CRANK_DEBUG_HEAD_HASH}: ${
              hit ? `present (${hit.name || 'lane'})` : 'missing'
            }`,
          );
        }
        if (CRANK_RELAY_EVENT_LOG_PATH.length) {
          // Force eager dynamic refresh on the next loop when stalled.
          lastDynamicRefreshMs = 0;
        }
      }
    } catch (err) {
      console.error('queue cranker loop error:', err);
    }

    await new Promise((resolve) => setTimeout(resolve, CRANK_INTERVAL_MS));
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
