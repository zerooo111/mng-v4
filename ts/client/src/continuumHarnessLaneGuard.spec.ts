import { expect } from 'chai';
import { Keypair } from '@solana/web3.js';
import {
  QueueProcessStatus,
  RelayIntentAcceptedEvent,
} from './continuumHarnessTsEngine';
import {
  ContinuumHarnessLaneGuard,
  computeAcceptedIntentLaneHash,
} from './continuumHarnessLaneGuard';

function key(): string {
  return Keypair.generate().publicKey.toBase58();
}

function acceptedEvent(
  sequence: string,
  overrides: Partial<RelayIntentAcceptedEvent> = {},
): RelayIntentAcceptedEvent {
  const group = overrides.group || key();
  const executionQueue = overrides.execution_queue || key();
  const market = overrides.market || '0';
  const owner = overrides.user_owner || key();
  const mangoAccount = overrides.mango_account || key();
  return {
    event_type: 'relay_intent_accepted',
    ts_ms: Number(sequence),
    group,
    execution_queue: executionQueue,
    market,
    sequence,
    kind: 0,
    payload_b64: Buffer.from(`payload-${sequence}`).toString('base64'),
    remaining_accounts:
      overrides.remaining_accounts || [
        {
          pubkey: group,
          is_signer: false,
          is_writable: false,
        },
        {
          pubkey: mangoAccount,
          is_signer: false,
          is_writable: true,
        },
      ],
    min_execute_slot: '0',
    expires_at_slot: '0',
    user_owner: owner,
    mango_account: mangoAccount,
    enqueue_tx_signature: `enqueue-${sequence}`,
    ...overrides,
  };
}

describe('Continuum Harness Lane Guard', () => {
  it('computes a stable lane hash after merging fixed-account flags', () => {
    const group = key();
    const executionQueue = key();
    const shared = key();

    const hashA = computeAcceptedIntentLaneHash(
      acceptedEvent('1', {
        group,
        execution_queue: executionQueue,
        remaining_accounts: [
          {
            pubkey: shared,
            is_signer: false,
            is_writable: false,
          },
          {
            pubkey: shared,
            is_signer: false,
            is_writable: true,
          },
        ],
      }),
    );
    const hashB = computeAcceptedIntentLaneHash(
      acceptedEvent('2', {
        group,
        execution_queue: executionQueue,
        remaining_accounts: [
          {
            pubkey: shared,
            is_signer: false,
            is_writable: true,
          },
          {
            pubkey: shared,
            is_signer: false,
            is_writable: false,
          },
        ],
      }),
    );

    expect(hashA).eq(hashB);
  });

  it('suppresses the second unresolved intent on a lane and marks the market suppressed', () => {
    const guard = new ContinuumHarnessLaneGuard({
      maxOptimisticPendingPerLane: 1,
      marketPendingSoftLimit: 2,
      failureThreshold: 2,
      suppressionMs: 5_000,
      pendingStaleMs: 30_000,
    });

    const base = acceptedEvent('1');
    const first = guard.observeAccepted(base, 1_000);
    const second = guard.observeAccepted(
      acceptedEvent('2', {
        group: base.group,
        execution_queue: base.execution_queue,
        market: base.market,
        user_owner: base.user_owner,
        mango_account: base.mango_account,
        remaining_accounts: base.remaining_accounts,
      }),
      1_001,
    );

    expect(first.shouldSuppressOptimisticReplay).eq(false);
    expect(second.shouldSuppressOptimisticReplay).eq(true);
    expect(second.suppressionReason).eq('lane_pending_limit');

    const suppressedMarkets = guard.getSuppressedMarkets(1_001);
    expect(suppressedMarkets.get(base.market)?.pendingCount).eq(2);

    guard.observeQueueItemProcessed(
      {
        event_type: 'queue_item_processed',
        ts_ms: 2_000,
        group: base.group,
        sequence: '1',
        kind: 0,
        status: QueueProcessStatus.Executed,
        slot: '10',
        tx_signature: 'processed-1',
      },
      2_000,
    );

    expect(guard.getSuppressedMarkets(2_000).size).eq(0);
  });

  it('activates a timed backoff after repeated failures on the same lane', () => {
    const guard = new ContinuumHarnessLaneGuard({
      maxOptimisticPendingPerLane: 1,
      marketPendingSoftLimit: 2,
      failureThreshold: 2,
      suppressionMs: 1_000,
      pendingStaleMs: 30_000,
    });

    const base = acceptedEvent('1');
    guard.observeAccepted(base, 1_000);
    guard.observeQueueItemProcessed(
      {
        event_type: 'queue_item_processed',
        ts_ms: 1_100,
        group: base.group,
        sequence: '1',
        kind: 0,
        status: QueueProcessStatus.Failed,
        slot: '11',
        tx_signature: 'failed-1',
      },
      1_100,
    );

    guard.observeAccepted(
      acceptedEvent('2', {
        group: base.group,
        execution_queue: base.execution_queue,
        market: base.market,
        user_owner: base.user_owner,
        mango_account: base.mango_account,
        remaining_accounts: base.remaining_accounts,
      }),
      1_200,
    );
    guard.observeQueueItemProcessed(
      {
        event_type: 'queue_item_processed',
        ts_ms: 1_300,
        group: base.group,
        sequence: '2',
        kind: 0,
        status: QueueProcessStatus.Failed,
        slot: '12',
        tx_signature: 'failed-2',
      },
      1_300,
    );

    const third = guard.observeAccepted(
      acceptedEvent('3', {
        group: base.group,
        execution_queue: base.execution_queue,
        market: base.market,
        user_owner: base.user_owner,
        mango_account: base.mango_account,
        remaining_accounts: base.remaining_accounts,
      }),
      1_301,
    );

    expect(third.shouldSuppressOptimisticReplay).eq(true);
    expect(third.suppressionReason).eq('failure_backoff');
    expect(guard.getSuppressedMarkets(1_301).get(base.market)).to.not.eq(undefined);
    expect(guard.getSuppressedMarkets(2_500).size).eq(0);
  });
});
