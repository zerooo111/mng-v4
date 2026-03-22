# Fermi DEX v1 — API Guide

A practical guide to building trading bots, market makers, and integrations against Fermi's interface. Covers the full lifecycle: connecting, reading state, placing orders, tracking fills, and the direct-chain fallback for when off-chain services are unavailable.

---

## Part 1: Trading via the Relayer

This is the primary path. The relayer provides low-latency order submission with FIFO sequencing. The harness provides fast state reads with optimistic and confirmed views.

### 1.1 Setup

**Dependencies:**

```bash
npm install @blockworks-foundation/mango-v4 @coral-xyz/anchor @solana/web3.js
```

**Initialization:**

```typescript
import { MangoClient, Group, MangoAccount, PerpMarket } from '@blockworks-foundation/mango-v4';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Connection, Keypair, PublicKey } from '@solana/web3.js';

// Connect to Solana
const connection = new Connection(process.env.CLUSTER_URL_OVERRIDE!);
const wallet = new Wallet(Keypair.fromSecretKey(/* your keypair bytes */));
const provider = new AnchorProvider(connection, wallet, { commitment: 'confirmed' });

// Initialize client
const client = MangoClient.connect(provider, 'devnet', new PublicKey(process.env.PROGRAM_ID!));

// Load group and market
const group = await client.getGroup(new PublicKey(process.env.EXECUTION_QUEUE_GROUP_PK!));
const perpMarket = group.getPerpMarketByName('SOL-PERP');

// Load or create your MangoAccount
const mangoAccounts = await client.getMangoAccountsForOwner(group, wallet.publicKey);
const mangoAccount = mangoAccounts[0]; // or create one
```

### 1.2 Reading State from the Harness

The harness (default `http://127.0.0.1:9091`) serves state via REST. Append `?view=optimistic` (default) or `?view=confirmed` to any state endpoint.

#### Orderbook

```typescript
// Fetch the full orderbook for a market
const response = await fetch(`${HARNESS_URL}/state/markets/${perpMarket.perpMarketIndex}`);
const data = await response.json();

// data.bids: [{ price, size, owner? }, ...]
// data.asks: [{ price, size, owner? }, ...]
// data.oracle_price: current oracle price
// data.funding_rate: current funding rate
```

**Optimistic vs confirmed — when to use which:**

| Use case | View | Why |
|---|---|---|
| Displaying orderbook to trader | Optimistic | Reflects pending orders, feels responsive |
| Computing fill probability | Optimistic | Includes orders not yet on-chain |
| Determining account health for risk | Confirmed | Only trust finalized state for risk decisions |
| Reconciling PnL | Confirmed | Need ground truth for accounting |
| Checking if your order filled | Optimistic first, confirmed to verify | Fast feedback, then certainty |

#### Your positions and balances

```typescript
// Positions and open orders
const userState = await fetch(`${HARNESS_URL}/state/users/${wallet.publicKey}`);
const { accounts, positions, openOrders } = await userState.json();

// Token balances
const balances = await fetch(`${HARNESS_URL}/state/balances/${wallet.publicKey}`);
const { available, reserved } = await balances.json();
```

#### Your orders in a specific market

```typescript
const orders = await fetch(
  `${HARNESS_URL}/state/orders/${perpMarket.perpMarketIndex}?owner=${wallet.publicKey}`
);
const myOrders = await orders.json();
// [{ orderId, clientOrderId, side, price, size, status }, ...]
```

#### Recent trades / fills

```typescript
const trades = await fetch(
  `${HARNESS_URL}/state/trades/${perpMarket.perpMarketIndex}?limit=100`
);
const recentFills = await trades.json();
// [{ price, size, side, timestamp, makerOrderId, takerOrderId }, ...]
```

#### Streaming updates (SSE)

```typescript
const eventSource = new EventSource(`${HARNESS_URL}/state/stream`);

eventSource.onmessage = (event) => {
  const update = JSON.parse(event.data);
  // update.type: 'orderbook' | 'trade' | 'position' | 'balance' | ...
  // update.market: market index
  // update.data: payload
};
```

#### OHLCV candles

```typescript
const candles = await fetch(
  `${HARNESS_URL}/state/candles/${perpMarket.perpMarketIndex}?tf=1h&limit=500`
);
const ohlcv = await candles.json();
// [{ time, open, high, low, close, volume }, ...]
```

### 1.3 Placing Orders via the Relayer

Order placement is a four-step process: encode payload → build remaining accounts → sign intent → submit to relayer.

#### Step 1: Encode the payload

```typescript
import {
  encodePerpPlaceOrderV2QueuePayload,
  encodePerpCancelOrderByClientOrderIdQueuePayload,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpCancelAllOrdersBySideQueuePayload,
} from '@blockworks-foundation/mango-v4';

// Place a limit buy
const placePayload = encodePerpPlaceOrderV2QueuePayload({
  side: PerpOrderSide.bid,
  priceLots: BigInt(perpMarket.uiPriceToLots(148.50).toString()),
  maxBaseLots: BigInt(perpMarket.uiBaseToLots(2.0).toString()),
  maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(300.0).toString()),
  clientOrderId: BigInt(Date.now()),  // unique per-user tracking ID
  orderType: PerpOrderType.limit,
  selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
  reduceOnly: false,
  expiryTimestamp: BigInt(Math.floor(Date.now() / 1000) + 120), // 2 min TIF
  limit: 20,  // max orders to match against
});
```

**Order types reference:**

| Type | Behavior |
|---|---|
| `PerpOrderType.limit` | Rests on book at specified price |
| `PerpOrderType.immediateOrCancel` | Fills what it can, cancels remainder |
| `PerpOrderType.postOnly` | Rejected if it would cross the book |
| `PerpOrderType.market` | Crosses book immediately (effectively IOC with no price limit) |
| `PerpOrderType.postOnlySlide` | Slides to best bid/ask if would cross |

**Reduce-only:** Set `reduceOnly: true` to ensure the order can only decrease your position, never increase or flip it. Essential for closing positions safely.

#### Step 2: Build canonical remaining accounts

```typescript
import { executionQueueCanonicalPerpRemainingAccounts } from '@blockworks-foundation/mango-v4';

const remainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
  client,
  group,
  mangoAccount,
  marketIndex: perpMarket.perpMarketIndex,
  userOwner: wallet.publicKey,
});
// Returns: [group, mangoAccount, userOwner, perpMarket, bids, asks, eventQueue, oracle, ...healthAccounts]
```

These accounts are hashed at enqueue time and re-verified at execute time. The canonical ordering ensures the hash is deterministic.

#### Step 3: Sign the user intent

```typescript
import {
  buildExecutionQueueUserIntent,
  signExecutionQueueIntentMessage,
} from '@blockworks-foundation/mango-v4';

const intent = await buildExecutionQueueUserIntent({
  group: group.publicKey,
  executionQueue: new PublicKey(process.env.EXECUTION_QUEUE_PK!),
  mangoAccount: mangoAccount.publicKey,
  userOwner: wallet.publicKey,
  payload: placePayload,
  remainingAccounts,
});

// Sign with your keypair (Ed25519, NOT a Solana transaction signature)
const userSignature = signExecutionQueueIntentMessage(
  wallet.payer.secretKey,      // 64-byte Ed25519 secret key
  intent.userIntentMessage,    // canonical message bytes
);
```

#### Step 4: Submit to relayer

**Via gRPC (recommended for bots):**

```typescript
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';

const packageDef = protoLoader.loadSync('ctm_sequencer.proto');
const proto = grpc.loadPackageDefinition(packageDef);
const relayerClient = new proto.ctm.CtmSequencerRelayer(
  process.env.CTM_RELAYER_ADDR || '127.0.0.1:9090',
  grpc.credentials.createInsecure()
);

const response = await new Promise((resolve, reject) => {
  relayerClient.SubmitIntent({
    group: group.publicKey.toBase58(),
    execution_queue: process.env.EXECUTION_QUEUE_PK,
    market: perpMarket.perpMarketIndex.toString(),
    payload: placePayload,
    remaining_accounts: remainingAccounts.map(a => ({
      pubkey: a.pubkey.toBase58(),
      is_signer: a.isSigner,
      is_writable: a.isWritable,
    })),
    min_execute_slot: '0',     // 0 = execute ASAP
    expires_at_slot: '0',      // 0 = no expiry
    user_owner: wallet.publicKey.toBase58(),
    mango_account: mangoAccount.publicKey.toBase58(),
    user_signature: Buffer.from(userSignature),
  }, (err, res) => err ? reject(err) : resolve(res));
});

console.log(`Sequence: ${response.sequence}, TX: ${response.tx_signature}`);
```

**Via HTTP bridge (simpler, slightly higher latency):**

```typescript
const response = await fetch(`${BRIDGE_URL}/relay/submit-intent`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    group: group.publicKey.toBase58(),
    execution_queue: process.env.EXECUTION_QUEUE_PK,
    market: perpMarket.perpMarketIndex.toString(),
    payload: Buffer.from(placePayload).toString('base64'),
    remaining_accounts: remainingAccounts.map(a => ({
      pubkey: a.pubkey.toBase58(),
      is_signer: a.isSigner,
      is_writable: a.isWritable,
    })),
    user_owner: wallet.publicKey.toBase58(),
    mango_account: mangoAccount.publicKey.toBase58(),
    user_signature: Buffer.from(userSignature).toString('base64'),
  }),
});
```

### 1.4 Canceling Orders

Canceling follows the same encode → sign → submit flow. Cancel payloads are lighter (no health check required on-chain).

**Cancel by client order ID:**

```typescript
const cancelPayload = encodePerpCancelOrderByClientOrderIdQueuePayload({
  clientOrderId: BigInt(myClientOrderId),
});
// then: build remaining accounts, sign intent, submit to relayer (same steps 2-4)
```

**Cancel all orders in a market:**

```typescript
const cancelAllPayload = encodePerpCancelAllOrdersQueuePayload({
  limit: 255,  // max orders to cancel in one dispatch
});
```

**Cancel all orders on one side:**

```typescript
const cancelBidsPayload = encodePerpCancelAllOrdersBySideQueuePayload({
  side: PerpOrderSide.bid,
  limit: 255,
});
```

### 1.5 Checking if Orders Were Filled

There is no single "order status" endpoint — you infer fill status from multiple signals:

**1. Check your open orders (unfilled = still resting):**

```typescript
const myOrders = await fetch(
  `${HARNESS_URL}/state/orders/${marketIndex}?owner=${wallet.publicKey}`
).then(r => r.json());

const isStillOpen = myOrders.some(o => o.clientOrderId === myClientOrderId);
```

**2. Check recent trades (filled = appears in trades):**

```typescript
const trades = await fetch(
  `${HARNESS_URL}/state/trades/${marketIndex}?limit=200`
).then(r => r.json());

const myFills = trades.filter(t =>
  t.makerOrderId === myOrderId || t.takerOrderId === myOrderId
);
```

**3. Check position change (filled = position size changed):**

```typescript
const userState = await fetch(`${HARNESS_URL}/state/users/${wallet.publicKey}`).then(r => r.json());
const position = userState.positions.find(p => p.marketIndex === marketIndex);
// Compare position.basePositionLots to what you expected
```

**4. Use SSE stream for real-time notification:**

```typescript
const es = new EventSource(`${HARNESS_URL}/state/stream`);
es.onmessage = (event) => {
  const update = JSON.parse(event.data);
  if (update.type === 'trade' && update.market === marketIndex) {
    // check if this trade involves your order
  }
};
```

### 1.6 Designing a Market Maker

Here's the structure of a basic market maker against Fermi's interface:

```
┌─────────────────────────────────────────────────────────┐
│                    Market Maker Loop                     │
│                                                         │
│  every TICK_INTERVAL_MS:                                │
│                                                         │
│  1. Read state                                          │
│     ├── GET /state/markets/{id}  → orderbook            │
│     ├── GET /state/users/{owner} → my positions         │
│     └── fetch reference price (oracle, CEX, etc.)       │
│                                                         │
│  2. Compute quotes                                      │
│     ├── mid = reference_price                           │
│     ├── spread = f(volatility, inventory, risk)         │
│     ├── bid = mid - spread/2                            │
│     ├── ask = mid + spread/2                            │
│     └── size = f(inventory_target, max_exposure)        │
│                                                         │
│  3. Cancel stale orders                                 │
│     └── cancelAll or cancel-by-clientOrderId            │
│                                                         │
│  4. Place new orders                                    │
│     ├── place bid (PostOnly)                            │
│     └── place ask (PostOnly)                            │
│                                                         │
│  5. Risk management                                     │
│     ├── check health (confirmed view)                   │
│     ├── if inventory too large → reduce-only order      │
│     └── if health too low → cancel all, reduce          │
└─────────────────────────────────────────────────────────┘
```

**Key considerations for MM design on Fermi:**

| Concern | Recommendation |
|---|---|
| Order type | Use `PostOnly` for maker orders — ensures you never cross the spread accidentally and always earn maker-side priority |
| Cancel-before-place | Cancel existing orders before placing new ones each tick. Use `cancelAllOrders` for simplicity or `cancelByClientOrderId` for precision. |
| Client order IDs | Use monotonically increasing IDs (e.g., `Date.now()` or a counter). These are your primary handle for tracking individual orders. |
| Inventory management | Track your base position and skew quotes toward reducing it. Set hard position limits and use `reduceOnly` orders to enforce them. |
| Health monitoring | Read from the confirmed view for health decisions. The optimistic view can temporarily show better health than reality. |
| Expiry | Set `expiryTimestamp` on all orders (e.g., 2-5 minutes). Stale orders that the MM hasn't refreshed should auto-expire rather than linger. |
| Parallelism | Submit cancel and place intents in parallel (they get separate sequence numbers). Don't wait for cancel confirmation before placing. |
| Error handling | If the relayer returns an error (backpressure, RPC failure), back off exponentially. Don't hammer the relayer. |
| Account reloading | Reload your MangoAccount from the harness every N ticks to stay in sync with on-chain state (positions, health, etc.). |

**Example tick implementation:**

```typescript
async function mmTick() {
  // 1. Read state
  const [book, userState, refPrice] = await Promise.all([
    fetch(`${HARNESS_URL}/state/markets/${marketIndex}`).then(r => r.json()),
    fetch(`${HARNESS_URL}/state/users/${wallet.publicKey}`).then(r => r.json()),
    fetchReferencePrice(),  // your oracle / CEX feed
  ]);

  const position = userState.positions.find(p => p.marketIndex === marketIndex);
  const basePosition = position?.basePositionLots ?? 0;

  // 2. Compute quotes
  const spread = computeSpread(refPrice, basePosition);
  const bidPrice = refPrice - spread / 2;
  const askPrice = refPrice + spread / 2;
  const size = computeSize(basePosition);

  // 3. Cancel all existing orders
  const cancelPayload = encodePerpCancelAllOrdersQueuePayload({ limit: 255 });
  await submitIntent(cancelPayload);

  // 4. Place new quotes (parallel)
  const bidPayload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.bid,
    priceLots: BigInt(perpMarket.uiPriceToLots(bidPrice).toString()),
    maxBaseLots: BigInt(perpMarket.uiBaseToLots(size).toString()),
    maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(size * bidPrice * 1.01).toString()),
    clientOrderId: BigInt(nextClientOrderId++),
    orderType: PerpOrderType.postOnly,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: BigInt(Math.floor(Date.now() / 1000) + 120),
    limit: 10,
  });

  const askPayload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.ask,
    priceLots: BigInt(perpMarket.uiPriceToLots(askPrice).toString()),
    maxBaseLots: BigInt(perpMarket.uiBaseToLots(size).toString()),
    maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(size * askPrice * 1.01).toString()),
    clientOrderId: BigInt(nextClientOrderId++),
    orderType: PerpOrderType.postOnly,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: BigInt(Math.floor(Date.now() / 1000) + 120),
    limit: 10,
  });

  // Submit both in parallel (they get independent sequences)
  await Promise.all([
    submitIntent(bidPayload),
    submitIntent(askPayload),
  ]);
}
```

### 1.7 Deposits and Withdrawals

Liquidity operations (deposits and withdrawals) go through a separate queue path with a 25-slot delay:

```typescript
// These are enqueued as liquidity items, not CTM items
// The 25-slot delay prevents deposit-trade-withdraw arbitrage within a few slots

// Deposit: use the standard client method, or enqueue via liquidity path
await client.tokenDeposit(group, mangoAccount, mintPk, amount);

// Withdraw:
await client.tokenWithdraw(group, mangoAccount, mintPk, amount, allowBorrow);
```

When using the queue path, liquidity items execute permissionlessly (no user signature at execute time, since the funds flow is constrained by the MangoAccount and bank PDA).

---

## Part 2: Direct Chain Fallback

If the relayer and/or harness are unreachable, you can interact with the chain directly. Everything works — it's just slower and requires you to build and pay for Solana transactions yourself.

### 2.1 When to use the fallback

Use direct chain interaction when:
- The relayer health check (`GET /healthz` on :9093) fails or returns errors
- The harness health check (`GET /healthz` on :9091) fails
- You need to perform an emergency action (close position, cancel all orders, withdraw)
- You want to verify on-chain state without trusting the harness

### 2.2 Reading state directly from chain

Without the harness, you read on-chain accounts directly via Solana RPC:

```typescript
// Reload your MangoAccount from chain
await mangoAccount.reload(client);

// Get current position
const position = mangoAccount.getPerpPosition(perpMarket.perpMarketIndex);
const basePositionUi = mangoAccount.getPerpPositionUi(group, perpMarket.perpMarketIndex);

// Get token balances
const tokenBalance = mangoAccount.getTokenBalanceUi(group.getFirstBankByMint(usdcMint));

// Get health
const healthRemainingAccounts = await client.buildHealthRemainingAccounts(
  group, [mangoAccount], [], [perpMarket], [], []
);
// Health is computed client-side from the loaded account data
```

**Reading the orderbook directly:**

```typescript
// Reload the perp market (loads bids, asks, event queue from chain)
await perpMarket.loadBids(client);
await perpMarket.loadAsks(client);

// Iterate the book
for (const bid of perpMarket.bids.items()) {
  console.log(`Bid: ${perpMarket.priceLotsToUi(bid.priceLots)} x ${perpMarket.baseLotsToUi(bid.sizeLots)}`);
}
```

**Reading the execution queue:**

```typescript
// Fetch the raw queue account
const queueAccount = await connection.getAccountInfo(new PublicKey(process.env.EXECUTION_QUEUE_PK!));
// Parse header to check next_sequence_to_execute, max_seen_sequence, etc.
```

### 2.3 Submitting orders via enqueue_direct

The `enqueue_direct` instruction bypasses the relayer entirely. It requires only the user's Ed25519 signature (as a pre-instruction) and enforces a 10-slot execution delay.

```typescript
import {
  encodePerpPlaceOrderV2QueuePayload,
  buildExecutionQueueUserIntent,
  signExecutionQueueIntentMessage,
} from '@blockworks-foundation/mango-v4';
import { Ed25519Program, TransactionInstruction, Transaction } from '@solana/web3.js';

// 1. Build payload (same as relayer path)
const payload = encodePerpPlaceOrderV2QueuePayload({
  side: PerpOrderSide.bid,
  priceLots: BigInt(perpMarket.uiPriceToLots(145.0).toString()),
  maxBaseLots: BigInt(perpMarket.uiBaseToLots(1.0).toString()),
  maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(150.0).toString()),
  clientOrderId: BigInt(Date.now()),
  orderType: PerpOrderType.limit,
  selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
  reduceOnly: false,
  expiryTimestamp: BigInt(0), // no expiry
  limit: 10,
});

// 2. Build remaining accounts (same as relayer path)
const remainingAccounts = await executionQueueCanonicalPerpRemainingAccounts({
  client, group, mangoAccount,
  marketIndex: perpMarket.perpMarketIndex,
  userOwner: wallet.publicKey,
});

// 3. Build and sign user intent
const intent = await buildExecutionQueueUserIntent({
  group: group.publicKey,
  executionQueue: new PublicKey(process.env.EXECUTION_QUEUE_PK!),
  mangoAccount: mangoAccount.publicKey,
  userOwner: wallet.publicKey,
  payload,
  remainingAccounts,
});

const userSignature = signExecutionQueueIntentMessage(
  wallet.payer.secretKey,
  intent.userIntentMessage,
);

// 4. Build the Ed25519 pre-instruction
const ed25519Ix = Ed25519Program.createInstructionWithPrivateKey({
  privateKey: wallet.payer.secretKey,
  message: intent.userIntentMessage,
});

// 5. Build the enqueue_direct instruction
const enqueueDirectIx = /* build execution_queue_enqueue_direct instruction
   with payload, remaining_accounts, and reference to ed25519 preinstruction */;

// 6. Build and send the Solana transaction
const tx = new Transaction();
tx.add(ed25519Ix);
tx.add(enqueueDirectIx);
const sig = await client.sendAndConfirmTransactionForGroup(group, tx.instructions);
```

**Important:** The 10-slot delay means your order won't execute for ~4 seconds after the transaction lands. This is by design — it prevents race conditions with any in-flight relayer transactions.

### 2.4 Emergency operations

#### Cancel all orders (direct)

```typescript
// Option A: Via enqueue_direct (queued, 10-slot delay)
const cancelPayload = encodePerpCancelAllOrdersQueuePayload({ limit: 255 });
// ... sign and submit via enqueue_direct flow above

// Option B: Via direct instruction (immediate, no queue)
// If the program supports direct perp_cancel_all_orders outside the queue:
const cancelIx = await client.perpCancelAllOrdersIx(
  group, mangoAccount, perpMarket.perpMarketIndex, 255
);
await client.sendAndConfirmTransactionForGroup(group, [cancelIx]);
```

#### Close a position (direct)

```typescript
// Place a reduce-only market order for the opposite side of your position
const position = mangoAccount.getPerpPosition(perpMarket.perpMarketIndex);
const side = position.basePositionLots > 0 ? PerpOrderSide.ask : PerpOrderSide.bid;
const size = Math.abs(Number(position.basePositionLots));

const closePayload = encodePerpPlaceOrderV2QueuePayload({
  side,
  priceLots: side === PerpOrderSide.ask
    ? BigInt(1)                              // sell at any price
    : BigInt(perpMarket.uiPriceToLots(999999).toString()),  // buy at any price
  maxBaseLots: BigInt(size),
  maxQuoteLots: BigInt(perpMarket.uiQuoteToLots(size * 999999).toString()),
  clientOrderId: BigInt(Date.now()),
  orderType: PerpOrderType.immediateOrCancel,
  selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
  reduceOnly: true,     // CRITICAL: ensures this can only reduce, not flip
  expiryTimestamp: BigInt(0),
  limit: 50,
});
// ... submit via enqueue_direct
```

#### Withdraw funds (direct)

```typescript
// Via standard client method (direct chain transaction, no queue)
await client.tokenWithdraw(group, mangoAccount, usdcMint, amount, false /* allowBorrow */);
```

Or via the liquidity queue:
```typescript
// Enqueue a liquidity withdrawal (25-slot delay)
// This is permissionless at execute time
```

### 2.5 Running your own cranker

If the embedded executor is down, you can crank the queue yourself:

```typescript
// Poll the queue head and submit execute transactions
async function crankLoop() {
  while (true) {
    try {
      // Read queue state
      const queueAccount = await connection.getAccountInfo(executionQueuePk);
      // Parse to find next_sequence_to_execute and check if head item is pending

      // Build execute instruction with the head item's required accounts
      const executeIx = /* execution_queue_execute instruction */;

      await client.sendAndConfirmTransactionForGroup(group, [executeIx]);
    } catch (e) {
      // Queue empty, head missing, or already executed — normal, retry
    }
    await sleep(100); // 100ms poll interval
  }
}
```

The execute instruction is fully permissionless — anyone can submit it, and the only accounts that change are the queue account and the orderbook. The cranker pays the transaction fee but doesn't need any special authority.

### 2.6 Liquidation (always direct, never queued)

Liquidation is always a direct on-chain operation, regardless of whether the relayer is up:

```typescript
// Check if an account is liquidatable
const liqeeAccount = await client.getMangoAccount(liqeePk);
const health = liqeeAccount.getHealth(group, HealthType.Maint);

if (health.isNeg()) {
  // This account is liquidatable
  const liqIx = await client.perpLiqBaseOrPositivePnlIx(
    group,
    myMangoAccount,       // liquidator (you)
    liqeeAccount,         // account being liquidated
    perpMarket.perpMarketIndex,
    maxBaseLots,
    maxPnlTransfer,
  );
  await client.sendAndConfirmTransactionForGroup(group, [liqIx]);
}
```

---

## Quick Reference

### Endpoint summary

| Service | Port | Protocol | Purpose |
|---|---|---|---|
| Relayer | 9090 | gRPC | Order submission (SubmitIntent) |
| Harness | 9091 | HTTP | State reads (orderbook, positions, trades) |
| Bridge | 9092 | HTTP | REST wrapper for relayer gRPC |
| Relayer health | 9093 | HTTP | Health check + Prometheus metrics |
| Solana RPC | 8899 | HTTP/WS | Direct chain access (fallback) |

### Payload variant cheat sheet

| Action | Encoder function | Health-gated? |
|---|---|---|
| Place order | `encodePerpPlaceOrderV2QueuePayload` | Yes |
| Cancel by order ID | `encodePerpCancelOrderQueuePayload` | No |
| Cancel by client ID | `encodePerpCancelOrderByClientOrderIdQueuePayload` | No |
| Cancel all | `encodePerpCancelAllOrdersQueuePayload` | No |
| Cancel all (one side) | `encodePerpCancelAllOrdersBySideQueuePayload` | No |

### Timing reference

| Event | Expected latency |
|---|---|
| Intent → relayer response | 50-100ms |
| Intent → optimistic state update | 100-300ms |
| Intent → on-chain enqueue | 1.5-3s (1-2 slots) |
| Enqueue → execution | 1-3s (queue head processing) |
| Execution → confirmed state | 6-10 slots (2.4-4s) |
| Direct-enqueue → execution | 10 slots minimum (4s) |
| Liquidity item → execution | 25 slots minimum (10s) |

### Error handling patterns

| Error | Cause | Action |
|---|---|---|
| Relayer returns backpressure | `max_queued` exceeded | Exponential backoff, retry in 200-1000ms |
| Relayer returns RPC error | Solana RPC down or overloaded | Switch to backup RPC or fall back to direct-enqueue |
| Relayer unreachable | Service down | Fall back to direct-enqueue |
| Order not appearing in book | Health check failed on-chain | Check account health (confirmed view), add collateral |
| Sequence gap on-chain | Network jitter | Normal — queue auto-skips after gap_wait_slots |
| "Queue full" | 1024 pending items | Wait for queue to drain; reduce submission rate |
