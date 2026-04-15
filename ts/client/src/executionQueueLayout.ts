// Execution queue layout codec.
//
// Two on-chain layouts are supported and auto-detected from the
// `layout_version` byte at raw offset 147 (with 8-byte discriminator).
//
//   layout_version == 0  →  v1 (legacy single-queue)
//   layout_version == 2  →  v2 (16-way per-market sub-queues + global liquidity ring)
//
// All `decode*` entrypoints below dispatch on the version. Most callers can
// keep using `decodeExecutionQueueHeader` / `decodeExecutionQueuePendingItems`
// without caring which layout the queue is on. v2-aware consumers can use
// `decodeExecutionQueueSubQueues` for per-market state.

// ── v1 layout (kept verbatim for the legacy unit tests + any straggler scripts) ──
export const EXECUTION_QUEUE_LAYOUT = {
  totalCountOffset: 152,
  liquidityHeadOffset: 156,
  nextSequenceOffset: 160,
  maxSeenSequenceOffset: 168,
  ctmCountOffset: 200,
  liquidityCountOffset: 204,
  ctmItemsOffset: 280,
  liquidityItemsOffset: 377112,
  ctmCapacity: 1024,
  liquidityCapacity: 128,
  itemSize: 368,
  itemSequenceOffset: 0,
  itemMinExecuteSlotOffset: 8,
  itemKindOffset: 32,
  itemStatusOffset: 33,
  itemRetriesOffset: 34,
  itemPayloadLenOffset: 40,
  itemPayloadHashOffset: 48,
  itemAccountsHashOffset: 80,
  itemPayloadOffset: 112,
  pendingStatus: 1,
  ctmWrappedKind: 0,
} as const;

// ── v2 layout (sub-queue) ──
//
// Mirrors the canonical Rust offsets in
// `programs/mango-v4/src/state/execution_queue.rs`. All offsets include the
// 8-byte Anchor discriminator.
export const EXECUTION_QUEUE_LAYOUT_V2 = {
  layoutVersionOffset: 147,
  pausedIngressOffset: 145,
  pausedExecuteOffset: 146,
  globalHeaderOffset: 152,
  // Within global header (relative to globalHeaderOffset):
  ghGapWaitSlotsOffset: 0,
  ghLiquidityDelaySlotsOffset: 8,
  ghLiquidityHeadOffset: 16,
  ghLiquidityCountOffset: 20,
  ghTotalCountOffset: 24,
  // Sub-queue header table:
  subQueueHeadersOffset: 264,
  subQueueHeaderStride: 64,
  nMaxMarkets: 16,
  perMarketCtmCapacity: 64,
  // Within a sub-queue header:
  shMarketIndexOffset: 0,
  shActiveOffset: 2,
  shPausedIngressOffset: 3,
  shPausedExecuteOffset: 4,
  shCtmCountOffset: 8,
  shNextSequenceOffset: 16,
  shMaxSeenSequenceOffset: 24,
  shGapObservedSlotOffset: 32,
  shFirstFailureSlotOffset: 40,
  // Item arrays:
  ctmItemsOffset: 1288,
  ctmCapacity: 1024,
  liquidityItemsOffset: 378120,
  liquidityCapacity: 128,
  // Item internal offsets (unchanged from v1):
  itemSize: 368,
  itemSequenceOffset: 0,
  itemMinExecuteSlotOffset: 8,
  itemKindOffset: 32,
  itemStatusOffset: 33,
  itemRetriesOffset: 34,
  itemPayloadLenOffset: 40,
  itemPayloadHashOffset: 48,
  itemAccountsHashOffset: 80,
  itemPayloadOffset: 112,
  pendingStatus: 1,
  ctmWrappedKind: 0,
} as const;

export const EXECUTION_QUEUE_ACCOUNT_SPACE_V1 = 424216;
export const EXECUTION_QUEUE_ACCOUNT_SPACE_V2 = 425224;
// Default to v2 — bootstrap resizers grow newly created queues to this size.
export const EXECUTION_QUEUE_ACCOUNT_SPACE = EXECUTION_QUEUE_ACCOUNT_SPACE_V2;

export type ExecutionQueueLayoutVersion = 1 | 2;

export function detectExecutionQueueLayoutVersion(
  data: Buffer,
): ExecutionQueueLayoutVersion {
  if (data.length <= EXECUTION_QUEUE_LAYOUT_V2.layoutVersionOffset) {
    return 1;
  }
  return data[EXECUTION_QUEUE_LAYOUT_V2.layoutVersionOffset] === 2 ? 2 : 1;
}

export type DecodedExecutionQueueHeader = {
  capacity: number;
  totalCount: number;
  count: number;
  liquidityHead: number;
  ctmCount: number;
  liquidityCount: number;
  headerInvariantOk: boolean;
  gapSpan: bigint;
  nextSequence: bigint;
  maxSeenSequence: bigint;
  // v2-only: aggregate fields exposed for consumers that want a per-market
  // breakdown without re-parsing. Empty on v1.
  layoutVersion: ExecutionQueueLayoutVersion;
  subQueues: DecodedSubQueueHeader[];
};

export type DecodedExecutionQueueHeadItem = DecodedExecutionQueueHeader & {
  section: 'ctm' | 'liquidity';
  physicalIndex: number;
  // v2 sub-queue: market_index of the sub-queue this item belongs to. null on
  // v1 (single-stream) and on liquidity items (which are token-keyed, not
  // market-keyed).
  marketIndex: number | null;
  sequence: bigint;
  kind: number;
  status: number;
  minExecuteSlot: bigint;
  retries: number;
  payloadLen: number;
  payloadHash: Buffer;
  accountsHash: Buffer;
  payload: Buffer;
};

export type DecodedExecutionQueuePendingItem = DecodedExecutionQueueHeadItem;

export type DecodedSubQueueHeader = {
  slotIndex: number;
  marketIndex: number;
  active: boolean;
  pausedIngress: boolean;
  pausedExecute: boolean;
  ctmCount: number;
  nextSequenceToExecute: bigint;
  maxSeenSequence: bigint;
  gapObservedSlot: bigint;
};

// ── Item decoder (shared between v1 and v2 — item layout is identical) ──

function decodeItem(
  data: Buffer,
  offset: number,
  section: 'ctm' | 'liquidity',
  itemsBase: number,
  header: DecodedExecutionQueueHeader,
  marketIndex: number | null = null,
): DecodedExecutionQueueHeadItem | null {
  const itemSize = EXECUTION_QUEUE_LAYOUT_V2.itemSize;
  if (offset + itemSize > data.length) {
    return null;
  }
  const payloadLen = data.readUInt16LE(
    offset + EXECUTION_QUEUE_LAYOUT_V2.itemPayloadLenOffset,
  );
  return {
    ...header,
    section,
    physicalIndex: Math.floor((offset - itemsBase) / itemSize),
    marketIndex,
    sequence: data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT_V2.itemSequenceOffset,
    ),
    minExecuteSlot: data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT_V2.itemMinExecuteSlotOffset,
    ),
    kind: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT_V2.itemKindOffset),
    status: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT_V2.itemStatusOffset),
    retries: data.readUInt8(
      offset + EXECUTION_QUEUE_LAYOUT_V2.itemRetriesOffset,
    ),
    payloadLen,
    payloadHash: Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT_V2.itemPayloadHashOffset,
        offset + EXECUTION_QUEUE_LAYOUT_V2.itemPayloadHashOffset + 32,
      ),
    ),
    accountsHash: Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT_V2.itemAccountsHashOffset,
        offset + EXECUTION_QUEUE_LAYOUT_V2.itemAccountsHashOffset + 32,
      ),
    ),
    payload: Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT_V2.itemPayloadOffset,
        offset + EXECUTION_QUEUE_LAYOUT_V2.itemPayloadOffset + payloadLen,
      ),
    ),
  };
}

export function decodeExecutionQueueCapacity(
  _dataOrLength: Buffer | number,
): number {
  return (
    EXECUTION_QUEUE_LAYOUT_V2.ctmCapacity +
    EXECUTION_QUEUE_LAYOUT_V2.liquidityCapacity
  );
}

// ── v1 raw field readers (legacy path) ──

function v1Head(data: Buffer): number {
  if (data.length < EXECUTION_QUEUE_LAYOUT.liquidityHeadOffset + 4) return 0;
  return data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.liquidityHeadOffset);
}

function v1Count(data: Buffer): number {
  if (data.length < EXECUTION_QUEUE_LAYOUT.totalCountOffset + 4) return 0;
  return data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.totalCountOffset);
}

function v1NextSequence(data: Buffer): bigint {
  if (data.length < EXECUTION_QUEUE_LAYOUT.nextSequenceOffset + 8) return 0n;
  return data.readBigUInt64LE(EXECUTION_QUEUE_LAYOUT.nextSequenceOffset);
}

function v1MaxSeen(data: Buffer): bigint {
  if (data.length < EXECUTION_QUEUE_LAYOUT.maxSeenSequenceOffset + 8) return 0n;
  return data.readBigUInt64LE(EXECUTION_QUEUE_LAYOUT.maxSeenSequenceOffset);
}

function decodeV1Header(data: Buffer): DecodedExecutionQueueHeader {
  const totalCount = v1Count(data);
  const liquidityHead = v1Head(data);
  const ctmCount =
    data.length >= EXECUTION_QUEUE_LAYOUT.ctmCountOffset + 4
      ? data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.ctmCountOffset)
      : 0;
  const liquidityCount =
    data.length >= EXECUTION_QUEUE_LAYOUT.liquidityCountOffset + 4
      ? data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.liquidityCountOffset)
      : 0;
  const nextSequence = v1NextSequence(data);
  const maxSeen = v1MaxSeen(data);
  return {
    capacity: decodeExecutionQueueCapacity(data),
    totalCount,
    count: totalCount,
    liquidityHead,
    ctmCount,
    liquidityCount,
    headerInvariantOk: totalCount === ctmCount + liquidityCount,
    gapSpan: maxSeen >= nextSequence ? maxSeen - nextSequence : 0n,
    nextSequence,
    maxSeenSequence: maxSeen,
    layoutVersion: 1,
    subQueues: [],
  };
}

// ── v2 readers ──

export function decodeExecutionQueueSubQueues(
  data: Buffer,
): DecodedSubQueueHeader[] {
  const out: DecodedSubQueueHeader[] = [];
  const base = EXECUTION_QUEUE_LAYOUT_V2.subQueueHeadersOffset;
  const stride = EXECUTION_QUEUE_LAYOUT_V2.subQueueHeaderStride;
  for (let slot = 0; slot < EXECUTION_QUEUE_LAYOUT_V2.nMaxMarkets; slot++) {
    const off = base + slot * stride;
    if (off + stride > data.length) break;
    const active = data[off + EXECUTION_QUEUE_LAYOUT_V2.shActiveOffset] === 1;
    out.push({
      slotIndex: slot,
      marketIndex: data.readUInt16LE(
        off + EXECUTION_QUEUE_LAYOUT_V2.shMarketIndexOffset,
      ),
      active,
      pausedIngress:
        data[off + EXECUTION_QUEUE_LAYOUT_V2.shPausedIngressOffset] === 1,
      pausedExecute:
        data[off + EXECUTION_QUEUE_LAYOUT_V2.shPausedExecuteOffset] === 1,
      ctmCount: data.readUInt32LE(
        off + EXECUTION_QUEUE_LAYOUT_V2.shCtmCountOffset,
      ),
      nextSequenceToExecute: data.readBigUInt64LE(
        off + EXECUTION_QUEUE_LAYOUT_V2.shNextSequenceOffset,
      ),
      maxSeenSequence: data.readBigUInt64LE(
        off + EXECUTION_QUEUE_LAYOUT_V2.shMaxSeenSequenceOffset,
      ),
      gapObservedSlot: data.readBigUInt64LE(
        off + EXECUTION_QUEUE_LAYOUT_V2.shGapObservedSlotOffset,
      ),
    });
  }
  return out;
}

function decodeV2Header(data: Buffer): DecodedExecutionQueueHeader {
  const globalBase = EXECUTION_QUEUE_LAYOUT_V2.globalHeaderOffset;
  const liquidityHead =
    data.length >= globalBase + EXECUTION_QUEUE_LAYOUT_V2.ghLiquidityHeadOffset + 4
      ? data.readUInt32LE(
          globalBase + EXECUTION_QUEUE_LAYOUT_V2.ghLiquidityHeadOffset,
        )
      : 0;
  const liquidityCount =
    data.length >= globalBase + EXECUTION_QUEUE_LAYOUT_V2.ghLiquidityCountOffset + 4
      ? data.readUInt32LE(
          globalBase + EXECUTION_QUEUE_LAYOUT_V2.ghLiquidityCountOffset,
        )
      : 0;
  const totalCount =
    data.length >= globalBase + EXECUTION_QUEUE_LAYOUT_V2.ghTotalCountOffset + 4
      ? data.readUInt32LE(
          globalBase + EXECUTION_QUEUE_LAYOUT_V2.ghTotalCountOffset,
        )
      : 0;

  const subQueues = decodeExecutionQueueSubQueues(data);
  const actives = subQueues.filter((s) => s.active);
  const ctmCount = actives.reduce((acc, s) => acc + s.ctmCount, 0);

  // For backwards compat with single-stream callers, expose the *oldest*
  // pending head across markets as `nextSequence` and the highest seen as
  // `maxSeenSequence`. This gives latency probers a meaningful single number
  // for "is the queue draining". Per-market detail is in `subQueues`.
  let oldestNext: bigint | null = null;
  let highestSeen: bigint = 0n;
  let aggregateGap: bigint = 0n;
  for (const sq of actives) {
    if (sq.ctmCount > 0) {
      if (oldestNext === null || sq.nextSequenceToExecute < oldestNext) {
        oldestNext = sq.nextSequenceToExecute;
      }
    }
    if (sq.maxSeenSequence > highestSeen) {
      highestSeen = sq.maxSeenSequence;
    }
    if (sq.maxSeenSequence >= sq.nextSequenceToExecute) {
      const span = sq.maxSeenSequence - sq.nextSequenceToExecute;
      if (span > aggregateGap) aggregateGap = span;
    }
  }
  // If no market has pending CTM items, nextSequence == max seen (or 0).
  const nextSequence =
    oldestNext !== null ? oldestNext : highestSeen;

  return {
    capacity: decodeExecutionQueueCapacity(data),
    totalCount,
    count: totalCount,
    liquidityHead,
    ctmCount,
    liquidityCount,
    headerInvariantOk: totalCount === ctmCount + liquidityCount,
    gapSpan: aggregateGap,
    nextSequence,
    maxSeenSequence: highestSeen,
    layoutVersion: 2,
    subQueues,
  };
}

// ── Public dispatchers ──

export function decodeExecutionQueueHead(data: Buffer): number {
  if (detectExecutionQueueLayoutVersion(data) === 2) {
    return data.readUInt32LE(
      EXECUTION_QUEUE_LAYOUT_V2.globalHeaderOffset +
        EXECUTION_QUEUE_LAYOUT_V2.ghLiquidityHeadOffset,
    );
  }
  return v1Head(data);
}

export function decodeExecutionQueueCount(data: Buffer): number {
  if (detectExecutionQueueLayoutVersion(data) === 2) {
    return data.readUInt32LE(
      EXECUTION_QUEUE_LAYOUT_V2.globalHeaderOffset +
        EXECUTION_QUEUE_LAYOUT_V2.ghTotalCountOffset,
    );
  }
  return v1Count(data);
}

export function decodeExecutionQueueNextSequence(data: Buffer): bigint {
  if (detectExecutionQueueLayoutVersion(data) === 2) {
    return decodeV2Header(data).nextSequence;
  }
  return v1NextSequence(data);
}

export function decodeExecutionQueueMaxSeenSequence(data: Buffer): bigint {
  if (detectExecutionQueueLayoutVersion(data) === 2) {
    return decodeV2Header(data).maxSeenSequence;
  }
  return v1MaxSeen(data);
}

export function decodeExecutionQueueHeader(
  data: Buffer,
): DecodedExecutionQueueHeader {
  return detectExecutionQueueLayoutVersion(data) === 2
    ? decodeV2Header(data)
    : decodeV1Header(data);
}

// ── v1 head item / pending items (legacy path) ──

function v1ExecutionQueueItemOffset(
  head: number,
  logicalIndex: number,
): number | null {
  if (
    logicalIndex < 0 ||
    logicalIndex >= EXECUTION_QUEUE_LAYOUT.liquidityCapacity
  ) {
    return null;
  }
  const physicalIndex =
    (head + logicalIndex) % EXECUTION_QUEUE_LAYOUT.liquidityCapacity;
  return (
    EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset +
    physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize
  );
}

export function executionQueueItemOffset(
  _dataOrLength: Buffer | number,
  head: number,
  logicalIndex: number,
): number | null {
  return v1ExecutionQueueItemOffset(head, logicalIndex);
}

function decodeV1HeadItem(
  data: Buffer,
): DecodedExecutionQueueHeadItem | null {
  const header = decodeV1Header(data);
  if (header.totalCount === 0) {
    return null;
  }

  if (header.ctmCount > 0) {
    const ctmIndex =
      Number(header.nextSequence % BigInt(EXECUTION_QUEUE_LAYOUT.ctmCapacity));
    const ctmOffset =
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
      ctmIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
    const ctmItem = decodeItem(
      data,
      ctmOffset,
      'ctm',
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset,
      header,
    );
    if (
      ctmItem &&
      ctmItem.status === EXECUTION_QUEUE_LAYOUT.pendingStatus &&
      ctmItem.kind === EXECUTION_QUEUE_LAYOUT.ctmWrappedKind &&
      ctmItem.sequence === header.nextSequence
    ) {
      return ctmItem;
    }
  }

  if (header.liquidityCount === 0) {
    return null;
  }
  const liqOffset =
    EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset +
    header.liquidityHead * EXECUTION_QUEUE_LAYOUT.itemSize;
  const liqItem = decodeItem(
    data,
    liqOffset,
    'liquidity',
    EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset,
    header,
  );
  if (!liqItem || liqItem.status !== EXECUTION_QUEUE_LAYOUT.pendingStatus) {
    return null;
  }
  return liqItem;
}

function decodeV1PendingItems(
  data: Buffer,
): DecodedExecutionQueuePendingItem[] {
  const header = decodeV1Header(data);
  const items: DecodedExecutionQueuePendingItem[] = [];

  for (
    let physicalIndex = 0;
    physicalIndex < EXECUTION_QUEUE_LAYOUT.ctmCapacity;
    physicalIndex += 1
  ) {
    const offset =
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
      physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
    const item = decodeItem(
      data,
      offset,
      'ctm',
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset,
      header,
    );
    if (
      !item ||
      item.status !== EXECUTION_QUEUE_LAYOUT.pendingStatus ||
      item.kind !== EXECUTION_QUEUE_LAYOUT.ctmWrappedKind ||
      item.sequence < header.nextSequence ||
      item.sequence > header.maxSeenSequence
    ) {
      continue;
    }
    items.push(item);
  }

  for (
    let logicalIndex = 0;
    logicalIndex < header.liquidityCount;
    logicalIndex += 1
  ) {
    const offset = v1ExecutionQueueItemOffset(header.liquidityHead, logicalIndex);
    if (offset === null) continue;
    const item = decodeItem(
      data,
      offset,
      'liquidity',
      EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset,
      header,
    );
    if (!item || item.status !== EXECUTION_QUEUE_LAYOUT.pendingStatus) continue;
    items.push(item);
  }

  items.sort((a, b) => {
    if (a.sequence === b.sequence) {
      if (a.section === b.section) {
        return a.physicalIndex - b.physicalIndex;
      }
      return a.section === 'ctm' ? -1 : 1;
    }
    return a.sequence < b.sequence ? -1 : 1;
  });
  return items;
}

// ── v2 head item / pending items ──

function v2CtmSlotOffset(slotIndex: number, sequence: bigint): number {
  // Per-market window of `perMarketCtmCapacity` items inside the flat
  // ctm_items array. Slot `s` for sub-queue index `m` lives at
  // `m * perMarketCtmCapacity + s`. Mod by capacity to get the ring index.
  const perMarket = EXECUTION_QUEUE_LAYOUT_V2.perMarketCtmCapacity;
  const ringIdx = Number(sequence % BigInt(perMarket));
  const flatIdx = slotIndex * perMarket + ringIdx;
  return (
    EXECUTION_QUEUE_LAYOUT_V2.ctmItemsOffset +
    flatIdx * EXECUTION_QUEUE_LAYOUT_V2.itemSize
  );
}

function v2SubQueueWindowBase(slotIndex: number): number {
  return (
    EXECUTION_QUEUE_LAYOUT_V2.ctmItemsOffset +
    slotIndex *
      EXECUTION_QUEUE_LAYOUT_V2.perMarketCtmCapacity *
      EXECUTION_QUEUE_LAYOUT_V2.itemSize
  );
}

function decodeV2HeadItem(
  data: Buffer,
): DecodedExecutionQueueHeadItem | null {
  const header = decodeV2Header(data);
  if (header.totalCount === 0) {
    return null;
  }

  // Walk active sub-queues in slot order; return the first that has a
  // valid pending head. (Multi-market dispatch happens at execute time —
  // this is just a single representative head for diagnostic callers.)
  for (const sq of header.subQueues) {
    if (!sq.active || sq.ctmCount === 0) continue;
    const offset = v2CtmSlotOffset(sq.slotIndex, sq.nextSequenceToExecute);
    const item = decodeItem(
      data,
      offset,
      'ctm',
      v2SubQueueWindowBase(sq.slotIndex),
      header,
      sq.marketIndex,
    );
    if (
      item &&
      item.status === EXECUTION_QUEUE_LAYOUT_V2.pendingStatus &&
      item.kind === EXECUTION_QUEUE_LAYOUT_V2.ctmWrappedKind &&
      item.sequence === sq.nextSequenceToExecute
    ) {
      return item;
    }
  }

  if (header.liquidityCount === 0) {
    return null;
  }
  const liqOffset =
    EXECUTION_QUEUE_LAYOUT_V2.liquidityItemsOffset +
    header.liquidityHead * EXECUTION_QUEUE_LAYOUT_V2.itemSize;
  const liqItem = decodeItem(
    data,
    liqOffset,
    'liquidity',
    EXECUTION_QUEUE_LAYOUT_V2.liquidityItemsOffset,
    header,
    null,
  );
  if (!liqItem || liqItem.status !== EXECUTION_QUEUE_LAYOUT_V2.pendingStatus) {
    return null;
  }
  return liqItem;
}

function decodeV2PendingItems(
  data: Buffer,
): DecodedExecutionQueuePendingItem[] {
  const header = decodeV2Header(data);
  const items: DecodedExecutionQueuePendingItem[] = [];

  for (const sq of header.subQueues) {
    if (!sq.active || sq.ctmCount === 0) continue;
    const windowBase = v2SubQueueWindowBase(sq.slotIndex);
    const perMarket = EXECUTION_QUEUE_LAYOUT_V2.perMarketCtmCapacity;
    for (let ring = 0; ring < perMarket; ring += 1) {
      const offset = windowBase + ring * EXECUTION_QUEUE_LAYOUT_V2.itemSize;
      const item = decodeItem(
        data,
        offset,
        'ctm',
        windowBase,
        header,
        sq.marketIndex,
      );
      if (
        !item ||
        item.status !== EXECUTION_QUEUE_LAYOUT_V2.pendingStatus ||
        item.kind !== EXECUTION_QUEUE_LAYOUT_V2.ctmWrappedKind ||
        item.sequence < sq.nextSequenceToExecute ||
        item.sequence > sq.maxSeenSequence
      ) {
        continue;
      }
      items.push(item);
    }
  }

  for (let logicalIndex = 0; logicalIndex < header.liquidityCount; logicalIndex += 1) {
    const physicalIndex =
      (header.liquidityHead + logicalIndex) %
      EXECUTION_QUEUE_LAYOUT_V2.liquidityCapacity;
    const offset =
      EXECUTION_QUEUE_LAYOUT_V2.liquidityItemsOffset +
      physicalIndex * EXECUTION_QUEUE_LAYOUT_V2.itemSize;
    const item = decodeItem(
      data,
      offset,
      'liquidity',
      EXECUTION_QUEUE_LAYOUT_V2.liquidityItemsOffset,
      header,
      null,
    );
    if (!item || item.status !== EXECUTION_QUEUE_LAYOUT_V2.pendingStatus) continue;
    items.push(item);
  }

  items.sort((a, b) => {
    if (a.sequence === b.sequence) {
      if (a.section === b.section) {
        return a.physicalIndex - b.physicalIndex;
      }
      return a.section === 'ctm' ? -1 : 1;
    }
    return a.sequence < b.sequence ? -1 : 1;
  });
  return items;
}

export function decodeExecutionQueueHeadItem(
  data: Buffer,
): DecodedExecutionQueueHeadItem | null {
  return detectExecutionQueueLayoutVersion(data) === 2
    ? decodeV2HeadItem(data)
    : decodeV1HeadItem(data);
}

export function decodeExecutionQueuePendingItems(
  data: Buffer,
): DecodedExecutionQueuePendingItem[] {
  return detectExecutionQueueLayoutVersion(data) === 2
    ? decodeV2PendingItems(data)
    : decodeV1PendingItems(data);
}
