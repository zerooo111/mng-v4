//! Inline v5 commit-reveal pipeline. Drop-in successor to v4_pipeline for
//! the single-account ring-buffer queue design (no page PDAs, no page-init
//! workers). Commit/reveal semantics and signing logic are unchanged.

use anchor_lang::{InstructionData, ToAccountMetas};
use anyhow::{anyhow, Result};
use mango_v4::instructions::{CommitEntryV5, ExecutionQueueV5GlobalPauseParams, RevealArgsV5};
use mango_v4::state::{
    EXECUTION_QUEUE_V5_N_MAX_MARKETS, EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY,
    EXECUTION_QUEUE_V5_SUB_QUEUE_HEADERS_OFFSET, EXECUTION_QUEUE_V5_SUB_QUEUE_HEADER_STRIDE,
    SQH_V5_ACTIVE_OFFSET, SQH_V5_LIVE_COUNT_OFFSET, SQH_V5_MARKET_INDEX_OFFSET,
    SQH_V5_MAX_SEEN_SEQ_OFFSET, SQH_V5_NEXT_SEQ_OFFSET,
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
// RPC failover — Helius → Triton on throttle-class errors
// ---------------------------------------------------------------------------

/// Helius periodically returns a cluster of throttle-class errors when the
/// pooled HTTP/2 connection hits its internal reset limit. Solana's
/// reqwest-based RpcClient surfaces these as nested strings that don't map
/// cleanly to a specific variant — match on the message instead.
pub fn is_rpc_throttle_error(err: &solana_client::client_error::ClientError) -> bool {
    let s = err.to_string();
    s.contains("too_many_internal_resets")
        || s.contains("detected excessive load generating behavior")
        || s.contains("http2 error: connection error detected")
        || s.contains("connection error detected")
        || s.contains("connection closed before message completed")
}

/// Send a versioned transaction via `primary`; on throttle-class errors,
/// fall back to `secondary` when configured. Returns the landed signature
/// and a bool indicating whether the secondary handled the send.
pub async fn send_tx_with_failover(
    primary: &RpcClient,
    secondary: Option<&RpcClient>,
    tx: &solana_sdk::transaction::VersionedTransaction,
    cfg: RpcSendTransactionConfig,
) -> std::result::Result<(solana_sdk::signature::Signature, bool), solana_client::client_error::ClientError>
{
    match primary.send_transaction_with_config(tx, cfg).await {
        Ok(sig) => Ok((sig, false)),
        Err(e) if is_rpc_throttle_error(&e) => {
            if let Some(sec) = secondary {
                warn!(
                    target: "rpc_failover",
                    error = %e,
                    "primary RPC throttled; failing over to secondary"
                );
                match sec.send_transaction_with_config(tx, cfg).await {
                    Ok(sig) => Ok((sig, true)),
                    Err(e2) => Err(e2),
                }
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

/// Legacy-transaction variant of `send_tx_with_failover`. The reveal and
/// autodrop paths build `solana_sdk::transaction::Transaction` (not
/// VersionedTransaction), so they need a separate entry point.
pub async fn send_legacy_tx_with_failover(
    primary: &RpcClient,
    secondary: Option<&RpcClient>,
    tx: &solana_sdk::transaction::Transaction,
    cfg: RpcSendTransactionConfig,
) -> std::result::Result<(solana_sdk::signature::Signature, bool), solana_client::client_error::ClientError>
{
    match primary.send_transaction_with_config(tx, cfg).await {
        Ok(sig) => Ok((sig, false)),
        Err(e) if is_rpc_throttle_error(&e) => {
            if let Some(sec) = secondary {
                warn!(
                    target: "rpc_failover",
                    error = %e,
                    "primary RPC throttled; failing over to secondary"
                );
                match sec.send_transaction_with_config(tx, cfg).await {
                    Ok(sig) => Ok((sig, true)),
                    Err(e2) => Err(e2),
                }
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
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

/// Snapshot of (max_seen_sequence, live_count) for a specific market's
/// sub-queue. Returned separately from `read_starting_sequences` so the
/// startup reconciliation can classify restored WAL entries as pending
/// vs orphaned.
pub fn read_sub_queue_snapshot(data: &[u8], market_index: u16) -> Option<(u64, u32)> {
    parse_sub_queue_for_market(data, market_index)
        .map(|sqh| (sqh.max_seen_sequence, sqh.live_count))
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

/// P0.5: the single unified user-intent hash used by v5 commit-reveal.
/// Must match `programs/mango-v4/src/instructions/execution_queue.rs
/// canonical_user_intent_message_v3` byte-for-byte. Commit stores this
/// hash as `commit_hash`; reveal recomputes and matches; the ed25519
/// pre-ix signs this same hash.
///
/// `client_order_id` is the 8-byte user-supplied randomizer that makes
/// brute-forcing the commit pre-image infeasible even for small payload
/// spaces (cancels, standard-size places).
#[allow(clippy::too_many_arguments)]
pub fn canonical_user_intent_v3(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    kind: u8,
    target_kind: u8,
    target_index: u16,
    payload_hash: &[u8; 32],
    client_order_id: u64,
) -> [u8; 32] {
    hashv(&[
        b"mango-v5-user-intent-v1",
        group.as_ref(),
        mango_account.as_ref(),
        user_owner.as_ref(),
        &[kind],
        &[target_kind],
        &target_index.to_le_bytes(),
        payload_hash,
        &client_order_id.to_le_bytes(),
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
    // P0.5: the on-chain reveal handler recomputes ONE unified hash
    // (canonical_user_intent_v3 over group, mango_account, user_owner,
    // kind, target_kind, market_index, payload_hash, client_order_id)
    // and fails the reveal if it does not match the stored commit. Every
    // argument that feeds that hash must round-trip through the WAL
    // unchanged — the reveal_entry carries mango_account, user_owner,
    // kind, and client_order_id exactly as they were at commit time.
    let recomputed_commit = canonical_user_intent_v3(
        mkt.group,
        reveal_entry.mango_account,
        reveal_entry.user_owner,
        reveal_entry.kind,
        /* target_kind = PerpMarket */ 0,
        mkt.market_index,
        &reveal_entry.payload_hash,
        reveal_entry.client_order_id,
    );
    debug!(
        target: "v5_reveal_debug",
        seq = reveal_entry.sequence,
        n_dispatch = reveal_entry.dispatch_accounts.len(),
        kind = reveal_entry.kind,
        client_order_id = reveal_entry.client_order_id,
        payload_hash = hex_short(&reveal_entry.payload_hash),
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

    // Under the unified hash, the user signs the SAME bytes that land
    // on-chain as the commit. user_intent_hash on the reveal entry is
    // the unified hash; use that if present.
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
                client_order_id: reveal_entry.client_order_id,
                mango_account: reveal_entry.mango_account,
                user_owner: reveal_entry.user_owner,
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
    secondary_rpc: Option<Arc<RpcClient>>,
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
                let res = send_legacy_tx_with_failover(
                    &rpc,
                    secondary_rpc.as_deref(),
                    &tx,
                    send_cfg,
                )
                .await;
                match res {
                    Ok((sig, via_secondary)) => {
                        debug!(
                            target: "v5_reveal",
                            seq,
                            %sig,
                            via_secondary,
                            "reveal sent"
                        );
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

            // On-chain `validate_commit_sequence` admits commits only in
            // the half-open interval [next_sequence_to_execute,
            // next_sequence_to_execute + admission_limit). `admission_limit
            // <= per-market capacity (256)`, so:
            //
            //   admission_ceiling = on_chain_next + capacity        // first INVALID seq
            //   max_valid_seq     = admission_ceiling - 1            // last valid seq
            //
            // Three invariants to enforce against a drifted local counter:
            //
            //  (1) Purge reveal-store entries at seq >= admission_ceiling.
            //      They can never land and poison rewind targets.
            //
            //  (2) When the system is WEDGED (local already past ceiling
            //      even after (1)) the entries between max_seen+1 and
            //      ceiling-1 are almost certainly orphans from
            //      commits-that-sent-but-errored-on-chain: max_seen only
            //      advances via successful commits, so anything above it
            //      for long is suspect. Purge these aggressively.
            //
            //  (3) Rewind target must itself be a VALID seq — capped at
            //      max_valid_seq, not admission_ceiling. The old code
            //      targeted `max(on_chain_next, max_stored+1)` which
            //      equals admission_ceiling when max_stored = ceiling-1,
            //      and every subsequent commit immediately fails 6076.
            const IN_FLIGHT_TOLERANCE: u64 = 32;
            let capacity = EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY as u64;
            let admission_ceiling = on_chain_next.saturating_add(capacity);
            let max_valid_seq = admission_ceiling.saturating_sub(1);
            let stale_below_cutoff = sqh.next_sequence_to_execute;

            let local_pre = next_seq.load(Ordering::Acquire);
            let system_wedged = local_pre >= admission_ceiling;
            let aggressive_floor = sqh
                .max_seen_sequence
                .saturating_add(IN_FLIGHT_TOLERANCE.saturating_add(1));

            let (purged_orphan, purged_aggressive, max_stored_seq) = {
                let mut store = reveal_store.lock();
                // (a) stale-below-head (already-consumed entries).
                let stale: Vec<u64> =
                    store.range(..stale_below_cutoff).map(|(k, _)| *k).collect();
                for k in &stale {
                    store.remove(k);
                }
                // (b) entries at or beyond admission_ceiling — unreachable.
                let orphan: Vec<u64> =
                    store.range(admission_ceiling..).map(|(k, _)| *k).collect();
                for k in &orphan {
                    store.remove(k);
                }
                // (c) aggressive purge when system is wedged: entries far
                //     above max_seen are assumed orphaned (stale in-flight
                //     from commits that errored on-chain).
                let aggressive: Vec<u64> = if system_wedged {
                    let v: Vec<u64> =
                        store.range(aggressive_floor..).map(|(k, _)| *k).collect();
                    for k in &v {
                        store.remove(k);
                    }
                    v
                } else {
                    Vec::new()
                };
                let max = store.keys().next_back().copied().unwrap_or(0);
                (orphan, aggressive, max)
            };
            if !purged_orphan.is_empty() {
                warn!(
                    target: "v5_drift",
                    count = purged_orphan.len(),
                    first = purged_orphan.first().copied().unwrap_or(0),
                    last = purged_orphan.last().copied().unwrap_or(0),
                    on_chain_next,
                    admission_ceiling,
                    "purged unreachable reveal-store entries (>= admission_ceiling)"
                );
            }
            if !purged_aggressive.is_empty() {
                warn!(
                    target: "v5_drift",
                    count = purged_aggressive.len(),
                    first = purged_aggressive.first().copied().unwrap_or(0),
                    last = purged_aggressive.last().copied().unwrap_or(0),
                    max_seen = sqh.max_seen_sequence,
                    aggressive_floor,
                    "purged stale in-flight reveal-store entries (system wedged, in-range entries suspected orphans)"
                );
            }

            // Rewind target:
            //   - at least on_chain_next (don't rewind into already-processed seqs)
            //   - at least max_stored + 1 (don't duplicate existing entries)
            //   - at most max_valid_seq (next fetch_add must return a valid seq)
            let mut safe_rewind_target =
                on_chain_next.max(max_stored_seq.saturating_add(1));
            if safe_rewind_target > max_valid_seq {
                safe_rewind_target = max_valid_seq;
            }

            let local = next_seq.load(Ordering::Acquire);
            if local > on_chain_next.saturating_add(max_lookahead) {
                warn!(
                    target: "v5_drift",
                    local,
                    on_chain_next,
                    max_stored_seq,
                    rewind_to = safe_rewind_target,
                    admission_ceiling,
                    drift = local - on_chain_next,
                    "local counter drifted; rewinding"
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
    secondary_rpc: Option<Arc<RpcClient>>,
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
            let ad_cfg = RpcSendTransactionConfig {
                skip_preflight: true,
                max_retries: Some(5),
                ..Default::default()
            };
            let ad_res = send_legacy_tx_with_failover(
                &rpc,
                secondary_rpc.as_deref(),
                &tx,
                ad_cfg,
            )
            .await;
            match ad_res {
                Ok((sig, via_secondary)) => {
                    info!(
                        target: "v5_autodrop",
                        head,
                        %sig,
                        via_secondary,
                        "admin drop sent"
                    );
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
