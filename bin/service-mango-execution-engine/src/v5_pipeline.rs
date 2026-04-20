//! Inline v5 commit-reveal pipeline. Drop-in successor to v4_pipeline for
//! the single-account ring-buffer queue design (no page PDAs, no page-init
//! workers). Commit/reveal semantics and signing logic are unchanged.

use anchor_lang::{InstructionData, ToAccountMetas};
use anyhow::{anyhow, Result};
use mango_v4::instructions::{CommitEntryV5, ExecutionQueueV5GlobalPauseParams, RevealArgsV5};
use mango_v4::state::{
    EXECUTION_QUEUE_V5_N_MAX_MARKETS, EXECUTION_QUEUE_V5_SUB_QUEUE_HEADERS_OFFSET,
    EXECUTION_QUEUE_V5_SUB_QUEUE_HEADER_STRIDE, SQH_V5_ACTIVE_OFFSET, SQH_V5_LIVE_COUNT_OFFSET,
    SQH_V5_MARKET_INDEX_OFFSET, SQH_V5_MAX_SEEN_SEQ_OFFSET, SQH_V5_NEXT_SEQ_OFFSET,
};
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

fn hex_short(h: &[u8; 32]) -> String {
    let mut s = String::with_capacity(16);
    for b in &h[..8] {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

// ---------------------------------------------------------------------------
// Market state — single queue PDA replaces queue_root + per-page PDAs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct V5MarketState {
    pub program_id: Pubkey,
    pub group: Pubkey,
    pub authority_state: Pubkey,
    /// Single queue PDA: seeds = [b"execution-queue-v5", group].
    pub queue: Pubkey,
    pub perp_market: Pubkey,
    pub perp_bids: Pubkey,
    pub perp_asks: Pubkey,
    pub perp_event_queue: Pubkey,
    pub perp_oracle: Pubkey,
    pub usdc_bank: Pubkey,
    pub usdc_oracle: Pubkey,
    pub market_index: u16,
    /// Fee payer pubkey used on every reveal tx. Stored so the accounts_hash
    /// computation can flag-merge against it — the Solana runtime promotes the
    /// payer to writable+signer globally, which flows into the on-chain
    /// `hash_accounts(account_metas_from_infos(...))` computation.
    pub payer: Pubkey,
}

/// V5 entries are structurally identical to V4 entries; alias so WAL calls
/// (`append_insert`, `replay_live`) work without conversion.
pub use crate::v4_pipeline::V4RevealEntry as V5RevealEntry;

/// Shared map of sequence → reveal material. Reveal worker drains this
/// as head advances. Populated by the submit_intent handler.
pub type V5RevealStore = crate::v4_pipeline::V4RevealStore;

/// Sequence counter for v5 commits. Monotonic per queue.
pub type V5NextSequence = crate::v4_pipeline::V4NextSequence;

// ---------------------------------------------------------------------------
// Raw sub-queue header parser (zero-copy account, read off-chain via bytes)
// ---------------------------------------------------------------------------

struct SubQueueSnapshot {
    next_sequence_to_execute: u64,
    max_seen_sequence: u64,
    live_count: u32,
}

fn parse_sub_queue_for_market(data: &[u8], market_index: u16) -> Option<SubQueueSnapshot> {
    for i in 0..EXECUTION_QUEUE_V5_N_MAX_MARKETS {
        let base = EXECUTION_QUEUE_V5_SUB_QUEUE_HEADERS_OFFSET
            + i * EXECUTION_QUEUE_V5_SUB_QUEUE_HEADER_STRIDE;
        let mi_bytes: [u8; 2] = data
            .get(base + SQH_V5_MARKET_INDEX_OFFSET..base + SQH_V5_MARKET_INDEX_OFFSET + 2)?
            .try_into()
            .ok()?;
        let active = *data.get(base + SQH_V5_ACTIVE_OFFSET)?;
        if active != 1 || u16::from_le_bytes(mi_bytes) != market_index {
            continue;
        }
        let live_count = u32::from_le_bytes(
            data.get(base + SQH_V5_LIVE_COUNT_OFFSET..base + SQH_V5_LIVE_COUNT_OFFSET + 4)?
                .try_into()
                .ok()?,
        );
        let next_sequence_to_execute = u64::from_le_bytes(
            data.get(base + SQH_V5_NEXT_SEQ_OFFSET..base + SQH_V5_NEXT_SEQ_OFFSET + 8)?
                .try_into()
                .ok()?,
        );
        let max_seen_sequence = u64::from_le_bytes(
            data.get(base + SQH_V5_MAX_SEEN_SEQ_OFFSET..base + SQH_V5_MAX_SEEN_SEQ_OFFSET + 8)?
                .try_into()
                .ok()?,
        );
        return Some(SubQueueSnapshot {
            next_sequence_to_execute,
            max_seen_sequence,
            live_count,
        });
    }
    None
}

/// Derive the queue PDA from group + program_id. Seeds match on-chain
/// `ExecutionQueueV5Create`.
pub fn derive_queue_v5_pda(program_id: &Pubkey, group: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"execution-queue-v5", group.as_ref()],
        program_id,
    )
}

/// Parse (start_sequence, head_sequence) for `market_index` from raw queue
/// account bytes. `start_sequence` is the next sequence to commit;
/// `head_sequence` is `next_sequence_to_execute` (the on-chain head).
/// Returns (0, 0) when the sub-queue is not yet configured.
pub fn read_starting_sequences(data: &[u8], market_index: u16) -> (u64, u64) {
    match parse_sub_queue_for_market(data, market_index) {
        Some(sqh) => {
            let start = if sqh.live_count == 0 {
                sqh.next_sequence_to_execute
            } else {
                sqh.max_seen_sequence.saturating_add(1)
            };
            (start, sqh.next_sequence_to_execute)
        }
        None => (0, 0),
    }
}

// ---------------------------------------------------------------------------
// hashing (must match on-chain exactly)
// ---------------------------------------------------------------------------

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
/// see them at reveal time — with flags merged against every other position in
/// the reveal tx (the fixed `ExecutionQueueV5RevealExecuteMarket` accounts plus
/// the trailing program_id). The `queue` account replaces `queue_root +
/// queue_page` from v4; the fixed set shrinks from 7 to 6 entries.
pub fn hash_dispatch_accounts_for_reveal(
    mkt: &V5MarketState,
    dispatch_accounts: &[AccountMeta],
) -> [u8; 32] {
    let fixed: [AccountMeta; 6] = [
        AccountMeta::new(mkt.group, false),
        AccountMeta::new_readonly(mkt.authority_state, false),
        AccountMeta::new(mkt.queue, false),
        AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false),
        AccountMeta::new_readonly(mkt.program_id, false),
        AccountMeta { pubkey: mkt.payer, is_signer: true, is_writable: true },
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
    entries: &[CommitEntryV5],
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
// dispatch-accounts layout for perp_place_order
// ---------------------------------------------------------------------------

pub fn build_dispatch_accounts(
    mkt: &V5MarketState,
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
    mkt: &V5MarketState,
    ctm_signer: &Keypair,
    payer: &Keypair,
    first_sequence: u64,
    entry: CommitEntryV5,
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
    let commit_accounts = mango_v4::accounts::ExecutionQueueV5CommitMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    let commit_ix = Instruction {
        program_id: mkt.program_id,
        accounts: commit_accounts,
        data: mango_v4::instruction::ExecutionQueueV5CommitMarket {
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
    mkt: &V5MarketState,
    payer: &Keypair,
    reveal_entry: &V5RevealEntry,
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    // The on-chain reveal handler recomputes commit_hash from (group,
    // market_index, sequence, kind, payload_hash, accounts_hash,
    // min_execute_slot, expires_at_slot) and fails the reveal if it
    // doesn't match the stored commit. All four of these must come from
    // the same envelope the relayer used at commit time — they're carried
    // through the reveal_store (and WAL) on V5RevealEntry.
    let computed_accounts_hash =
        hash_dispatch_accounts_for_reveal(mkt, &reveal_entry.dispatch_accounts);
    let recomputed_commit = canonical_commit_hash(
        mkt.group,
        mkt.market_index,
        reveal_entry.sequence,
        reveal_entry.kind,
        &reveal_entry.payload_hash,
        &computed_accounts_hash,
        reveal_entry.min_execute_slot,
        reveal_entry.expires_at_slot,
    );
    debug!(
        target: "v5_reveal_debug",
        seq = reveal_entry.sequence,
        n_dispatch = reveal_entry.dispatch_accounts.len(),
        kind = reveal_entry.kind,
        min_execute_slot = reveal_entry.min_execute_slot,
        expires_at_slot = reveal_entry.expires_at_slot,
        payload_hash = hex_short(&reveal_entry.payload_hash),
        accounts_hash = hex_short(&computed_accounts_hash),
        commit_hash = hex_short(&recomputed_commit),
        "reveal dispatch"
    );
    for (i, m) in reveal_entry.dispatch_accounts.iter().enumerate() {
        debug!(
            target: "v5_reveal_debug",
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

    let mut accounts = mango_v4::accounts::ExecutionQueueV5RevealExecuteMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    accounts.extend(reveal_entry.dispatch_accounts.clone());
    accounts.push(AccountMeta::new_readonly(mkt.program_id, false));

    let reveal_ix = Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV5RevealExecuteMarket {
            market_index: mkt.market_index,
            reveals: vec![RevealArgsV5 {
                payload: reveal_entry.payload.clone(),
                kind: reveal_entry.kind,
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

#[derive(Clone, Default)]
pub struct V5HeadStallState {
    inner: Arc<Mutex<V5HeadStallInner>>,
}

#[derive(Default)]
struct V5HeadStallInner {
    last_head: u64,
    last_head_changed_at: Option<Instant>,
    last_max_seen: u64,
    last_live_count: u32,
    reveal_attempts_for_current_head: u64,
}

impl V5HeadStallState {
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
    mkt: Arc<V5MarketState>,
    payer: Arc<Keypair>,
    store: V5RevealStore,
    reveal_spacing_ms: u64,
    reveal_pipeline_depth: u64,
    stall: V5HeadStallState,
    wal: Option<Arc<crate::v4_reveal_wal::V4RevealWal>>,
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
        let in_flight: u64 = reveal_pipeline_depth.max(1);
        loop {
            let queue_acct = match rpc.get_account(&mkt.queue).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(target: "v5_reveal", error = %e, "queue rpc fetch failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let sqh = match parse_sub_queue_for_market(&queue_acct.data, mkt.market_index) {
                Some(s) => s,
                None => {
                    warn!(
                        target: "v5_reveal",
                        market_index = mkt.market_index,
                        "sub-queue not found in queue account"
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let head = sqh.next_sequence_to_execute;
            let max_seen = sqh.max_seen_sequence;
            stall.observe(head, max_seen, sqh.live_count);

            if head != last_log_head || last_log_at.elapsed() >= Duration::from_secs(15) {
                info!(
                    target: "v5_reveal",
                    head,
                    max_seen,
                    live_count = sqh.live_count,
                    last_fired,
                    "queue state"
                );
                last_log_head = head;
                last_log_at = Instant::now();
            }

            if sqh.live_count == 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            }
            if head > last_fired {
                last_fired = head;
            }
            let window_end = max_seen.saturating_add(1).min(head + in_flight);
            let fire_count = window_end.saturating_sub(last_fired) as usize;
            if fire_count == 0 {
                if last_fired > head {
                    last_fired = head;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            let bh = match rpc.get_latest_blockhash().await {
                Ok(h) => h,
                Err(e) => {
                    warn!(target: "v5_reveal", error = %e, "blockhash fetch failed");
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
                    debug!(target: "v5_reveal", seq, head, "no reveal material in store; head will need autodrop");
                    stall.note_reveal_attempt();
                    tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
                    continue;
                };
                let tx = build_reveal_execute_tx(&mkt, &payer, &entry, bh);
                stall.note_reveal_attempt();
                match rpc.send_transaction_with_config(&tx, send_cfg).await {
                    Ok(sig) => {
                        debug!(target: "v5_reveal", seq, %sig, "reveal sent");
                    }
                    Err(e) => {
                        warn!(target: "v5_reveal", seq, head, error = %e, "reveal send failed");
                    }
                }
                tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
            }
            last_fired += fire_count as u64;

            let gc_count = {
                let mut g = store.lock();
                let to_drop: Vec<u64> = g.range(..head).map(|(k, _)| *k).collect();
                let n = to_drop.len();
                for k in to_drop {
                    g.remove(&k);
                }
                n
            };
            if gc_count > 0 {
                if let Some(w) = wal.as_ref() {
                    if let Err(e) = w.compact(head) {
                        warn!(target: "v5_reveal", error = %e, head, "WAL compaction failed");
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
}

// ---------------------------------------------------------------------------
// Admin tx builders
// ---------------------------------------------------------------------------

fn build_set_global_pause_ix(
    mkt: &V5MarketState,
    admin: Pubkey,
    pause_ingress: bool,
    pause_execute: bool,
) -> Instruction {
    let accounts = mango_v4::accounts::ExecutionQueueV5Admin {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        admin,
    }
    .to_account_metas(None);
    Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV5SetGlobalPause {
            params: ExecutionQueueV5GlobalPauseParams {
                pause_ingress,
                pause_execute,
            },
        }
        .data(),
    }
}

fn build_drop_head_market_ix(mkt: &V5MarketState, admin: Pubkey, count: u16) -> Instruction {
    let accounts = mango_v4::accounts::ExecutionQueueV5Admin {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        admin,
    }
    .to_account_metas(None);
    Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV5DropHeadMarket {
            market_index: mkt.market_index,
            count,
        }
        .data(),
    }
}

/// Atomic pause → drop_head_market(count) → unpause tx.
pub fn build_pause_drop_unpause_tx(
    mkt: &V5MarketState,
    admin: &Keypair,
    payer: &Keypair,
    count: u16,
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
    let pause = build_set_global_pause_ix(mkt, admin.pubkey(), false, true);
    let drop_ix = build_drop_head_market_ix(mkt, admin.pubkey(), count);
    let unpause = build_set_global_pause_ix(mkt, admin.pubkey(), false, false);
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

/// Poll signature status until confirmed or timeout.
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
// Counter-drift resync
// ---------------------------------------------------------------------------

pub fn spawn_drift_resync_worker(
    rpc: Arc<RpcClient>,
    mkt: Arc<V5MarketState>,
    next_seq: V5NextSequence,
    reveal_store: V5RevealStore,
    interval_ms: u64,
    max_lookahead: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            let acct = match rpc.get_account(&mkt.queue).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(target: "v5_drift", error = %e, "queue fetch failed");
                    continue;
                }
            };
            let sqh = match parse_sub_queue_for_market(&acct.data, mkt.market_index) {
                Some(s) => s,
                None => {
                    warn!(
                        target: "v5_drift",
                        market_index = mkt.market_index,
                        "sub-queue not found"
                    );
                    continue;
                }
            };
            let on_chain_next = if sqh.live_count == 0 {
                sqh.next_sequence_to_execute
            } else {
                sqh.max_seen_sequence.saturating_add(1)
            };
            let max_stored_seq: u64 = {
                let store = reveal_store.lock();
                store.keys().next_back().copied().unwrap_or(0)
            };
            let safe_rewind_target = on_chain_next.max(max_stored_seq.saturating_add(1));

            let local = next_seq.load(Ordering::Acquire);
            if local > on_chain_next.saturating_add(max_lookahead) {
                warn!(
                    target: "v5_drift",
                    local,
                    on_chain_next,
                    max_stored_seq,
                    rewind_to = safe_rewind_target,
                    drift = local - on_chain_next,
                    "local counter drifted; rewinding past in-flight reveal-store entries"
                );
                if local > safe_rewind_target {
                    next_seq.store(safe_rewind_target, Ordering::Release);
                }
            } else if local + 16 < on_chain_next {
                info!(
                    target: "v5_drift",
                    local,
                    on_chain_next,
                    "local counter behind on-chain; advancing"
                );
                next_seq.store(on_chain_next, Ordering::Release);
            }

            {
                let mut store = reveal_store.lock();
                let cutoff = sqh.next_sequence_to_execute;
                let stale: Vec<u64> = store.range(..cutoff).map(|(k, _)| *k).collect();
                for k in stale {
                    store.remove(&k);
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Autodrop unadvanceable heads
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct V5AutodropConfig {
    pub stall_threshold_secs: u64,
    pub min_reveal_attempts: u64,
    pub max_drops_per_minute: u32,
    pub drop_count_per_call: u16,
    pub poll_interval_secs: u64,
}

impl V5AutodropConfig {
    pub fn defaults() -> Self {
        Self {
            stall_threshold_secs: 30,
            min_reveal_attempts: 6,
            max_drops_per_minute: 30,
            drop_count_per_call: 1,
            poll_interval_secs: 10,
        }
    }
}

pub fn spawn_autodrop_worker(
    rpc: Arc<RpcClient>,
    mkt: Arc<V5MarketState>,
    admin: Arc<Keypair>,
    payer: Arc<Keypair>,
    stall: V5HeadStallState,
    cfg: V5AutodropConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let drop_window_us: u64 = 60 * 1_000_000;
        let drop_window_capacity = cfg.max_drops_per_minute as u64;
        let drop_history: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));

        // Back off for 1h if deployed program lacks the drop_head_market handler
        // (manifests as Anchor InstructionFallbackNotFound, custom error 101).
        let unsupported_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        info!(
            target: "v5_autodrop",
            stall_threshold_secs = cfg.stall_threshold_secs,
            min_reveal_attempts = cfg.min_reveal_attempts,
            max_drops_per_minute = cfg.max_drops_per_minute,
            "autodrop worker started"
        );

        loop {
            tokio::time::sleep(Duration::from_secs(cfg.poll_interval_secs.max(1))).await;

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
            if age.as_secs() < cfg.stall_threshold_secs {
                continue;
            }
            // v5 has no per-page orphan state: if live_count == 0 the queue
            // is genuinely empty and there is nothing to drop.
            if live_count == 0 {
                continue;
            }
            if attempts < cfg.min_reveal_attempts {
                continue;
            }

            {
                let mut history = drop_history.lock();
                let now = Instant::now();
                history.retain(|t| now.duration_since(*t).as_micros() <= drop_window_us as u128);
                if history.len() as u64 >= drop_window_capacity {
                    warn!(
                        target: "v5_autodrop",
                        head,
                        live_count,
                        rate_limit = drop_window_capacity,
                        "would drop but rate-limited"
                    );
                    continue;
                }
                history.push(now);
            }

            let bh = match rpc
                .get_latest_blockhash_with_commitment(
                    solana_sdk::commitment_config::CommitmentConfig::finalized(),
                )
                .await
            {
                Ok(h) => h.0,
                Err(e) => {
                    warn!(target: "v5_autodrop", error = %e, "blockhash fetch failed");
                    continue;
                }
            };
            let tx = build_pause_drop_unpause_tx(
                &mkt,
                &admin,
                &payer,
                cfg.drop_count_per_call,
                bh,
            );
            warn!(
                target: "v5_autodrop",
                head,
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
                    info!(target: "v5_autodrop", head, %sig, "admin drop sent");
                    match wait_for_signature_confirmation(&rpc, &sig, Duration::from_secs(30)).await
                    {
                        Ok(true) => info!(
                            target: "v5_autodrop",
                            head, %sig,
                            "admin drop landed"
                        ),
                        Ok(false) => warn!(
                            target: "v5_autodrop",
                            head, %sig,
                            "admin drop confirmation timed out"
                        ),
                        Err(e) => {
                            let msg = format!("{e}");
                            if msg.contains("Custom(101)") {
                                let until = Instant::now() + Duration::from_secs(3600);
                                *unsupported_until.lock() = Some(until);
                                error!(
                                    target: "v5_autodrop",
                                    head, %sig,
                                    "deployed program lacks drop_head_market handler — \
                                     autodrop disabled for 1h. Redeploy the on-chain \
                                     program to enable autodrop."
                                );
                            } else {
                                error!(
                                    target: "v5_autodrop",
                                    head, %sig,
                                    error = %e,
                                    "admin drop confirmation failed"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(target: "v5_autodrop", head, error = %e, "admin drop send failed");
                }
            }
        }
    })
}

pub use std::sync::atomic::AtomicU64 as ReexportAtomicU64;
pub use std::sync::atomic::Ordering as ReexportAtomicOrdering;
