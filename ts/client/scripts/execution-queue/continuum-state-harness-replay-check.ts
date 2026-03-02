import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import {
  ContinuumStateEngine,
  HarnessEvent,
} from '../../src/continuumHarness';

dotenv.config();

const EVENT_LOG_PATH =
  process.env.CONTINUUM_HARNESS_EVENT_LOG_PATH ||
  '/tmp/continuum-harness-events.jsonl';
const REPLAY_CHECK_ITERATIONS = Number(process.env.REPLAY_CHECK_ITERATIONS || '25');
const REPLAY_CHECK_SEED = Number(process.env.REPLAY_CHECK_SEED || '1337');
const REPLAY_CHECK_VIEW = (process.env.REPLAY_CHECK_VIEW || 'both').toLowerCase();
const REPLAY_CHECK_FAIL_ON_MISMATCH =
  (process.env.REPLAY_CHECK_FAIL_ON_MISMATCH || 'true') === 'true';

type ReplayResult = {
  ok: boolean;
  event_count: number;
  iterations: number;
  mismatches: Array<{
    iteration: number;
    view: 'optimistic' | 'confirmed';
    reason: string;
  }>;
  generated_ts_ms: number;
};

function parseEventLog(filePath: string): HarnessEvent[] {
  const abs = path.resolve(filePath);
  if (!fs.existsSync(abs)) {
    throw new Error(`event log not found: ${abs}`);
  }

  const events: HarnessEvent[] = [];
  const raw = fs.readFileSync(abs, 'utf-8');
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed.length) {
      continue;
    }
    const parsed = JSON.parse(trimmed) as HarnessEvent;
    if (
      parsed.event_type !== 'relay_intent_accepted' &&
      parsed.event_type !== 'queue_item_enqueued' &&
      parsed.event_type !== 'queue_item_processed'
    ) {
      continue;
    }
    events.push(parsed);
  }

  return events;
}

function applyEvent(engine: ContinuumStateEngine, event: HarnessEvent): void {
  if (event.event_type === 'relay_intent_accepted') {
    engine.ingestRelayIntent(event);
  } else if (event.event_type === 'queue_item_enqueued') {
    engine.ingestQueueEnqueued(event);
  } else if (event.event_type === 'queue_item_processed') {
    engine.ingestQueueProcessed(event);
  }
}

function sanitizeSnapshot(snapshot: unknown): string {
  const cloned = JSON.parse(JSON.stringify(snapshot));
  if (cloned && typeof cloned === 'object' && 'generated_ts_ms' in cloned) {
    cloned.generated_ts_ms = 0;
  }
  return JSON.stringify(cloned);
}

function seededShuffle<T>(input: T[], seed: number): T[] {
  const out = input.slice();
  let state = seed >>> 0;
  const rnd = (): number => {
    state = (1664525 * state + 1013904223) >>> 0;
    return state / 0x100000000;
  };

  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

function shouldCheckView(view: 'optimistic' | 'confirmed'): boolean {
  if (REPLAY_CHECK_VIEW === 'both') {
    return true;
  }
  return REPLAY_CHECK_VIEW === view;
}

function buildSnapshotStrings(events: HarnessEvent[]): {
  optimistic: string;
  confirmed: string;
} {
  const engine = new ContinuumStateEngine();
  for (const event of events) {
    applyEvent(engine, event);
  }
  return {
    optimistic: sanitizeSnapshot(engine.getSnapshot('optimistic')),
    confirmed: sanitizeSnapshot(engine.getSnapshot('confirmed')),
  };
}

async function main(): Promise<void> {
  const events = parseEventLog(EVENT_LOG_PATH);
  const baseline = buildSnapshotStrings(events);

  const mismatches: ReplayResult['mismatches'] = [];

  for (let i = 0; i < REPLAY_CHECK_ITERATIONS; i++) {
    const shuffled = seededShuffle(events, REPLAY_CHECK_SEED + i);
    const candidate = buildSnapshotStrings(shuffled);

    if (shouldCheckView('optimistic') && candidate.optimistic !== baseline.optimistic) {
      mismatches.push({
        iteration: i,
        view: 'optimistic',
        reason: 'optimistic snapshot mismatch after shuffled replay',
      });
    }
    if (shouldCheckView('confirmed') && candidate.confirmed !== baseline.confirmed) {
      mismatches.push({
        iteration: i,
        view: 'confirmed',
        reason: 'confirmed snapshot mismatch after shuffled replay',
      });
    }
  }

  const result: ReplayResult = {
    ok: mismatches.length === 0,
    event_count: events.length,
    iterations: REPLAY_CHECK_ITERATIONS,
    mismatches,
    generated_ts_ms: Date.now(),
  };

  console.log(JSON.stringify(result, null, 2));

  if (mismatches.length && REPLAY_CHECK_FAIL_ON_MISMATCH) {
    process.exit(2);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
