# Relayer Resilience — v4 Commit / Reveal

Status: **not implemented**. This document captures the risk model and the
options we've considered so we can make a deliberate choice before turning on
high-volume v4 traffic.

## Risk model

With v4, the relayer is the *only* party that can turn a committed slot back
into an executable intent. The sequence of events is:

1. User submits an intent to the relayer (gRPC).
2. Relayer assigns sequence N, computes `commit_hash`, batches and lands a
   `commit_market` tx. Slot N on chain is now **committed** but the payload is
   only held in the relayer's process memory.
3. Relayer later submits `reveal_execute` with the full payload and
   dispatch accounts. Slot N becomes **executed**.

The failure cases we have to reason about:

| Failure | Consequence if unmitigated |
|---|---|
| Relayer process crash after commit, before reveal | Slot N cannot be revealed — it will sit at the queue head until `expires_at_slot`, blocking everything behind it. After expiry, the on-chain gap-recovery path skips it, but the user's intent is dropped without execution. |
| Relayer disk loss (memory-only WAL) | All in-flight intents between last durable checkpoint and crash become unrevealable, even with a warm spare. |
| Relayer host unreachable (network partition) | Same as crash, but recovery is automatic if the host returns before expiry. |
| Active relayer buggy (crashes mid-batch) | Partial batch state is ambiguous; see "Commit idempotency" below. |

The user's stated scope trusts the relayer for ingress/sequencing, so
Byzantine-relayer attacks are out of scope. The failures above are all
liveness, not safety.

## Write-ahead log (minimum viable resilience)

Before the gRPC handler acknowledges an intent (or at the latest, before the
commit tx is signed), the relayer must durably persist:

```
(sequence, market_index, full_payload, envelope_metadata, user_sig_if_any,
 accounts_hash_precursor, dispatch_accounts)
```

On restart, the reveal worker replays the WAL for every committed sequence
`>= queue_root.next_sequence_to_execute` and resumes submitting reveals.

Design points:

* **Write amplification**: one fsync per intent hurts throughput. An
  append-only log batched with `fsync` at the same cadence as the commit
  batcher (every 50 intents or 1 s) is a good match. If the process crashes
  mid-batch, we may lose up to `fsync_window` intents — but those intents
  were also not committed on chain, so no on-chain slot is stranded.
* **Ordering**: WAL entries must be written *before* the commit tx is sent.
  Otherwise the on-chain slot references a payload we never persisted.
* **Storage**: SQLite on a local SSD is enough for the per-market throughput
  this system targets. Rotate segments keyed by `next_sequence_to_execute`
  advancement from the chain.

## Replication options

### Option A — Active / cold standby

* Primary relayer is the only one that signs commits.
* Standby continuously tails the primary's WAL over a network socket (or
  shared block storage).
* On primary failure: standby promotes itself, starts submitting reveals
  for any `committed` slot it has in its copy of the WAL.
* **Failover window**: anything committed since the last WAL shipment is
  lost. Minimize by streaming WAL records synchronously (primary waits for
  standby ack before signing the commit tx).
* **Split-brain risk**: if both nodes think they're primary and both sign
  commits, the chain will reject the second commit (sequence conflict), so
  the chain itself arbitrates. No extra work needed beyond ensuring only
  one node actively ingests from clients.

### Option B — Consensus-backed WAL (Raft / etcd / FoundationDB)

* All relayer replicas are ingress-capable, all write to a shared replicated
  log before committing on chain.
* One replica is the elected "proposer" that actually signs and submits the
  commit tx.
* On proposer failure, another replica takes over with identical state.
* **Cost**: every intent takes at least one consensus round-trip in
  addition to the Solana round-trip. If ingress p99 is already a few ms,
  this can double the latency.
* **Complexity**: operating a stateful consensus cluster is a full separate
  ops surface.

### Option C — Stateless relayer + chain-durable commit payloads

An escape valve if neither A nor B is acceptable: put the full payload on
chain inside the commit, accepting the loss of privacy for that sequence.
Treat this as a degraded-mode fallback for a relayer outage — it is
identical to v3 enqueue.

## Commit idempotency

The on-chain `commit_market` rejects duplicate writes into a committed slot
(see `CommitPageV4::write_pending_item`), so replaying a partially-landed
batch is safe: successful entries will fail with `ExecutionQueueFull` and
the rest will land. Ensure the reveal worker treats "no such commit on
chain" as a transient state (the tx may still be confirming) and retries
rather than dropping the payload.

## Recommendation

Ship A (active / cold standby with synchronous WAL shipping) once v4 goes
to more than one market. The latency cost is minimal when the standby is
co-located, and it covers the two most realistic failure modes (process
crash, host loss) without the operational burden of a consensus cluster.

Revisit B only if we need geographic failover or multi-ingress for scale.
