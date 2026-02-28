import { expect } from 'chai';
import { Keypair, SYSVAR_INSTRUCTIONS_PUBKEY } from '@solana/web3.js';
import {
  QueueItemKind,
  QueuePlaceOrderType,
  QueuePayloadVariant,
  QueueSelfTradeBehavior,
  QueueSide,
  buildCtmEnvelopeMessage,
  buildExecutionQueueEnqueueCtmWithIntentIxs,
  buildExecutionQueueEnqueueLiquidityIx,
  buildExecutionQueueUserIntent,
  buildExecutionQueueExecuteIx,
  buildUserIntentMessage,
  encodeLiquidityDepositQueuePayload,
  encodePerpCancelAllOrdersBySideQueuePayload,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
  hashExecutionQueueAccounts,
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
    expect(payload.length).eq(46);
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

    const userA = await buildUserIntentMessage(group, mangoAccount, ownerA, envelope);
    const userB = await buildUserIntentMessage(group, mangoAccount, ownerB, envelope);
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
    const expectedAccountsHash = await hashExecutionQueueAccounts(remainingAccounts);
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
      maxItems: 3,
      remainingAccounts: [
        {
          pubkey: Keypair.generate().publicKey,
          isWritable: false,
          isSigner: false,
        },
      ],
    });

    expect(enqueueLiquidityIx.keys.length).eq(4);
    expect(executeIx.keys.length).eq(5);
    expect(executeIx.data.length).eq(10);
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
      remainingAccounts,
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
});
