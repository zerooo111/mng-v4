//! Dynamic per-market commit batching worker.
//!
//! Flush policy: whichever fires first —
//!   * buffer hits `MAX_BATCH` (default 50)
//!   * `MAX_IDLE_BEFORE_FLUSH` elapsed with >= 1 pending entry
//!
//! A page-boundary check also forces flushing before a sequence crosses into
//! a new queue page; the on-chain commit instruction refuses a batch that
//! straddles pages.
//!
//! The batcher runs one task per market queue. It consumes `PendingCommit`
//! inputs from an mpsc channel and produces `CommitBatch` outputs that the
//! submit loop signs and lands.
//!
//! Ingress (relayer gRPC handler) path:
//!   1. precompute payload_hash, accounts_hash, commit_hash for the intent
//!   2. persist payload + envelope metadata to the local WAL keyed by seq
//!      (needed later for reveal; see relayer_resilience.md)
//!   3. push a `PendingCommit` into the batcher channel
//!
//! The ingress handler must have already assigned a monotonic `sequence` for
//! this market before calling in — the batcher treats sequences as opaque and
//! only validates they're strictly increasing within a batch.

use crate::v4_builders::{
    build_commit_market_instruction, build_multi_ed25519_instruction,
    canonical_commit_batch_message,
};
use mango_v4::instructions::CommitEntryV4;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const V4_MAX_BATCH: usize = 50;
pub const V4_MAX_IDLE_BEFORE_FLUSH: Duration = Duration::from_millis(1000);

/// Inputs the gRPC submit handler pushes to the batcher.
#[derive(Debug, Clone)]
pub struct PendingCommit {
    pub sequence: u64,
    pub entry: CommitEntryV4,
    /// The page slot the relayer expects this sequence to land on. All commits
    /// in a batch must share this value; used to force a flush at page
    /// boundaries.
    pub expected_page_slot: u16,
}

/// Output: a fully-prepared tx-ready batch, plus the relayer-signed pre-ix.
#[derive(Debug)]
pub struct CommitBatch {
    pub first_sequence: u64,
    pub page_slot: u16,
    pub ed25519_preix: Instruction,
    pub commit_ix: Instruction,
}

/// Per-market static parameters captured once at worker start.
pub struct BatcherMarketContext {
    pub program_id: Pubkey,
    pub group: Pubkey,
    pub authority_state: Pubkey,
    pub queue_root: Pubkey,
    pub market_index: u16,
    pub shard_id: u8,
    /// Looks up the commit_queue_page pubkey for a given page_slot. Injected
    /// so the batcher doesn't need to re-derive PDAs inside the hot loop.
    pub page_pda_for_slot: std::sync::Arc<dyn Fn(u16) -> Pubkey + Send + Sync>,
}

/// Run the batcher. Returns when the input channel is closed.
pub async fn run_batcher(
    ctx: BatcherMarketContext,
    ctm_signer: Keypair,
    mut rx: mpsc::Receiver<PendingCommit>,
    tx: mpsc::Sender<CommitBatch>,
) {
    let mut buf: Vec<PendingCommit> = Vec::with_capacity(V4_MAX_BATCH);
    let mut window_started: Option<Instant> = None;

    loop {
        // Wait for either next incoming or a flush deadline.
        let flush_at =
            window_started.map(|t| t + V4_MAX_IDLE_BEFORE_FLUSH);
        let recv_result = if let Some(deadline) = flush_at {
            let now = Instant::now();
            if now >= deadline {
                // Overdue — flush without waiting.
                flush(&ctx, &ctm_signer, &mut buf, &tx).await;
                window_started = None;
                continue;
            }
            tokio::time::timeout(deadline - now, rx.recv()).await
        } else {
            // Empty buffer: block until something arrives.
            Ok(rx.recv().await)
        };

        match recv_result {
            Ok(Some(item)) => {
                // Force a flush if this item would cross a page boundary.
                if let Some(first) = buf.first() {
                    if first.expected_page_slot != item.expected_page_slot {
                        flush(&ctx, &ctm_signer, &mut buf, &tx).await;
                        window_started = None;
                    }
                }
                if buf.is_empty() {
                    window_started = Some(Instant::now());
                }
                buf.push(item);
                if buf.len() >= V4_MAX_BATCH {
                    flush(&ctx, &ctm_signer, &mut buf, &tx).await;
                    window_started = None;
                }
            }
            Ok(None) => {
                // Channel closed: final flush and exit.
                flush(&ctx, &ctm_signer, &mut buf, &tx).await;
                break;
            }
            Err(_elapsed) => {
                // Window deadline hit.
                flush(&ctx, &ctm_signer, &mut buf, &tx).await;
                window_started = None;
            }
        }
    }
}

async fn flush(
    ctx: &BatcherMarketContext,
    ctm_signer: &Keypair,
    buf: &mut Vec<PendingCommit>,
    tx: &mpsc::Sender<CommitBatch>,
) {
    if buf.is_empty() {
        return;
    }

    // Validate strictly-increasing sequences (defense in depth; ingress should
    // already guarantee this).
    for w in buf.windows(2) {
        debug_assert_eq!(w[1].sequence, w[0].sequence + 1);
    }

    let first_sequence = buf[0].sequence;
    let page_slot = buf[0].expected_page_slot;
    let queue_page = (ctx.page_pda_for_slot)(page_slot);

    let entries: Vec<CommitEntryV4> = buf.iter().map(|p| p.entry.clone()).collect();

    let msg = canonical_commit_batch_message(
        ctx.group,
        ctx.market_index,
        ctx.shard_id,
        first_sequence,
        &entries,
    );
    let sig_bytes: [u8; 64] = ctm_signer
        .sign_message(&msg)
        .as_ref()
        .try_into()
        .expect("ed25519 sig is 64 bytes");
    // multi-sig form is forward-compatible if we ever attach user sigs here.
    let ed25519_preix = build_multi_ed25519_instruction(&[(
        ctm_signer.pubkey(),
        sig_bytes,
        msg.to_vec(),
    )]);

    let commit_ix = build_commit_market_instruction(
        ctx.program_id,
        ctx.group,
        ctx.authority_state,
        ctx.queue_root,
        queue_page,
        ctx.market_index,
        first_sequence,
        entries,
    );

    let batch = CommitBatch {
        first_sequence,
        page_slot,
        ed25519_preix,
        commit_ix,
    };
    let _ = tx.send(batch).await;
    buf.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn batches_fire_at_size_cap() {
        let (tx_in, rx_in) = mpsc::channel::<PendingCommit>(V4_MAX_BATCH * 2);
        let (tx_out, mut rx_out) = mpsc::channel::<CommitBatch>(4);
        let ctm = Keypair::new();
        let program_id = Pubkey::new_unique();
        let queue_root = Pubkey::new_unique();

        let ctx = BatcherMarketContext {
            program_id,
            group: Pubkey::new_unique(),
            authority_state: Pubkey::new_unique(),
            queue_root,
            market_index: 7,
            shard_id: 0,
            page_pda_for_slot: std::sync::Arc::new(|_| Pubkey::new_unique()),
        };

        let handle = tokio::spawn(async move { run_batcher(ctx, ctm, rx_in, tx_out).await });

        for i in 0..V4_MAX_BATCH as u64 {
            tx_in
                .send(PendingCommit {
                    sequence: i,
                    entry: CommitEntryV4 {
                        commit_hash: [i as u8; 32],
                        min_execute_slot: 0,
                        expires_at_slot: 0,
                    },
                    expected_page_slot: 0,
                })
                .await
                .unwrap();
        }

        let batch = rx_out.recv().await.expect("batch");
        assert_eq!(batch.first_sequence, 0);
        drop(tx_in);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn batches_fire_on_idle() {
        let (tx_in, rx_in) = mpsc::channel::<PendingCommit>(4);
        let (tx_out, mut rx_out) = mpsc::channel::<CommitBatch>(4);
        let ctm = Keypair::new();
        let ctx = BatcherMarketContext {
            program_id: Pubkey::new_unique(),
            group: Pubkey::new_unique(),
            authority_state: Pubkey::new_unique(),
            queue_root: Pubkey::new_unique(),
            market_index: 7,
            shard_id: 0,
            page_pda_for_slot: std::sync::Arc::new(|_| Pubkey::new_unique()),
        };
        let handle = tokio::spawn(async move { run_batcher(ctx, ctm, rx_in, tx_out).await });
        tx_in
            .send(PendingCommit {
                sequence: 0,
                entry: CommitEntryV4 {
                    commit_hash: [9; 32],
                    min_execute_slot: 0,
                    expires_at_slot: 0,
                },
                expected_page_slot: 0,
            })
            .await
            .unwrap();
        let batch = tokio::time::timeout(Duration::from_millis(1500), rx_out.recv())
            .await
            .expect("batch within idle window")
            .expect("some");
        assert_eq!(batch.first_sequence, 0);
        drop(tx_in);
        handle.await.unwrap();
    }
}
