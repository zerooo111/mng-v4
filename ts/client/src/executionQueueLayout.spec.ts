import { expect } from 'chai';
import {
  EXECUTION_QUEUE_LAYOUT,
  decodeExecutionQueueHeadItem,
  decodeExecutionQueueHeader,
} from './executionQueueLayout';

function writeU32(data: Buffer, offset: number, value: number): void {
  data.writeUInt32LE(value, offset);
}

function writeU64(data: Buffer, offset: number, value: bigint): void {
  data.writeBigUInt64LE(value, offset);
}

function ctmItemOffset(sequence: bigint): number {
  return (
    EXECUTION_QUEUE_LAYOUT.ctmItemsOffset +
    (Number(sequence % BigInt(EXECUTION_QUEUE_LAYOUT.ctmCapacity)) *
      EXECUTION_QUEUE_LAYOUT.itemSize)
  );
}

function liquidityItemOffset(head: number): number {
  return (
    EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset +
    ((head % EXECUTION_QUEUE_LAYOUT.liquidityCapacity) *
      EXECUTION_QUEUE_LAYOUT.itemSize)
  );
}

describe('executionQueueLayout', () => {
  it('decodes header invariants and a CTM head item', () => {
    const data = Buffer.alloc(
      EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset +
        EXECUTION_QUEUE_LAYOUT.itemSize,
    );
    writeU32(data, EXECUTION_QUEUE_LAYOUT.totalCountOffset, 2);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.ctmCountOffset, 2);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.liquidityCountOffset, 0);
    writeU64(data, EXECUTION_QUEUE_LAYOUT.nextSequenceOffset, 10n);
    writeU64(data, EXECUTION_QUEUE_LAYOUT.maxSeenSequenceOffset, 11n);

    const itemOffset = ctmItemOffset(10n);
    writeU64(data, itemOffset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset, 10n);
    writeU64(data, itemOffset + EXECUTION_QUEUE_LAYOUT.itemMinExecuteSlotOffset, 25n);
    data.writeUInt8(
      EXECUTION_QUEUE_LAYOUT.ctmWrappedKind,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemKindOffset,
    );
    data.writeUInt8(
      EXECUTION_QUEUE_LAYOUT.pendingStatus,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset,
    );
    data.writeUInt16LE(
      3,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemPayloadLenOffset,
    );
    Buffer.from([1, 2, 3]).copy(
      data,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemPayloadOffset,
    );
    Buffer.alloc(32, 7).copy(
      data,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemAccountsHashOffset,
    );

    const header = decodeExecutionQueueHeader(data);
    expect(header.totalCount).eq(2);
    expect(header.ctmCount).eq(2);
    expect(header.headerInvariantOk).eq(true);
    expect(header.gapSpan).eq(1n);

    const head = decodeExecutionQueueHeadItem(data);
    expect(head).to.not.eq(null);
    expect(head?.section).eq('ctm');
    expect(head?.sequence).eq(10n);
    expect(head?.minExecuteSlot).eq(25n);
    expect(head?.accountsHash.equals(Buffer.alloc(32, 7))).eq(true);
    expect(Array.from(head?.payload || [])).deep.eq([1, 2, 3]);
  });

  it('falls back to the liquidity head when no CTM head is pending', () => {
    const data = Buffer.alloc(
      EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset +
        EXECUTION_QUEUE_LAYOUT.itemSize,
    );
    writeU32(data, EXECUTION_QUEUE_LAYOUT.totalCountOffset, 1);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.ctmCountOffset, 0);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.liquidityCountOffset, 1);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.liquidityHeadOffset, 0);

    const itemOffset = liquidityItemOffset(0);
    writeU64(data, itemOffset + EXECUTION_QUEUE_LAYOUT.itemSequenceOffset, 77n);
    data.writeUInt8(1, itemOffset + EXECUTION_QUEUE_LAYOUT.itemKindOffset);
    data.writeUInt8(
      EXECUTION_QUEUE_LAYOUT.pendingStatus,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemStatusOffset,
    );
    Buffer.alloc(32, 9).copy(
      data,
      itemOffset + EXECUTION_QUEUE_LAYOUT.itemAccountsHashOffset,
    );

    const head = decodeExecutionQueueHeadItem(data);
    expect(head).to.not.eq(null);
    expect(head?.section).eq('liquidity');
    expect(head?.sequence).eq(77n);
    expect(head?.accountsHash.equals(Buffer.alloc(32, 9))).eq(true);
  });

  it('reports inconsistent headers and refuses malformed heads', () => {
    const data = Buffer.alloc(EXECUTION_QUEUE_LAYOUT.liquidityItemsOffset);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.totalCountOffset, 1);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.ctmCountOffset, 2);
    writeU32(data, EXECUTION_QUEUE_LAYOUT.liquidityCountOffset, 0);
    writeU64(data, EXECUTION_QUEUE_LAYOUT.nextSequenceOffset, 5n);
    writeU64(data, EXECUTION_QUEUE_LAYOUT.maxSeenSequenceOffset, 5n);

    const header = decodeExecutionQueueHeader(data);
    expect(header.headerInvariantOk).eq(false);
    expect(decodeExecutionQueueHeadItem(data)).eq(null);
  });
});
