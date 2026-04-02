# Rust Harness Plan

## Goal

Replace the TypeScript order simulation in the continuum-state-harness with the actual Mango v4 Rust code compiled as a native library. The TS harness wrapper continues to handle HTTP/SSE/gRPC ingress and serves state to the frontend, but delegates all state computation (orderbook matching, position updates, health checks) to the Rust lib.

## Architecture

```
                    ┌─────────────────────────────────┐
                    │   TS Harness Wrapper (Node.js)   │
                    │                                   │
 HTTP/SSE ◄────────┤  /healthz, /state/*, /events      │
                    │  Ingress: /ingest/relay-intent    │
                    │  Admin:  /airdrop, /admin/*       │
                    │                                   │
                    │  On intent received:              │
                    │    payload = decode(intent)        │
                    │    result = rust.execute(payload)  │──── NAPI call ────┐
                    │    update SSE subscribers          │                   │
                    │    serve updated state             │                   │
                    └─────────────────────────────────┘                   │
                                                                          ▼
                    ┌─────────────────────────────────┐
                    │   Rust Native Lib (NAPI-RS)      │
                    │                                   │
                    │  In-memory state:                 │
                    │    - PerpMarket (orderbook, OI)   │
                    │    - MangoAccounts (positions)    │
                    │    - Oracle prices                │
                    │    - Health cache                 │
                    │                                   │
                    │  Entrypoints:                     │
                    │    execute_perp_place_order()     │
                    │    execute_perp_cancel_order()    │
                    │    get_orderbook_snapshot()       │
                    │    get_account_positions()        │
                    │    set_oracle_price()             │
                    │    bootstrap_from_snapshot()      │
                    └─────────────────────────────────┘
```

## Source: `/home/hetalkenaudekar/stagin4/rust-harness/mango-v4-src/`

Full copy of `programs/mango-v4/src/` (213 files, 12,214 lines in key modules).

---

## Phase 1: Extract Core State + Matching (~1500 lines)

### Files to keep (core engine):
```
state/orderbook/book.rs          513 lines  - matching algorithm
state/orderbook/bookside.rs      511 lines  - BookSide red-black tree
state/orderbook/nodes.rs         434 lines  - LeafNode, InnerNode
state/orderbook/queue.rs         385 lines  - EventQueue (fills, outs)
state/orderbook/ordertree.rs     ~200 lines - OrderTree operations
state/orderbook/bookside_iterator.rs  ~100 lines
state/orderbook/ordertree_iterator.rs ~100 lines
state/perp_market.rs             551 lines  - PerpMarket state + fees
state/mango_account.rs           3298 lines - MangoAccount (positions, orders)
state/mango_account_components.rs 1553 lines - PerpPosition, TokenPosition
state/oracle.rs                  ~400 lines - oracle price types
state/bank.rs                    ~500 lines - token bank state
health/cache.rs                  2474 lines - health computation
instructions/perp_place_order.rs 432 lines  - order execution entry
```

### Files to remove (not needed for simulation):
- All `instructions/` except `perp_place_order.rs` and `perp_cancel_order.rs`
- `accounts_ix/` (Anchor account validation — replaced by direct struct access)
- `lib.rs` (Anchor entrypoint — replaced by lib exports)
- Serum3, OpenBook, flash loan, token ops, liquidation, etc.

### Modifications:

1. **Replace `#[account(zero_copy)]` with `#[repr(C)]`**
   - Anchor's `zero_copy` is just `#[repr(C)]` + `bytemuck::Pod`
   - Keep the same memory layout, drop Anchor dependency

2. **Replace `AccountInfo` / `AccountLoader` with direct references**
   - On-chain: `let market = ctx.accounts.perp_market.load_mut()?`
   - Rust lib: `fn execute(&mut self, market: &mut PerpMarket, ...)`
   - State lives in Rust heap, not Solana account memory

3. **Replace `emit!()` / `msg!()` with callbacks or return values**
   - `emit!(FillEvent{...})` → push to `Vec<FillEvent>` returned to caller
   - `msg!("...")` → log to stderr or ignore

4. **Replace `Clock::get()` with a passed-in timestamp**
   - `let clock = Clock::get()?` → `fn execute(clock: ClockInfo, ...)`

5. **Replace Solana `hashv()` with `sha2` crate**
   - Direct SHA256, same output

6. **Health checks: replace account retriever with in-memory oracle store**
   - On-chain: reads oracle accounts via `AccountInfo`
   - Rust lib: reads from `HashMap<Pubkey, OraclePrice>` stored in engine state
   - The TS wrapper feeds oracle prices into the Rust lib (from RPC or stub)

---

## Phase 2: Build as NAPI-RS Native Module

### Why NAPI-RS (not WASM or FFI):
- **NAPI-RS**: Compiles Rust to a native `.node` addon. Called directly from Node.js with zero serialization overhead for simple types. Supports `&mut` borrows, async, Buffer passing.
- **WASM**: Requires serialization across the JS/WASM boundary. No direct memory sharing. Slower for large state (orderbook snapshots).
- **FFI (ffi-napi)**: Raw C ABI. Manual memory management. Error-prone.

### Crate structure:
```
rust-harness/
  Cargo.toml           # [lib] crate-type = ["cdylib"]
  src/
    lib.rs             # NAPI exports
    engine.rs          # HarnessEngine struct (owns all state)
    orderbook/         # Extracted from mango-v4-src/state/orderbook/
    state/             # PerpMarket, MangoAccount, etc.
    health/            # Health cache (simplified)
  package.json         # @napi-rs/cli build config
```

### NAPI Exports:
```rust
#[napi]
impl HarnessEngine {
    #[napi(constructor)]
    pub fn new() -> Self { ... }

    #[napi]
    pub fn bootstrap(&mut self, snapshot_json: String) -> Result<()> { ... }

    #[napi]
    pub fn set_oracle_price(&mut self, market_index: u32, price: f64) { ... }

    #[napi]
    pub fn execute_perp_place_order(&mut self, payload: Buffer)
        -> Result<ExecuteResult> { ... }

    #[napi]
    pub fn execute_perp_cancel_order(&mut self, order_id: BigInt)
        -> Result<bool> { ... }

    #[napi]
    pub fn get_orderbook_snapshot(&self, market_index: u32)
        -> Result<OrderbookSnapshot> { ... }

    #[napi]
    pub fn get_account_positions(&self, owner: String)
        -> Result<Vec<PositionSnapshot>> { ... }

    #[napi]
    pub fn get_open_orders(&self, owner: String, market_index: u32)
        -> Result<Vec<OrderSnapshot>> { ... }

    #[napi]
    pub fn get_market_stats(&self, market_index: u32)
        -> Result<MarketStats> { ... }
}
```

### Build:
```bash
cd rust-harness
npm install @napi-rs/cli
napi build --release
# Produces: rust-harness.linux-x64-gnu.node
```

---

## Phase 3: Integrate with TS Harness Wrapper

### Changes to `continuum-state-harness.ts`:

1. **Replace `ContinuumStateEngine` with Rust engine**:
```typescript
// Before:
import { ContinuumStateEngine } from '../../src/continuumHarness';
const engine = new ContinuumStateEngine();

// After:
import { HarnessEngine } from '../../rust-harness';
const engine = new HarnessEngine();
```

2. **Intent ingestion** — same flow, different execution:
```typescript
// Before (TS simulation):
engine.ingestRelayIntent(payload);

// After (Rust execution):
const result = engine.execute_perp_place_order(payload.payloadBuffer);
// result contains: fills[], position_changes[], new_orders[]
```

3. **State queries** — same HTTP endpoints, different source:
```typescript
// Before:
const snapshot = engine.getSnapshot('optimistic');

// After:
const bids = engine.get_orderbook_snapshot(marketIndex);
const positions = engine.get_account_positions(owner);
```

4. **What stays in TypeScript**:
   - HTTP/SSE server (express/http)
   - `/healthz`, `/livez`, `/metrics` endpoints
   - `/ingest/relay-intent` handler (decodes, calls Rust, broadcasts SSE)
   - `/airdrop`, `/airdrop-deposit` (on-chain tx, not simulated)
   - Event log writing
   - CoinGecko price feed → `engine.set_oracle_price()`
   - Reconciliation loop (reads on-chain state, compares to Rust engine)

5. **What moves to Rust**:
   - Orderbook state (BookSide trees, nodes)
   - PerpMarket state (OI, fees, funding)
   - MangoAccount state (positions, open orders)
   - Order matching algorithm
   - Health checks
   - Fill/out event generation

---

## Phase 4: Verification

1. **Unit tests**: Port the existing `test_execution_queue.rs` tests to run against the native lib
2. **Snapshot comparison**: Bootstrap both TS engine and Rust engine from the same on-chain snapshot, replay the same intents, compare state after each intent
3. **Fuzzing**: Random order sequences, compare TS vs Rust output
4. **Production shadow**: Run both engines in parallel for 1 hour, alert on any divergence

---

## Estimated Effort

| Phase | Scope | Effort |
|-------|-------|--------|
| 1. Extract core | Remove Anchor/Solana deps, adapt 12K lines | 2-3 days |
| 2. NAPI-RS build | Crate setup, exports, types | 1 day |
| 3. TS integration | Replace engine calls, adapt endpoints | 1-2 days |
| 4. Verification | Tests, shadow mode, fuzzing | 2-3 days |
| **Total** | | **6-9 days** |

---

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Health cache complexity (2474 lines) | Largest single module to extract | Can start with health checks disabled (like current TS harness) and add incrementally |
| MangoAccount dynamic sizing | On-chain uses variable-length accounts | In native lib, use Vec instead of fixed arrays |
| Anchor macro dependencies | `#[zero_copy]`, `#[account]`, error types | Replace with standard Rust derives (`#[repr(C)]`, `thiserror`) |
| I80F48 fixed-point math | Used throughout for prices/quantities | Already a standalone crate (`fixed`), no Solana dependency |
| Ongoing sync with on-chain changes | On-chain program may evolve | Rust lib source is a copy — re-copy and re-adapt when on-chain changes |

---

## Confirmation: TS ↔ Rust Integration via NAPI

**Yes, the TS wrapper can call Rust lib entrypoints directly.** NAPI-RS compiles to a native Node.js addon (`.node` file) that is `require()`-able from TypeScript. The call overhead is ~50 nanoseconds per invocation (function pointer, no serialization for primitives/Buffers).

The TS harness wrapper continues to:
- Listen on ports 9091 (HTTP/SSE)
- Handle all external API requests
- Accept intents from the relayer
- Broadcast SSE events to subscribers
- Write event logs
- Run reconciliation checks

It delegates to the Rust lib for:
- All state mutations (order placement, cancellation, fills)
- All state queries (orderbook, positions, balances)
- Health computation

No breakage to external interfaces — the HTTP API surface stays identical.
