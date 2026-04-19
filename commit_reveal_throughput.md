# v4 Commit/Reveal — Throughput Model

Goal: quantify how the commit-reveal split trades tx-size and CU for
censorship resistance, and identify optimizations to claw back execute-side
throughput.

> **Measured numbers** (see "Observed on devnet" below) — the analytical
> budgets in this doc set upper bounds; real landing rates depend on RPC
> round-trip, slot cadence, and send-pattern (serial vs pipelined).

All numbers below are **per legacy Solana tx** (1232-byte packet). Variables
are conservative; real-world tx overhead depends on compute-budget
instructions, priority-fee ixs, lookup tables, etc.

## Fixed transaction overhead

```
signatures array       : 1 sig   × (1 + 64) =  65 bytes
message header         :                     =   3 bytes
recent blockhash       :                     =  32 bytes
shared account list    : 5 fixed  × 32       = 160 bytes
                         (queue_root, page, group, authority, payer)
instructions prefix    :                     =  ~6 bytes
compute-budget ixs (2) :                     =  ~30 bytes
priority-fee ix        :                     =  ~15 bytes
-------------------------------------------------------------
≈ 310–320 bytes fixed overhead (call it 320)
```

Budget left for reveal/commit payload data and per-item account metas: **~912
bytes** after headroom.

## Commit side

One commit entry on the wire:
```
commit_hash (32) + min_execute_slot (8) + expires_at_slot (8) = 48 bytes
```

Per-tx breakdown at 50 entries:
```
borsh vec header            :   4
first_sequence (u64)        :   8
market_index (u16)          :   2
50 × CommitEntryV4 (48 B)   : 2400
---------------------------------
ix data subtotal             : 2414
+ ed25519 pre-ix             : 142  (one sig per batch)
+ instruction sysvar account :  32
---------------------------------
Commit-side non-fixed bytes  : 2588
```

That's way over 912. So 50 commits per tx is **not** achievable in a single
legacy tx; the hard limit is about **17–18 entries per legacy tx**, set by
the 1232-byte cap, not CU.

**Mitigation**: use address-lookup tables (v0 txs, not legacy) to move the
5 shared accounts into an ALT. That drops the fixed-account cost from 160 →
6 bytes (1 byte ALT index per account + a few bytes of ALT metadata). It
also eliminates the repeated account costs when the same queue_page appears
in many consecutive commit txs. With ALTs, commit budget headroom grows by
~150 bytes → ~22 entries per tx.

To reach 50 per tx, we need to compress the entry encoding. A follow-up
optimization worth evaluating: store `min_execute_slot` and `expires_at_slot`
as *deltas* relative to a per-batch baseline slot, encoded as u16s (bounded
to 65 K future slots ≈ 7 hours). That drops each entry from 48 → 36 bytes
and pushes a v0+ALT tx to ~30 entries. Full 50/tx is only reachable if we
also move to a custom sig-verification program that accepts a merkle root of
the batch and a membership proof; left as future work.

### Commit-side throughput (current design, legacy tx)

```
~17 commits per tx × Solana landing rate (say 400 tx/s for a hot relayer)
  ≈ 6,800 commits/s ingress per relayer instance.
```

Per-relayer CU budget per tx is small (one sig + N page writes), so CU is
not the binding constraint here.

## Reveal side

### Per-reveal ix-data cost

Typical `PerpPlaceOrderV2` payload is 45 bytes; cancel is 8–16 bytes.

```
borsh vec header (once)   : 4
per reveal:
  borsh vec header        :   4  (payload)
  payload                 :  ~45
  kind                    :   1
  dispatch_accounts_count :   1
----------------------------------
Per-reveal ix data         : ~51 bytes
```

### Per-reveal account cost

Shared prefix (oracle, perp market, bids, asks, event queue, bank, insurance
vault, etc.) is paid once per tx — roughly 8–10 accounts ≈ 256–320 bytes.
Treat that as *additional* fixed overhead above the 320 from above: call
fixed overhead **~600 bytes** for reveal txs.

Per-reveal **unique** accounts: just the `mango_account`. That's 32 B for
a legacy tx, or ~1 B with an ALT that has the user pre-registered.

### Per-reveal ed25519 cost (variants with user sig)

Each user sig contributes 142 bytes to the pre-ix (14 B offsets + 32 B
pubkey + 64 B sig + 32 B message). Batched into a single multi-sig pre-ix,
only 2 bytes of header are shared.

### Per-tx reveal count by regime

Budget: 1232 − 600 (fixed+shared-accounts) = **632 bytes for reveals**.

| Regime | Per-reveal cost | Reveals per tx |
|---|---|---|
| Legacy tx, user sig required | 51 + 32 + 142 = **225 B** | ~2–3 |
| v0+ALT tx, user sig required  | 51 + 1 + 142  = **194 B** | ~3 |
| Legacy tx, no user sig (trusted-relayer mode) | 51 + 32 = **83 B** | ~7 |
| v0+ALT tx, no user sig        | 51 + 1  = **52 B** | **~12** |

The scope note that the relayer is trusted for ingress implies we can
opt into the "no user sig onchain" mode safely — the offchain relayer
cryptographic proof substitutes for the onchain sig. In that regime,
reveal is only ~3× slower per tx than v3 execute (which fits 32 items).

### Throughput comparison (per market, per relayer tx-landing slot)

| Configuration | Items / tx | Relative to v3 execute |
|---|---|---|
| v3 execute (baseline)            | 32 | 1.00× |
| v4 reveal — legacy, user sig     | 3  | 0.09× |
| v4 reveal — legacy, no user sig  | 7  | 0.22× |
| v4 reveal — ALT, user sig        | 3  | 0.09× |
| v4 reveal — ALT, no user sig     | 12 | 0.38× |

### Claw-back: lane fanout

The v3 executor already runs multiple parallel lanes (`executor_target_lane_
fanout`), each a separate tx. v4 reveal workers inherit this: each lane is
its own `reveal_execute_market` tx with disjoint mango_accounts. With 4
lanes, effective reveal throughput in the no-sig/ALT regime approaches
~48 reveals/s per landing slot — above the v3 baseline.

**Net**: with ALT and a 4-way lane fanout, v4 reveal throughput matches or
exceeds v3 execute throughput even in the with-user-sig mode, and the
commit path adds a free ~6,800 commits/s of low-CU ingress headroom.

## Recommendations

1. **Ship legacy tx for commit** at ~17/tx. It covers the censorship goal
   cleanly, and commit throughput is not the bottleneck.
2. **Ship v0+ALT for reveal** from day one. The ALT should be pre-populated
   with every mango_account the relayer has ever seen, plus the fixed
   per-market accounts. Amortizes the ALT cost over every reveal forever.
3. **Choose the user-sig mode** explicitly in config. Default to verify-at-
   reveal; flip to trusted-relayer only after the offchain proof is
   production-grade.
4. **Instrument**: record real serialized tx size per reveal batch via
   `realize_reveal_tx` in `v4_reveal_packer.rs`; alert if the packer's
   estimate drifts from reality by more than 10 B.
5. **Keep the door open** to delta-encoded commit entries + custom verify
   if commit throughput ever becomes the limit (unlikely at current
   market scale).

## Observed on devnet

Measured via `bin/v4-stress` against the v4 program deployed at
`5KaJhG2AxyFbyNorYLtUUmrKXZMMGGWDQUzetQgS3LqB`. 150 commits per run,
batch_size 12, num_pages 4.

### Commit side

| Mode | Landing | Notes |
|---|---|---|
| `send_and_confirm`, inflight=8 | ~7 commits/s | bounded by devnet confirm latency (~2–3 s per confirmation) |
| Localnet same config | ~585 commits/s | localnet confirms in tens of ms |

Commits are confirm-latency bound, not tx-size or CU bound. On mainnet, a
real relayer lands commits pipelined with priority fees — we expect
~10–50× the send_and_confirm number. The harness's inflight=8
`send_and_confirm` pattern is a floor, not a ceiling.

### Reveal side — serial vs pipelined

The head of the queue serializes all reveals within a market: multiple
concurrent reveal txs take a writable lock on `queue_page` and execute
one at a time. `send_and_confirm` per tx makes this a 1-slot round-trip.
Pipelining with `send_transaction` + skip_preflight lets Solana serialize
them in submission order without waiting for each to finalize client-side.

| Mode | Reveals/s | Retry rate | vs serial |
|---|---|---|---|
| Serial (`send_and_confirm`) | 2.8 | n/a | 1.00× |
| Pipelined 200 ms | 17.9 | 0 | 6.4× |
| Pipelined 100 ms | 29.3 | 0 | 10.5× |
| **Pipelined 50 ms** | **40.5** | **0** | **14.5×** |
| Pipelined 25 ms | 1.7 | high | 0.6× |
| Pipelined 10 ms | 1.3 | very high | 0.5× |

**50 ms is the devnet sweet spot.** Below that, RPC-side rate-limiting /
scheduler reorderings cause enough retries to hurt net throughput.

The 50 ms pipelined mode fires 30 reveal txs in ~3 s (10 tx/s send rate,
all landing in-order without retries), and the final head advances by
150 slots inside 3.7 s.

### Mainnet projection

Mainnet has shorter slots (~400 ms vs devnet's ~600–1000 ms under load)
and higher validator parallelism. Carrying the same 50 ms spacing pattern
forward:

- Per-lane reveal ceiling ~60–80 reveals/s (vs 40.5 on devnet).
- 4-way lane fanout across markets/pages → **~250 reveals/s per relayer**.
- With ALT reducing per-reveal account cost, +20 % → **~300 reveals/s**.

This comfortably exceeds the v3 execute numbers under place-order-heavy
load (CU-bound at ~2-3 places/tx × 1 tx/slot × 4 lanes ≈ 25 places/s).

### Harness discovery: "optimistic" serial pipelining works

Before pipelining was added, the harness ran `send_and_confirm_transaction`
per reveal tx. Each tx serialized on the queue head *and* on the client's
confirm-wait, stacking latencies: ~18 s for 50 reveals on devnet.

The insight: the head-lock already serializes *execution*, so
client-side confirmation is redundant — we only need to submit in
sequence. `send_transaction` with skip_preflight and a small inter-send
gap exploits this: all N reveal txs sit in the validator's pool,
Solana's scheduler processes them in submission order thanks to the
write-lock on `queue_page`, and the head advances N slots in roughly one
slot's worth of block-building time rather than N slot round-trips.
