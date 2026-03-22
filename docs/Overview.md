# Fermi DEX v1 — Overview

## What Fermi Is

Fermi is a fully on-chain perpetual futures exchange on Solana built around a single design thesis: **predictable, tamper-proof order execution**. Every order enters a FIFO queue whose position is cryptographically committed at ingress; no participant — not the relayer, not the cranker, not a validator — can reorder, front-run, or selectively execute queued intents without detection.

The result is a DEX that behaves like a centralized exchange for the trader (sub-second optimistic fills, familiar limit/market/IOC order types, streaming state) while preserving the properties that matter on-chain (self-custody, permissionless liquidation, verifiable execution order, censorship-resistant fallback).

---

## Design Goals

1. **Deterministic FIFO execution** — orders execute in the exact sequence they were accepted by the cooperative relayer. Queue position is hash-locked; any attempt to substitute accounts or reorder items is rejected on-chain.

2. **Fast perceived finality** — an off-chain simulation harness reflects order state optimistically within milliseconds of relay acceptance, with on-chain settlement confirming in 6-10 slots (~2.4–4s, configurable via `gap_wait_slots`).

3. **High throughput per market** — targeting up to 100 sustained TPS per market on mainnet (single queue), with a path to 500+ via account-hash queue sharding.

4. **Censorship-resistant fallback** — if the relayer goes offline, any user can submit orders directly to the chain via `enqueue_direct` with a 10-slot execution delay. No cooperation from any off-chain service is required to close positions, cancel orders, or withdraw funds.

5. **Transparent risk engine** — health checks, liquidation thresholds, and oracle prices are all computed on-chain with published weights and parameters. Nothing is hidden behind an off-chain black box.

---

## How It Works — The 30-Second Version

```
 User signs intent           Relayer sequences          On-chain queue           Cranker executes
 ─────────────────  ───>  ───────────────────  ───>  ──────────────────  ───>  ──────────────────
  "buy 2 SOL-PERP            assigns seq #317,         item lands at            head-of-queue
   @ $148 limit"              CTM-signs envelope,       position 317,            dispatched into
                              submits enqueue tx        hash-locked              orderbook, fill
                                                                                 or resting
```

1. **Sign** — the trader signs a canonical intent message (market, side, price, size, accounts) with their wallet. This is an Ed25519 signature, not a Solana transaction.

2. **Relay** — the intent is sent to the Fermi relayer (gRPC). The relayer assigns a monotonically increasing sequence number, wraps the intent in a CTM envelope (co-signed by the relayer's key), and submits an `execution_queue_enqueue_ctm` transaction to Solana.

3. **Queue** — the on-chain program verifies both signatures, checks the sequence is in the valid window, and writes the item to a circular buffer at the sequence-indexed slot. The item's `payload_hash` and `accounts_hash` are stored alongside it.

4. **Execute** — a permissionless cranker calls `execution_queue_execute`. The program pops the head item, re-derives the accounts hash from the provided accounts, confirms it matches the stored hash, then dispatches the order into the on-chain orderbook (with a health-region check wrapping place-order variants). If the head item is missing after `gap_wait_slots` (default 4-6 slots), the queue skips forward.

5. **Settle** — matched orders produce events on the perp event queue. PnL settlement, funding payments, and liquidations proceed through standard Mango v4 mechanics.

---

## The Trading Experience in Practice

### What the trader sees

| Event | Latency | What happened |
|---|---|---|
| Order submitted | ~50-100ms | Intent accepted by relayer, sequence assigned |
| Optimistic fill | ~100-300ms | Harness simulates execution against optimistic book |
| On-chain inclusion | ~1.5-3s | Enqueue transaction lands in a Solana slot |
| Execution | ~2.5-5s | Cranker dispatches head-of-queue into orderbook |
| Confirmed state | ~6-10 slots | Transaction finalized, confirmed view updated |

The harness provides an **optimistic view** — updated the moment the relayer accepts an intent — and a **confirmed view** — updated only after on-chain finalization. The frontend defaults to the optimistic view, giving traders the responsiveness of a centralized venue while the on-chain state catches up in the background.

### Order types supported

- **Limit** — rests on the book at the specified price
- **Market** — crosses the book immediately (IOC semantics)
- **Immediate-or-Cancel (IOC)** — fills what it can, cancels the rest
- **Post-Only** — rejected if it would cross the book (maker-only)
- **Reduce-Only** — can only decrease an existing position
- **Pegged** — price tracks best bid/ask with a configurable offset
- **Time-in-Force** — automatic expiry via `expiry_timestamp`

### What you can do without the relayer

Everything. The relayer provides sequencing and speed, but is not required for safety. If it goes down:

- **Cancel orders** — submit `enqueue_direct` with a cancel payload (10-slot delay)
- **Close positions** — place a reduce-only order via direct enqueue
- **Withdraw funds** — enqueue a liquidity withdrawal (25-slot delay on liquidity items)
- **Liquidate** — liquidation instructions are not queued; they are direct on-chain calls, always available

---

## Performance Characteristics

### Current benchmarks (localnet / devnet)

| Metric | Value | Notes |
|---|---|---|
| Peak place TPS (localnet) | 102 | Single-threaded validator |
| Avg sustained place TPS | 35-40 | With gap handling and health reverts |
| Per-item CU cost (direct dispatch) | ~140K | 10 items fit in 1.4M CU budget |
| Execute tx prep time | ~9.3ms | Relayer-side instruction building |
| Queue capacity | 1,024 CTM items + 128 liquidity items | Per execution queue account |

### Mainnet projections

| Scenario | Sustained TPS | Key dependencies |
|---|---|---|
| Conservative (single queue, no SWQoS) | ~17 | Default infrastructure |
| Optimized (single queue, SWQoS, gap=6, pre-checks) | ~116 | Tier 1+2 optimizations |
| Sharded (4 queues, full optimization) | ~500+ | Account-hash sharding |

### p99 inclusion guarantee

With stake-weighted quality-of-service (SWQoS) RPC endpoints and a gap_wait_slots setting of 6, the system achieves:

- **>99% FIFO-ordered execution** — items execute in sequence with <0.5% gap-skip rate
- **p99 inclusion within 6 slots** (~2.4s) of relay submission
- **92% strict FIFO fidelity** — the remaining 8% are items that arrived out-of-order due to network jitter and were executed after a bounded wait

---

## Comparison to Existing Solana Perps DEXes

### The landscape

Most Solana perpetual DEXes fall into two categories:

**AMM/vAMM-based** (e.g., Jupiter Perps, Flash Trade) — traders interact with a liquidity pool rather than a counterparty. Price is determined by pool math, not by order matching. Simple and capital-efficient for the protocol, but offers no price discovery, limited order types, and high slippage on larger sizes.

**Off-chain orderbook with on-chain settlement** (e.g., Drift v2, Zeta) — an off-chain keeper or market-maker provides quotes, and the chain settles matched trades. Fast, but the orderbook state is opaque: you cannot verify queue position, execution priority, or whether the keeper is providing fair ordering.

### Where Fermi differs

| Property | AMM-based | Off-chain book | Fermi |
|---|---|---|---|
| Orderbook on-chain | No | No | **Yes** |
| Verifiable execution order | N/A | No | **Yes (FIFO, hash-locked)** |
| Price discovery | Pool math | Off-chain | **On-chain matching** |
| Front-running resistance | Partial (AMM) | Trust keeper | **Cryptographic (queue position committed at ingress)** |
| Censorship-resistant trading | Yes | No (need keeper) | **Yes (direct-enqueue fallback)** |
| Limit orders | Limited | Yes | **Yes (full order types)** |
| Liquidation | Permissionless | Usually keeper | **Permissionless** |

### Why FIFO queue position matters

On most DEXes, the entity that controls execution ordering has the power to:

1. **Front-run** — see a large order, insert one ahead of it
2. **Sandwich** — bracket a trade with buys before and sells after to extract value
3. **Selectively delay** — hold back unfavorable orders while fast-tracking preferred ones
4. **Cherry-pick execution** — execute profitable items and skip unprofitable ones

Fermi's design makes all of these detectable or impossible:

- **Queue position is committed at ingress** — the item's `payload_hash` and `accounts_hash` are stored on-chain when enqueued. At execute time, the program recomputes these hashes from the actual accounts provided. Any substitution fails hash verification.

- **Execution is strictly head-of-queue** — the cranker cannot skip ahead to a more profitable item. The program enforces `next_sequence_to_execute` and halts if the head doesn't match.

- **Relayer sequencing is auditable** — every sequence number, every CTM envelope, and every intent signature is recorded. Sequence gaps are observable on-chain and trigger bounded skip-ahead after a configurable wait.

- **Direct-enqueue bypasses the relayer entirely** — if the relayer misbehaves (censors, reorders), users fall back to `enqueue_direct` with a 10-slot delay. The delay prevents race conditions with in-flight relayer transactions but preserves liveness unconditionally.

The net effect: **you do not need to trust anyone to get fair execution**. The queue is append-only, sequence-ordered, hash-verified, and permissionlessly executable. This is not a soft guarantee enforced by reputation — it is a hard constraint enforced by the program's state machine.

---

## System Architecture at a Glance

```
                                    ┌─────────────────────────────┐
                                    │        Trader / Bot         │
                                    │   signs intent with wallet  │
                                    └──────────┬──────────────────┘
                                               │
                              ┌────────────────┼────────────────┐
                              │ Primary path   │  Fallback path │
                              │ (relayer up)   │  (relayer down)│
                              ▼                │                ▼
                   ┌──────────────────┐        │    ┌───────────────────┐
                   │  Rust Relayer    │        │    │  enqueue_direct   │
                   │  (gRPC :9090)    │        │    │  (user submits tx │
                   │                  │        │    │   directly, 10-   │
                   │  - sequences     │        │    │   slot delay)     │
                   │  - CTM-signs     │        │    └─────────┬─────────┘
                   │  - submits tx    │        │              │
                   └────────┬─────────┘        │              │
                            │                  │              │
                            ▼                  │              ▼
               ┌────────────────────────────────────────────────────┐
               │              Solana Blockchain                     │
               │                                                    │
               │  ┌──────────────────────────────────────────────┐ │
               │  │         Execution Queue (on-chain)           │ │
               │  │                                              │ │
               │  │  seq 315 │ seq 316 │ seq 317 │ seq 318 │... │ │
               │  │  (done)  │ (done)  │ (HEAD)  │ (pending)│   │ │
               │  │                     ▲                        │ │
               │  │          next_sequence_to_execute            │ │
               │  └──────────────────────────────────────────────┘ │
               │                        │                          │
               │                        ▼                          │
               │  ┌──────────────────────────────────────────────┐ │
               │  │      Perp Orderbook (on-chain)               │ │
               │  │      Bids / Asks / Event Queue               │ │
               │  └──────────────────────────────────────────────┘ │
               └───────────────────────┬────────────────────────────┘
                                       │
                            ┌──────────┼──────────┐
                            ▼                     ▼
                 ┌────────────────────┐  ┌─────────────────┐
                 │  Continuum Harness │  │  Cranker         │
                 │  (state server)    │  │  (permissionless │
                 │                    │  │   executor)      │
                 │  optimistic view ──│  │                  │
                 │  confirmed view  ──│  │  pops head,      │
                 │  SSE stream      ──│  │  dispatches into │
                 │  REST API          │  │  orderbook       │
                 └────────────────────┘  └─────────────────┘
```

### Component summary

| Component | Role | Required for trading? |
|---|---|---|
| **On-chain program** | Queue management, orderbook, health, liquidation | Yes (always) |
| **Relayer** | Sequences intents, submits to chain | No (direct-enqueue fallback) |
| **Cranker/Executor** | Pops queue head, dispatches execution | Yes (permissionless, anyone can run) |
| **Harness** | Optimistic state, REST API, SSE stream | No (convenience for fast reads) |
| **HTTP Bridge** | REST-to-gRPC translation for relayer | No (convenience for non-gRPC clients) |
| **Frontend** | Trading UI | No (bots interact via SDK/API directly) |

---

## Security Model — What You're Trusting

| Trust assumption | Consequence if violated | Mitigation |
|---|---|---|
| Relayer is live and honest | Orders delayed; possible reordering | Direct-enqueue fallback (10-slot delay); sequence gaps auditable on-chain |
| Cranker is live | Queue items wait longer to execute | Permissionless — anyone can crank; embedded executor in relayer service |
| Oracle is accurate | Mispriced health checks, bad liquidations | On-chain oracle validation; dual-oracle band check recommended for CLMM oracles |
| Solana is live | Nothing works | Fundamental platform dependency |
| Program has no bugs | Loss of funds | Audit completed; 165/165 test pass; independent review recommended before mainnet |

The key insight: **liveness of off-chain components affects speed, not safety**. Even if every off-chain service goes down simultaneously, users retain the ability to cancel orders, close positions, and withdraw funds by submitting transactions directly to the chain.

---

## What's Next

The following docs go deeper into each layer:

- **[Onchain Programs](./Onchain_Programs.md)** — the execution queue, orderbook, health engine, and liquidation mechanics
- **[Offchain Components](./Offchain_Components.md)** — the relayer, harness, bridge, and how they compose into a full trading stack
- **[API](./API.md)** — a practical guide to building bots and trading programs against Fermi's interface
