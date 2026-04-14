import { expect } from 'chai';
import { Keypair, SYSVAR_INSTRUCTIONS_PUBKEY } from '@solana/web3.js';
import {
  QueueItemKind,
  QueuePlaceOrderType,
  QueuePayloadVariant,
  PerpBatchSubOpVariant,
  QueueSelfTradeBehavior,
  QueueSide,
  UserIntentTargetKind,
  UserIntentVersion,
  buildCtmEnvelopeMessage,
  buildExecutionQueueEnqueueCtmWithIntentIxs,
  buildExecutionQueueEnqueueDirectWithIntentIxs,
  buildExecutionQueueEnqueueLiquidityIx,
  buildExecutionQueueUserIntent,
  buildExecutionQueueExecuteIx,
  buildLegacyUserIntentMessage,
  buildUserIntentMessage,
  encodeLiquidityDepositQueuePayload,
  encodeLiquidityWithdrawQueuePayload,
  encodePerpCancelAllOrdersBySideQueuePayload,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpBatchIntentQueuePayload,
  encodePerpCancelOrderQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
  hashExecutionQueuePayload,
  signExecutionQueueIntentMessage,
} from './executionQueue';
import nacl from 'tweetnacl';

describe('Execution Queue Helpers', () => {
  it('encodes payload header and body fields correctly', () => {
    const payload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Bid,
      priceLots: 10,
      maxBaseLots: 2,
      maxQuoteLots: 100,
      clientOrderId: 7,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 4,
    });
    expect(payload[0]).eq(1);
    expect(payload[1]).eq(QueuePayloadVariant.PerpPlaceOrderV2);
    expect(payload[2]).eq(0);
    expect(payload[3]).eq(0);
    expect(payload.length).eq(49);
  });

  it('encodes option<side> payloads deterministically', () => {
    const noneSide = encodePerpCancelAllOrdersBySideQueuePayload({
      side: null,
      limit: 5,
    });
    const bidSide = encodePerpCancelAllOrdersBySideQueuePayload({
      side: QueueSide.Bid,
      limit: 5,
    });
    expect(Array.from(noneSide.slice(0, 4))).deep.eq([1, 4, 0, 0]);
    expect(Array.from(noneSide.slice(4))).deep.eq([0, 5]);
    expect(Array.from(bidSide.slice(4))).deep.eq([1, 0, 5]);
  });

  it('builds intent and envelope messages that bind user owner', async () => {
    const group = Keypair.generate().publicKey;
    const mangoAccount = Keypair.generate().publicKey;
    const ownerA = Keypair.generate().publicKey;
    const ownerB = Keypair.generate().publicKey;
    const envelope = {
      sequence: 1n,
      minExecuteSlot: 2n,
      kind: QueueItemKind.CtmWrapped,
      payloadHash: Buffer.alloc(32, 7),
      accountsHash: Buffer.alloc(32, 9),
      expiresAtSlot: 0n,
    };

    const target = { kind: UserIntentTargetKind.PerpMarket, index: 7 };
    const userA = await buildUserIntentMessage(
      group,
      mangoAccount,
      ownerA,
      envelope,
      target,
    );
    const userB = await buildUserIntentMessage(
      group,
      mangoAccount,
      ownerB,
      envelope,
      target,
    );
    const ctmA = await buildCtmEnvelopeMessage(group, envelope);
    const ctmB = await buildCtmEnvelopeMessage(group, envelope);

    expect(Buffer.compare(userA, userB)).not.eq(0);
    expect(Buffer.compare(ctmA, ctmB)).eq(0);
  });

  it('builds ordered preinstructions and enqueue ix', async () => {
    const programId = Keypair.generate().publicKey;
    const group = Keypair.generate().publicKey;
    const executionQueue = Keypair.generate().publicKey;
    const executionQueueBuffer = Keypair.generate().publicKey;
    const mangoAccount = Keypair.generate().publicKey;
    const user = Keypair.generate();
    const ctm = Keypair.generate();

    const remainingAccounts = [
      {
        pubkey: Keypair.generate().publicKey,
        isWritable: false,
        isSigner: false,
      },
      { pubkey: mangoAccount, isWritable: true, isSigner: false },
      { pubkey: executionQueue, isWritable: false, isSigner: false },
      {
        pubkey: Keypair.generate().publicKey,
        isWritable: true,
        isSigner: false,
      },
    ];
    const payload = encodePerpCancelAllOrdersQueuePayload({ limit: 10 });

    const built = await buildExecutionQueueEnqueueCtmWithIntentIxs({
      programId,
      group,
      executionQueue,
      executionQueueBuffer,
      marketIndex: 0,
      remainingAccounts,
      payload,
      sequence: 42,
      minExecuteSlot: 99,
      mangoAccount,
      userOwner: user.publicKey,
      userSigner: { kind: 'keypair', privateKey: user.secretKey },
      ctmSigner: { kind: 'keypair', privateKey: ctm.secretKey },
    });

    expect(built.instructions.length).eq(3);
    expect(built.instructions[0]).eq(built.userIntentPreInstruction);
    expect(built.instructions[1]).eq(built.ctmEnvelopePreInstruction);
    expect(built.instructions[2]).eq(built.enqueueInstruction);
    expect(
      built.enqueueInstruction.keys[2].pubkey.equals(SYSVAR_INSTRUCTIONS_PUBKEY),
    ).to.be.true;
    expect(built.userIntentMessage.length).eq(32);
    expect(built.ctmEnvelopeMessage.length).eq(32);

    const expectedPayloadHash = await hashExecutionQueuePayload(payload);
    const expectedAccountsHash = buildExecutionQueueUserIntent({
      group,
      executionQueue,
      mangoAccount,
      userOwner: user.publicKey,
      payload,
      target: { kind: UserIntentTargetKind.PerpMarket, index: 0 },
      remainingAccounts,
    }).accountsHash;
    expect(Buffer.compare(built.envelope.payloadHash, expectedPayloadHash)).eq(0);
    expect(Buffer.compare(built.envelope.accountsHash, expectedAccountsHash)).eq(0);
  });

  it('builds liquidity enqueue and execute instructions', async () => {
    const programId = Keypair.generate().publicKey;
    const group = Keypair.generate().publicKey;
    const executionQueue = Keypair.generate().publicKey;
    const executionQueueBuffer = Keypair.generate().publicKey;
    const liquidityPayload = encodeLiquidityDepositQueuePayload({
      amount: 1,
      reduceOnly: false,
    });
    const enqueueLiquidityIx = await buildExecutionQueueEnqueueLiquidityIx({
      programId,
      group,
      executionQueue,
      executionQueueBuffer,
      kind: QueueItemKind.LiquidityDeposit,
      remainingAccounts: [],
      payload: liquidityPayload,
    });
    const executeIx = await buildExecutionQueueExecuteIx({
      programId,
      group,
      executionQueue,
      executionQueueBuffer,
      marketIndex: 0,
      maxItems: 3,
      remainingAccounts: [
        {
          pubkey: Keypair.generate().publicKey,
          isWritable: false,
          isSigner: false,
        },
      ],
    });

    expect(enqueueLiquidityIx.keys.length).eq(3);
    expect(executeIx.keys.length).eq(3);
    // v2 execute_ix data: 8 (discriminator) + 2 (market_index) + 2 (max_items) = 12
    expect(executeIx.data.length).eq(12);
  });

  it('builds and signs a user intent payload for relayer submission', async () => {
    const user = Keypair.generate();
    const group = Keypair.generate().publicKey;
    const mangoAccount = Keypair.generate().publicKey;
    const payload = encodePerpCancelAllOrdersQueuePayload({ limit: 9 });
    const remainingAccounts = [
      { pubkey: Keypair.generate().publicKey, isWritable: false, isSigner: false },
      { pubkey: mangoAccount, isWritable: true, isSigner: false },
      { pubkey: Keypair.generate().publicKey, isWritable: false, isSigner: false },
    ];
    const built = await buildExecutionQueueUserIntent({
      group,
      mangoAccount,
      userOwner: user.publicKey,
      payload,
      target: { kind: UserIntentTargetKind.PerpMarket, index: 3 },
    });
    const sig = signExecutionQueueIntentMessage(
      user.secretKey,
      built.userIntentMessage,
    );
    expect(sig.length).eq(64);
    expect(
      nacl.sign.detached.verify(
        new Uint8Array(built.userIntentMessage),
        new Uint8Array(sig),
        user.publicKey.toBytes(),
      ),
    ).to.be.true;
  });

  it('encodes_u128_order_id_for_cancel', () => {
    const largeOrderId = (1n << 127n) - 1n;
    const payload = encodePerpCancelOrderQueuePayload({
      orderId: largeOrderId,
    });
    // header (4 bytes) + u128 body (16 bytes) = 20 bytes
    expect(payload.length).eq(20);
    // version=1, variant=PerpCancelOrder(1)
    expect(payload[0]).eq(1);
    expect(payload[1]).eq(1);
    // flags = 0
    expect(payload.readUInt16LE(2)).eq(0);
    // read back u128 LE from body
    const lo = payload.readBigUInt64LE(4);
    const hi = payload.readBigUInt64LE(12);
    const decoded = (hi << 64n) + lo;
    expect(decoded).eq(largeOrderId);
  });

  it('client_order_id_near_2_pow_53', () => {
    const clientOrderId = Number.MAX_SAFE_INTEGER; // 2^53 - 1
    const payload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Ask,
      priceLots: 50,
      maxBaseLots: 1,
      maxQuoteLots: 100,
      clientOrderId,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 5,
    });
    expect(payload.length).eq(49);
    // Read back clientOrderId from body offset: header(4) + side(1) + i64*3(24) = offset 29
    const readBack = payload.readBigUInt64LE(29);
    expect(readBack).eq(BigInt(clientOrderId));
  });

  it('zero_amount_deposit', () => {
    const payload = encodeLiquidityDepositQueuePayload({
      amount: 0,
      reduceOnly: false,
    });
    // header (4 bytes) + u64 amount (8) + u8 reduceOnly (1) = 13 bytes
    expect(payload.length).eq(13);
    expect(payload[0]).eq(1);
    expect(payload[1]).eq(5); // LiquidityDeposit variant
    const amount = payload.readBigUInt64LE(4);
    expect(amount).eq(0n);
    expect(payload[12]).eq(0); // reduceOnly false
  });

  it('max_amount_withdraw', () => {
    const maxU64 = (1n << 64n) - 1n;
    const payload = encodeLiquidityWithdrawQueuePayload({
      amount: maxU64,
      allowBorrow: true,
    });
    // header (4 bytes) + u64 amount (8) + u8 allowBorrow (1) = 13 bytes
    expect(payload.length).eq(13);
    expect(payload[0]).eq(1);
    expect(payload[1]).eq(6); // LiquidityWithdraw variant
    const amount = payload.readBigUInt64LE(4);
    expect(amount).eq(maxU64);
    expect(payload[12]).eq(1); // allowBorrow true
  });

  it('message_changes_with_mango_account', () => {
    const group = Keypair.generate().publicKey;
    const mangoAccountA = Keypair.generate().publicKey;
    const mangoAccountB = Keypair.generate().publicKey;
    const owner = Keypair.generate().publicKey;
    const envelope = {
      sequence: 1n,
      minExecuteSlot: 2n,
      kind: QueueItemKind.CtmWrapped,
      payloadHash: Buffer.alloc(32, 7),
      accountsHash: Buffer.alloc(32, 9),
      expiresAtSlot: 0n,
    };

    const msgA = buildUserIntentMessage(
      group,
      mangoAccountA,
      owner,
      envelope,
      { kind: UserIntentTargetKind.PerpMarket, index: 7 },
    );
    const msgB = buildUserIntentMessage(
      group,
      mangoAccountB,
      owner,
      envelope,
      { kind: UserIntentTargetKind.PerpMarket, index: 7 },
    );
    expect(Buffer.compare(msgA, msgB)).not.eq(0);
  });

  it('v2 user intent message does not depend on accounts hash', () => {
    const group = Keypair.generate().publicKey;
    const mangoAccount = Keypair.generate().publicKey;
    const owner = Keypair.generate().publicKey;
    const envelopeA = {
      sequence: 1n,
      minExecuteSlot: 2n,
      kind: QueueItemKind.CtmWrapped,
      payloadHash: Buffer.alloc(32, 7),
      accountsHash: Buffer.alloc(32, 9),
      expiresAtSlot: 0n,
    };
    const envelopeB = {
      ...envelopeA,
      accountsHash: Buffer.alloc(32, 11),
    };

    const v2A = buildUserIntentMessage(
      group,
      mangoAccount,
      owner,
      envelopeA,
      { kind: UserIntentTargetKind.PerpMarket, index: 7 },
    );
    const v2B = buildUserIntentMessage(
      group,
      mangoAccount,
      owner,
      envelopeB,
      { kind: UserIntentTargetKind.PerpMarket, index: 7 },
    );
    const v1A = buildLegacyUserIntentMessage(group, mangoAccount, owner, envelopeA);
    const v1B = buildLegacyUserIntentMessage(group, mangoAccount, owner, envelopeB);

    expect(Buffer.compare(v2A, v2B)).eq(0);
    expect(Buffer.compare(v1A, v1B)).not.eq(0);
  });

  it('v2 user intent message changes with target index', () => {
    const group = Keypair.generate().publicKey;
    const mangoAccount = Keypair.generate().publicKey;
    const owner = Keypair.generate().publicKey;
    const envelope = {
      sequence: 1n,
      minExecuteSlot: 2n,
      kind: QueueItemKind.CtmWrapped,
      payloadHash: Buffer.alloc(32, 7),
      accountsHash: Buffer.alloc(32, 9),
      expiresAtSlot: 0n,
    };

    const market0 = buildUserIntentMessage(
      group,
      mangoAccount,
      owner,
      envelope,
      { kind: UserIntentTargetKind.PerpMarket, index: 0 },
    );
    const market1 = buildUserIntentMessage(
      group,
      mangoAccount,
      owner,
      envelope,
      { kind: UserIntentTargetKind.PerpMarket, index: 1 },
    );

    expect(Buffer.compare(market0, market1)).not.eq(0);
  });

  it('builds direct enqueue with user-only preinstruction', () => {
    const programId = Keypair.generate().publicKey;
    const group = Keypair.generate().publicKey;
    const executionQueue = Keypair.generate().publicKey;
    const mangoAccount = Keypair.generate().publicKey;
    const user = Keypair.generate();
    const payload = encodePerpCancelAllOrdersQueuePayload({ limit: 10 });
    const remainingAccounts = [
      { pubkey: group, isWritable: false, isSigner: false },
      { pubkey: mangoAccount, isWritable: true, isSigner: false },
      { pubkey: user.publicKey, isWritable: false, isSigner: false },
      { pubkey: Keypair.generate().publicKey, isWritable: true, isSigner: false },
    ];

    const built = buildExecutionQueueEnqueueDirectWithIntentIxs({
      programId,
      group,
      executionQueue,
      marketIndex: 2,
      remainingAccounts,
      payload,
      intentVersion: UserIntentVersion.V2,
      userOwner: user.publicKey,
      mangoAccount,
      userSigner: { kind: 'keypair', privateKey: user.secretKey },
    });

    expect(built.instructions.length).eq(2);
    expect(built.instructions[0]).eq(built.userIntentPreInstruction);
    expect(built.instructions[1]).eq(built.enqueueInstruction);
    expect(built.envelope.sequence).eq(0n);
  });

  it('message_changes_with_sequence', () => {
    const group = Keypair.generate().publicKey;
    const envelopeA = {
      sequence: 1n,
      minExecuteSlot: 2n,
      kind: QueueItemKind.CtmWrapped,
      payloadHash: Buffer.alloc(32, 7),
      accountsHash: Buffer.alloc(32, 9),
      expiresAtSlot: 0n,
    };
    const envelopeB = {
      ...envelopeA,
      sequence: 2n,
    };

    const msgA = buildCtmEnvelopeMessage(group, envelopeA);
    const msgB = buildCtmEnvelopeMessage(group, envelopeB);
    expect(Buffer.compare(msgA, msgB)).not.eq(0);
  });

  it('encodes batch intent payloads deterministically', () => {
    const payload = encodePerpBatchIntentQueuePayload({
      operations: [
        {
          kind: 'cancelBySlot',
          slot: 3,
          expectedOrderId: 17n,
        },
        {
          kind: 'place',
          side: QueueSide.Bid,
          priceLots: 12,
          maxBaseLots: 4,
          maxQuoteLots: 48,
          clientOrderId: 99,
          orderType: QueuePlaceOrderType.Limit,
          selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
          reduceOnly: false,
          expiryTimestamp: 0,
          limit: 7,
        },
      ],
    });

    expect(Array.from(payload.slice(0, 5))).deep.eq([
      1,
      QueuePayloadVariant.PerpBatchIntent,
      0,
      0,
      2,
    ]);
    expect(payload[5]).eq(PerpBatchSubOpVariant.PerpCancelOrderBySlot);
    expect(payload[23]).eq(PerpBatchSubOpVariant.PerpPlaceOrderV2);
  });
});
