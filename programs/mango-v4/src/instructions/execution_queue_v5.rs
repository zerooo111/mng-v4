// execution_queue_v5 instructions. Single-account ring buffer design —
// see state/execution_queue_v5.rs. Commit-reveal flow mirrors v4
// (same canonical hash shape, same ed25519 signature model) so the
// existing relayer signing logic ports unchanged.

use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::system_instruction;

use super::execution_queue::{
    account_metas_from_infos, canonical_user_intent_message_v2, decode_queue_payload,
    dispatch_queue_payload, extract_user_owner_for_ctm_payload, hash_accounts,
    prevalidate_terminal_ctm_payload, queue_health_region_begin, queue_health_region_end,
    queue_health_region_spec, queue_item_kind_for_payload_variant, require_dispatch_market_index,
    terminal_ctm_failure_msg, validate_queue_payload_dispatch_accounts, variant_uses_user_signature,
    verify_ed25519_preinstruction, verify_user_ed25519_preinstruction, QueueItemEnqueued,
    QueueItemProcessed, UserIntentTargetKind, EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE,
    EXECUTION_QUEUE_MAX_RETRIES,
};

// ---------------------------------------------------------------------------
// Params + canonical message helpers
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV5ConfigureMarketParams {
    pub market_index: u16,
    pub shard_id: u8,
    pub soft_limit: u16,
    pub gap_wait_slots: u16,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV5SubQueueConfigParams {
    pub market_index: u16,
    pub soft_limit: u16,
    pub gap_wait_slots: u16,
    pub pause_ingress: bool,
    pub pause_execute: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV5GlobalPauseParams {
    pub pause_ingress: bool,
    pub pause_execute: bool,
}

/// One entry in a committed batch. Same shape as v4 so the relayer's
/// signing code does not need to know which queue version it is writing to.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CommitEntryV5 {
    pub commit_hash: [u8; 32],
    pub min_execute_slot: u64,
    pub expires_at_slot: u64,
}

/// Reveal args: relayer presents the real intent data; program
/// recomputes the commit_hash and rejects any mismatch.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct RevealArgsV5 {
    pub payload: Vec<u8>,
    pub kind: u8,
    pub dispatch_accounts_count: u8,
}

fn canonical_commit_message_v5(
    group: Pubkey,
    market_index: u16,
    sequence: u64,
    kind: u8,
    payload_hash: &[u8; 32],
    accounts_hash: &[u8; 32],
    min_execute_slot: u64,
    expires_at_slot: u64,
) -> [u8; 32] {
    // NOTE: kept on the same wire as v4 ("mango-v4-commit-v1"). The
    // message binds (group, market_index, sequence, kind, hashes,
    // timing) — the storage layout is off-wire and doesn't participate.
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

fn canonical_commit_batch_message_v5(
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

// ---------------------------------------------------------------------------
// Lifecycle: create → resize loop → init → configure_market
// ---------------------------------------------------------------------------

pub fn execution_queue_v5_create(_ctx: Context<ExecutionQueueV5Create>) -> Result<()> {
    // Account created at 8 bytes by Anchor. The `resize` ix grows it in
    // MAX_PERMITTED_DATA_INCREASE-sized chunks until it reaches the full
    // queue size; then `init` finalizes the zero-copy state.
    Ok(())
}

pub fn execution_queue_v5_resize(ctx: Context<ExecutionQueueV5Resize>) -> Result<()> {
    let queue_ai = ctx.accounts.queue.to_account_info();
    let current_len = queue_ai.data_len();
    let target_len = EXECUTION_QUEUE_V5_ACCOUNT_SPACE;
    require!(current_len > 0, MangoError::SomeError);
    if current_len >= target_len {
        return Ok(());
    }
    let next_len = (current_len + MAX_PERMITTED_DATA_INCREASE).min(target_len);
    let rent = Rent::get()?;
    let needed_lamports = rent
        .minimum_balance(next_len)
        .saturating_sub(queue_ai.lamports());
    if needed_lamports > 0 {
        invoke(
            &system_instruction::transfer(
                &ctx.accounts.payer.key(),
                queue_ai.key,
                needed_lamports,
            ),
            &[
                ctx.accounts.payer.to_account_info(),
                queue_ai.clone(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;
    }
    queue_ai.realloc(next_len, true)?;
    Ok(())
}

pub fn execution_queue_v5_init(ctx: Context<ExecutionQueueV5Init>) -> Result<()> {
    require!(
        ctx.accounts.queue.to_account_info().data_len() == EXECUTION_QUEUE_V5_ACCOUNT_SPACE,
        MangoError::ExecutionQueueV5LayoutNotReady
    );
    let bump = *ctx
        .bumps
        .get("queue")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let mut queue = ctx.accounts.queue.load_init()?;
    queue.init(ctx.accounts.group.key(), ctx.accounts.authority_state.key(), bump);
    Ok(())
}

pub fn execution_queue_v5_configure_market(
    ctx: Context<ExecutionQueueV5Admin>,
    params: ExecutionQueueV5ConfigureMarketParams,
) -> Result<()> {
    require!(params.shard_id == 0, MangoError::ExecutionQueueV3UnsupportedShardId);
    let mut queue = ctx.accounts.queue.load_mut()?;
    require!(
        queue.header.layout_version == EXECUTION_QUEUE_V5_LAYOUT_VERSION,
        MangoError::ExecutionQueueSubQueueLayoutVersionMismatch
    );
    let soft_limit = if params.soft_limit == 0 {
        EXECUTION_QUEUE_V5_DEFAULT_SOFT_LIMIT
    } else {
        require!(
            (params.soft_limit as usize) <= EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY,
            MangoError::ExecutionQueueV3InvalidSoftLimit
        );
        params.soft_limit
    };
    let gap_wait = if params.gap_wait_slots == 0 {
        EXECUTION_QUEUE_V5_DEFAULT_GAP_WAIT_SLOTS
    } else {
        params.gap_wait_slots
    };
    queue.allocate_sub_queue(params.market_index, params.shard_id, soft_limit, gap_wait)?;
    Ok(())
}

pub fn execution_queue_v5_configure_sub_queue(
    ctx: Context<ExecutionQueueV5Admin>,
    params: ExecutionQueueV5SubQueueConfigParams,
) -> Result<()> {
    let mut queue = ctx.accounts.queue.load_mut()?;
    let idx = queue
        .find_sub_queue(params.market_index)
        .ok_or_else(|| error!(MangoError::ExecutionQueueV5SubQueueNotConfigured))?;
    let sqh = &mut queue.sub_queue_headers[idx];
    if params.soft_limit != 0 {
        require!(
            (params.soft_limit as usize) <= EXECUTION_QUEUE_V5_PER_MARKET_CAPACITY,
            MangoError::ExecutionQueueV3InvalidSoftLimit
        );
        sqh.soft_limit = params.soft_limit;
    }
    if params.gap_wait_slots != 0 {
        sqh.gap_wait_slots = params.gap_wait_slots;
    }
    sqh.paused_ingress = u8::from(params.pause_ingress);
    sqh.paused_execute = u8::from(params.pause_execute);
    Ok(())
}

pub fn execution_queue_v5_set_global_pause(
    ctx: Context<ExecutionQueueV5Admin>,
    params: ExecutionQueueV5GlobalPauseParams,
) -> Result<()> {
    let mut queue = ctx.accounts.queue.load_mut()?;
    queue.header.paused_ingress = u8::from(params.pause_ingress);
    queue.header.paused_execute = u8::from(params.pause_execute);
    Ok(())
}

pub fn execution_queue_v5_close(_ctx: Context<ExecutionQueueV5Close>) -> Result<()> {
    // Anchor's `close = receiver` attribute handles rent recovery + discriminator
    // zero-ing. Admin is enforced at the account context.
    Ok(())
}

// ---------------------------------------------------------------------------
// drop_head_market — admin escape hatch. Always makes progress: one branch.
// ---------------------------------------------------------------------------

pub fn execution_queue_v5_drop_head_market(
    ctx: Context<ExecutionQueueV5Admin>,
    market_index: u16,
    count: u16,
) -> Result<()> {
    let mut queue = ctx.accounts.queue.load_mut()?;
    require!(
        queue.header.paused_execute != 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );
    let idx = queue
        .find_sub_queue(market_index)
        .ok_or_else(|| error!(MangoError::ExecutionQueueV5SubQueueNotConfigured))?;
    let mut dropped = 0u16;
    for _ in 0..count {
        if queue.sub_queue_headers[idx].live_count == 0 {
            break;
        }
        queue.clear_head(idx)?;
        dropped = dropped.saturating_add(1);
    }
    msg!(
        "drop_head_market_v5: market={} dropped={}",
        market_index,
        dropped
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// commit_market — relayer writes a signed batch of hashes.
// ---------------------------------------------------------------------------

pub fn execution_queue_v5_commit_market(
    ctx: Context<ExecutionQueueV5CommitMarket>,
    market_index: u16,
    first_sequence: u64,
    entries: Vec<CommitEntryV5>,
) -> Result<()> {
    require!(
        !entries.is_empty() && entries.len() <= EXECUTION_QUEUE_V5_MAX_COMMIT_BATCH,
        MangoError::ExecutionQueueV5InvalidCommitBatch
    );

    let clock = Clock::get()?;
    ctx.accounts
        .authority_state
        .maybe_activate_pending_ctm(clock.slot);

    let mut queue = ctx.accounts.queue.load_mut()?;
    require!(
        queue.header.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    let idx = queue
        .find_sub_queue(market_index)
        .ok_or_else(|| error!(MangoError::ExecutionQueueV5SubQueueNotConfigured))?;
    let shard_id = queue.sub_queue_headers[idx].shard_id;
    require!(shard_id == 0, MangoError::ExecutionQueueV3UnsupportedShardId);
    require!(
        queue.sub_queue_headers[idx].paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );

    // Batch signature check (single ed25519 pre-instruction).
    let batch_msg = canonical_commit_batch_message_v5(
        ctx.accounts.group.key(),
        market_index,
        shard_id,
        first_sequence,
        &entries,
    );
    verify_ed25519_preinstruction(
        ctx.accounts.instructions.as_ref(),
        ctx.accounts.authority_state.ctm_signer,
        batch_msg.as_ref(),
    )?;

    // Admission-time liveness check: reject items that would sit at the head
    // past their expiry (same rule as v4).
    for (i, entry) in entries.iter().enumerate() {
        let seq = first_sequence + i as u64;
        queue.sub_queue_headers[idx].validate_commit_sequence(seq)?;
        if entry.expires_at_slot != 0 {
            require!(
                entry.expires_at_slot >= clock.slot,
                MangoError::ExecutionQueueEnvelopeExpired
            );
            require!(
                entry.min_execute_slot <= entry.expires_at_slot,
                MangoError::ExecutionQueueEnvelopeTimingInvalid
            );
        }
        let mut item = CommitItemV5::default();
        item.sequence = seq;
        item.commit_hash = entry.commit_hash;
        item.min_execute_slot = entry.min_execute_slot;
        item.expires_at_slot = entry.expires_at_slot;
        item.ingress_slot = clock.slot;
        item.status = CommitStatusV5::Committed as u8;
        queue.write_commit(idx, item)?;

        emit!(QueueItemEnqueued {
            group: ctx.accounts.group.key(),
            market_index,
            sequence: seq,
            kind: QueueItemKind::CtmWrapped as u8,
            min_execute_slot: entry.min_execute_slot,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// reveal_execute_market — verify commit hash, dispatch via CPI, advance head.
// ---------------------------------------------------------------------------

/// Skip over gap sequences at head when the relayer has failed to commit them
/// within the gap_wait_slots window. Emits a Skipped event per gap sequence.
/// Mirrors v4's `skip_market_head_gaps_v4` semantics but operates on the
/// single-account v5 layout.
fn skip_sub_queue_head_gaps_v5(
    queue: &mut ExecutionQueueV5,
    idx: usize,
    group: Pubkey,
    market_index: u16,
) {
    let mut skipped = 0u16;
    loop {
        let sqh = &queue.sub_queue_headers[idx];
        if skipped >= EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE {
            break;
        }
        if sqh.live_count == 0 || sqh.max_seen_sequence < sqh.next_sequence_to_execute {
            break;
        }
        let next_seq = sqh.next_sequence_to_execute;
        if queue.get_item(idx, next_seq).is_some() {
            break;
        }
        emit!(QueueItemProcessed {
            group,
            market_index,
            sequence: next_seq,
            kind: QueueItemKind::CtmWrapped as u8,
            status: CommitStatusV5::Failed as u8,
        });
        // Gap: no committed item at head. Advance over it without touching
        // live_count (it wasn't counted). `gap_observed_slot` reset is below.
        queue.sub_queue_headers[idx].next_sequence_to_execute = next_seq.saturating_add(1);
        queue.sub_queue_headers[idx].gap_observed_slot = 0;
        skipped = skipped.saturating_add(1);
    }
}

pub fn execution_queue_v5_reveal_execute_market(
    ctx: Context<ExecutionQueueV5RevealExecuteMarket>,
    market_index: u16,
    reveals: Vec<RevealArgsV5>,
) -> Result<()> {
    require!(
        !reveals.is_empty() && reveals.len() <= EXECUTION_QUEUE_V5_MAX_REVEAL_BATCH,
        MangoError::ExecutionQueueV5InvalidRevealBatch
    );

    let group_key = ctx.accounts.group.key();
    let clock = Clock::get()?;
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);

    // Resolve sub-queue once; borrow release between iterations for mut operations.
    let sub_queue_idx = {
        let queue = ctx.accounts.queue.load()?;
        require!(
            queue.header.paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
        let idx = queue
            .find_sub_queue(market_index)
            .ok_or_else(|| error!(MangoError::ExecutionQueueV5SubQueueNotConfigured))?;
        require!(
            queue.sub_queue_headers[idx].paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
        idx
    };

    let mut cursor: usize = 0;
    let total_accounts = ctx.remaining_accounts.len();

    for reveal in reveals.iter() {
        // Snapshot per-iteration head state.
        let (next_sequence, gap_wait_slots, head_item) = {
            let queue = ctx.accounts.queue.load()?;
            let sqh = &queue.sub_queue_headers[sub_queue_idx];
            if sqh.live_count == 0 {
                break;
            }
            let next_sequence = sqh.next_sequence_to_execute;
            let gap_wait_slots = sqh.gap_wait_slots as u64;
            let head_item = queue.get_item(sub_queue_idx, next_sequence).cloned();
            (next_sequence, gap_wait_slots, head_item)
        };

        // Gap handling: head exists in the ring but the slot is Empty / wrong
        // sequence. Wait gap_wait_slots slots, then skip past.
        let Some(head_item) = head_item else {
            let should_break = {
                let mut queue = ctx.accounts.queue.load_mut()?;
                let sqh = &mut queue.sub_queue_headers[sub_queue_idx];
                if sqh.max_seen_sequence < next_sequence {
                    true
                } else {
                    if sqh.gap_observed_slot == 0 {
                        sqh.gap_observed_slot = clock.slot;
                    }
                    if clock.slot >= sqh.gap_observed_slot.saturating_add(gap_wait_slots) {
                        skip_sub_queue_head_gaps_v5(
                            &mut queue,
                            sub_queue_idx,
                            group_key,
                            market_index,
                        );
                        false
                    } else {
                        true
                    }
                }
            };
            if should_break {
                break;
            }
            continue;
        };

        // Timing gates (slot-based). Envelope timing was bound into commit_hash,
        // so these compare against what the relayer committed.
        if clock.slot < head_item.min_execute_slot {
            break;
        }
        if head_item.expires_at_slot != 0 && clock.slot > head_item.expires_at_slot {
            ctx.accounts.queue.load_mut()?.clear_head(sub_queue_idx)?;
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: head_item.sequence,
                kind: QueueItemKind::CtmWrapped as u8,
                status: CommitStatusV5::Failed as u8,
            });
            continue;
        }

        // Slice this reveal's dispatch accounts.
        let count = reveal.dispatch_accounts_count as usize;
        require!(
            count > 0 && cursor + count <= total_accounts,
            MangoError::ExecutionQueueDispatchAccountLayoutInvalid
        );
        let dispatch_accounts = &ctx.remaining_accounts[cursor..cursor + count];
        cursor += count;

        // Recompute commit_hash and compare to the stored one. Any mismatch
        // — wrong accounts, wrong payload, wrong kind, wrong envelope — fails here.
        let payload_hash = hashv(&[&reveal.payload]).to_bytes();
        let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
        let expected_commit_hash = canonical_commit_message_v5(
            group_key,
            market_index,
            head_item.sequence,
            reveal.kind,
            &payload_hash,
            &accounts_hash,
            head_item.min_execute_slot,
            head_item.expires_at_slot,
        );
        require!(
            expected_commit_hash == head_item.commit_hash,
            MangoError::ExecutionQueueV5CommitRevealMismatch
        );

        // Decode + variant-level pre-validation.
        let decoded_payload = match decode_queue_payload(&reveal.payload) {
            Ok(p) if p.flags == 0 => p,
            _ => {
                ctx.accounts.queue.load_mut()?.clear_head(sub_queue_idx)?;
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: head_item.sequence,
                    kind: reveal.kind,
                    status: CommitStatusV5::Failed as u8,
                });
                continue;
            }
        };
        require!(
            queue_item_kind_for_payload_variant(decoded_payload.variant) == reveal.kind,
            MangoError::ExecutionQueuePayloadKindMismatch
        );

        if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
            ctx.accounts.queue.load_mut()?.clear_head(sub_queue_idx)?;
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: head_item.sequence,
                kind: reveal.kind,
                status: CommitStatusV5::Failed as u8,
            });
            msg!(
                "{} seq={} reason={:?}",
                terminal_ctm_failure_msg(reason),
                head_item.sequence,
                reason
            );
            continue;
        }

        // Dispatch-account layout check (market-binding + tail-shape). The
        // accounts_hash already bound into commit_hash means substitution is
        // caught above, but the shape check gives cleaner error messages on
        // malformed reveals.
        require_dispatch_market_index(dispatch_accounts, market_index)?;
        validate_queue_payload_dispatch_accounts(group_key, &decoded_payload, dispatch_accounts)?;

        // User-signature for variants that require it.
        if variant_uses_user_signature(decoded_payload.variant) {
            let (mango_account_key, user_owner) =
                extract_user_owner_for_ctm_payload(group_key, dispatch_accounts)?;
            let user_intent_hash_v2 = canonical_user_intent_message_v2(
                group_key,
                mango_account_key,
                user_owner,
                reveal.kind,
                UserIntentTargetKind::PerpMarket,
                market_index,
                &payload_hash,
            );
            verify_user_ed25519_preinstruction(
                ctx.accounts.instructions.as_ref(),
                user_owner,
                &[user_intent_hash_v2],
            )?;
        }

        // Health region wrap + dispatch.
        let item_health_region = queue_health_region_spec(decoded_payload.variant);
        if let Some(spec) = item_health_region {
            if queue_health_region_begin(dispatch_accounts, spec).is_err() {
                let retries = {
                    let mut queue = ctx.accounts.queue.load_mut()?;
                    queue.increment_retry(sub_queue_idx, head_item.sequence, clock.slot)?
                };
                if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                    ctx.accounts.queue.load_mut()?.clear_head(sub_queue_idx)?;
                    emit!(QueueItemProcessed {
                        group: group_key,
                        market_index,
                        sequence: head_item.sequence,
                        kind: reveal.kind,
                        status: CommitStatusV5::Failed as u8,
                    });
                    continue;
                }
                break;
            }
        }

        // Actual CPI. On success, clear head; on failure, increment retry
        // and break (caller resubmits with fresh accounts).
        let queue_key = ctx.accounts.queue.key();
        let queue_bump = { ctx.accounts.queue.load()?.header.bump };
        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            &[],
            group_key,
            queue_key,
            queue_bump,
        );
        let dispatch_result = if let Some(spec) = item_health_region {
            match dispatch_result {
                Ok(()) => queue_health_region_end(dispatch_accounts, spec),
                Err(err) => Err(err),
            }
        } else {
            dispatch_result
        };

        match dispatch_result {
            Ok(_) => {
                ctx.accounts.queue.load_mut()?.clear_head(sub_queue_idx)?;
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: head_item.sequence,
                    kind: reveal.kind,
                    status: CommitStatusV5::Revealed as u8,
                });
            }
            Err(_) => {
                let retries = {
                    let mut queue = ctx.accounts.queue.load_mut()?;
                    queue.increment_retry(sub_queue_idx, head_item.sequence, clock.slot)?
                };
                if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                    ctx.accounts.queue.load_mut()?.clear_head(sub_queue_idx)?;
                    emit!(QueueItemProcessed {
                        group: group_key,
                        market_index,
                        sequence: head_item.sequence,
                        kind: reveal.kind,
                        status: CommitStatusV5::Failed as u8,
                    });
                    continue;
                }
                break;
            }
        }
    }
    Ok(())
}
