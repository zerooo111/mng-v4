use super::*;
use anchor_lang::prelude::Pubkey;
use anchor_lang::{InstructionData, ToAccountMetas};
use mango_v4::accounts_ix::{CompactOrderParamsArgs, EnqueuePerpEventArgs, PerpEnqueueOperationArgs};
use solana_sdk::ed25519_instruction::new_ed25519_instruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

async fn set_group_continuum(solana: &SolanaCookie, group: Pubkey, continuum: Pubkey) {
    let mut group_account = solana.get_account::<Group>(group).await;
    let bytes = continuum.to_bytes();
    group_account.reserved[0..bytes.len()].copy_from_slice(&bytes);
    solana.set_account(group, &group_account).await;
}

fn build_order_intent(
    group: Pubkey,
    account: Pubkey,
    owner: Pubkey,
    perp_market: Pubkey,
    market_index: PerpMarketIndex,
    event_type: QueueEventType,
    client_order_id: u64,
    price_lots: i64,
    expiry_ts: u64,
) -> OrderIntent {
    OrderIntent {
        version: 0,
        group,
        account,
        owner,
        perp_market,
        event_type,
        params: CompactOrderParamsPayload {
            market_index,
            side: Side::Bid as u8,
            order_type: PlaceOrderType::Limit as u8,
            self_trade_behavior: SelfTradeBehavior::DecrementTake as u8,
            reduce_only: 0,
            limit: 10,
            tif_offset: 0,
            max_base_lots: 1,
            max_quote_lots: 10,
            price_lots,
            client_order_id,
        },
        expiry_ts,
    }
}

fn build_queued_intent(order_intent: &OrderIntent, seq_no: u64, expiry_ts: u64) -> QueuedOrderIntent {
    QueuedOrderIntent {
        version: 0,
        order_intent_hash: order_intent.intent_hash().unwrap(),
        seq_no,
        expiry_ts,
    }
}

fn build_perp_enqueue_operation_ix(
    group: Pubkey,
    queue: Pubkey,
    account: Pubkey,
    perp_market: Pubkey,
    order_intent: OrderIntent,
    queued_order_intent: QueuedOrderIntent,
) -> Instruction {
    let accounts = mango_v4::accounts::PerpEnqueueOperation {
        group,
        queue,
        account,
        perp_market,
        instructions: solana_program::sysvar::instructions::id(),
    };
    Instruction {
        program_id: mango_v4::id(),
        accounts: accounts.to_account_metas(None),
        data: mango_v4::instruction::PerpEnqueueOperation {
            args: PerpEnqueueOperationArgs {
                order_intent,
                queued_order_intent,
            },
        }
        .data(),
    }
}

fn build_enqueue_perp_event_ix(
    group: Pubkey,
    queue: Pubkey,
    perp_market: Pubkey,
    args: EnqueuePerpEventArgs,
) -> Instruction {
    let accounts = mango_v4::accounts::EnqueuePerpEvent {
        group,
        queue,
        perp_market,
        instructions: solana_program::sysvar::instructions::id(),
    };
    Instruction {
        program_id: mango_v4::id(),
        accounts: accounts.to_account_metas(None),
        data: mango_v4::instruction::EnqueuePerpEvent { args }.data(),
    }
}

fn build_perp_crank_ix(
    group: Pubkey,
    queue: Pubkey,
    account: Pubkey,
    perp_market: Pubkey,
    bids: Pubkey,
    asks: Pubkey,
    event_queue: Pubkey,
    oracle: Pubkey,
    max_operations: u8,
) -> Instruction {
    let accounts = mango_v4::accounts::PerpCrankQueuedOperations {
        group,
        queue,
        account,
        perp_market,
        bids,
        asks,
        event_queue,
        oracle,
    };
    Instruction {
        program_id: mango_v4::id(),
        accounts: accounts.to_account_metas(None),
        data: mango_v4::instruction::PerpCrankQueuedOperations { max_operations }.data(),
    }
}

#[tokio::test]
async fn test_perp_queue_enqueue_and_crank_order() -> Result<(), TransportError> {
    let context = TestContext::new().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..2];

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(
        solana,
        group,
        owner,
        0,
        &context.users[1],
        mints,
        1_000,
        0,
    )
    .await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        bids,
        asks,
        event_queue,
        queue,
        ..
    } = send_tx(
        solana,
        PerpCreateMarketInstruction {
            group,
            admin,
            payer,
            perp_market_index: 0,
            quote_lot_size: 10,
            base_lot_size: 100,
            maint_base_asset_weight: 0.975,
            init_base_asset_weight: 0.95,
            maint_base_liab_weight: 1.025,
            init_base_liab_weight: 1.05,
            base_liquidation_fee: 0.012,
            maker_fee: -0.0001,
            taker_fee: 0.0002,
            settle_pnl_limit_factor: -1.0,
            settle_pnl_limit_window_size_ts: 24 * 60 * 60,
            ..PerpCreateMarketInstruction::with_new_book_and_queue(solana, &tokens[0]).await
        },
    )
    .await
    .unwrap();

    let continuum = Keypair::new();
    set_group_continuum(solana, group, continuum.pubkey()).await;

    let order_intent = build_order_intent(
        group,
        account,
        owner.pubkey(),
        perp_market,
        0,
        QueueEventType::PerpPlaceOrder,
        7,
        123,
        0,
    );
    let queued_order_intent = build_queued_intent(&order_intent, 1, 0);

    let order_bytes = order_intent.message_bytes().unwrap();
    let queued_bytes = queued_order_intent.message_bytes().unwrap();

    let mut tx = ClientTransaction::new(solana);
    tx.add_instruction_direct(new_ed25519_instruction(&owner.to_keypair(), &order_bytes));
    tx.add_instruction_direct(new_ed25519_instruction(&continuum, &queued_bytes));
    tx.add_instruction_direct(build_perp_enqueue_operation_ix(
        group,
        queue,
        account,
        perp_market,
        order_intent,
        queued_order_intent,
    ));
    tx.send().await.unwrap();

    let mut crank_tx = ClientTransaction::new(solana);
    crank_tx.add_instruction_direct(build_perp_crank_ix(
        group,
        queue,
        account,
        perp_market,
        bids,
        asks,
        event_queue,
        tokens[0].oracle,
        1,
    ));
    crank_tx.send().await.unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    let queue_data = solana.get_account::<QueueFifo>(queue).await;
    assert_eq!(queue_data.header.count, 0);

    let perp_market_data = solana.get_account::<PerpMarket>(perp_market).await;
    assert_eq!(perp_market_data.queue_last_executed_seq, 1);

    Ok(())
}

#[tokio::test]
async fn test_perp_queue_gap_skip_and_cancel() -> Result<(), TransportError> {
    let context = TestContext::new().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..2];

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(
        solana,
        group,
        owner,
        0,
        &context.users[1],
        mints,
        1_000,
        0,
    )
    .await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        bids,
        asks,
        event_queue,
        queue,
        ..
    } = send_tx(
        solana,
        PerpCreateMarketInstruction {
            group,
            admin,
            payer,
            perp_market_index: 0,
            quote_lot_size: 10,
            base_lot_size: 100,
            maint_base_asset_weight: 0.975,
            init_base_asset_weight: 0.95,
            maint_base_liab_weight: 1.025,
            init_base_liab_weight: 1.05,
            base_liquidation_fee: 0.012,
            maker_fee: -0.0001,
            taker_fee: 0.0002,
            settle_pnl_limit_factor: -1.0,
            settle_pnl_limit_window_size_ts: 24 * 60 * 60,
            ..PerpCreateMarketInstruction::with_new_book_and_queue(solana, &tokens[0]).await
        },
    )
    .await
    .unwrap();

    let continuum = Keypair::new();
    set_group_continuum(solana, group, continuum.pubkey()).await;

    let mut queue_data = solana.get_account::<QueueFifo>(queue).await;
    queue_data.header.max_lag_slots = 0;
    solana.set_account(queue, &queue_data).await;

    let place_intent = build_order_intent(
        group,
        account,
        owner.pubkey(),
        perp_market,
        0,
        QueueEventType::PerpPlaceOrder,
        9,
        125,
        0,
    );
    let place_queued = build_queued_intent(&place_intent, 1, 0);
    let mut tx = ClientTransaction::new(solana);
    tx.add_instruction_direct(new_ed25519_instruction(
        &owner.to_keypair(),
        &place_intent.message_bytes().unwrap(),
    ));
    tx.add_instruction_direct(new_ed25519_instruction(
        &continuum,
        &place_queued.message_bytes().unwrap(),
    ));
    tx.add_instruction_direct(build_perp_enqueue_operation_ix(
        group,
        queue,
        account,
        perp_market,
        place_intent,
        place_queued,
    ));
    tx.send().await.unwrap();

    let cancel_intent = build_order_intent(
        group,
        account,
        owner.pubkey(),
        perp_market,
        0,
        QueueEventType::PerpCancelOrder,
        9,
        0,
        0,
    );
    let cancel_queued = build_queued_intent(&cancel_intent, 3, 0);
    let mut cancel_tx = ClientTransaction::new(solana);
    cancel_tx.add_instruction_direct(new_ed25519_instruction(
        &owner.to_keypair(),
        &cancel_intent.message_bytes().unwrap(),
    ));
    cancel_tx.add_instruction_direct(new_ed25519_instruction(
        &continuum,
        &cancel_queued.message_bytes().unwrap(),
    ));
    cancel_tx.add_instruction_direct(build_perp_enqueue_operation_ix(
        group,
        queue,
        account,
        perp_market,
        cancel_intent,
        cancel_queued,
    ));
    cancel_tx.send().await.unwrap();

    let mut crank_tx = ClientTransaction::new(solana);
    crank_tx.add_instruction_direct(build_perp_crank_ix(
        group,
        queue,
        account,
        perp_market,
        bids,
        asks,
        event_queue,
        tokens[0].oracle,
        2,
    ));
    crank_tx.send().await.unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);

    let queue_data = solana.get_account::<QueueFifo>(queue).await;
    assert_eq!(queue_data.header.count, 0);

    let perp_market_data = solana.get_account::<PerpMarket>(perp_market).await;
    assert_eq!(perp_market_data.queue_last_executed_seq, 3);

    Ok(())
}

#[tokio::test]
async fn test_perp_queue_failures() -> Result<(), TransportError> {
    let context = TestContext::new().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..2];

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(
        solana,
        group,
        owner,
        0,
        &context.users[1],
        mints,
        1_000,
        0,
    )
    .await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        queue,
        ..
    } = send_tx(
        solana,
        PerpCreateMarketInstruction {
            group,
            admin,
            payer,
            perp_market_index: 0,
            quote_lot_size: 10,
            base_lot_size: 100,
            maint_base_asset_weight: 0.975,
            init_base_asset_weight: 0.95,
            maint_base_liab_weight: 1.025,
            init_base_liab_weight: 1.05,
            base_liquidation_fee: 0.012,
            maker_fee: -0.0001,
            taker_fee: 0.0002,
            settle_pnl_limit_factor: -1.0,
            settle_pnl_limit_window_size_ts: 24 * 60 * 60,
            ..PerpCreateMarketInstruction::with_new_book_and_queue(solana, &tokens[0]).await
        },
    )
    .await
    .unwrap();

    let continuum = Keypair::new();
    set_group_continuum(solana, group, continuum.pubkey()).await;

    let now_ts = solana.clock_timestamp().await;
    let expired_intent = build_order_intent(
        group,
        account,
        owner.pubkey(),
        perp_market,
        0,
        QueueEventType::PerpPlaceOrder,
        11,
        130,
        now_ts.saturating_sub(1),
    );
    let expired_queued = build_queued_intent(&expired_intent, 1, 0);
    let mut expired_tx = ClientTransaction::new(solana);
    expired_tx.add_instruction_direct(new_ed25519_instruction(
        &owner.to_keypair(),
        &expired_intent.message_bytes().unwrap(),
    ));
    expired_tx.add_instruction_direct(new_ed25519_instruction(
        &continuum,
        &expired_queued.message_bytes().unwrap(),
    ));
    expired_tx.add_instruction_direct(build_perp_enqueue_operation_ix(
        group,
        queue,
        account,
        perp_market,
        expired_intent,
        expired_queued,
    ));
    expired_tx
        .send_expect_error(MangoError::OrderIntentExpired)
        .await
        .unwrap();

    let place_intent = build_order_intent(
        group,
        account,
        owner.pubkey(),
        perp_market,
        0,
        QueueEventType::PerpPlaceOrder,
        13,
        131,
        0,
    );
    let place_queued = build_queued_intent(&place_intent, 1, 0);
    let mut place_tx = ClientTransaction::new(solana);
    place_tx.add_instruction_direct(new_ed25519_instruction(
        &owner.to_keypair(),
        &place_intent.message_bytes().unwrap(),
    ));
    place_tx.add_instruction_direct(new_ed25519_instruction(
        &continuum,
        &place_queued.message_bytes().unwrap(),
    ));
    place_tx.add_instruction_direct(build_perp_enqueue_operation_ix(
        group,
        queue,
        account,
        perp_market,
        place_intent,
        place_queued,
    ));
    place_tx.send().await.unwrap();

    let out_of_order_intent = build_order_intent(
        group,
        account,
        owner.pubkey(),
        perp_market,
        0,
        QueueEventType::PerpPlaceOrder,
        14,
        132,
        0,
    );
    let out_of_order_queued = build_queued_intent(&out_of_order_intent, 1, 0);
    let mut out_of_order_tx = ClientTransaction::new(solana);
    out_of_order_tx.add_instruction_direct(new_ed25519_instruction(
        &owner.to_keypair(),
        &out_of_order_intent.message_bytes().unwrap(),
    ));
    out_of_order_tx.add_instruction_direct(new_ed25519_instruction(
        &continuum,
        &out_of_order_queued.message_bytes().unwrap(),
    ));
    out_of_order_tx.add_instruction_direct(build_perp_enqueue_operation_ix(
        group,
        queue,
        account,
        perp_market,
        out_of_order_intent,
        out_of_order_queued,
    ));
    out_of_order_tx
        .send_expect_error(MangoError::SequenceNumberTooLow)
        .await
        .unwrap();

    let invalid_queue_event = QueueFifoEvent {
        seq_no: 2,
        submitted_slot: 0,
        user: owner.pubkey(),
        continuum: continuum.pubkey(),
        event_type: QueueEventType::PerpPlaceOrder as u8,
        padding: [0u8; 7],
        params: CompactOrderParamsArgs {
            market_index: 0,
            side: Side::Bid as u8,
            order_type: PlaceOrderType::Limit as u8,
            self_trade_behavior: SelfTradeBehavior::DecrementTake as u8,
            reduce_only: 0,
            limit: 10,
            tif_offset: 0,
            max_base_lots: 1,
            max_quote_lots: 10,
            price_lots: 140,
            client_order_id: 21,
        }
        .into(),
        user_signature: SignatureBlob {
            bytes: [0u8; 64],
            is_present: 1,
            padding: [0u8; 7],
        },
        continuum_signature: SignatureBlob {
            bytes: [1u8; 64],
            is_present: 1,
            padding: [0u8; 7],
        },
    };
    let intent_hash = invalid_queue_event.intent_hash();
    let mut invalid_signature_tx = ClientTransaction::new(solana);
    invalid_signature_tx.add_instruction_direct(new_ed25519_instruction(
        &owner.to_keypair(),
        &intent_hash,
    ));
    invalid_signature_tx.add_instruction_direct(build_enqueue_perp_event_ix(
        group,
        queue,
        perp_market,
        EnqueuePerpEventArgs {
            seq_no: 2,
            user: owner.pubkey(),
            event_type: QueueEventType::PerpPlaceOrder,
            params: CompactOrderParamsArgs {
                market_index: 0,
                side: Side::Bid as u8,
                order_type: PlaceOrderType::Limit as u8,
                self_trade_behavior: SelfTradeBehavior::DecrementTake as u8,
                reduce_only: 0,
                limit: 10,
                tif_offset: 0,
                max_base_lots: 1,
                max_quote_lots: 10,
                price_lots: 140,
                client_order_id: 21,
            },
            user_signature: [0u8; 64],
            continuum_signature: [1u8; 64],
        },
    ));
    invalid_signature_tx
        .send_expect_error(MangoError::InvalidContinuumSignature)
        .await
        .unwrap();

    Ok(())
}
