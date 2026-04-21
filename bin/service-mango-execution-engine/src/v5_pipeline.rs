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
    address_lookup_table_account::AddressLookupTableAccount,
    compute_budget::ComputeBudgetInstruction,
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    message::{v0::Message as MessageV0, VersionedMessage},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    sysvar::instructions::ID as INSTRUCTIONS_SYSVAR_ID,
    transaction::{Transaction, VersionedTransaction},
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

/// Re-export for `main.rs` to size the admission-ceiling atomic without
/// adding another use-line.
pub const PER_MARKET_CAPACITY: u64 = EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY as u64;

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
/// Per-(group, market_index) queue PDA. Prior layout used a single queue
/// PDA per group containing all markets' sub-queues, which forced Solana
/// to serialize every reveal across markets on a shared account lock.
/// Moving to per-market PDAs unlocks parallel block-time drain across
/// markets.
pub fn derive_queue_v5_pda(
    program_id: &Pubkey,
    group: &Pubkey,
    market_index: u16,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"execution-queue-v5",
            group.as_ref(),
            &market_index.to_le_bytes(),
        ],
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
            // Seed the allocator past the larger of the two candidates:
            //   - next_sequence_to_execute (the head)
            //   - max_seen_sequence + 1 (next seq that has never been committed)
            // The max() is load-bearing. On a healthy queue the two collapse
            // to the same value, but after an admin drop_range / reset path
            // the state can land at live_count=0 while max_seen > next
            // (slots in (next, max_seen] still carry status=Committed from
            // the prior cycle). If we seed at `next` in that case, the next
            // ~(max_seen - next) commit attempts hit write_commit's
            // ExecutionQueueFull check on every stuck slot and get rejected
            // at preflight. Seeding past max_seen+1 wraps around the ring
            // buffer into fresh slot territory.
            let start = sqh
                .next_sequence_to_execute
                .max(sqh.max_seen_sequence.saturating_add(1));
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
    // on-chain as the commit. The pre-ix must contain the EXACT bytes the
    // signature covers — if the user signed utf8-hex(intent_hash) (some
    // wallet SDKs default to that), putting the raw 32-byte hash in the
    // pre-ix would fail Ed25519SigVerify even though the program's own
    // recompute accepts either form. Prefer `user_sig_message` (captured
    // at commit-time from the verify_user_signature return variant) and
    // fall back to the raw hash only for legacy WAL entries that
    // predate the field.
    if let Some(user_sig) = reveal_entry.user_sig {
        let msg_bytes: Option<Vec<u8>> = if let Some(m) = reveal_entry.user_sig_message.as_ref() {
            Some(m.clone())
        } else {
            reveal_entry.user_intent_hash.map(|h| h.to_vec())
        };
        if let Some(msg_bytes) = msg_bytes {
            ixs.push(build_presigned_ed25519_instruction(
                reveal_entry.user_owner.to_bytes(),
                &msg_bytes,
                user_sig,
            ));
        }
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

/// Build a no-op `reveal_execute_market` tx purely to trigger the
/// program's pre-loop orphan-tail sweep. The sweep (deployed 2026-04-21)
/// runs unconditionally at the top of the ix handler, advances
/// `next_sequence_to_execute` over any stale Committed/Empty slots in
/// `[next, max_seen]` when `live_count == 0`, then the reveal loop sees
/// `live_count == 0` and breaks with `Ok(())`.
///
/// Used by the drift-resync worker when it detects the wedge pattern
/// (live_count == 0 && max_seen >= next). Without this trigger the
/// ix never fires on a wedged market (relayer's reveal_store is empty
/// so the normal reveal dispatch never generates a tx), and the
/// sweep — despite being on-chain — stays dormant.
///
/// The tx carries one dummy `RevealArgsV5` with `dispatch_accounts_count=0`
/// because the program requires `reveals.is_empty() == false`. Post-sweep
/// the reveal loop exits before touching the dummy entry.
pub fn build_sweep_trigger_tx(
    mkt: &V5MarketState,
    payer: &Keypair,
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let mut accounts = mango_v4::accounts::ExecutionQueueV5RevealExecuteMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    accounts.push(AccountMeta::new_readonly(mkt.program_id, false));

    let reveal_ix = Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV5RevealExecuteMarket {
            market_index: mkt.market_index,
            reveals: vec![RevealArgsV5 {
                payload: Vec::new(),
                kind: 0,
                dispatch_accounts_count: 0,
                client_order_id: 0,
                mango_account: Pubkey::default(),
                user_owner: Pubkey::default(),
            }],
        }
        .data(),
    };
    Transaction::new_signed_with_payer(
        &[cu, reveal_ix],
        Some(&payer.pubkey()),
        &[payer],
        blockhash,
    )
}

/// Pack N head-sequential reveals into a single tx. The program's reveal
/// handler iterates `for reveal in reveals.iter()` and reloads `head_item`
/// fresh each iteration, so a batch tx processes head=H, H+1, H+2, …
/// atomically in one landing. This sidesteps the pipelined-multi-tx
/// landing-order race that forces single-tx reveals to a ~2/s ceiling on
/// devnet.
///
/// Caller responsibility:
///   * `entries` must be sorted by sequence, contiguous, and start at the
///     current on-chain head. A gap or wrong-start invalidates the batch.
///   * Tx size budget (legacy = 1232 B) is not enforced here. The caller
///     should size batches based on measured per-reveal marginal cost
///     (~281 B for mixed users, ~217 B for same-user) and the expected
///     number of ed25519 pre-ixs.
///
/// Note: each reveal that requires a user signature gets its own ed25519
/// pre-ix in this initial version. Merging multiple sig entries into one
/// pre-ix (for same-user batches) is a follow-up that saves ~18 B per
/// reveal after the first — worth it only to squeeze batch=3 into legacy
/// tx.
pub fn build_reveal_execute_batch_tx(
    mkt: &V5MarketState,
    payer: &Keypair,
    entries: &[V5RevealEntry],
    blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    assert!(!entries.is_empty(), "batch reveal requires ≥1 entry");

    debug!(
        target: "v5_reveal_debug",
        first_seq = entries.first().map(|e| e.sequence).unwrap_or(0),
        last_seq = entries.last().map(|e| e.sequence).unwrap_or(0),
        batch_size = entries.len(),
        "reveal batch dispatch"
    );

    let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let mut ixs = vec![cu];

    // One ed25519 pre-ix per reveal that carries a user signature. The
    // handler's `verify_user_ed25519_preinstruction` scans ALL pre-ixs and
    // matches any one with the expected hash, so separate pre-ixs compose
    // cleanly across distinct users and distinct intent hashes. Same-user
    // sigs could be merged into one multi-entry pre-ix for a ~18B/reveal
    // saving; deferred as an optimization.
    for entry in entries {
        if let Some(user_sig) = entry.user_sig {
            let msg_bytes: Option<Vec<u8>> = if let Some(m) = entry.user_sig_message.as_ref() {
                Some(m.clone())
            } else {
                entry.user_intent_hash.map(|h| h.to_vec())
            };
            if let Some(msg_bytes) = msg_bytes {
                ixs.push(build_presigned_ed25519_instruction(
                    entry.user_owner.to_bytes(),
                    &msg_bytes,
                    user_sig,
                ));
            }
        }
    }

    // remaining_accounts = [fixed accounts][dispatch for reveal_0][dispatch for reveal_1]…[program_id]
    // The handler slices it by RevealArgsV5.dispatch_accounts_count at a
    // running cursor, so order must match the reveals vec exactly.
    let mut accounts = mango_v4::accounts::ExecutionQueueV5RevealExecuteMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    for entry in entries {
        accounts.extend(entry.dispatch_accounts.clone());
    }
    accounts.push(AccountMeta::new_readonly(mkt.program_id, false));

    let reveals: Vec<RevealArgsV5> = entries
        .iter()
        .map(|e| RevealArgsV5 {
            payload: e.payload.clone(),
            kind: e.kind,
            dispatch_accounts_count: e.dispatch_accounts.len() as u8,
            client_order_id: e.client_order_id,
            mango_account: e.mango_account,
            user_owner: e.user_owner,
        })
        .collect();

    let reveal_ix = Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV5RevealExecuteMarket {
            market_index: mkt.market_index,
            reveals,
        }
        .data(),
    };
    ixs.push(reveal_ix);

    Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer], blockhash)
}

/// VersionedTransaction variant of `build_reveal_execute_batch_tx`. Uses
/// address-lookup-table(s) to compress shared pubkey storage, freeing
/// budget for more reveals per tx. When `alts` is empty this reduces to
/// the same legacy shape as the non-v0 builder (just wrapped in a
/// VersionedMessage::V0) — all addresses stay in the static `accountKeys`.
///
/// Returns `Err` if MessageV0 compilation fails (usually: too many
/// instructions/accounts → would overflow the compact-u16 size limits) or
/// if signing fails.
pub fn build_reveal_execute_batch_tx_v0(
    mkt: &V5MarketState,
    payer: &Keypair,
    entries: &[V5RevealEntry],
    alts: &[AddressLookupTableAccount],
    blockhash: solana_sdk::hash::Hash,
) -> anyhow::Result<VersionedTransaction> {
    if entries.is_empty() {
        anyhow::bail!("batch reveal requires ≥1 entry");
    }

    debug!(
        target: "v5_reveal_debug",
        first_seq = entries.first().map(|e| e.sequence).unwrap_or(0),
        last_seq = entries.last().map(|e| e.sequence).unwrap_or(0),
        batch_size = entries.len(),
        alts = alts.len(),
        "reveal batch dispatch (v0)"
    );

    let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    // The reveal handler allocates per-reveal scratch in heap (decoded
    // payload + dispatch-account slices + health cache). Default 32KB heap
    // is enough for 1-2 reveals; batch=3+ needs extra headroom. Request
    // 128KB — covers up to ~6 reveals of current shape with margin.
    let heap = ComputeBudgetInstruction::request_heap_frame(128 * 1024);
    let mut ixs = vec![cu, heap];

    for entry in entries {
        if let Some(user_sig) = entry.user_sig {
            let msg_bytes: Option<Vec<u8>> = if let Some(m) = entry.user_sig_message.as_ref() {
                Some(m.clone())
            } else {
                entry.user_intent_hash.map(|h| h.to_vec())
            };
            if let Some(msg_bytes) = msg_bytes {
                ixs.push(build_presigned_ed25519_instruction(
                    entry.user_owner.to_bytes(),
                    &msg_bytes,
                    user_sig,
                ));
            }
        }
    }

    let mut accounts = mango_v4::accounts::ExecutionQueueV5RevealExecuteMarket {
        group: mkt.group,
        authority_state: mkt.authority_state,
        queue: mkt.queue,
        instructions: INSTRUCTIONS_SYSVAR_ID,
    }
    .to_account_metas(None);
    for entry in entries {
        accounts.extend(entry.dispatch_accounts.clone());
    }
    accounts.push(AccountMeta::new_readonly(mkt.program_id, false));

    let reveals: Vec<RevealArgsV5> = entries
        .iter()
        .map(|e| RevealArgsV5 {
            payload: e.payload.clone(),
            kind: e.kind,
            dispatch_accounts_count: e.dispatch_accounts.len() as u8,
            client_order_id: e.client_order_id,
            mango_account: e.mango_account,
            user_owner: e.user_owner,
        })
        .collect();

    let reveal_ix = Instruction {
        program_id: mkt.program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueV5RevealExecuteMarket {
            market_index: mkt.market_index,
            reveals,
        }
        .data(),
    };
    ixs.push(reveal_ix);

    let message = MessageV0::try_compile(&payer.pubkey(), &ixs, alts, blockhash)?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(message), &[payer])?;
    Ok(tx)
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
    reveal_batch_size: u64,
    // When non-empty, reveal txs are built as VersionedTransaction (v0)
    // referencing these ALTs — unlocks larger batches under the 1280 B
    // versioned-tx limit (vs 1232 B legacy) and compresses shared
    // pubkeys down to 1-byte indices. Empty falls back to legacy txs.
    alts: Arc<Vec<AddressLookupTableAccount>>,
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
            // Total seqs permitted in flight = pipeline_depth * batch_size
            // (each tx can process batch_size head advances atomically). The
            // `max_seen + 1` cap prevents firing for a seq that hasn't been
            // committed on-chain yet — reveals for uncommitted seqs would
            // hit the gap path (live_count==0 break) and waste slots.
            let batch_size_u64 = reveal_batch_size.max(1);
            let in_flight_seqs = in_flight.saturating_mul(batch_size_u64);
            let window_end = max_seen
                .saturating_add(1)
                .min(head.saturating_add(in_flight_seqs));
            let remaining_seqs = window_end.saturating_sub(last_fired);
            if remaining_seqs == 0 {
                if last_fired > head {
                    last_fired = head;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            // Fire ceil(remaining_seqs / batch_size) txs, each bundling up
            // to batch_size sequential reveals.
            let fire_count =
                ((remaining_seqs + batch_size_u64 - 1) / batch_size_u64) as usize;

            let bh = match rpc.get_latest_blockhash().await {
                Ok(h) => h,
                Err(e) => {
                    warn!(target: "v5_reveal", error = %e, "blockhash fetch failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };

            // Batch-mode send: pack up to `reveal_batch_size` head-sequential
            // entries into one tx per outer-loop tick, then fire at most
            // `fire_count` such txs per tick (each advances last_fired by
            // its batch_size). This trades the legacy "one reveal per tx,
            // depth-many in flight" pattern for "batched reveals inside a
            // serialized tx" — atomic per-tx, no landing-order race, and
            // each batch tx executes N head advances on the chain in one
            // slot.
            let batch_size = reveal_batch_size.max(1) as usize;
            let mut consumed_so_far: u64 = 0;
            'pipeline: for _tx_idx in 0..fire_count {
                // Collect up to `batch_size` consecutive entries starting
                // at last_fired + consumed_so_far. Stop early if a seq is
                // missing from the store (orphan slot, will be handled by
                // autodrop) or if we reach window_end.
                let batch_start = last_fired + consumed_so_far;
                let mut batch: Vec<V5RevealEntry> =
                    Vec::with_capacity(batch_size);
                let batch_limit = std::cmp::min(
                    batch_size as u64,
                    window_end.saturating_sub(batch_start),
                );
                {
                    let g = store.lock();
                    for j in 0..batch_limit {
                        let seq = batch_start + j;
                        match g.get(&seq) {
                            Some(entry) => batch.push(entry.clone()),
                            None => break,
                        }
                    }
                }
                if batch.is_empty() {
                    // Gap at head — bail this tick and let autodrop / next
                    // loop pass handle it. Advance consumed_so_far by one
                    // to avoid tight-looping on a missing seq.
                    debug!(
                        target: "v5_reveal",
                        seq = batch_start,
                        head,
                        "no reveal material at batch head; autodrop will handle"
                    );
                    stall.note_reveal_attempt();
                    consumed_so_far = consumed_so_far.saturating_add(1);
                    tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
                    break 'pipeline;
                }
                let batch_len = batch.len() as u64;
                let first_seq = batch.first().map(|e| e.sequence).unwrap_or(0);
                let last_seq = batch.last().map(|e| e.sequence).unwrap_or(0);
                stall.note_reveal_attempt();
                let res = if alts.is_empty() {
                    let tx = build_reveal_execute_batch_tx(&mkt, &payer, &batch, bh);
                    send_legacy_tx_with_failover(
                        &rpc,
                        secondary_rpc.as_deref(),
                        &tx,
                        send_cfg,
                    )
                    .await
                } else {
                    match build_reveal_execute_batch_tx_v0(&mkt, &payer, &batch, &alts, bh) {
                        Ok(tx) => send_tx_with_failover(
                            &rpc,
                            secondary_rpc.as_deref(),
                            &tx,
                            send_cfg,
                        )
                        .await,
                        Err(e) => {
                            warn!(
                                target: "v5_reveal",
                                first_seq,
                                last_seq,
                                batch_len,
                                error = %e,
                                "v0 tx build failed — skipping batch"
                            );
                            consumed_so_far = consumed_so_far.saturating_add(batch_len);
                            continue;
                        }
                    }
                };
                match res {
                    Ok((sig, via_secondary)) => {
                        debug!(
                            target: "v5_reveal",
                            first_seq,
                            last_seq,
                            batch_len,
                            %sig,
                            via_secondary,
                            "reveal batch sent"
                        );
                    }
                    Err(e) => {
                        warn!(
                            target: "v5_reveal",
                            first_seq,
                            last_seq,
                            batch_len,
                            head,
                            error = %e,
                            "reveal batch send failed"
                        );
                    }
                }
                consumed_so_far = consumed_so_far.saturating_add(batch_len);
                if batch_start + batch_len >= window_end {
                    break 'pipeline;
                }
                tokio::time::sleep(Duration::from_millis(reveal_spacing_ms)).await;
            }
            last_fired += consumed_so_far;

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
    secondary_rpc: Option<Arc<RpcClient>>,
    mkt: Arc<V5MarketState>,
    next_seq: V5NextSequence,
    reveal_store: V5RevealStore,
    admission_ceiling_atomic: Arc<AtomicU64>,
    interval_ms: u64,
    max_lookahead: u64,
    sweep_trigger_payer: Arc<Keypair>,
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

            // On-chain `validate_commit_sequence` admits commits in the
            // half-open interval [next_sequence_to_execute,
            // next_sequence_to_execute + admission_limit). The ceiling is
            // derived from the ACTUAL head (next_sequence_to_execute),
            // NOT `max_seen_sequence+1`. Those two collapse on a healthy
            // queue, but when a gap-tail wedges the head below max_seen
            // (live_count=0 && max_seen > next_seq, or live=N with the
            // head stuck at a failing reveal) the distinction is
            // load-bearing: picking `max_seen+1 + capacity` over-estimates
            // the ceiling and the next rewind target lands in a window
            // the program rejects with ExecutionQueueFull (6076). Mirror
            // the program verbatim.
            //
            // `on_chain_next_floor` tracks the lowest seq that's safe to
            // rewind to — still max(next_seq, max_seen+1) so rewinds
            // don't collide with ring slots that are still in
            // status=Committed from the prior generation.
            const IN_FLIGHT_TOLERANCE: u64 = 32;
            let capacity = EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY as u64;
            let admission_ceiling = sqh.next_sequence_to_execute.saturating_add(capacity);
            let on_chain_next_floor = sqh
                .next_sequence_to_execute
                .max(sqh.max_seen_sequence.saturating_add(1));
            let on_chain_next = on_chain_next_floor; // kept for the existing
                                                     // drift-detection log field
            let max_valid_seq = admission_ceiling.saturating_sub(1);
            let stale_below_cutoff = sqh.next_sequence_to_execute;

            // Publish the ceiling so ingress can back-pressure instead
            // of letting the allocator race past it. Store unconditionally
            // — the relaxed read on the ingress side tolerates a slightly
            // stale value; the worst case is one extra sim before the
            // next refresh catches up.
            admission_ceiling_atomic.store(admission_ceiling, Ordering::Release);

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
            //   - at least on_chain_next_floor (don't rewind into
            //     already-processed seqs OR into ring slots still in
            //     status=Committed from the prior cycle)
            //   - at least max_stored + 1 (don't duplicate existing entries)
            //   - at most max_valid_seq (next fetch_add must return a valid seq)
            //
            // When `on_chain_next_floor >= admission_ceiling` the queue is
            // GENUINELY WEDGED: head stuck low enough that every safe seq
            // is also out-of-window. We clamp the target to max_valid_seq
            // to avoid panicking, and rely on the ingress-side admission
            // guard to start rejecting new intents until autodrop / head
            // advance opens up room.
            let wedged_no_headroom = on_chain_next_floor >= admission_ceiling;
            let mut safe_rewind_target =
                on_chain_next_floor.max(max_stored_seq.saturating_add(1));
            if safe_rewind_target > max_valid_seq {
                safe_rewind_target = max_valid_seq;
            }

            let local = next_seq.load(Ordering::Acquire);
            // Send a sweep-trigger tx when the wedge pattern is live.
            // Program fix (deployed 2026-04-21) runs an unconditional
            // orphan-tail sweep at the top of reveal_execute_market
            // that advances next_seq past max_seen when live_count==0.
            // But the ix fires only when the relayer actually sends a
            // reveal tx — and on a wedged market the reveal_store is
            // empty, so the normal reveal dispatch never generates one.
            // Drive the sweep explicitly here with a dummy reveal tx
            // (see build_sweep_trigger_tx). Guarded on both (a)
            // live_count==0 (otherwise normal reveal flow is already
            // advancing head and the sweep is a no-op), and (b)
            // max_seen >= next (a strictly-empty queue has nothing to
            // sweep).
            let wedge_live_zero =
                sqh.live_count == 0 && sqh.max_seen_sequence >= sqh.next_sequence_to_execute;
            if wedge_live_zero {
                match rpc.get_latest_blockhash().await {
                    Ok(bh) => {
                        let tx = build_sweep_trigger_tx(&mkt, &sweep_trigger_payer, bh);
                        let cfg = solana_client::rpc_config::RpcSendTransactionConfig {
                            skip_preflight: false,
                            preflight_commitment: Some(
                                solana_sdk::commitment_config::CommitmentLevel::Processed,
                            ),
                            max_retries: Some(0),
                            ..Default::default()
                        };
                        match send_legacy_tx_with_failover(
                            &rpc,
                            secondary_rpc.as_deref(),
                            &tx,
                            cfg,
                        )
                        .await
                        {
                            Ok((sig, via_sec)) => info!(
                                target: "v5_drift",
                                event = "sweep_trigger_sent",
                                market_index = mkt.market_index,
                                next = sqh.next_sequence_to_execute,
                                max_seen = sqh.max_seen_sequence,
                                %sig,
                                via_secondary = via_sec,
                                "dispatched on-chain sweep trigger for wedged sub-queue"
                            ),
                            Err(e) => warn!(
                                target: "v5_drift",
                                event = "sweep_trigger_failed",
                                market_index = mkt.market_index,
                                error = %e,
                                "sweep trigger send failed; will retry next tick"
                            ),
                        }
                    }
                    Err(e) => warn!(
                        target: "v5_drift",
                        event = "sweep_trigger_blockhash_failed",
                        error = %e,
                        "blockhash fetch for sweep trigger failed"
                    ),
                }
            }
            if wedged_no_headroom {
                // Observability: every tick while wedged emits a structured
                // event tied to (market, next_seq_to_execute, max_seen,
                // admission_ceiling) so the trace endpoint can explain why
                // ingress is rejecting intents. Per CLAUDE.md "No silent
                // drops" rule — saturation MUST be visible, not silent.
                warn!(
                    target: "v5_drift",
                    event = "admission_saturated",
                    market_index = mkt.market_index,
                    local,
                    on_chain_head = sqh.next_sequence_to_execute,
                    max_seen = sqh.max_seen_sequence,
                    live_count = sqh.live_count,
                    admission_ceiling,
                    max_valid_seq,
                    safe_rewind_target,
                    "admission window saturated (head wedged below max_seen+1 — \
                     new commits will fail with ExecutionQueueFull until autodrop \
                     or reveal advances the head)"
                );
            }
            if local > on_chain_next.saturating_add(max_lookahead) {
                warn!(
                    target: "v5_drift",
                    event = "rewind",
                    market_index = mkt.market_index,
                    local,
                    on_chain_next,
                    on_chain_head = sqh.next_sequence_to_execute,
                    max_seen = sqh.max_seen_sequence,
                    live_count = sqh.live_count,
                    max_stored_seq,
                    rewind_to = safe_rewind_target,
                    admission_ceiling,
                    max_valid_seq,
                    drift = local - on_chain_next,
                    wedged_no_headroom,
                    "local counter drifted; rewinding"
                );
                if local > safe_rewind_target {
                    next_seq.store(safe_rewind_target, Ordering::Release);
                }
            } else if local + 16 < on_chain_next {
                info!(
                    target: "v5_drift",
                    event = "advance",
                    market_index = mkt.market_index,
                    local,
                    on_chain_next,
                    on_chain_head = sqh.next_sequence_to_execute,
                    max_seen = sqh.max_seen_sequence,
                    admission_ceiling,
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
