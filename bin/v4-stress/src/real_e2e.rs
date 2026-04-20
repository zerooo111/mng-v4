//! Full real-dispatch e2e bootstrap + relay pipeline.
//!
//! Three sub-phases:
//!   1. `bootstrap-market` — USDC mint, stub oracles, token_register,
//!      perp_create_market. Writes `market-state.json`.
//!   2. `bootstrap-users N` — generate N user keypairs, fund each with SOL,
//!      create USDC ATAs, mint USDC to them, create mango accounts, deposit
//!      USDC as collateral. Writes `users-state.json`.
//!   3. `relay --tps X` — run a TPS-driven load generator across all
//!      bootstrapped users: each emits place/cancel intents at the target
//!      rate; a commit batcher flushes in 12-entry batches; a reveal worker
//!      pipelines reveals at 50ms spacing.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anyhow::{anyhow, Context, Result};
use mango_v4::accounts_ix::InterestRateParams;
use mango_v4::instructions::{CommitEntryV4, RevealArgsV4};
use mango_v4::state::{
    OracleConfigParams, PerpMarket, PerpMarketCommitRootV4, PlaceOrderType, SelfTradeBehavior, Side,
    StablePriceModel,
};
use serde::{Deserialize, Serialize};
use solana_client::{nonblocking::rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};
use solana_program::hash::hashv;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    system_instruction,
    sysvar::{self, instructions::ID as INSTRUCTIONS_SYSVAR_ID},
    transaction::Transaction,
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use anchor_lang::AnchorSerialize;

const SPL_TOKEN: Pubkey = solana_sdk::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ATA_PROGRAM: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// -------------------------- persisted state --------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MarketState {
    pub program_id: String,
    pub group: String,
    pub admin_pubkey: String, // same as payer on our setup
    pub authority_state: String,
    pub ctm_signer_pubkey: String, // = payer so --no-setup works
    pub queue_root: String,
    pub queue_page0: String,
    // token (USDC) pieces
    pub usdc_mint: String,
    pub usdc_bank: String,
    pub usdc_vault: String,
    pub usdc_oracle: String,
    // perp pieces
    pub perp_market: String,
    pub perp_bids: String,
    pub perp_asks: String,
    pub perp_event_queue: String,
    pub perp_oracle: String,
    pub perp_market_index: u16,
    pub base_lot_size: i64,
    pub quote_lot_size: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserState {
    pub owner_secret: Vec<u8>, // raw keypair bytes (64)
    pub mango_account: String,
    pub usdc_ata: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UsersState {
    pub users: Vec<UserState>,
}

impl MarketState {
    pub fn load(p: &PathBuf) -> Result<Self> {
        let s = std::fs::read_to_string(p)?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn save(&self, p: &PathBuf) -> Result<()> {
        std::fs::write(p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
impl UsersState {
    pub fn load(p: &PathBuf) -> Result<Self> {
        let s = std::fs::read_to_string(p)?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn save(&self, p: &PathBuf) -> Result<()> {
        std::fs::write(p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

// -------------------------- ix primitives ---------------------------------

pub fn program_id(s: &str) -> Pubkey {
    s.parse().unwrap()
}

pub fn find_group_pda(program: Pubkey, creator: Pubkey, group_num: u32) -> Pubkey {
    Pubkey::find_program_address(
        &[b"Group", creator.as_ref(), &group_num.to_le_bytes()],
        &program,
    )
    .0
}
pub fn find_authority_state_pda(program: Pubkey, group: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"queue-authority", group.as_ref()], &program).0
}
pub fn find_commit_queue_root_pda(
    program: Pubkey,
    group: Pubkey,
    market_index: u16,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-root",
            group.as_ref(),
            &market_index.to_le_bytes(),
            &[0u8],
        ],
        &program,
    )
    .0
}
pub fn find_commit_queue_page_pda(program: Pubkey, queue_root: Pubkey, page_slot: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-page",
            queue_root.as_ref(),
            &page_slot.to_le_bytes(),
        ],
        &program,
    )
    .0
}
pub fn find_insurance_vault_pda(program: Pubkey, group: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"InsuranceVault", group.as_ref()], &program).0
}
pub fn find_bank_pda(program: Pubkey, group: Pubkey, token_index: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"Bank".as_ref(),
            group.as_ref(),
            &token_index.to_le_bytes(),
            &0u32.to_le_bytes(),
        ],
        &program,
    )
    .0
}
pub fn find_vault_pda(program: Pubkey, group: Pubkey, token_index: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"Vault".as_ref(),
            group.as_ref(),
            &token_index.to_le_bytes(),
            &0u32.to_le_bytes(),
        ],
        &program,
    )
    .0
}
pub fn find_mint_info_pda(program: Pubkey, group: Pubkey, mint: Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"MintInfo", group.as_ref(), mint.as_ref()],
        &program,
    )
    .0
}
pub fn find_perp_market_pda(program: Pubkey, group: Pubkey, perp_market_index: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"PerpMarket",
            group.as_ref(),
            &perp_market_index.to_le_bytes(),
        ],
        &program,
    )
    .0
}
pub fn find_mango_account_pda(
    program: Pubkey,
    group: Pubkey,
    owner: Pubkey,
    account_num: u32,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"MangoAccount",
            group.as_ref(),
            owner.as_ref(),
            &account_num.to_le_bytes(),
        ],
        &program,
    )
    .0
}
pub fn find_ata(owner: Pubkey, mint: Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), SPL_TOKEN.as_ref(), mint.as_ref()],
        &ATA_PROGRAM,
    )
    .0
}

// -------------------------- SPL helpers -----------------------------------

fn init_mint_ix(mint: &Pubkey, authority: &Pubkey, decimals: u8) -> Instruction {
    // InitializeMint2 = 20; [20, decimals, 32 authority, 1 freeze=0]
    let mut data = vec![20u8, decimals];
    data.extend_from_slice(authority.as_ref());
    data.push(0u8);
    Instruction {
        program_id: SPL_TOKEN,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}
fn mint_to_ix(mint: &Pubkey, destination: &Pubkey, authority: &Pubkey, amount: u64) -> Instruction {
    // MintTo = 7; [7, u64 amount]
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: SPL_TOKEN,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}
fn create_ata_idempotent_ix(funder: &Pubkey, owner: &Pubkey, mint: &Pubkey) -> Instruction {
    // AssociatedTokenProgram: CreateIdempotent = 1
    let ata = find_ata(*owner, *mint);
    Instruction {
        program_id: ATA_PROGRAM,
        accounts: vec![
            AccountMeta::new(*funder, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(SPL_TOKEN, false),
        ],
        data: vec![1u8],
    }
}

// -------------------------- tx submission helpers ------------------------

pub async fn send_confirmed(
    rpc: &RpcClient,
    ixs: Vec<Instruction>,
    payer: &Keypair,
    extra: &[&Keypair],
) -> Result<Signature> {
    let blockhash = rpc.get_latest_blockhash().await?;
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend(extra.iter().copied());
    let mut seen = std::collections::BTreeSet::new();
    let signers: Vec<&Keypair> = signers
        .into_iter()
        .filter(|k| seen.insert(k.pubkey()))
        .collect();
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &signers, blockhash);
    match rpc.send_and_confirm_transaction(&tx).await {
        Ok(sig) => Ok(sig),
        Err(e) => {
            // Print sim logs for debug
            if let Ok(sim) = rpc.simulate_transaction(&tx).await {
                if let Some(logs) = sim.value.logs {
                    eprintln!("-- sim logs --");
                    for l in logs {
                        eprintln!("  {}", l);
                    }
                }
            }
            Err(anyhow!("send: {e}"))
        }
    }
}

// -------------------------- hashing ---------------------------------------

pub fn hash_account_metas(metas: &[AccountMeta]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(metas.len() * 34);
    for a in metas {
        buf.extend_from_slice(a.pubkey.as_ref());
        buf.push(u8::from(a.is_signer));
        buf.push(u8::from(a.is_writable));
    }
    hashv(&[&buf]).to_bytes()
}

/// Mirrors Solana runtime's flag-merge behavior: when the same pubkey
/// appears multiple times across instruction accounts, all references
/// receive the OR'd flags (signer OR, writable OR). We need this on the
/// offchain side so the accounts_hash we commit matches the on-chain
/// recompute at reveal.
pub fn hash_account_metas_runtime(metas: &[AccountMeta]) -> [u8; 32] {
    use std::collections::BTreeMap;
    let mut flags: BTreeMap<Pubkey, (bool, bool)> = BTreeMap::new();
    for a in metas {
        let e = flags.entry(a.pubkey).or_insert((false, false));
        e.0 |= a.is_signer;
        e.1 |= a.is_writable;
    }
    let mut buf = Vec::with_capacity(metas.len() * 34);
    for a in metas {
        let (s, w) = flags.get(&a.pubkey).copied().unwrap_or((a.is_signer, a.is_writable));
        buf.extend_from_slice(a.pubkey.as_ref());
        buf.push(u8::from(s));
        buf.push(u8::from(w));
    }
    hashv(&[&buf]).to_bytes()
}

#[allow(clippy::too_many_arguments)]
pub fn canonical_commit_hash(
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

pub fn canonical_commit_batch_message(
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

pub fn canonical_user_intent_v2(
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
        &[0u8],
        &[0u8],
        &market_index.to_le_bytes(),
        payload_hash,
    ])
    .to_bytes()
}

pub fn build_presigned_ed25519_instruction(
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

pub fn encode_perp_place_order_v2_payload(
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
    payload.push(1);
    payload.push(0);
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&body);
    payload
}

// -------------------------- bootstrap -------------------------------------

pub struct BootstrapMarketArgs {
    pub rpc: String,
    pub program: Pubkey,
    pub admin_keypair: Keypair,
    pub output: PathBuf,
    pub group_num: u32,
    pub base_price: f64,  // e.g. 100.0 for SOL
    pub perp_market_index: u16,
}

pub async fn bootstrap_market(args: BootstrapMarketArgs) -> Result<MarketState> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = &args.admin_keypair;
    let program = args.program;
    let bal = rpc.get_balance(&admin.pubkey()).await?;
    println!("admin={} balance={} SOL", admin.pubkey(), bal as f64 / 1e9);

    // 1. Create USDC mint (admin is mint authority so we can mint to users later).
    let usdc_mint_kp = Keypair::new();
    {
        let rent = rpc.get_minimum_balance_for_rent_exemption(82).await?;
        let create = system_instruction::create_account(
            &admin.pubkey(),
            &usdc_mint_kp.pubkey(),
            rent,
            82,
            &SPL_TOKEN,
        );
        let init = init_mint_ix(&usdc_mint_kp.pubkey(), &admin.pubkey(), 6);
        send_confirmed(&rpc, vec![create, init], admin, &[&usdc_mint_kp]).await?;
    }
    println!("usdc_mint={}", usdc_mint_kp.pubkey());

    // 2. Create stub oracle for USDC (price=1.0). StubOracleCreate's
    // accounts struct has #[account(init)] so the instruction itself
    // allocates — do NOT pre-create the account.
    let usdc_oracle_kp = Keypair::new();
    {
        let ix_accounts = mango_v4::accounts::StubOracleCreate {
            group: find_group_pda(program, admin.pubkey(), args.group_num),
            oracle: usdc_oracle_kp.pubkey(),
            admin: admin.pubkey(),
            mint: usdc_mint_kp.pubkey(),
            payer: admin.pubkey(),
            system_program: solana_sdk::system_program::id(),
        };
        let ix = Instruction {
            program_id: program,
            accounts: ix_accounts.to_account_metas(None),
            data: mango_v4::instruction::StubOracleCreate {
                price: fixed::types::I80F48::from_num(1.0),
            }
            .data(),
        };
        // Need to create group first if not done. Check.
        let group = find_group_pda(program, admin.pubkey(), args.group_num);
        if rpc.get_account(&group).await.is_err() {
            let insurance_vault = find_insurance_vault_pda(program, group);
            let gix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::GroupCreate {
                    group,
                    creator: admin.pubkey(),
                    insurance_mint: usdc_mint_kp.pubkey(),
                    insurance_vault,
                    payer: admin.pubkey(),
                    token_program: SPL_TOKEN,
                    system_program: solana_sdk::system_program::id(),
                    rent: sysvar::rent::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::GroupCreate {
                    group_num: args.group_num,
                    testing: 1,
                    version: 0,
                }
                .data(),
            };
            send_confirmed(&rpc, vec![gix], admin, &[]).await?;
            println!("group={}", group);
        }
        send_confirmed(&rpc, vec![ix], admin, &[&usdc_oracle_kp]).await?;
    }
    println!("usdc_oracle={}", usdc_oracle_kp.pubkey());

    let group = find_group_pda(program, admin.pubkey(), args.group_num);

    // 3. token_register for USDC @ token_index=0.
    {
        let token_index = 0u16;
        let bank = find_bank_pda(program, group, token_index);
        let vault = find_vault_pda(program, group, token_index);
        let mint_info = find_mint_info_pda(program, group, usdc_mint_kp.pubkey());
        let ix_accounts = mango_v4::accounts::TokenRegister {
            group,
            admin: admin.pubkey(),
            mint: usdc_mint_kp.pubkey(),
            bank,
            vault,
            mint_info,
            oracle: usdc_oracle_kp.pubkey(),
            fallback_oracle: Pubkey::default(),
            payer: admin.pubkey(),
            token_program: SPL_TOKEN,
            system_program: solana_sdk::system_program::id(),
        };
        let ix = Instruction {
            program_id: program,
            accounts: ix_accounts.to_account_metas(None),
            data: mango_v4::instruction::TokenRegister {
                token_index,
                name: "USDC".to_string(),
                oracle_config: OracleConfigParams {
                    conf_filter: 0.1,
                    max_staleness_slots: None,
                },
                interest_rate_params: InterestRateParams {
                    adjustment_factor: 0.004,
                    util0: 0.5,
                    rate0: 0.07,
                    util1: 0.8,
                    rate1: 0.9,
                    max_rate: 1.5,
                },
                loan_fee_rate: 0.005,
                loan_origination_fee_rate: 0.0005,
                maint_asset_weight: 1.0,
                init_asset_weight: 1.0,
                maint_liab_weight: 1.0,
                init_liab_weight: 1.0,
                liquidation_fee: 0.0,
                stable_price_delay_interval_seconds: StablePriceModel::default().delay_interval_seconds,
                stable_price_delay_growth_limit: StablePriceModel::default().delay_growth_limit,
                stable_price_growth_limit: StablePriceModel::default().stable_growth_limit,
                min_vault_to_deposits_ratio: 0.0,
                net_borrow_limit_per_window_quote: i64::MAX,
                net_borrow_limit_window_size_ts: 24 * 60 * 60,
                borrow_weight_scale_start_quote: f64::MAX,
                deposit_weight_scale_start_quote: f64::MAX,
                reduce_only: 0,
                token_conditional_swap_taker_fee_rate: 0.0,
                token_conditional_swap_maker_fee_rate: 0.0,
                flash_loan_swap_fee_rate: 0.0,
                interest_curve_scaling: 1.0,
                interest_target_utilization: 0.5,
                group_insurance_fund: true,
                deposit_limit: 0,
                zero_util_rate: 0.0,
                platform_liquidation_fee: 0.0,
                disable_asset_liquidation: false,
                collateral_fee_per_day: 0.0,
            }
            .data(),
        };
        send_confirmed(&rpc, vec![ix], admin, &[]).await?;
        println!("usdc_bank={}", bank);
    }
    let usdc_bank = find_bank_pda(program, group, 0);
    let usdc_vault = find_vault_pda(program, group, 0);

    // 4. Stub oracle for base (SOL).
    let perp_oracle_kp = Keypair::new();
    // base "mint" doesn't need to be a real SPL mint — stub oracle's 'mint'
    // field just stores decimals. We create a throwaway mint for decimals=9.
    let base_mint_kp = Keypair::new();
    {
        let rent = rpc.get_minimum_balance_for_rent_exemption(82).await?;
        let create = system_instruction::create_account(
            &admin.pubkey(),
            &base_mint_kp.pubkey(),
            rent,
            82,
            &SPL_TOKEN,
        );
        let init = init_mint_ix(&base_mint_kp.pubkey(), &admin.pubkey(), 9);
        send_confirmed(&rpc, vec![create, init], admin, &[&base_mint_kp]).await?;

        let ix_accounts = mango_v4::accounts::StubOracleCreate {
            group,
            oracle: perp_oracle_kp.pubkey(),
            admin: admin.pubkey(),
            mint: base_mint_kp.pubkey(),
            payer: admin.pubkey(),
            system_program: solana_sdk::system_program::id(),
        };
        let ix = Instruction {
            program_id: program,
            accounts: ix_accounts.to_account_metas(None),
            data: mango_v4::instruction::StubOracleCreate {
                price: fixed::types::I80F48::from_num(args.base_price),
            }
            .data(),
        };
        send_confirmed(&rpc, vec![ix], admin, &[&perp_oracle_kp]).await?;
    }
    println!("perp_oracle={}  base_mint={}", perp_oracle_kp.pubkey(), base_mint_kp.pubkey());

    // 5. Perp create market — pre-create bids/asks/event_queue big accounts.
    let bids_kp = Keypair::new();
    let asks_kp = Keypair::new();
    let eq_kp = Keypair::new();
    let book_space = 8 + std::mem::size_of::<mango_v4::state::BookSide>();
    let eq_space = 8 + std::mem::size_of::<mango_v4::state::EventQueue>();
    println!(
        "  preallocating bids={} asks={} eq={}",
        book_space, book_space, eq_space
    );
    for (kp, space) in [
        (&bids_kp, book_space),
        (&asks_kp, book_space),
        (&eq_kp, eq_space),
    ] {
        let rent = rpc.get_minimum_balance_for_rent_exemption(space).await?;
        let create = system_instruction::create_account(
            &admin.pubkey(),
            &kp.pubkey(),
            rent,
            space as u64,
            &program,
        );
        send_confirmed(&rpc, vec![create], admin, &[kp]).await?;
    }
    let perp_market = find_perp_market_pda(program, group, args.perp_market_index);
    {
        let ix_accounts = mango_v4::accounts::PerpCreateMarket {
            group,
            admin: admin.pubkey(),
            oracle: perp_oracle_kp.pubkey(),
            perp_market,
            bids: bids_kp.pubkey(),
            asks: asks_kp.pubkey(),
            event_queue: eq_kp.pubkey(),
            payer: admin.pubkey(),
            system_program: solana_sdk::system_program::id(),
        };
        let ix = Instruction {
            program_id: program,
            accounts: ix_accounts.to_account_metas(None),
            data: mango_v4::instruction::PerpCreateMarket {
                name: "SOL-PERP".to_string(),
                oracle_config: OracleConfigParams {
                    conf_filter: 0.1,
                    max_staleness_slots: None,
                },
                settle_token_index: 0,
                perp_market_index: args.perp_market_index,
                quote_lot_size: 100,
                base_lot_size: 100,
                maint_base_asset_weight: 0.95,
                init_base_asset_weight: 0.9,
                maint_base_liab_weight: 1.05,
                init_base_liab_weight: 1.1,
                maint_overall_asset_weight: 1.0,
                init_overall_asset_weight: 1.0,
                base_liquidation_fee: 0.02,
                maker_fee: -0.0001,
                taker_fee: 0.0002,
                max_funding: 0.05,
                min_funding: 0.05,
                impact_quantity: 100,
                base_decimals: 9,
                group_insurance_fund: true,
                fee_penalty: 0.0,
                settle_fee_flat: 0.0,
                settle_fee_amount_threshold: 0.0,
                settle_fee_fraction_low_health: 0.0,
                settle_pnl_limit_factor: -1.0,
                settle_pnl_limit_window_size_ts: 24 * 60 * 60,
                positive_pnl_liquidation_fee: 0.0,
                platform_liquidation_fee: 0.0,
            }
            .data(),
        };
        send_confirmed(&rpc, vec![ix], admin, &[]).await?;
    }
    println!("perp_market={}", perp_market);

    // 6. v4 queue setup (authority_state, queue_root, 1 page).
    let authority_state = find_authority_state_pda(program, group);
    {
        let ix = Instruction {
            program_id: program,
            accounts: mango_v4::accounts::ExecutionQueueV3InitAuthorityState {
                group,
                authority_state,
                payer: admin.pubkey(),
                admin: admin.pubkey(),
                system_program: solana_sdk::system_program::id(),
            }
            .to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV3InitAuthorityState {
                ctm_signer: admin.pubkey(),
            }
            .data(),
        };
        send_confirmed(&rpc, vec![ix], admin, &[]).await?;
    }
    let queue_root = find_commit_queue_root_pda(program, group, args.perp_market_index);
    {
        let ix = Instruction {
            program_id: program,
            accounts: mango_v4::accounts::ExecutionQueueV4InitMarketRoot {
                group,
                authority_state,
                queue_root,
                payer: admin.pubkey(),
                admin: admin.pubkey(),
                system_program: solana_sdk::system_program::id(),
            }
            .to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV4InitMarketRoot {
                market_index: args.perp_market_index,
                shard_id: 0,
                params: mango_v4::instructions::ExecutionQueueV4MarketRootCreateParams {
                    page_size: 256,
                    num_pages: 4,
                    soft_limit: 0,
                    gap_wait_slots: 4,
                },
            }
            .data(),
        };
        send_confirmed(&rpc, vec![ix], admin, &[]).await?;
    }
    let queue_page0 = find_commit_queue_page_pda(program, queue_root, 0);
    {
        let create_ix = Instruction {
            program_id: program,
            accounts: mango_v4::accounts::ExecutionQueueV4CreateMarketPage {
                group,
                authority_state,
                queue_root,
                queue_page: queue_page0,
                payer: admin.pubkey(),
                system_program: solana_sdk::system_program::id(),
            }
            .to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV4CreateMarketPage { page_slot: 0 }.data(),
        };
        send_confirmed(&rpc, vec![create_ix], admin, &[]).await?;
        const TARGET: usize = 8 + std::mem::size_of::<mango_v4::state::CommitPageV4>();
        loop {
            let acct = rpc.get_account(&queue_page0).await?;
            if acct.data.len() >= TARGET {
                break;
            }
            let resize_ix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::ExecutionQueueV4ResizeMarketPage {
                    group,
                    authority_state,
                    queue_root,
                    queue_page: queue_page0,
                    payer: admin.pubkey(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::ExecutionQueueV4ResizeMarketPage { page_slot: 0 }.data(),
            };
            send_confirmed(&rpc, vec![resize_ix], admin, &[]).await?;
        }
        let init_ix = Instruction {
            program_id: program,
            accounts: mango_v4::accounts::ExecutionQueueV4InitMarketPage {
                group,
                authority_state,
                queue_root,
                queue_page: queue_page0,
            }
            .to_account_metas(None),
            data: mango_v4::instruction::ExecutionQueueV4InitMarketPage {
                page_slot: 0,
                assigned_abs_page_no: 0,
            }
            .data(),
        };
        send_confirmed(&rpc, vec![init_ix], admin, &[]).await?;
    }

    let state = MarketState {
        program_id: program.to_string(),
        group: group.to_string(),
        admin_pubkey: admin.pubkey().to_string(),
        authority_state: authority_state.to_string(),
        ctm_signer_pubkey: admin.pubkey().to_string(),
        queue_root: queue_root.to_string(),
        queue_page0: queue_page0.to_string(),
        usdc_mint: usdc_mint_kp.pubkey().to_string(),
        usdc_bank: usdc_bank.to_string(),
        usdc_vault: usdc_vault.to_string(),
        usdc_oracle: usdc_oracle_kp.pubkey().to_string(),
        perp_market: perp_market.to_string(),
        perp_bids: bids_kp.pubkey().to_string(),
        perp_asks: asks_kp.pubkey().to_string(),
        perp_event_queue: eq_kp.pubkey().to_string(),
        perp_oracle: perp_oracle_kp.pubkey().to_string(),
        perp_market_index: args.perp_market_index,
        base_lot_size: 100,
        quote_lot_size: 100,
    };
    state.save(&args.output)?;
    println!("wrote {}", args.output.display());
    Ok(state)
}

pub struct BootstrapUsersArgs {
    pub rpc: String,
    pub admin_keypair: Keypair,
    pub market_state: MarketState,
    pub output: PathBuf,
    pub n_users: usize,
    pub sol_per_user: u64, // lamports
    pub usdc_per_user: u64, // USDC (base units, 6 decimals)
}

pub async fn bootstrap_users(args: BootstrapUsersArgs) -> Result<UsersState> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = &args.admin_keypair;
    let program: Pubkey = args.market_state.program_id.parse()?;
    let group: Pubkey = args.market_state.group.parse()?;
    let usdc_mint: Pubkey = args.market_state.usdc_mint.parse()?;
    let usdc_bank: Pubkey = args.market_state.usdc_bank.parse()?;
    let usdc_vault: Pubkey = args.market_state.usdc_vault.parse()?;
    let usdc_oracle: Pubkey = args.market_state.usdc_oracle.parse()?;

    // Incrementally save as users are created — easier recovery on failure.
    let mut users: Vec<UserState> = if args.output.exists() {
        match UsersState::load(&args.output) {
            Ok(s) => { println!("resuming with {} existing users", s.users.len()); s.users }
            Err(_) => Vec::with_capacity(args.n_users),
        }
    } else {
        Vec::with_capacity(args.n_users)
    };
    let start = users.len();
    for i in start..args.n_users {
        let owner = Keypair::new();
        println!("user[{}]={}", i, owner.pubkey());
        // 1. fund with SOL
        let fund = system_instruction::transfer(&admin.pubkey(), &owner.pubkey(), args.sol_per_user);
        // 2. create USDC ATA
        let ata = find_ata(owner.pubkey(), usdc_mint);
        let create_ata = create_ata_idempotent_ix(&admin.pubkey(), &owner.pubkey(), &usdc_mint);
        // 3. mint USDC to ATA
        let mint_to = mint_to_ix(&usdc_mint, &ata, &admin.pubkey(), args.usdc_per_user);
        // 4. create mango account
        let mango_account = find_mango_account_pda(program, group, owner.pubkey(), 0);
        let account_create = Instruction {
            program_id: program,
            accounts: mango_v4::accounts::AccountCreate {
                group,
                account: mango_account,
                owner: owner.pubkey(),
                payer: admin.pubkey(),
                system_program: solana_sdk::system_program::id(),
            }
            .to_account_metas(None),
            data: mango_v4::instruction::AccountCreate {
                account_num: 0,
                token_count: 8,
                serum3_count: 0,
                perp_count: 4,
                perp_oo_count: 64,
                name: format!("u{}", i),
            }
            .data(),
        };
        send_confirmed(
            &rpc,
            vec![fund, create_ata, mint_to, account_create],
            admin,
            &[&owner],
        )
        .await?;

        // 5. deposit USDC into mango account
        let deposit_ix = Instruction {
            program_id: program,
            accounts: mango_v4::accounts::TokenDeposit {
                group,
                account: mango_account,
                owner: owner.pubkey(),
                bank: usdc_bank,
                vault: usdc_vault,
                oracle: usdc_oracle,
                token_account: ata,
                token_authority: owner.pubkey(),
                token_program: SPL_TOKEN,
            }
            .to_account_metas(None),
            data: mango_v4::instruction::TokenDeposit {
                amount: args.usdc_per_user,
                reduce_only: false,
            }
            .data(),
        };
        send_confirmed(&rpc, vec![deposit_ix], admin, &[&owner]).await?;

        users.push(UserState {
            owner_secret: owner.to_bytes().to_vec(),
            mango_account: mango_account.to_string(),
            usdc_ata: ata.to_string(),
        });
        // Save progress incrementally.
        UsersState { users: users.clone() }.save(&args.output)?;
    }

    let state = UsersState { users };
    state.save(&args.output)?;
    println!("wrote {} users", state.users.len());
    Ok(state)
}

// -----------------------------------------------------------------------------
// bootstrap-existing-users: same as bootstrap-users but loads keypairs from a
// caller-supplied list of paths (instead of generating fresh ones) and SKIPS
// the deposit step. Useful when bot operators already hold the keypairs and
// will run token_deposit themselves.
// -----------------------------------------------------------------------------

pub struct BootstrapExistingUsersArgs {
    pub rpc: String,
    pub admin_keypair: Keypair,
    pub market_state: MarketState,
    pub output: PathBuf,
    pub keypair_paths: Vec<PathBuf>,
    pub sol_per_user: u64,
    pub usdc_per_user: u64,
}

pub async fn bootstrap_existing_users(args: BootstrapExistingUsersArgs) -> Result<UsersState> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = &args.admin_keypair;
    let program: Pubkey = args.market_state.program_id.parse()?;
    let group: Pubkey = args.market_state.group.parse()?;
    let usdc_mint: Pubkey = args.market_state.usdc_mint.parse()?;

    let mut users: Vec<UserState> = Vec::with_capacity(args.keypair_paths.len());
    for (i, path) in args.keypair_paths.iter().enumerate() {
        let owner = solana_sdk::signer::keypair::read_keypair_file(path)
            .map_err(|e| anyhow!("kp {}: {e}", path.display()))?;
        println!(
            "user[{}]={} (from {})",
            i,
            owner.pubkey(),
            path.display()
        );

        let ata = find_ata(owner.pubkey(), usdc_mint);
        let mango_account = find_mango_account_pda(program, group, owner.pubkey(), 0);

        // Skip ix if mango_account already exists.
        let mango_exists = rpc.get_account(&mango_account).await.is_ok();

        let mut ixs: Vec<Instruction> = Vec::new();
        // 1. fund with SOL (always safe; small amount)
        if args.sol_per_user > 0 {
            ixs.push(system_instruction::transfer(
                &admin.pubkey(),
                &owner.pubkey(),
                args.sol_per_user,
            ));
        }
        // 2. idempotent create ATA
        ixs.push(create_ata_idempotent_ix(
            &admin.pubkey(),
            &owner.pubkey(),
            &usdc_mint,
        ));
        // 3. mint USDC to ATA
        if args.usdc_per_user > 0 {
            ixs.push(mint_to_ix(
                &usdc_mint,
                &ata,
                &admin.pubkey(),
                args.usdc_per_user,
            ));
        }
        // 4. create mango account (if not already)
        if !mango_exists {
            ixs.push(Instruction {
                program_id: program,
                accounts: mango_v4::accounts::AccountCreate {
                    group,
                    account: mango_account,
                    owner: owner.pubkey(),
                    payer: admin.pubkey(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::AccountCreate {
                    account_num: 0,
                    token_count: 8,
                    serum3_count: 0,
                    perp_count: 4,
                    perp_oo_count: 64,
                    name: format!("bot-{}", i),
                }
                .data(),
            });
        }

        send_confirmed(&rpc, ixs, admin, &[&owner]).await?;

        users.push(UserState {
            owner_secret: owner.to_bytes().to_vec(),
            mango_account: mango_account.to_string(),
            usdc_ata: ata.to_string(),
        });
        UsersState { users: users.clone() }.save(&args.output)?;
    }

    let state = UsersState { users };
    state.save(&args.output)?;
    println!("wrote {} existing-user bootstraps", state.users.len());
    Ok(state)
}

// -------------------------- relay -----------------------------------------

#[derive(Clone)]
pub struct Intent {
    pub sequence: u64,
    pub owner_index: usize,
    pub payload: Vec<u8>,
    pub payload_hash: [u8; 32],
    pub dispatch_accounts: Vec<AccountMeta>,
    pub accounts_hash: [u8; 32],
    pub commit_hash: [u8; 32],
    pub user_sig: [u8; 64],
    pub user_msg: [u8; 32],
}

fn build_dispatch_accounts(
    group: Pubkey,
    mango_account: Pubkey,
    owner: Pubkey,
    perp_market: Pubkey,
    bids: Pubkey,
    asks: Pubkey,
    event_queue: Pubkey,
    perp_oracle: Pubkey,
    usdc_bank: Pubkey,
    usdc_oracle: Pubkey,
) -> Vec<AccountMeta> {
    // Layout matches perp_place_order_from_account_infos:
    //   [0..8]  = group, account, owner, perp_market, bids, asks, event_queue, oracle
    //   [8..]   = health accounts (scanned by ScanningAccountRetriever):
    //             - bank + oracle for each active token position
    //             - perp_market + oracle for each active perp position
    // The place order will auto-activate the perp position, so health scan
    // must find perp_market + perp_oracle too.
    vec![
        AccountMeta::new(group, false),
        AccountMeta::new(mango_account, false),
        AccountMeta::new_readonly(owner, false),
        AccountMeta::new(perp_market, false),
        AccountMeta::new(bids, false),
        AccountMeta::new(asks, false),
        AccountMeta::new(event_queue, false),
        AccountMeta::new_readonly(perp_oracle, false),
        // health accounts:
        AccountMeta::new(usdc_bank, false),
        AccountMeta::new_readonly(usdc_oracle, false),
        AccountMeta::new_readonly(perp_market, false),
        AccountMeta::new_readonly(perp_oracle, false),
    ]
}

/// Build one intent for a given user at a given sequence, with a given side+price.
pub fn build_intent(
    market: &MarketState,
    owners: &[Keypair],
    owner_index: usize,
    mango_accounts: &[Pubkey],
    sequence: u64,
    side: Side,
    price_lots: i64,
    max_base_lots: i64,
    client_order_id: u64,
) -> Intent {
    let program: Pubkey = market.program_id.parse().unwrap();
    let group: Pubkey = market.group.parse().unwrap();
    let perp_market: Pubkey = market.perp_market.parse().unwrap();
    let bids: Pubkey = market.perp_bids.parse().unwrap();
    let asks: Pubkey = market.perp_asks.parse().unwrap();
    let event_queue: Pubkey = market.perp_event_queue.parse().unwrap();
    let perp_oracle: Pubkey = market.perp_oracle.parse().unwrap();
    let usdc_bank: Pubkey = market.usdc_bank.parse().unwrap();
    let usdc_oracle: Pubkey = market.usdc_oracle.parse().unwrap();

    let _ = program;

    let owner = &owners[owner_index];
    let mango_account = mango_accounts[owner_index];
    let payload = encode_perp_place_order_v2_payload(side, price_lots, max_base_lots, client_order_id);
    let payload_hash = hashv(&[&payload]).to_bytes();
    let dispatch_accounts = build_dispatch_accounts(
        group,
        mango_account,
        owner.pubkey(),
        perp_market,
        bids,
        asks,
        event_queue,
        perp_oracle,
        usdc_bank,
        usdc_oracle,
    );
    let accounts_hash = hash_account_metas_runtime(&dispatch_accounts);
    let commit_hash = canonical_commit_hash(
        group,
        market.perp_market_index,
        sequence,
        0,
        &payload_hash,
        &accounts_hash,
        0,
        0,
    );
    let user_msg = canonical_user_intent_v2(
        group,
        mango_account,
        owner.pubkey(),
        market.perp_market_index,
        &payload_hash,
    );
    let user_sig: [u8; 64] = owner
        .sign_message(&user_msg)
        .as_ref()
        .try_into()
        .unwrap();
    Intent {
        sequence,
        owner_index,
        payload,
        payload_hash,
        dispatch_accounts,
        accounts_hash,
        commit_hash,
        user_sig,
        user_msg,
    }
}

pub struct RelayArgs {
    pub rpc: String,
    pub admin_keypair: Keypair,
    pub market_state: MarketState,
    pub users: UsersState,
    pub target_tps_per_user: f64,
    pub run_seconds: u64,
    pub reveal_spacing_ms: u64,
    pub commit_batch_size: usize,
}

pub async fn run_relay(args: RelayArgs) -> Result<()> {
    println!("[relay] init rpc={}", args.rpc);
    let rpc = Arc::new(RpcClient::new_with_commitment(
        args.rpc.clone(),
        CommitmentConfig::confirmed(),
    ));
    let admin = Arc::new(args.admin_keypair);
    let program: Pubkey = args.market_state.program_id.parse()?;
    let group: Pubkey = args.market_state.group.parse()?;
    let authority_state: Pubkey = args.market_state.authority_state.parse()?;
    let queue_root: Pubkey = args.market_state.queue_root.parse()?;
    let queue_page0: Pubkey = args.market_state.queue_page0.parse()?;
    let market_index = args.market_state.perp_market_index;

    // Load user keypairs + mango accounts.
    let mut owners: Vec<Keypair> = Vec::with_capacity(args.users.users.len());
    let mut mango_accounts: Vec<Pubkey> = Vec::with_capacity(args.users.users.len());
    for u in &args.users.users {
        owners.push(Keypair::from_bytes(&u.owner_secret)?);
        mango_accounts.push(u.mango_account.parse()?);
    }
    let owners = Arc::new(owners);
    let mango_accounts = Arc::new(mango_accounts);
    let market_state = Arc::new(args.market_state.clone());

    // Query starting sequence.
    println!("[relay] fetch queue_root={}", queue_root);
    let root_acct = rpc.get_account(&queue_root).await?;
    let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])?;
    let next_seq: u64 = root.next_enqueue_sequence();
    println!("[relay] starting at sequence={}", next_seq);

    // Skip oracle-based price lookup — the stub oracle + lot sizes produce
    // degenerate values. Use a fixed bid/ask band that's inside the
    // orderbook-node validity window.
    let center_price_lots: i64 = 1_000_000;
    println!("[relay] center price_lots = {} (fixed)", center_price_lots);

    // Intent generator: round-robin over users, generate at target TPS.
    let n_users = owners.len();
    let total_tps = args.target_tps_per_user * n_users as f64;
    let inter_intent_us = (1_000_000.0 / total_tps) as u64;
    println!(
        "n_users={} target_tps_per_user={} total_tps={} inter_intent_us={}",
        n_users, args.target_tps_per_user, total_tps, inter_intent_us
    );

    let (intent_tx, mut intent_rx) = mpsc::channel::<Intent>(8192);
    let (batch_tx, batch_rx) = mpsc::channel::<Vec<Intent>>(64);

    // Load generator
    let owners_g = owners.clone();
    let mangos_g = mango_accounts.clone();
    let market_g = market_state.clone();
    let run_seconds = args.run_seconds;
    let start_seq = next_seq;
    let load_gen = tokio::spawn(async move {
        let t0 = Instant::now();
        let mut count: u64 = 0;
        while t0.elapsed() < Duration::from_secs(run_seconds) {
            let seq = start_seq + count;
            // Deterministic by seq, so reveal's reconstruction matches.
            let rr = (seq % n_users as u64) as usize;
            let side = if seq % 2 == 0 { Side::Bid } else { Side::Ask };
            let price_lots = if seq % 2 == 0 {
                center_price_lots - 100
            } else {
                center_price_lots + 100
            };
            let intent = build_intent(
                &market_g,
                &owners_g,
                rr,
                &mangos_g,
                seq,
                side,
                price_lots,
                1,
                100_000 + seq,
            );
            if intent_tx.send(intent).await.is_err() {
                break;
            }
            count += 1;
            tokio::time::sleep(Duration::from_micros(inter_intent_us)).await;
        }
        drop(intent_tx);
        println!("load gen done: emitted {} intents in {:.2}s", count, t0.elapsed().as_secs_f64());
    });

    // Commit batcher — pull from intent_rx, flush at size/time bounds.
    let batch_size = args.commit_batch_size;
    let batcher = tokio::spawn(async move {
        let mut buf: Vec<Intent> = Vec::with_capacity(batch_size);
        let mut window_start: Option<Instant> = None;
        loop {
            let flush_at = window_start.map(|t| t + Duration::from_millis(250));
            let rv = if let Some(d) = flush_at {
                let now = Instant::now();
                if now >= d {
                    if !buf.is_empty() {
                        let _ = batch_tx.send(std::mem::take(&mut buf)).await;
                        window_start = None;
                    }
                    continue;
                }
                tokio::time::timeout(d - now, intent_rx.recv()).await
            } else {
                Ok(intent_rx.recv().await)
            };
            match rv {
                Ok(Some(it)) => {
                    if buf.is_empty() {
                        window_start = Some(Instant::now());
                    }
                    buf.push(it);
                    if buf.len() >= batch_size {
                        let _ = batch_tx.send(std::mem::take(&mut buf)).await;
                        window_start = None;
                    }
                }
                Ok(None) => {
                    if !buf.is_empty() {
                        let _ = batch_tx.send(std::mem::take(&mut buf)).await;
                    }
                    break;
                }
                Err(_) => {
                    if !buf.is_empty() {
                        let _ = batch_tx.send(std::mem::take(&mut buf)).await;
                        window_start = None;
                    }
                }
            }
        }
        drop(batch_tx);
    });

    // Commit submitter — pull batches, sign + send with send_transaction (skip preflight).
    let admin_c = admin.clone();
    let rpc_c = rpc.clone();
    let group_c = group;
    let committed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let committed_c = committed.clone();
    let commit_tx_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let commit_tx_count_c = commit_tx_count.clone();
    let commit_submitter = tokio::spawn(async move {
        let mut batch_rx = batch_rx;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            max_retries: Some(0),
            ..Default::default()
        };
        while let Some(batch) = batch_rx.recv().await {
            if batch.is_empty() { continue; }
            let first_seq = batch[0].sequence;
            let entries: Vec<CommitEntryV4> = batch
                .iter()
                .map(|it| CommitEntryV4 {
                    commit_hash: it.commit_hash,
                    min_execute_slot: 0,
                    expires_at_slot: 0,
                })
                .collect();
            let msg = canonical_commit_batch_message(group_c, market_index, 0, first_seq, &entries);
            let sig_bytes: [u8; 64] = admin_c
                .sign_message(&msg)
                .as_ref()
                .try_into()
                .unwrap();
            let preix = build_presigned_ed25519_instruction(
                admin_c.pubkey().to_bytes(),
                &msg,
                sig_bytes,
            );
            let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
            let commit_ix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::ExecutionQueueV4CommitMarket {
                    group: group_c,
                    authority_state,
                    queue_root,
                    queue_page: queue_page0,
                    instructions: INSTRUCTIONS_SYSVAR_ID,
                }
                .to_account_metas(None),
                data: mango_v4::instruction::ExecutionQueueV4CommitMarket {
                    market_index,
                    first_sequence: first_seq,
                    entries,
                }
                .data(),
            };
            let bh = match rpc_c.get_latest_blockhash().await {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("commit get bh err: {}", e);
                    continue;
                }
            };
            let tx = Transaction::new_signed_with_payer(
                &[cu, preix, commit_ix],
                Some(&admin_c.pubkey()),
                &[admin_c.as_ref()],
                bh,
            );
            match rpc_c.send_transaction_with_config(&tx, send_cfg).await {
                Ok(_sig) => {
                    committed_c.fetch_add(batch.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    commit_tx_count_c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!("commit send err at seq={}: {}", first_seq, e);
                }
            }
        }
    });

    // Reveal worker — poll head, build reveal txs for fixed sequence ranges,
    // pipeline with spacing.
    let admin_r = admin.clone();
    let rpc_r = rpc.clone();
    let reveal_spacing_ms = args.reveal_spacing_ms;
    let owners_r = owners.clone();
    let mangos_r = mango_accounts.clone();
    let market_r = market_state.clone();
    let revealed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let revealed_r = revealed.clone();
    let reveal_tx_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let reveal_tx_count_r = reveal_tx_count.clone();
    let reveal_worker = tokio::spawn(async move {
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            max_retries: Some(0),
            ..Default::default()
        };
        let reveals_per_tx = 2usize; // real-dispatch + user sig, ~1180 B/tx (2 reveals fit)
        let mut idle_iters = 0;
        let mut last_fired: u64 = 0;
        const IN_FLIGHT_LIMIT: u64 = 8; // max reveals ahead of head
        loop {
            let root_acct = match rpc_r.get_account(&queue_root).await {
                Ok(a) => a,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    idle_iters += 1;
                    if idle_iters > 80 { break; }
                    continue;
                }
            };
            let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..]).unwrap();
            let head = root.next_sequence_to_execute;
            let max_seen = root.max_seen_sequence;
            if root.live_count == 0 {
                // Don't exit on idle — commits may still be landing. Just back off.
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            }
            idle_iters = 0;
            if head > last_fired {
                last_fired = head;
            }
            // Seqs we can fire: [last_fired .. window_end), capped to in-flight.
            let window_end = max_seen
                .saturating_add(1)
                .min(head + IN_FLIGHT_LIMIT);
            let seqs_to_fire = window_end.saturating_sub(last_fired) as usize;
            if seqs_to_fire == 0 {
                // Either stalled at head (tx in flight — wait) or all fired.
                // Reset last_fired back to head so we re-try dropped reveals.
                if last_fired > head + 2 {
                    last_fired = head;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            // Number of txs (each tx carries reveals_per_tx seqs).
            let fire_count = (seqs_to_fire + reveals_per_tx - 1) / reveals_per_tx;
            let bh = rpc_r.get_latest_blockhash().await.unwrap();
            for i in 0..fire_count {
                let start = last_fired + (i * reveals_per_tx) as u64;
                if start >= window_end { break; }
                let n = reveals_per_tx.min((max_seen - start + 1) as usize);
                // Rebuild the intents for this range deterministically.
                // We need to know: which user emitted seq X and what order?
                // From the load gen we used round-robin starting at start_seq.
                // For simplicity here, owner_index = (seq - start_seq) mod n_users
                // — but the load gen advanced sequences globally. We need a
                // smarter lookup. Compromise: track emitted intents in a map.
                // For now we rebuild from the same deterministic generator state.
                let mut reveals: Vec<RevealArgsV4> = Vec::with_capacity(n);
                let mut accts: Vec<AccountMeta> = Vec::with_capacity(n * 10);
                let mut user_preixs: Vec<Instruction> = Vec::new();
                for j in 0..n as u64 {
                    let seq = start + j;
                    let rr = (seq % owners_r.len() as u64) as usize;
                    let side = if seq % 2 == 0 { Side::Bid } else { Side::Ask };
                    let price_lots = if seq % 2 == 0 {
                        center_price_lots - 100
                    } else {
                        center_price_lots + 100
                    };
                    let it = build_intent(
                        &market_r,
                        &owners_r,
                        rr,
                        &mangos_r,
                        seq,
                        side,
                        price_lots,
                        1,
                        100_000 + seq,
                    );
                    reveals.push(RevealArgsV4 {
                        payload: it.payload.clone(),
                        kind: 0,
                        dispatch_accounts_count: it.dispatch_accounts.len() as u8,
                    });
                    accts.extend(it.dispatch_accounts.clone());
                    user_preixs.push(build_presigned_ed25519_instruction(
                        owners_r[it.owner_index].pubkey().to_bytes(),
                        &it.user_msg,
                        it.user_sig,
                    ));
                }
                let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
                let mut reveal_accounts = mango_v4::accounts::ExecutionQueueV4RevealExecuteMarket {
                    group: group_c,
                    authority_state,
                    queue_root,
                    queue_page: queue_page0,
                    instructions: INSTRUCTIONS_SYSVAR_ID,
                }
                .to_account_metas(None);
                reveal_accounts.extend(accts);
                reveal_accounts.push(AccountMeta::new_readonly(program, false));
                let reveal_ix = Instruction {
                    program_id: program,
                    accounts: reveal_accounts,
                    data: mango_v4::instruction::ExecutionQueueV4RevealExecuteMarket { reveals }
                        .data(),
                };
                let mut ixs = vec![cu];
                ixs.extend(user_preixs);
                ixs.push(reveal_ix);
                let tx = Transaction::new_signed_with_payer(
                    &ixs,
                    Some(&admin_r.pubkey()),
                    &[admin_r.as_ref()],
                    bh,
                );
                let tx_bytes = bincode::serialize(&tx).ok().map(|v| v.len()).unwrap_or(0);
                match rpc_r.send_transaction_with_config(&tx, send_cfg).await {
                    Ok(_) => {
                        reveal_tx_count_r.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("reveal send err seq={} n={} bytes={} err={}", start, n, tx_bytes, e);
                    }
                }
                tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
            }
            last_fired += seqs_to_fire as u64;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        // final revealed count estimate
        if let Ok(a) = rpc_r.get_account(&queue_root).await {
            if let Ok(r) = PerpMarketCommitRootV4::try_deserialize(&mut &a.data[..]) {
                revealed_r.store(r.next_sequence_to_execute, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });

    // Monitor — print stats every second.
    let revealed_mon = revealed.clone();
    let committed_mon = committed.clone();
    let commit_txs_mon = commit_tx_count.clone();
    let reveal_txs_mon = reveal_tx_count.clone();
    let rpc_mon = rpc.clone();
    let monitor = tokio::spawn(async move {
        let t0 = Instant::now();
        let mut last_committed = 0u64;
        let mut last_revealed_head = 0u64;
        while t0.elapsed() < Duration::from_secs(run_seconds + 30) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let committed_now = committed_mon.load(std::sync::atomic::Ordering::Relaxed);
            let commit_txs = commit_txs_mon.load(std::sync::atomic::Ordering::Relaxed);
            let reveal_txs = reveal_txs_mon.load(std::sync::atomic::Ordering::Relaxed);
            let revealed_head = match rpc_mon.get_account(&queue_root).await {
                Ok(a) => match PerpMarketCommitRootV4::try_deserialize(&mut &a.data[..]) {
                    Ok(r) => r.next_sequence_to_execute,
                    Err(e) => {
                        eprintln!("mon: deser err: {}", e);
                        0
                    }
                },
                Err(e) => {
                    eprintln!("mon: get_account err: {}", e);
                    0
                }
            };
            revealed_mon.store(revealed_head, std::sync::atomic::Ordering::Relaxed);
            let t = t0.elapsed().as_secs_f64();
            println!(
                "t={:.1}s  committed={} (+{}/s)  commit_txs={}  reveal_txs={}  revealed_head={} (+{}/s)  in_queue={}",
                t,
                committed_now,
                committed_now.saturating_sub(last_committed),
                commit_txs,
                reveal_txs,
                revealed_head,
                revealed_head.saturating_sub(last_revealed_head),
                committed_now.saturating_sub(revealed_head.saturating_sub(start_seq)),
            );
            last_committed = committed_now;
            last_revealed_head = revealed_head;
        }
    });

    let _ = load_gen.await;
    let _ = batcher.await;
    let _ = commit_submitter.await;
    let _ = reveal_worker.await;
    let _ = monitor.await;
    Ok(())
}

pub async fn debug_reveal_one(args: RelayArgs) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = args.admin_keypair;
    let program: Pubkey = args.market_state.program_id.parse()?;
    let group: Pubkey = args.market_state.group.parse()?;
    let authority_state: Pubkey = args.market_state.authority_state.parse()?;
    let queue_root: Pubkey = args.market_state.queue_root.parse()?;
    let queue_page0: Pubkey = args.market_state.queue_page0.parse()?;
    let market_index = args.market_state.perp_market_index;
    let mut owners: Vec<Keypair> = args.users.users.iter()
        .map(|u| Keypair::from_bytes(&u.owner_secret).unwrap()).collect();
    let mangos: Vec<Pubkey> = args.users.users.iter()
        .map(|u| u.mango_account.parse().unwrap()).collect();

    let root_acct = rpc.get_account(&queue_root).await?;
    let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])?;
    let head = root.next_sequence_to_execute;
    println!("head={} max_seen={} live_count={}", head, root.max_seen_sequence, root.live_count);

    // Build reveal for head seq
    let seq = head;
    let rr = (seq % owners.len() as u64) as usize;
    let center: i64 = 1_000_000;
    let side = if seq % 2 == 0 { Side::Bid } else { Side::Ask };
    let price_lots = if seq % 2 == 0 { center - 100 } else { center + 100 };
    let it = build_intent(&args.market_state, &owners, rr, &mangos, seq, side, price_lots, 1, 100_000 + seq);

    let user_preix = build_presigned_ed25519_instruction(
        owners[rr].pubkey().to_bytes(),
        &it.user_msg,
        it.user_sig,
    );
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let mut reveal_accounts = mango_v4::accounts::ExecutionQueueV4RevealExecuteMarket {
        group,
        authority_state,
        queue_root,
        queue_page: queue_page0,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    reveal_accounts.extend(it.dispatch_accounts.clone());
    reveal_accounts.push(AccountMeta::new_readonly(program, false));
    let reveal_ix = Instruction {
        program_id: program,
        accounts: reveal_accounts,
        data: mango_v4::instruction::ExecutionQueueV4RevealExecuteMarket {
            reveals: vec![RevealArgsV4 {
                payload: it.payload.clone(),
                kind: 0,
                dispatch_accounts_count: it.dispatch_accounts.len() as u8,
            }],
        }
        .data(),
    };
    let bh = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[cu, user_preix, reveal_ix],
        Some(&admin.pubkey()),
        &[&admin],
        bh,
    );
    let sim = rpc.simulate_transaction(&tx).await?;
    println!("sim err: {:?}", sim.value.err);
    if let Some(logs) = sim.value.logs {
        for l in logs { println!("  {}", l); }
    }
    println!("tx_bytes={}", bincode::serialize(&tx).unwrap().len());
    Ok(())
}

pub async fn close_market_recover(args: BootstrapMarketArgs) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = &args.admin_keypair;
    let program = args.program;
    let market = MarketState::load(&args.output)?;
    let group: Pubkey = market.group.parse()?;
    let perp_market: Pubkey = market.perp_market.parse()?;
    let bids: Pubkey = market.perp_bids.parse()?;
    let asks: Pubkey = market.perp_asks.parse()?;
    let eq: Pubkey = market.perp_event_queue.parse()?;
    println!("closing perp_market={} bids={} asks={} eq={}", perp_market, bids, asks, eq);
    // Need token_program
    let close_ix = Instruction {
        program_id: program,
        accounts: mango_v4::accounts::PerpCloseMarket {
            group,
            admin: admin.pubkey(),
            perp_market,
            bids,
            asks,
            event_queue: eq,
            sol_destination: admin.pubkey(),
            token_program: SPL_TOKEN,
        }.to_account_metas(None),
        data: mango_v4::instruction::PerpCloseMarket {}.data(),
    };
    send_confirmed(&rpc, vec![close_ix], admin, &[]).await?;
    let bal = rpc.get_balance(&admin.pubkey()).await?;
    println!("closed. new balance = {} SOL", bal as f64 / 1e9);
    Ok(())
}

pub struct AirdropUsdcArgs {
    pub rpc: String,
    pub admin_keypair: Keypair,
    pub market_state: MarketState,
    pub senders_file: PathBuf,
    pub amount_usdc: u64, // human units (e.g. 5000)
}

pub async fn airdrop_usdc(args: AirdropUsdcArgs) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = &args.admin_keypair;
    let usdc_mint: Pubkey = args.market_state.usdc_mint.parse()?;
    // USDC has 6 decimals — base units = human * 10^6
    let amount_base = args.amount_usdc * 1_000_000;

    let senders: Vec<Pubkey> = std::fs::read_to_string(&args.senders_file)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse::<Pubkey>())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    println!("airdropping {} USDC ({} base) to {} owners", args.amount_usdc, amount_base, senders.len());

    let mut landed = 0usize;
    let mut failed = 0usize;
    for (i, owner) in senders.iter().enumerate() {
        let ata = find_ata(*owner, usdc_mint);
        // CreateIdempotent is safe even if ATA already exists
        let create_ix = create_ata_idempotent_ix(&admin.pubkey(), owner, &usdc_mint);
        let mint_ix = mint_to_ix(&usdc_mint, &ata, &admin.pubkey(), amount_base);
        match send_confirmed(&rpc, vec![create_ix, mint_ix], admin, &[]).await {
            Ok(sig) => {
                landed += 1;
                println!("[{:2}/{}] ok {} ata={} sig={}", i + 1, senders.len(), owner, ata, sig);
            }
            Err(e) => {
                failed += 1;
                eprintln!("[{:2}/{}] FAIL {}: {}", i + 1, senders.len(), owner, e);
            }
        }
    }
    println!("airdrop done: ok={} fail={}", landed, failed);
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExtraMarket {
    pub perp_market_index: u16,
    pub base_mint: String,
    pub perp_oracle: String,
    pub perp_market: String,
    pub perp_bids: String,
    pub perp_asks: String,
    pub perp_event_queue: String,
    pub queue_root: String,
    pub queue_page0: String,
    pub base_lot_size: i64,
    pub quote_lot_size: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExtraMarketsState {
    pub markets: Vec<ExtraMarket>,
}

impl ExtraMarketsState {
    pub fn load_or_empty(p: &PathBuf) -> Self {
        match std::fs::read_to_string(p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| Self { markets: vec![] }),
            Err(_) => Self { markets: vec![] },
        }
    }
    pub fn save(&self, p: &PathBuf) -> Result<()> {
        std::fs::write(p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub struct BootstrapExtraMarketsArgs {
    pub rpc: String,
    pub admin_keypair: Keypair,
    pub market_state: MarketState,
    pub extras_output: PathBuf,
    pub first_market_index: u16,
    pub count: u16,
    pub base_price: f64,
    /// If Some, use this oracle instead of creating a stub oracle + base mint.
    pub perp_oracle_override: Option<Pubkey>,
    pub base_decimals: u8,
    pub base_lot_size: i64,
    pub quote_lot_size: i64,
    /// Number of queue pages to create+resize+init (slots 0..num_pages-1).
    pub num_pages: u16,
}

pub async fn bootstrap_extra_markets(args: BootstrapExtraMarketsArgs) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    let admin = &args.admin_keypair;
    let program: Pubkey = args.market_state.program_id.parse()?;
    let group: Pubkey = args.market_state.group.parse()?;
    let authority_state: Pubkey = args.market_state.authority_state.parse()?;

    let mut state = ExtraMarketsState::load_or_empty(&args.extras_output);

    for i in 0..args.count {
        let pmi = args.first_market_index + i;
        // Skip if already present
        if state.markets.iter().any(|m| m.perp_market_index == pmi) {
            println!("market_index={} already bootstrapped, skipping", pmi);
            continue;
        }
        let bal = rpc.get_balance(&admin.pubkey()).await?;
        println!("=== bootstrap market_index={} (admin balance={:.3} SOL) ===", pmi, bal as f64 / 1e9);

        // Oracle — either reuse a provided (Pyth) oracle or create a stub.
        let (perp_oracle, base_mint_str) = if let Some(override_oracle) = args.perp_oracle_override
        {
            // Pyth / sponsored feed — no new mint or stub oracle required.
            (override_oracle, String::new())
        } else {
            // Unique base mint for decimals+oracle
            let base_mint_kp = Keypair::new();
            {
                let rent = rpc.get_minimum_balance_for_rent_exemption(82).await?;
                let create = system_instruction::create_account(
                    &admin.pubkey(),
                    &base_mint_kp.pubkey(),
                    rent,
                    82,
                    &SPL_TOKEN,
                );
                let init = init_mint_ix(&base_mint_kp.pubkey(), &admin.pubkey(), 9);
                send_confirmed(&rpc, vec![create, init], admin, &[&base_mint_kp]).await?;
            }

            let perp_oracle_kp = Keypair::new();
            {
                let ix = Instruction {
                    program_id: program,
                    accounts: mango_v4::accounts::StubOracleCreate {
                        group,
                        oracle: perp_oracle_kp.pubkey(),
                        admin: admin.pubkey(),
                        mint: base_mint_kp.pubkey(),
                        payer: admin.pubkey(),
                        system_program: solana_sdk::system_program::id(),
                    }
                    .to_account_metas(None),
                    data: mango_v4::instruction::StubOracleCreate {
                        price: fixed::types::I80F48::from_num(args.base_price),
                    }
                    .data(),
                };
                send_confirmed(&rpc, vec![ix], admin, &[&perp_oracle_kp]).await?;
            }
            (perp_oracle_kp.pubkey(), base_mint_kp.pubkey().to_string())
        };

        // Pre-allocate bids/asks/event_queue
        let bids_kp = Keypair::new();
        let asks_kp = Keypair::new();
        let eq_kp = Keypair::new();
        let book_space = 8 + std::mem::size_of::<mango_v4::state::BookSide>();
        let eq_space = 8 + std::mem::size_of::<mango_v4::state::EventQueue>();
        for (kp, space) in [
            (&bids_kp, book_space),
            (&asks_kp, book_space),
            (&eq_kp, eq_space),
        ] {
            let rent = rpc.get_minimum_balance_for_rent_exemption(space).await?;
            let create = system_instruction::create_account(
                &admin.pubkey(),
                &kp.pubkey(),
                rent,
                space as u64,
                &program,
            );
            send_confirmed(&rpc, vec![create], admin, &[kp]).await?;
        }

        let perp_market = find_perp_market_pda(program, group, pmi);
        {
            let ix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::PerpCreateMarket {
                    group,
                    admin: admin.pubkey(),
                    oracle: perp_oracle,
                    perp_market,
                    bids: bids_kp.pubkey(),
                    asks: asks_kp.pubkey(),
                    event_queue: eq_kp.pubkey(),
                    payer: admin.pubkey(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::PerpCreateMarket {
                    name: format!("M{}-PERP", pmi),
                    oracle_config: OracleConfigParams {
                        conf_filter: 0.1,
                        max_staleness_slots: Some(600),
                    },
                    settle_token_index: 0,
                    perp_market_index: pmi,
                    quote_lot_size: args.quote_lot_size,
                    base_lot_size: args.base_lot_size,
                    maint_base_asset_weight: 0.95,
                    init_base_asset_weight: 0.9,
                    maint_base_liab_weight: 1.05,
                    init_base_liab_weight: 1.1,
                    maint_overall_asset_weight: 1.0,
                    init_overall_asset_weight: 1.0,
                    base_liquidation_fee: 0.02,
                    maker_fee: -0.0001,
                    taker_fee: 0.0002,
                    max_funding: 0.05,
                    min_funding: 0.05,
                    impact_quantity: 100,
                    base_decimals: args.base_decimals,
                    group_insurance_fund: true,
                    fee_penalty: 0.0,
                    settle_fee_flat: 0.0,
                    settle_fee_amount_threshold: 0.0,
                    settle_fee_fraction_low_health: 0.0,
                    settle_pnl_limit_factor: -1.0,
                    settle_pnl_limit_window_size_ts: 24 * 60 * 60,
                    positive_pnl_liquidation_fee: 0.0,
                    platform_liquidation_fee: 0.0,
                }
                .data(),
            };
            send_confirmed(&rpc, vec![ix], admin, &[]).await?;
        }

        // v4 queue root for this market
        let queue_root = find_commit_queue_root_pda(program, group, pmi);
        {
            let ix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::ExecutionQueueV4InitMarketRoot {
                    group,
                    authority_state,
                    queue_root,
                    payer: admin.pubkey(),
                    admin: admin.pubkey(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::ExecutionQueueV4InitMarketRoot {
                    market_index: pmi,
                    shard_id: 0,
                    params: mango_v4::instructions::ExecutionQueueV4MarketRootCreateParams {
                        page_size: 256,
                        num_pages: args.num_pages,
                        soft_limit: 0,
                        gap_wait_slots: 4,
                    },
                }
                .data(),
            };
            send_confirmed(&rpc, vec![ix], admin, &[]).await?;
        }

        // All queue pages: create + resize + init for each slot 0..num_pages-1.
        let queue_page0 = find_commit_queue_page_pda(program, queue_root, 0);
        const TARGET: usize = 8 + std::mem::size_of::<mango_v4::state::CommitPageV4>();
        for page_slot in 0..args.num_pages {
            let page_pda = find_commit_queue_page_pda(program, queue_root, page_slot);
            println!("  page_slot={} pda={}", page_slot, page_pda);
            let create_ix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::ExecutionQueueV4CreateMarketPage {
                    group,
                    authority_state,
                    queue_root,
                    queue_page: page_pda,
                    payer: admin.pubkey(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None),
                data: mango_v4::instruction::ExecutionQueueV4CreateMarketPage { page_slot }.data(),
            };
            send_confirmed(&rpc, vec![create_ix], admin, &[]).await?;
            loop {
                let acct = rpc.get_account(&page_pda).await?;
                if acct.data.len() >= TARGET {
                    break;
                }
                let resize_ix = Instruction {
                    program_id: program,
                    accounts: mango_v4::accounts::ExecutionQueueV4ResizeMarketPage {
                        group,
                        authority_state,
                        queue_root,
                        queue_page: page_pda,
                        payer: admin.pubkey(),
                        system_program: solana_sdk::system_program::id(),
                    }
                    .to_account_metas(None),
                    data: mango_v4::instruction::ExecutionQueueV4ResizeMarketPage { page_slot }
                        .data(),
                };
                send_confirmed(&rpc, vec![resize_ix], admin, &[]).await?;
            }
            let init_ix = Instruction {
                program_id: program,
                accounts: mango_v4::accounts::ExecutionQueueV4InitMarketPage {
                    group,
                    authority_state,
                    queue_root,
                    queue_page: page_pda,
                }
                .to_account_metas(None),
                data: mango_v4::instruction::ExecutionQueueV4InitMarketPage {
                    page_slot,
                    assigned_abs_page_no: page_slot as u64,
                }
                .data(),
            };
            send_confirmed(&rpc, vec![init_ix], admin, &[]).await?;
        }

        state.markets.push(ExtraMarket {
            perp_market_index: pmi,
            base_mint: base_mint_str,
            perp_oracle: perp_oracle.to_string(),
            perp_market: perp_market.to_string(),
            perp_bids: bids_kp.pubkey().to_string(),
            perp_asks: asks_kp.pubkey().to_string(),
            perp_event_queue: eq_kp.pubkey().to_string(),
            queue_root: queue_root.to_string(),
            queue_page0: queue_page0.to_string(),
            base_lot_size: args.base_lot_size,
            quote_lot_size: args.quote_lot_size,
        });
        state.save(&args.extras_output)?;
        println!("market_index={} ready. state={}", pmi, args.extras_output.display());
    }
    Ok(())
}
