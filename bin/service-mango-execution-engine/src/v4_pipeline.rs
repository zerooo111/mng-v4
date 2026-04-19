//! Inline v4 commit-reveal pipeline used when `V4_ROUTE_ALL=1`.
//!
//! The submit_intent handler calls `submit_as_v4_commit` instead of the
//! legacy v2 enqueue path. That function:
//!
//!   1. Computes payload_hash, accounts_hash, commit_hash (matching
//!      on-chain `canonical_commit_message` in execution_queue_v4.rs).
//!   2. Builds a 1-entry `commit_market_batch` tx (relayer-signed) and
//!      submits it.
//!   3. Stores the intent material keyed by sequence in a shared registry
//!      so the reveal worker can later present the payload + accounts +
//!      user_sig at reveal time.
//!
//! A single tokio task (`spawn_reveal_worker`) polls queue_root, reads the
//! pending head, looks up the intent material, and fires pipelined
//! `reveal_execute_market` txs with configurable spacing.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anyhow::{anyhow, Result};
use mango_v4::instructions::{CommitEntryV4, RevealArgsV4};
use mango_v4::state::PerpMarketCommitRootV4;
use solana_client::{
    nonblocking::rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig,
};
use solana_program::hash::hashv;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    sysvar::instructions::ID as INSTRUCTIONS_SYSVAR_ID,
    transaction::Transaction,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use parking_lot::Mutex;
use tracing::{debug, error, info, warn};

/// Marker discriminator literal for the seed used by both `create_market_page`
/// and `init_market_page`.
const COMMIT_QUEUE_PAGE_SEED: &[u8] = b"commit-queue-page";

/// Anchor discriminator helper — `global:<name>` SHA-256 truncated to 8 bytes.
fn anchor_discriminator(name: &str) -> [u8; 8] {
    let h = solana_program::hash::hash(format!("global:{name}").as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&h.to_bytes()[..8]);
    out
}

fn hex_short(h: &[u8; 32]) -> String {
    let mut s = String::with_capacity(16);
    for b in &h[..8] {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

pub fn derive_queue_page_pda(
    program_id: &Pubkey,
    queue_root: &Pubkey,
    page_slot: u16,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            COMMIT_QUEUE_PAGE_SEED,
            queue_root.as_ref(),
            &page_slot.to_le_bytes(),
        ],
        program_id,
    )
}

#[derive(Clone, Debug)]
pub struct V4MarketState {
    pub program_id: Pubkey,
    pub group: Pubkey,
    pub authority_state: Pubkey,
    pub queue_root: Pubkey,
    pub queue_page0: Pubkey,
    pub perp_market: Pubkey,
    pub perp_bids: Pubkey,
    pub perp_asks: Pubkey,
    pub perp_event_queue: Pubkey,
    pub perp_oracle: Pubkey,
    pub usdc_bank: Pubkey,
    pub usdc_oracle: Pubkey,
    pub market_index: u16,
    pub page_size: u16,
    pub num_pages: u16,
}

impl V4MarketState {
    /// Derive the queue_page PDA responsible for `sequence` given the queue's
    /// page_size / num_pages geometry.
    pub fn queue_page_for_sequence(&self, sequence: u64) -> Pubkey {
        let page_slot = self.page_slot_for_sequence(sequence);
        if page_slot == 0 {
            return self.queue_page0;
        }
        derive_queue_page_pda(&self.program_id, &self.queue_root, page_slot).0
    }

    pub fn page_slot_for_sequence(&self, sequence: u64) -> u16 {
        let page_size = self.page_size.max(1) as u64;
        let num_pages = self.num_pages.max(1) as u64;
        ((sequence / page_size) % num_pages) as u16
    }
}

#[derive(Clone)]
pub struct V4RevealEntry {
    pub sequence: u64,
    pub payload: Vec<u8>,
    pub payload_hash: [u8; 32],
    pub dispatch_accounts: Vec<AccountMeta>,
    pub user_owner: Pubkey,
    pub mango_account: Pubkey,
    pub user_sig: Option<[u8; 64]>,
    pub user_intent_hash: Option<[u8; 32]>,
}

/// Shared map of sequence → reveal material. Reveal worker drains this
/// as head advances. Populated by `submit_as_v4_commit`.
pub type V4RevealStore = Arc<Mutex<BTreeMap<u64, V4RevealEntry>>>;

/// Sequence counter for v4 commits. Monotonic per queue.
pub type V4NextSequence = Arc<std::sync::atomic::AtomicU64>;

// ---------------------------------------------------------------------------
// hashing (must match on-chain exactly)
// ---------------------------------------------------------------------------

/// Low-level hash used by both the reveal-aware and plain helpers below.
/// Emits `(pubkey || is_signer || is_writable)` for each meta in order, with
/// flags drawn from the supplied `flags` map (if the pubkey is present there)
/// or the meta's own flags (if not).
fn hash_metas_with_flag_map(
    metas: &[AccountMeta],
    flags: &BTreeMap<Pubkey, (bool, bool)>,
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(metas.len() * 34);
    for a in metas {
        let (s, w) = flags
            .get(&a.pubkey)
            .copied()
            .unwrap_or((a.is_signer, a.is_writable));
        buf.extend_from_slice(a.pubkey.as_ref());
        buf.push(u8::from(s));
        buf.push(u8::from(w));
    }
    hashv(&[&buf]).to_bytes()
}

/// Flag-OR `metas` with itself only — historic helper kept for callers that
/// don't know about reveal-fixed accounts. NOT equivalent to what the on-chain
/// `hash_accounts(account_metas_from_infos(dispatch))` computes for the v4
/// reveal path — use `hash_dispatch_accounts_for_reveal` for that.
pub fn hash_account_metas_runtime(metas: &[AccountMeta]) -> [u8; 32] {
    let mut flags: BTreeMap<Pubkey, (bool, bool)> = BTreeMap::new();
    for a in metas {
        let e = flags.entry(a.pubkey).or_insert((false, false));
        e.0 |= a.is_signer;
        e.1 |= a.is_writable;
    }
    hash_metas_with_flag_map(metas, &flags)
}

/// Hash the dispatch_accounts slice EXACTLY as on-chain `hash_accounts` would
/// see them at reveal time — i.e. with flags merged against every other
/// position in the reveal tx (the fixed `ExecutionQueueV4RevealExecuteMarket`
/// accounts plus the trailing program_id). Without this, duplicate-pubkey
/// positions in the dispatch list get the wrong flags and the commit_hash
/// fails to verify with `ExecutionQueueV4CommitRevealMismatch` (error 6114).
pub fn hash_dispatch_accounts_for_reveal(
    mkt: &V4MarketState,
    dispatch_accounts: &[AccountMeta],
) -> [u8; 32] {
    // Fixed accounts the relayer adds to every reveal tx, with on-chain
    // writability matching the `#[derive(Accounts)]` struct for
    // ExecutionQueueV4RevealExecuteMarket.
    let fixed: [AccountMeta; 6] = [
        AccountMeta::new(mkt.group, false),            // group — `#[account(mut)]`
        AccountMeta::new_readonly(mkt.authority_state, false),
        AccountMeta::new(mkt.queue_root, false),       // queue_root — `#[account(mut)]`
        AccountMeta::new(mkt.queue_page0, false),      // queue_page (any page, same flags)
        AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false),
        AccountMeta::new_readonly(mkt.program_id, false), // trailing program_id
    ];
    let mut flags: BTreeMap<Pubkey, (bool, bool)> = BTreeMap::new();
    for a in fixed.iter().chain(dispatch_accounts.iter()) {
        let e = flags.entry(a.pubkey).or_insert((false, false));
        e.0 |= a.is_signer;
        e.1 |= a.is_writable;
    }
    hash_metas_with_flag_map(dispatch_accounts, &flags)
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
        &[0u8], // kind = CtmWrapped
        &[0u8], // target_kind = PerpMarket
        &market_index.to_le_bytes(),
        payload_hash,
    ])
    .to_bytes()
}

// ---------------------------------------------------------------------------
// ed25519 pre-ix (single-sig)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// dispatch-accounts layout for perp_place_order (matches v4-stress)
// ---------------------------------------------------------------------------

pub fn build_dispatch_accounts(
    mkt: &V4MarketState,
    mango_account: Pubkey,
    user_owner: Pubkey,
) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(mkt.group, false),
        AccountMeta::new(mango_account, false),
        AccountMeta::new_readonly(user_owner, false),
        AccountMeta::new(mkt.perp_market, false),
        AccountMeta::new(mkt.perp_bids, false),
        AccountMeta::new(mkt.perp_asks, false),
        AccountMeta::new(mkt.perp_event_queue, false),
        AccountMeta::new_readonly(mkt.perp_oracle, false),
        AccountMeta::new(mkt.usdc_bank, false),
        AccountMeta::new_readonly(mkt.usdc_oracle, false),
        AccountMeta::new_readonly(mkt.perp_market, false),
        AccountMeta::new_readonly(mkt.perp_oracle, false),
    ]
}

// ---------------------------------------------------------------------------
// Build a 1-entry commit_market tx
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn build_commit_market_tx(
    mkt: &V4MarketState,
    ctm_signer: &Keypair,
    payer: &Keypair,
    first_sequence: u64,
    entry: CommitEntryV4,
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    let entries = vec![entry];
    let msg = canonical_commit_batch_message(
        mkt.group,
        mkt.market_index,
        0,
        first_sequence,
        &entries,
    );
    let sig_bytes: [u8; 64] = ctm_signer
        .sign_message(&msg)
        .as_ref()
        .try_into()
        .expect("ed25519 sig is 64 bytes");
    let preix = build_presigned_ed25519_instruction(
        ctm_signer.pubkey().to_bytes(),
        &msg,
        sig_bytes,
    );
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);

    let queue_page = mkt.queue_page_for_sequence(first_sequence);
    let commit_accounts = mango_v4::accounts::ExecutionQueueV4CommitMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue_root: mkt.queue_root,
        queue_page,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    let commit_ix = Instruction {
        program_id: mkt.program_id,
        accounts: commit_accounts,
        data: mango_v4::instruction::ExecutionQueueV4CommitMarket {
            market_index: mkt.market_index,
            first_sequence,
            entries,
        }
        .data(),
    };
    Transaction::new_signed_with_payer(
        &[cu, preix, commit_ix],
        Some(&payer.pubkey()),
        &[payer],
        blockhash,
    )
}

// ---------------------------------------------------------------------------
// Build a reveal_execute_market tx for a single entry
// ---------------------------------------------------------------------------

pub fn build_reveal_execute_tx(
    mkt: &V4MarketState,
    payer: &Keypair,
    reveal_entry: &V4RevealEntry,
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    // Debug: recompute commit-side hashes and log what we're about to reveal.
    let computed_accounts_hash =
        hash_dispatch_accounts_for_reveal(mkt, &reveal_entry.dispatch_accounts);
    let recomputed_commit = canonical_commit_hash(
        mkt.group,
        mkt.market_index,
        reveal_entry.sequence,
        0,
        &reveal_entry.payload_hash,
        &computed_accounts_hash,
        0,
        0,
    );
    debug!(
        target: "v4_reveal_debug",
        seq = reveal_entry.sequence,
        n_dispatch = reveal_entry.dispatch_accounts.len(),
        payload_hash = hex_short(&reveal_entry.payload_hash),
        accounts_hash = hex_short(&computed_accounts_hash),
        commit_hash_min0_exp0 = hex_short(&recomputed_commit),
        "reveal dispatch"
    );
    for (i, m) in reveal_entry.dispatch_accounts.iter().enumerate() {
        debug!(
            target: "v4_reveal_debug",
            seq = reveal_entry.sequence,
            i,
            pubkey = %m.pubkey,
            is_signer = m.is_signer,
            is_writable = m.is_writable,
            "  meta"
        );
    }
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let mut ixs = vec![cu];

    if let (Some(user_sig), Some(msg_hash)) =
        (reveal_entry.user_sig, reveal_entry.user_intent_hash)
    {
        ixs.push(build_presigned_ed25519_instruction(
            reveal_entry.user_owner.to_bytes(),
            &msg_hash,
            user_sig,
        ));
    }

    let queue_page = mkt.queue_page_for_sequence(reveal_entry.sequence);
    let mut accounts = mango_v4::accounts::ExecutionQueueV4RevealExecuteMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue_root: mkt.queue_root,
        queue_page,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    accounts.extend(reveal_entry.dispatch_accounts.clone());
    accounts.push(AccountMeta::new_readonly(mkt.program_id, false));

    let reveal_ix = Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV4RevealExecuteMarket {
            reveals: vec![RevealArgsV4 {
                payload: reveal_entry.payload.clone(),
                kind: 0,
                dispatch_accounts_count: reveal_entry.dispatch_accounts.len() as u8,
            }],
        }
        .data(),
    };
    ixs.push(reveal_ix);

    Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[payer],
        blockhash,
    )
}

// ---------------------------------------------------------------------------
// Reveal worker — head-polling loop that pipelines reveals.
// ---------------------------------------------------------------------------

/// Shared head-stall tracking the autodrop worker reads. Updated by the
/// reveal worker on every successful queue_root poll.
#[derive(Clone, Default)]
pub struct V4HeadStallState {
    inner: Arc<Mutex<V4HeadStallInner>>,
}

#[derive(Default)]
struct V4HeadStallInner {
    last_head: u64,
    last_head_changed_at: Option<Instant>,
    last_max_seen: u64,
    last_live_count: u32,
    reveal_attempts_for_current_head: u64,
}

impl V4HeadStallState {
    pub fn observe(&self, head: u64, max_seen: u64, live_count: u32) {
        let mut g = self.inner.lock();
        if g.last_head_changed_at.is_none() || head != g.last_head {
            g.last_head = head;
            g.last_head_changed_at = Some(Instant::now());
            g.reveal_attempts_for_current_head = 0;
        }
        g.last_max_seen = max_seen;
        g.last_live_count = live_count;
    }

    pub fn note_reveal_attempt(&self) {
        let mut g = self.inner.lock();
        g.reveal_attempts_for_current_head =
            g.reveal_attempts_for_current_head.saturating_add(1);
    }

    pub fn snapshot(&self) -> (u64, Option<Duration>, u32, u64) {
        let g = self.inner.lock();
        (
            g.last_head,
            g.last_head_changed_at.map(|t| t.elapsed()),
            g.last_live_count,
            g.reveal_attempts_for_current_head,
        )
    }
}

pub fn spawn_reveal_worker(
    rpc: Arc<RpcClient>,
    mkt: Arc<V4MarketState>,
    payer: Arc<Keypair>,
    store: V4RevealStore,
    reveal_spacing_ms: u64,
    stall: V4HeadStallState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            max_retries: Some(0),
            ..Default::default()
        };
        let mut last_fired: u64 = 0;
        let mut last_log_head: u64 = u64::MAX;
        let mut last_log_at = Instant::now();
        const IN_FLIGHT: u64 = 6;
        loop {
            let root_acct = match rpc.get_account(&mkt.queue_root).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(target: "v4_reveal", error = %e, "queue_root rpc fetch failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let root = match PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..]) {
                Ok(r) => r,
                Err(e) => {
                    warn!(target: "v4_reveal", error = %e, "queue_root deserialize failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let head = root.next_sequence_to_execute;
            let max_seen = root.max_seen_sequence;
            stall.observe(head, max_seen, root.live_count);

            // Periodic heartbeat so operators can see queue health at a glance.
            if head != last_log_head || last_log_at.elapsed() >= Duration::from_secs(15) {
                info!(
                    target: "v4_reveal",
                    head,
                    max_seen,
                    live_count = root.live_count,
                    last_fired,
                    "queue_root state"
                );
                last_log_head = head;
                last_log_at = Instant::now();
            }

            if root.live_count == 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            }
            if head > last_fired {
                last_fired = head;
            }
            let window_end = max_seen.saturating_add(1).min(head + IN_FLIGHT);
            let fire_count = window_end.saturating_sub(last_fired) as usize;
            if fire_count == 0 {
                if last_fired > head + 2 {
                    last_fired = head;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            let bh = match rpc.get_latest_blockhash().await {
                Ok(h) => h,
                Err(e) => {
                    warn!(target: "v4_reveal", error = %e, "blockhash fetch failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };

            for i in 0..fire_count {
                let seq = last_fired + i as u64;
                let entry = {
                    let g = store.lock();
                    g.get(&seq).cloned()
                };
                let Some(entry) = entry else {
                    // No reveal material — usually means the commit was made
                    // by a previous relayer process (in-memory store wiped on
                    // restart) or a different relayer instance. Counts as an
                    // attempt for autodrop purposes: from the head's POV we
                    // *cannot* advance it.
                    debug!(target: "v4_reveal", seq, head, "no reveal material in store; head will need autodrop");
                    stall.note_reveal_attempt();
                    tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
                    continue;
                };
                let tx = build_reveal_execute_tx(&mkt, &payer, &entry, bh);
                stall.note_reveal_attempt();
                match rpc.send_transaction_with_config(&tx, send_cfg).await {
                    Ok(sig) => {
                        debug!(target: "v4_reveal", seq, %sig, "reveal sent");
                    }
                    Err(e) => {
                        // Surface so we can tell the difference between "didn't
                        // send" and "head not advancing because reveals fail
                        // on-chain".
                        warn!(target: "v4_reveal", seq, head, error = %e, "reveal send failed");
                    }
                }
                tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
            }
            last_fired += fire_count as u64;

            // Garbage-collect store entries below head.
            {
                let mut g = store.lock();
                let to_drop: Vec<u64> = g.range(..head).map(|(k, _)| *k).collect();
                for k in to_drop {
                    g.remove(&k);
                }
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
}

// ---------------------------------------------------------------------------
// Admin tx builders (page init, configure_market_root, drop_head_market)
// ---------------------------------------------------------------------------

/// `execution_queue_v4_create_market_page(page_slot)` — creates the on-chain
/// account at the page PDA. Idempotent at the call site (caller checks
/// existence first).
pub fn build_create_market_page_ix(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    payer: Pubkey,
    page_slot: u16,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new_readonly(group, false),
        AccountMeta::new_readonly(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new(queue_page, false),
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
    ];
    let mut data = anchor_discriminator("execution_queue_v4_create_market_page").to_vec();
    data.extend_from_slice(&page_slot.to_le_bytes());
    Instruction { program_id, accounts, data }
}

/// `execution_queue_v4_resize_market_page(page_slot)` — chunked grow path.
/// Must be called repeatedly until the account reaches full size before
/// `init_market_page` can load it as zero-copy.
pub fn build_resize_market_page_ix(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    payer: Pubkey,
    page_slot: u16,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new_readonly(group, false),
        AccountMeta::new_readonly(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new(queue_page, false),
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
    ];
    let mut data = anchor_discriminator("execution_queue_v4_resize_market_page").to_vec();
    data.extend_from_slice(&page_slot.to_le_bytes());
    Instruction { program_id, accounts, data }
}

/// `execution_queue_v4_init_market_page(page_slot, assigned_abs_page_no)`
pub fn build_init_market_page_ix(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    page_slot: u16,
    assigned_abs_page_no: u64,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new_readonly(group, false),
        AccountMeta::new_readonly(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new(queue_page, false),
    ];
    let mut data = anchor_discriminator("execution_queue_v4_init_market_page").to_vec();
    data.extend_from_slice(&page_slot.to_le_bytes());
    data.extend_from_slice(&assigned_abs_page_no.to_le_bytes());
    Instruction { program_id, accounts, data }
}

/// `execution_queue_v4_configure_market_root(params)` — admin-only.
/// Only `pause_execute` matters for our autodrop flow.
pub fn build_configure_market_root_ix(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    admin: Pubkey,
    soft_limit: u16,
    gap_wait_slots: u16,
    pause_ingress: bool,
    pause_execute: bool,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new_readonly(group, false),
        AccountMeta::new_readonly(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new_readonly(admin, true),
    ];
    let mut data = anchor_discriminator("execution_queue_v4_configure_market_root").to_vec();
    // ExecutionQueueV4MarketRootConfigParams { soft_limit: u16, gap_wait_slots: u16,
    //                                          pause_ingress: bool, pause_execute: bool }
    data.extend_from_slice(&soft_limit.to_le_bytes());
    data.extend_from_slice(&gap_wait_slots.to_le_bytes());
    data.push(u8::from(pause_ingress));
    data.push(u8::from(pause_execute));
    Instruction { program_id, accounts, data }
}

/// `execution_queue_v4_drop_head_market(count)` — admin-only, requires
/// `paused_execute = true`.
pub fn build_drop_head_market_ix(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    admin: Pubkey,
    count: u16,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new_readonly(group, false),
        AccountMeta::new_readonly(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new(queue_page, false),
        AccountMeta::new_readonly(admin, true),
    ];
    let mut data = anchor_discriminator("execution_queue_v4_drop_head_market").to_vec();
    data.extend_from_slice(&count.to_le_bytes());
    Instruction { program_id, accounts, data }
}

/// Atomic "pause → drop_head_market(count) → unpause" tx. Atomicity is
/// important so a crash mid-tx never leaves the queue paused.
pub fn build_pause_drop_unpause_tx(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    admin: &Keypair,
    payer: &Keypair,
    count: u16,
    soft_limit: u16,
    gap_wait_slots: u16,
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
    let pause = build_configure_market_root_ix(
        program_id,
        group,
        authority_state,
        queue_root,
        admin.pubkey(),
        soft_limit,
        gap_wait_slots,
        false,
        true,
    );
    let drop_ix = build_drop_head_market_ix(
        program_id,
        group,
        authority_state,
        queue_root,
        queue_page,
        admin.pubkey(),
        count,
    );
    let unpause = build_configure_market_root_ix(
        program_id,
        group,
        authority_state,
        queue_root,
        admin.pubkey(),
        soft_limit,
        gap_wait_slots,
        false,
        false,
    );
    let mut signers: Vec<&Keypair> = vec![payer];
    if admin.pubkey() != payer.pubkey() {
        signers.push(admin);
    }
    Transaction::new_signed_with_payer(
        &[cu, pause, drop_ix, unpause],
        Some(&payer.pubkey()),
        &signers,
        blockhash,
    )
}

/// Poll signature status until confirmed or timeout. Returns `Ok(true)` if
/// the tx confirmed without on-chain error, `Ok(false)` on timeout, `Err`
/// if the tx confirmed with an InstructionError or the RPC failed.
pub async fn wait_for_signature_confirmation(
    rpc: &RpcClient,
    sig: &solana_sdk::signature::Signature,
    timeout: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        let resp = rpc.get_signature_statuses(&[*sig]).await?;
        if let Some(Some(status)) = resp.value.into_iter().next() {
            if let Some(err) = status.err {
                return Err(anyhow!("tx on-chain error: {err:?}"));
            }
            if status.confirmation_status.is_some() {
                return Ok(true);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// ---------------------------------------------------------------------------
// Page auto-initialization (one-shot at startup, on-demand during drift)
// ---------------------------------------------------------------------------

/// Full size of an initialized `CommitPageV4` account (matches on-chain
/// `EXECUTION_QUEUE_COMMIT_PAGE_V4_SPACE`). Used to decide when the chunked
/// grow loop is done.
const COMMIT_PAGE_V4_FULL_SIZE: u64 = 24_648;
/// Solana max per-tx realloc grow.
const MAX_PERMITTED_DATA_INCREASE: u64 = 10_240;

/// Send a single tx (skip-preflight + manual confirmation poll) and report
/// final landing. Returns `Ok(())` if the tx confirmed without error.
async fn send_and_confirm_skip_preflight(
    rpc: &RpcClient,
    payer: &Keypair,
    extra_signers: &[&Keypair],
    ixs: Vec<Instruction>,
    label: &str,
) -> Result<()> {
    let bh = rpc
        .get_latest_blockhash_with_commitment(
            solana_sdk::commitment_config::CommitmentConfig::finalized(),
        )
        .await?
        .0;
    let mut signers: Vec<&Keypair> = vec![payer];
    for s in extra_signers {
        if s.pubkey() != payer.pubkey() {
            signers.push(s);
        }
    }
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &signers, bh);
    let sig = rpc
        .send_transaction_with_config(
            &tx,
            RpcSendTransactionConfig {
                skip_preflight: true,
                max_retries: Some(5),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| anyhow!("{label}: send failed: {e}"))?;
    debug!(target: "v4_pages", %sig, label, "tx sent");
    let confirmed = wait_for_signature_confirmation(rpc, &sig, Duration::from_secs(30)).await;
    match confirmed {
        Ok(true) => {
            debug!(target: "v4_pages", %sig, label, "tx confirmed");
            Ok(())
        }
        Ok(false) => Err(anyhow!("{label}: confirmation timed out (sig={sig})")),
        Err(e) => Err(anyhow!("{label}: {e} (sig={sig})")),
    }
}

/// Initialize a single page slot end-to-end: create → resize×N → init.
/// Each step is its own tx (CU / data limits) with skip-preflight + manual
/// confirmation polling.
async fn init_one_page(
    rpc: &RpcClient,
    mkt: &V4MarketState,
    payer: &Keypair,
    slot: u16,
    page_pda: Pubkey,
) -> Result<()> {
    // Step 1: create (if missing)
    let create_needed = rpc.get_account(&page_pda).await.is_err();
    if create_needed {
        info!(target: "v4_pages", slot, %page_pda, "creating page account");
        let cu = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
        let create_ix = build_create_market_page_ix(
            mkt.program_id,
            mkt.group,
            mkt.authority_state,
            mkt.queue_root,
            page_pda,
            payer.pubkey(),
            slot,
        );
        send_and_confirm_skip_preflight(rpc, payer, &[], vec![cu, create_ix], "create_page")
            .await?;
    }

    // Step 2: resize loop until full size
    loop {
        let acct = rpc.get_account(&page_pda).await?;
        let cur = acct.data.len() as u64;
        if cur >= COMMIT_PAGE_V4_FULL_SIZE {
            break;
        }
        let next = (cur + MAX_PERMITTED_DATA_INCREASE).min(COMMIT_PAGE_V4_FULL_SIZE);
        info!(
            target: "v4_pages", slot, %page_pda,
            current_size = cur, next_size = next,
            "resizing page"
        );
        let cu = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
        let resize_ix = build_resize_market_page_ix(
            mkt.program_id,
            mkt.group,
            mkt.authority_state,
            mkt.queue_root,
            page_pda,
            payer.pubkey(),
            slot,
        );
        send_and_confirm_skip_preflight(rpc, payer, &[], vec![cu, resize_ix], "resize_page")
            .await?;
    }

    // Step 3: init (zero-copy load)
    info!(target: "v4_pages", slot, %page_pda, "init_market_page");
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
    let init_ix = build_init_market_page_ix(
        mkt.program_id,
        mkt.group,
        mkt.authority_state,
        mkt.queue_root,
        page_pda,
        slot,
        u64::from(slot),
    );
    send_and_confirm_skip_preflight(rpc, payer, &[], vec![cu, init_ix], "init_page").await?;
    Ok(())
}

/// Returns `true` when the page exists *and* has reached full size *and* its
/// `page_state` is Active (== 1). A partially-grown or uninitialized page
/// returns false, prompting `init_one_page` to finish the job idempotently.
async fn page_is_fully_initialized(rpc: &RpcClient, page_pda: &Pubkey) -> bool {
    let Ok(acct) = rpc.get_account(page_pda).await else { return false };
    if (acct.data.len() as u64) < COMMIT_PAGE_V4_FULL_SIZE {
        return false;
    }
    // CommitPageV4 layout:
    //   disc(8) + queue_root(32) + assigned_abs_page_no(8) + live_count(2)
    //   + first_pending_offset(2) + page_slot(2) + page_state(1)
    // → page_state is at offset 8+32+8+2+2+2 = 54
    acct.data.get(54).copied() == Some(1)
}

/// Read queue_root, derive every `page_slot` PDA, init the missing ones.
/// Safe to call repeatedly. Returns the set of page_slots that are fully
/// initialized after the call (ideally `0..num_pages`).
pub async fn ensure_all_pages_initialized(
    rpc: &RpcClient,
    mkt: &V4MarketState,
    payer: &Keypair,
) -> Result<Vec<u16>> {
    let root_acct = rpc.get_account(&mkt.queue_root).await?;
    let root = PerpMarketCommitRootV4::try_deserialize(&mut &root_acct.data[..])
        .map_err(|e| anyhow!("queue_root deserialize: {e}"))?;
    let num_pages = root.num_pages;
    let mut existing = Vec::new();

    for slot in 0..num_pages {
        let (page_pda, _) = derive_queue_page_pda(&mkt.program_id, &mkt.queue_root, slot);
        if page_is_fully_initialized(rpc, &page_pda).await {
            existing.push(slot);
            debug!(target: "v4_pages", slot, %page_pda, "page exists and fully initialized");
            continue;
        }
        match init_one_page(rpc, mkt, payer, slot, page_pda).await {
            Ok(()) => {
                info!(target: "v4_pages", slot, %page_pda, "page initialized");
                existing.push(slot);
            }
            Err(e) => {
                error!(target: "v4_pages", slot, %page_pda, error = %e, "page init failed");
            }
        }
    }
    Ok(existing)
}

pub fn spawn_page_init_worker(
    rpc: Arc<RpcClient>,
    mkt: Arc<V4MarketState>,
    payer: Arc<Keypair>,
    refresh_interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Burst at startup so we don't block on the periodic interval.
        match ensure_all_pages_initialized(&rpc, &mkt, &payer).await {
            Ok(existing) => info!(
                target: "v4_pages",
                existing = existing.len(),
                pages = ?existing,
                "startup page check complete"
            ),
            Err(e) => warn!(target: "v4_pages", error = %e, "startup page check failed"),
        }
        if refresh_interval_secs == 0 {
            return;
        }
        loop {
            tokio::time::sleep(Duration::from_secs(refresh_interval_secs)).await;
            if let Err(e) = ensure_all_pages_initialized(&rpc, &mkt, &payer).await {
                warn!(target: "v4_pages", error = %e, "periodic page check failed");
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Counter-drift resync
// ---------------------------------------------------------------------------

/// Periodically read queue_root from chain and roll back the local atomic
/// counter if it has drifted past `max_lookahead` ahead of `next_enqueue_sequence`.
///
/// Drift happens when commit txs fail on-chain (skip_preflight buries the
/// failure) but the local `fetch_add` already advanced. Without this, the
/// relayer's "next sequence" wanders linearly until commits land in invalid
/// page_slot ranges and silently fail forever.
pub fn spawn_drift_resync_worker(
    rpc: Arc<RpcClient>,
    mkt: Arc<V4MarketState>,
    next_seq: V4NextSequence,
    interval_ms: u64,
    max_lookahead: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            let root = match rpc.get_account(&mkt.queue_root).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(target: "v4_drift", error = %e, "queue_root fetch failed");
                    continue;
                }
            };
            let parsed = match PerpMarketCommitRootV4::try_deserialize(&mut &root.data[..]) {
                Ok(r) => r,
                Err(e) => {
                    warn!(target: "v4_drift", error = %e, "queue_root deserialize failed");
                    continue;
                }
            };
            let on_chain_next = if parsed.live_count == 0 {
                parsed.next_sequence_to_execute
            } else {
                parsed.max_seen_sequence.saturating_add(1)
            };
            let local = next_seq.load(Ordering::Acquire);
            if local > on_chain_next.saturating_add(max_lookahead) {
                warn!(
                    target: "v4_drift",
                    local,
                    on_chain_next,
                    drift = local - on_chain_next,
                    "local counter drifted; rewinding to on_chain_next"
                );
                // Rewind. This is safe even with concurrent submits: a
                // concurrent fetch_add will produce a duplicate sequence,
                // which the on-chain validate_commit_sequence rejects as
                // out-of-range, and the next resync rewinds again.
                next_seq.store(on_chain_next, Ordering::Release);
            } else if local + 16 < on_chain_next {
                // We're behind on-chain (someone else committed). Catch up.
                info!(
                    target: "v4_drift",
                    local,
                    on_chain_next,
                    "local counter behind on-chain; advancing"
                );
                next_seq.store(on_chain_next, Ordering::Release);
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Autodrop unadvanceable heads
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct V4AutodropConfig {
    pub stall_threshold_secs: u64,
    pub min_reveal_attempts: u64,
    pub max_drops_per_minute: u32,
    pub drop_count_per_call: u16,
    pub poll_interval_secs: u64,
    pub queue_soft_limit: u16,
    pub queue_gap_wait_slots: u16,
}

impl V4AutodropConfig {
    pub fn defaults() -> Self {
        Self {
            stall_threshold_secs: 30,
            min_reveal_attempts: 6,
            max_drops_per_minute: 30,
            drop_count_per_call: 1,
            poll_interval_secs: 10,
            queue_soft_limit: 0,
            queue_gap_wait_slots: 4,
        }
    }
}

/// Detect heads that aren't advancing despite repeated reveal attempts
/// (typically commits whose dispatch_accounts referenced now-deleted on-chain
/// accounts — e.g. after a perp_market re-deploy). When detected, send an
/// atomic pause+drop+unpause tx using the admin keypair.
pub fn spawn_autodrop_worker(
    rpc: Arc<RpcClient>,
    mkt: Arc<V4MarketState>,
    admin: Arc<Keypair>,
    payer: Arc<Keypair>,
    stall: V4HeadStallState,
    cfg: V4AutodropConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let drop_window_us: u64 = 60 * 1_000_000;
        let drop_window_capacity = cfg.max_drops_per_minute as u64;
        let drop_history: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
        let (page_pda, _) = derive_queue_page_pda(&mkt.program_id, &mkt.queue_root, 0);

        // Long back-off when the deployed on-chain program is missing the
        // `drop_head_market` handler (manifests as Anchor `InstructionFallbackNotFound`,
        // custom error 101). Without this we'd hammer the RPC every poll.
        let unsupported_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        info!(
            target: "v4_autodrop",
            stall_threshold_secs = cfg.stall_threshold_secs,
            min_reveal_attempts = cfg.min_reveal_attempts,
            max_drops_per_minute = cfg.max_drops_per_minute,
            page = %page_pda,
            "autodrop worker started"
        );

        loop {
            tokio::time::sleep(Duration::from_secs(cfg.poll_interval_secs.max(1))).await;

            // If we previously detected the on-chain program lacks the
            // drop_head_market handler, sleep here until the back-off expires.
            {
                let g = unsupported_until.lock();
                if let Some(until) = *g {
                    if Instant::now() < until {
                        continue;
                    }
                }
            }

            let (head, head_age, live_count, attempts) = stall.snapshot();
            let Some(age) = head_age else { continue };
            if live_count == 0 {
                continue;
            }
            if age.as_secs() < cfg.stall_threshold_secs {
                continue;
            }
            if attempts < cfg.min_reveal_attempts {
                continue;
            }

            // Rate limit
            {
                let mut history = drop_history.lock();
                let now = Instant::now();
                history.retain(|t| now.duration_since(*t).as_micros() <= drop_window_us as u128);
                if history.len() as u64 >= drop_window_capacity {
                    warn!(
                        target: "v4_autodrop",
                        head,
                        live_count,
                        rate_limit = drop_window_capacity,
                        "would drop but rate-limited"
                    );
                    continue;
                }
                history.push(now);
            }

            // For drop_head_market we need the page that owns the current head
            // sequence — derive from queue_root state.
            let head_page_slot = match rpc.get_account(&mkt.queue_root).await {
                Ok(a) => match PerpMarketCommitRootV4::try_deserialize(&mut &a.data[..]) {
                    Ok(r) => r.page_slot_for_sequence(r.next_sequence_to_execute),
                    Err(e) => {
                        warn!(target: "v4_autodrop", error = %e, "queue_root decode failed");
                        continue;
                    }
                },
                Err(e) => {
                    warn!(target: "v4_autodrop", error = %e, "queue_root fetch failed");
                    continue;
                }
            };
            let (head_page_pda, _) =
                derive_queue_page_pda(&mkt.program_id, &mkt.queue_root, head_page_slot);

            let bh = match rpc
                .get_latest_blockhash_with_commitment(
                    solana_sdk::commitment_config::CommitmentConfig::finalized(),
                )
                .await
            {
                Ok(h) => h.0,
                Err(e) => {
                    warn!(target: "v4_autodrop", error = %e, "blockhash fetch failed");
                    continue;
                }
            };
            let tx = build_pause_drop_unpause_tx(
                mkt.program_id,
                mkt.group,
                mkt.authority_state,
                mkt.queue_root,
                head_page_pda,
                &admin,
                &payer,
                cfg.drop_count_per_call,
                cfg.queue_soft_limit,
                cfg.queue_gap_wait_slots,
                bh,
            );
            warn!(
                target: "v4_autodrop",
                head,
                head_page_slot,
                age_secs = age.as_secs(),
                attempts,
                live_count,
                "head unadvanceable — issuing admin pause+drop+unpause"
            );
            match rpc
                .send_transaction_with_config(
                    &tx,
                    RpcSendTransactionConfig {
                        skip_preflight: true,
                        max_retries: Some(5),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(sig) => {
                    info!(target: "v4_autodrop", head, %sig, "admin drop sent");
                    match wait_for_signature_confirmation(&rpc, &sig, Duration::from_secs(30))
                        .await
                    {
                        Ok(true) => info!(
                            target: "v4_autodrop",
                            head, %sig,
                            "admin drop landed"
                        ),
                        Ok(false) => warn!(
                            target: "v4_autodrop",
                            head, %sig,
                            "admin drop confirmation timed out"
                        ),
                        Err(e) => {
                            let msg = format!("{e}");
                            // Anchor InstructionFallbackNotFound = Custom 101.
                            // Means the deployed program lacks the handler — we
                            // back off for an hour to avoid spamming.
                            if msg.contains("Custom(101)") {
                                let until = Instant::now() + Duration::from_secs(3600);
                                *unsupported_until.lock() = Some(until);
                                error!(
                                    target: "v4_autodrop",
                                    head, %sig,
                                    "deployed program lacks drop_head_market handler — \
                                     autodrop disabled for 1h. Redeploy the on-chain \
                                     program (5KaJhG2…) to enable autodrop."
                                );
                            } else {
                                error!(
                                    target: "v4_autodrop",
                                    head, %sig,
                                    error = %e,
                                    "admin drop confirmation failed"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(target: "v4_autodrop", head, error = %e, "admin drop send failed");
                }
            }
        }
    })
}

// Re-export AtomicU64 / Ordering paths so external callers don't need to
// reach into std::sync directly.
pub use std::sync::atomic::AtomicU64 as ReexportAtomicU64;
pub use std::sync::atomic::Ordering as ReexportAtomicOrdering;
