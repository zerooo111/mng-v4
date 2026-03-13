export const EXECUTION_QUEUE_LAYOUT = {
  totalCountOffset: 152,
  liquidityHeadOffset: 156,
  nextSequenceOffset: 160,
  maxSeenSequenceOffset: 168,
  ctmCountOffset: 192,
  liquidityCountOffset: 196,
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

export const EXECUTION_QUEUE_ACCOUNT_SPACE = 424216;

export type DecodedExecutionQueueHeader = {
  capacity: number;
  totalCount: number;
  count: number;
  liquidityHead: number;
  ctmCount: number;
  liquidityCount: number;
  nextSequence: bigint;
  maxSeenSequence: bigint;
};

export type DecodedExecutionQueueHeadItem = DecodedExecutionQueueHeader & {
  section: 'ctm' | 'liquidity';
  physicalIndex: number;
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

function decodeItem(
  data: Buffer,
  offset: number,
  section: 'ctm' | 'liquidity',
  header: DecodedExecutionQueueHeader,
): DecodedExecutionQueueHeadItem | null {
  if (offset + EXECUTION_QUEUE_LAYOUT.itemSize > data.length) {
    return null;
  }
  const payloadLen = data.readUInt16LE(
    offset + EXECUTION_QUEUE_LAYOUT.itemPayloadLenOffset,
  );
  return {
    ...header,
    section,
    physicalIndex:
      section === 'ctm'
        ? Math.floor(
            (offset - EXECUTION_QUEUE_LAYOUT.ctmItemsOffset) /
              EXECUTION_QUEUE_LAYOUT.itemSize,
          )
        : Math.floor(
            (offset - EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset) /
              EXECUTION_QUEUE_LAYOUT.itemSize,
          ),
    sequence: data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset,
    ),
    minExecuteSlot: data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT.itemMinExecuteSlotOffset,
    ),
    kind: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemKindOffset),
    status: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset),
    retries: data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemRetriesOffset),
    payloadLen,
    payloadHash: Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadHashOffset,
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadHashOffset + 32,
      ),
    ),
    accountsHash: Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT.itemAccountsHashOffset,
        offset + EXECUTION_QUEUE_LAYOUT.itemAccountsHashOffset + 32,
      ),
    ),
    payload: Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadOffset,
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadOffset + payloadLen,
      ),
    ),
  };
}

export function decodeExecutionQueueCapacity(_dataOrLength: Buffer | number): number {
  return (
    EXECUTION_QUEUE_LAYOUT.ctmCapacity + EXECUTION_QUEUE_LAYOUT.liquidityCapacity
  );
}

export function decodeExecutionQueueHead(data: Buffer): number {
  if (data.length < EXECUTION_QUEUE_LAYOUT.liquidityHeadOffset + 4) {
    return 0;
  }
  return data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.liquidityHeadOffset);
}

export function decodeExecutionQueueCount(data: Buffer): number {
  if (data.length < EXECUTION_QUEUE_LAYOUT.totalCountOffset + 4) {
    return 0;
  }
  return data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.totalCountOffset);
}

export function decodeExecutionQueueNextSequence(data: Buffer): bigint {
  if (data.length < EXECUTION_QUEUE_LAYOUT.nextSequenceOffset + 8) {
    return 0n;
  }
  return data.readBigUInt64LE(EXECUTION_QUEUE_LAYOUT.nextSequenceOffset);
}

export function decodeExecutionQueueMaxSeenSequence(data: Buffer): bigint {
  if (data.length < EXECUTION_QUEUE_LAYOUT.maxSeenSequenceOffset + 8) {
    return 0n;
  }
  return data.readBigUInt64LE(EXECUTION_QUEUE_LAYOUT.maxSeenSequenceOffset);
}

export function decodeExecutionQueueHeader(data: Buffer): DecodedExecutionQueueHeader {
  const totalCount = decodeExecutionQueueCount(data);
  const liquidityHead = decodeExecutionQueueHead(data);
  const ctmCount =
    data.length >= EXECUTION_QUEUE_LAYOUT.ctmCountOffset + 4
      ? data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.ctmCountOffset)
      : 0;
  const liquidityCount =
    data.length >= EXECUTION_QUEUE_LAYOUT.liquidityCountOffset + 4
      ? data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.liquidityCountOffset)
      : 0;
  return {
    capacity: decodeExecutionQueueCapacity(data),
    totalCount,
    count: totalCount,
    liquidityHead,
    ctmCount,
    liquidityCount,
    nextSequence: decodeExecutionQueueNextSequence(data),
    maxSeenSequence: decodeExecutionQueueMaxSeenSequence(data),
  };
}

export function executionQueueItemOffset(
  _dataOrLength: Buffer | number,
  head: number,
  logicalIndex: number,
): number | null {
  if (logicalIndex < 0 || logicalIndex >= EXECUTION_QUEUE_LAYOUT.liquidityCapacity) {
    return null;
  }
  const physicalIndex = (head + logicalIndex) % EXECUTION_QUEUE_LAYOUT.liquidityCapacity;
  return (
    EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset +
    physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize
  );
}

export function decodeExecutionQueueHeadItem(
  data: Buffer,
): DecodedExecutionQueueHeadItem | null {
  const header = decodeExecutionQueueHeader(data);
  if (header.totalCount === 0) {
    return null;
  }

  if (header.ctmCount > 0) {
    const ctmIndex =
      Number(header.nextSequence % BigInt(EXECUTION_QUEUE_LAYOUT.ctmCapacity));
    const ctmOffset =
      EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
      ctmIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
    const ctmItem = decodeItem(data, ctmOffset, 'ctm', header);
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
  const liqItem = decodeItem(data, liqOffset, 'liquidity', header);
  if (
    !liqItem ||
    liqItem.status !== EXECUTION_QUEUE_LAYOUT.pendingStatus
  ) {
    return null;
  }
  return liqItem;
}
