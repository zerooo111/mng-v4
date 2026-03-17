use super::*;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::system_program;

fn encode_queue_payload_v1(variant: u8, body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + body.len());
    payload.push(1);
    payload.push(variant);
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(body);
    payload
}

fn encode_liquidity_deposit_payload(amount: u64, reduce_only: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&amount.to_le_bytes());
    body.push(u8::from(reduce_only));
    encode_queue_payload_v1(5, &body)
}

async fn create_initialized_execution_queue(
    solana: &SolanaCookie,
    group: Pubkey,
    admin: TestKeypair,
    payer: TestKeypair,
    ctm_signer: Pubkey,
) -> Pubkey {
    send_tx(
        solana,
        ExecutionQueueCreateInstruction {
            group,
            payer,
            admin,
        },
    )
    .await
    .unwrap();

    let queue = execution_queue_pda(group);
    while solana.get_account_data(queue).await.unwrap().len() < EXECUTION_QUEUE_ACCOUNT_SPACE {
        send_tx(
            solana,
            ExecutionQueueResizeInstruction {
                group,
                payer,
                admin,
            },
        )
        .await
        .unwrap();
    }

    send_tx(
        solana,
        ExecutionQueueInitInstruction {
            group,
            admin,
            ctm_signer,
        },
    )
    .await
    .unwrap();

    queue
}

fn dummy_dispatch_accounts() -> Vec<AccountMeta> {
    vec![AccountMeta {
        pubkey: system_program::id(),
        is_signer: false,
        is_writable: false,
    }]
}

#[tokio::test]
async fn test_execution_queue_lifecycle_and_liquidity_enqueue() -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let ctm_signer = TestKeypair::new().pubkey();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer).await;

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.group, group);
    assert_eq!(queue.admin, admin.pubkey());
    assert_eq!(queue.ctm_signer, ctm_signer);
    assert_eq!(queue.header.gap_wait_slots, 2);
    assert_eq!(queue.header.liquidity_delay_slots, 25);

    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 7,
            liquidity_delay_slots: 11,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

    let before_slot = solana.clock().await.slot;
    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(42, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    let head = queue.liquidity_head_item().unwrap();
    assert_eq!(queue.header.gap_wait_slots, 7);
    assert_eq!(queue.header.liquidity_delay_slots, 11);
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.liquidity_count, 1);
    assert_eq!(head.kind, QueueItemKind::LiquidityDeposit as u8);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);
    assert_eq!(head.sequence, 0);
    assert_eq!(head.payload_len as usize, encode_liquidity_deposit_payload(42, false).len());
    assert!(head.ingress_slot >= before_slot);
    assert_eq!(head.min_execute_slot, head.ingress_slot + 11);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_pause_flags_block_ingress_and_execute() -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, TestKeypair::new().pubkey())
            .await;
    let payload = encode_liquidity_deposit_payload(5, false);

    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 2,
            liquidity_delay_slots: 1,
            pause_ingress: true,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: payload.clone(),
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueueIngressPaused,
    );

    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 2,
            liquidity_delay_slots: 1,
            pause_ingress: false,
            pause_execute: true,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload,
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    send_tx_expect_error!(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueueExecutePaused,
    );

    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::CtmWrapped as u8,
            payload: encode_liquidity_deposit_payload(1, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueueInvalidItemKind,
    );

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_respects_liquidity_delay() -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, TestKeypair::new().pubkey())
            .await;

    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 2,
            liquidity_delay_slots: 5,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(7, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    solana.advance_by_slots(2).await;

    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    let head = queue.liquidity_head_item().unwrap();
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.liquidity_count, 1);
    assert_eq!(head.sequence, 0);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);
    assert_eq!(head.retries, 0);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_retries_and_drops_failed_liquidity(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, TestKeypair::new().pubkey())
            .await;

    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 2,
            liquidity_delay_slots: 1,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(9, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    for expected_retries in 1..5 {
        let current_slot = solana.clock().await.slot;
        let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
        let head = queue.liquidity_head_item().unwrap();
        if current_slot < head.min_execute_slot {
            solana
                .advance_by_slots(head.min_execute_slot - current_slot)
                .await;
        }

        send_tx(
            solana,
            ExecutionQueueExecuteInstruction {
                group,
                execution_queue,
                max_items: 1,
                remaining_accounts: dummy_dispatch_accounts(),
            },
        )
        .await
        .unwrap();

        let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
        let head = queue.liquidity_head_item().unwrap();
        assert_eq!(queue.header.total_count, 1);
        assert_eq!(queue.header.liquidity_count, 1);
        assert_eq!(head.sequence, 0);
        assert_eq!(head.status, QueueItemStatus::Pending as u8);
        assert_eq!(head.retries, expected_retries);
    }

    let current_slot = solana.clock().await.slot;
    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    let head = queue.liquidity_head_item().unwrap();
    if current_slot < head.min_execute_slot {
        solana
            .advance_by_slots(head.min_execute_slot - current_slot)
            .await;
    }

    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.liquidity_count, 0);
    assert!(queue.liquidity_head_item().is_none());

    Ok(())
}
