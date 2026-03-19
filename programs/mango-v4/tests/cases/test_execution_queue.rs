use super::*;
use anchor_lang::AnchorSerialize;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::sysvar;
use mango_v4::instructions::CtmEnvelope;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signature::Signer;
use solana_sdk::system_program;
use std::collections::HashMap;

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

fn encode_perp_cancel_all_orders_payload(limit: u8) -> Vec<u8> {
    encode_queue_payload_v1(3, &[limit])
}

fn encode_perp_place_order_v2_payload(
    side: Side,
    price_lots: i64,
    max_base_lots: i64,
    max_quote_lots: i64,
    client_order_id: u64,
    order_type: PlaceOrderType,
    self_trade_behavior: SelfTradeBehavior,
    reduce_only: bool,
    expiry_timestamp: u64,
    limit: u8,
) -> Vec<u8> {
    let body = mango_v4::instructions::PerpPlaceOrderV2Payload {
        side,
        price_lots,
        max_base_lots,
        max_quote_lots,
        client_order_id,
        order_type,
        self_trade_behavior,
        reduce_only,
        expiry_timestamp,
        limit,
    }
    .try_to_vec()
    .unwrap();
    encode_queue_payload_v1(0, &body)
}

fn anchor_discriminator(ix_name: &str) -> [u8; 8] {
    let mut preimage = b"global:".to_vec();
    preimage.extend_from_slice(ix_name.as_bytes());
    let digest = anchor_lang::solana_program::hash::hash(&preimage);
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&digest.to_bytes()[..8]);
    discriminator
}

fn hash_accounts(accounts: &[AccountMeta]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(accounts.len() * 34);
    for account in accounts {
        bytes.extend_from_slice(account.pubkey.as_ref());
        bytes.push(u8::from(account.is_signer));
        bytes.push(u8::from(account.is_writable));
    }
    hashv(&[&bytes]).to_bytes()
}

fn hash_accounts_with_fixed_runtime_flags(
    remaining_accounts: &[AccountMeta],
    fixed_accounts: &[AccountMeta],
) -> [u8; 32] {
    let mut merged = HashMap::<Pubkey, (bool, bool)>::new();
    for account in fixed_accounts.iter().chain(remaining_accounts.iter()) {
        let entry = merged
            .entry(account.pubkey)
            .or_insert((account.is_signer, account.is_writable));
        entry.0 |= account.is_signer;
        entry.1 |= account.is_writable;
    }

    let effective_accounts = remaining_accounts
        .iter()
        .map(|account| {
            let (is_signer, is_writable) = merged
                .get(&account.pubkey)
                .copied()
                .unwrap_or((account.is_signer, account.is_writable));
            AccountMeta {
                pubkey: account.pubkey,
                is_signer,
                is_writable,
            }
        })
        .collect::<Vec<_>>();

    hash_accounts(&effective_accounts)
}

fn canonical_envelope_message(group: Pubkey, envelope: &CtmEnvelope) -> [u8; 32] {
    hashv(&[
        b"mango-v4-ctm-envelope-v1",
        group.as_ref(),
        &envelope.sequence.to_le_bytes(),
        &envelope.min_execute_slot.to_le_bytes(),
        &[envelope.kind],
        &envelope.payload_hash,
        &envelope.accounts_hash,
        &envelope.expires_at_slot.to_le_bytes(),
    ])
    .to_bytes()
}

fn canonical_user_intent_message(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    envelope: &CtmEnvelope,
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-user-intent-v1",
        group.as_ref(),
        mango_account.as_ref(),
        user_owner.as_ref(),
        &[envelope.kind],
        &envelope.payload_hash,
        &envelope.accounts_hash,
    ])
    .to_bytes()
}

fn build_execution_queue_enqueue_ctm_instruction(
    group: Pubkey,
    execution_queue: Pubkey,
    envelope: CtmEnvelope,
    payload: Vec<u8>,
    remaining_accounts: Vec<AccountMeta>,
) -> Instruction {
    let mut data = anchor_discriminator("execution_queue_enqueue_ctm").to_vec();
    data.extend(envelope.try_to_vec().unwrap());
    data.extend(payload.try_to_vec().unwrap());

    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(execution_queue, false),
        AccountMeta::new_readonly(sysvar::instructions::id(), false),
    ];
    accounts.extend(remaining_accounts);

    Instruction {
        program_id: mango_v4::id(),
        accounts,
        data,
    }
}

fn build_execution_queue_set_ctm_pending_instruction(
    group: Pubkey,
    execution_queue: Pubkey,
    admin: Pubkey,
    pending_ctm_signer: Pubkey,
    activate_at_slot: u64,
) -> Instruction {
    let mut data = anchor_discriminator("execution_queue_set_ctm_pending").to_vec();
    data.extend_from_slice(pending_ctm_signer.as_ref());
    data.extend_from_slice(&activate_at_slot.to_le_bytes());
    Instruction {
        program_id: mango_v4::id(),
        accounts: vec![
            AccountMeta::new_readonly(group, false),
            AccountMeta::new(execution_queue, false),
            AccountMeta::new_readonly(admin, true),
        ],
        data,
    }
}

fn build_execution_queue_drop_ctm_instruction(
    group: Pubkey,
    execution_queue: Pubkey,
    admin: Pubkey,
    sequence: u64,
) -> Instruction {
    let mut data = anchor_discriminator("execution_queue_drop_ctm").to_vec();
    data.extend_from_slice(&sequence.to_le_bytes());
    Instruction {
        program_id: mango_v4::id(),
        accounts: vec![
            AccountMeta::new_readonly(group, false),
            AccountMeta::new(execution_queue, false),
            AccountMeta::new_readonly(admin, true),
        ],
        data,
    }
}

fn build_execution_queue_execute_multi_instruction(
    group: Pubkey,
    execution_queue: Pubkey,
    max_items: u16,
    lane_count: u8,
    accounts_per_lane: u16,
    lane_hashes: Vec<[u8; 32]>,
    remaining_accounts: Vec<AccountMeta>,
) -> Instruction {
    let mut data = anchor_discriminator("execution_queue_execute_multi").to_vec();
    data.extend_from_slice(&max_items.to_le_bytes());
    data.push(lane_count);
    data.extend_from_slice(&accounts_per_lane.to_le_bytes());
    data.extend_from_slice(&(lane_hashes.len() as u32).to_le_bytes());
    for lane_hash in lane_hashes {
        data.extend_from_slice(&lane_hash);
    }
    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(execution_queue, false),
    ];
    accounts.extend(remaining_accounts);
    Instruction {
        program_id: mango_v4::id(),
        accounts,
        data,
    }
}

fn build_presigned_ed25519_instruction(
    public_key: [u8; 32],
    message: &[u8],
    signature: [u8; 64],
) -> Instruction {
    let public_key_offset = 16u16;
    let signature_offset = public_key_offset + public_key.len() as u16;
    let message_data_offset = signature_offset + signature.len() as u16;
    let mut data = vec![0u8; message_data_offset as usize + message.len()];
    data[0] = 1;
    data[1] = 0;
    data[2..4].copy_from_slice(&signature_offset.to_le_bytes());
    data[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
    data[6..8].copy_from_slice(&public_key_offset.to_le_bytes());
    data[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    data[10..12].copy_from_slice(&message_data_offset.to_le_bytes());
    data[12..14].copy_from_slice(&(message.len() as u16).to_le_bytes());
    data[14..16].copy_from_slice(&u16::MAX.to_le_bytes());
    data[public_key_offset as usize..signature_offset as usize].copy_from_slice(&public_key);
    data[signature_offset as usize..message_data_offset as usize].copy_from_slice(&signature);
    data[message_data_offset as usize..].copy_from_slice(message);
    Instruction {
        program_id: ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

async fn send_signed_ctm_enqueue(
    solana: &SolanaCookie,
    group: Pubkey,
    execution_queue: Pubkey,
    envelope: CtmEnvelope,
    payload: Vec<u8>,
    user_owner: TestKeypair,
    ctm_signer: TestKeypair,
    remaining_accounts: Vec<AccountMeta>,
) -> Result<(), TransportError> {
    let user_message =
        canonical_user_intent_message(group, remaining_accounts[1].pubkey, user_owner.pubkey(), &envelope);
    let envelope_message = canonical_envelope_message(group, &envelope);
    let user_signature: [u8; 64] = user_owner
        .to_keypair()
        .sign_message(&user_message)
        .as_ref()
        .try_into()
        .unwrap();
    let ctm_signature: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();
    let user_preinstruction =
        build_presigned_ed25519_instruction(user_owner.pubkey().to_bytes(), &user_message, user_signature);
    let ctm_preinstruction =
        build_presigned_ed25519_instruction(ctm_signer.pubkey().to_bytes(), &envelope_message, ctm_signature);
    let enqueue_instruction = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(
            &[user_preinstruction, ctm_preinstruction, enqueue_instruction],
            None,
        )
        .await?;
    result.result?;
    Ok(())
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

async fn set_pending_ctm_signer(
    solana: &SolanaCookie,
    group: Pubkey,
    execution_queue: Pubkey,
    admin: TestKeypair,
    pending_ctm_signer: Pubkey,
    activate_at_slot: u64,
) -> Result<(), TransportError> {
    let instruction = build_execution_queue_set_ctm_pending_instruction(
        group,
        execution_queue,
        admin.pubkey(),
        pending_ctm_signer,
        activate_at_slot,
    );
    let result = solana
        .process_transaction(&[instruction], Some(&[admin]))
        .await?;
    result.result?;
    Ok(())
}

fn dummy_dispatch_accounts() -> Vec<AccountMeta> {
    vec![AccountMeta {
        pubkey: system_program::id(),
        is_signer: false,
        is_writable: false,
    }]
}

async fn assert_no_perp_orders(solana: &SolanaCookie, account: Pubkey) {
    let mango_account = solana.get_account::<MangoAccount>(account).await;

    for order in mango_account.perp_open_orders.iter() {
        assert_eq!(order.id, 0);
        assert_eq!(order.side_and_tree(), SideAndOrderTree::BidFixed);
        assert_eq!(order.client_id, 0);
        assert_eq!(order.market, FREE_ORDER_SLOT);
    }
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

#[tokio::test]
async fn test_execution_queue_enqueue_ctm_and_wrong_lane_execute_preserves_head(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[2].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    let mango_account = send_tx(
        solana,
        AccountCreateInstruction {
            account_num: 0,
            group,
            owner,
            payer,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .account;

    let remaining_accounts = vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new(mango_account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
    ];
    let payload = encode_perp_cancel_all_orders_payload(3);
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts_with_fixed_runtime_flags(
            &remaining_accounts,
            &[
                AccountMeta::new(group, false),
                AccountMeta::new(execution_queue, false),
                AccountMeta::new_readonly(sysvar::instructions::id(), false),
            ],
        ),
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    let head = queue.current_ctm_head().unwrap();
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);
    assert_eq!(head.sequence, 0);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);

    let mut wrong_lane_accounts = remaining_accounts;
    wrong_lane_accounts[5].pubkey = Pubkey::new_unique();
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: wrong_lane_accounts,
        },
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    let head = queue.current_ctm_head().unwrap();
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);
    assert_eq!(queue.header.next_sequence_to_execute, 0);
    assert_eq!(head.sequence, 0);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_matched_failed_ctm_clears_head(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[2].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    let mango_account = send_tx(
        solana,
        AccountCreateInstruction {
            account_num: 0,
            group,
            owner,
            payer,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .account;

    let remaining_accounts = vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new(mango_account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
    ];
    let payload = encode_perp_cancel_all_orders_payload(3);
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts,
        },
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.next_sequence_to_execute, 0);
    assert!(queue.current_ctm_head().is_none());

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_matched_successful_ctm_cancels_perp_orders(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[0].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 1000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        bids,
        asks,
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

    let price_lots = {
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market.native_price_to_lot(I80F48::ONE)
    };

    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            client_order_id: 41,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];
    let payload = encode_perp_cancel_all_orders_payload(10);
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts,
        },
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert!(queue.current_ctm_head().is_none());

    assert_no_perp_orders(solana, account).await;

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_matched_failed_perp_place_due_to_health_keeps_head(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[0].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 1, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        bids,
        asks,
        event_queue,
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

    let price_lots = {
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market.native_price_to_lot(I80F48::ONE)
    };
    let max_base_lots = 10_000;
    let max_quote_lots = i64::MAX;
    let client_order_id = 77;
    let self_trade_behavior = SelfTradeBehavior::DecrementTake;
    let limit = 10;
    let payload = encode_perp_place_order_v2_payload(
        Side::Bid,
        price_lots,
        max_base_lots,
        max_quote_lots,
        client_order_id,
        PlaceOrderType::Limit,
        self_trade_behavior,
        false,
        0,
        limit,
    );

    let (_, direct_place_instruction) = PerpPlaceOrderInstruction {
        account,
        perp_market,
        owner,
        side: Side::Bid,
        price_lots,
        max_base_lots,
        max_quote_lots,
        reduce_only: false,
        client_order_id,
        self_trade_behavior,
        limit,
    }
    .to_instruction(solana)
    .await;

    let mut remaining_accounts = direct_place_instruction.accounts;
    remaining_accounts[0].is_writable = true;
    remaining_accounts[2].is_signer = false;

    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts_with_fixed_runtime_flags(
            &remaining_accounts,
            &[
                AccountMeta::new(group, false),
                AccountMeta::new(execution_queue, false),
                AccountMeta::new_readonly(sysvar::instructions::id(), false),
            ],
        ),
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    send_tx_expect_error!(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts,
        },
        MangoError::HealthMustBePositiveOrIncrease,
    );

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);
    let head = queue.current_ctm_head().copied().unwrap();
    assert_eq!(head.sequence, 0);

    assert_no_perp_orders(solana, account).await;

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    let asks_data = solana.get_account_boxed::<BookSide>(asks).await;
    let event_queue_data = solana.get_account_boxed::<EventQueue>(event_queue).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);
    assert_eq!(asks_data.roots[0].leaf_count, 0);
    assert_eq!(event_queue_data.header.count(), 0);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_pending_ctm_signer_activates_by_slot(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[2].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let ctm_signer_old = TestKeypair::new();
    let ctm_signer_new = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer_old.pubkey())
            .await;

    let mango_account = send_tx(
        solana,
        AccountCreateInstruction {
            account_num: 0,
            group,
            owner,
            payer,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .account;

    let remaining_accounts = vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new(mango_account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
    ];
    let payload = encode_perp_cancel_all_orders_payload(3);

    let activate_at_slot = solana.clock().await.slot + 3;
    set_pending_ctm_signer(
        solana,
        group,
        execution_queue,
        admin,
        ctm_signer_new.pubkey(),
        activate_at_slot,
    )
    .await
    .unwrap();

    let old_envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        old_envelope,
        payload.clone(),
        owner,
        ctm_signer_old,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.pending_ctm_signer, ctm_signer_new.pubkey());
    assert_eq!(queue.ctm_signer, ctm_signer_old.pubkey());
    assert_eq!(queue.header.total_count, 1);

    solana.advance_by_slots(activate_at_slot - solana.clock().await.slot).await;

    let new_envelope = CtmEnvelope {
        sequence: 1,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        new_envelope.clone(),
        payload.clone(),
        owner,
        ctm_signer_old,
        remaining_accounts.clone(),
    )
    .await;
    assert!(result.is_err());

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        new_envelope,
        payload,
        owner,
        ctm_signer_new,
        remaining_accounts,
    )
    .await
    .unwrap();

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.pending_ctm_signer, Pubkey::default());
    assert_eq!(queue.pending_ctm_activate_slot, 0);
    assert_eq!(queue.ctm_signer, ctm_signer_new.pubkey());
    assert_eq!(queue.header.total_count, 2);
    assert_eq!(queue.header.ctm_count, 2);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_multi_underwater_perp_place_keeps_head(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[0].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 1, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        bids,
        asks,
        event_queue,
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

    let price_lots = {
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market.native_price_to_lot(I80F48::ONE)
    };
    let max_base_lots = 10_000;
    let max_quote_lots = i64::MAX;
    let client_order_id = 99;
    let self_trade_behavior = SelfTradeBehavior::DecrementTake;
    let limit = 10;
    let payload = encode_perp_place_order_v2_payload(
        Side::Bid,
        price_lots,
        max_base_lots,
        max_quote_lots,
        client_order_id,
        PlaceOrderType::Limit,
        self_trade_behavior,
        false,
        0,
        limit,
    );

    let (_, direct_place_instruction) = PerpPlaceOrderInstruction {
        account,
        perp_market,
        owner,
        side: Side::Bid,
        price_lots,
        max_base_lots,
        max_quote_lots,
        reduce_only: false,
        client_order_id,
        self_trade_behavior,
        limit,
    }
    .to_instruction(solana)
    .await;

    let mut lane_accounts = direct_place_instruction.accounts;
    lane_accounts[0].is_writable = true;
    lane_accounts[2].is_signer = false;
    let lane_hash = hash_accounts_with_fixed_runtime_flags(
        &lane_accounts,
        &[
            AccountMeta::new(group, false),
            AccountMeta::new(execution_queue, false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ],
    );

    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: lane_hash,
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        lane_accounts.clone(),
    )
    .await
    .unwrap();

    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        1,
        1,
        lane_accounts.len() as u16,
        vec![lane_hash],
        lane_accounts,
    );
    let result = solana.process_transaction(&[instruction], None).await.unwrap();
    let result = result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &result,
        MangoError::HealthMustBePositiveOrIncrease.into(),
        "queued multi execute health failure".to_string(),
    );

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);
    let head = queue.current_ctm_head().copied().unwrap();
    assert_eq!(head.sequence, 0);

    assert_no_perp_orders(solana, account).await;

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    let asks_data = solana.get_account_boxed::<BookSide>(asks).await;
    let event_queue_data = solana.get_account_boxed::<EventQueue>(event_queue).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);
    assert_eq!(asks_data.roots[0].leaf_count, 0);
    assert_eq!(event_queue_data.header.count(), 0);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_drop_ctm_requires_pause_and_clears_sticky_head(
) -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[0].key;
    let mints = &mints[0..2];
    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 1, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    let mango_v4::accounts::PerpCreateMarket {
        perp_market,
        bids,
        asks,
        event_queue,
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

    let price_lots = {
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market.native_price_to_lot(I80F48::ONE)
    };
    let max_base_lots = 10_000;
    let max_quote_lots = i64::MAX;
    let client_order_id = 77;
    let self_trade_behavior = SelfTradeBehavior::DecrementTake;
    let limit = 10;
    let payload = encode_perp_place_order_v2_payload(
        Side::Bid,
        price_lots,
        max_base_lots,
        max_quote_lots,
        client_order_id,
        PlaceOrderType::Limit,
        self_trade_behavior,
        false,
        0,
        limit,
    );
    let (_, direct_place_instruction) = PerpPlaceOrderInstruction {
        account,
        perp_market,
        owner,
        side: Side::Bid,
        price_lots,
        max_base_lots,
        max_quote_lots,
        reduce_only: false,
        client_order_id,
        self_trade_behavior,
        limit,
    }
    .to_instruction(solana)
    .await;

    let mut lane_accounts = direct_place_instruction.accounts;
    lane_accounts[0].is_writable = true;
    lane_accounts[2].is_signer = false;
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts_with_fixed_runtime_flags(
            &lane_accounts,
            &[
                AccountMeta::new(group, false),
                AccountMeta::new(execution_queue, false),
                AccountMeta::new_readonly(sysvar::instructions::id(), false),
            ],
        ),
        expires_at_slot: 0,
    };

    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        lane_accounts.clone(),
    )
    .await
    .unwrap();

    let result = send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: lane_accounts.clone(),
        },
    )
    .await;
    assert_mango_error(
        &result,
        MangoError::HealthMustBePositiveOrIncrease.into(),
        "queued place health failure".to_string(),
    );

    let drop_instruction = build_execution_queue_drop_ctm_instruction(
        group,
        execution_queue,
        admin.pubkey(),
        0,
    );
    let drop_result = solana
        .process_transaction(&[drop_instruction.clone()], Some(&[admin]))
        .await
        .unwrap();
    let drop_result = drop_result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &drop_result,
        MangoError::ExecutionQueueAdminActionRequiresPause.into(),
        "drop requires pause".to_string(),
    );

    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 2,
            liquidity_delay_slots: 25,
            pause_ingress: false,
            pause_execute: true,
        },
    )
    .await
    .unwrap();

    let drop_result = solana
        .process_transaction(&[drop_instruction.clone()], Some(&[admin]))
        .await
        .unwrap();
    drop_result.result?;

    let queue = solana.get_account_boxed::<ExecutionQueue>(execution_queue).await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert!(queue.current_ctm_head().is_none());

    let second_drop_result = solana
        .process_transaction(&[drop_instruction], Some(&[admin]))
        .await
        .unwrap();
    let second_drop_result = second_drop_result
        .result
        .map_err(TransportError::TransactionError);
    assert_mango_error(
        &second_drop_result,
        MangoError::ExecutionQueueSequenceNotPending.into(),
        "drop missing sequence".to_string(),
    );

    assert_no_perp_orders(solana, account).await;

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    let asks_data = solana.get_account_boxed::<BookSide>(asks).await;
    let event_queue_data = solana.get_account_boxed::<EventQueue>(event_queue).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);
    assert_eq!(asks_data.roots[0].leaf_count, 0);
    assert_eq!(event_queue_data.header.count(), 0);

    Ok(())
}
