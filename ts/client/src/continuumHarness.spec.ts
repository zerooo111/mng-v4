import { expect } from 'chai';
import { Keypair } from '@solana/web3.js';
import {
  ContinuumStateEngine,
  QueueProcessStatus,
  decodeQueuePayload,
} from './continuumHarness';
import {
  QueuePlaceOrderType,
  QueueSelfTradeBehavior,
  QueueSide,
  encodePerpCancelAllOrdersQueuePayload,
  encodePerpPlaceOrderV2QueuePayload,
} from './executionQueue';

function key(): string {
  return Keypair.generate().publicKey.toBase58();
}

describe('Continuum State Harness', () => {
  it('decodes v1 place-order queue payload', () => {
    const payload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Bid,
      priceLots: 101,
      maxBaseLots: 5,
      maxQuoteLots: 200,
      clientOrderId: 77,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });

    const decoded = decodeQueuePayload(payload);
    expect(decoded.variant).eq(0);
    if (decoded.variant !== 0) {
      throw new Error('expected place-order variant');
    }
    expect(decoded.price_lots.toString()).eq('101');
    expect(decoded.max_base_lots.toString()).eq('5');
    expect(decoded.client_order_id.toString()).eq('77');
    expect(decoded.limit).eq(10);
  });

  it('builds optimistic and confirmed views from relay + processed events', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '0';

    const placePayload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Bid,
      priceLots: 100,
      maxBaseLots: 2,
      maxQuoteLots: 100,
      clientOrderId: 1,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '1',
      kind: 0,
      payload_b64: placePayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '10',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'tx-enqueue-1',
    });

    const confirmedBefore = engine.getMarketState(market, 'confirmed');
    const optimisticBefore = engine.getMarketState(market, 'optimistic');

    expect(confirmedBefore.open_orders.length).eq(0);
    expect(optimisticBefore.open_orders.length).eq(1);

    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 2,
      group,
      sequence: '1',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '20',
      tx_signature: 'tx-exec-1',
    });

    const confirmedAfter = engine.getMarketState(market, 'confirmed');
    expect(confirmedAfter.open_orders.length).eq(1);

    const cancelAllPayload = encodePerpCancelAllOrdersQueuePayload({
      limit: 20,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 3,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '2',
      kind: 0,
      payload_b64: cancelAllPayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '10',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'tx-enqueue-2',
    });
    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 4,
      group,
      sequence: '2',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '21',
      tx_signature: 'tx-exec-2',
    });

    const confirmedFinal = engine.getMarketState(market, 'confirmed');
    expect(confirmedFinal.open_orders.length).eq(0);
  });

  it('replays deterministically regardless of ingestion order', () => {
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '7';

    const placePayload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Ask,
      priceLots: 88,
      maxBaseLots: 3,
      maxQuoteLots: 100,
      clientOrderId: 44,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });
    const cancelAllPayload = encodePerpCancelAllOrdersQueuePayload({ limit: 10 });

    const ordered = new ContinuumStateEngine();
    ordered.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '1',
      kind: 0,
      payload_b64: placePayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'a',
    });
    ordered.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 2,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '2',
      kind: 0,
      payload_b64: cancelAllPayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'b',
    });
    ordered.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 3,
      group,
      sequence: '1',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '10',
      tx_signature: 'c',
    });
    ordered.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 4,
      group,
      sequence: '2',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '11',
      tx_signature: 'd',
    });

    const outOfOrder = new ContinuumStateEngine();
    outOfOrder.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 2,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '2',
      kind: 0,
      payload_b64: cancelAllPayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'b',
    });
    outOfOrder.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 4,
      group,
      sequence: '2',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '11',
      tx_signature: 'd',
    });
    outOfOrder.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '1',
      kind: 0,
      payload_b64: placePayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'a',
    });
    outOfOrder.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 3,
      group,
      sequence: '1',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '10',
      tx_signature: 'c',
    });

    const orderedSnapshot = ordered.getSnapshot('confirmed');
    const outOfOrderSnapshot = outOfOrder.getSnapshot('confirmed');
    expect({
      ...outOfOrderSnapshot,
      generated_ts_ms: 0,
    }).deep.eq({
      ...orderedSnapshot,
      generated_ts_ms: 0,
    });
  });

  it('tracks divergences for processed events without relay acceptance', () => {
    const engine = new ContinuumStateEngine();
    const group = key();

    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: Date.now(),
      group,
      sequence: '99',
      kind: 0,
      status: QueueProcessStatus.Failed,
      slot: '123',
      tx_signature: 'unknown-processed',
    });

    const divergences = engine.listDivergences();
    expect(divergences.length).eq(1);
    expect(divergences[0].reason).eq('processed_without_relay_intent');

    const queue = engine.getQueueState('unknown');
    expect(queue.unmatched_processed_count).eq(1);
  });

  it('builds trade prints and candles from crossing executed orders', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const makerOwner = key();
    const takerOwner = key();
    const makerMango = key();
    const takerMango = key();
    const market = '0';

    const makerBid = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Bid,
      priceLots: 100,
      maxBaseLots: 2,
      maxQuoteLots: 500,
      clientOrderId: 1,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });
    const takerAsk = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Ask,
      priceLots: 99,
      maxBaseLots: 1,
      maxQuoteLots: 500,
      clientOrderId: 2,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1000,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '1',
      kind: 0,
      payload_b64: makerBid.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: makerOwner,
      mango_account: makerMango,
      enqueue_tx_signature: 'mk',
    });
    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 2000,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '2',
      kind: 0,
      payload_b64: takerAsk.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: takerOwner,
      mango_account: takerMango,
      enqueue_tx_signature: 'tk',
    });
    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 3000,
      group,
      sequence: '1',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '10',
      tx_signature: 'mk-exec',
    });
    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 4000,
      group,
      sequence: '2',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '11',
      tx_signature: 'tk-exec',
    });

    const trades = engine.getTrades(market, 'confirmed', 10);
    expect(trades.length).eq(1);
    expect(trades[0].price_lots).eq('100');
    expect(trades[0].base_lots).eq('1');
    expect(trades[0].taker_side).eq('ask');

    const candles = engine.getCandles(market, 'confirmed', 60, 10);
    expect(candles.length).eq(1);
    expect(candles[0].open_price_lots).eq('100');
    expect(candles[0].close_price_lots).eq('100');
    expect(candles[0].base_volume_lots).eq('1');

    const balances = engine.getBalances(makerOwner, 'confirmed');
    expect(balances.totals.total_open_order_base_lots_bid).eq('1');
  });

  it('deduplicates repeated processed events and tracks skipped queue items', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '12';

    const placePayload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Bid,
      priceLots: 100,
      maxBaseLots: 1,
      maxQuoteLots: 100,
      clientOrderId: 5,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '5',
      kind: 0,
      payload_b64: placePayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'enqueue-5',
    });

    const processedEvent = {
      event_type: 'queue_item_processed' as const,
      ts_ms: 2,
      group,
      sequence: '5',
      kind: 0,
      status: QueueProcessStatus.Skipped,
      slot: '33',
      tx_signature: 'skip-5',
    };
    engine.ingestQueueProcessed(processedEvent);
    engine.ingestQueueProcessed(processedEvent);

    const queue = engine.getQueueState(market);
    expect(queue.processed_count).eq(1);
    expect(queue.skipped_count).eq(1);
    expect(queue.failed_count).eq(0);
    expect(queue.pending_count).eq(0);

    const confirmed = engine.getMarketState(market, 'confirmed');
    expect(confirmed.open_orders.length).eq(0);
  });

  it('removes optimistic queued place orders after a failed processed status', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '14';

    const placePayload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Bid,
      priceLots: 101,
      maxBaseLots: 4,
      maxQuoteLots: 1000,
      clientOrderId: 9,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '9',
      kind: 0,
      payload_b64: placePayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '5',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'enqueue-9',
    });

    const optimisticBefore = engine.getMarketState(market, 'optimistic');
    expect(optimisticBefore.open_orders.length).eq(1);
    expect(optimisticBefore.open_orders[0].client_order_id).eq('9');

    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 2,
      group,
      sequence: '9',
      kind: 0,
      status: QueueProcessStatus.Failed,
      slot: '55',
      tx_signature: 'failed-9',
    });

    const optimisticAfter = engine.getMarketState(market, 'optimistic');
    const confirmedAfter = engine.getMarketState(market, 'confirmed');
    const queue = engine.getQueueState(market);

    expect(optimisticAfter.open_orders.length).eq(0);
    expect(confirmedAfter.open_orders.length).eq(0);
    expect(queue.pending_count).eq(0);
    expect(queue.processed_count).eq(1);
    expect(queue.failed_count).eq(1);
    expect(queue.skipped_count).eq(0);
  });

  it('emits a divergence when processed status changes for the same queue item', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '13';

    const cancelPayload = encodePerpCancelAllOrdersQueuePayload({
      limit: 10,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '8',
      kind: 0,
      payload_b64: cancelPayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '1',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'enqueue-8',
    });

    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 2,
      group,
      sequence: '8',
      kind: 0,
      status: QueueProcessStatus.Failed,
      slot: '41',
      tx_signature: 'failed-8',
    });
    engine.ingestQueueProcessed({
      event_type: 'queue_item_processed',
      ts_ms: 3,
      group,
      sequence: '8',
      kind: 0,
      status: QueueProcessStatus.Executed,
      slot: '42',
      tx_signature: 'executed-8',
    });

    const divergences = engine
      .listDivergences()
      .filter((d) => d.reason === 'processed_status_changed');
    expect(divergences.length).eq(1);
    expect(divergences[0].key).eq(`${group}:8:0`);
  });

  it('state_projection_after_enqueue', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '20';

    const placePayload = encodePerpPlaceOrderV2QueuePayload({
      side: QueueSide.Ask,
      priceLots: 55,
      maxBaseLots: 10,
      maxQuoteLots: 1000,
      clientOrderId: 42,
      orderType: QueuePlaceOrderType.Limit,
      selfTradeBehavior: QueueSelfTradeBehavior.DecrementTake,
      reduceOnly: false,
      expiryTimestamp: 0,
      limit: 10,
    });

    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '1',
      kind: 0,
      payload_b64: placePayload.toString('base64'),
      remaining_accounts: [],
      min_execute_slot: '10',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'tx-enqueue-state-1',
    });

    const optimistic = engine.getMarketState(market, 'optimistic');
    expect(optimistic.open_orders.length).eq(1);
    expect(optimistic.open_orders[0].side).eq('ask');
    expect(optimistic.open_orders[0].price_lots).eq('55');
    expect(optimistic.open_orders[0].base_lots).eq('10');
    expect(optimistic.open_orders[0].client_order_id).eq('42');

    const confirmed = engine.getMarketState(market, 'confirmed');
    expect(confirmed.open_orders.length).eq(0);
  });

  it('malformed_payload_graceful_error', () => {
    const engine = new ContinuumStateEngine();
    const group = key();
    const executionQueue = key();
    const owner = key();
    const mangoAccount = key();
    const market = '21';

    // Use invalid base64 that decodes to bytes too short / invalid for any variant
    const badPayload = Buffer.from([0xff, 0xff]);
    const badB64 = badPayload.toString('base64');

    // Should not throw
    engine.ingestRelayIntent({
      event_type: 'relay_intent_accepted',
      ts_ms: 1,
      group,
      execution_queue: executionQueue,
      market,
      sequence: '1',
      kind: 0,
      payload_b64: badB64,
      remaining_accounts: [],
      min_execute_slot: '10',
      expires_at_slot: '0',
      user_owner: owner,
      mango_account: mangoAccount,
      enqueue_tx_signature: 'tx-malformed-1',
    });

    // The intent should be tracked (decoded_payload will be null due to decode failure)
    const intents = engine.listIntents();
    expect(intents.length).eq(1);
    expect(intents[0].decoded_payload).eq(null);

    // Optimistic state should have no orders since the payload couldn't be decoded
    const optimistic = engine.getMarketState(market, 'optimistic');
    expect(optimistic.open_orders.length).eq(0);
  });
});
