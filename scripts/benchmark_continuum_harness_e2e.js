#!/usr/bin/env node

const http = require('http');
const https = require('https');
const { URL } = require('url');
const path = require('path');
const { performance } = require('perf_hooks');
const crypto = require('crypto');
const grpc = require('@grpc/grpc-js');
const protoLoader = require('@grpc/proto-loader');
const { Keypair } = require('@solana/web3.js');

const HARNESS_URL = process.env.BENCH_HARNESS_URL || 'http://127.0.0.1:9291';
const SEQUENCER_ADDR = process.env.BENCH_SEQUENCER_ADDR || '127.0.0.1:9392';
const SEQUENCER_STATUS_URL =
  process.env.BENCH_SEQUENCER_STATUS_URL || 'http://127.0.0.1:9394/status';
const TX_COUNT = Number(process.env.BENCH_TX_COUNT || '100');
const TIMEOUT_MS = Number(process.env.BENCH_TIMEOUT_MS || '10000');
const MARKET = String(process.env.BENCH_MARKET || '4242');
const GROUP = process.env.BENCH_GROUP || Keypair.generate().publicKey.toBase58();
const EXECUTION_QUEUE =
  process.env.BENCH_EXECUTION_QUEUE || Keypair.generate().publicKey.toBase58();
const OWNER_MODE = (process.env.BENCH_OWNER_MODE || 'single').toLowerCase();
const STREAM_FILTER = (
  process.env.BENCH_STREAM_FILTER ||
  (OWNER_MODE === 'single' ? 'owner_market' : 'market')
).toLowerCase();
const SERIAL_WAIT =
  (process.env.BENCH_SERIAL_WAIT || 'false').toLowerCase() === 'true';

function toMillis(start, end) {
  return Number(end - start) / 1e6;
}

function percentile(sorted, p) {
  if (!sorted.length) {
    return 0;
  }
  const idx = Math.min(
    sorted.length - 1,
    Math.max(0, Math.ceil((p / 100) * sorted.length) - 1),
  );
  return sorted[idx];
}

function summarize(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const total = sorted.reduce((sum, value) => sum + value, 0);
  return {
    count: sorted.length,
    min_ms: sorted[0] || 0,
    p50_ms: percentile(sorted, 50),
    p95_ms: percentile(sorted, 95),
    p99_ms: percentile(sorted, 99),
    max_ms: sorted[sorted.length - 1] || 0,
    mean_ms: sorted.length ? total / sorted.length : 0,
  };
}

function formatSummary(label, summary) {
  return `${label}: count=${summary.count} min=${summary.min_ms.toFixed(
    3,
  )}ms p50=${summary.p50_ms.toFixed(3)}ms p95=${summary.p95_ms.toFixed(
    3,
  )}ms p99=${summary.p99_ms.toFixed(3)}ms max=${summary.max_ms.toFixed(
    3,
  )}ms mean=${summary.mean_ms.toFixed(3)}ms`;
}

function encodePlaceOrderPayload({
  side = 0,
  priceLots,
  maxBaseLots,
  maxQuoteLots,
  clientOrderId,
  orderType = 2,
  selfTradeBehavior = 0,
  reduceOnly = false,
  expiryTimestamp = 0n,
  limit = 10,
}) {
  const buffer = Buffer.alloc(4 + 1 + 8 + 8 + 8 + 8 + 1 + 1 + 1 + 8 + 1);
  let offset = 0;
  buffer.writeUInt8(1, offset++);
  buffer.writeUInt8(0, offset++);
  buffer.writeUInt16LE(0, offset);
  offset += 2;
  buffer.writeUInt8(side, offset++);
  buffer.writeBigInt64LE(BigInt(priceLots), offset);
  offset += 8;
  buffer.writeBigInt64LE(BigInt(maxBaseLots), offset);
  offset += 8;
  buffer.writeBigInt64LE(BigInt(maxQuoteLots), offset);
  offset += 8;
  buffer.writeBigUInt64LE(BigInt(clientOrderId), offset);
  offset += 8;
  buffer.writeUInt8(orderType, offset++);
  buffer.writeUInt8(selfTradeBehavior, offset++);
  buffer.writeUInt8(reduceOnly ? 1 : 0, offset++);
  buffer.writeBigUInt64LE(BigInt(expiryTimestamp), offset);
  offset += 8;
  buffer.writeUInt8(limit, offset++);
  return buffer;
}

function loadSequencerProto() {
  const candidatePaths = [
    path.resolve(
      __dirname,
      '../../stagin4/continuum-monorepo-staging/crates/sequencer/proto/sequencer.proto',
    ),
    path.resolve(
      __dirname,
      '../ts/client/scripts/execution-queue/continuum_sequencer.proto',
    ),
  ];
  const protoPath = candidatePaths.find((candidate) => {
    try {
      return require('fs').existsSync(candidate);
    } catch {
      return false;
    }
  });
  if (!protoPath) {
    throw new Error('Unable to locate sequencer proto definition');
  }
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  return grpc.loadPackageDefinition(pkgDef);
}

function createSequencerClient(address) {
  const proto = loadSequencerProto();
  return new proto.continuum.sequencer.v1.SequencerService(
    address,
    grpc.credentials.createInsecure(),
  );
}

function submitTransaction(client, transaction) {
  const submitRpc =
    client.submitTransaction?.bind(client) ||
    client.submit_transaction?.bind(client);
  if (!submitRpc) {
    throw new Error('Sequencer gRPC client is missing submit transaction RPC');
  }
  return new Promise((resolve, reject) => {
    submitRpc({ transaction }, (err, response) => {
      if (err) {
        reject(err);
        return;
      }
      resolve(response);
    });
  });
}

function httpGetJson(url) {
  const parsed = new URL(url);
  const mod = parsed.protocol === 'https:' ? https : http;
  return new Promise((resolve, reject) => {
    const req = mod.request(
      parsed,
      { method: 'GET', headers: { accept: 'application/json' } },
      (res) => {
        let body = '';
        res.setEncoding('utf8');
        res.on('data', (chunk) => {
          body += chunk;
        });
        res.on('end', () => {
          if (res.statusCode && res.statusCode >= 400) {
            reject(
              new Error(`GET ${url} failed with ${res.statusCode}: ${body}`),
            );
            return;
          }
          try {
            resolve(JSON.parse(body));
          } catch (err) {
            reject(err);
          }
        });
      },
    );
    req.on('error', reject);
    req.end();
  });
}

function openSseStream(url, onEvent) {
  const parsed = new URL(url);
  const mod = parsed.protocol === 'https:' ? https : http;
  const req = mod.request(parsed, {
    method: 'GET',
    headers: {
      accept: 'text/event-stream',
      'cache-control': 'no-cache',
    },
  });

  let connectedResolve;
  let connectedReject;
  const connected = new Promise((resolve, reject) => {
    connectedResolve = resolve;
    connectedReject = reject;
  });
  let settledConnected = false;

  req.on('response', (res) => {
    if (res.statusCode !== 200) {
      connectedReject(
        new Error(`SSE request failed with status ${res.statusCode}`),
      );
      res.resume();
      return;
    }
    res.setEncoding('utf8');
    let buffer = '';
    res.on('data', (chunk) => {
      buffer += chunk;
      let splitIndex;
      while ((splitIndex = buffer.indexOf('\n\n')) !== -1) {
        const rawEvent = buffer.slice(0, splitIndex);
        buffer = buffer.slice(splitIndex + 2);
        const lines = rawEvent
          .split('\n')
          .map((line) => line.replace(/\r$/, ''))
          .filter(Boolean);
        let eventType = 'message';
        const dataLines = [];
        for (const line of lines) {
          if (line.startsWith('event:')) {
            eventType = line.slice('event:'.length).trim();
          } else if (line.startsWith('data:')) {
            dataLines.push(line.slice('data:'.length).trim());
          }
        }
        if (!dataLines.length) {
          continue;
        }
        let payload;
        try {
          payload = JSON.parse(dataLines.join('\n'));
        } catch {
          continue;
        }
        if (eventType === 'connected' && !settledConnected) {
          settledConnected = true;
          connectedResolve(payload);
        }
        onEvent(eventType, payload, process.hrtime.bigint());
      }
    });
    res.on('error', (err) => {
      if (!settledConnected) {
        settledConnected = true;
        connectedReject(err);
      }
    });
    res.on('end', () => {
      if (!settledConnected) {
        settledConnected = true;
        connectedReject(new Error('SSE stream ended before connected event'));
      }
    });
  });

  req.on('error', (err) => {
    if (!settledConnected) {
      settledConnected = true;
      connectedReject(err);
    }
  });

  req.end();

  return {
    connected,
    close() {
      req.destroy();
    },
  };
}

async function waitFor(url, predicate, label) {
  const started = Date.now();
  while (Date.now() - started < TIMEOUT_MS) {
    try {
      const payload = await httpGetJson(url);
      if (predicate(payload)) {
        return payload;
      }
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}`);
}

async function waitForRecord(records, sequence, predicate, label) {
  const started = Date.now();
  while (Date.now() - started < TIMEOUT_MS) {
    const record = records.get(sequence);
    if (record && predicate(record)) {
      return record;
    }
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
  throw new Error(`Timed out waiting for ${label} sequence=${sequence}`);
}

async function main() {
  const sharedOwner = OWNER_MODE === 'single' ? Keypair.generate() : null;
  const sharedMangoAccount =
    OWNER_MODE === 'single' ? Keypair.generate() : null;
  const client = createSequencerClient(SEQUENCER_ADDR);

  await waitFor(`${HARNESS_URL}/healthz`, (payload) => payload && payload.ok, 'harness');
  const sequencerStatus = await waitFor(
    SEQUENCER_STATUS_URL,
    (payload) => payload && payload.ok,
    'sequencer',
  );
  const sequenceBase =
    Number(sequencerStatus.total_transactions || 0) +
    Number(sequencerStatus.pending_transactions || 0) +
    1;

  const streamUrl = new URL('/state/stream/frontend', HARNESS_URL);
  streamUrl.searchParams.set('view', 'optimistic');
  streamUrl.searchParams.set('market', MARKET);
  streamUrl.searchParams.set('include', 'pre_confirm,validated_local');
  if (STREAM_FILTER === 'owner_market') {
    if (!sharedOwner || !sharedMangoAccount) {
      throw new Error(
        'owner_market stream filter requires BENCH_OWNER_MODE=single',
      );
    }
    streamUrl.searchParams.set('owner', sharedOwner.publicKey.toBase58());
    streamUrl.searchParams.set(
      'mango_account',
      sharedMangoAccount.publicKey.toBase58(),
    );
  }

  const records = new Map();
  const eventCounts = { pre_confirm: 0, validated_local: 0 };

  const sse = openSseStream(streamUrl.toString(), (eventType, payload, arrivedNs) => {
    if (
      (eventType === 'pre_confirm' || eventType === 'validated_local') &&
      payload &&
      payload.sequence !== undefined &&
      payload.sequence !== null
    ) {
      const key = String(payload.sequence);
      const record = records.get(key);
      if (!record) {
        return;
      }
      const arrivedMs = toMillis(record.originNs, arrivedNs);
      if (eventType === 'pre_confirm' && record.preConfirmMs === null) {
        record.preConfirmMs = arrivedMs;
        eventCounts.pre_confirm += 1;
      } else if (
        eventType === 'validated_local' &&
        record.validatedLocalMs === null
      ) {
        record.validatedLocalMs = arrivedMs;
        eventCounts.validated_local += 1;
      }
    }
  });

  await sse.connected;

  for (let i = 0; i < TX_COUNT; i += 1) {
    const sequence = String(sequenceBase + i);
    const originNs = process.hrtime.bigint();
    const txId = `bench-${Date.now()}-${i}-${crypto.randomBytes(4).toString('hex')}`;
    const owner = sharedOwner || Keypair.generate();
    const mangoAccount = sharedMangoAccount || Keypair.generate();
    records.set(sequence, {
      sequence,
      originNs,
      submitStartMs: 0,
      ackMs: null,
      preConfirmMs: null,
      validatedLocalMs: null,
      txId,
      owner: owner.publicKey.toBase58(),
      mangoAccount: mangoAccount.publicKey.toBase58(),
    });
    const record = records.get(sequence);
    const payload = encodePlaceOrderPayload({
      side: 0,
      priceLots: 100 + (i % 23),
      maxBaseLots: 1,
      maxQuoteLots: 100 + (i % 23),
      clientOrderId: BigInt(1_000_000 + i),
      orderType: 2,
      selfTradeBehavior: 0,
      reduceOnly: false,
      expiryTimestamp: 0n,
      limit: 10,
    });

    const transaction = {
      tx_id: txId,
      payload,
      signature: Buffer.from(owner.secretKey.slice(0, 64)),
      public_key: Buffer.from(owner.publicKey.toBytes()),
      nonce: i + 1,
      timestamp: Math.floor(Date.now() * 1000),
      intent_metadata: {
        group: GROUP,
        execution_queue: EXECUTION_QUEUE,
        market: MARKET,
        kind: 0,
        remaining_accounts: [],
        min_execute_slot: 0,
        expires_at_slot: 0,
        user_owner: owner.publicKey.toBase58(),
        mango_account: mangoAccount.publicKey.toBase58(),
      },
    };

    const ackResponse = await submitTransaction(client, transaction);
    record.ackMs = toMillis(originNs, process.hrtime.bigint());
    if (String(ackResponse.sequence_number) !== sequence) {
      throw new Error(
        `Sequence mismatch: expected ${sequence}, got ${ackResponse.sequence_number}`,
      );
    }
    if (SERIAL_WAIT) {
      await waitForRecord(
        records,
        sequence,
        (current) =>
          current.preConfirmMs !== null && current.validatedLocalMs !== null,
        'serial events',
      );
    }
  }

  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    let done = true;
    for (const record of records.values()) {
      if (record.preConfirmMs === null || record.validatedLocalMs === null) {
        done = false;
        break;
      }
    }
    if (done) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }

  sse.close();
  client.close();

  const ackLatencies = [];
  const preConfirmFromSubmit = [];
  const preConfirmFromAck = [];
  const validatedFromSubmit = [];
  const validatedFromAck = [];
  let ackedCount = 0;
  let preConfirmBeforeAck = 0;
  let validatedBeforeAck = 0;
  const incomplete = [];

  for (const record of records.values()) {
    if (record.ackMs !== null) {
      ackedCount += 1;
      ackLatencies.push(record.ackMs);
    }
    if (record.preConfirmMs !== null) {
      preConfirmFromSubmit.push(record.preConfirmMs);
      if (record.ackMs !== null) {
        if (record.preConfirmMs < record.ackMs) {
          preConfirmBeforeAck += 1;
        }
        preConfirmFromAck.push(Math.max(0, record.preConfirmMs - record.ackMs));
      }
    }
    if (record.validatedLocalMs !== null) {
      validatedFromSubmit.push(record.validatedLocalMs);
      if (record.ackMs !== null) {
        if (record.validatedLocalMs < record.ackMs) {
          validatedBeforeAck += 1;
        }
        validatedFromAck.push(
          Math.max(0, record.validatedLocalMs - record.ackMs),
        );
      }
    }
    if (
      record.ackMs === null ||
      record.preConfirmMs === null ||
      record.validatedLocalMs === null
    ) {
      incomplete.push(record.sequence);
    }
  }

  const output = {
    config: {
      harness_url: HARNESS_URL,
      sequencer_addr: SEQUENCER_ADDR,
      tx_count: TX_COUNT,
      market: MARKET,
      group: GROUP,
      execution_queue: EXECUTION_QUEUE,
      owner_mode: OWNER_MODE,
      stream_filter: STREAM_FILTER,
      serial_wait: SERIAL_WAIT,
      owner: sharedOwner ? sharedOwner.publicKey.toBase58() : null,
      mango_account: sharedMangoAccount
        ? sharedMangoAccount.publicKey.toBase58()
        : null,
    },
    counts: {
      submitted: TX_COUNT,
      acked: ackedCount,
      pre_confirmed: eventCounts.pre_confirm,
      validated_local: eventCounts.validated_local,
      complete: TX_COUNT - incomplete.length,
      incomplete_sequences: incomplete,
      pre_confirm_before_ack: preConfirmBeforeAck,
      validated_before_ack: validatedBeforeAck,
    },
    summaries: {
      submit_to_ack_ms: summarize(ackLatencies),
      submit_to_pre_confirm_ms: summarize(preConfirmFromSubmit),
      ack_to_pre_confirm_ms_clamped: summarize(preConfirmFromAck),
      submit_to_validated_local_ms: summarize(validatedFromSubmit),
      ack_to_validated_local_ms_clamped: summarize(validatedFromAck),
    },
  };

  console.log(JSON.stringify(output, null, 2));
  console.log(formatSummary('submit->ack', output.summaries.submit_to_ack_ms));
  console.log(
    formatSummary(
      'submit->pre_confirm',
      output.summaries.submit_to_pre_confirm_ms,
    ),
  );
  console.log(
    formatSummary(
      'ack->pre_confirm(clamped)',
      output.summaries.ack_to_pre_confirm_ms_clamped,
    ),
  );
  console.log(
    formatSummary(
      'submit->validated_local',
      output.summaries.submit_to_validated_local_ms,
    ),
  );
  console.log(
    formatSummary(
      'ack->validated_local(clamped)',
      output.summaries.ack_to_validated_local_ms_clamped,
    ),
  );

  try {
    const diagnostics = await httpGetJson(
      `${HARNESS_URL}/diagnostics/latency?limit=200`,
    );
    const interesting = Array.isArray(diagnostics.components)
      ? diagnostics.components.filter((component) =>
          [
            'sequencer_accept_stream_message',
            'sequencer_tick_stream_message',
            'notify_frontend_preconfirm_subscribers',
            'notify_frontend_validated_local_subscribers',
            'backend_getValidatedLocalPayload',
          ].includes(component.component),
        )
      : [];
    console.log(JSON.stringify({ harness_diagnostics: interesting }, null, 2));
  } catch (err) {
    console.warn(`failed to fetch harness diagnostics: ${err.message}`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
