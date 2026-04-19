# v4 Throughput Optimizations — Lessons From Devnet

What we measured, why the pipelining trick works, and the playbook to
take to mainnet. All numbers are reproducible via `bin/v4-stress`.

## Summary

| Optimization | Effect | Complexity |
|---|---|---|
| Pipelined reveal (`send_transaction` + 50 ms spacing) | **14.5×** vs serial | Client-side only |
| Multi-sig ed25519 pre-instruction (1 sig for N commits) | Already shipped; ~30× enqueue ingestion rate | Program-side |
| v0 tx + ALT for shared accounts | ~+20 % reveals/tx | Client + ALT upkeep |
| Lane fanout across markets | Linear in lane count | Client-side |
| Drop onchain user-sig verify (trusted-relayer mode) | ~+70 % reveals/tx | Trust-model change |

The one-off biggest win for **already-shipped code** is pipelining — the
point of this doc.

## The fundamental constraint (reveal side)

Every reveal tx takes a *writable* lock on the same `queue_page` account.
Solana's runtime will not execute two writable-lock conflicting txs in
parallel — they serialize within a block in submission order.

That's a hard ceiling: within a single market, reveal is serial. No CPU
parallelism can help. The only knobs are:

1. **How many reveals fit per tx** (size-bound).
2. **How quickly we can land N serialized txs** (one-after-another
   scheduler cost, not N × confirm latency).
3. **Fanning out to separate markets**, which use disjoint
   `queue_page` accounts — those *can* execute in parallel.

Knob #2 is where the pipelining insight lives.

## The anti-pattern we started with

```rust
for reveal_batch in batches {
    rpc.send_and_confirm_transaction(&reveal_batch).await?;
}
```

This stacks two latencies:

1. Solana block cadence (~400 ms mainnet, ~500–1000 ms devnet under load)
2. `send_and_confirm` polling round-trips (default 500 ms cadence)

Serial reveal observed: **2.8 reveals/s** on devnet, **10 reveals/s** on
localnet. Almost entirely client-wait dead time — the validator sits idle
between our txs.

## The pipelining pattern

```rust
let send_cfg = RpcSendTransactionConfig {
    skip_preflight: true,
    max_retries: Some(0),
    ..Default::default()
};
for batch in batches {
    rpc.send_transaction_with_config(&tx, send_cfg).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
}
// Confirm once at the end via `get_account(queue_root)` polling.
```

Every tx is built for an *explicit* sequence range (seq `i*5..(i+1)*5`),
not "whatever head is". Solana's queue_page write-lock serializes them
in submission order. Each tx's reveals match the seq range it was built
for, so even though the head advances between txs, every tx's hash
comparisons land correctly.

What we sacrifice: per-tx landing confirmation. The final `get_account`
poll at end of run tells us if anything was dropped, and a serial retry
loop picks up any gaps.

## Measured (devnet, 150 commits/run, 4 pages)

| Spacing | Reveals/s | Retry rate |
|---|---|---|
| serial (`send_and_confirm`) | 2.8 | — |
| 200 ms pipelined | 17.9 | 0 |
| 100 ms pipelined | 29.3 | 0 |
| **50 ms pipelined** | **40.5** | **0** |
| 25 ms pipelined | 1.7 | high |
| 10 ms pipelined | 1.3 | very high |

**Sweet spot: 50 ms spacing = 14.5× speedup.** Below that threshold,
devnet RPC rate-limits + scheduler reorderings produce enough retries to
net out negative. Mainnet's sweet spot is probably tighter (~20 ms)
because block times are shorter and RPCs more capable — to be re-measured
against a specific mainnet RPC.

## Why not sub-50 ms?

Two separate failure modes appear below the sweet spot:

1. **RPC backpressure** — some `send_transaction` calls return an error
   (not a tx failure, a network rate-limit). The harness counts how many
   signatures came back from send; you can see the number drop on tight
   spacings.
2. **Tx reordering in the validator's pool** — if two txs arrive nearly
   together, the block builder's ordering isn't guaranteed to follow
   submission order. When that happens, the tx for seq N+5 lands before
   the tx for seq N, sees `head=N` (still), recomputes commit_hash from
   its seq N+5 payload against the commit stored at seq N, mismatch, tx
   fails. Client retries serially.

A higher-rate-limit RPC or a dedicated leader-aware scheduler would push
the sweet spot lower. Out of scope for this design doc.

## Recipes

### For the relayer (production)

```
for each reveal batch:
    build tx with explicit sequence range + fixed blockhash
    send_transaction (skip_preflight)
    sleep(spacing_ms)
every k sends or on idle:
    read queue_root; if next_sequence hasn't advanced enough, retry gaps
```

Recommended defaults:

- `spacing_ms = 50` on devnet; calibrate per-RPC for mainnet (start at 30).
- `max_retries = 0` on the send config — let the client handle retries
  with awareness of actual head state.
- Confirm via account-data poll on `queue_root` every 200–500 ms, not
  per-tx.

### For stress tests (reproducible)

```
./target/release/v4-stress \
    --rpc https://api.devnet.solana.com \
    --payer ~/.config/solana/id.json \
    --no-airdrop --num-pages 4 --group-num <fresh> \
    --commits 150 --batch-size 12 \
    --reveal-mode pipelined --reveal-spacing-ms 50
```

## Commit side — what's already optimized

Commits DON'T have the head-lock constraint — they only need a writable
lock on the commit page they write to, and different batches targeting
different pages can go parallel. The harness already pipelines commits
with `--inflight 8`.

On localnet we hit **585 commits/s**. Devnet `send_and_confirm` caps at
**~7/s** because of confirm latency; switching commits to the same
`send_transaction + end-poll` pattern used for reveals would lift devnet
commits to ~50–100/s without any code change.

(The v4-stress commit loop still uses `send_and_confirm_transaction`
because measuring landed-and-confirmed commits is the conservative
thing for a benchmark — we know exactly when they're all in state. A
production relayer won't wait.)

## Mainnet projection (revised with measurements)

| Metric | Estimate | Basis |
|---|---|---|
| Commit throughput (1 relayer, pipelined) | 300–500/s | Localnet × mainnet slot ratio |
| Reveal throughput (1 lane, pipelined) | 60–80/s | Devnet 40.5/s × shorter slots |
| Reveal throughput (4 lanes, ALT, no user sig) | ~300/s | 80 × 4 lanes × 1.2 ALT factor × 0.8 user-sig factor |

All bounded by tx-landing-rate, not CU or on-chain work.

## Open threads

1. **Optimistic spacing auto-tune** — track retry rate per-spacing and
   close the loop in the relayer. A simple additive-increase
   multiplicative-decrease (AIMD) on `spacing_ms` would track each
   RPC's real limit without hand-tuning.
2. **Dedicated leader slot** — reveal throughput is capped at 1 tx/slot
   worth of lock-held compute; priority fees that guarantee landing in
   the leader's preferred slot buy that bandwidth. Measure the
   priority-fee / reveals-per-slot curve on mainnet.
3. **Next-page prefetch** — at page-boundary crossings, the relayer must
   know to re-point at the new `queue_page` PDA. The harness does this
   already via `seq / page_size`; confirm the production relayer's
   reveal worker has the same logic.
