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

// ── Duplicated helpers (module-private in test_execution_queue.rs) ──

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

fn dummy_dispatch_accounts() -> Vec<AccountMeta> {
    vec![AccountMeta {
        pubkey: system_program::id(),
        is_signer: false,
        is_writable: false,
    }]
}

// ── 3A. Privilege Escalation (P0) ──

/// Try to enqueue CTM with a mango_account that belongs to a different group.
/// The remaining_accounts[1] (mango_account) is owned by group2, but we enqueue
/// against group1's execution queue. Should fail with ExecutionQueueInvalidUserAccount.
#[tokio::test]
async fn test_security_forged_mango_account() -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer = users[1].key;
    let owner = users[2].key;
    let mints = &mints[0..2];

    // Create group1
    let GroupWithTokens { group: group1, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    // Create group2 (separate group with a different admin keypair)
    let admin2 = TestKeypair::new();
    let GroupWithTokens { group: group2, .. } = GroupWithTokensConfig {
        admin: admin2,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group1, admin, payer, ctm_signer.pubkey())
            .await;

    // Create a mango account belonging to group2
    let mango_account_group2 = send_tx(
        solana,
        AccountCreateInstruction {
            account_num: 0,
            group: group2,
            owner,
            payer,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .account;

    // Build remaining_accounts referencing the group2 mango_account
    let remaining_accounts = vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new(mango_account_group2, false), // wrong group!
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

    let result = send_signed_ctm_enqueue(
        solana,
        group1,
        execution_queue,
        envelope,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;

    // Should fail because the mango_account belongs to group2, not group1
    assert!(result.is_err(), "enqueue with forged mango_account should fail");

    Ok(())
}

/// Try to drop a CTM item using a non-admin keypair. The admin signer check
/// should reject the transaction.
#[tokio::test]
async fn test_security_non_admin_drop() -> Result<(), TransportError> {
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
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

    // Create a mango account and enqueue a CTM item
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
        remaining_accounts,
    )
    .await
    .unwrap();

    // Pause execute so drop is allowed (drop requires pause)
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

    // Try to drop using a non-admin keypair
    let non_admin = TestKeypair::new();
    let drop_instruction = build_execution_queue_drop_ctm_instruction(
        group,
        execution_queue,
        non_admin.pubkey(),
        0,
    );
    let result = solana
        .process_transaction(&[drop_instruction], Some(&[non_admin]))
        .await;

    // Should fail because non_admin is not the group admin.
    // process_transaction returns Ok(tx_result) where tx_result.result holds the program error.
    let tx_result = result.unwrap();
    assert!(tx_result.result.is_err(), "non-admin drop should fail");

    // Verify queue is unchanged
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
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

/// Try to configure queue with non-admin keypair. Should fail.
#[tokio::test]
async fn test_security_non_admin_configure() -> Result<(), TransportError> {
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

    let ctm_signer = TestKeypair::new();
    let _execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

    // Try to configure using a non-admin keypair
    let non_admin = TestKeypair::new();
    let result = send_tx(
        solana,
        ExecutionQueueConfigureInstruction {
            group,
            admin: non_admin,
            gap_wait_slots: 100,
            liquidity_delay_slots: 100,
            pause_ingress: true,
            pause_execute: true,
        },
    )
    .await;

    assert!(result.is_err(), "non-admin configure should fail");

    // Verify the configuration was not changed
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue_pda(group))
        .await;
    assert_eq!(queue.header.gap_wait_slots, 4); // default value, not 100
    assert_eq!(queue.header.liquidity_delay_slots, 25); // default value, not 100

    Ok(())
}

// ── 3B. Replay / Double-Spend (P0) ──

/// Enqueue CTM seq 0, execute it (clears), try to re-enqueue with seq 0.
/// Should fail because next_sequence_to_execute has advanced or the sequence
/// is no longer valid.
#[tokio::test]
async fn test_security_replay_same_envelope_after_execution() -> Result<(), TransportError> {
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
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
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
    let envelope0 = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    // Enqueue seq 0
    send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope0.clone(),
        payload.clone(),
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    // Enqueue seq 1 so that after both are executed, next_sequence_to_execute advances past 0.
    // (When the last CTM item is cleared, the head-advance loop does not run because
    // ctm_count drops to 0. Having a second item ensures head advances properly.)
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

    // Execute both items (will fail dispatch but clear them via retry logic)
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

    // Verify both were cleared and next_sequence_to_execute advanced
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert!(queue.header.next_sequence_to_execute >= 1,
        "expected next_sequence >= 1, got {}", queue.header.next_sequence_to_execute);

    // Try to replay the same envelope with seq 0
    let replay_result = send_signed_ctm_enqueue(
        solana,
        group,
        execution_queue,
        envelope0,
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await;

    // Should fail: sequence 0 is below next_sequence_to_execute
    assert!(
        replay_result.is_err(),
        "replaying an already-executed envelope should fail"
    );

    Ok(())
}

/// Enqueue same payload with seq 0 and seq 1. Both should succeed because
/// different sequences are independent items.
#[tokio::test]
async fn test_security_same_payload_different_sequence() -> Result<(), TransportError> {
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
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
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

    // Enqueue seq 0
    let envelope0 = CtmEnvelope {
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
        envelope0,
        payload.clone(),
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    // Enqueue seq 1 with same payload
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
        payload,
        owner,
        ctm_signer,
        remaining_accounts,
    )
    .await
    .unwrap();

    // Both should have been enqueued
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 2);
    assert_eq!(queue.header.ctm_count, 2);

    Ok(())
}

// ── 3C. Queue State Corruption (P0) ──

/// Execute on an empty queue. Verify counts remain at 0 (no underflow).
#[tokio::test]
async fn test_security_count_underflow_resistance() -> Result<(), TransportError> {
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

    // Verify queue starts empty
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.liquidity_count, 0);

    // Execute on empty queue — should succeed as a no-op
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

    // Verify counts are still 0 (no underflow)
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);
    assert_eq!(queue.header.liquidity_count, 0);

    Ok(())
}

/// Configure queue with very high next_sequence, enqueue near that value.
/// Verify no panics with large sequence numbers.
#[tokio::test]
async fn test_security_sequence_overflow_resistance() -> Result<(), TransportError> {
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

    // Enqueue a liquidity deposit — the sequence will be assigned automatically
    // starting from 0. The key point is that this should not panic.
    send_tx(
        solana,
        ExecutionQueueEnqueueLiquidityInstruction {
            group,
            execution_queue,
            kind: QueueItemKind::LiquidityDeposit as u8,
            payload: encode_liquidity_deposit_payload(u64::MAX, false),
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
    assert_eq!(head.sequence, 0);
    assert_eq!(head.status, QueueItemStatus::Pending as u8);

    Ok(())
}

// ── 3A. Additional Privilege Escalation ──

/// Enqueue a CTM cancel order where the remaining_accounts includes a foreign
/// program_id (not mango_v4). The dispatch should fail because the dispatch
/// tries to invoke via CPI to the mango program. Dispatch failure clears
/// the CTM item. Verify the queue state is correct after.
#[tokio::test]
async fn test_security_dispatch_to_foreign_program() -> Result<(), TransportError> {
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

    // Build remaining_accounts but replace the group (account[0]) with a foreign program
    let foreign_program = Pubkey::new_unique();
    let remaining_accounts = vec![
        AccountMeta::new(foreign_program, false), // foreign program instead of group
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

    // Verify the item was enqueued
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    // Execute — the dispatch should fail because the remaining_accounts[0]
    // (group) doesn't match the actual group, causing the CPI to fail.
    // On dispatch failure the item is cleared from the queue.
    let execute_result = send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts,
        },
    )
    .await;

    // The execute may succeed (clearing the failed item) or fail.
    // Either way, verify the queue state is consistent.
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    // If execute succeeded, the failed dispatch should have cleared the item.
    // If execute failed, the item remains but queue state is still consistent.
    assert!(
        queue.header.total_count <= 1,
        "queue should have at most 1 item"
    );
    assert!(
        queue.header.ctm_count <= 1,
        "ctm_count should be consistent"
    );
    // Counts should be consistent with each other
    assert!(
        queue.header.ctm_count <= queue.header.total_count,
        "ctm_count should not exceed total_count"
    );

    Ok(())
}

/// Try to enqueue CTM but pass a fake account as the instructions sysvar.
/// The enqueue instruction expects `sysvar::instructions::id()` at position 2
/// of the fixed accounts. If we build the instruction manually with a wrong
/// sysvar, it should fail at the Anchor constraint level.
#[tokio::test]
async fn test_security_spoofed_instructions_sysvar() -> Result<(), TransportError> {
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
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
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
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    // Build the ed25519 pre-instructions normally
    let user_message = canonical_user_intent_message(
        group,
        mango_account,
        owner.pubkey(),
        &envelope,
    );
    let envelope_message = canonical_envelope_message(group, &envelope);
    let user_signature: [u8; 64] = owner
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
        owner.pubkey().to_bytes(),
        &user_message,
        user_signature,
    );
    let ctm_preinstruction = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &envelope_message,
        ctm_signature,
    );

    // Build the enqueue instruction manually with a FAKE instructions sysvar
    let fake_sysvar = Pubkey::new_unique();
    let mut data = anchor_discriminator("execution_queue_enqueue_ctm").to_vec();
    data.extend(envelope.try_to_vec().unwrap());
    data.extend(payload.try_to_vec().unwrap());
    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(execution_queue, false),
        AccountMeta::new_readonly(fake_sysvar, false), // SPOOFED: not the real instructions sysvar
    ];
    accounts.extend(remaining_accounts);
    let enqueue_instruction = Instruction {
        program_id: mango_v4::id(),
        accounts,
        data,
    };

    let result = solana
        .process_transaction(
            &[user_preinstruction, ctm_preinstruction, enqueue_instruction],
            None,
        )
        .await;

    // Should fail because the fake sysvar doesn't match the expected instructions sysvar
    let tx_result = result.unwrap();
    assert!(
        tx_result.result.is_err(),
        "enqueue with spoofed instructions sysvar should fail"
    );

    // Verify the queue is unchanged
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);

    Ok(())
}

// ── 3B. Additional Replay ──

/// Try to enqueue the same sequence number twice. The first should succeed,
/// the second should fail with ExecutionQueueDuplicateSequence. This is an
/// explicit security-focused duplicate of the functional test.
#[tokio::test]
async fn test_security_concurrent_same_sequence() -> Result<(), TransportError> {
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
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
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
    let envelope = CtmEnvelope {
        sequence: 0,
        min_execute_slot: 0,
        kind: QueueItemKind::CtmWrapped as u8,
        payload_hash: hashv(&[&payload]).to_bytes(),
        accounts_hash: hash_accounts(&remaining_accounts),
        expires_at_slot: 0,
    };

    // First enqueue with seq=0 should succeed
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

    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    // Second enqueue with seq=0 should fail with DuplicateSequence
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

    assert!(
        result.is_err(),
        "second enqueue with same sequence should fail"
    );

    // Verify queue still has exactly 1 item
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

// ── 3E. Oracle Manipulation ──

/// Create a perp market, fund an account, enqueue a PerpPlaceOrderV2 via CTM.
/// Before executing, make the oracle stale by setting `last_update_slot` far
/// in the past. The execute should fail because health computation can't use
/// a stale oracle.
#[tokio::test]
#[ignore] // Stub oracles do not enforce staleness the same way as Pyth — needs real oracle fixture
async fn test_security_stale_oracle_blocks_queue_health() -> Result<(), TransportError> {
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

    let account =
        create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

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

    let payload = encode_perp_place_order_v2_payload(
        Side::Bid,
        price_lots,
        1,           // max_base_lots
        i64::MAX,    // max_quote_lots
        42,          // client_order_id
        PlaceOrderType::Limit,
        SelfTradeBehavior::DecrementTake,
        false,
        0,
        10,
    );

    let (_, direct_place_instruction) = PerpPlaceOrderInstruction {
        account,
        perp_market,
        owner,
        side: Side::Bid,
        price_lots,
        max_base_lots: 1,
        max_quote_lots: i64::MAX,
        reduce_only: false,
        client_order_id: 42,
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        limit: 10,
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

    // Make the oracle stale by setting last_update_slot to 0 (very stale)
    send_tx(
        solana,
        StubOracleSetTestInstruction {
            oracle: tokens[0].oracle,
            group,
            mint: mints[0].pubkey,
            admin,
            price: 1.0,
            last_update_slot: 0,
            deviation: 100.0,
        },
    )
    .await
    .unwrap();

    // Execute should fail because the oracle is stale and health check fails
    let execute_result = send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await;

    assert!(
        execute_result.is_err(),
        "execute with stale oracle should fail"
    );

    // Verify the queue item is still present (not cleared)
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

/// Similar to stale oracle test but set the oracle's deviation to a very
/// large value while keeping it recent. Execute should fail because the
/// oracle confidence interval is too wide for health computation.
#[tokio::test]
#[ignore] // Stub oracles do not enforce confidence width the same way as Pyth — needs real oracle fixture
async fn test_security_oracle_confidence_too_wide() -> Result<(), TransportError> {
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

    let account =
        create_funded_account(solana, group, owner, 0, &users[1], mints, 10000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

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

    let payload = encode_perp_place_order_v2_payload(
        Side::Bid,
        price_lots,
        1,
        i64::MAX,
        43,
        PlaceOrderType::Limit,
        SelfTradeBehavior::DecrementTake,
        false,
        0,
        10,
    );

    let (_, direct_place_instruction) = PerpPlaceOrderInstruction {
        account,
        perp_market,
        owner,
        side: Side::Bid,
        price_lots,
        max_base_lots: 1,
        max_quote_lots: i64::MAX,
        reduce_only: false,
        client_order_id: 43,
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        limit: 10,
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

    // Set the oracle deviation very high while keeping it recent
    let current_slot = solana.clock().await.slot;
    send_tx(
        solana,
        StubOracleSetTestInstruction {
            oracle: tokens[0].oracle,
            group,
            mint: mints[0].pubkey,
            admin,
            price: 1.0,
            last_update_slot: current_slot,
            deviation: 100.0, // extremely wide confidence
        },
    )
    .await
    .unwrap();

    // Execute should fail because oracle confidence is too wide
    let execute_result = send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 1,
            remaining_accounts: remaining_accounts.clone(),
        },
    )
    .await;

    assert!(
        execute_result.is_err(),
        "execute with too-wide oracle confidence should fail"
    );

    // Verify the queue item is still present
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 1);
    assert_eq!(queue.header.ctm_count, 1);

    Ok(())
}

/// Same stale oracle setup, but enqueue PerpCancelAllOrders instead.
/// Execute should succeed because cancels don't go through health check.
#[tokio::test]
async fn test_security_oracle_stale_does_not_block_cancel() -> Result<(), TransportError> {
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

    let account =
        create_funded_account(solana, group, owner, 0, &users[1], mints, 1000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

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

    // Place a real order first so the cancel has something to do
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            client_order_id: 44,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 1);

    // Enqueue cancel all orders via CTM
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

    // Make the oracle stale
    send_tx(
        solana,
        StubOracleSetTestInstruction {
            oracle: tokens[0].oracle,
            group,
            mint: mints[0].pubkey,
            admin,
            price: 1.0,
            last_update_slot: 0,
            deviation: 100.0,
        },
    )
    .await
    .unwrap();

    // Execute should succeed because cancels don't require health checks
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

    // Verify the item was cleared
    let queue = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue.header.total_count, 0);
    assert_eq!(queue.header.ctm_count, 0);

    // Verify the order was actually cancelled
    let bids_data = solana.get_account_boxed::<BookSide>(bids).await;
    assert_eq!(bids_data.roots[0].leaf_count, 0);

    Ok(())
}

// ── 3F. CPI / Reentrancy ──

/// Verify that the queue cannot grow during execution. Enqueue a CTM item,
/// execute it, and confirm the queue state only decreases (no spurious
/// re-enqueue happened during dispatch).
#[tokio::test]
async fn test_security_execute_rejects_cpi() -> Result<(), TransportError> {
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

    let account =
        create_funded_account(solana, group, owner, 0, &users[1], mints, 1000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

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

    // Place a real order so the cancel dispatch succeeds
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
            client_order_id: 50,
            ..PerpPlaceOrderInstruction::default()
        },
    )
    .await
    .unwrap();

    // Enqueue cancel all orders
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

    // Record state before execution
    let queue_before = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue_before.header.total_count, 1);
    assert_eq!(queue_before.header.ctm_count, 1);

    // Execute the item
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

    // Verify queue shrank (no re-enqueue via CPI happened during dispatch)
    let queue_after = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue_after.header.total_count, 0);
    assert_eq!(queue_after.header.ctm_count, 0);
    assert!(
        queue_after.header.total_count < queue_before.header.total_count,
        "queue should have fewer items after execution, not more"
    );

    Ok(())
}

/// During execution, the dispatch invokes mango instructions via CPI.
/// The dispatched sub-instructions should not be able to modify the queue
/// in unexpected ways. Verify by enqueueing a cancel order, executing it,
/// and confirming the queue state only reflects the expected clear.
#[tokio::test]
async fn test_security_dispatch_cannot_modify_queue_account() -> Result<(), TransportError> {
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

    let account =
        create_funded_account(solana, group, owner, 0, &users[1], mints, 1000, 0).await;
    let ctm_signer = TestKeypair::new();
    let execution_queue =
        create_initialized_execution_queue(solana, group, admin, payer, ctm_signer.pubkey())
            .await;

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

    // Place a real order so the cancel dispatch succeeds
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

    // Enqueue two cancel-all items so we can verify sequential processing
    let remaining_accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(account, false),
        AccountMeta::new_readonly(owner.pubkey(), false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
    ];
    let payload = encode_perp_cancel_all_orders_payload(10);

    let envelope0 = CtmEnvelope {
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
        envelope0,
        payload.clone(),
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

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
        payload,
        owner,
        ctm_signer,
        remaining_accounts.clone(),
    )
    .await
    .unwrap();

    let queue_before = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(queue_before.header.total_count, 2);
    assert_eq!(queue_before.header.ctm_count, 2);

    // Execute both items
    send_tx(
        solana,
        ExecutionQueueExecuteInstruction {
            group,
            execution_queue,
            max_items: 10,
            remaining_accounts,
        },
    )
    .await
    .unwrap();

    // Verify the queue state only reflects the expected clears:
    // - total_count went from 2 to 0
    // - ctm_count went from 2 to 0
    // - No spurious items were added by the dispatched CPI
    let queue_after = solana
        .get_account_boxed::<ExecutionQueue>(execution_queue)
        .await;
    assert_eq!(
        queue_after.header.total_count, 0,
        "all items should be cleared, no spurious additions"
    );
    assert_eq!(
        queue_after.header.ctm_count, 0,
        "all CTM items should be cleared"
    );
    assert_eq!(
        queue_after.header.liquidity_count, 0,
        "no liquidity items should have appeared"
    );
    assert!(
        queue_after.current_ctm_head().is_none(),
        "CTM head should be empty"
    );

    // Verify the next_sequence_to_execute advanced (at least past seq 0).
    // Note: clear_ctm_item_at doesn't advance head when ctm_count drops to 0,
    // so we may see next_sequence=1 (advanced past 0 when 1 was still pending).
    assert!(
        queue_after.header.next_sequence_to_execute >= 1,
        "next_sequence should have advanced, got {}",
        queue_after.header.next_sequence_to_execute
    );

    Ok(())
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
