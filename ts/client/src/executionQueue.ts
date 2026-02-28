import BN from 'bn.js';
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
  | { kind: 'keypair'; privateKey: Uint8Array }
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
  remainingAccounts: AccountMeta[];
  envelope: CtmEnvelopeWire;
  payload: Uint8Array;
};

export type BuildExecutionQueueEnqueueLiquidityParams = {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  kind: QueueItemKind.LiquidityDeposit | QueueItemKind.LiquidityWithdraw;
  remainingAccounts: AccountMeta[];
  payload: Uint8Array;
};

export type BuildExecutionQueueExecuteParams = {
  programId: PublicKey;
  group: PublicKey;
  executionQueue: PublicKey;
  remainingAccounts: AccountMeta[];
  maxItems: number;
};

export type BuildExecutionQueueUserIntentParams = {
  group: PublicKey;
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

async function sha256(data: Uint8Array): Promise<Buffer> {
  if (globalThis.crypto?.subtle) {
    const digest = await globalThis.crypto.subtle.digest('SHA-256', data);
    return Buffer.from(digest);
  }
  const crypto = await import('crypto');
  return Buffer.from(crypto.createHash('sha256').update(data).digest());
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

export async function anchorInstructionDiscriminator(
  ixName: string,
): Promise<Buffer> {
  const h = await sha256(Buffer.from(`global:${ixName}`, 'utf-8'));
  return h.subarray(0, 8);
}

export async function hashExecutionQueueAccounts(
  accounts: AccountMeta[],
): Promise<Buffer> {
  const bytes: number[] = [];
  for (const a of accounts) {
    bytes.push(...a.pubkey.toBytes());
    bytes.push(a.isSigner ? 1 : 0);
    bytes.push(a.isWritable ? 1 : 0);
  }
  return await sha256(Buffer.from(bytes));
}

export async function hashExecutionQueuePayload(
  payload: Uint8Array,
): Promise<Buffer> {
  return await sha256(Buffer.from(payload));
}

export async function buildCtmEnvelopeMessage(
  group: PublicKey,
  envelope: CtmEnvelopeWire,
): Promise<Buffer> {
  return await sha256(
    Buffer.concat([
      Buffer.from(EXECUTION_QUEUE_DOMAIN, 'utf-8'),
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

export async function buildUserIntentMessage(
  group: PublicKey,
  mangoAccount: PublicKey,
  userOwner: PublicKey,
  envelope: CtmEnvelopeWire,
): Promise<Buffer> {
  return await sha256(
    Buffer.concat([
      Buffer.from(USER_INTENT_DOMAIN, 'utf-8'),
      Buffer.from(group.toBytes()),
      Buffer.from(mangoAccount.toBytes()),
      Buffer.from(userOwner.toBytes()),
      u8(envelope.kind),
      Buffer.from(envelope.payloadHash),
      Buffer.from(envelope.accountsHash),
    ]),
  );
}

export async function buildExecutionQueueUserIntent(
  params: BuildExecutionQueueUserIntentParams,
): Promise<{
  envelopeLike: CtmEnvelopeWire;
  payloadHash: Buffer;
  accountsHash: Buffer;
  userIntentMessage: Buffer;
}> {
  const payloadHash = await hashExecutionQueuePayload(params.payload);
  const accountsHash = await hashExecutionQueueAccounts(params.remainingAccounts);
  const envelopeLike: CtmEnvelopeWire = {
    sequence: 0n,
    minExecuteSlot: 0n,
    kind: params.kind ?? QueueItemKind.CtmWrapped,
    payloadHash,
    accountsHash,
    expiresAtSlot: 0n,
  };
  const userIntentMessage = await buildUserIntentMessage(
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
    return Ed25519Program.createInstructionWithPrivateKey({
      privateKey: signer.privateKey,
      message,
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

export async function buildExecutionQueueEnqueueCtmIx(
  params: BuildExecutionQueueEnqueueCtmParams,
): Promise<TransactionInstruction> {
  const discriminator = await anchorInstructionDiscriminator(
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
  ];

  return new TransactionInstruction({
    programId: params.programId,
    keys,
    data,
  });
}

export async function buildExecutionQueueEnqueueLiquidityIx(
  params: BuildExecutionQueueEnqueueLiquidityParams,
): Promise<TransactionInstruction> {
  if (
    params.kind !== QueueItemKind.LiquidityDeposit &&
    params.kind !== QueueItemKind.LiquidityWithdraw
  ) {
    throw new Error('enqueue_liquidity requires a liquidity kind');
  }

  const discriminator = await anchorInstructionDiscriminator(
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
      { pubkey: params.group, isSigner: false, isWritable: false },
      { pubkey: params.executionQueue, isSigner: false, isWritable: true },
      ...params.remainingAccounts,
    ],
    data,
  });
}

export async function buildExecutionQueueExecuteIx(
  params: BuildExecutionQueueExecuteParams,
): Promise<TransactionInstruction> {
  const discriminator = await anchorInstructionDiscriminator(
    'execution_queue_execute',
  );
  const data = Buffer.concat([discriminator, u16ToLe(params.maxItems)]);
  return new TransactionInstruction({
    programId: params.programId,
    keys: [
      { pubkey: params.group, isSigner: false, isWritable: false },
      { pubkey: params.executionQueue, isSigner: false, isWritable: true },
      ...params.remainingAccounts,
    ],
    data,
  });
}

export async function buildExecutionQueueEnqueueCtmWithIntentIxs(
  params: BuildExecutionQueueEnqueueCtmWithIntentParams,
): Promise<{
  envelope: CtmEnvelopeWire;
  userIntentMessage: Buffer;
  ctmEnvelopeMessage: Buffer;
  userIntentPreInstruction: TransactionInstruction;
  ctmEnvelopePreInstruction: TransactionInstruction;
  enqueueInstruction: TransactionInstruction;
  instructions: TransactionInstruction[];
}> {
  const kind = params.kind ?? QueueItemKind.CtmWrapped;
  if (kind !== QueueItemKind.CtmWrapped) {
    throw new Error('enqueue_ctm requires QueueItemKind.CtmWrapped');
  }

  const payloadHash = await hashExecutionQueuePayload(params.payload);
  const accountsHash = await hashExecutionQueueAccounts(params.remainingAccounts);
  const envelope: CtmEnvelopeWire = {
    sequence: toBigInt(params.sequence),
    minExecuteSlot: toBigInt(params.minExecuteSlot),
    kind,
    payloadHash,
    accountsHash,
    expiresAtSlot: toBigInt(params.expiresAtSlot ?? 0),
  };

  const userIntentMessage = await buildUserIntentMessage(
    params.group,
    params.mangoAccount,
    params.userOwner,
    envelope,
  );
  const ctmEnvelopeMessage = await buildCtmEnvelopeMessage(params.group, envelope);

  const userIntentPreInstruction = buildIntentEd25519Instruction(
    userIntentMessage,
    params.userSigner,
  );
  const ctmEnvelopePreInstruction = buildIntentEd25519Instruction(
    ctmEnvelopeMessage,
    params.ctmSigner,
  );
  const enqueueInstruction = await buildExecutionQueueEnqueueCtmIx({
    programId: params.programId,
    group: params.group,
    executionQueue: params.executionQueue,
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
