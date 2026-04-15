import { createHash } from 'crypto';
import { PublicKey } from '@solana/web3.js';
import {
  QueueProcessStatus,
  RelayIntentAcceptedEvent,
  RelayIntentStatusCode,
  RelayIntentStatusEvent,
  QueueItemProcessedEvent,
} from './continuumHarnessTsEngine';

const EXECUTION_QUEUE_SYSVAR_INSTRUCTIONS =
  'Sysvar1nstructions1111111111111111111111111';

export type LaneGuardOptions = {
  maxOptimisticPendingPerLane: number;
  marketPendingSoftLimit: number;
  failureThreshold: number;
  suppressionMs: number;
  pendingStaleMs: number;
};

export type LaneGuardSuppressionReason =
  | 'duplicate_payload_pending'
  | 'failure_backoff'
  | 'lane_pending_limit';

export type LaneGuardDecision = {
  trackingKey: string;
  laneHash: string;
  payloadHash: string;
  market: string;
  pendingCount: number;
  shouldSuppressOptimisticReplay: boolean;
  suppressionReason: LaneGuardSuppressionReason | null;
  marketSuppressed: boolean;
  marketSuppressionActivated: boolean;
};

export type LaneGuardTrackedIntent = {
  trackingKey: string;
  laneHash: string;
  payloadHash: string;
  group: string;
  sequence: string;
  kind: number;
  market: string;
  owner: string;
  mangoAccount: string;
  acceptedTsMs: number;
  optimisticSuppressed: boolean;
  terminalTsMs: number | null;
};

export type LaneGuardMarketSuppression = {
  market: string;
  laneCount: number;
  pendingCount: number;
  pendingSequences: string[];
  reasons: LaneGuardSuppressionReason[];
  suppressUntilTsMs: number | null;
};

type LaneState = {
  laneHash: string;
  market: string;
  owner: string;
  mangoAccount: string;
  pendingTrackingKeys: Set<string>;
  pendingSequences: Set<string>;
  pendingPayloadCounts: Map<string, number>;
  suppressUntilTsMs: number;
  consecutiveFailures: number;
  lastFailureReason: string | null;
  lastFailureTsMs: number | null;
  lastAcceptedTsMs: number;
  totalSuppressed: number;
};

export type LaneGuardStats = {
  activeLanes: number;
  trackedIntents: number;
  pendingIntents: number;
  suppressedMarkets: number;
  totalSuppressedIntents: number;
};

export function relayIntentTrackingKey(
  event: Pick<RelayIntentAcceptedEvent, 'group' | 'sequence' | 'kind'>,
): string {
  return `${event.group}:${event.sequence}:${event.kind}`;
}

export function computeAcceptedIntentLaneHash(
  event: Pick<
    RelayIntentAcceptedEvent,
    'group' | 'execution_queue' | 'remaining_accounts'
  >,
): string {
  const merged = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  for (const account of [
    {
      pubkey: event.group,
      is_signer: false,
      is_writable: true,
    },
    {
      pubkey: event.execution_queue,
      is_signer: false,
      is_writable: true,
    },
    {
      pubkey: EXECUTION_QUEUE_SYSVAR_INSTRUCTIONS,
      is_signer: false,
      is_writable: false,
    },
    ...(event.remaining_accounts || []),
  ]) {
    const existing = merged.get(account.pubkey);
    if (existing) {
      existing.isSigner = existing.isSigner || !!account.is_signer;
      existing.isWritable = existing.isWritable || !!account.is_writable;
    } else {
      merged.set(account.pubkey, {
        isSigner: !!account.is_signer,
        isWritable: !!account.is_writable,
      });
    }
  }

  const bytes = Buffer.concat(
    (event.remaining_accounts || []).map((account) => {
      const flags = merged.get(account.pubkey) || {
        isSigner: !!account.is_signer,
        isWritable: !!account.is_writable,
      };
      return Buffer.concat([
        new PublicKey(account.pubkey).toBuffer(),
        Buffer.from([flags.isSigner ? 1 : 0, flags.isWritable ? 1 : 0]),
      ]);
    }),
  );

  return createHash('sha256').update(bytes).digest('hex');
}

export function computeAcceptedIntentPayloadHash(
  event: Pick<RelayIntentAcceptedEvent, 'payload_b64'>,
): string {
  return createHash('sha256')
    .update(Buffer.from(event.payload_b64, 'base64'))
    .digest('hex');
}

export class ContinuumHarnessLaneGuard {
  private readonly opts: LaneGuardOptions;

  private readonly trackedIntents = new Map<string, LaneGuardTrackedIntent>();

  private readonly lanes = new Map<string, LaneState>();

  constructor(options: LaneGuardOptions) {
    this.opts = {
      maxOptimisticPendingPerLane: Math.max(
        0,
        options.maxOptimisticPendingPerLane,
      ),
      marketPendingSoftLimit: Math.max(1, options.marketPendingSoftLimit),
      failureThreshold: Math.max(1, options.failureThreshold),
      suppressionMs: Math.max(1, options.suppressionMs),
      pendingStaleMs: Math.max(1_000, options.pendingStaleMs),
    };
  }

  observeAccepted(
    event: RelayIntentAcceptedEvent,
    nowMs = Date.now(),
  ): LaneGuardDecision {
    this.cleanup(nowMs);

    const trackingKey = relayIntentTrackingKey(event);
    const existing = this.trackedIntents.get(trackingKey);
    if (existing) {
      const lane = this.lanes.get(existing.laneHash);
      return {
        trackingKey,
        laneHash: existing.laneHash,
        payloadHash: existing.payloadHash,
        market: existing.market,
        pendingCount: lane?.pendingTrackingKeys.size || 0,
        shouldSuppressOptimisticReplay: existing.optimisticSuppressed,
        suppressionReason: existing.optimisticSuppressed
          ? this.activeSuppressionReason(lane, existing.payloadHash, nowMs)
          : null,
        marketSuppressed: this.isMarketSuppressed(lane, nowMs),
        marketSuppressionActivated: false,
      };
    }

    const laneHash = computeAcceptedIntentLaneHash(event);
    const payloadHash = computeAcceptedIntentPayloadHash(event);
    const lane = this.ensureLane(laneHash, event, nowMs);
    const marketSuppressedBefore = this.isMarketSuppressed(lane, nowMs);
    const duplicatePayloadPending =
      (lane.pendingPayloadCounts.get(payloadHash) || 0) > 0;

    let suppressionReason: LaneGuardSuppressionReason | null = null;
    if (lane.suppressUntilTsMs > nowMs) {
      suppressionReason = 'failure_backoff';
    } else if (duplicatePayloadPending) {
      suppressionReason = 'duplicate_payload_pending';
    } else if (
      lane.pendingTrackingKeys.size >= this.opts.maxOptimisticPendingPerLane
    ) {
      suppressionReason = 'lane_pending_limit';
    }

    lane.pendingTrackingKeys.add(trackingKey);
    lane.pendingSequences.add(event.sequence);
    lane.pendingPayloadCounts.set(
      payloadHash,
      (lane.pendingPayloadCounts.get(payloadHash) || 0) + 1,
    );
    lane.lastAcceptedTsMs = nowMs;
    if (suppressionReason) {
      lane.totalSuppressed += 1;
    }

    this.trackedIntents.set(trackingKey, {
      trackingKey,
      laneHash,
      payloadHash,
      group: event.group,
      sequence: event.sequence,
      kind: event.kind,
      market: event.market,
      owner: event.user_owner,
      mangoAccount: event.mango_account,
      acceptedTsMs: nowMs,
      optimisticSuppressed: suppressionReason !== null,
      terminalTsMs: null,
    });

    const marketSuppressedAfter = this.isMarketSuppressed(lane, nowMs);
    return {
      trackingKey,
      laneHash,
      payloadHash,
      market: event.market,
      pendingCount: lane.pendingTrackingKeys.size,
      shouldSuppressOptimisticReplay: suppressionReason !== null,
      suppressionReason,
      marketSuppressed: marketSuppressedAfter,
      marketSuppressionActivated:
        !marketSuppressedBefore && marketSuppressedAfter,
    };
  }

  observeRelayIntentStatus(
    event: RelayIntentStatusEvent,
    nowMs = Date.now(),
  ): LaneGuardDecision | null {
    this.cleanup(nowMs);
    if (!event.group || event.sequence === null || event.kind === null) {
      return null;
    }
    const trackingKey = relayIntentTrackingKey({
      group: event.group,
      sequence: event.sequence,
      kind: event.kind,
    });
    const tracked = this.trackedIntents.get(trackingKey);
    if (!tracked) {
      return null;
    }
    const lane = this.lanes.get(tracked.laneHash);
    if (!lane) {
      return null;
    }

    const marketSuppressedBefore = this.isMarketSuppressed(lane, nowMs);
    if (
      event.status_code === RelayIntentStatusCode.Rejected ||
      event.queue_process_status === QueueProcessStatus.Failed
    ) {
      this.recordFailure(
        lane,
        event.reason ||
          event.queue_process_status_name ||
          event.status_label ||
          'relay_intent_status_failure',
        nowMs,
      );
    }

    const marketSuppressedAfter = this.isMarketSuppressed(lane, nowMs);
    return {
      trackingKey,
      laneHash: tracked.laneHash,
      payloadHash: tracked.payloadHash,
      market: tracked.market,
      pendingCount: lane.pendingTrackingKeys.size,
      shouldSuppressOptimisticReplay: tracked.optimisticSuppressed,
      suppressionReason: this.activeSuppressionReason(
        lane,
        tracked.payloadHash,
        nowMs,
      ),
      marketSuppressed: marketSuppressedAfter,
      marketSuppressionActivated:
        !marketSuppressedBefore && marketSuppressedAfter,
    };
  }

  observeQueueItemProcessed(
    event: QueueItemProcessedEvent,
    nowMs = Date.now(),
  ): LaneGuardDecision | null {
    this.cleanup(nowMs);
    const trackingKey = `${event.group}:${event.sequence}:${event.kind}`;
    const tracked = this.trackedIntents.get(trackingKey);
    if (!tracked) {
      return null;
    }
    const lane = this.lanes.get(tracked.laneHash);
    if (!lane) {
      this.trackedIntents.delete(trackingKey);
      return null;
    }

    const marketSuppressedBefore = this.isMarketSuppressed(lane, nowMs);
    this.removePendingTracking(lane, tracked, event.sequence);
    tracked.terminalTsMs = nowMs;

    if (event.status === QueueProcessStatus.Failed) {
      this.recordFailure(lane, 'queue_item_failed', nowMs);
    } else if (
      event.status === QueueProcessStatus.Executed ||
      event.status === QueueProcessStatus.Skipped
    ) {
      lane.consecutiveFailures = 0;
      if (lane.pendingTrackingKeys.size === 0 && lane.suppressUntilTsMs <= nowMs) {
        lane.lastFailureReason = null;
        lane.lastFailureTsMs = null;
      }
    }

    const marketSuppressedAfter = this.isMarketSuppressed(lane, nowMs);
    this.maybeDeleteLane(tracked.laneHash, nowMs);

    return {
      trackingKey,
      laneHash: tracked.laneHash,
      payloadHash: tracked.payloadHash,
      market: tracked.market,
      pendingCount: lane.pendingTrackingKeys.size,
      shouldSuppressOptimisticReplay: false,
      suppressionReason: this.activeSuppressionReason(
        lane,
        tracked.payloadHash,
        nowMs,
      ),
      marketSuppressed: marketSuppressedAfter,
      marketSuppressionActivated:
        !marketSuppressedBefore && marketSuppressedAfter,
    };
  }

  findTrackedIntent(
    group: string,
    sequence: string,
    kind: number,
  ): LaneGuardTrackedIntent | null {
    return this.trackedIntents.get(`${group}:${sequence}:${kind}`) || null;
  }

  /**
   * Sweep pending intents whose sequence is below the on-chain queue's
   * next-to-execute watermark and that are no longer in the on-chain
   * pending set. Called after each periodic queue-state refresh so the
   * lane guard releases pending intents that have been crankered on-chain
   * without waiting for the 8 s stale timeout or a code-3 event.
   *
   * Returns the swept intents so the caller can emit synthetic
   * QueueItemProcessed events to the engine (advancing confirmed_seq).
   */
  sweepProcessedBelow(
    nextOnchainSequence: bigint,
    pendingOnchainSequenceKinds: Set<string>,
    nowMs = Date.now(),
  ): Array<
    Pick<
      LaneGuardTrackedIntent,
      'group' | 'sequence' | 'kind' | 'market' | 'owner' | 'mangoAccount'
    >
  > {
    if (nextOnchainSequence === 0n) {
      return [];
    }
    const swept: Array<
      Pick<
        LaneGuardTrackedIntent,
        'group' | 'sequence' | 'kind' | 'market' | 'owner' | 'mangoAccount'
      >
    > = [];
    for (const tracked of this.trackedIntents.values()) {
      if (tracked.terminalTsMs !== null) {
        continue; // already resolved
      }
      if (BigInt(tracked.sequence) >= nextOnchainSequence) {
        continue; // still possibly pending on-chain
      }
      if (pendingOnchainSequenceKinds.has(`${tracked.sequence}:${tracked.kind}`)) {
        continue; // still in on-chain pending queue
      }
      // Intent is below the on-chain watermark and not pending — processed.
      const lane = this.lanes.get(tracked.laneHash);
      if (lane) {
        this.removePendingTracking(lane, tracked, tracked.sequence);
        lane.consecutiveFailures = 0;
        if (lane.pendingTrackingKeys.size === 0 && lane.suppressUntilTsMs <= nowMs) {
          lane.lastFailureReason = null;
          lane.lastFailureTsMs = null;
        }
      }
      tracked.terminalTsMs = nowMs;
      swept.push({
        group: tracked.group,
        sequence: tracked.sequence,
        kind: tracked.kind,
        market: tracked.market,
        owner: tracked.owner,
        mangoAccount: tracked.mangoAccount,
      });
    }
    return swept;
  }

  getSuppressedMarkets(nowMs = Date.now()): Map<string, LaneGuardMarketSuppression> {
    this.cleanup(nowMs);
    const byMarket = new Map<string, LaneGuardMarketSuppression>();
    for (const lane of this.lanes.values()) {
      if (!this.isMarketSuppressed(lane, nowMs)) {
        continue;
      }
      const current = byMarket.get(lane.market) || {
        market: lane.market,
        laneCount: 0,
        pendingCount: 0,
        pendingSequences: [],
        reasons: [],
        suppressUntilTsMs: null,
      };
      current.laneCount += 1;
      current.pendingCount += lane.pendingTrackingKeys.size;
      current.pendingSequences.push(...Array.from(lane.pendingSequences));
      for (const reason of this.activeReasons(lane, nowMs)) {
        if (!current.reasons.includes(reason)) {
          current.reasons.push(reason);
        }
      }
      if (lane.suppressUntilTsMs > nowMs) {
        current.suppressUntilTsMs = Math.max(
          current.suppressUntilTsMs || 0,
          lane.suppressUntilTsMs,
        );
      }
      byMarket.set(lane.market, current);
    }

    for (const value of byMarket.values()) {
      value.pendingSequences.sort(compareNumericStrings);
    }
    return byMarket;
  }

  getStats(nowMs = Date.now()): LaneGuardStats {
    this.cleanup(nowMs);
    let pendingIntents = 0;
    let totalSuppressedIntents = 0;
    for (const lane of this.lanes.values()) {
      pendingIntents += lane.pendingTrackingKeys.size;
      totalSuppressedIntents += lane.totalSuppressed;
    }
    return {
      activeLanes: this.lanes.size,
      trackedIntents: this.trackedIntents.size,
      pendingIntents,
      suppressedMarkets: this.getSuppressedMarkets(nowMs).size,
      totalSuppressedIntents,
    };
  }

  private ensureLane(
    laneHash: string,
    event: RelayIntentAcceptedEvent,
    nowMs: number,
  ): LaneState {
    const existing = this.lanes.get(laneHash);
    if (existing) {
      existing.market = event.market;
      existing.owner = event.user_owner;
      existing.mangoAccount = event.mango_account;
      existing.lastAcceptedTsMs = nowMs;
      return existing;
    }
    const created: LaneState = {
      laneHash,
      market: event.market,
      owner: event.user_owner,
      mangoAccount: event.mango_account,
      pendingTrackingKeys: new Set<string>(),
      pendingSequences: new Set<string>(),
      pendingPayloadCounts: new Map<string, number>(),
      suppressUntilTsMs: 0,
      consecutiveFailures: 0,
      lastFailureReason: null,
      lastFailureTsMs: null,
      lastAcceptedTsMs: nowMs,
      totalSuppressed: 0,
    };
    this.lanes.set(laneHash, created);
    return created;
  }

  private removePendingTracking(
    lane: LaneState,
    tracked: LaneGuardTrackedIntent,
    sequence: string,
  ): void {
    lane.pendingTrackingKeys.delete(tracked.trackingKey);
    lane.pendingSequences.delete(sequence);
    const payloadCount = lane.pendingPayloadCounts.get(tracked.payloadHash) || 0;
    if (payloadCount <= 1) {
      lane.pendingPayloadCounts.delete(tracked.payloadHash);
    } else {
      lane.pendingPayloadCounts.set(tracked.payloadHash, payloadCount - 1);
    }
  }

  private recordFailure(
    lane: LaneState,
    reason: string,
    nowMs: number,
  ): void {
    lane.consecutiveFailures += 1;
    lane.lastFailureReason = reason;
    lane.lastFailureTsMs = nowMs;
    if (lane.consecutiveFailures >= this.opts.failureThreshold) {
      lane.suppressUntilTsMs = Math.max(
        lane.suppressUntilTsMs,
        nowMs + this.opts.suppressionMs,
      );
    }
  }

  private activeReasons(
    lane: LaneState,
    nowMs: number,
  ): LaneGuardSuppressionReason[] {
    const reasons: LaneGuardSuppressionReason[] = [];
    if (lane.suppressUntilTsMs > nowMs) {
      reasons.push('failure_backoff');
    }
    if (lane.pendingTrackingKeys.size >= this.opts.marketPendingSoftLimit) {
      reasons.push('lane_pending_limit');
    }
    return reasons;
  }

  private activeSuppressionReason(
    lane: LaneState | undefined,
    payloadHash: string,
    nowMs: number,
  ): LaneGuardSuppressionReason | null {
    if (!lane) {
      return null;
    }
    if (lane.suppressUntilTsMs > nowMs) {
      return 'failure_backoff';
    }
    if ((lane.pendingPayloadCounts.get(payloadHash) || 0) > 1) {
      return 'duplicate_payload_pending';
    }
    if (lane.pendingTrackingKeys.size >= this.opts.marketPendingSoftLimit) {
      return 'lane_pending_limit';
    }
    return null;
  }

  private isMarketSuppressed(
    lane: LaneState | undefined,
    nowMs: number,
  ): boolean {
    if (!lane) {
      return false;
    }
    return (
      lane.suppressUntilTsMs > nowMs ||
      lane.pendingTrackingKeys.size >= this.opts.marketPendingSoftLimit
    );
  }

  private cleanup(nowMs: number): void {
    for (const [trackingKey, tracked] of Array.from(this.trackedIntents.entries())) {
      if (
        tracked.terminalTsMs !== null &&
        nowMs - tracked.terminalTsMs <= this.opts.pendingStaleMs
      ) {
        continue;
      }
      if (nowMs - tracked.acceptedTsMs <= this.opts.pendingStaleMs) {
        continue;
      }
      const lane = this.lanes.get(tracked.laneHash);
      if (lane) {
        this.removePendingTracking(lane, tracked, tracked.sequence);
      }
      this.trackedIntents.delete(trackingKey);
    }
    for (const laneHash of Array.from(this.lanes.keys())) {
      this.maybeDeleteLane(laneHash, nowMs);
    }
  }

  private maybeDeleteLane(laneHash: string, nowMs: number): void {
    const lane = this.lanes.get(laneHash);
    if (!lane) {
      return;
    }
    if (lane.pendingTrackingKeys.size > 0) {
      return;
    }
    if (lane.suppressUntilTsMs > nowMs) {
      return;
    }
    if (lane.lastFailureTsMs && nowMs - lane.lastFailureTsMs <= this.opts.pendingStaleMs) {
      return;
    }
    if (nowMs - lane.lastAcceptedTsMs <= this.opts.pendingStaleMs) {
      return;
    }
    this.lanes.delete(laneHash);
  }
}

function compareNumericStrings(a: string, b: string): number {
  const left = BigInt(a);
  const right = BigInt(b);
  if (left === right) {
    return 0;
  }
  return left < right ? -1 : 1;
}
