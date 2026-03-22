import crypto from 'crypto';
import bs58 from 'bs58';

export enum QueueProcessStatus {
  Empty = 0,
  Pending = 1,
  Executed = 2,
  Failed = 3,
  Skipped = 4,
}

export enum QueueItemKindHarness {
  CtmWrapped = 0,
  LiquidityDeposit = 1,
  LiquidityWithdraw = 2,
}

export enum QueuePayloadVariantHarness {
  PerpPlaceOrderV2 = 0,
  PerpCancelOrder = 1,
  PerpCancelOrderByClientOrderId = 2,
  PerpCancelAllOrders = 3,
  PerpCancelAllOrdersBySide = 4,
  LiquidityDeposit = 5,
  LiquidityWithdraw = 6,
}

export type QueueView = 'optimistic' | 'confirmed';

export type AccountMetaWire = {
  pubkey: string;
  is_signer: boolean;
  is_writable: boolean;
};

export type RelayIntentAcceptedEvent = {
  event_type: 'relay_intent_accepted';
  ts_ms: number;
  group: string;
  execution_queue: string;
  market: string;
  sequence: string;
  kind: number;
  payload_b64: string;
  remaining_accounts: AccountMetaWire[];
  min_execute_slot: string;
  expires_at_slot: string;
  user_owner: string;
  mango_account: string;
  enqueue_tx_signature: string;
};

export type QueueItemEnqueuedEvent = {
  event_type: 'queue_item_enqueued';
  ts_ms: number;
  group: string;
  sequence: string;
  kind: number;
  min_execute_slot: string;
  slot: string;
  tx_signature: string;
};

export type QueueItemProcessedEvent = {
  event_type: 'queue_item_processed';
  ts_ms: number;
  group: string;
  sequence: string;
  kind: number;
  status: number;
  slot: string;
  tx_signature: string;
};

export type DivergenceEvent = {
  event_type: 'divergence_event';
  ts_ms: number;
  reason: string;
  key: string;
  details: Record<string, string>;
};

export type HarnessEvent =
  | RelayIntentAcceptedEvent
  | QueueItemEnqueuedEvent
  | QueueItemProcessedEvent
  | DivergenceEvent;

export type QueuePayloadPlaceOrder = {
  variant: QueuePayloadVariantHarness.PerpPlaceOrderV2;
  side: number;
  price_lots: bigint;
  max_base_lots: bigint;
  max_quote_lots: bigint;
  client_order_id: bigint;
  order_type: number;
  self_trade_behavior: number;
  reduce_only: boolean;
  expiry_timestamp: bigint;
  limit: number;
};

export type QueuePayloadCancelOrder = {
  variant: QueuePayloadVariantHarness.PerpCancelOrder;
  order_id: bigint;
};

export type QueuePayloadCancelOrderByClientId = {
  variant: QueuePayloadVariantHarness.PerpCancelOrderByClientOrderId;
  client_order_id: bigint;
};

export type QueuePayloadCancelAll = {
  variant: QueuePayloadVariantHarness.PerpCancelAllOrders;
  limit: number;
};

export type QueuePayloadCancelAllBySide = {
  variant: QueuePayloadVariantHarness.PerpCancelAllOrdersBySide;
  side_option: number | null;
  limit: number;
};

export type QueuePayloadLiquidityDeposit = {
  variant: QueuePayloadVariantHarness.LiquidityDeposit;
  amount: bigint;
  reduce_only: boolean;
};

export type QueuePayloadLiquidityWithdraw = {
  variant: QueuePayloadVariantHarness.LiquidityWithdraw;
  amount: bigint;
  allow_borrow: boolean;
};

export type DecodedQueuePayload =
  | QueuePayloadPlaceOrder
  | QueuePayloadCancelOrder
  | QueuePayloadCancelOrderByClientId
  | QueuePayloadCancelAll
  | QueuePayloadCancelAllBySide
  | QueuePayloadLiquidityDeposit
  | QueuePayloadLiquidityWithdraw;

export type CanonicalIntent = {
  key: string;
  group: string;
  execution_queue: string;
  market: string;
  sequence: bigint;
  kind: number;
  payload: Buffer;
  payload_b64: string;
  decoded_payload: DecodedQueuePayload | null;
  remaining_accounts: AccountMetaWire[];
  min_execute_slot: bigint;
  expires_at_slot: bigint;
  user_owner: string;
  mango_account: string;
  enqueue_tx_signature: string;
  accepted_ts_ms: number;
  enqueued_slot: bigint | null;
  processed_slot: bigint | null;
  processed_status: QueueProcessStatus | null;
  processed_tx_signature: string | null;
};

export type OpenOrderSummary = {
  order_id: string;
  owner: string;
  mango_account: string;
  market: string;
  side: 'bid' | 'ask';
  price_lots: string;
  base_lots: string;
  quote_lots: string;
  client_order_id: string;
  sequence: string;
  expiry_timestamp: string;
  status: 'open';
};

export type MarketState = {
  market: string;
  bids: Array<{ price_lots: string; base_lots: string }>;
  asks: Array<{ price_lots: string; base_lots: string }>;
  open_orders: OpenOrderSummary[];
  watermarks: {
    optimistic_seq: string;
    confirmed_seq: string;
    last_slot: string;
  };
};

export type UserState = {
  owner: string;
  mango_accounts: string[];
  open_orders: OpenOrderSummary[];
  per_market: Array<{
    market: string;
    open_order_base_lots_bid: string;
    open_order_base_lots_ask: string;
    quote_reserved_lots: string;
    base_position_lots: string;
    quote_position_native: string;
  }>;
  margin_summary: MarginSummary;
};

export type QueueState = {
  market: string;
  pending_count: number;
  processed_count: number;
  failed_count: number;
  skipped_count: number;
  last_processed_sequence: string;
  lag_slots: string;
  unmatched_processed_count: number;
};

export type MarketTrade = {
  trade_id: string;
  market: string;
  price_lots: string;
  base_lots: string;
  quote_lots: string;
  taker_side: 'bid' | 'ask';
  maker_owner: string;
  taker_owner: string;
  maker_order_id: string;
  taker_sequence: string;
  ts_ms: number;
  view: QueueView;
};

export type MarketCandle = {
  market: string;
  bucket_start_ts_ms: number;
  resolution_sec: number;
  open_price_lots: string;
  high_price_lots: string;
  low_price_lots: string;
  close_price_lots: string;
  base_volume_lots: string;
  quote_volume_lots: string;
  trade_count: number;
  view: QueueView;
};

export type UserBalances = {
  owner: string;
  mango_accounts: string[];
  per_market: Array<{
    market: string;
    open_order_base_lots_bid: string;
    open_order_base_lots_ask: string;
    quote_reserved_lots: string;
    base_position_lots: string;
    quote_position_native: string;
  }>;
  totals: {
    total_open_order_base_lots_bid: string;
    total_open_order_base_lots_ask: string;
    total_quote_reserved_lots: string;
  };
  margin_summary: MarginSummary;
  view: QueueView;
};

export type MarginSummaryPlaceholder = {
  status: 'placeholder';
  source: 'queue-replay';
};

export type MarginSummaryEmpty = {
  status: 'empty';
  source: 'onchain-mango-health';
  account_count: 0;
  totals: {
    equity_native_quote: string;
    pnl_native_quote: string;
    assets_native_quote: string;
    liabs_native_quote: string;
    init_health_native_quote: string;
    maint_health_native_quote: string;
    margin_usage_fraction: number;
  };
  accounts: [];
};

export type MarginSummaryAccount = {
  mango_account: string;
  owner: string;
  equity_native_quote: string;
  pnl_native_quote: string;
  assets_native_quote: string;
  liabs_native_quote: string;
  init_health_native_quote: string;
  maint_health_native_quote: string;
  init_health_ratio: string;
  maint_health_ratio: string;
  margin_usage_fraction: number;
  perp_positions: Array<{
    market_index: number;
    base_position_lots: string;
    quote_position_native: string;
  }>;
};

export type MarginSummaryOk = {
  status: 'ok';
  source: 'onchain-mango-health';
  account_count: number;
  totals: {
    equity_native_quote: string;
    pnl_native_quote: string;
    assets_native_quote: string;
    liabs_native_quote: string;
    init_health_native_quote: string;
    maint_health_native_quote: string;
    margin_usage_fraction: number;
  };
  // Compatibility fields for consumers that read top-level values.
  equity_native_quote?: string;
  pnl_native_quote?: string;
  assets_native_quote?: string;
  liabs_native_quote?: string;
  init_health_native_quote?: string;
  maint_health_native_quote?: string;
  margin_usage_fraction?: number;
  accounts: MarginSummaryAccount[];
};

export type MarginSummary =
  | MarginSummaryPlaceholder
  | MarginSummaryEmpty
  | MarginSummaryOk;

export type EngineSnapshot = {
  view: QueueView;
  markets: Record<string, MarketState>;
  users: Record<string, UserState>;
  queue: Record<string, QueueState>;
  generated_ts_ms: number;
};

const ENQUEUE_EVENT_DISCRIMINATOR = eventDiscriminator('QueueItemEnqueued');
const PROCESSED_EVENT_DISCRIMINATOR = eventDiscriminator('QueueItemProcessed');

const QUEUE_PAYLOAD_VERSION_V1 = 1;
const QUEUE_PAYLOAD_HEADER_LEN = 4;
const PERP_ORDER_TYPE_LIMIT = 0;
const PERP_ORDER_TYPE_IOC = 1;
const PERP_ORDER_TYPE_POST_ONLY = 2;
const PERP_ORDER_TYPE_MARKET = 3;
const PERP_ORDER_TYPE_POST_ONLY_SLIDE = 4;

function eventDiscriminator(name: string): Buffer {
  return crypto.createHash('sha256').update(`event:${name}`).digest().subarray(0, 8);
}

function readU64(buffer: Buffer, offset: number): bigint {
  return buffer.readBigUInt64LE(offset);
}

function readI64(buffer: Buffer, offset: number): bigint {
  return buffer.readBigInt64LE(offset);
}

function readU128(buffer: Buffer, offset: number): bigint {
  const lo = buffer.readBigUInt64LE(offset);
  const hi = buffer.readBigUInt64LE(offset + 8);
  return (hi << 64n) + lo;
}

function ensureMinLength(buffer: Buffer, expected: number, label: string): void {
  if (buffer.length < expected) {
    throw new Error(`${label} too short: expected at least ${expected}, got ${buffer.length}`);
  }
}

export function queueItemKey(group: string, sequence: string | bigint, kind: string | number): string {
  return `${group}:${sequence.toString()}:${kind.toString()}`;
}

export function statusToString(status: number): string {
  switch (status) {
    case QueueProcessStatus.Empty:
      return 'empty';
    case QueueProcessStatus.Pending:
      return 'pending';
    case QueueProcessStatus.Executed:
      return 'executed';
    case QueueProcessStatus.Failed:
      return 'failed';
    case QueueProcessStatus.Skipped:
      return 'skipped';
    default:
      return `unknown:${status}`;
  }
}

export function parseProgramDataLogLine(line: string): Buffer | null {
  const marker = 'Program data: ';
  const idx = line.indexOf(marker);
  if (idx < 0) {
    return null;
  }
  const b64 = line.slice(idx + marker.length).trim();
  if (!b64.length) {
    return null;
  }
  try {
    return Buffer.from(b64, 'base64');
  } catch {
    return null;
  }
}

export function decodeQueueAnchorEvent(
  encoded: Buffer,
):
  | {
      type: 'QueueItemEnqueued';
      group: string;
      sequence: bigint;
      kind: number;
      min_execute_slot: bigint;
    }
  | {
      type: 'QueueItemProcessed';
      group: string;
      sequence: bigint;
      kind: number;
      status: number;
    }
  | null {
  if (encoded.length < 8) {
    return null;
  }
  const disc = encoded.subarray(0, 8);
  if (disc.equals(ENQUEUE_EVENT_DISCRIMINATOR)) {
    ensureMinLength(encoded, 57, 'QueueItemEnqueued event');
    const groupBytes = encoded.subarray(8, 40);
    const sequence = readU64(encoded, 40);
    const kind = encoded.readUInt8(48);
    const minExecuteSlot = readU64(encoded, 49);
    return {
      type: 'QueueItemEnqueued',
      group: toPubkeyBase58(groupBytes),
      sequence,
      kind,
      min_execute_slot: minExecuteSlot,
    };
  }
  if (disc.equals(PROCESSED_EVENT_DISCRIMINATOR)) {
    ensureMinLength(encoded, 50, 'QueueItemProcessed event');
    const groupBytes = encoded.subarray(8, 40);
    const sequence = readU64(encoded, 40);
    const kind = encoded.readUInt8(48);
    const status = encoded.readUInt8(49);
    return {
      type: 'QueueItemProcessed',
      group: toPubkeyBase58(groupBytes),
      sequence,
      kind,
      status,
    };
  }
  return null;
}

function toPubkeyBase58(bytes: Uint8Array): string {
  return bs58.encode(Buffer.from(bytes));
}

export function decodeQueuePayload(payload: Buffer): DecodedQueuePayload {
  ensureMinLength(payload, QUEUE_PAYLOAD_HEADER_LEN, 'queue payload');
  const version = payload.readUInt8(0);
  if (version !== QUEUE_PAYLOAD_VERSION_V1) {
    throw new Error(`unsupported queue payload version: ${version}`);
  }
  const variant = payload.readUInt8(1);
  const flags = payload.readUInt16LE(2);
  if (flags !== 0) {
    throw new Error(`unsupported queue payload flags: ${flags}`);
  }
  const body = payload.subarray(4);

  switch (variant) {
    case QueuePayloadVariantHarness.PerpPlaceOrderV2: {
      ensureMinLength(body, 45, 'perp place-order payload body');
      return {
        variant,
        side: body.readUInt8(0),
        price_lots: readI64(body, 1),
        max_base_lots: readI64(body, 9),
        max_quote_lots: readI64(body, 17),
        client_order_id: readU64(body, 25),
        order_type: body.readUInt8(33),
        self_trade_behavior: body.readUInt8(34),
        reduce_only: body.readUInt8(35) === 1,
        expiry_timestamp: readU64(body, 36),
        limit: body.readUInt8(44),
      };
    }
    case QueuePayloadVariantHarness.PerpCancelOrder: {
      ensureMinLength(body, 16, 'perp cancel-order payload body');
      return {
        variant,
        order_id: readU128(body, 0),
      };
    }
    case QueuePayloadVariantHarness.PerpCancelOrderByClientOrderId: {
      ensureMinLength(body, 8, 'perp cancel-order-by-client-id payload body');
      return {
        variant,
        client_order_id: readU64(body, 0),
      };
    }
    case QueuePayloadVariantHarness.PerpCancelAllOrders: {
      ensureMinLength(body, 1, 'perp cancel-all payload body');
      return {
        variant,
        limit: body.readUInt8(0),
      };
    }
    case QueuePayloadVariantHarness.PerpCancelAllOrdersBySide: {
      ensureMinLength(body, 2, 'perp cancel-all-by-side payload body');
      const hasSide = body.readUInt8(0) === 1;
      ensureMinLength(
        body,
        hasSide ? 3 : 2,
        'perp cancel-all-by-side payload body',
      );
      return {
        variant,
        side_option: hasSide ? body.readUInt8(1) : null,
        limit: body.readUInt8(hasSide ? 2 : 1),
      };
    }
    case QueuePayloadVariantHarness.LiquidityDeposit: {
      ensureMinLength(body, 9, 'liquidity deposit payload body');
      return {
        variant,
        amount: readU64(body, 0),
        reduce_only: body.readUInt8(8) === 1,
      };
    }
    case QueuePayloadVariantHarness.LiquidityWithdraw: {
      ensureMinLength(body, 9, 'liquidity withdraw payload body');
      return {
        variant,
        amount: readU64(body, 0),
        allow_borrow: body.readUInt8(8) === 1,
      };
    }
    default:
      throw new Error(`unsupported queue payload variant: ${variant}`);
  }
}

type InternalOrder = {
  order_id: string;
  owner: string;
  mango_account: string;
  market: string;
  side: 'bid' | 'ask';
  price_lots: bigint;
  base_lots: bigint;
  quote_lots: bigint;
  client_order_id: bigint;
  sequence: bigint;
  expiry_timestamp: bigint;
};

type InternalTrade = {
  trade_id: string;
  market: string;
  price_lots: bigint;
  base_lots: bigint;
  quote_lots: bigint;
  taker_side: 'bid' | 'ask';
  maker_owner: string;
  taker_owner: string;
  maker_order_id: string;
  taker_sequence: bigint;
  ts_ms: number;
};

type InternalMarket = {
  market: string;
  bids: Map<string, bigint>;
  asks: Map<string, bigint>;
  orders: Map<string, InternalOrder>;
  trades: InternalTrade[];
  optimistic_seq: bigint;
  confirmed_seq: bigint;
};

type InternalUser = {
  owner: string;
  mango_accounts: Set<string>;
  orders: Set<string>;
};

type InternalQueue = {
  market: string;
  pending_count: number;
  processed_count: number;
  failed_count: number;
  skipped_count: number;
  last_processed_sequence: bigint;
  min_pending_execute_slot: bigint | null;
  unmatched_processed_count: number;
};

type InternalProjection = {
  markets: Map<string, InternalMarket>;
  users: Map<string, InternalUser>;
  queue: Map<string, InternalQueue>;
  last_slot: bigint;
};

export class ContinuumStateEngine {
  private readonly intentsByKey = new Map<string, CanonicalIntent>();
  private readonly processedEventIds = new Set<string>();
  private readonly enqueuedEventIds = new Set<string>();
  private readonly divergences: DivergenceEvent[] = [];
  private readonly listeners = new Set<(event: HarnessEvent) => void>();
  private revision = 0;
  private lastSeenSlot = 0n;
  private cachedConfirmed: { revision: number; snapshot: EngineSnapshot } | null = null;
  private cachedOptimistic: { revision: number; snapshot: EngineSnapshot } | null = null;

  subscribe(listener: (event: HarnessEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  listDivergences(limit = 200): DivergenceEvent[] {
    return this.divergences.slice(Math.max(0, this.divergences.length - limit));
  }

  reportExternalDivergence(
    reason: string,
    key: string,
    details: Record<string, string>,
  ): void {
    this.emitDivergence(reason, key, details);
  }

  listIntents(): CanonicalIntent[] {
    return Array.from(this.intentsByKey.values()).sort((a, b) => {
      if (a.group !== b.group) {
        return a.group.localeCompare(b.group);
      }
      if (a.market !== b.market) {
        return a.market.localeCompare(b.market);
      }
      if (a.sequence !== b.sequence) {
        return a.sequence < b.sequence ? -1 : 1;
      }
      return a.accepted_ts_ms - b.accepted_ts_ms;
    });
  }

  ingestRelayIntent(event: RelayIntentAcceptedEvent): void {
    const payload = Buffer.from(event.payload_b64, 'base64');
    let decodedPayload: DecodedQueuePayload | null = null;
    try {
      decodedPayload = decodeQueuePayload(payload);
    } catch {
      decodedPayload = null;
    }

    const key = queueItemKey(event.group, event.sequence, event.kind);
    const existing = this.intentsByKey.get(key);

    const canonical: CanonicalIntent = {
      key,
      group: event.group,
      execution_queue: event.execution_queue,
      market: event.market,
      sequence: BigInt(event.sequence),
      kind: event.kind,
      payload,
      payload_b64: event.payload_b64,
      decoded_payload: decodedPayload,
      remaining_accounts: event.remaining_accounts || [],
      min_execute_slot: BigInt(event.min_execute_slot || '0'),
      expires_at_slot: BigInt(event.expires_at_slot || '0'),
      user_owner: event.user_owner,
      mango_account: event.mango_account,
      enqueue_tx_signature: event.enqueue_tx_signature,
      accepted_ts_ms: event.ts_ms || Date.now(),
      enqueued_slot: existing?.enqueued_slot || null,
      processed_slot: existing?.processed_slot || null,
      processed_status: existing?.processed_status || null,
      processed_tx_signature: existing?.processed_tx_signature || null,
    };

    if (existing && existing.enqueue_tx_signature !== canonical.enqueue_tx_signature) {
      this.emitDivergence('duplicate_key_different_signature', key, {
        existing_signature: existing.enqueue_tx_signature,
        incoming_signature: canonical.enqueue_tx_signature,
      });
    }

    this.intentsByKey.set(key, canonical);
    this.revision += 1;
    this.emitEvent(event);
  }

  ingestQueueEnqueued(event: QueueItemEnqueuedEvent): void {
    const id = `${event.tx_signature}:${event.group}:${event.sequence}:${event.kind}`;
    if (this.enqueuedEventIds.has(id)) {
      return;
    }
    this.enqueuedEventIds.add(id);

    const key = queueItemKey(event.group, event.sequence, event.kind);
    const existing = this.intentsByKey.get(key);
    const slot = BigInt(event.slot || '0');
    if (slot > this.lastSeenSlot) {
      this.lastSeenSlot = slot;
    }

    if (existing) {
      existing.enqueued_slot = slot;
      existing.min_execute_slot = BigInt(event.min_execute_slot || existing.min_execute_slot.toString());
    }

    this.revision += 1;
    this.emitEvent(event);
  }

  ingestQueueProcessed(event: QueueItemProcessedEvent): void {
    const id = `${event.tx_signature}:${event.group}:${event.sequence}:${event.kind}:${event.status}`;
    if (this.processedEventIds.has(id)) {
      return;
    }
    this.processedEventIds.add(id);

    const key = queueItemKey(event.group, event.sequence, event.kind);
    const intent = this.intentsByKey.get(key);
    const slot = BigInt(event.slot || '0');
    if (slot > this.lastSeenSlot) {
      this.lastSeenSlot = slot;
    }

    if (!intent) {
      this.emitDivergence('processed_without_relay_intent', key, {
        status: event.status.toString(),
        slot: event.slot,
        tx_signature: event.tx_signature,
      });
      const placeholder: CanonicalIntent = {
        key,
        group: event.group,
        execution_queue: 'unknown',
        market: 'unknown',
        sequence: BigInt(event.sequence),
        kind: event.kind,
        payload: Buffer.alloc(0),
        payload_b64: '',
        decoded_payload: null,
        remaining_accounts: [],
        min_execute_slot: 0n,
        expires_at_slot: 0n,
        user_owner: 'unknown',
        mango_account: 'unknown',
        enqueue_tx_signature: 'unknown',
        accepted_ts_ms: event.ts_ms || Date.now(),
        enqueued_slot: null,
        processed_slot: slot,
        processed_status: event.status as QueueProcessStatus,
        processed_tx_signature: event.tx_signature,
      };
      this.intentsByKey.set(key, placeholder);
      this.revision += 1;
      this.emitEvent(event);
      return;
    }

    if (
      intent.processed_status !== null &&
      intent.processed_status !== event.status
    ) {
      this.emitDivergence('processed_status_changed', key, {
        previous_status: intent.processed_status.toString(),
        incoming_status: event.status.toString(),
      });
    }

    intent.processed_status = event.status as QueueProcessStatus;
    intent.processed_slot = slot;
    intent.processed_tx_signature = event.tx_signature;

    this.revision += 1;
    this.emitEvent(event);
  }

  getMarketState(market: string, view: QueueView): MarketState {
    const snapshot = this.getSnapshot(view);
    return (
      snapshot.markets[market] || {
        market,
        bids: [],
        asks: [],
        open_orders: [],
        watermarks: {
          optimistic_seq: '0',
          confirmed_seq: '0',
          last_slot: this.lastSeenSlot.toString(),
        },
      }
    );
  }

  getUserState(owner: string, view: QueueView): UserState {
    const snapshot = this.getSnapshot(view);
    return (
      snapshot.users[owner] || {
        owner,
        mango_accounts: [],
        open_orders: [],
        per_market: [],
        margin_summary: {
          status: 'placeholder',
          source: 'queue-replay',
        },
      }
    );
  }

  getOrders(market: string, owner: string | null, view: QueueView): OpenOrderSummary[] {
    const marketState = this.getMarketState(market, view);
    if (!owner) {
      return marketState.open_orders;
    }
    return marketState.open_orders.filter((o) => o.owner === owner);
  }

  getQueueState(market: string): QueueState {
    const optimistic = this.getSnapshot('optimistic');
    return (
      optimistic.queue[market] || {
        market,
        pending_count: 0,
        processed_count: 0,
        failed_count: 0,
        skipped_count: 0,
        last_processed_sequence: '0',
        lag_slots: '0',
        unmatched_processed_count: 0,
      }
    );
  }

  getBalances(owner: string, view: QueueView): UserBalances {
    const user = this.getUserState(owner, view);
    let totalBid = 0n;
    let totalAsk = 0n;
    let totalQuoteReserved = 0n;
    for (const entry of user.per_market) {
      totalBid += BigInt(entry.open_order_base_lots_bid);
      totalAsk += BigInt(entry.open_order_base_lots_ask);
      totalQuoteReserved += BigInt(entry.quote_reserved_lots);
    }

    return {
      owner: user.owner,
      mango_accounts: user.mango_accounts,
      per_market: user.per_market,
      totals: {
        total_open_order_base_lots_bid: totalBid.toString(),
        total_open_order_base_lots_ask: totalAsk.toString(),
        total_quote_reserved_lots: totalQuoteReserved.toString(),
      },
      margin_summary: user.margin_summary,
      view,
    };
  }

  getTrades(
    market: string,
    view: QueueView,
    limit = 200,
  ): MarketTrade[] {
    const projection = this.buildProjection(view);
    const state = projection.markets.get(market);
    if (!state) {
      return [];
    }
    const capped = Math.max(0, limit);
    return state.trades
      .slice(Math.max(0, state.trades.length - capped))
      .map((trade) => ({
        trade_id: trade.trade_id,
        market: trade.market,
        price_lots: trade.price_lots.toString(),
        base_lots: trade.base_lots.toString(),
        quote_lots: trade.quote_lots.toString(),
        taker_side: trade.taker_side,
        maker_owner: trade.maker_owner,
        taker_owner: trade.taker_owner,
        maker_order_id: trade.maker_order_id,
        taker_sequence: trade.taker_sequence.toString(),
        ts_ms: trade.ts_ms,
        view,
      }));
  }

  getCandles(
    market: string,
    view: QueueView,
    resolutionSec: number,
    limit = 200,
  ): MarketCandle[] {
    const safeResolution = Number.isFinite(resolutionSec) && resolutionSec > 0
      ? Math.floor(resolutionSec)
      : 60;
    const trades = this.getTrades(market, view, 10000);
    if (!trades.length) {
      return [];
    }

    const bucketSizeMs = safeResolution * 1000;
    const buckets = new Map<number, MarketCandle>();
    for (const trade of trades) {
      const bucketStart = Math.floor(trade.ts_ms / bucketSizeMs) * bucketSizeMs;
      const price = BigInt(trade.price_lots);
      const baseLots = BigInt(trade.base_lots);
      const quoteLots = BigInt(trade.quote_lots);
      const existing = buckets.get(bucketStart);
      if (!existing) {
        buckets.set(bucketStart, {
          market,
          bucket_start_ts_ms: bucketStart,
          resolution_sec: safeResolution,
          open_price_lots: price.toString(),
          high_price_lots: price.toString(),
          low_price_lots: price.toString(),
          close_price_lots: price.toString(),
          base_volume_lots: baseLots.toString(),
          quote_volume_lots: quoteLots.toString(),
          trade_count: 1,
          view,
        });
        continue;
      }

      existing.close_price_lots = price.toString();
      if (price > BigInt(existing.high_price_lots)) {
        existing.high_price_lots = price.toString();
      }
      if (price < BigInt(existing.low_price_lots)) {
        existing.low_price_lots = price.toString();
      }
      existing.base_volume_lots = (
        BigInt(existing.base_volume_lots) + baseLots
      ).toString();
      existing.quote_volume_lots = (
        BigInt(existing.quote_volume_lots) + quoteLots
      ).toString();
      existing.trade_count += 1;
    }

    return Array.from(buckets.values())
      .sort((a, b) => a.bucket_start_ts_ms - b.bucket_start_ts_ms)
      .slice(Math.max(0, buckets.size - Math.max(0, limit)));
  }

  getSnapshot(view: QueueView): EngineSnapshot {
    const cached = view === 'confirmed' ? this.cachedConfirmed : this.cachedOptimistic;
    if (cached && cached.revision === this.revision) {
      return cached.snapshot;
    }

    const snapshot = this.buildSnapshot(view);
    if (view === 'confirmed') {
      this.cachedConfirmed = { revision: this.revision, snapshot };
    } else {
      this.cachedOptimistic = { revision: this.revision, snapshot };
    }
    return snapshot;
  }

  private buildSnapshot(view: QueueView): EngineSnapshot {
    const projection = this.buildProjection(view);
    const positionByOwner = new Map<
      string,
      Map<string, { basePositionLots: bigint; quotePositionNative: bigint }>
    >();

    const addPositionDelta = (
      owner: string,
      market: string,
      baseDelta: bigint,
      quoteDelta: bigint,
    ): void => {
      const ownerPositions = positionByOwner.get(owner) || new Map();
      const current =
        ownerPositions.get(market) || {
          basePositionLots: 0n,
          quotePositionNative: 0n,
        };
      current.basePositionLots += baseDelta;
      current.quotePositionNative += quoteDelta;
      ownerPositions.set(market, current);
      positionByOwner.set(owner, ownerPositions);
    };

    for (const market of projection.markets.values()) {
      for (const trade of market.trades) {
        if (trade.taker_side === 'bid') {
          // Taker buys base, maker sells base.
          addPositionDelta(
            trade.taker_owner,
            market.market,
            trade.base_lots,
            -trade.quote_lots,
          );
          addPositionDelta(
            trade.maker_owner,
            market.market,
            -trade.base_lots,
            trade.quote_lots,
          );
        } else {
          // Taker sells base, maker buys base.
          addPositionDelta(
            trade.taker_owner,
            market.market,
            -trade.base_lots,
            trade.quote_lots,
          );
          addPositionDelta(
            trade.maker_owner,
            market.market,
            trade.base_lots,
            -trade.quote_lots,
          );
        }
      }
    }

    const markets: Record<string, MarketState> = {};
    for (const [market, state] of projection.markets.entries()) {
      const bids = Array.from(state.bids.entries())
        .map(([priceLots, baseLots]) => ({
          price_lots: priceLots,
          base_lots: baseLots.toString(),
        }))
        .sort((a, b) => {
          const av = BigInt(a.price_lots);
          const bv = BigInt(b.price_lots);
          if (av === bv) {
            return 0;
          }
          return av > bv ? -1 : 1;
        });
      const asks = Array.from(state.asks.entries())
        .map(([priceLots, baseLots]) => ({
          price_lots: priceLots,
          base_lots: baseLots.toString(),
        }))
        .sort((a, b) => {
          const av = BigInt(a.price_lots);
          const bv = BigInt(b.price_lots);
          if (av === bv) {
            return 0;
          }
          return av < bv ? -1 : 1;
        });

      const openOrders = Array.from(state.orders.values()).map((o) =>
        this.orderToSummary(o),
      );

      markets[market] = {
        market,
        bids,
        asks,
        open_orders: openOrders,
        watermarks: {
          optimistic_seq: state.optimistic_seq.toString(),
          confirmed_seq: state.confirmed_seq.toString(),
          last_slot: projection.last_slot.toString(),
        },
      };
    }

    const users: Record<string, UserState> = {};
    for (const owner of positionByOwner.keys()) {
      if (!projection.users.has(owner)) {
        projection.users.set(owner, {
          owner,
          mango_accounts: new Set(),
          orders: new Set(),
        });
      }
    }
    for (const [owner, user] of projection.users.entries()) {
      const orders = Array.from(user.orders)
        .map((orderId) =>
          Array.from(projection.markets.values()).find((m) => m.orders.has(orderId))?.orders.get(orderId),
        )
        .filter((o): o is InternalOrder => !!o)
        .map((o) => this.orderToSummary(o));

      const perMarketMap = new Map<
        string,
        {
          openBid: bigint;
          openAsk: bigint;
          quoteReserved: bigint;
          basePositionLots: bigint;
          quotePositionNative: bigint;
        }
      >();
      for (const o of orders) {
        const current = perMarketMap.get(o.market) || {
          openBid: 0n,
          openAsk: 0n,
          quoteReserved: 0n,
          basePositionLots: 0n,
          quotePositionNative: 0n,
        };
        if (o.side === 'bid') {
          current.openBid += BigInt(o.base_lots);
        } else {
          current.openAsk += BigInt(o.base_lots);
        }
        current.quoteReserved += BigInt(o.quote_lots);
        perMarketMap.set(o.market, current);
      }
      const ownerPositions = positionByOwner.get(owner);
      if (ownerPositions) {
        for (const [market, pos] of ownerPositions.entries()) {
          const current = perMarketMap.get(market) || {
            openBid: 0n,
            openAsk: 0n,
            quoteReserved: 0n,
            basePositionLots: 0n,
            quotePositionNative: 0n,
          };
          current.basePositionLots += pos.basePositionLots;
          current.quotePositionNative += pos.quotePositionNative;
          perMarketMap.set(market, current);
        }
      }

      users[owner] = {
        owner,
        mango_accounts: Array.from(user.mango_accounts),
        open_orders: orders,
        per_market: Array.from(perMarketMap.entries()).map(([market, agg]) => ({
          market,
          open_order_base_lots_bid: agg.openBid.toString(),
          open_order_base_lots_ask: agg.openAsk.toString(),
          quote_reserved_lots: agg.quoteReserved.toString(),
          base_position_lots: agg.basePositionLots.toString(),
          quote_position_native: agg.quotePositionNative.toString(),
        })),
        margin_summary: {
          status: 'placeholder',
          source: 'queue-replay',
        },
      };
    }

    const queue: Record<string, QueueState> = {};
    for (const [market, q] of projection.queue.entries()) {
      const lag =
        q.min_pending_execute_slot && projection.last_slot > q.min_pending_execute_slot
          ? projection.last_slot - q.min_pending_execute_slot
          : 0n;
      queue[market] = {
        market,
        pending_count: q.pending_count,
        processed_count: q.processed_count,
        failed_count: q.failed_count,
        skipped_count: q.skipped_count,
        last_processed_sequence: q.last_processed_sequence.toString(),
        lag_slots: lag.toString(),
        unmatched_processed_count: q.unmatched_processed_count,
      };
    }

    return {
      view,
      markets,
      users,
      queue,
      generated_ts_ms: Date.now(),
    };
  }

  private buildProjection(view: QueueView): InternalProjection {
    const projection: InternalProjection = {
      markets: new Map(),
      users: new Map(),
      queue: new Map(),
      last_slot: this.lastSeenSlot,
    };

    const intents = this.listIntents();
    const confirmed = intents
      .filter((it) => it.processed_status === QueueProcessStatus.Executed)
      .sort(orderIntentsDeterministically);

    for (const intent of confirmed) {
      this.applyIntent(intent, projection);
      const marketQueue = this.getOrCreateQueue(projection, intent.market);
      marketQueue.processed_count += 1;
      marketQueue.last_processed_sequence = maxBigint(
        marketQueue.last_processed_sequence,
        intent.sequence,
      );
    }

    for (const intent of intents) {
      const marketQueue = this.getOrCreateQueue(projection, intent.market);
      if (intent.market === 'unknown' && intent.processed_status !== null) {
        marketQueue.unmatched_processed_count += 1;
      }
      if (intent.processed_status === QueueProcessStatus.Failed) {
        marketQueue.processed_count += 1;
        marketQueue.failed_count += 1;
      } else if (intent.processed_status === QueueProcessStatus.Skipped) {
        marketQueue.processed_count += 1;
        marketQueue.skipped_count += 1;
      } else if (intent.processed_status === null) {
        marketQueue.pending_count += 1;
        if (marketQueue.min_pending_execute_slot === null) {
          marketQueue.min_pending_execute_slot = intent.min_execute_slot;
        } else {
          marketQueue.min_pending_execute_slot = minBigint(
            marketQueue.min_pending_execute_slot,
            intent.min_execute_slot,
          );
        }
      }
    }

    if (view === 'optimistic') {
      const optimisticPending = intents
        .filter((it) => it.processed_status === null)
        .sort(orderIntentsDeterministically);
      for (const intent of optimisticPending) {
        this.applyIntent(intent, projection);
      }
    }

    this.pruneExpiredOrders(projection, BigInt(Math.floor(Date.now() / 1000)));

    for (const market of projection.markets.values()) {
      const marketIntents = intents.filter((it) => it.market === market.market);
      const optimisticSeq = maxBigintFrom(
        marketIntents
          .filter((it) => it.kind === QueueItemKindHarness.CtmWrapped)
          .map((it) => it.sequence),
      );
      const confirmedSeq = maxBigintFrom(
        marketIntents
          .filter(
            (it) =>
              it.kind === QueueItemKindHarness.CtmWrapped &&
              it.processed_status === QueueProcessStatus.Executed,
          )
          .map((it) => it.sequence),
      );
      market.optimistic_seq = optimisticSeq;
      market.confirmed_seq = confirmedSeq;
    }

    return projection;
  }

  private applyIntent(intent: CanonicalIntent, projection: InternalProjection): void {
    if (!intent.decoded_payload) {
      return;
    }

    const market = this.getOrCreateMarket(projection, intent.market);
    const user = this.getOrCreateUser(projection, intent.user_owner);
    user.mango_accounts.add(intent.mango_account);

    const payload = intent.decoded_payload;
    switch (payload.variant) {
      case QueuePayloadVariantHarness.PerpPlaceOrderV2: {
        this.applyPerpPlaceOrderIntent(intent, payload, projection, market, user);
        return;
      }

      case QueuePayloadVariantHarness.PerpCancelOrderByClientOrderId: {
        this.cancelOrders(market, projection, (order) => {
          return (
            order.owner === intent.user_owner &&
            order.market === intent.market &&
            order.client_order_id === payload.client_order_id
          );
        });
        return;
      }

      case QueuePayloadVariantHarness.PerpCancelOrder: {
        const targetOrderId = payload.order_id.toString();
        this.cancelOrders(market, projection, (order) => {
          return (
            order.owner === intent.user_owner &&
            (order.order_id === targetOrderId ||
              order.order_id.endsWith(`:${targetOrderId}`))
          );
        });
        return;
      }

      case QueuePayloadVariantHarness.PerpCancelAllOrders: {
        this.cancelOrders(market, projection, (order) => {
          return order.owner === intent.user_owner && order.market === intent.market;
        });
        return;
      }

      case QueuePayloadVariantHarness.PerpCancelAllOrdersBySide: {
        const desiredSide =
          payload.side_option === null ? null : payload.side_option === 0 ? 'bid' : 'ask';
        this.cancelOrders(market, projection, (order) => {
          if (order.owner !== intent.user_owner || order.market !== intent.market) {
            return false;
          }
          if (desiredSide === null) {
            return true;
          }
          return order.side === desiredSide;
        });
        return;
      }

      case QueuePayloadVariantHarness.LiquidityDeposit:
      case QueuePayloadVariantHarness.LiquidityWithdraw:
        return;

      default:
        return;
    }
  }

  private applyPerpPlaceOrderIntent(
    intent: CanonicalIntent,
    payload: QueuePayloadPlaceOrder,
    projection: InternalProjection,
    market: InternalMarket,
    takerUser: InternalUser,
  ): void {
    const nowTs = BigInt(Math.floor(intent.accepted_ts_ms / 1000));
    this.pruneExpiredOrdersForMarket(market, projection, nowTs);

    const side: 'bid' | 'ask' = payload.side === 0 ? 'bid' : 'ask';
    let remainingBaseLots =
      payload.max_base_lots < 0n ? -payload.max_base_lots : payload.max_base_lots;
    if (remainingBaseLots <= 0n) {
      return;
    }
    const quoteLimitLots =
      payload.max_quote_lots < 0n ? -payload.max_quote_lots : payload.max_quote_lots;

    const orderType = payload.order_type;
    const isPostOnly =
      orderType === PERP_ORDER_TYPE_POST_ONLY ||
      orderType === PERP_ORDER_TYPE_POST_ONLY_SLIDE;
    const isImmediateOnly =
      orderType === PERP_ORDER_TYPE_IOC || orderType === PERP_ORDER_TYPE_MARKET;
    const canTake = !isPostOnly;
    const canRest = !isImmediateOnly && !isPostOnly;

    const opposing = this.findCrossingOrders(market, side, payload.price_lots);
    if (isPostOnly && opposing.length > 0) {
      return;
    }

    let quoteConsumed = 0n;
    if (canTake && opposing.length > 0) {
      for (const makerOrder of opposing) {
        if (remainingBaseLots <= 0n) {
          break;
        }
        if (!market.orders.has(makerOrder.order_id)) {
          continue;
        }
        const matchBaseLots = minBigint(remainingBaseLots, makerOrder.base_lots);
        if (matchBaseLots <= 0n) {
          continue;
        }
        const matchQuoteLots = matchBaseLots * makerOrder.price_lots;
        if (
          quoteLimitLots > 0n &&
          quoteConsumed + matchQuoteLots > quoteLimitLots
        ) {
          if (quoteConsumed >= quoteLimitLots) {
            break;
          }
          const allowedBase = (quoteLimitLots - quoteConsumed) / makerOrder.price_lots;
          if (allowedBase <= 0n) {
            break;
          }
          remainingBaseLots -= allowedBase;
          quoteConsumed += allowedBase * makerOrder.price_lots;
          this.decrementMakerOrder(
            market,
            projection,
            makerOrder,
            allowedBase,
          );
          this.appendTrade(market, {
            trade_id: `${intent.group}:${intent.market}:${intent.sequence.toString()}:${makerOrder.order_id}:${market.trades.length}`,
            market: intent.market,
            price_lots: makerOrder.price_lots,
            base_lots: allowedBase,
            quote_lots: allowedBase * makerOrder.price_lots,
            taker_side: side,
            maker_owner: makerOrder.owner,
            taker_owner: intent.user_owner,
            maker_order_id: makerOrder.order_id,
            taker_sequence: intent.sequence,
            ts_ms: intent.accepted_ts_ms,
          });
          break;
        }

        remainingBaseLots -= matchBaseLots;
        quoteConsumed += matchQuoteLots;
        this.decrementMakerOrder(market, projection, makerOrder, matchBaseLots);
        this.appendTrade(market, {
          trade_id: `${intent.group}:${intent.market}:${intent.sequence.toString()}:${makerOrder.order_id}:${market.trades.length}`,
          market: intent.market,
          price_lots: makerOrder.price_lots,
          base_lots: matchBaseLots,
          quote_lots: matchQuoteLots,
          taker_side: side,
          maker_owner: makerOrder.owner,
          taker_owner: intent.user_owner,
          maker_order_id: makerOrder.order_id,
          taker_sequence: intent.sequence,
          ts_ms: intent.accepted_ts_ms,
        });
      }
    }

    if (!canRest || remainingBaseLots <= 0n) {
      return;
    }

    if (this.isExpiredAt(payload.expiry_timestamp, nowTs)) {
      return;
    }

    const restQuoteLots = payload.price_lots * remainingBaseLots;
    const orderId = `${intent.group}:${intent.market}:${intent.sequence.toString()}:${payload.client_order_id.toString()}`;
    const order: InternalOrder = {
      order_id: orderId,
      owner: intent.user_owner,
      mango_account: intent.mango_account,
      market: intent.market,
      side,
      price_lots: payload.price_lots,
      base_lots: remainingBaseLots,
      quote_lots: restQuoteLots > 0n ? restQuoteLots : 0n,
      client_order_id: payload.client_order_id,
      sequence: intent.sequence,
      expiry_timestamp: payload.expiry_timestamp,
    };
    market.orders.set(orderId, order);
    takerUser.orders.add(orderId);
    this.accumulateDepth(market, side, payload.price_lots, remainingBaseLots);
  }

  private findCrossingOrders(
    market: InternalMarket,
    takerSide: 'bid' | 'ask',
    takerPriceLots: bigint,
  ): InternalOrder[] {
    const candidate = Array.from(market.orders.values()).filter((order) => {
      if (order.base_lots <= 0n) {
        return false;
      }
      if (takerSide === 'bid') {
        return order.side === 'ask' && order.price_lots <= takerPriceLots;
      }
      return order.side === 'bid' && order.price_lots >= takerPriceLots;
    });

    candidate.sort((a, b) => {
      if (a.price_lots !== b.price_lots) {
        if (takerSide === 'bid') {
          return a.price_lots < b.price_lots ? -1 : 1;
        }
        return a.price_lots > b.price_lots ? -1 : 1;
      }
      if (a.sequence !== b.sequence) {
        return a.sequence < b.sequence ? -1 : 1;
      }
      return a.order_id.localeCompare(b.order_id);
    });

    return candidate;
  }

  private decrementMakerOrder(
    market: InternalMarket,
    projection: InternalProjection,
    makerOrder: InternalOrder,
    matchedBaseLots: bigint,
  ): void {
    this.accumulateDepth(
      market,
      makerOrder.side,
      makerOrder.price_lots,
      -matchedBaseLots,
    );
    makerOrder.base_lots -= matchedBaseLots;
    makerOrder.quote_lots = makerOrder.base_lots * makerOrder.price_lots;
    if (makerOrder.base_lots <= 0n) {
      market.orders.delete(makerOrder.order_id);
      const makerUser = this.getOrCreateUser(projection, makerOrder.owner);
      makerUser.orders.delete(makerOrder.order_id);
    } else {
      market.orders.set(makerOrder.order_id, makerOrder);
    }
  }

  private appendTrade(market: InternalMarket, trade: InternalTrade): void {
    market.trades.push(trade);
    if (market.trades.length > 5000) {
      market.trades.splice(0, market.trades.length - 5000);
    }
  }

  private accumulateDepth(
    market: InternalMarket,
    side: 'bid' | 'ask',
    priceLots: bigint,
    deltaBaseLots: bigint,
  ): void {
    const depth = side === 'bid' ? market.bids : market.asks;
    const key = priceLots.toString();
    const current = depth.get(key) || 0n;
    const next = current + deltaBaseLots;
    if (next <= 0n) {
      depth.delete(key);
    } else {
      depth.set(key, next);
    }
  }

  private cancelOrders(
    market: InternalMarket,
    projection: InternalProjection,
    matcher: (order: InternalOrder) => boolean,
  ): void {
    for (const [orderId, order] of market.orders.entries()) {
      if (!matcher(order)) {
        continue;
      }
      this.accumulateDepth(market, order.side, order.price_lots, -order.base_lots);
      market.orders.delete(orderId);
      const ownerUser = this.getOrCreateUser(projection, order.owner);
      ownerUser.orders.delete(orderId);
    }
  }

  private isExpiredAt(expiryTimestamp: bigint, nowTs: bigint): boolean {
    return expiryTimestamp > 0n && nowTs >= expiryTimestamp;
  }

  private removeOrder(
    market: InternalMarket,
    projection: InternalProjection,
    order: InternalOrder,
  ): void {
    this.accumulateDepth(market, order.side, order.price_lots, -order.base_lots);
    market.orders.delete(order.order_id);
    const ownerUser = this.getOrCreateUser(projection, order.owner);
    ownerUser.orders.delete(order.order_id);
  }

  private pruneExpiredOrdersForMarket(
    market: InternalMarket,
    projection: InternalProjection,
    nowTs: bigint,
  ): void {
    for (const order of Array.from(market.orders.values())) {
      if (!this.isExpiredAt(order.expiry_timestamp, nowTs)) {
        continue;
      }
      this.removeOrder(market, projection, order);
    }
  }

  private pruneExpiredOrders(
    projection: InternalProjection,
    nowTs: bigint,
  ): void {
    for (const market of projection.markets.values()) {
      this.pruneExpiredOrdersForMarket(market, projection, nowTs);
    }
  }

  private getOrCreateMarket(projection: InternalProjection, market: string): InternalMarket {
    const existing = projection.markets.get(market);
    if (existing) {
      return existing;
    }
    const created: InternalMarket = {
      market,
      bids: new Map(),
      asks: new Map(),
      orders: new Map(),
      trades: [],
      optimistic_seq: 0n,
      confirmed_seq: 0n,
    };
    projection.markets.set(market, created);
    return created;
  }

  private getOrCreateUser(projection: InternalProjection, owner: string): InternalUser {
    const existing = projection.users.get(owner);
    if (existing) {
      return existing;
    }
    const created: InternalUser = {
      owner,
      mango_accounts: new Set(),
      orders: new Set(),
    };
    projection.users.set(owner, created);
    return created;
  }

  private getOrCreateQueue(projection: InternalProjection, market: string): InternalQueue {
    const existing = projection.queue.get(market);
    if (existing) {
      return existing;
    }
    const created: InternalQueue = {
      market,
      pending_count: 0,
      processed_count: 0,
      failed_count: 0,
      skipped_count: 0,
      last_processed_sequence: 0n,
      min_pending_execute_slot: null,
      unmatched_processed_count: 0,
    };
    projection.queue.set(market, created);
    return created;
  }

  private orderToSummary(o: InternalOrder): OpenOrderSummary {
    return {
      order_id: o.order_id,
      owner: o.owner,
      mango_account: o.mango_account,
      market: o.market,
      side: o.side,
      price_lots: o.price_lots.toString(),
      base_lots: o.base_lots.toString(),
      quote_lots: o.quote_lots.toString(),
      client_order_id: o.client_order_id.toString(),
      sequence: o.sequence.toString(),
      expiry_timestamp: o.expiry_timestamp.toString(),
      status: 'open',
    };
  }

  private emitDivergence(
    reason: string,
    key: string,
    details: Record<string, string>,
  ): void {
    const event: DivergenceEvent = {
      event_type: 'divergence_event',
      ts_ms: Date.now(),
      reason,
      key,
      details,
    };
    this.divergences.push(event);
    this.emitEvent(event);
  }

  private emitEvent(event: HarnessEvent): void {
    for (const listener of this.listeners) {
      listener(event);
    }
  }
}

function maxBigint(a: bigint, b: bigint): bigint {
  return a > b ? a : b;
}

function minBigint(a: bigint, b: bigint): bigint {
  return a < b ? a : b;
}

function maxBigintFrom(values: bigint[]): bigint {
  let out = 0n;
  for (const value of values) {
    if (value > out) {
      out = value;
    }
  }
  return out;
}

function orderIntentsDeterministically(a: CanonicalIntent, b: CanonicalIntent): number {
  if (a.group !== b.group) {
    return a.group.localeCompare(b.group);
  }
  if (a.market !== b.market) {
    return a.market.localeCompare(b.market);
  }
  if (a.kind !== b.kind) {
    return a.kind - b.kind;
  }
  if (a.kind === QueueItemKindHarness.CtmWrapped && a.sequence !== b.sequence) {
    return a.sequence < b.sequence ? -1 : 1;
  }
  if (a.accepted_ts_ms !== b.accepted_ts_ms) {
    return a.accepted_ts_ms - b.accepted_ts_ms;
  }
  return a.key.localeCompare(b.key);
}
