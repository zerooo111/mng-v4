/**
 * Harness Prom metrics — Phase 6.
 *
 * Exposes Redis-publisher and state-mirror health on a local HTTP listener at
 * `/metrics`. Prometheus scrapes this from the metrics VM over the VPC.
 *
 * Uses `prom-client`'s default registry so any other module in the harness
 * (or `collectDefaultMetrics()`) lands in the same scrape. Metric names use
 * the `harness_redis_*` prefix to avoid collision with `continuum_harness_*`
 * (domain metrics) and `gateway_*` (Rust gateway).
 *
 * Target path on harness VM:
 *   /home/hetalkenaudekar/stagin4/mng-v4/ts/client/scripts/execution-queue/metrics.ts
 */

import { createServer } from 'http';
import { collectDefaultMetrics, Counter, Histogram, register } from 'prom-client';

// ── Config ──────────────────────────────────────────────────────────

const METRICS_ENABLED =
  (process.env.HARNESS_METRICS_ENABLED ?? 'true').toLowerCase() === 'true';
const METRICS_PORT = Number(process.env.HARNESS_METRICS_PORT ?? 9464);

// ── Metric definitions ──────────────────────────────────────────────

const writesTotal = new Counter({
  name: 'harness_redis_writes_total',
  help: 'Successful XADDs from the harness Redis publisher.',
  labelNames: ['stream'],
});

const writeErrorsTotal = new Counter({
  name: 'harness_redis_write_errors_total',
  help: 'Failed XADDs (connection or xadd error) from the harness publisher.',
  labelNames: ['kind'],
});

// Counts publishes that the pipeline intentionally skipped because the
// source event carried nothing to publish (e.g. queue_item_processed with
// no fills attached by enrichProcessedWithFills). Distinct from
// write_errors — nothing failed; there was simply nothing to write. A
// sustained rate here with `reason="no_fills"` means the enrichment path
// is broken even though Redis itself is healthy.
const publishSkippedTotal = new Counter({
  name: 'harness_redis_publish_skipped_total',
  help: 'Publishes skipped because the source event had no data to emit.',
  labelNames: ['stream', 'reason'],
});

const writeDurationSeconds = new Histogram({
  name: 'harness_redis_write_duration_seconds',
  help: 'XADD round-trip duration from issue to resolution.',
  labelNames: ['stream'],
  buckets: [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0],
});

const mirrorTicksTotal = new Counter({
  name: 'harness_redis_mirror_ticks_total',
  help: 'State-mirror tick passes that wrote snapshots to Redis.',
  labelNames: ['view', 'result'], // view = opt|conf, result = ok|error
});

const mirrorTickDurationSeconds = new Histogram({
  name: 'harness_redis_mirror_tick_duration_seconds',
  help: 'State-mirror tick duration (snapshot projection + pipeline.exec).',
  labelNames: ['view'],
  buckets: [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
});

// Default process / node metrics (event loop lag, RSS, GC). Cheap, ~20 series.
if (METRICS_ENABLED) {
  collectDefaultMetrics({ prefix: 'harness_' });
}

// ── Exports for the publisher (matches its MetricsHooks shape) ──────

/** Hooks object to pass to `createPublisher(...)`. */
export const publisherMetricsHooks = {
  writesTotal: {
    inc(labels: Record<string, string> = {}, value = 1) {
      writesTotal.inc(labels, value);
    },
  },
  writeErrorsTotal: {
    inc(labels: Record<string, string> = {}, value = 1) {
      writeErrorsTotal.inc(labels, value);
    },
  },
  publishSkippedTotal: {
    inc(labels: Record<string, string> = {}, value = 1) {
      publishSkippedTotal.inc(labels, value);
    },
  },
  writeDurationSeconds: {
    observe(labels: Record<string, string>, value: number) {
      writeDurationSeconds.observe(labels, value);
    },
  },
};

// ── Exports for the state mirror ────────────────────────────────────

/** Called once per state-mirror tick (per view). `durSec` covers the full pass. */
export function recordMirrorTick(view: 'opt' | 'conf', durSec: number, ok: boolean): void {
  mirrorTickDurationSeconds.observe({ view }, durSec);
  mirrorTicksTotal.inc({ view, result: ok ? 'ok' : 'error' });
}

// ── HTTP listener ───────────────────────────────────────────────────

/**
 * Start the metrics HTTP server. Idempotent-ish: if METRICS_ENABLED is false
 * or the port is already bound, this is a no-op that logs once.
 */
export function startMetricsServer(): void {
  if (!METRICS_ENABLED) {
    // eslint-disable-next-line no-console
    console.info('[harness-metrics] disabled (HARNESS_METRICS_ENABLED=false)');
    return;
  }

  const server = createServer(async (req, res) => {
    if (!req.url || !req.url.startsWith('/metrics')) {
      res.writeHead(404).end('not found');
      return;
    }
    try {
      const body = await register.metrics();
      res.writeHead(200, { 'content-type': register.contentType });
      res.end(body);
    } catch (err) {
      res.writeHead(500).end(String((err as Error).message ?? err));
    }
  });

  server.on('error', (err) => {
    // eslint-disable-next-line no-console
    console.warn('[harness-metrics] listener error:', (err as Error).message);
  });

  server.listen(METRICS_PORT, '0.0.0.0', () => {
    // eslint-disable-next-line no-console
    console.info('[harness-metrics] listening on :%d/metrics', METRICS_PORT);
  });
  server.unref(); // don't keep the process alive purely for the metrics port
}
