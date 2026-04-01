use super::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::sysvar;
use anchor_lang::AnchorSerialize;
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
    let user_message = canonical_user_intent_message(
        group,
        remaining_accounts[1].pubkey,
        user_owner.pubkey(),
        &envelope,
    );
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
    let user_preinstruction = build_presigned_ed25519_instruction(
        user_owner.pubkey().to_bytes(),
        &user_message,
        user_signature,
    );
    let ctm_preinstruction = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.group, group);
    assert_eq!(queue.admin, admin.pubkey());
    assert_eq!(queue.ctm_signer, ctm_signer);
    assert_eq!(queue.header.gap_wait_slots, 4);
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    let head = queue.liquidity_head_item().unwrap();
    assert_eq!(queue.header.gap_wait_slots, 7);
    assert_eq!(queue.header.liquidity_delay_slots, 11);
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.liquidity_count, 1);
    assert_eq!(head.kind, QueueItemKind::LiquidityDeposit as u8);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);
    assert_eq!(head.sequence, 0);
    assert_eq!(
        head.payload_len as usize,
        encode_liquidity_deposit_payload(42, false).len()
    );
    assert!(head.ingress_slot >= before_slot);
    assert_eq!(head.min_execute_slot, head.ingress_slot + 11);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_pause_flags_block_ingress_and_execute() -> Result<(), TransportError>
{
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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

    // EXECUTION_QUEUE_MAX_RETRIES is 1, so the first failed dispatch immediately
    // drops the item: next_retry = 0 + 1 = 1 >= MAX_RETRIES(1), so pop.
    {
        let current_slot = solana.clock().await.slot;
        let queue = solana
            .get_account_boxed::<ExecutionQueue>(execution_queue)
            .await;
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

        // Item is dropped on first failure with MAX_RETRIES=1
        let queue = solana
            .get_account_boxed::<ExecutionQueue>(execution_queue)
            .await;
        assert_eq!(queue.header.total_count, 0);
        assert_eq!(queue.header.liquidity_count, 0);
        assert!(queue.liquidity_head_item().is_none());
    }

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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    let head = queue.current_ctm_head().unwrap();
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);
    assert_eq!(queue.header.next_sequence_to_execute, 0);
    assert_eq!(head.sequence, 0);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_execute_matched_failed_ctm_clears_head() -> Result<(), TransportError>
{
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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
async fn test_execution_queue_pending_ctm_signer_activates_by_slot() -> Result<(), TransportError> {
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.pending_ctm_signer, ctm_signer_new.pubkey());
    assert_eq!(queue.ctm_signer, ctm_signer_old.pubkey());
    assert_eq!(queue.header.total_count, 1);

    solana
        .advance_by_slots(activate_at_slot - solana.clock().await.slot)
        .await;

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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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
    let result = solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap();
    let result = result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &result,
        MangoError::HealthMustBePositiveOrIncrease.into(),
        "queued multi execute health failure".to_string(),
    );

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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

    let drop_instruction =
        build_execution_queue_drop_ctm_instruction(group, execution_queue, admin.pubkey(), 0);
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
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

// ── Additional payload encoding helpers ──

fn encode_liquidity_withdraw_payload(amount: u64, allow_borrow: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&amount.to_le_bytes());
    body.push(u8::from(allow_borrow));
    encode_queue_payload_v1(6, &body)
}

#[allow(dead_code)]
fn encode_perp_cancel_order_payload(order_id: u128) -> Vec<u8> {
    encode_queue_payload_v1(1, &order_id.to_le_bytes())
}

#[allow(dead_code)]
fn encode_perp_cancel_order_by_client_order_id_payload(client_order_id: u64) -> Vec<u8> {
    encode_queue_payload_v1(2, &client_order_id.to_le_bytes())
}

#[allow(dead_code)]
fn encode_perp_cancel_all_orders_by_side_payload(side: Option<Side>, limit: u8) -> Vec<u8> {
    let side_body = match side {
        None => vec![0u8],
        Some(s) => {
            let mut v = vec![1u8];
            v.extend_from_slice(&(s as u8).to_le_bytes());
            v
        }
    };
    let mut body = side_body;
    body.push(limit);
    encode_queue_payload_v1(4, &body)
}

// ── 2A. Enqueue CTM Error Paths (P0) ──

#[tokio::test]
async fn test_enqueue_ctm_payload_too_large() -> Result<(), TransportError> {
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

    // Payload of 257 bytes (exceeds EXECUTION_QUEUE_PAYLOAD_MAX = 256)
    let payload = vec![1u8; 257];
    let envelope = CtmEnvelope {
        sequence: 0,
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
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_hash_mismatch() -> Result<(), TransportError> {
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
    // Use wrong payload_hash — hash of different data
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[b"wrong data"]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_accounts_hash_mismatch() -> Result<(), TransportError> {
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
    // Use wrong accounts_hash
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: [0xAB; 32],
        expires_at_slot: 0,
    };

    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_envelope_expired() -> Result<(), TransportError> {
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

    // Advance slots so the clock is well past slot 1
    solana.advance_by_slots(10).await;
    let current_slot = solana.clock().await.slot;

    // Set expires_at_slot to a slot that is definitely in the past
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: current_slot.saturating_sub(2),
    };

    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_sequence_below_floor() -> Result<(), TransportError> {
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

    // Enqueue seq 0 and seq 1, then drop both to advance next_sequence_to_execute past 0
    for seq in 0..2u64 {
        let envelope = CtmEnvelope {
            sequence: seq,
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
            payload.clone(),
            owner,
            ctm_signer,
            remaining_accounts.clone(),
        )
        .await
        .unwrap();
    }

    // Pause execution so we can use drop_ctm
    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 4,
            liquidity_delay_slots: 25,
            pause_ingress: false,
            pause_execute: true,
        },
    )
    .await
    .unwrap();

    // Drop seq 0, which advances head to seq 1
    send_tx(
        solana,
        ExecutionQueueDropCtmInstruction {
            group,
            admin,
            sequence: 0,
        },
    )
    .await
    .unwrap();

    // Drop seq 1, which clears the queue
    send_tx(
        solana,
        ExecutionQueueDropCtmInstruction {
            group,
            admin,
            sequence: 1,
        },
    )
    .await
    .unwrap();

    // Unpause
    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 4,
            liquidity_delay_slots: 25,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

    // Verify next_sequence_to_execute advanced past 0
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert!(queue.header.next_sequence_to_execute >= 1);

    // Now try to enqueue seq 0 again — it's below next_sequence_to_execute
    let envelope_old = CtmEnvelope {
        sequence: 0,
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
        envelope_old,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_duplicate_sequence() -> Result<(), TransportError> {
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
        envelope.clone(),
        payload.clone(),
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    // Enqueue same sequence again
    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_missing_ctm_signature() -> Result<(), TransportError> {
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

    // Only include user signature, NOT ctm signature
    let user_message =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope);
    let user_signature: [u8; 64] = owner
        .to_keypair()
        .sign_message(&user_message)
        .as_ref()
        .try_into()
        .unwrap();
    let user_preinstruction = build_presigned_ed25519_instruction(
        owner.pubkey().to_bytes(),
        &user_message,
        user_signature,
    );
    let enqueue_instruction = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(&[user_preinstruction, enqueue_instruction], None)
        .await
        .unwrap();
    assert!(result.result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_wrong_ctm_signer() -> Result<(), TransportError> {
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
    let wrong_signer = TestKeypair::new();
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

    // Sign with wrong_signer instead of ctm_signer
    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        wrong_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_invalid_payload_version() -> Result<(), TransportError> {
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

    // Payload with version=2 (invalid, must be 1)
    let mut payload = vec![2u8, 3u8, 0u8, 0u8]; // version=2, variant=3 (cancel_all), flags=0
    payload.push(10); // limit
    let envelope = CtmEnvelope {
        sequence: 0,
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
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_invalid_payload_variant() -> Result<(), TransportError> {
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

    // Payload with invalid variant=255
    let payload = encode_queue_payload_v1(255, &[0u8; 8]);
    let envelope = CtmEnvelope {
        sequence: 0,
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
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_truncated_payload() -> Result<(), TransportError> {
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

    // Valid header (version=1, variant=5 = liquidity_deposit) but body too short
    // Liquidity deposit needs 9 bytes (u64 + bool), give it only 2
    let payload = encode_queue_payload_v1(5, &[0u8, 1u8]);
    let envelope = CtmEnvelope {
        sequence: 0,
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
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_payload_kind_mismatch() -> Result<(), TransportError> {
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

    // Use a liquidity deposit payload variant (5) with CTM envelope kind
    let payload = encode_liquidity_deposit_payload(100, false);
    let envelope = CtmEnvelope {
        sequence: 0,
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
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_nonzero_flags() -> Result<(), TransportError> {
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

    // Build payload with nonzero flags (flags = 1 instead of 0)
    let mut payload = Vec::with_capacity(6);
    payload.push(1); // version
    payload.push(3); // variant = cancel_all_orders
    payload.extend_from_slice(&1u16.to_le_bytes()); // flags = 1 (nonzero!)
    payload.push(10); // limit body
    let envelope = CtmEnvelope {
        sequence: 0,
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
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_zero_expiry_never_expires() -> Result<(), TransportError> {
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

    // Advance slots significantly
    solana.advance_by_slots(100).await;

    let payload = encode_perp_cancel_all_orders_payload(3);
    // expires_at_slot = 0 means never expires
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
        remaining_accounts,
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

// ── 2B. Enqueue Liquidity Error Paths (P0) ──

#[tokio::test]
async fn test_enqueue_liquidity_invalid_kind_ctm_wrapped() -> Result<(), TransportError> {
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

    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::CtmWrapped as u8,
            payload: encode_liquidity_deposit_payload(100, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueueInvalidItemKind,
    );

    Ok(())
}

#[tokio::test]
async fn test_enqueue_liquidity_kind_mismatch() -> Result<(), TransportError> {
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

    // kind = LiquidityDeposit (1) but payload variant = LiquidityWithdraw (6)
    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_withdraw_payload(50, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueuePayloadKindMismatch,
    );

    Ok(())
}

#[tokio::test]
async fn test_enqueue_liquidity_payload_too_large() -> Result<(), TransportError> {
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

    // Payload > 256 bytes
    let oversized_payload = vec![1u8; 257];
    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: oversized_payload,
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueuePayloadTooLarge,
    );

    Ok(())
}

#[tokio::test]
async fn test_enqueue_liquidity_ring_full() -> Result<(), TransportError> {
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

    // Fill 128 items (EXECUTION_QUEUE_LIQUIDITY_CAPACITY)
    for _ in 0..128 {
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
    }

    // 129th should fail
    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(1, false),
            remaining_accounts: dummy_dispatch_accounts(),
        },
        MangoError::ExecutionQueueFull,
    );

    Ok(())
}

#[tokio::test]
async fn test_enqueue_liquidity_empty_remaining_accounts() -> Result<(), TransportError> {
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

    send_tx_expect_error!(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(100, false),
            remaining_accounts: vec![],
        },
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid,
    );

    Ok(())
}

// ── 2C-2D. Execute Paths ──

#[tokio::test]
async fn test_execute_blocked_by_min_execute_slot() -> Result<(), TransportError> {
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
    let future_slot = solana.clock().await.slot + 100;
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: future_slot,
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

    // Execute should be a no-op since min_execute_slot is in the future
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await
    .unwrap();

    // Item should still be pending
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    let head = queue.current_ctm_head().unwrap();
    assert_eq!(head.sequence, 0);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

#[tokio::test]
async fn test_execute_gap_skip_after_wait() -> Result<(), TransportError> {
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

    // Configure gap_wait_slots
    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 3,
            liquidity_delay_slots: 1,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

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

    // Enqueue only seq 1 (skipping seq 0 — creating a gap at head)
    // This means next_sequence_to_execute=0 but seq 0 was never enqueued.
    let envelope1 = CtmEnvelope {
        sequence: 1,
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
        envelope1,
        payload.clone(),
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    // Verify queue state: head at seq 0 (gap), ctm_count=1, max_seen=1
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.next_sequence_to_execute, 0);
    assert_eq!(queue.header.ctm_count, 1);
    assert_eq!(queue.header.max_seen_sequence, 1);

    // Execute — head is a gap (seq 0 not present). Gap wait timer starts.
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 10,
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await
    .unwrap();

    // Gap was observed but not yet skipped — still at seq 0
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.next_sequence_to_execute, 0);
    assert!(queue.header.gap_observed_slot > 0);

    // Advance past gap_wait_slots (configured to 3)
    solana.advance_by_slots(5).await;

    // Execute again — now the gap at seq 0 should be skipped
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 10,
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await
    .unwrap();

    // seq 0 (gap) should have been skipped, seq 1 should have been executed or attempted
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    // next_sequence_to_execute should be past seq 1 (gap skipped, then item cleared)
    assert!(
        queue.header.next_sequence_to_execute >= 1,
        "expected next_sequence >= 1, got {}",
        queue.header.next_sequence_to_execute
    );

    Ok(())
}

#[tokio::test]
async fn test_execute_gap_skip_limited_to_32() -> Result<(), TransportError> {
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

    // Configure very short gap_wait
    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 1,
            liquidity_delay_slots: 1,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

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

    // Enqueue only seq 50 (creating a large gap of 50 sequences: 0..49)
    let envelope = CtmEnvelope {
        sequence: 50,
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

    // Advance past gap_wait
    solana.advance_by_slots(5).await;

    // Execute — should skip at most 32 items per call
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 100,
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    // Should have advanced by at most 32 gap slots from 0
    assert!(queue.header.next_sequence_to_execute <= 32);

    Ok(())
}

#[tokio::test]
async fn test_execute_empty_queue_is_noop() -> Result<(), TransportError> {
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

    // Execute on empty queue should succeed
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 10,
            remaining_accounts: dummy_dispatch_accounts(),
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.liquidity_count, 0);

    Ok(())
}

// ── 2F. Multi-Lane Execute ──

#[tokio::test]
async fn test_execute_multi_lane_count_zero_rejected() -> Result<(), TransportError> {
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

    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        1,      // max_items
        0,      // lane_count = 0 (invalid)
        1,      // accounts_per_lane
        vec![], // no lane hashes
        dummy_dispatch_accounts(),
    );
    let result = solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap();
    assert!(result.result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_execute_multi_lane_count_exceeds_20_rejected() -> Result<(), TransportError> {
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

    let lane_hashes: Vec<[u8; 32]> = (0..21).map(|i| [i as u8; 32]).collect();
    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        1,  // max_items
        21, // lane_count = 21 (exceeds limit of 20)
        1,  // accounts_per_lane
        lane_hashes,
        dummy_dispatch_accounts(),
    );
    let result = solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap();
    assert!(result.result.is_err());

    Ok(())
}

// ── 2G. Admin Operations ──

#[tokio::test]
async fn test_drop_ctm_non_pending_rejected() -> Result<(), TransportError> {
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

    // Pause execute so drop is allowed
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

    // Try to drop sequence 0 which was never enqueued
    let drop_instruction =
        build_execution_queue_drop_ctm_instruction(group, execution_queue, admin.pubkey(), 0);
    let result = solana
        .process_transaction(&[drop_instruction], Some(&[admin]))
        .await
        .unwrap();
    let result = result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &result,
        MangoError::ExecutionQueueSequenceNotPending.into(),
        "drop non-pending sequence".to_string(),
    );

    Ok(())
}

// ── 2H. Queue Create/Resize/Init (P1) ──

#[tokio::test]
async fn test_execution_queue_full_lifecycle() -> Result<(), TransportError> {
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

    // Step 1: Create
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

    let queue_pubkey = execution_queue_pda(group);

    // Step 2: Resize to full size
    while solana.get_account_data(queue_pubkey).await.unwrap().len() < EXECUTION_QUEUE_ACCOUNT_SPACE
    {
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

    let data = solana.get_account_data(queue_pubkey).await.unwrap();
    assert!(data.len() >= EXECUTION_QUEUE_ACCOUNT_SPACE);

    // Step 3: Init
    let ctm_signer = TestKeypair::new().pubkey();
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(queue_pubkey)
        .await;
    assert_eq!(queue.group, group);
    assert_eq!(queue.admin, admin.pubkey());
    assert_eq!(queue.ctm_signer, ctm_signer);

    // Step 4: Configure
    send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin,
            gap_wait_slots: 10,
            liquidity_delay_slots: 20,
            pause_ingress: false,
            pause_execute: false,
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(queue_pubkey)
        .await;
    assert_eq!(queue.header.gap_wait_slots, 10);
    assert_eq!(queue.header.liquidity_delay_slots, 20);
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.liquidity_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_execution_queue_double_init_rejected() -> Result<(), TransportError> {
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
    // First init succeeds
    let _execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer).await;

    // Second init should fail (account already has discriminator set)
    let result = send_tx(
        solana,
        ExecutionQueueInitInstruction {
            group,
            admin,
            ctm_signer,
        },
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

// ── Phase 2: Additional Tests ──

// ── 2A. Missing CTM Enqueue Error Paths ──

#[tokio::test]
async fn test_enqueue_ctm_missing_user_signature() -> Result<(), TransportError> {
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

    // Only include CTM signer ed25519 pre-instruction, NO user intent pre-instruction
    let envelope_message = canonical_envelope_message(group, &envelope);
    let ctm_signature: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();
    let ctm_pre = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );
    let enqueue = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(&[ctm_pre, enqueue], None)
        .await
        .unwrap();
    let result = result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &result,
        MangoError::ExecutionQueueUserSignatureMissing.into(),
        "missing user signature".to_string(),
    );

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_wrong_user_signer() -> Result<(), TransportError> {
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
    let wrong_user = TestKeypair::new();
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

    // Sign user intent with wrong_user instead of owner
    let user_message =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope);
    let wrong_user_signature: [u8; 64] = wrong_user
        .to_keypair()
        .sign_message(&user_message)
        .as_ref()
        .try_into()
        .unwrap();
    let user_pre = build_presigned_ed25519_instruction(
        wrong_user.pubkey().to_bytes(),
        &user_message,
        wrong_user_signature,
    );

    let envelope_message = canonical_envelope_message(group, &envelope);
    let ctm_signature: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();
    let ctm_pre = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );

    let enqueue = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(&[user_pre, ctm_pre, enqueue], None)
        .await
        .unwrap();
    let result = result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &result,
        MangoError::ExecutionQueueUserSignatureMissing.into(),
        "wrong user signer".to_string(),
    );

    Ok(())
}

#[tokio::test]
async fn test_enqueue_ctm_invalid_envelope_kind() -> Result<(), TransportError> {
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
    // Set kind to LiquidityDeposit instead of CtmWrapped
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::LiquidityDeposit as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    let result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;
    assert!(result.is_err());

    Ok(())
}

// ── 2C. Execute Happy Paths ──

#[tokio::test]
async fn test_execute_perp_place_order_v2_happy() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
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
    let max_base_lots = 1;
    let max_quote_lots = i64::MAX;
    let client_order_id = 100;
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert!(queue.current_ctm_head().is_none());

    // Verify order appears on the book
    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    Ok(())
}

#[tokio::test]
async fn test_execute_perp_cancel_order_happy() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
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

    // Place an order directly
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            client_order_id: 50,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    // Get the order_id from the mango account
    let mango_account_data = solana.get_account::<MangoAccount>(account).await;
    let order_id = mango_account_data.perp_open_orders[0].id;

    // Now enqueue PerpCancelOrder with that order_id
    let payload = encode_perp_cancel_order_payload(order_id);

    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];

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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);

    // Verify order removed from book
    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);

    assert_no_perp_orders(solana, account).await;

    Ok(())
}

#[tokio::test]
async fn test_execute_perp_cancel_all_orders_by_side_happy() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
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

    // Place a bid
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            client_order_id: 60,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    // Place an ask at a higher price
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Ask,
            price_lots: price_lots + 100,
            max_base_lots: 1,
            client_order_id: 61,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    let asks_data = solana.get_account_boxed::<BookSide>(asks).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);
    assert_eq!(asks_data.roots[0].leaf_count, 1);

    // Enqueue PerpCancelAllOrdersBySide(side=Bid)
    let payload = encode_perp_cancel_all_orders_by_side_payload(Some(Side::Bid), 10);

    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];

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

    // Verify only bid removed, ask still present
    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    let asks_data = solana.get_account_boxed::<BookSide>(asks).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);
    assert_eq!(asks_data.roots[0].leaf_count, 1);

    Ok(())
}

#[tokio::test]
async fn test_execute_perp_cancel_order_by_client_order_id_happy() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
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

    // Place an order with client_order_id=42
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            client_order_id: 42,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    // Enqueue PerpCancelOrderByClientOrderId(42)
    let payload = encode_perp_cancel_order_by_client_order_id_payload(42);

    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];

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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);

    // Verify order removed
    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);

    assert_no_perp_orders(solana, account).await;

    Ok(())
}

#[tokio::test]
async fn test_execute_liquidity_deposit_happy() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
    let execution_queue = create_initialized_execution_queue(
        solana,
        group,
        admin,
        payer,
        TestKeypair::new().pubkey(),
    )
    .await;

    // Get remaining_accounts from TokenDepositInstruction
    let deposit_ix = TokenDepositInstruction {
        amount: 100,
        reduce_only: false,
        account,
        owner,
        token_account: users[1].token_accounts[0],
        token_authority: users[1].key,
        bank_index: 0,
    };
    let (_, direct_deposit_instruction) = deposit_ix.to_instruction(solana).await;

    let mut remaining_accounts = direct_deposit_instruction.accounts;
    // Adjust flags: group is writable, owner is not signer in dispatch
    remaining_accounts[0].is_writable = true;
    remaining_accounts[2].is_signer = false;
    // token_authority is not signer in dispatch
    for acc in remaining_accounts.iter_mut() {
        acc.is_signer = false;
    }

    let payload = encode_liquidity_deposit_payload(100, false);

    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: payload.clone(),
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await
    .unwrap();

    // Advance past liquidity delay
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    let head = queue.liquidity_head_item().unwrap();
    let current_slot = solana.clock().await.slot;
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
            remaining_accounts,
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    // Item should have been processed (either succeeded and popped, or failed and popped due to MAX_RETRIES=1)
    assert_eq!(queue.header.liquidity_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_execute_liquidity_withdraw_happy() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
    let execution_queue = create_initialized_execution_queue(
        solana,
        group,
        admin,
        payer,
        TestKeypair::new().pubkey(),
    )
    .await;

    // Get remaining_accounts from TokenWithdrawInstruction
    let withdraw_ix = TokenWithdrawInstruction {
        amount: 100,
        allow_borrow: false,
        account,
        owner,
        token_account: users[1].token_accounts[0],
        bank_index: 0,
    };
    let (_, direct_withdraw_instruction) = withdraw_ix.to_instruction(solana).await;

    let mut remaining_accounts = direct_withdraw_instruction.accounts;
    remaining_accounts[0].is_writable = true;
    for acc in remaining_accounts.iter_mut() {
        acc.is_signer = false;
    }

    let payload = encode_liquidity_withdraw_payload(100, false);

    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityWithdraw as u8,
            payload: payload.clone(),
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await
    .unwrap();

    // Advance past liquidity delay
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    let head = queue.liquidity_head_item().unwrap();
    let current_slot = solana.clock().await.slot;
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
            remaining_accounts,
        },
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.liquidity_count, 0);

    Ok(())
}

// ── 2E. Health-Gated Execution ──

#[tokio::test]
async fn test_execute_reduce_only_bypasses_health() -> Result<(), TransportError> {
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

    // Underfunded account with only 1 token
    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 1, 0).await;
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

    // Enqueue PerpPlaceOrderV2 with reduce_only=true
    let max_base_lots = 1;
    let max_quote_lots = i64::MAX;
    let client_order_id = 200;
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
        true, // reduce_only
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
        reduce_only: true,
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

    // Execute should succeed because reduce_only bypasses health
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_execute_cancel_ignores_health() -> Result<(), TransportError> {
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

    // Fund account enough to place the initial order (we'll test that cancel works
    // regardless of health state — cancels never check health)
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
        let perp_market_data = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market_data.native_price_to_lot(I80F48::ONE)
    };

    // Place a small order directly (this uses the user's own signer, so it works)
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            client_order_id: 300,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    // Enqueue PerpCancelAllOrders
    let payload = encode_perp_cancel_all_orders_payload(10);

    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];

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

    // Execute should succeed because cancels don't check health
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);

    // Verify orders cleared
    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);

    assert_no_perp_orders(solana, account).await;

    Ok(())
}

// ── 2F. Multi-Lane Execute ──

#[tokio::test]
async fn test_execute_multi_two_lanes_match_both() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
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

    // Both items use cancel_all with same accounts but different dummy data to get different accounts_hash
    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];

    // Enqueue seq 0
    let payload0 = encode_perp_cancel_all_orders_payload(5);
    let lane_hash = hash_accounts(&remaining_accounts);
    let envelope0 = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload0]).to_bytes(),
        accounts_hash: lane_hash,
        expires_at_slot: 0,
    };
    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope0,
        payload0,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    // Enqueue seq 1 with same accounts
    let payload1 = encode_perp_cancel_all_orders_payload(10);
    let envelope1 = CtmEnvelope {
        sequence: 1,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload1]).to_bytes(),
        accounts_hash: lane_hash,
        expires_at_slot: 0,
    };
    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope1,
        payload1,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.ctm_count, 2);

    // Execute multi with 2 lanes, both using same accounts
    let accounts_per_lane = remaining_accounts.len() as u16;
    let mut all_remaining = remaining_accounts.clone();
    all_remaining.extend(remaining_accounts.iter().cloned());

    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        2,
        2,
        accounts_per_lane,
        vec![lane_hash, lane_hash],
        all_remaining,
    );
    let result = solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap();
    result.result?;

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.total_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_execute_multi_hash_count_mismatch() -> Result<(), TransportError> {
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

    // lane_count=2 but lane_hashes has 1 entry (mismatch)
    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        1,
        2,               // lane_count = 2
        1,               // accounts_per_lane
        vec![[0u8; 32]], // only 1 lane hash
        dummy_dispatch_accounts(),
    );
    let result = solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap();
    assert!(result.result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_execute_multi_insufficient_remaining_accounts() -> Result<(), TransportError> {
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

    // lane_count=2, accounts_per_lane=3, but only 1 remaining account provided (need 6)
    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        1,
        2, // lane_count = 2
        3, // accounts_per_lane = 3 (need 6 total)
        vec![[0u8; 32], [1u8; 32]],
        dummy_dispatch_accounts(), // only 1 account
    );
    let result = solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap();
    assert!(result.result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_execute_multi_gap_handling_across_lanes() -> Result<(), TransportError> {
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

    let account = create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey()).await;

    // Configure short gap_wait
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

    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];
    let lane_hash = hash_accounts(&remaining_accounts);

    // Enqueue only seq 1 (skip seq 0 — creating a gap)
    let payload = encode_perp_cancel_all_orders_payload(10);
    let envelope = CtmEnvelope {
        sequence: 1,
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
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    // First execute multi — gap observed but not yet skipped
    let accounts_per_lane = remaining_accounts.len() as u16;
    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        2,
        1,
        accounts_per_lane,
        vec![lane_hash],
        remaining_accounts.clone(),
    );
    solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap()
        .result?;

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.next_sequence_to_execute, 0);
    assert!(queue.header.gap_observed_slot > 0);

    // Advance past gap_wait_slots
    solana.advance_by_slots(5).await;

    // Second execute multi — gap skipped, seq 1 should execute
    let instruction = build_execution_queue_execute_multi_instruction(
        group,
        execution_queue,
        2,
        1,
        accounts_per_lane,
        vec![lane_hash],
        remaining_accounts,
    );
    solana
        .process_transaction(&[instruction], None)
        .await
        .unwrap()
        .result?;

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert!(queue.header.next_sequence_to_execute >= 1);
    assert_eq!(queue.header.ctm_count, 0);

    Ok(())
}

// ── 2H. Queue Resize ──

#[tokio::test]
async fn test_execution_queue_resize_when_already_full() -> Result<(), TransportError> {
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

    // Create and resize to full size
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

    let queue_pubkey = execution_queue_pda(group);
    while solana.get_account_data(queue_pubkey).await.unwrap().len() < EXECUTION_QUEUE_ACCOUNT_SPACE
    {
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

    let data_before = solana.get_account_data(queue_pubkey).await.unwrap();
    assert!(data_before.len() >= EXECUTION_QUEUE_ACCOUNT_SPACE);

    // Call resize again — should succeed as no-op
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

    let data_after = solana.get_account_data(queue_pubkey).await.unwrap();
    assert_eq!(data_before.len(), data_after.len());

    Ok(())
}

// ── 2I. Signature Edge Cases ──

fn canonical_user_intent_message_hex_utf8(msg_hash: [u8; 32]) -> [u8; 64] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 64];
    let mut i = 0usize;
    while i < 32 {
        let b = msg_hash[i];
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0x0f) as usize];
        i += 1;
    }
    out
}

#[tokio::test]
async fn test_sig_hex_utf8_format() -> Result<(), TransportError> {
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

    // Build user intent message hash, then hex-encode it and sign that
    let user_intent_hash =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope);
    let hex_message = canonical_user_intent_message_hex_utf8(user_intent_hash);
    let user_signature: [u8; 64] = owner
        .to_keypair()
        .sign_message(&hex_message)
        .as_ref()
        .try_into()
        .unwrap();
    let user_pre = build_presigned_ed25519_instruction(
        owner.pubkey().to_bytes(),
        &hex_message,
        user_signature,
    );

    // CTM signer signs the envelope normally
    let envelope_message = canonical_envelope_message(group, &envelope);
    let ctm_signature: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();
    let ctm_pre = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );

    let enqueue = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(&[user_pre, ctm_pre, enqueue], None)
        .await
        .unwrap();
    result.result?;

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

#[tokio::test]
async fn test_sig_different_tx_positions() -> Result<(), TransportError> {
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

    let user_message =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope);
    let user_signature: [u8; 64] = owner
        .to_keypair()
        .sign_message(&user_message)
        .as_ref()
        .try_into()
        .unwrap();
    let user_pre = build_presigned_ed25519_instruction(
        owner.pubkey().to_bytes(),
        &user_message,
        user_signature,
    );

    let envelope_message = canonical_envelope_message(group, &envelope);
    let ctm_signature: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();
    let ctm_pre = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );

    let enqueue = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    // Place ctm_pre first, then user_pre (reversed from normal order)
    let result = solana
        .process_transaction(&[ctm_pre, user_pre, enqueue], None)
        .await
        .unwrap();
    result.result?;

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

fn build_dual_ed25519_instruction(
    pubkey1: [u8; 32],
    message1: &[u8],
    signature1: [u8; 64],
    pubkey2: [u8; 32],
    message2: &[u8],
    signature2: [u8; 64],
) -> Instruction {
    // 2 signatures, each with 14-byte offsets header, then data blobs
    let header_len = 2 + 2 * 14; // 2 bytes header + 2 * 14 bytes offsets
                                 // Entry 1: sig1 (64) + pubkey1 (32) + message1
    let sig1_offset = header_len;
    let pk1_offset = sig1_offset + 64;
    let msg1_offset = pk1_offset + 32;
    // Entry 2: sig2 (64) + pubkey2 (32) + message2
    let sig2_offset = msg1_offset + message1.len();
    let pk2_offset = sig2_offset + 64;
    let msg2_offset = pk2_offset + 32;
    let total_len = msg2_offset + message2.len();

    let mut data = vec![0u8; total_len];
    data[0] = 2; // signature count
    data[1] = 0; // padding

    // Offsets for entry 0
    let off0 = 2;
    data[off0..off0 + 2].copy_from_slice(&(sig1_offset as u16).to_le_bytes());
    data[off0 + 2..off0 + 4].copy_from_slice(&u16::MAX.to_le_bytes()); // sig ix index = self
    data[off0 + 4..off0 + 6].copy_from_slice(&(pk1_offset as u16).to_le_bytes());
    data[off0 + 6..off0 + 8].copy_from_slice(&u16::MAX.to_le_bytes()); // pk ix index = self
    data[off0 + 8..off0 + 10].copy_from_slice(&(msg1_offset as u16).to_le_bytes());
    data[off0 + 10..off0 + 12].copy_from_slice(&(message1.len() as u16).to_le_bytes());
    data[off0 + 12..off0 + 14].copy_from_slice(&u16::MAX.to_le_bytes()); // msg ix index = self

    // Offsets for entry 1
    let off1 = 2 + 14;
    data[off1..off1 + 2].copy_from_slice(&(sig2_offset as u16).to_le_bytes());
    data[off1 + 2..off1 + 4].copy_from_slice(&u16::MAX.to_le_bytes());
    data[off1 + 4..off1 + 6].copy_from_slice(&(pk2_offset as u16).to_le_bytes());
    data[off1 + 6..off1 + 8].copy_from_slice(&u16::MAX.to_le_bytes());
    data[off1 + 8..off1 + 10].copy_from_slice(&(msg2_offset as u16).to_le_bytes());
    data[off1 + 10..off1 + 12].copy_from_slice(&(message2.len() as u16).to_le_bytes());
    data[off1 + 12..off1 + 14].copy_from_slice(&u16::MAX.to_le_bytes());

    // Data blobs
    data[sig1_offset..sig1_offset + 64].copy_from_slice(&signature1);
    data[pk1_offset..pk1_offset + 32].copy_from_slice(&pubkey1);
    data[msg1_offset..msg1_offset + message1.len()].copy_from_slice(message1);
    data[sig2_offset..sig2_offset + 64].copy_from_slice(&signature2);
    data[pk2_offset..pk2_offset + 32].copy_from_slice(&pubkey2);
    data[msg2_offset..msg2_offset + message2.len()].copy_from_slice(message2);

    Instruction {
        program_id: ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

#[tokio::test]
async fn test_sig_multiple_sigs_in_one_ix() -> Result<(), TransportError> {
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

    let user_message =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope);
    let user_signature: [u8; 64] = owner
        .to_keypair()
        .sign_message(&user_message)
        .as_ref()
        .try_into()
        .unwrap();

    let envelope_message = canonical_envelope_message(group, &envelope);
    let ctm_signature: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();

    // Build a single ed25519 instruction with both signatures
    let dual_pre = build_dual_ed25519_instruction(
        owner.pubkey().to_bytes(),
        &user_message,
        user_signature,
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );

    let enqueue = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(&[dual_pre, enqueue], None)
        .await
        .unwrap();
    result.result?;

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

#[tokio::test]
async fn test_sig_correct_message_wrong_signer() -> Result<(), TransportError> {
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
    let wrong_keypair = TestKeypair::new();
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

    // User signs correctly
    let user_message =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope);
    let user_signature: [u8; 64] = owner
        .to_keypair()
        .sign_message(&user_message)
        .as_ref()
        .try_into()
        .unwrap();
    let user_pre = build_presigned_ed25519_instruction(
        owner.pubkey().to_bytes(),
        &user_message,
        user_signature,
    );

    // Sign the correct envelope message with the WRONG keypair
    let envelope_message = canonical_envelope_message(group, &envelope);
    let wrong_signature: [u8; 64] = wrong_keypair
        .to_keypair()
        .sign_message(&envelope_message)
        .as_ref()
        .try_into()
        .unwrap();
    // But claim it's from wrong_keypair's pubkey (not ctm_signer)
    let ctm_pre = build_presigned_ed25519_instruction(
        wrong_keypair.pubkey().to_bytes(),
        &envelope_message,
        wrong_signature,
    );

    let enqueue = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope,
        payload,
        remaining_accounts,
    );

    let result = solana
        .process_transaction(&[user_pre, ctm_pre, enqueue], None)
        .await
        .unwrap();
    let result = result.result.map_err(TransportError::TransactionError);
    assert_mango_error(
        &result,
        MangoError::CtmSignatureMissing.into(),
        "wrong signer for CTM".to_string(),
    );

    Ok(())
}

#[tokio::test]
async fn test_sig_replay_from_different_envelope() -> Result<(), TransportError> {
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

    // Create envelope A (sequence=0)
    let envelope_a = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    // Create envelope B (sequence=1) with same payload
    let envelope_b = CtmEnvelope {
        sequence: 1,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    // Sign envelope A's messages
    let user_message_a =
        canonical_user_intent_message(group, mango_account, owner.pubkey(), &envelope_a);
    let user_sig_a: [u8; 64] = owner
        .to_keypair()
        .sign_message(&user_message_a)
        .as_ref()
        .try_into()
        .unwrap();
    let envelope_message_a = canonical_envelope_message(group, &envelope_a);
    let ctm_sig_a: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&envelope_message_a)
        .as_ref()
        .try_into()
        .unwrap();

    // Try to use envelope A's signatures with envelope B's data
    let user_pre_a =
        build_presigned_ed25519_instruction(owner.pubkey().to_bytes(), &user_message_a, user_sig_a);
    let ctm_pre_a = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message_a,
        ctm_sig_a,
    );

    // Build the enqueue with envelope B's data
    let enqueue_b = build_execution_queue_enqueue_ctm_instruction(
        group,
        execution_queue,
        envelope_b,
        payload,
        remaining_accounts,
    );

    // This should fail because the ed25519 pre-instructions verify envelope A's message
    // but the enqueue instruction computes envelope B's message (different sequence)
    let result = solana
        .process_transaction(&[user_pre_a, ctm_pre_a, enqueue_b], None)
        .await
        .unwrap();
    assert!(result.result.is_err());

    Ok(())
}
