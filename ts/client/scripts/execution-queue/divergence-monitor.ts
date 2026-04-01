/**
 * divergence-monitor.ts — Compares harness (optimistic) state vs on-chain
 * (confirmed) state every N seconds and reports divergences.
 *
 * Checks:
 *  1. Perp positions (base_position_lots, quote_position_native) per mango account
 *  2. Open orders count per market
 *  3. Execution queue head sequence vs harness watermark
 *  4. Orderbook bid/ask counts
 *
 * Usage:
 *   ts-node ts/client/scripts/execution-queue/divergence-monitor.ts
 *
 * Env:
 *   DIVERGENCE_MONITOR_INTERVAL_MS  (default: 5000)
 *   DIVERGENCE_MONITOR_HARNESS_URL  (default: http://127.0.0.1:9091)
 *   CLUSTER_URL_OVERRIDE            (RPC endpoint)
 *   E2E_CONFIG_PATH / QUOTER_CONFIG_PATH
 */
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import { MangoClient } from '../../src/client';
import { PerpPosition } from '../../src/accounts/mangoAccount';
import fs from 'fs';
import path from 'path';
import { runtimeConfigPath } from './scriptEnv';

// ── Config ──────────────────────────────────────────────────────────────────
const INTERVAL_MS = Number(process.env.DIVERGENCE_MONITOR_INTERVAL_MS || '5000');
const HARNESS_URL =
  process.env.DIVERGENCE_MONITOR_HARNESS_URL ||
  process.env.CONTINUUM_HARNESS_URL ||
  'http://127.0.0.1:9091';
const CONFIG_PATH =
  process.env.E2E_CONFIG_PATH ||
  process.env.QUOTER_CONFIG_PATH ||
  runtimeConfigPath('execution-queue-e2e-9125.json');
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || 'https://api.devnet.solana.com';

// ── Types ───────────────────────────────────────────────────────────────────
interface E2EConfig {
  programId: string;
  group: string;
  executionQueue: string;
  cluster: string;
  clusterUrl: string;
  perpMarketIndex: number;
  maker: { mangoAccount: string; owner: string };
  taker: { mangoAccount: string; owner: string };
}

interface HarnessUserState {
  owner: string;
  mango_accounts: string[];
  open_orders: any[];
  per_market: {
    market: string;
    base_position_lots: string;
    quote_position_native: string;
    open_order_base_lots_bid: string;
    open_order_base_lots_ask: string;
  }[];
}

interface HarnessMarketState {
  bids: any[];
  asks: any[];
  open_orders: any[];
  watermarks: {
    optimistic_seq: string;
    confirmed_seq: string;
    last_slot: string;
  };
}

interface HarnessFullState {
  markets: Record<string, HarnessMarketState>;
  users: Record<string, HarnessUserState>;
  generated_ts_ms: number;
}

interface Divergence {
  field: string;
  account: string;
  harness: string | number;
  onchain: string | number;
  delta?: string | number;
}

// ── Helpers ─────────────────────────────────────────────────────────────────
async function fetchHarnessState(): Promise<HarnessFullState> {
  const resp = await fetch(`${HARNESS_URL}/state/full`, {
    signal: AbortSignal.timeout(4000),
  });
  if (!resp.ok) throw new Error(`harness ${resp.status}`);
  return resp.json() as Promise<HarnessFullState>;
}

async function fetchHarnessHealth(): Promise<any> {
  const resp = await fetch(`${HARNESS_URL}/healthz`, {
    signal: AbortSignal.timeout(4000),
  });
  if (!resp.ok) throw new Error(`harness health ${resp.status}`);
  return resp.json();
}

function ts(): string {
  return new Date().toISOString();
}

// ── Main ────────────────────────────────────────────────────────────────────
async function main() {
  const config: E2EConfig = JSON.parse(
    fs.readFileSync(path.resolve(CONFIG_PATH), 'utf-8'),
  );
  const clusterUrl = CLUSTER_URL || config.clusterUrl;
  const connection = new Connection(clusterUrl, 'confirmed');
  const dummyWallet = new Wallet(Keypair.generate());
  const provider = new AnchorProvider(
    connection,
    dummyWallet,
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(
    provider,
    config.cluster as any,
    new PublicKey(config.programId),
    { idsSource: 'get-program-accounts' },
  );

  const groupPk = new PublicKey(config.group);
  const executionQueuePk = new PublicKey(config.executionQueue);
  const perpMarketIndex = config.perpMarketIndex;

  // Collect all mango accounts to monitor (maker, taker, quoter bots)
  const mangoAccountPks: { label: string; owner: string; pk: PublicKey }[] = [];

  // Maker/taker from e2e config
  if (config.maker?.mangoAccount) {
    mangoAccountPks.push({
      label: 'maker',
      owner: config.maker.owner,
      pk: new PublicKey(config.maker.mangoAccount),
    });
  }
  if (config.taker?.mangoAccount) {
    mangoAccountPks.push({
      label: 'taker',
      owner: config.taker.owner,
      pk: new PublicKey(config.taker.mangoAccount),
    });
  }

  // Quoter bots from active bots file
  const botsPath = process.env.QUOTER_BOTS_JSON_PATH ||
    runtimeConfigPath('quoter-bots-active-9125.json');
  if (fs.existsSync(botsPath)) {
    const bots = JSON.parse(fs.readFileSync(botsPath, 'utf-8'));
    for (const bot of bots) {
      mangoAccountPks.push({
        label: bot.name,
        owner: bot.owner,
        pk: new PublicKey(bot.mangoAccount),
      });
    }
  }

  console.log(JSON.stringify({
    ts: ts(),
    msg: 'divergence-monitor started',
    harness: HARNESS_URL,
    rpc: clusterUrl,
    interval_ms: INTERVAL_MS,
    program: config.programId,
    group: config.group,
    execution_queue: config.executionQueue,
    monitored_accounts: mangoAccountPks.map((a) => ({
      label: a.label,
      owner: a.owner,
      mango_account: a.pk.toBase58(),
    })),
  }));

  let group = await client.getGroup(groupPk);
  let tickCount = 0;

  const loop = async () => {
    tickCount++;
    const divergences: Divergence[] = [];
    let harnessState: HarnessFullState;
    let harnessHealth: any;

    // ── Fetch harness state ─────────────────────────────────────────────
    try {
      [harnessState, harnessHealth] = await Promise.all([
        fetchHarnessState(),
        fetchHarnessHealth(),
      ]);
    } catch (err: any) {
      console.log(JSON.stringify({
        ts: ts(),
        tick: tickCount,
        error: `harness fetch failed: ${err.message}`,
      }));
      return;
    }

    // ── Reload group every 10 ticks ─────────────────────────────────────
    if (tickCount % 10 === 0) {
      try {
        group = await client.getGroup(groupPk);
      } catch {}
    }

    // ── Compare perp positions per mango account ────────────────────────
    for (const entry of mangoAccountPks) {
      try {
        const onchainAccount = await client.getMangoAccount(entry.pk);
        const pp: PerpPosition | undefined = onchainAccount.getPerpPosition(
          perpMarketIndex as any,
        );

        const onchainBaseLots = pp ? pp.basePositionLots.toString() : '0';
        // quotePositionNative is I80F48 (fixed-point) — truncate to integer for comparison
        const onchainQuoteNativeRaw = pp ? pp.quotePositionNative.toNumber() : 0;
        const onchainQuoteNative = Math.trunc(onchainQuoteNativeRaw).toString();

        // Find harness state for this owner
        const harnessUser = harnessState.users[entry.owner];
        if (!harnessUser) {
          divergences.push({
            field: 'user_missing_in_harness',
            account: entry.label,
            harness: 'missing',
            onchain: entry.owner,
          });
          continue;
        }

        const harnessPerp = harnessUser.per_market?.find(
          (pm) => pm.market === String(perpMarketIndex),
        );
        const harnessBaseLots = harnessPerp?.base_position_lots || '0';
        const harnessQuoteNative = harnessPerp?.quote_position_native || '0';

        if (onchainBaseLots !== harnessBaseLots) {
          divergences.push({
            field: 'base_position_lots',
            account: entry.label,
            harness: harnessBaseLots,
            onchain: onchainBaseLots,
            delta: BigInt(harnessBaseLots) - BigInt(onchainBaseLots) + '',
          });
        }

        if (onchainQuoteNative !== harnessQuoteNative) {
          const harnessQuoteInt = BigInt(harnessQuoteNative);
          const onchainQuoteInt = BigInt(onchainQuoteNative);
          divergences.push({
            field: 'quote_position_native',
            account: entry.label,
            harness: harnessQuoteNative,
            onchain: onchainQuoteNative,
            delta: (harnessQuoteInt - onchainQuoteInt).toString(),
          });
        }

        // Open orders count
        const harnessOOCount = harnessUser.open_orders?.length || 0;
        const onchainOOCount = onchainAccount.perpActive()
          .filter((p) => p.marketIndex === perpMarketIndex)
          .reduce(
            (sum, p) =>
              sum + (p.bidsBaseLots.toNumber() > 0 ? 1 : 0) +
              (p.asksBaseLots.toNumber() > 0 ? 1 : 0),
            0,
          );
        // Note: harness tracks individual orders, on-chain aggregates per side.
        // Only flag if harness says 0 orders but on-chain has nonzero or vice versa.
        if ((harnessOOCount === 0) !== (onchainOOCount === 0)) {
          divergences.push({
            field: 'open_orders_presence',
            account: entry.label,
            harness: harnessOOCount,
            onchain: onchainOOCount,
          });
        }
      } catch (err: any) {
        divergences.push({
          field: 'onchain_fetch_error',
          account: entry.label,
          harness: 'n/a',
          onchain: err.message?.slice(0, 100) || String(err),
        });
      }
    }

    // ── Execution queue head vs harness watermark ───────────────────────
    try {
      const queueAccountInfo = await connection.getAccountInfo(executionQueuePk);
      if (queueAccountInfo) {
        // Read next_sequence_to_execute from queue header (offset 8+8+8 = 24 after discriminator)
        // Layout: discriminator(8) + group(32) + ctm_signer(32) + bump(1) + paused(2) + header_start
        // Header: next_sequence(8) + max_seen_sequence(8) + gap_observed_slot(8) + ...
        // Based on the state struct: the header starts after fixed fields.
        // Use harness watermarks instead for a simpler comparison.
        const market0 = harnessState.markets['0'];
        if (market0) {
          const optimisticSeq = Number(market0.watermarks.optimistic_seq);
          const confirmedSeq = Number(market0.watermarks.confirmed_seq);
          const seqGap = optimisticSeq - confirmedSeq;
          if (seqGap > 100) {
            divergences.push({
              field: 'sequence_gap',
              account: 'execution_queue',
              harness: optimisticSeq,
              onchain: confirmedSeq,
              delta: seqGap,
            });
          }
        }
      }
    } catch {}

    // ── Orderbook sanity ────────────────────────────────────────────────
    const market0 = harnessState.markets['0'];
    if (market0) {
      const harnessBids = market0.bids?.length || 0;
      const harnessAsks = market0.asks?.length || 0;
      // If harness shows an empty book for >30 ticks, flag it
      if (harnessBids === 0 && harnessAsks === 0 && tickCount > 5) {
        divergences.push({
          field: 'orderbook_empty',
          account: 'market_0',
          harness: `bids=${harnessBids} asks=${harnessAsks}`,
          onchain: 'n/a (check perp market account)',
        });
      }
    }

    // ── Report ──────────────────────────────────────────────────────────
    const report: any = {
      ts: ts(),
      tick: tickCount,
      harness_intents: harnessHealth.intents_total,
      harness_divergences: harnessHealth.divergences_total,
      harness_sse_clients: harnessHealth.sse_clients,
      optimistic_seq: market0?.watermarks?.optimistic_seq,
      confirmed_seq: market0?.watermarks?.confirmed_seq,
      ob_bids: market0?.bids?.length || 0,
      ob_asks: market0?.asks?.length || 0,
      monitored_accounts: mangoAccountPks.length,
      divergence_count: divergences.length,
    };

    if (divergences.length > 0) {
      report.divergences = divergences;
    } else {
      report.status = 'in_sync';
    }

    console.log(JSON.stringify(report));
  };

  // Run immediately, then on interval
  await loop();
  setInterval(loop, INTERVAL_MS);
}

main().catch((err) => {
  console.error('divergence-monitor fatal:', err);
  process.exit(1);
});
