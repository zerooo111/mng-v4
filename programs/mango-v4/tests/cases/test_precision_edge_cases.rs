use super::*;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::AnchorSerialize;
use mango_v4::instructions::CtmEnvelope;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signature::Signer;
use solana_sdk::system_program;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::sysvar;
use std::collections::HashMap;

// ── Duplicated helpers ──

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

fn dummy_dispatch_accounts() -> Vec<AccountMeta> {
    vec![AccountMeta {
        pubkey: system_program::id(),
        is_signer: false,
        is_writable: false,
    }]
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

/// Deposit exactly 1 lamport via the execution queue. Should succeed.
#[tokio::test]
async fn test_precision_deposit_1_lamport() -> Result<(), TransportError> {
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

    let execution_queue = create_initialized_execution_queue(
        solana,
        group,
        admin,
        payer,
        TestKeypair::new().pubkey(),
    )
    .await;

    // Configure short delays for testing
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

    // Enqueue a deposit of exactly 1 lamport
    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(1, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.liquidity_count, 1);
    let head = queue.liquidity_head_item().unwrap();
    assert_eq!(head.status, QueueItemStatus::Pending as u8);

    Ok(())
}

/// Deposit 0 amount via the execution queue. The enqueue itself should succeed
/// (it just stores the payload). Behavior on execute depends on the dispatch
/// handler, but the enqueue should not reject a zero-amount payload.
#[tokio::test]
async fn test_precision_zero_amount_deposit() -> Result<(), TransportError> {
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

    let execution_queue = create_initialized_execution_queue(
        solana,
        group,
        admin,
        payer,
        TestKeypair::new().pubkey(),
    )
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

    // Enqueue a deposit of 0 amount
    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(0, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.liquidity_count, 1);
    let head = queue.liquidity_head_item().unwrap();
    assert_eq!(head.status, QueueItemStatus::Pending as u8);
    assert_eq!(head.sequence, 0);

    Ok(())
}
