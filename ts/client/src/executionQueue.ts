import BN from 'bn.js';
import { createHash } from 'crypto';
import nacl from 'tweetnacl';
import {
  AccountMeta,
  Ed25519Program,
  PublicKey,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  TransactionInstruction,
} from '@solana/web3.js';

export const EXECUTION_QUEUE_DOMAIN = 'mango-v4-ctm-envelope-v1';
export const USER_INTENT_DOMAIN = 'mango-v4-user-intent-v1';
const EXECUTION_QUEUE_DOMAIN_BYTES = Buffer.from(EXECUTION_QUEUE_DOMAIN, 'utf-8');
const USER_INTENT_DOMAIN_BYTES = Buffer.from(USER_INTENT_DOMAIN, 'utf-8');
const instructionDiscriminatorCache = new Map<string, Buffer>();

export enum QueuePayloadVariant {
  PerpPlaceOrderV2 = 0,
  PerpCancelOrder = 1,
  PerpCancelOrderByClientOrderId = 2,
  PerpCancelAllOrders = 3,
  PerpCancelAllOrdersBySide = 4,
  LiquidityDeposit = 5,
  LiquidityWithdraw = 6,
}

export enum QueueItemKind {
  CtmWrapped = 0,
  LiquidityDeposit = 1,
  LiquidityWithdraw = 2,
}

export enum QueueSide {
  Bid = 0,
  Ask = 1,
}

export enum QueuePlaceOrderType {
  Limit = 0,
  ImmediateOrCancel = 1,
  PostOnly = 2,
  Market = 3,
  PostOnlySlide = 4,
}

export enum QueueSelfTradeBehavior {
  DecrementTake = 0,
  CancelProvide = 1,
  AbortTransaction = 2,
}

export type CtmEnvelopeWire = {
  sequence: bigint;
  minExecuteSlot: bigint;
  kind: number;
  payloadHash: Buffer;
  accountsHash: Buffer;
  expiresAtSlot: bigint;
};

export type BigNumberish = bigint | BN | number;

export type IntentSigner =
  | { kind: 'keypair'; privateKey: Uint8Array; publicKey?: Uint8Array }
  | { kind: 'presigned'; publicKey: PublicKey; signature: Uint8Array };

export type QueueSideLike =
  | QueueSide
  | { bid: Record<string, never> }
  | { ask: Record<string, never> };

export type QueuePlaceOrderTypeLike =
  | QueuePlaceOrderType
  | { limit: Record<string, never> }
  | { immediateOrCancel: Record<string, never> }
  | { postOnly: Record<string, never> }
  | { market: Record<string, never> }
  | { postOnlySlide: Record<string, never> };

export type QueueSelfTradeBehaviorLike =
  | QueueSelfTradeBehavior
  | { decrementTake: Record<string, never> }
  | { cancelProvide: Record<string, never> }
  | { abortTransaction: Record<string, never> };

export type PerpPlaceOrderV2QueuePayloadFields = {
  side: QueueSideLike;
  priceLots: BigNumberish;
  maxBaseLots: BigNumberish;
  maxQuoteLots: BigNumberish;
  clientOrderId: BigNumberish;
  orderType: QueuePlaceOrderTypeLike;
  selfTradeBehavior: QueueSelfTradeBehaviorLike;
  reduceOnly: boolean;
  expiryTimestamp: BigNumberish;
  limit: number;
};

export type PerpCancelOrderQueuePayloadFields = {
  orderId: BigNumberish;
};

export type PerpCancelOrderByClientOrderIdQueuePayloadFields = {
  clientOrderId: BigNumberish;
};

export type PerpCancelAllOrdersQueuePayloadFields = {
  limit: number;
};

export type PerpCancelAllOrdersBySideQueuePayloadFields = {
  side: QueueSideLike | null;
  limit: number;
};

export type LiquidityDepositQueuePayloadFields = {
  amount: BigNumberish;
  reduceOnly: boolean;
};

export type LiquidityWithdrawQueuePayloadFields = {
  amount: BigNumberish;
  allowBorrow: boolean;
};

export type BuildExecutionQueueEnqueueCtmParams = {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  executionQueueBuffer?: PublicKey;
  remainingAccounts: AccountMeta[];
  envelope: CtmEnvelopeWire;
  payload: Uint8Array;
};

export type BuildExecutionQueueEnqueueLiquidityParams = {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  executionQueueBuffer?: PublicKey;
  kind: QueueItemKind.LiquidityDeposit | QueueItemKind.LiquidityWithdraw;
  remainingAccounts: AccountMeta[];
  payload: Uint8Array;
};

export type BuildExecutionQueueExecuteParams = {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  executionQueueBuffer?: PublicKey;
  remainingAccounts: AccountMeta[];
  maxItems: number;
};

export type BuildExecutionQueueUserIntentParams = {
  group: PublicKey;
  executionQueue?: PublicKey;
  mangoAccount: PublicKey;
  userOwner: PublicKey;
  kind?: QueueItemKind;
  payload: Uint8Array;
  remainingAccounts: AccountMeta[];
};

export type BuildExecutionQueueEnqueueCtmWithIntentParams = {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  executionQueueBuffer?: PublicKey;
  remainingAccounts: AccountMeta[];
  payload: Uint8Array;
  sequence: BigNumberish;
  minExecuteSlot: BigNumberish;
  expiresAtSlot?: BigNumberish;
  kind?: QueueItemKind;
  userOwner: PublicKey;
  mangoAccount: PublicKey;
  userSigner: IntentSigner;
  ctmSigner: IntentSigner;
};

const U16_MAX = 0xffff;
const U32_MAX = 0xffffffff;
const U64_MAX = (1n << 64n) - 1n;
const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;
const U128_MAX = (1n << 128n) - 1n;

function toBigInt(value: BigNumberish): bigint {
  if (typeof value === 'bigint') {
    return value;
  }
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) {
      throw new Error('number inputs must be safe integers');
    }
    return BigInt(value);
  }
  return BigInt(value.toString());
}

function u8(value: number): Buffer {
  if (!Number.isInteger(value) || value < 0 || value > 0xff) {
    throw new Error(`u8 out of range: ${value}`);
  }
  return Buffer.from([value]);
}

function u64ToLe(value: BigNumberish): Buffer {
  const n = toBigInt(value);
  if (n < 0 || n > U64_MAX) {
    throw new Error(`u64 out of range: ${n.toString()}`);
  }
  const out = Buffer.alloc(8);
  out.writeBigUInt64LE(n);
  return out;
}

function i64ToLe(value: BigNumberish): Buffer {
  const n = toBigInt(value);
  if (n < I64_MIN || n > I64_MAX) {
    throw new Error(`i64 out of range: ${n.toString()}`);
  }
  const out = Buffer.alloc(8);
  out.writeBigInt64LE(n);
  return out;
}

function u128ToLe(value: BigNumberish): Buffer {
  const n = toBigInt(value);
  if (n < 0 || n > U128_MAX) {
    throw new Error(`u128 out of range: ${n.toString()}`);
  }
  const out = Buffer.alloc(16);
  let x = n;
  for (let i = 0; i < 16; i++) {
    out[i] = Number(x & 0xffn);
    x >>= 8n;
  }
  return out;
}

function u32ToLe(value: number): Buffer {
  if (!Number.isInteger(value) || value < 0 || value > U32_MAX) {
    throw new Error(`u32 out of range: ${value}`);
  }
  const out = Buffer.alloc(4);
  out.writeUInt32LE(value, 0);
  return out;
}

function u16ToLe(value: number): Buffer {
  if (!Number.isInteger(value) || value < 0 || value > U16_MAX) {
    throw new Error(`u16 out of range: ${value}`);
  }
  const out = Buffer.alloc(2);
  out.writeUInt16LE(value, 0);
  return out;
}

function sha256(data: Uint8Array): Buffer {
  return Buffer.from(createHash('sha256').update(data).digest());
}

function encodeEnvelope(envelope: CtmEnvelopeWire): Buffer {
  if (envelope.payloadHash.length !== 32) {
    throw new Error('payloadHash must be 32 bytes');
  }
  if (envelope.accountsHash.length !== 32) {
    throw new Error('accountsHash must be 32 bytes');
  }
  return Buffer.concat([
    u64ToLe(envelope.sequence),
    u64ToLe(envelope.minExecuteSlot),
    u8(envelope.kind),
    Buffer.from(envelope.payloadHash),
    Buffer.from(envelope.accountsHash),
    u64ToLe(envelope.expiresAtSlot),
  ]);
}

function sideToU8(side: QueueSideLike): number {
  if (typeof side === 'number') {
    if (side === QueueSide.Bid || side === QueueSide.Ask) {
      return side;
    }
    throw new Error(`invalid side: ${side}`);
  }
  if ('bid' in side) {
    return QueueSide.Bid;
  }
  if ('ask' in side) {
    return QueueSide.Ask;
  }
  throw new Error('invalid side enum object');
}

function orderTypeToU8(orderType: QueuePlaceOrderTypeLike): number {
  if (typeof orderType === 'number') {
    if (orderType >= 0 && orderType <= 4) {
      return orderType;
    }
    throw new Error(`invalid orderType: ${orderType}`);
  }
  if ('limit' in orderType) {
    return QueuePlaceOrderType.Limit;
  }
  if ('immediateOrCancel' in orderType) {
    return QueuePlaceOrderType.ImmediateOrCancel;
  }
  if ('postOnly' in orderType) {
    return QueuePlaceOrderType.PostOnly;
  }
  if ('market' in orderType) {
    return QueuePlaceOrderType.Market;
  }
  if ('postOnlySlide' in orderType) {
    return QueuePlaceOrderType.PostOnlySlide;
  }
  throw new Error('invalid orderType enum object');
}

function selfTradeBehaviorToU8(
  selfTradeBehavior: QueueSelfTradeBehaviorLike,
): number {
  if (typeof selfTradeBehavior === 'number') {
    if (selfTradeBehavior >= 0 && selfTradeBehavior <= 2) {
      return selfTradeBehavior;
    }
    throw new Error(`invalid selfTradeBehavior: ${selfTradeBehavior}`);
  }
  if ('decrementTake' in selfTradeBehavior) {
    return QueueSelfTradeBehavior.DecrementTake;
  }
  if ('cancelProvide' in selfTradeBehavior) {
    return QueueSelfTradeBehavior.CancelProvide;
  }
  if ('abortTransaction' in selfTradeBehavior) {
    return QueueSelfTradeBehavior.AbortTransaction;
  }
  throw new Error('invalid selfTradeBehavior enum object');
}

export function encodeQueuePayloadV1(
  variant: QueuePayloadVariant,
  body: Uint8Array,
  flags = 0,
): Buffer {
  if (variant < 0 || variant > 6) {
    throw new Error(`invalid queue payload variant: ${variant}`);
  }
  return Buffer.concat([
    Buffer.from([1, variant]),
    u16ToLe(flags),
    Buffer.from(body),
  ]);
}

export function encodePerpPlaceOrderV2QueuePayload(
  fields: PerpPlaceOrderV2QueuePayloadFields,
): Buffer {
  const body = Buffer.concat([
    u8(sideToU8(fields.side)),
    i64ToLe(fields.priceLots),
    i64ToLe(fields.maxBaseLots),
    i64ToLe(fields.maxQuoteLots),
    u64ToLe(fields.clientOrderId),
    u8(orderTypeToU8(fields.orderType)),
    u8(selfTradeBehaviorToU8(fields.selfTradeBehavior)),
    u8(fields.reduceOnly ? 1 : 0),
    u64ToLe(fields.expiryTimestamp),
    u8(fields.limit),
  ]);
  return encodeQueuePayloadV1(QueuePayloadVariant.PerpPlaceOrderV2, body);
}

export function encodePerpCancelOrderQueuePayload(
  fields: PerpCancelOrderQueuePayloadFields,
): Buffer {
  return encodeQueuePayloadV1(
    QueuePayloadVariant.PerpCancelOrder,
    u128ToLe(fields.orderId),
  );
}

export function encodePerpCancelOrderByClientOrderIdQueuePayload(
  fields: PerpCancelOrderByClientOrderIdQueuePayloadFields,
): Buffer {
  return encodeQueuePayloadV1(
    QueuePayloadVariant.PerpCancelOrderByClientOrderId,
    u64ToLe(fields.clientOrderId),
  );
}

export function encodePerpCancelAllOrdersQueuePayload(
  fields: PerpCancelAllOrdersQueuePayloadFields,
): Buffer {
  return encodeQueuePayloadV1(
    QueuePayloadVariant.PerpCancelAllOrders,
    u8(fields.limit),
  );
}

export function encodePerpCancelAllOrdersBySideQueuePayload(
  fields: PerpCancelAllOrdersBySideQueuePayloadFields,
): Buffer {
  const sideOption =
    fields.side === null
      ? Buffer.from([0])
      : Buffer.from([1, sideToU8(fields.side)]);
  return encodeQueuePayloadV1(
    QueuePayloadVariant.PerpCancelAllOrdersBySide,
    Buffer.concat([sideOption, u8(fields.limit)]),
  );
}

export function encodeLiquidityDepositQueuePayload(
  fields: LiquidityDepositQueuePayloadFields,
): Buffer {
  return encodeQueuePayloadV1(
    QueuePayloadVariant.LiquidityDeposit,
    Buffer.concat([u64ToLe(fields.amount), u8(fields.reduceOnly ? 1 : 0)]),
  );
}

export function encodeLiquidityWithdrawQueuePayload(
  fields: LiquidityWithdrawQueuePayloadFields,
): Buffer {
  return encodeQueuePayloadV1(
    QueuePayloadVariant.LiquidityWithdraw,
    Buffer.concat([u64ToLe(fields.amount), u8(fields.allowBorrow ? 1 : 0)]),
  );
}

export function anchorInstructionDiscriminator(ixName: string): Buffer {
  const cached = instructionDiscriminatorCache.get(ixName);
  if (cached) {
    return cached;
  }
  const h = sha256(Buffer.from(`global:${ixName}`, 'utf-8'));
  const discriminator = Buffer.from(h.subarray(0, 8));
  instructionDiscriminatorCache.set(ixName, discriminator);
  return discriminator;
}

export function hashExecutionQueueAccounts(accounts: AccountMeta[]): Buffer {
  const bytes = Buffer.alloc(accounts.length * 34);
  let offset = 0;
  for (const a of accounts) {
    bytes.set(a.pubkey.toBytes(), offset);
    offset += 32;
    bytes[offset] = a.isSigner ? 1 : 0;
    offset += 1;
    bytes[offset] = a.isWritable ? 1 : 0;
    offset += 1;
  }
  return sha256(bytes);
}

function mergeEffectiveRuntimeFlags(
  remainingAccounts: AccountMeta[],
  fixedAccounts: AccountMeta[],
): AccountMeta[] {
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const a of [...fixedAccounts, ...remainingAccounts]) {
    const key = a.pubkey.toBase58();
    const prev = merged.get(key);
    if (!prev) {
      merged.set(key, { isSigner: !!a.isSigner, isWritable: !!a.isWritable });
      continue;
    }
    prev.isSigner = prev.isSigner || !!a.isSigner;
    prev.isWritable = prev.isWritable || !!a.isWritable;
  }
  return remainingAccounts.map((a) => {
    const key = a.pubkey.toBase58();
    const effective = merged.get(key)!;
    return {
      pubkey: a.pubkey,
      isSigner: effective.isSigner,
      isWritable: effective.isWritable,
    };
  });
}

function hashExecutionQueueAccountsForCtmEnqueue(
  group: PublicKey,
  executionQueue: PublicKey,
  remainingAccounts: AccountMeta[],
): Buffer {
  const effectiveRemaining = mergeEffectiveRuntimeFlags(remainingAccounts, [
    { pubkey: group, isSigner: false, isWritable: true },
    { pubkey: executionQueue, isSigner: false, isWritable: true },
    { pubkey: SYSVAR_INSTRUCTIONS_PUBKEY, isSigner: false, isWritable: false },
  ]);
  return hashExecutionQueueAccounts(effectiveRemaining);
}

export function hashExecutionQueuePayload(payload: Uint8Array): Buffer {
  return sha256(Buffer.from(payload));
}

export function buildCtmEnvelopeMessage(
  group: PublicKey,
  envelope: CtmEnvelopeWire,
): Buffer {
  return sha256(
    Buffer.concat([
      EXECUTION_QUEUE_DOMAIN_BYTES,
      Buffer.from(group.toBytes()),
      u64ToLe(envelope.sequence),
      u64ToLe(envelope.minExecuteSlot),
      u8(envelope.kind),
      Buffer.from(envelope.payloadHash),
      Buffer.from(envelope.accountsHash),
      u64ToLe(envelope.expiresAtSlot),
    ]),
  );
}

export function buildUserIntentMessage(
  group: PublicKey,
  mangoAccount: PublicKey,
  userOwner: PublicKey,
  envelope: CtmEnvelopeWire,
): Buffer {
  return sha256(
    Buffer.concat([
      USER_INTENT_DOMAIN_BYTES,
      Buffer.from(group.toBytes()),
      Buffer.from(mangoAccount.toBytes()),
      Buffer.from(userOwner.toBytes()),
      u8(envelope.kind),
      Buffer.from(envelope.payloadHash),
      Buffer.from(envelope.accountsHash),
    ]),
  );
}

export function buildExecutionQueueUserIntent(
  params: BuildExecutionQueueUserIntentParams,
): {
  envelopeLike: CtmEnvelopeWire;
  payloadHash: Buffer;
  accountsHash: Buffer;
  userIntentMessage: Buffer;
} {
  const payloadHash = hashExecutionQueuePayload(params.payload);
  const accountsHash = params.executionQueue
    ? hashExecutionQueueAccountsForCtmEnqueue(
        params.group,
        params.executionQueue,
        params.remainingAccounts,
      )
    : hashExecutionQueueAccounts(params.remainingAccounts);
  const envelopeLike: CtmEnvelopeWire = {
    sequence: 0n,
    minExecuteSlot: 0n,
    kind: params.kind ?? QueueItemKind.CtmWrapped,
    payloadHash,
    accountsHash,
    expiresAtSlot: 0n,
  };
  const userIntentMessage = buildUserIntentMessage(
    params.group,
    params.mangoAccount,
    params.userOwner,
    envelopeLike,
  );
  return { envelopeLike, payloadHash, accountsHash, userIntentMessage };
}

export function signExecutionQueueIntentMessage(
  privateKey: Uint8Array,
  message: Uint8Array,
): Uint8Array {
  if (privateKey.length !== 64) {
    throw new Error('privateKey must be 64 bytes');
  }
  return nacl.sign.detached(message, privateKey);
}

export function buildIntentEd25519Instruction(
  message: Uint8Array,
  signer: IntentSigner,
): TransactionInstruction {
  if (signer.kind === 'keypair') {
    if (signer.privateKey.length !== 64) {
      throw new Error('keypair privateKey must be 64 bytes');
    }
    const publicKey = signer.publicKey ?? signer.privateKey.subarray(32, 64);
    const signature = nacl.sign.detached(message, signer.privateKey);
    return Ed25519Program.createInstructionWithPublicKey({
      publicKey,
      message,
      signature,
    });
  }
  if (signer.signature.length !== 64) {
    throw new Error('presigned signature must be 64 bytes');
  }
  return Ed25519Program.createInstructionWithPublicKey({
    publicKey: signer.publicKey.toBytes(),
    message,
    signature: signer.signature,
  });
}

export function buildExecutionQueueEnqueueCtmIx(
  params: BuildExecutionQueueEnqueueCtmParams,
): TransactionInstruction {
  const discriminator = anchorInstructionDiscriminator(
    'execution_queue_enqueue_ctm',
  );

  const data = Buffer.concat([
    discriminator,
    encodeEnvelope(params.envelope),
    u32ToLe(params.payload.length),
    Buffer.from(params.payload),
  ]);

  const keys: AccountMeta[] = [
    { pubkey: params.group, isSigner: false, isWritable: true },
    { pubkey: params.executionQueue, isSigner: false, isWritable: true },
    {
      pubkey: SYSVAR_INSTRUCTIONS_PUBKEY,
      isSigner: false,
      isWritable: false,
    },
    ...params.remainingAccounts,
    {
      pubkey: params.programId,
      isSigner: false,
      isWritable: false,
    },
  ];

  return new TransactionInstruction({
    programId: params.programId,
    keys,
    data,
  });
}

export function buildExecutionQueueEnqueueLiquidityIx(
  params: BuildExecutionQueueEnqueueLiquidityParams,
): TransactionInstruction {
  if (
    params.kind !== QueueItemKind.LiquidityDeposit &&
    params.kind !== QueueItemKind.LiquidityWithdraw
  ) {
    throw new Error('enqueue_liquidity requires a liquidity kind');
  }

  const discriminator = anchorInstructionDiscriminator(
    'execution_queue_enqueue_liquidity',
  );
  const data = Buffer.concat([
    discriminator,
    u8(params.kind),
    u32ToLe(params.payload.length),
    Buffer.from(params.payload),
  ]);
  return new TransactionInstruction({
    programId: params.programId,
    keys: [
      { pubkey: params.group, isSigner: false, isWritable: true },
      { pubkey: params.executionQueue, isSigner: false, isWritable: true },
      ...params.remainingAccounts,
      {
        pubkey: params.programId,
        isSigner: false,
        isWritable: false,
      },
    ],
    data,
  });
}

export function buildExecutionQueueExecuteIx(
  params: BuildExecutionQueueExecuteParams,
): TransactionInstruction {
  const discriminator = anchorInstructionDiscriminator(
    'execution_queue_execute',
  );
  const data = Buffer.concat([discriminator, u16ToLe(params.maxItems)]);
  return new TransactionInstruction({
    programId: params.programId,
    keys: [
      { pubkey: params.group, isSigner: false, isWritable: true },
      { pubkey: params.executionQueue, isSigner: false, isWritable: true },
      ...params.remainingAccounts,
      {
        pubkey: params.programId,
        isSigner: false,
        isWritable: false,
      },
    ],
    data,
  });
}

export function buildExecutionQueueEnqueueCtmWithIntentIxs(
  params: BuildExecutionQueueEnqueueCtmWithIntentParams,
): {
  envelope: CtmEnvelopeWire;
  userIntentMessage: Buffer;
  ctmEnvelopeMessage: Buffer;
  userIntentPreInstruction: TransactionInstruction;
  ctmEnvelopePreInstruction: TransactionInstruction;
  enqueueInstruction: TransactionInstruction;
  instructions: TransactionInstruction[];
} {
  const kind = params.kind ?? QueueItemKind.CtmWrapped;
  if (kind !== QueueItemKind.CtmWrapped) {
    throw new Error('enqueue_ctm requires QueueItemKind.CtmWrapped');
  }

  const payloadHash = hashExecutionQueuePayload(params.payload);
  const accountsHash = hashExecutionQueueAccountsForCtmEnqueue(
    params.group,
    params.executionQueue,
    params.remainingAccounts,
  );
  const envelope: CtmEnvelopeWire = {
    sequence: toBigInt(params.sequence),
    minExecuteSlot: toBigInt(params.minExecuteSlot),
    kind,
    payloadHash,
    accountsHash,
    expiresAtSlot: toBigInt(params.expiresAtSlot ?? 0),
  };

  const userIntentMessage = buildUserIntentMessage(
    params.group,
    params.mangoAccount,
    params.userOwner,
    envelope,
  );
  const ctmEnvelopeMessage = buildCtmEnvelopeMessage(params.group, envelope);

  const userIntentPreInstruction = buildIntentEd25519Instruction(
    userIntentMessage,
    params.userSigner,
  );
  const ctmEnvelopePreInstruction = buildIntentEd25519Instruction(
    ctmEnvelopeMessage,
    params.ctmSigner,
  );
  const enqueueInstruction = buildExecutionQueueEnqueueCtmIx({
    programId: params.programId,
    group: params.group,
    executionQueue: params.executionQueue,
    executionQueueBuffer: params.executionQueueBuffer,
    remainingAccounts: params.remainingAccounts,
    envelope,
    payload: params.payload,
  });

  return {
    envelope,
    userIntentMessage,
    ctmEnvelopeMessage,
    userIntentPreInstruction,
    ctmEnvelopePreInstruction,
    enqueueInstruction,
    instructions: [
      userIntentPreInstruction,
      ctmEnvelopePreInstruction,
      enqueueInstruction,
    ],
  };
}
