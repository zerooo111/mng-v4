//! v4 commit/reveal end-to-end benchmark with REAL perp dispatch.
//!
//! Uses the full test framework (group/USDC/oracle/perp market/mango
//! accounts) and drives real PerpPlaceOrderV2 intents through v4
//! commit_market + reveal_execute_market. Reports CU per reveal and
//! reveals-per-tx under real CU pressure.
//!
//! Run with:
//!   cargo test -p mango-v4 --features "test-bpf enable-gpl" \
//!     --test test_execution_queue_v4_bench -- --nocapture

#![cfg(feature = "test-bpf")]

mod program_test {
    include!("program_test/mod.rs");
}

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::sysvar;
use anchor_lang::AnchorSerialize;
use anchor_lang::{InstructionData, ToAccountMetas};
use fixed::types::I80F48;
use mango_v4::instructions::{CommitEntryV4, RevealArgsV4};
use mango_v4::state::{PerpMarket, Side, PlaceOrderType, SelfTradeBehavior};
use program_test::mango_setup::{Token, GroupWithTokens, GroupWithTokensConfig, create_funded_account};
use program_test::mango_client::*;
use program_test::solana::SolanaCookie;
use program_test::*;
use solana_program_test::*;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signature::Signer;
use solana_sdk::transport::TransportError;

const PAGE_SIZE: u64 = 256;

fn encode_perp_place_order_v2_payload(
    side: Side,
    price_lots: i64,
    max_base_lots: i64,
    client_order_id: u64,
) -> Vec<u8> {
    let body = mango_v4::instructions::PerpPlaceOrderV2Payload {
        side,
        price_lots,
        max_base_lots,
        max_quote_lots: i64::MAX,
        client_order_id,
        order_type: PlaceOrderType::Limit,
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        reduce_only: false,
        expiry_timestamp: 0,
        limit: 10,
    }
    .try_to_vec()
    .unwrap();
    let mut payload = Vec::with_capacity(4 + body.len());
    payload.push(1); // QUEUE_PAYLOAD_VERSION_V1
    payload.push(0); // variant PerpPlaceOrderV2
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&body);
    payload
}

fn hash_accounts(accounts: &[AccountMeta]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(accounts.len() * 34);
    for a in accounts {
        bytes.extend_from_slice(a.pubkey.as_ref());
        bytes.push(u8::from(a.is_signer));
        bytes.push(u8::from(a.is_writable));
    }
    hashv(&[&bytes]).to_bytes()
}

#[allow(clippy::too_many_arguments)]
fn canonical_commit_hash(
    group: Pubkey,
    market_index: u16,
    sequence: u64,
    kind: u8,
    payload_hash: &[u8; 32],
    accounts_hash: &[u8; 32],
    min_execute_slot: u64,
    expires_at_slot: u64,
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-commit-v1",
        group.as_ref(),
        &market_index.to_le_bytes(),
        &sequence.to_le_bytes(),
        &[kind],
        payload_hash,
        accounts_hash,
        &min_execute_slot.to_le_bytes(),
        &expires_at_slot.to_le_bytes(),
    ])
    .to_bytes()
}

fn canonical_commit_batch_message(
    group: Pubkey,
    market_index: u16,
    shard_id: u8,
    first_sequence: u64,
    entries: &[CommitEntryV4],
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(entries.len() * 48 + 64);
    buf.extend_from_slice(b"mango-v4-commit-batch-v1");
    buf.extend_from_slice(group.as_ref());
    buf.extend_from_slice(&market_index.to_le_bytes());
    buf.push(shard_id);
    buf.extend_from_slice(&first_sequence.to_le_bytes());
    buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in entries {
        buf.extend_from_slice(&e.commit_hash);
        buf.extend_from_slice(&e.min_execute_slot.to_le_bytes());
        buf.extend_from_slice(&e.expires_at_slot.to_le_bytes());
    }
    hashv(&[&buf]).to_bytes()
}

fn canonical_user_intent_v2(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    market_index: u16,
    payload_hash: &[u8; 32],
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-user-intent-v2",
        group.as_ref(),
        mango_account.as_ref(),
        user_owner.as_ref(),
        &[0u8], // kind = CtmWrapped
        &[0u8], // target_kind = PerpMarket
        &market_index.to_le_bytes(),
        payload_hash,
    ])
    .to_bytes()
}

fn build_presigned_ed25519_instruction(
    public_key: [u8; 32],
    message: &[u8],
    signature: [u8; 64],
) -> Instruction {
    let pk_off = 16u16;
    let sig_off = pk_off + 32;
    let msg_off = sig_off + 64;
    let mut data = vec![0u8; msg_off as usize + message.len()];
    data[0] = 1;
    data[1] = 0;
    data[2..4].copy_from_slice(&sig_off.to_le_bytes());
    data[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
    data[6..8].copy_from_slice(&pk_off.to_le_bytes());
    data[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    data[10..12].copy_from_slice(&msg_off.to_le_bytes());
    data[12..14].copy_from_slice(&(message.len() as u16).to_le_bytes());
    data[14..16].copy_from_slice(&u16::MAX.to_le_bytes());
    data[pk_off as usize..sig_off as usize].copy_from_slice(&public_key);
    data[sig_off as usize..msg_off as usize].copy_from_slice(&signature);
    data[msg_off as usize..].copy_from_slice(message);
    Instruction {
        program_id: ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

fn find_authority_state_pda(group: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"queue-authority", group.as_ref()], &mango_v4::id()).0
}

fn find_commit_queue_root_pda(group: Pubkey, market_index: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-root",
            group.as_ref(),
            &market_index.to_le_bytes(),
            &[0u8],
        ],
        &mango_v4::id(),
    )
    .0
}

fn find_commit_queue_page_pda(queue_root: Pubkey, page_slot: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-page",
            queue_root.as_ref(),
            &page_slot.to_le_bytes(),
        ],
        &mango_v4::id(),
    )
    .0
}

async fn setup_v4_queue(
    solana: &SolanaCookie,
    group: Pubkey,
    admin: TestKeypair,
    payer: TestKeypair,
    ctm_signer_pk: Pubkey,
    market_index: u16,
    num_pages: u16,
) -> Pubkey {
    let authority_state = find_authority_state_pda(group);

    // authority_state
    let ix = Instruction {
        program_id: mango_v4::id(),
        accounts: mango_v4::accounts::ExecutionQueueV3InitAuthorityState {
            group,
            authority_state,
            payer: payer.pubkey(),
            admin: admin.pubkey(),
            system_program: solana_sdk::system_program::id(),
        }
        .to_account_metas(None),
        data: mango_v4::instruction::ExecutionQueueV3InitAuthorityState {
            ctm_signer: ctm_signer_pk,
        }
        .data(),
    };
    solana
        .process_transaction(&[ix], Some(&[payer, admin]))
        .await
        .unwrap();

    // queue root
    let queue_root = find_commit_queue_root_pda(group, market_index);
    let ix = Instruction {
        program_id: mango_v4::id(),
        accounts: mango_v4::accounts::ExecutionQueueV4InitMarketRoot {
            group,
            authority_state,
            queue_root,
            payer: payer.pubkey(),
            admin: admin.pubkey(),
            system_program: solana_sdk::system_program::id(),
        }
        .to_account_metas(None),
        data: mango_v4::instruction::ExecutionQueueV4InitMarketRoot {
            market_index,
            shard_id: 0,
            params: mango_v4::instructions::ExecutionQueueV4MarketRootCreateParams {
                page_size: 256,
                num_pages,
                soft_limit: 0,
                gap_wait_slots: 4,
            },
        }
        .data(),
    };
    solana
        .process_transaction(&[ix], Some(&[payer, admin]))
        .await
        .unwrap();

    // pages (create + resize loop + init)
    for page_slot in 0..num_pages {
        let queue_page = find_commit_queue_page_pda(queue_root, page_slot);
        let create_ix = Instruction {
            program_id: mango_v4::id(),
            accounts: mango_v4::accounts::ExecutionQueueV4CreateMarketPage {
                group,
                authority_state,
                queue_root,
                queue_page,
                payer: payer.pubkey(),
                system_program: solana_sdk::system_program::id(),
            }
            .to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV4CreateMarketPage { page_slot }.data(),
        };
        solana.process_transaction(&[create_ix], Some(&[payer])).await.unwrap();

        // resize loop — each resize grows ~10 KB; page is ~24 KB
        const TARGET: usize = 8 + std::mem::size_of::<mango_v4::state::CommitPageV4>();
        loop {
            let acct = solana.context.borrow_mut().banks_client.get_account(queue_page).await.unwrap().unwrap();
            if acct.data.len() >= TARGET {
                break;
            }
            let resize_ix = Instruction {
                program_id: mango_v4::id(),
                accounts: mango_v4::accounts::ExecutionQueueV4ResizeMarketPage {
                    group,
                    authority_state,
                    queue_root,
                    queue_page,
                    payer: payer.pubkey(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::ExecutionQueueV4ResizeMarketPage { page_slot }.data(),
            };
            solana.process_transaction(&[resize_ix], Some(&[payer])).await.unwrap();
        }

        let init_ix = Instruction {
            program_id: mango_v4::id(),
            accounts: mango_v4::accounts::ExecutionQueueV4InitMarketPage {
                group,
                authority_state,
                queue_root,
                queue_page,
            }
            .to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV4InitMarketPage {
                page_slot,
                assigned_abs_page_no: page_slot as u64,
            }
            .data(),
        };
        solana.process_transaction(&[init_ix], Some(&[payer])).await.unwrap();
    }

    queue_root
}

/// One user's place-order intent, ready to commit and later reveal.
struct PreparedIntent {
    sequence: u64,
    payload: Vec<u8>,
    payload_hash: [u8; 32],
    dispatch_accounts: Vec<AccountMeta>,
    accounts_hash: [u8; 32],
    commit_hash: [u8; 32],
    user_owner: TestKeypair,
    mango_account: Pubkey,
}

fn build_place_order_dispatch_accounts(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    perp_market: Pubkey,
    bids: Pubkey,
    asks: Pubkey,
    event_queue: Pubkey,
    oracle: Pubkey,
    usdc_bank: Pubkey,
    usdc_oracle: Pubkey,
) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(group, false),
        AccountMeta::new(mango_account, false),
        AccountMeta::new_readonly(user_owner, false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
        AccountMeta::new(event_queue, false),
        AccountMeta::new_readonly(oracle, false),
        // health accounts: USDC bank + oracle for the deposit position
        AccountMeta::new(usdc_bank, false),
        AccountMeta::new_readonly(usdc_oracle, false),
    ]
}

#[allow(clippy::too_many_arguments)]
fn prepare_intents(
    group: Pubkey,
    market_index: u16,
    users: &[(TestKeypair, Pubkey)],
    perp_market: Pubkey,
    bids: Pubkey,
    asks: Pubkey,
    event_queue: Pubkey,
    oracle: Pubkey,
    usdc_bank: Pubkey,
    usdc_oracle: Pubkey,
    price_lots: i64,
    start_seq: u64,
) -> Vec<PreparedIntent> {
    users
        .iter()
        .enumerate()
        .map(|(i, (owner, mango_account))| {
            let sequence = start_seq + i as u64;
            let payload = encode_perp_place_order_v2_payload(
                if i % 2 == 0 { Side::Bid } else { Side::Ask },
                if i % 2 == 0 { price_lots - 10 } else { price_lots + 10 },
                1,
                10_000 + sequence,
            );
            let payload_hash = hashv(&[&payload]).to_bytes();
            let dispatch_accounts = build_place_order_dispatch_accounts(
                group,
                *mango_account,
                owner.pubkey(),
                perp_market,
                bids,
                asks,
                event_queue,
                oracle,
                usdc_bank,
                usdc_oracle,
            );
            let accounts_hash = hash_accounts(&dispatch_accounts);
            let commit_hash = canonical_commit_hash(
                group,
                market_index,
                sequence,
                0,
                &payload_hash,
                &accounts_hash,
                0,
                0,
            );
            PreparedIntent {
                sequence,
                payload,
                payload_hash,
                dispatch_accounts,
                accounts_hash,
                commit_hash,
                user_owner: *owner,
                mango_account: *mango_account,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn commit_batch(
    solana: &SolanaCookie,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    market_index: u16,
    first_sequence: u64,
    intents: &[&PreparedIntent],
    ctm_signer: TestKeypair,
    payer: TestKeypair,
) -> Result<(), TransportError> {
    let entries: Vec<CommitEntryV4> = intents
        .iter()
        .map(|it| CommitEntryV4 {
            commit_hash: it.commit_hash,
            min_execute_slot: 0,
            expires_at_slot: 0,
        })
        .collect();
    let batch_msg =
        canonical_commit_batch_message(group, market_index, 0, first_sequence, &entries);
    let sig: [u8; 64] = ctm_signer
        .to_keypair()
        .sign_message(&batch_msg)
        .as_ref()
        .try_into()
        .unwrap();
    let preix = build_presigned_ed25519_instruction(ctm_signer.pubkey().to_bytes(), &batch_msg, sig);

    let commit_ix = Instruction {
        program_id: mango_v4::id(),
        accounts: mango_v4::accounts::ExecutionQueueV4CommitMarket {
            group,
            authority_state,
            queue_root,
            queue_page,
            instructions: sysvar::instructions::id(),
        }
        .to_account_metas(None),
        data: mango_v4::instruction::ExecutionQueueV4CommitMarket {
            market_index,
            first_sequence,
            entries,
        }
        .data(),
    };

    let r = solana
        .process_transaction(&[preix, commit_ix], Some(&[payer]))
        .await?;
    r.result?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn reveal_execute_batch(
    solana: &SolanaCookie,
    group: Pubkey,
    market_index: u16,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    intents: &[&PreparedIntent],
    payer: TestKeypair,
) -> Result<u64 /* cu_used */, TransportError> {
    let mut reveals: Vec<RevealArgsV4> = Vec::new();
    let mut remaining: Vec<AccountMeta> = Vec::new();
    let mut user_preixs: Vec<Instruction> = Vec::new();
    for it in intents {
        reveals.push(RevealArgsV4 {
            payload: it.payload.clone(),
            kind: 0,
            dispatch_accounts_count: it.dispatch_accounts.len() as u8,
        });
        remaining.extend(it.dispatch_accounts.clone());
        let user_msg = canonical_user_intent_v2(
            group,
            it.mango_account,
            it.user_owner.pubkey(),
            market_index,
            &it.payload_hash,
        );
        let user_sig: [u8; 64] = it
            .user_owner
            .to_keypair()
            .sign_message(&user_msg)
            .as_ref()
            .try_into()
            .unwrap();
        user_preixs.push(build_presigned_ed25519_instruction(
            it.user_owner.pubkey().to_bytes(),
            &user_msg,
            user_sig,
        ));
    }

    let cu_limit_ix = solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(
        1_400_000,
    );

    let mut accounts = mango_v4::accounts::ExecutionQueueV4RevealExecuteMarket {
        group,
        authority_state,
        queue_root,
        queue_page,
        instructions: sysvar::instructions::id(),
    }
    .to_account_metas(None);
    accounts.extend(remaining);
    accounts.push(AccountMeta::new_readonly(mango_v4::id(), false));

    let reveal_ix = Instruction {
        program_id: mango_v4::id(),
        accounts,
        data: mango_v4::instruction::ExecutionQueueV4RevealExecuteMarket { reveals }.data(),
    };

    let mut ixs = vec![cu_limit_ix];
    ixs.extend(user_preixs);
    ixs.push(reveal_ix);
    let r = solana.process_transaction(&ixs, Some(&[payer])).await?;
    // Pull CU from the simulate (program-test doesn't return it directly on
    // process_transaction; simulate gives us the number).
    let sim = solana
        .context
        .borrow_mut()
        .banks_client
        .simulate_transaction(r.transaction.clone())
        .await
        .unwrap();
    let cu = sim.simulation_details.map(|d| d.units_consumed).unwrap_or(0);
    r.result?;
    Ok(cu)
}

#[tokio::test]
async fn test_v4_bench_e2e_place_cancel_10_users() -> Result<(), TransportError> {
    let mut builder = TestContextBuilder::new();
    let mints = builder.create_mints();
    let users = builder.create_users(&mints);
    let solana = builder.start().await;
    let solana = solana.as_ref();

    let admin = TestKeypair::new();
    let payer_user = &users[1];
    let payer = payer_user.key;
    // mints[0] = USDC, mints[1] = base token (for perp)
    let reg_mints = &mints[0..2];
    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: reg_mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;
    let usdc_bank = tokens[0].bank;
    let usdc_oracle = tokens[0].oracle;

    // 10 user accounts, each funded with USDC only (collateral).
    const N_USERS: usize = 10;
    let mut user_intents: Vec<(TestKeypair, Pubkey)> = Vec::with_capacity(N_USERS);
    for i in 0..N_USERS {
        let owner = users[(i % (users.len() - 1)) + 0].key;
        let account = create_funded_account(
            solana,
            group,
            owner,
            100 + i as u32, // account_num
            &users[1],
            &[reg_mints[0].clone()], // USDC only
            1_000_000,
            0,
        )
        .await;
        user_intents.push((owner, account));
    }

    // Perp market on base token.
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
            ..PerpCreateMarketInstruction::with_new_book_and_queue(solana, &tokens[1]).await
        },
    )
    .await
    .unwrap();

    let event_queue = solana
        .get_account::<PerpMarket>(perp_market)
        .await
        .event_queue;
    let oracle = solana.get_account::<PerpMarket>(perp_market).await.oracle;

    let price_lots = {
        let pm = solana.get_account::<PerpMarket>(perp_market).await;
        pm.native_price_to_lot(I80F48::ONE)
    };

    // v4 queue setup.
    let ctm_signer = TestKeypair::new();
    let market_index = 0u16;
    let queue_root = setup_v4_queue(solana, group, admin, payer, ctm_signer.pubkey(), market_index, 1).await;
    let authority_state = find_authority_state_pda(group);
    let queue_page0 = find_commit_queue_page_pda(queue_root, 0);

    // Prepare 10 intents.
    let intents = prepare_intents(
        group,
        market_index,
        &user_intents,
        perp_market,
        bids,
        asks,
        event_queue,
        oracle,
        usdc_bank,
        usdc_oracle,
        price_lots,
        0,
    );

    // Commit all 10 in one batch.
    let refs: Vec<&PreparedIntent> = intents.iter().collect();
    commit_batch(
        solana,
        group,
        authority_state,
        queue_root,
        queue_page0,
        market_index,
        0,
        &refs,
        ctm_signer,
        payer,
    )
    .await
    .unwrap();
    println!("== committed 10 intents in 1 tx ==");

    // Reveal-execute in batches; measure CU / reveals-per-tx.
    // Try N=5 first (our size-bound ceiling from earlier).
    for batch_size in [1usize, 2, 3, 5] {
        // Reset only if head got drained; otherwise advance from current head.
        let root_acct = solana
            .context
            .borrow_mut()
            .banks_client
            .get_account(queue_root)
            .await
            .unwrap()
            .unwrap();
        let root = <mango_v4::state::PerpMarketCommitRootV4 as anchor_lang::AccountDeserialize>::try_deserialize(
            &mut &root_acct.data[..],
        )
        .unwrap();
        let head = root.next_sequence_to_execute as usize;
        if head >= intents.len() {
            println!("all drained, stopping sweep");
            break;
        }
        let end = (head + batch_size).min(intents.len());
        let batch: Vec<&PreparedIntent> = (head..end).map(|i| &intents[i]).collect();
        if batch.is_empty() {
            break;
        }
        match reveal_execute_batch(
            solana,
            group,
            market_index,
            authority_state,
            queue_root,
            queue_page0,
            &batch,
            payer,
        )
        .await
        {
            Ok(cu) => {
                println!(
                    "== reveal batch={} seqs [{}, {}) cu_consumed={} cu_per_reveal={:.0} ==",
                    batch.len(),
                    head,
                    end,
                    cu,
                    cu as f64 / batch.len() as f64
                );
            }
            Err(e) => {
                println!("reveal batch={} seqs [{}, {}) failed: {:?}", batch.len(), head, end, e);
                break;
            }
        }
    }

    // Final state.
    let root_acct = solana
        .context
        .borrow_mut()
        .banks_client
        .get_account(queue_root)
        .await
        .unwrap()
        .unwrap();
    let root = <mango_v4::state::PerpMarketCommitRootV4 as anchor_lang::AccountDeserialize>::try_deserialize(
        &mut &root_acct.data[..],
    )
    .unwrap();
    println!(
        "final: next_sequence_to_execute={} live_count={}",
        root.next_sequence_to_execute, root.live_count
    );

    Ok(())
}
