export const EXECUTION_QUEUE_ACCOUNT_SPACE = 368280;
export const EXECUTION_QUEUE_LAYOUT = {
  headerOffset: 152,
  headOffset: 152,
  countOffset: 156,
  nextSequenceOffset: 160,
  maxSeenSequenceOffset: 168,
  itemsOffset: 280,
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

export type DecodedExecutionQueueHeader = {
  capacity: number;
  head: number;
  count: number;
  nextSequence: bigint;
  maxSeenSequence: bigint;
};

export type DecodedExecutionQueueHeadItem = DecodedExecutionQueueHeader & {
  logicalIndex: number;
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

function queueDataLength(dataOrLength: Buffer | number): number {
  return typeof dataOrLength === 'number' ? dataOrLength : dataOrLength.length;
}

export function decodeExecutionQueueCapacity(dataOrLength: Buffer | number): number {
  const dataLength = queueDataLength(dataOrLength);
  if (dataLength <= EXECUTION_QUEUE_LAYOUT.itemsOffset) {
    return 0;
  }
  return Math.floor(
    (dataLength - EXECUTION_QUEUE_LAYOUT.itemsOffset) / EXECUTION_QUEUE_LAYOUT.itemSize,
  );
}

export function decodeExecutionQueueHead(data: Buffer): number {
  if (data.length < EXECUTION_QUEUE_LAYOUT.headOffset + 4) {
    return 0;
  }
  return data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.headOffset);
}

export function decodeExecutionQueueCount(data: Buffer): number {
  if (data.length < EXECUTION_QUEUE_LAYOUT.countOffset + 4) {
    return 0;
  }
  return data.readUInt32LE(EXECUTION_QUEUE_LAYOUT.countOffset);
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
  return {
    capacity: decodeExecutionQueueCapacity(data),
    head: decodeExecutionQueueHead(data),
    count: decodeExecutionQueueCount(data),
    nextSequence: decodeExecutionQueueNextSequence(data),
    maxSeenSequence: decodeExecutionQueueMaxSeenSequence(data),
  };
}

export function executionQueueItemOffset(
  dataOrLength: Buffer | number,
  head: number,
  logicalIndex: number,
): number | null {
  const capacity = decodeExecutionQueueCapacity(dataOrLength);
  if (capacity === 0) {
    return null;
  }
  const physicalIndex = (head + logicalIndex) % capacity;
  return (
    EXECUTION_QUEUE_LAYOUT.itemsOffset +
    physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize
  );
}

export function decodeExecutionQueueHeadItem(
  data: Buffer,
): DecodedExecutionQueueHeadItem | null {
  const header = decodeExecutionQueueHeader(data);
  if (header.count === 0 || header.capacity === 0) {
    return null;
  }

  for (let logicalIndex = 0; logicalIndex < header.count; logicalIndex += 1) {
    const physicalIndex = (header.head + logicalIndex) % header.capacity;
    const offset =
      EXECUTION_QUEUE_LAYOUT.itemsOffset +
      physicalIndex * EXECUTION_QUEUE_LAYOUT.itemSize;
    if (offset + EXECUTION_QUEUE_LAYOUT.itemSize > data.length) {
      break;
    }

    const sequence = data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset,
    );
    const kind = data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemKindOffset);
    const status = data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset);
    if (
      status !== EXECUTION_QUEUE_LAYOUT.pendingStatus ||
      kind !== EXECUTION_QUEUE_LAYOUT.ctmWrappedKind ||
      sequence !== header.nextSequence
    ) {
      continue;
    }

    const minExecuteSlot = data.readBigUInt64LE(
      offset + EXECUTION_QUEUE_LAYOUT.itemMinExecuteSlotOffset,
    );
    const retries = data.readUInt8(offset + EXECUTION_QUEUE_LAYOUT.itemRetriesOffset);
    const payloadLen = data.readUInt16LE(
      offset + EXECUTION_QUEUE_LAYOUT.itemPayloadLenOffset,
    );
    const payloadHash = Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadHashOffset,
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadHashOffset + 32,
      ),
    );
    const accountsHash = Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT.itemAccountsHashOffset,
        offset + EXECUTION_QUEUE_LAYOUT.itemAccountsHashOffset + 32,
      ),
    );
    const payload = Buffer.from(
      data.subarray(
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadOffset,
        offset + EXECUTION_QUEUE_LAYOUT.itemPayloadOffset + payloadLen,
      ),
    );

    return {
      ...header,
      logicalIndex,
      physicalIndex,
      sequence,
      kind,
      status,
      minExecuteSlot,
      retries,
      payloadLen,
      payloadHash,
      accountsHash,
      payload,
    };
  }

  return null;
}
