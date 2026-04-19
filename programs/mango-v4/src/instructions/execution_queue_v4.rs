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
// Params + canonical-message helpers
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV4MarketRootCreateParams {
    pub page_size: u16,
    pub num_pages: u16,
    pub soft_limit: u16,
    pub gap_wait_slots: u16,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV4MarketRootConfigParams {
    pub soft_limit: u16,
    pub gap_wait_slots: u16,
    pub pause_ingress: bool,
    pub pause_execute: bool,
}

/// One entry in a committed batch. The relayer provides these; the program
/// stores them verbatim and trusts the commit_hash to bind the underlying
/// payload + accounts + kind that the reveal will later present.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CommitEntryV4 {
    pub commit_hash: [u8; 32],
    pub min_execute_slot: u64,
    pub expires_at_slot: u64,
}

/// Reveal side: the relayer presents the real intent data. The program
/// recomputes the commit_hash and rejects any mismatch.
///
/// `dispatch_accounts_count` slices the execute tx's `remaining_accounts` —
/// the first `dispatch_accounts_count` on each iteration are consumed for the
/// corresponding reveal. This lets a single reveal batch mix different
/// mango_accounts without inflating tx size with repeated shared accounts
/// (the caller can still place shared accounts first and reuse them via
/// small dispatch_accounts_count on later reveals).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct RevealArgsV4 {
    pub payload: Vec<u8>,
    pub kind: u8,
    pub dispatch_accounts_count: u8,
}

fn canonical_commit_message(
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

/// Batch message that the relayer's single ed25519 pre-instruction signs.
///
/// Flat hash (not merkle) of (commit_hash_i, min_exec_slot_i, expires_at_i)
/// triples, prefixed with context. 48 bytes per entry, ~50 bytes header.
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

// ---------------------------------------------------------------------------
// Root + page lifecycle helpers
// ---------------------------------------------------------------------------

fn validate_page_geometry_v4(page_size: u16, num_pages: u16, soft_limit: u16) -> Result<u32> {
    require!(page_size > 0, MangoError::ExecutionQueueV3InvalidPageSize);
    require!(
        page_size as usize <= EXECUTION_QUEUE_V4_MAX_PAGE_SIZE,
        MangoError::ExecutionQueueV3InvalidPageSize
    );
    require!(num_pages > 0, MangoError::ExecutionQueueV3InvalidNumPages);
    require!(
        num_pages <= EXECUTION_QUEUE_V4_MAX_NUM_PAGES,
        MangoError::ExecutionQueueV3InvalidNumPages
    );
    let capacity = page_size as u32 * num_pages as u32;
    if soft_limit > 0 {
        require!(
            soft_limit as u32 <= capacity,
            MangoError::ExecutionQueueV3InvalidSoftLimit
        );
    }
    Ok(capacity)
}

fn validate_page_assignment_v4(
    page_slot: u16,
    assigned_abs_page_no: u64,
    num_pages: u16,
) -> Result<()> {
    require!(
        page_slot < num_pages,
        MangoError::ExecutionQueueV3PageSlotOutOfRange
    );
    require!(
        assigned_abs_page_no % num_pages as u64 == page_slot as u64,
        MangoError::ExecutionQueueV3AssignedPageMismatch
    );
    Ok(())
}

fn resize_commit_page_v4<'info>(
    queue_page: &UncheckedAccount<'info>,
    payer: &Signer<'info>,
    system_program: &Program<'info, System>,
) -> Result<()> {
    let queue_page_ai = queue_page.to_account_info();
    let current_len = queue_page_ai.data_len();
    let target_len = EXECUTION_QUEUE_COMMIT_PAGE_V4_SPACE;

    require!(current_len > 0, MangoError::SomeError);
    if current_len >= target_len {
        return Ok(());
    }

    let next_len = (current_len + MAX_PERMITTED_DATA_INCREASE).min(target_len);
    let rent = Rent::get()?;
    let needed_lamports = rent
        .minimum_balance(next_len)
        .saturating_sub(queue_page_ai.lamports());
    if needed_lamports > 0 {
        invoke(
            &system_instruction::transfer(&payer.key(), queue_page_ai.key, needed_lamports),
            &[
                payer.to_account_info(),
                queue_page_ai.clone(),
                system_program.to_account_info(),
            ],
        )?;
    }
    queue_page_ai.realloc(next_len, true)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-reveal execute helpers
// ---------------------------------------------------------------------------

fn clear_market_head_and_advance_v4(
    queue_root: &mut PerpMarketCommitRootV4,
    queue_page: &mut CommitPageV4,
) -> Result<()> {
    let offset = queue_root.head_page_offset();
    queue_page.clear_item(queue_root.page_size, offset)?;
    queue_root.live_count = queue_root.live_count.saturating_sub(1);
    queue_root.next_sequence_to_execute = queue_root.next_sequence_to_execute.saturating_add(1);
    queue_root.gap_observed_slot = 0;
    normalize_market_head_within_page_v4(queue_root, queue_page);
    Ok(())
}

fn normalize_market_head_within_page_v4(
    queue_root: &mut PerpMarketCommitRootV4,
    queue_page: &CommitPageV4,
) {
    while queue_root.live_count > 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
        && queue_root.head_abs_page_no() == queue_page.assigned_abs_page_no
    {
        let offset = queue_root.head_page_offset() as usize;
        let head = &queue_page.items[offset];
        if head.status == CommitStatusV4::Committed as u8
            && head.sequence == queue_root.next_sequence_to_execute
        {
            break;
        }
        queue_root.next_sequence_to_execute =
            queue_root.next_sequence_to_execute.saturating_add(1);
        queue_root.gap_observed_slot = 0;
    }

    if queue_root.live_count == 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
    {
        queue_root.next_sequence_to_execute = queue_root.max_seen_sequence.saturating_add(1);
        queue_root.gap_observed_slot = 0;
    }
}

fn skip_market_head_gaps_v4(
    queue_root: &mut PerpMarketCommitRootV4,
    queue_page: &CommitPageV4,
    group: Pubkey,
    market_index: u16,
) {
    let mut skipped = 0u16;
    while skipped < EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE
        && queue_root.live_count > 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
        && queue_root.head_abs_page_no() == queue_page.assigned_abs_page_no
    {
        let next_seq = queue_root.next_sequence_to_execute;
        let offset = queue_root.head_page_offset() as usize;
        let head = &queue_page.items[offset];
        if head.status == CommitStatusV4::Committed as u8 && head.sequence == next_seq {
            break;
        }
        emit!(QueueItemProcessed {
            group,
            market_index,
            sequence: next_seq,
            kind: QueueItemKind::CtmWrapped as u8,
            status: CommitStatusV4::Skipped as u8,
        });
        queue_root.next_sequence_to_execute = next_seq.saturating_add(1);
        queue_root.gap_observed_slot = 0;
        skipped = skipped.saturating_add(1);
    }

    normalize_market_head_within_page_v4(queue_root, queue_page);
}

// ---------------------------------------------------------------------------
// Root + page admin entrypoints
// ---------------------------------------------------------------------------

pub fn execution_queue_v4_init_market_root(
    ctx: Context<ExecutionQueueV4InitMarketRoot>,
    market_index: u16,
    shard_id: u8,
    params: ExecutionQueueV4MarketRootCreateParams,
) -> Result<()> {
    require!(shard_id == 0, MangoError::ExecutionQueueV3UnsupportedShardId);
    validate_page_geometry_v4(params.page_size, params.num_pages, params.soft_limit)?;

    let bump = *ctx
        .bumps
        .get("queue_root")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    ctx.accounts.queue_root.init(
        ctx.accounts.group.key(),
        ctx.accounts.authority_state.key(),
        market_index,
        shard_id,
        bump,
        params.page_size,
        params.num_pages,
        params.soft_limit,
        params.gap_wait_slots,
    );
    Ok(())
}

pub fn execution_queue_v4_configure_market_root(
    ctx: Context<ExecutionQueueV4MarketRootAdmin>,
    params: ExecutionQueueV4MarketRootConfigParams,
) -> Result<()> {
    let root = &mut ctx.accounts.queue_root;
    validate_page_geometry_v4(root.page_size, root.num_pages, params.soft_limit)?;
    root.soft_limit = params.soft_limit;
    root.gap_wait_slots = params.gap_wait_slots;
    root.paused_ingress = u8::from(params.pause_ingress);
    root.paused_execute = u8::from(params.pause_execute);
    Ok(())
}

pub fn execution_queue_v4_create_market_page(
    _ctx: Context<ExecutionQueueV4CreateMarketPage>,
    _page_slot: u16,
) -> Result<()> {
    Ok(())
}

pub fn execution_queue_v4_resize_market_page(
    ctx: Context<ExecutionQueueV4ResizeMarketPage>,
    _page_slot: u16,
) -> Result<()> {
    resize_commit_page_v4(
        &ctx.accounts.queue_page,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
    )
}

pub fn execution_queue_v4_init_market_page(
    ctx: Context<ExecutionQueueV4InitMarketPage>,
    page_slot: u16,
    assigned_abs_page_no: u64,
) -> Result<()> {
    let root = &ctx.accounts.queue_root;
    validate_page_assignment_v4(page_slot, assigned_abs_page_no, root.num_pages)?;
    let bump = *ctx
        .bumps
        .get("queue_page")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let mut queue_page = ctx.accounts.queue_page.load_init()?;
    queue_page.init(
        ctx.accounts.queue_root.key(),
        page_slot,
        assigned_abs_page_no,
        root.page_size,
        bump,
    );
    Ok(())
}

/// Admin escape hatch: force-advance head by N slots, clearing any
/// committed-but-unrevealable items. Used to recover from upgrades that
/// change the dispatch-account layout (commit_hash no longer matches any
/// recomputable intent). Requires admin + pause_execute=true.
pub fn execution_queue_v4_drop_head_market(
    ctx: Context<ExecutionQueueV4MarketPageAdmin>,
    count: u16,
) -> Result<()> {
    require!(
        ctx.accounts.queue_root.paused_execute != 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );
    let queue_root_key = ctx.accounts.queue_root.key();
    let page_size = ctx.accounts.queue_root.page_size;
    let mut queue_page = ctx.accounts.queue_page.load_mut()?;

    if ctx.accounts.queue_root.live_count == 0 {
        require!(
            queue_page.queue_root == queue_root_key,
            MangoError::ExecutionQueueV3PageQueueRootMismatch
        );
        require!(
            queue_page.page_state == CommitPageStateV4::Active as u8,
            MangoError::ExecutionQueueV3PageInactive
        );
        validate_page_assignment_v4(
            queue_page.page_slot,
            queue_page.assigned_abs_page_no,
            ctx.accounts.queue_root.num_pages,
        )?;
        let repaired = queue_page.admin_repair_orphaned_items(page_size, count)?;
        msg!(
            "drop_head_market: repaired orphan page_slot={} abs_page={} cleared {} items",
            queue_page.page_slot,
            queue_page.assigned_abs_page_no,
            repaired
        );
        return Ok(());
    }

    let mut dropped = 0u16;
    for _ in 0..count {
        if ctx.accounts.queue_root.live_count == 0 {
            break;
        }
        let head_abs_page = ctx.accounts.queue_root.head_abs_page_no();
        let head_slot = ctx.accounts.queue_root.head_page_slot();
        if queue_page.page_slot != head_slot || queue_page.assigned_abs_page_no != head_abs_page {
            break;
        }
        queue_page.validate_target(queue_root_key, head_slot, head_abs_page)?;
        let offset = ctx.accounts.queue_root.head_page_offset();
        queue_page.clear_item(page_size, offset)?;
        ctx.accounts.queue_root.live_count =
            ctx.accounts.queue_root.live_count.saturating_sub(1);
        ctx.accounts.queue_root.next_sequence_to_execute = ctx
            .accounts
            .queue_root
            .next_sequence_to_execute
            .saturating_add(1);
        dropped += 1;
    }
    msg!("drop_head_market: dropped {} items", dropped);
    Ok(())
}

pub fn execution_queue_v4_close_market_page(
    ctx: Context<ExecutionQueueV4CloseMarketPage>,
) -> Result<()> {
    let queue_page = ctx.accounts.queue_page.load()?;
    require!(queue_page.live_count == 0, MangoError::ExecutionQueueFull);
    require!(
        ctx.accounts.queue_root.live_count == 0
            || queue_page.assigned_abs_page_no != ctx.accounts.queue_root.head_abs_page_no(),
        MangoError::ExecutionQueueV3PageInactive
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// commit_market_batch
// ---------------------------------------------------------------------------

pub fn execution_queue_v4_commit_market(
    ctx: Context<ExecutionQueueV4CommitMarket>,
    market_index: u16,
    first_sequence: u64,
    entries: Vec<CommitEntryV4>,
) -> Result<()> {
    require!(
        !entries.is_empty() && entries.len() <= EXECUTION_QUEUE_V4_MAX_COMMIT_BATCH,
        MangoError::ExecutionQueueV4InvalidCommitBatch
    );
    require!(
        ctx.accounts.queue_root.market_index == market_index,
        MangoError::ExecutionQueueSubQueueMarketIndexMismatch
    );
    require!(
        ctx.accounts.queue_root.shard_id == 0,
        MangoError::ExecutionQueueV3UnsupportedShardId
    );

    let clock = Clock::get()?;
    ctx.accounts
        .authority_state
        .maybe_activate_pending_ctm(clock.slot);
    require!(
        ctx.accounts.queue_root.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );

    // Verify the single relayer signature that authorizes the whole batch.
    let batch_msg = canonical_commit_batch_message(
        ctx.accounts.group.key(),
        market_index,
        ctx.accounts.queue_root.shard_id,
        first_sequence,
        &entries,
    );
    verify_ed25519_preinstruction(
        ctx.accounts.instructions.as_ref(),
        ctx.accounts.authority_state.ctm_signer,
        batch_msg.as_ref(),
    )?;

    let queue_root_key = ctx.accounts.queue_root.key();
    let root_page_size = ctx.accounts.queue_root.page_size;

    // Constrain the whole batch to a single page so one queue_page account is
    // enough. The relayer is responsible for splitting across page boundaries.
    let last_sequence = first_sequence
        .checked_add(entries.len() as u64 - 1)
        .ok_or_else(|| error!(MangoError::ExecutionQueueV4InvalidCommitBatch))?;
    let first_abs_page = ctx
        .accounts
        .queue_root
        .abs_page_no_for_sequence(first_sequence);
    let last_abs_page = ctx
        .accounts
        .queue_root
        .abs_page_no_for_sequence(last_sequence);
    require!(
        first_abs_page == last_abs_page,
        MangoError::ExecutionQueueV4InvalidCommitBatch
    );
    let page_slot = ctx
        .accounts
        .queue_root
        .page_slot_for_sequence(first_sequence);

    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.prepare_for_write_target(
            queue_root_key,
            page_slot,
            first_abs_page,
            root_page_size,
        )?;
    }

    for (i, entry) in entries.iter().enumerate() {
        let seq = first_sequence + i as u64;
        ctx.accounts.queue_root.validate_commit_sequence(seq)?;

        // expiry sanity: do not admit commits the executor would immediately
        // cull. min_execute_slot==0 is allowed (=> executable now).
        if entry.expires_at_slot != 0 {
            require!(
                entry.expires_at_slot >= clock.slot,
                MangoError::ExecutionQueueEnvelopeExpired
            );
        }

        let offset = ctx.accounts.queue_root.page_offset_for_sequence(seq);
        let mut item = CommitItemV4::default();
        item.sequence = seq;
        item.commit_hash = entry.commit_hash;
        item.min_execute_slot = entry.min_execute_slot;
        item.expires_at_slot = entry.expires_at_slot;
        item.ingress_slot = clock.slot;
        item.status = CommitStatusV4::Committed as u8;

        {
            let mut queue_page = ctx.accounts.queue_page.load_mut()?;
            queue_page.write_pending_item(root_page_size, offset, item)?;
        }
        ctx.accounts.queue_root.note_commit(seq);
    }

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index,
        sequence: first_sequence,
        kind: QueueItemKind::CtmWrapped as u8,
        min_execute_slot: entries.last().unwrap().min_execute_slot,
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// reveal_execute_market
// ---------------------------------------------------------------------------

pub fn execution_queue_v4_reveal_execute_market(
    ctx: Context<ExecutionQueueV4RevealExecuteMarket>,
    reveals: Vec<RevealArgsV4>,
) -> Result<()> {
    require!(
        !reveals.is_empty() && reveals.len() <= EXECUTION_QUEUE_V4_MAX_REVEAL_BATCH,
        MangoError::ExecutionQueueV4InvalidRevealBatch
    );
    require!(
        ctx.accounts.queue_root.paused_execute == 0,
        MangoError::ExecutionQueueExecutePaused
    );

    let queue_root_key = ctx.accounts.queue_root.key();
    let market_index = ctx.accounts.queue_root.market_index;
    let group_key = ctx.accounts.group.key();
    let clock = Clock::get()?;
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);

    let mut cursor: usize = 0;
    let total_accounts = ctx.remaining_accounts.len();

    for reveal in reveals.iter() {
        if ctx.accounts.queue_root.live_count == 0 {
            break;
        }

        let head_abs_page_no = ctx.accounts.queue_root.head_abs_page_no();
        let head_page_slot = ctx.accounts.queue_root.head_page_slot();
        let next_sequence = ctx.accounts.queue_root.next_sequence_to_execute;
        let page_size = ctx.accounts.queue_root.page_size;
        let gap_wait_slots = ctx.accounts.queue_root.gap_wait_slots as u64;

        // Confirm the provided page is the head page, else stop — caller must
        // resubmit with the right page.
        {
            let queue_page = ctx.accounts.queue_page.load()?;
            require!(
                queue_page.page_state == CommitPageStateV4::Active as u8,
                MangoError::ExecutionQueueV3PageInactive
            );
            if queue_page.page_slot != head_page_slot
                || queue_page.assigned_abs_page_no != head_abs_page_no
            {
                break;
            }
            queue_page.validate_target(queue_root_key, head_page_slot, head_abs_page_no)?;
        }

        // Head lookup + gap handling.
        let head_item = {
            let queue_page = ctx.accounts.queue_page.load()?;
            let offset = (next_sequence % page_size as u64) as usize;
            let item = queue_page.items[offset].clone();
            if item.status == CommitStatusV4::Committed as u8 && item.sequence == next_sequence {
                Some(item)
            } else {
                None
            }
        };
        let Some(head_item) = head_item else {
            if ctx.accounts.queue_root.max_seen_sequence < next_sequence {
                break;
            }
            if ctx.accounts.queue_root.gap_observed_slot == 0 {
                ctx.accounts.queue_root.gap_observed_slot = clock.slot;
            }
            if clock.slot
                >= ctx
                    .accounts
                    .queue_root
                    .gap_observed_slot
                    .saturating_add(gap_wait_slots)
            {
                let queue_page = ctx.accounts.queue_page.load()?;
                skip_market_head_gaps_v4(
                    &mut ctx.accounts.queue_root,
                    &queue_page,
                    group_key,
                    market_index,
                );
                // Gap resolution does not consume the reveal; fall through to
                // re-check head on next iteration.
                continue;
            }
            break;
        };

        if clock.slot < head_item.min_execute_slot {
            break;
        }

        if head_item.expires_at_slot != 0 && clock.slot > head_item.expires_at_slot {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance_v4(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: head_item.sequence,
                kind: QueueItemKind::CtmWrapped as u8,
                status: CommitStatusV4::Failed as u8,
            });
            continue;
        }

        // Slice this reveal's dispatch accounts out of remaining_accounts.
        let count = reveal.dispatch_accounts_count as usize;
        require!(
            count > 0 && cursor + count <= total_accounts,
            MangoError::ExecutionQueueDispatchAccountLayoutInvalid
        );
        let dispatch_accounts = &ctx.remaining_accounts[cursor..cursor + count];
        cursor += count;
        // require_dispatch_market_index is deferred until after the payload
        // pre-validation so that terminal-expired / invalid payloads can be
        // cleared without a real PerpMarket account being present. The
        // accounts_hash is already bound into commit_hash, so any substitution
        // would have been caught by the commit_hash check below.

        // Hashes we recompute to verify the commit.
        let payload_hash = hashv(&[&reveal.payload]).to_bytes();
        let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
        let expected_commit_hash = canonical_commit_message(
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
            MangoError::ExecutionQueueV4CommitRevealMismatch
        );

        // Decode + variant-level pre-validation.
        let decoded_payload = match decode_queue_payload(&reveal.payload) {
            Ok(p) if p.flags == 0 => p,
            _ => {
                {
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    clear_market_head_and_advance_v4(
                        &mut ctx.accounts.queue_root,
                        &mut queue_page,
                    )?;
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: head_item.sequence,
                    kind: reveal.kind,
                    status: CommitStatusV4::Failed as u8,
                });
                continue;
            }
        };
        require!(
            queue_item_kind_for_payload_variant(decoded_payload.variant) == reveal.kind,
            MangoError::ExecutionQueuePayloadKindMismatch
        );

        if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance_v4(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: head_item.sequence,
                kind: reveal.kind,
                status: CommitStatusV4::Failed as u8,
            });
            msg!(
                "{} seq={} reason={:?}",
                terminal_ctm_failure_msg(reason),
                head_item.sequence,
                reason
            );
            continue;
        }

        // Now that we know the payload is non-terminal, verify account layout.
        require_dispatch_market_index(dispatch_accounts, market_index)?;
        validate_queue_payload_dispatch_accounts(group_key, &decoded_payload, dispatch_accounts)?;

        // User-sig check for variants that require it — bound to payload_hash
        // and market, so equivalent to what v3 verified at enqueue time.
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
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    queue_page.increment_retry(
                        page_size,
                        ctx.accounts.queue_root.head_page_offset(),
                        clock.slot,
                    )?
                };
                if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                    {
                        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                        clear_market_head_and_advance_v4(
                            &mut ctx.accounts.queue_root,
                            &mut queue_page,
                        )?;
                    }
                    emit!(QueueItemProcessed {
                        group: group_key,
                        market_index,
                        sequence: head_item.sequence,
                        kind: reveal.kind,
                        status: CommitStatusV4::Failed as u8,
                    });
                    continue;
                }
                break;
            }
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            &[],
            group_key,
            queue_root_key,
            ctx.accounts.queue_root.bump,
        );
        let dispatch_result = if let Some(spec) = item_health_region {
            match dispatch_result {
                Ok(()) => queue_health_region_end(dispatch_accounts, spec),
                Err(err) => Err(err),
            }
        } else {
            dispatch_result
        };

        if dispatch_result.is_ok() {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance_v4(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: head_item.sequence,
                kind: reveal.kind,
                status: CommitStatusV4::Executed as u8,
            });
            continue;
        }

        if item_health_region.is_some() {
            return dispatch_result;
        }

        let retries = {
            let mut queue_page = ctx.accounts.queue_page.load_mut()?;
            queue_page.increment_retry(
                page_size,
                ctx.accounts.queue_root.head_page_offset(),
                clock.slot,
            )?
        };
        if retries >= EXECUTION_QUEUE_MAX_RETRIES {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance_v4(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: head_item.sequence,
                kind: reveal.kind,
                status: CommitStatusV4::Failed as u8,
            });
            continue;
        }
        break;
    }

    Ok(())
}
