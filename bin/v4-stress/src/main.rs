//! v4 commit-reveal queue stress harness.
//!
//! What this measures:
//!
//! 1. **Commit throughput end-to-end on chain.** Sets up a group, authority
//!    state, v4 queue root, and one commit page. Then lands N commit batches
//!    and reports commits/s, txs/s, and per-tx commit count.
//!
//! 2. **Reveal tx size.** Constructs reveal batches of varying count and
//!    reports the real serialized legacy-tx byte size (no ALT). Validates
//!    the packer's throughput model from commit_reveal_throughput.md.
//!
//! 3. **Commit tx size.** Same but for the commit side: how many entries
//!    fit in a 1232-byte legacy tx in practice.
//!
//! Dispatch is *not* exercised — reveals against synthetic mango_accounts
//! fail at `validate_queue_payload_dispatch_accounts`, which is downstream
//! of the commit_hash verify + payload decode + user-sig verify we care
//! about measuring. If you want full reveal-to-fill numbers, pair this with
//! the regular integration test harness.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use mango_v4::instructions::{
    CommitEntryV4, ExecutionQueueV4MarketRootCreateParams, RevealArgsV4,
};
use mango_v4::state::PerpMarketCommitRootV4;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::hash::hashv;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
    sysvar::{self, instructions::ID as INSTRUCTIONS_SYSVAR_ID},
    transaction::Transaction,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

mod real_e2e;

const PROGRAM_ID_STR: &str = "9rpAcg1jNmUydb4QoeCeJBGf8JfRuxLciRbS7AHGnXEq";

#[derive(Parser, Debug)]
struct Args {
    #[clap(long, default_value = "http://127.0.0.1:38899")]
    rpc: String,
    #[clap(long, default_value = "http://127.0.0.1:39010")]
    faucet: String,
    /// How many commits to land in total during the throughput run.
    #[clap(long, default_value_t = 2000)]
    commits: usize,
    /// Max entries per commit batch tx.
    #[clap(long, default_value_t = 15)]
    batch_size: usize,
    /// Perp market_index to use.
    #[clap(long, default_value_t = 0u16)]
    market_index: u16,
    /// Keypair path to reuse a payer; otherwise generate fresh.
    #[clap(long)]
    payer: Option<PathBuf>,
    /// Skip setup (assume group/root/page already initialized by a prior run).
    #[clap(long)]
    no_setup: bool,
    /// Parallel in-flight commit txs. 1 = fully serial; larger = pipelined.
    #[clap(long, default_value_t = 16)]
    inflight: usize,
    /// Reveal pipelining: "serial" sends one at a time with
    /// send_and_confirm; "pipelined" fires send_transaction back-to-back
    /// with `reveal_spacing_ms` between sends and waits at the end.
    #[clap(long, default_value = "pipelined")]
    reveal_mode: String,
    /// Milliseconds between successive pipelined reveal sends.
    #[clap(long, default_value_t = 50)]
    reveal_spacing_ms: u64,
    /// Number of queue pages to init in setup (1 supports up to page_size
    /// commits; more for larger runs).
    #[clap(long, default_value_t = 8)]
    num_pages: u16,
    /// Skip airdrop call (required on devnet; use --payer instead).
    #[clap(long)]
    no_airdrop: bool,
    /// Group number (part of the group PDA seeds — use a fresh one to
    /// sidestep AccountAlreadyInitialized after a bad prior setup).
    #[clap(long, default_value_t = 0u32)]
    group_num: u32,

    /// Mode: "synthetic" (old benchmark), "bootstrap-market",
    /// "bootstrap-users", "relay".
    #[clap(long, default_value = "synthetic")]
    mode: String,
    /// Path to market-state.json (for bootstrap-users/relay modes; also
    /// output path for bootstrap-market).
    #[clap(long, default_value = "/tmp/v4-market-state.json")]
    market_state: PathBuf,
    /// Path to users-state.json
    #[clap(long, default_value = "/tmp/v4-users-state.json")]
    users_state: PathBuf,
    /// Number of users for bootstrap-users
    #[clap(long, default_value_t = 10)]
    n_users: usize,
    /// Target TPS per user for relay mode
    #[clap(long, default_value_t = 5.0)]
    tps_per_user: f64,
    /// Run duration (seconds) for relay mode
    #[clap(long, default_value_t = 30u64)]
    run_seconds: u64,
    /// Base token price (for stub oracle seeding)
    #[clap(long, default_value_t = 100.0)]
    base_price: f64,
    /// For airdrop-usdc: path to newline-separated owner pubkeys.
    #[clap(long, default_value = "/tmp/senders.txt")]
    senders_file: PathBuf,
    /// For airdrop-usdc: human USDC amount per recipient.
    #[clap(long, default_value_t = 5000u64)]
    amount_usdc: u64,
    /// For bootstrap-extra-markets: starting market_index (exclusive of existing 0).
    #[clap(long, default_value_t = 1u16)]
    first_market_index: u16,
    /// For bootstrap-extra-markets: count of markets to create.
    #[clap(long, default_value_t = 2u16)]
    extra_count: u16,
    /// Extras state file path.
    #[clap(long, default_value = "/tmp/v4-extras-state.json")]
    extras_state: PathBuf,
    /// For bootstrap-extra-markets: reuse this on-chain oracle (Pyth etc.)
    /// instead of creating a fresh stub oracle + base mint.
    #[clap(long)]
    perp_oracle_override: Option<String>,
    /// For bootstrap-extra-markets: PerpMarket base_decimals (default matches
    /// current SOL-PERP sizing of 5 for a $0.001/SOL tick).
    #[clap(long, default_value_t = 5u8)]
    bootstrap_base_decimals: u8,
    /// For bootstrap-extra-markets: base_lot_size.
    #[clap(long, default_value_t = 100i64)]
    bootstrap_base_lot_size: i64,
    /// For bootstrap-extra-markets: quote_lot_size.
    #[clap(long, default_value_t = 1i64)]
    bootstrap_quote_lot_size: i64,
    /// For bootstrap-extra-markets: number of queue pages to init.
    #[clap(long, default_value_t = 4u16)]
    bootstrap_num_pages: u16,
}

fn program_id() -> Pubkey {
    PROGRAM_ID_STR.parse().unwrap()
}

// ---- PDA helpers -----------------------------------------------------------

fn find_group_pda(creator: Pubkey, group_num: u32) -> Pubkey {
    Pubkey::find_program_address(
        &[b"Group", creator.as_ref(), &group_num.to_le_bytes()],
        &program_id(),
    )
    .0
}

fn group_num_from_args(n: u32) -> u32 {
    n
}

fn find_authority_state_pda(group: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"queue-authority", group.as_ref()], &program_id()).0
}

fn find_commit_queue_root_pda(group: Pubkey, market_index: u16, shard_id: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-root",
            group.as_ref(),
            &market_index.to_le_bytes(),
            &[shard_id],
        ],
        &program_id(),
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
        &program_id(),
    )
    .0
}

fn find_insurance_vault_pda(group: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"InsuranceVault", group.as_ref()], &program_id()).0
}

// ---- hashing ---------------------------------------------------------------

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

fn hash_account_metas(metas: &[AccountMeta]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(metas.len() * 34);
    for a in metas {
        buf.extend_from_slice(a.pubkey.as_ref());
        buf.push(u8::from(a.is_signer));
        buf.push(u8::from(a.is_writable));
    }
    hashv(&[&buf]).to_bytes()
}

// ---- ed25519 pre-ix --------------------------------------------------------

fn build_single_ed25519_preix(pubkey: [u8; 32], sig: [u8; 64], msg: &[u8]) -> Instruction {
    let pk_off = 16u16;
    let sig_off = pk_off + 32;
    let msg_off = sig_off + 64;
    let mut data = vec![0u8; msg_off as usize + msg.len()];
    data[0] = 1;
    data[1] = 0;
    data[2..4].copy_from_slice(&sig_off.to_le_bytes());
    data[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
    data[6..8].copy_from_slice(&pk_off.to_le_bytes());
    data[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    data[10..12].copy_from_slice(&msg_off.to_le_bytes());
    data[12..14].copy_from_slice(&(msg.len() as u16).to_le_bytes());
    data[14..16].copy_from_slice(&u16::MAX.to_le_bytes());
    data[pk_off as usize..sig_off as usize].copy_from_slice(&pubkey);
    data[sig_off as usize..msg_off as usize].copy_from_slice(&sig);
    data[msg_off as usize..].copy_from_slice(msg);
    Instruction {
        program_id: ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

// ---- setup -----------------------------------------------------------------

async fn airdrop(rpc: &RpcClient, to: &Pubkey, lamports: u64) -> Result<()> {
    let sig = rpc.request_airdrop(to, lamports).await?;
    for _ in 0..60 {
        if let Ok(ok) = rpc.confirm_transaction(&sig).await {
            if ok {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(anyhow!("airdrop did not confirm"))
}

async fn send_tx(
    rpc: &RpcClient,
    ixs: Vec<Instruction>,
    payer: &Keypair,
    extra_signers: &[&Keypair],
) -> Result<()> {
    let blockhash = rpc.get_latest_blockhash().await?;
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend(extra_signers.iter().copied());
    // Deduplicate signer list preserving order
    let mut seen = BTreeSet::new();
    let signers: Vec<&Keypair> = signers
        .into_iter()
        .filter(|k| seen.insert(k.pubkey()))
        .collect();
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &signers, blockhash);
    rpc.send_and_confirm_transaction(&tx)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!("send: {e}"))
}

async fn create_spl_mint(
    rpc: &RpcClient,
    payer: &Keypair,
    mint_kp: &Keypair,
    decimals: u8,
) -> Result<()> {
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(spl_token_account_len())
        .await?;
    let create = system_instruction::create_account(
        &payer.pubkey(),
        &mint_kp.pubkey(),
        rent,
        spl_token_account_len() as u64,
        &spl_token_id(),
    );
    // spl-token init_mint is program_id = TOKEN_PROGRAM_ID. Hand-build it.
    let init = init_mint_ix(&mint_kp.pubkey(), &payer.pubkey(), decimals);
    send_tx(rpc, vec![create, init], payer, &[mint_kp]).await
}

// Avoid depending on spl-token crate (keeps the stress binary's dep tree small).
fn spl_token_id() -> Pubkey {
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap()
}
fn spl_token_account_len() -> usize {
    82
}
fn init_mint_ix(mint: &Pubkey, authority: &Pubkey, decimals: u8) -> Instruction {
    // InitializeMint2 = 20; layout: [20, decimals, 32 authority, 1 freeze=0]
    let mut data = vec![20u8, decimals];
    data.extend_from_slice(authority.as_ref());
    data.push(0u8); // freeze_authority = None
    Instruction {
        program_id: spl_token_id(),
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

async fn create_group(
    rpc: &RpcClient,
    payer: &Keypair,
    creator: &Keypair,
    insurance_mint: Pubkey,
    group_num: u32,
) -> Result<Pubkey> {
    let group = find_group_pda(creator.pubkey(), group_num);
    let insurance_vault = find_insurance_vault_pda(group);

    let accounts = mango_v4::accounts::GroupCreate {
        group,
        creator: creator.pubkey(),
        insurance_mint,
        insurance_vault,
        payer: payer.pubkey(),
        token_program: spl_token_id(),
        system_program: solana_sdk::system_program::id(),
        rent: sysvar::rent::id(),
    };
    let data = mango_v4::instruction::GroupCreate {
        group_num,
        testing: 1,
        version: 0,
    }
    .data();
    let ix = Instruction {
        program_id: program_id(),
        accounts: accounts.to_account_metas(None),
        data,
    };
    send_tx(rpc, vec![ix], payer, &[creator]).await?;
    Ok(group)
}

async fn init_authority_state(
    rpc: &RpcClient,
    payer: &Keypair,
    admin: &Keypair,
    group: Pubkey,
    ctm_signer: Pubkey,
) -> Result<Pubkey> {
    let authority_state = find_authority_state_pda(group);
    let accounts = mango_v4::accounts::ExecutionQueueV3InitAuthorityState {
        group,
        authority_state,
        payer: payer.pubkey(),
        admin: admin.pubkey(),
        system_program: solana_sdk::system_program::id(),
    };
    let data = mango_v4::instruction::ExecutionQueueV3InitAuthorityState { ctm_signer }.data();
    let ix = Instruction {
        program_id: program_id(),
        accounts: accounts.to_account_metas(None),
        data,
    };
    send_tx(rpc, vec![ix], payer, &[admin]).await?;
    Ok(authority_state)
}

async fn init_v4_market_root(
    rpc: &RpcClient,
    payer: &Keypair,
    admin: &Keypair,
    group: Pubkey,
    market_index: u16,
    num_pages: u16,
) -> Result<Pubkey> {
    let authority_state = find_authority_state_pda(group);
    let queue_root = find_commit_queue_root_pda(group, market_index, 0);
    let accounts = mango_v4::accounts::ExecutionQueueV4InitMarketRoot {
        group,
        authority_state,
        queue_root,
        payer: payer.pubkey(),
        admin: admin.pubkey(),
        system_program: solana_sdk::system_program::id(),
    };
    let data = mango_v4::instruction::ExecutionQueueV4InitMarketRoot {
        market_index,
        shard_id: 0,
        params: ExecutionQueueV4MarketRootCreateParams {
            page_size: 256,
            num_pages,
            soft_limit: 0,
            gap_wait_slots: 4,
        },
    }
    .data();
    let ix = Instruction {
        program_id: program_id(),
        accounts: accounts.to_account_metas(None),
        data,
    };
    send_tx(rpc, vec![ix], payer, &[admin]).await?;
    Ok(queue_root)
}

async fn init_v4_page(
    rpc: &RpcClient,
    payer: &Keypair,
    group: Pubkey,
    queue_root: Pubkey,
    page_slot: u16,
) -> Result<Pubkey> {
    let authority_state = find_authority_state_pda(group);
    let queue_page = find_commit_queue_page_pda(queue_root, page_slot);

    // create
    let create_accounts = mango_v4::accounts::ExecutionQueueV4CreateMarketPage {
        group,
        authority_state,
        queue_root,
        queue_page,
        payer: payer.pubkey(),
        system_program: solana_sdk::system_program::id(),
    };
    let create_ix = Instruction {
        program_id: program_id(),
        accounts: create_accounts.to_account_metas(None),
        data: mango_v4::instruction::ExecutionQueueV4CreateMarketPage { page_slot }.data(),
    };
    send_tx(rpc, vec![create_ix], payer, &[]).await?;

    // resize (chunked grow) — we loop until it reaches target size
    loop {
        let acct = rpc.get_account(&queue_page).await?;
        const TARGET: usize = 8 + std::mem::size_of::<mango_v4::state::CommitPageV4>();
        if acct.data.len() >= TARGET {
            break;
        }
        let resize_accounts = mango_v4::accounts::ExecutionQueueV4ResizeMarketPage {
            group,
            authority_state,
            queue_root,
            queue_page,
            payer: payer.pubkey(),
            system_program: solana_sdk::system_program::id(),
        };
        let ix = Instruction {
            program_id: program_id(),
            accounts: resize_accounts.to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV4ResizeMarketPage { page_slot }.data(),
        };
        send_tx(rpc, vec![ix], payer, &[]).await?;
    }

    // init
    let init_accounts = mango_v4::accounts::ExecutionQueueV4InitMarketPage {
        group,
        authority_state,
        queue_root,
        queue_page,
    };
    let init_ix = Instruction {
        program_id: program_id(),
        accounts: init_accounts.to_account_metas(None),
        data: mango_v4::instruction::ExecutionQueueV4InitMarketPage {
            page_slot,
            assigned_abs_page_no: page_slot as u64, // first cycle: abs == slot
        }
        .data(),
    };
    send_tx(rpc, vec![init_ix], payer, &[]).await?;
    Ok(queue_page)
}

// ---- commit tx construction ------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_commit_ix(
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    market_index: u16,
    first_sequence: u64,
    entries: Vec<CommitEntryV4>,
) -> Instruction {
    let accounts = mango_v4::accounts::ExecutionQueueV4CommitMarket {
        group,
        authority_state,
        queue_root,
        queue_page,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    };
    Instruction {
        program_id: program_id(),
        accounts: accounts.to_account_metas(None),
        data: mango_v4::instruction::ExecutionQueueV4CommitMarket {
            market_index,
            first_sequence,
            entries,
        }
        .data(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_commit_batch_tx(
    rpc_blockhash: solana_sdk::hash::Hash,
    payer: &Keypair,
    ctm_signer: &Keypair,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    market_index: u16,
    first_sequence: u64,
    entries: Vec<CommitEntryV4>,
) -> Transaction {
    let msg = canonical_commit_batch_message(group, market_index, 0, first_sequence, &entries);
    let sig: [u8; 64] = ctm_signer
        .sign_message(&msg)
        .as_ref()
        .try_into()
        .expect("64-byte sig");
    let preix = build_single_ed25519_preix(ctm_signer.pubkey().to_bytes(), sig, &msg);
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let commit_ix = build_commit_ix(
        group,
        authority_state,
        queue_root,
        queue_page,
        market_index,
        first_sequence,
        entries,
    );
    Transaction::new_signed_with_payer(
        &[cu, preix, commit_ix],
        Some(&payer.pubkey()),
        &[payer],
        rpc_blockhash,
    )
}

// ---- reveal tx construction (size measurement only) ------------------------

fn build_reveal_ix(
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    remaining: Vec<AccountMeta>,
    reveals: Vec<RevealArgsV4>,
) -> Instruction {
    let mut base = mango_v4::accounts::ExecutionQueueV4RevealExecuteMarket {
        group,
        authority_state,
        queue_root,
        queue_page,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    base.extend(remaining);
    base.push(AccountMeta::new_readonly(program_id(), false));
    Instruction {
        program_id: program_id(),
        accounts: base,
        data: mango_v4::instruction::ExecutionQueueV4RevealExecuteMarket { reveals }.data(),
    }
}

fn synth_reveal(with_user_sig: bool, payload_len: usize) -> (RevealArgsV4, Vec<AccountMeta>) {
    // 45 bytes is the PerpPlaceOrderV2 envelope size on the wire.
    let payload = vec![0u8; payload_len];
    // 3 dispatch accounts = {mango_account, perp_market, oracle} sized sample.
    let accts = vec![
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
    ];
    (
        RevealArgsV4 {
            payload,
            kind: 0,
            dispatch_accounts_count: accts.len() as u8,
        },
        accts,
    )
}

/// Deterministic synthetic intent for sequence `seq`. Produces a
/// PerpPlaceOrderV2 payload with expiry_timestamp=1 — guaranteed terminal
/// at reveal (triggers `TerminalCtmFailureReason::Expired`), so the reveal
/// exercises hash verify + decode + prevalidate without requiring a real
/// PerpMarket / MangoAccount to be present on chain.
fn synthetic_intent(seq: u64) -> (Vec<u8>, Vec<AccountMeta>) {
    let mut payload = vec![0u8; 49];
    payload[0] = 1; // QUEUE_PAYLOAD_VERSION_V1
    payload[1] = 0; // variant PerpPlaceOrderV2
    // body @ offset 4
    payload[4] = 0; // side = Bid
    payload[5..13].copy_from_slice(&1000i64.to_le_bytes()); // price_lots
    payload[13..21].copy_from_slice(&1i64.to_le_bytes()); // max_base_lots
    payload[21..29].copy_from_slice(&1i64.to_le_bytes()); // max_quote_lots
    payload[29..37].copy_from_slice(&seq.to_le_bytes()); // client_order_id
    payload[37] = 2; // order_type = Limit
    payload[38] = 0; // self_trade_behavior = DecrementTake
    payload[39] = 0; // reduce_only = false
    payload[40..48].copy_from_slice(&1u64.to_le_bytes()); // expiry_timestamp = 1 (past)
    payload[48] = 32; // limit
    let mut accts = Vec::with_capacity(3);
    for i in 0u8..3 {
        // Avoid reserved pubkeys (system program is all-zeros; sysvars begin
        // with Sysvar... prefix). 0x42 prefix steers clear of both.
        let mut seed = [0x42u8; 32];
        seed[0..8].copy_from_slice(&seq.to_le_bytes());
        seed[8] = i;
        // seed[9..32] stays 0x42, distinguishing from any well-known pubkey.
        accts.push(AccountMeta::new(Pubkey::new_from_array(seed), false));
    }
    (payload, accts)
}

/// Build a reveal tx with N reveals, then add N user-sig ed25519 pre-ixs if
/// requested, and return the serialized size in bytes.
fn measure_reveal_tx_size(
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    n_reveals: usize,
    with_user_sigs: bool,
    payload_len: usize,
    blockhash: solana_sdk::hash::Hash,
    payer: &Keypair,
) -> usize {
    let mut reveals = Vec::with_capacity(n_reveals);
    let mut accts: Vec<AccountMeta> = Vec::with_capacity(n_reveals * 3);
    let mut preixs: Vec<Instruction> = vec![];
    if with_user_sigs {
        for _ in 0..n_reveals {
            let user = Keypair::new();
            let msg = [0u8; 32];
            let sig: [u8; 64] = user.sign_message(&msg).as_ref().try_into().unwrap();
            preixs.push(build_single_ed25519_preix(
                user.pubkey().to_bytes(),
                sig,
                &msg,
            ));
        }
    }
    for _ in 0..n_reveals {
        let (r, a) = synth_reveal(with_user_sigs, payload_len);
        reveals.push(r);
        accts.extend(a);
    }
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let reveal_ix = build_reveal_ix(group, authority_state, queue_root, queue_page, accts, reveals);
    let mut ixs = vec![cu];
    ixs.extend(preixs);
    ixs.push(reveal_ix);
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer], blockhash);
    bincode::serialize(&tx).unwrap().len()
}

// ---- main ------------------------------------------------------------------

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Real-dispatch modes — delegate to real_e2e module and return early.
    match args.mode.as_str() {
        "bootstrap-market" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            real_e2e::bootstrap_market(real_e2e::BootstrapMarketArgs {
                rpc: args.rpc.clone(),
                program: PROGRAM_ID_STR.parse().unwrap(),
                admin_keypair: admin,
                output: args.market_state.clone(),
                group_num: args.group_num,
                base_price: args.base_price,
                perp_market_index: args.market_index,
            })
            .await?;
            return Ok(());
        }
        "bootstrap-existing-users" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            let market = real_e2e::MarketState::load(&args.market_state)?;
            let list_path = std::path::PathBuf::from(args.senders_file.clone());
            let raw = std::fs::read_to_string(&list_path)?;
            let mut keypair_paths: Vec<std::path::PathBuf> = Vec::new();
            for line in raw.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') { continue; }
                keypair_paths.push(std::path::PathBuf::from(t));
            }
            real_e2e::bootstrap_existing_users(real_e2e::BootstrapExistingUsersArgs {
                rpc: args.rpc.clone(),
                admin_keypair: admin,
                market_state: market,
                output: args.users_state.clone(),
                keypair_paths,
                sol_per_user: 50_000_000, // 0.05 SOL (for mango_account rent + fees)
                usdc_per_user: args.amount_usdc.saturating_mul(1_000_000),
            })
            .await?;
            return Ok(());
        }
        "bootstrap-users" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            let market = real_e2e::MarketState::load(&args.market_state)?;
            real_e2e::bootstrap_users(real_e2e::BootstrapUsersArgs {
                rpc: args.rpc.clone(),
                admin_keypair: admin,
                market_state: market,
                output: args.users_state.clone(),
                n_users: args.n_users,
                sol_per_user: 10_000_000, // 0.01 SOL (fees only; admin pays mango/ATA rent)
                usdc_per_user: 10_000_000_000, // 10,000 USDC (6 decimals)
            })
            .await?;
            return Ok(());
        }
        "bootstrap-extra-markets" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            let market = real_e2e::MarketState::load(&args.market_state)?;
            let perp_oracle_override = args
                .perp_oracle_override
                .as_ref()
                .map(|s| s.parse::<solana_sdk::pubkey::Pubkey>())
                .transpose()
                .map_err(|e| anyhow!("bad --perp-oracle-override: {e}"))?;
            real_e2e::bootstrap_extra_markets(real_e2e::BootstrapExtraMarketsArgs {
                rpc: args.rpc.clone(),
                admin_keypair: admin,
                market_state: market,
                extras_output: args.extras_state.clone(),
                first_market_index: args.first_market_index,
                count: args.extra_count,
                base_price: args.base_price,
                perp_oracle_override,
                base_decimals: args.bootstrap_base_decimals,
                base_lot_size: args.bootstrap_base_lot_size,
                quote_lot_size: args.bootstrap_quote_lot_size,
                num_pages: args.bootstrap_num_pages,
            })
            .await?;
            return Ok(());
        }
        "airdrop-usdc" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            let market = real_e2e::MarketState::load(&args.market_state)?;
            real_e2e::airdrop_usdc(real_e2e::AirdropUsdcArgs {
                rpc: args.rpc.clone(),
                admin_keypair: admin,
                market_state: market,
                senders_file: args.senders_file.clone(),
                amount_usdc: args.amount_usdc,
            })
            .await?;
            return Ok(());
        }
        "close-market" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            real_e2e::close_market_recover(real_e2e::BootstrapMarketArgs {
                rpc: args.rpc.clone(),
                program: PROGRAM_ID_STR.parse().unwrap(),
                admin_keypair: admin,
                output: args.market_state.clone(),
                group_num: args.group_num,
                base_price: args.base_price,
                perp_market_index: args.market_index,
            })
            .await?;
            return Ok(());
        }
        "debug-reveal" => {
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            let market = real_e2e::MarketState::load(&args.market_state)?;
            let users = real_e2e::UsersState::load(&args.users_state)?;
            real_e2e::debug_reveal_one(real_e2e::RelayArgs {
                rpc: args.rpc.clone(),
                admin_keypair: admin,
                market_state: market,
                users,
                target_tps_per_user: args.tps_per_user,
                run_seconds: args.run_seconds,
                reveal_spacing_ms: args.reveal_spacing_ms,
                commit_batch_size: args.batch_size,
            })
            .await?;
            return Ok(());
        }
        "relay" => {
            eprintln!("[main] mode=relay");
            let kp_path = args.payer.clone().ok_or_else(|| anyhow!("--payer required"))?;
            let admin = read_keypair_file(&kp_path).map_err(|e| anyhow!("kp: {e}"))?;
            eprintln!("[main] loaded admin kp");
            let market = real_e2e::MarketState::load(&args.market_state)?;
            eprintln!("[main] loaded market state");
            let users = real_e2e::UsersState::load(&args.users_state)?;
            eprintln!("[main] loaded users state (n={})", users.users.len());
            real_e2e::run_relay(real_e2e::RelayArgs {
                rpc: args.rpc.clone(),
                admin_keypair: admin,
                market_state: market,
                users,
                target_tps_per_user: args.tps_per_user,
                run_seconds: args.run_seconds,
                reveal_spacing_ms: args.reveal_spacing_ms,
                commit_batch_size: args.batch_size,
            })
            .await?;
            return Ok(());
        }
        _ => {} // "synthetic" — fall through to old flow
    }

    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());

    let payer = match &args.payer {
        Some(p) => read_keypair_file(p).map_err(|e| anyhow!("keypair: {e}"))?,
        None => Keypair::new(),
    };
    // Use the payer keypair as the ctm_signer too so that `--no-setup` runs
    // against a previously-initialized authority_state keep working.
    let ctm = Keypair::from_bytes(&payer.to_bytes()).unwrap();
    let creator = Keypair::from_bytes(&payer.to_bytes()).unwrap();
    println!("payer={}  ctm_signer={}", payer.pubkey(), ctm.pubkey());

    if !args.no_airdrop {
        // Airdrop 100 SOL (localnet only).
        airdrop(&rpc, &payer.pubkey(), 100 * 1_000_000_000)
            .await
            .context("airdrop")?;
    } else {
        let bal = rpc.get_balance(&payer.pubkey()).await?;
        println!("payer balance: {} SOL", bal as f64 / 1e9);
    }

    let num_pages: u16 = args.num_pages;
    const PAGE_SIZE: u64 = 256;
    let (group, authority_state, queue_root) = if args.no_setup {
        let group = find_group_pda(creator.pubkey(), args.group_num);
        let authority_state = find_authority_state_pda(group);
        let queue_root = find_commit_queue_root_pda(group, args.market_index, 0);
        (group, authority_state, queue_root)
    } else {
        println!("-- setup --");
        let mint = Keypair::new();
        create_spl_mint(&rpc, &payer, &mint, 6).await.context("mint")?;
        println!("mint={}", mint.pubkey());
        let group = create_group(&rpc, &payer, &creator, mint.pubkey(), args.group_num)
            .await
            .context("group")?;
        println!("group={}", group);
        let authority_state = init_authority_state(&rpc, &payer, &creator, group, ctm.pubkey())
            .await
            .context("authority_state")?;
        println!("authority_state={}", authority_state);
        let queue_root =
            init_v4_market_root(&rpc, &payer, &creator, group, args.market_index, num_pages)
                .await
                .context("queue_root")?;
        println!("queue_root={}", queue_root);
        for page_slot in 0..num_pages {
            let qp = init_v4_page(&rpc, &payer, group, queue_root, page_slot)
                .await
                .with_context(|| format!("page_slot={}", page_slot))?;
            println!("queue_page[{}]={}", page_slot, qp);
        }
        (group, authority_state, queue_root)
    };
    let queue_page = find_commit_queue_page_pda(queue_root, 0);

    // ---- commit throughput ------------------------------------------------
    println!("\n== commit throughput ==");
    println!(
        "target commits={} batch_size={}",
        args.commits, args.batch_size
    );

    let entries_per_batch = args.batch_size;
    let mut landed = 0usize;
    let mut tx_count = 0usize;
    // Resume from the current next_enqueue_sequence so we don't collide with
    // slots committed on a prior run.
    let mut first_sequence: u64 = {
        let acct = rpc.get_account(&queue_root).await?;
        let root = PerpMarketCommitRootV4::try_deserialize(&mut &acct.data[..])?;
        root.next_enqueue_sequence()
    };
    println!("starting at first_sequence={}", first_sequence);
    let start_first_sequence = first_sequence;
    let mut commit_tx_sizes: Vec<usize> = Vec::new();
    println!("in-flight parallel sends = {}", args.inflight);
    let t0 = Instant::now();

    // Pipelined send loop: queue up to `inflight` send futures, harvest as
    // they confirm. Each one is independent (different first_sequence) and
    // can land in any order — the on-chain queue accepts out-of-order
    // commits within the admission window.
    let rpc_arc = std::sync::Arc::new(rpc);
    use tokio::task::JoinHandle;
    let mut inflight_tasks: Vec<JoinHandle<(u64, usize, Result<solana_sdk::signature::Signature, solana_client::client_error::ClientError>)>> = Vec::new();
    let mut next_seq: u64 = start_first_sequence;
    let stop_seq: u64 = start_first_sequence + args.commits as u64;

    loop {
        while inflight_tasks.len() < args.inflight && next_seq < stop_seq {
            let remaining = (stop_seq - next_seq) as usize;
            let mut n = remaining.min(entries_per_batch);
            let start_page = next_seq / PAGE_SIZE;
            let end_page = (next_seq + n as u64 - 1) / PAGE_SIZE;
            if start_page != end_page {
                let boundary = (start_page + 1) * PAGE_SIZE;
                n = (boundary - next_seq) as usize;
            }
            let page_slot = (start_page % num_pages as u64) as u16;
            let current_page = find_commit_queue_page_pda(queue_root, page_slot);

            let mut entries: Vec<CommitEntryV4> = Vec::with_capacity(n);
            for i in 0..n {
                let seq = next_seq + i as u64;
                let (payload, accts) = synthetic_intent(seq);
                let payload_hash = hashv(&[&payload]).to_bytes();
                let accounts_hash = hash_account_metas(&accts);
                let commit_hash = canonical_commit_hash(
                    group,
                    args.market_index,
                    seq,
                    0, // kind = CtmWrapped
                    &payload_hash,
                    &accounts_hash,
                    0, // min_execute_slot
                    0, // expires_at_slot
                );
                entries.push(CommitEntryV4 {
                    commit_hash,
                    min_execute_slot: 0,
                    expires_at_slot: 0,
                });
            }
            let blockhash = rpc_arc.get_latest_blockhash().await?;
            let tx = build_commit_batch_tx(
                blockhash,
                &payer,
                &ctm,
                group,
                authority_state,
                queue_root,
                current_page,
                args.market_index,
                next_seq,
                entries.clone(),
            );
            let tx_bytes = bincode::serialize(&tx).unwrap().len();
            commit_tx_sizes.push(tx_bytes);
            let rpc_cl = rpc_arc.clone();
            let seq_of_this = next_seq;
            let n_of_this = n;
            inflight_tasks.push(tokio::spawn(async move {
                let r = rpc_cl.send_and_confirm_transaction(&tx).await;
                (seq_of_this, n_of_this, r)
            }));
            next_seq += n as u64;
        }
        if inflight_tasks.is_empty() {
            break;
        }
        // Harvest: wait for the oldest in-flight task.
        let handle = inflight_tasks.remove(0);
        match handle.await {
            Ok((_, n, Ok(_))) => {
                landed += n;
                tx_count += 1;
            }
            Ok((seq, _n, Err(e))) => {
                eprintln!("commit tx failed at seq={}: {}", seq, e);
                for t in inflight_tasks.drain(..) {
                    let _ = t.await;
                }
                break;
            }
            Err(je) => {
                eprintln!("join err: {}", je);
                break;
            }
        }
    }
    let rpc = rpc_arc;

    let elapsed = t0.elapsed();
    let commits_per_s = landed as f64 / elapsed.as_secs_f64();
    let txs_per_s = tx_count as f64 / elapsed.as_secs_f64();
    let avg_bytes = commit_tx_sizes.iter().sum::<usize>() as f64 / commit_tx_sizes.len() as f64;
    let max_bytes = commit_tx_sizes.iter().max().copied().unwrap_or(0);
    println!(
        "commits_landed={}  txs_sent={}  elapsed={:.3}s\n\
         commits/s={:.1}  txs/s={:.1}  avg_tx_bytes={:.0}  max_tx_bytes={}",
        landed,
        tx_count,
        elapsed.as_secs_f64(),
        commits_per_s,
        txs_per_s,
        avg_bytes,
        max_bytes
    );

    // Read back the queue root to confirm state.
    let root_acct = rpc.get_account(&queue_root).await?;
    let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])?;
    println!(
        "on-chain queue_root: max_seen_sequence={}  next_sequence_to_execute={}  live_count={}",
        root.max_seen_sequence, root.next_sequence_to_execute, root.live_count
    );

    // ---- commit tx size sweep --------------------------------------------
    println!("\n== commit tx size sweep (legacy) ==");
    let blockhash = rpc.get_latest_blockhash().await?;
    println!("entries    tx_bytes  under_1232");
    for n in [1usize, 5, 10, 15, 17, 18, 20, 25, 30, 50] {
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let mut ch = [0u8; 32];
            ch[0] = i as u8;
            entries.push(CommitEntryV4 {
                commit_hash: ch,
                min_execute_slot: 0,
                expires_at_slot: 0,
            });
        }
        let tx = build_commit_batch_tx(
            blockhash,
            &payer,
            &ctm,
            group,
            authority_state,
            queue_root,
            queue_page,
            args.market_index,
            0,
            entries,
        );
        let sz = bincode::serialize(&tx).unwrap().len();
        println!(
            "{:>7}    {:>8}  {}",
            n,
            sz,
            if sz <= 1232 { "yes" } else { "NO" }
        );
    }

    // ---- reveal throughput (e2e) -----------------------------------------
    println!(
        "\n== reveal throughput (e2e, mode={}, spacing={}ms) ==",
        args.reveal_mode, args.reveal_spacing_ms
    );
    let reveals_per_tx: usize = 5; // fits legacy tx @ 45B payload, no user sig

    // Helper: build a single reveal tx for sequences [start, start+n).
    let build_reveal_tx_for = |start: u64,
                               n: usize,
                               blockhash: solana_sdk::hash::Hash|
     -> (Transaction, usize) {
        let abs_page_no = start / PAGE_SIZE;
        let page_slot = (abs_page_no % num_pages as u64) as u16;
        let qp = find_commit_queue_page_pda(queue_root, page_slot);
        let mut reveals: Vec<RevealArgsV4> = Vec::with_capacity(n);
        let mut accts: Vec<AccountMeta> = Vec::with_capacity(n * 3);
        for i in 0..n as u64 {
            let (payload, a) = synthetic_intent(start + i);
            reveals.push(RevealArgsV4 {
                payload,
                kind: 0,
                dispatch_accounts_count: a.len() as u8,
            });
            accts.extend(a);
        }
        let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
        let ix = build_reveal_ix(group, authority_state, queue_root, qp, accts, reveals);
        let tx = Transaction::new_signed_with_payer(
            &[cu, ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        let sz = bincode::serialize(&tx).unwrap().len();
        (tx, sz)
    };

    // Plan all batches up front; every one targets a contiguous seq range
    // bounded by page boundaries.
    let mut planned: Vec<(u64, usize)> = Vec::new();
    let mut seq_cursor: u64 = start_first_sequence;
    while seq_cursor < stop_seq {
        let remaining = stop_seq - seq_cursor;
        let mut n = (reveals_per_tx as u64).min(remaining);
        let page_start = seq_cursor / PAGE_SIZE;
        let page_end_seq = (page_start + 1) * PAGE_SIZE - 1;
        if seq_cursor + n - 1 > page_end_seq {
            n = page_end_seq - seq_cursor + 1;
        }
        planned.push((seq_cursor, n as usize));
        seq_cursor += n;
    }

    let mut revealed = 0usize;
    let mut reveal_tx_count = 0usize;
    let mut reveal_tx_sizes: Vec<usize> = Vec::new();
    let t_r0 = Instant::now();

    match args.reveal_mode.as_str() {
        "serial" => {
            for (start, n) in planned.iter() {
                let bh = rpc.get_latest_blockhash().await?;
                let (tx, sz) = build_reveal_tx_for(*start, *n, bh);
                reveal_tx_sizes.push(sz);
                match rpc.send_and_confirm_transaction(&tx).await {
                    Ok(_) => {
                        revealed += *n;
                        reveal_tx_count += 1;
                    }
                    Err(e) => {
                        eprintln!("serial reveal seq={} failed: {}", start, e);
                        break;
                    }
                }
            }
        }
        "pipelined" | _ => {
            // Fire send_transaction back-to-back with reveal_spacing_ms gaps,
            // keeping signatures to confirm later. Relies on Solana's
            // writable-account lock on queue_page to serialize execution
            // within a block in submission order.
            use solana_client::rpc_config::RpcSendTransactionConfig;
            let send_cfg = RpcSendTransactionConfig {
                skip_preflight: true,
                preflight_commitment: None,
                encoding: None,
                max_retries: Some(0),
                min_context_slot: None,
            };
            let mut sigs: Vec<(u64, usize, solana_sdk::signature::Signature)> =
                Vec::with_capacity(planned.len());
            let bh = rpc.get_latest_blockhash().await?;
            let t_send = Instant::now();
            for (i, (start, n)) in planned.iter().enumerate() {
                let (tx, sz) = build_reveal_tx_for(*start, *n, bh);
                reveal_tx_sizes.push(sz);
                match rpc.send_transaction_with_config(&tx, send_cfg).await {
                    Ok(sig) => sigs.push((*start, *n, sig)),
                    Err(e) => {
                        eprintln!("send[{}] seq={} err: {}", i, start, e);
                    }
                }
                if i + 1 < planned.len() {
                    tokio::time::sleep(Duration::from_millis(args.reveal_spacing_ms)).await;
                }
            }
            let send_elapsed = t_send.elapsed();
            println!(
                "pipelined sends: {} sent in {:.3}s ({:.1}/s)",
                sigs.len(),
                send_elapsed.as_secs_f64(),
                sigs.len() as f64 / send_elapsed.as_secs_f64()
            );

            // Wait for final head to advance; resubmit failed ranges.
            let target_head = stop_seq;
            let deadline = Instant::now() + Duration::from_secs(60);
            while Instant::now() < deadline {
                let root_acct = rpc.get_account(&queue_root).await?;
                let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])?;
                if root.next_sequence_to_execute >= target_head {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let root_acct = rpc.get_account(&queue_root).await?;
            let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])?;
            revealed = root
                .next_sequence_to_execute
                .saturating_sub(start_first_sequence) as usize;
            reveal_tx_count = sigs.len();

            // Retry any gaps (failed txs) serially.
            if root.next_sequence_to_execute < stop_seq {
                let retry_start = root.next_sequence_to_execute;
                println!(
                    "retrying reveals from seq={} (initial pipelined pass missed some)",
                    retry_start
                );
                let mut s = retry_start;
                while s < stop_seq {
                    let remaining = stop_seq - s;
                    let mut n = (reveals_per_tx as u64).min(remaining);
                    let page_end_seq = (s / PAGE_SIZE + 1) * PAGE_SIZE - 1;
                    if s + n - 1 > page_end_seq {
                        n = page_end_seq - s + 1;
                    }
                    let bh = rpc.get_latest_blockhash().await?;
                    let (tx, sz) = build_reveal_tx_for(s, n as usize, bh);
                    reveal_tx_sizes.push(sz);
                    match rpc.send_and_confirm_transaction(&tx).await {
                        Ok(_) => {
                            reveal_tx_count += 1;
                            s += n;
                        }
                        Err(e) => {
                            eprintln!("retry seq={} failed: {}", s, e);
                            break;
                        }
                    }
                }
                let root_acct = rpc.get_account(&queue_root).await?;
                let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])?;
                revealed = root
                    .next_sequence_to_execute
                    .saturating_sub(start_first_sequence) as usize;
            }
        }
    }

    let r_elapsed = t_r0.elapsed();
    let reveals_per_s = revealed as f64 / r_elapsed.as_secs_f64();
    let r_txs_per_s = reveal_tx_count as f64 / r_elapsed.as_secs_f64();
    let r_avg_bytes = if reveal_tx_sizes.is_empty() {
        0.0
    } else {
        reveal_tx_sizes.iter().sum::<usize>() as f64 / reveal_tx_sizes.len() as f64
    };
    println!(
        "revealed={}  reveal_txs={}  elapsed={:.3}s\n\
         reveals/s={:.1}  reveal_txs/s={:.1}  avg_reveal_tx_bytes={:.0}",
        revealed,
        reveal_tx_count,
        r_elapsed.as_secs_f64(),
        reveals_per_s,
        r_txs_per_s,
        r_avg_bytes
    );
    let root_acct2 = rpc.get_account(&queue_root).await?;
    let root2 = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct2.data[..])?;
    println!(
        "after reveals: next_sequence_to_execute={}  live_count={}",
        root2.next_sequence_to_execute, root2.live_count
    );

    // ---- reveal tx size sweep ---------------------------------------------
    println!("\n== reveal tx size sweep (legacy) ==");
    println!("mode                               n=1   n=2   n=3   n=5   n=8   n=12");
    for (label, with_sigs, payload_len) in [
        ("reveal, 45B payload, no user sig ", false, 45usize),
        ("reveal, 45B payload, user sig    ", true, 45usize),
        ("reveal, 8B payload, no user sig  ", false, 8usize),
        ("reveal, 8B payload, user sig     ", true, 8usize),
    ] {
        let mut row = format!("{}", label);
        for n in [1usize, 2, 3, 5, 8, 12] {
            let sz = measure_reveal_tx_size(
                group,
                authority_state,
                queue_root,
                queue_page,
                n,
                with_sigs,
                payload_len,
                blockhash,
                &payer,
            );
            let tag = if sz <= 1232 { "  " } else { "!!" };
            row.push_str(&format!(" {:>4}{}", sz, tag));
        }
        println!("{}", row);
    }
    println!("(!! = would exceed the 1232-byte legacy tx cap)");

    Ok(())
}
