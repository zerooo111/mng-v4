use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueConfigParams {
    pub gap_wait_slots: u64,
    pub liquidity_delay_slots: u64,
    pub pause_ingress: bool,
    pub pause_execute: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CtmEnvelope {
    pub sequence: u64,
    pub min_execute_slot: u64,
    pub kind: u8,
    pub payload_hash: [u8; 32],
    pub accounts_hash: [u8; 32],
    pub expires_at_slot: u64,
}

#[event]
pub struct QueueItemEnqueued {
    pub group: Pubkey,
    pub sequence: u64,
    pub kind: u8,
    pub min_execute_slot: u64,
}

#[event]
pub struct QueueItemProcessed {
    pub group: Pubkey,
    pub sequence: u64,
    pub kind: u8,
    pub status: u8,
}

fn find_free_slot(queue: &ExecutionQueue) -> Option<usize> {
    queue
        .items
        .iter()
        .position(|x| x.status == QueueItemStatus::Empty as u8)
}

fn queue_is_full(queue: &ExecutionQueue) -> bool {
    queue.count >= queue.capacity
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

fn canonical_envelope_message(group: Pubkey, envelope: &CtmEnvelope) -> [u8; 32] {
    hashv(&[
        b"mango-v4-ctm-envelope-v1",
        group.as_ref(),
        &envelope.sequence.to_le_bytes(),
        &envelope.min_execute_slot.to_le_bytes(),
        &[envelope.kind],
        &envelope.payload_hash,
        &envelope.accounts_hash,
        &envelope.expires_at_slot.to_le_bytes(),
    ])
    .to_bytes()
}

fn verify_ed25519_preinstruction(
    ixs: &AccountInfo,
    ctm_signer: Pubkey,
    msg_hash: [u8; 32],
) -> Result<()> {
    let mut index = 0;
    loop {
        let ix = match tx_instructions::load_instruction_at_checked(index, ixs) {
            Ok(ix) => ix,
            Err(ProgramError::InvalidArgument) => break,
            Err(e) => return Err(e.into()),
        };

        if ix.program_id == ed25519_program::id()
            && ix.data.windows(32).any(|x| x == ctm_signer.as_ref())
            && ix.data.windows(32).any(|x| x == msg_hash)
        {
            return Ok(());
        }
        index += 1;
    }

    err!(MangoError::CtmSignatureMissing)
}

pub fn execution_queue_init(ctx: Context<ExecutionQueueInit>, ctm_signer: Pubkey) -> Result<()> {
    let mut queue = ctx.accounts.execution_queue.load_init()?;
    queue.init(
        ctx.accounts.group.key(),
        ctx.accounts.admin.key(),
        ctm_signer,
        *ctx.bumps
            .get("execution_queue")
            .ok_or_else(|| error!(MangoError::SomeError))?,
    );
    Ok(())
}

pub fn execution_queue_configure(
    ctx: Context<ExecutionQueueAdmin>,
    params: ExecutionQueueConfigParams,
) -> Result<()> {
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.gap_wait_slots = params.gap_wait_slots;
    queue.liquidity_delay_slots = params.liquidity_delay_slots;
    queue.paused_ingress = u8::from(params.pause_ingress);
    queue.paused_execute = u8::from(params.pause_execute);
    Ok(())
}

pub fn execution_queue_set_ctm_pending(
    ctx: Context<ExecutionQueueAdmin>,
    pending_ctm_signer: Pubkey,
    activate_at_slot: u64,
) -> Result<()> {
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.pending_ctm_signer = pending_ctm_signer;
    queue.pending_ctm_activate_slot = activate_at_slot;
    Ok(())
}

pub fn execution_queue_enqueue_ctm(
    ctx: Context<ExecutionQueueEnqueueCtm>,
    envelope: CtmEnvelope,
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.maybe_activate_pending_ctm(clock.slot);

    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    require!(!queue_is_full(&queue), MangoError::ExecutionQueueFull);

    let payload_hash = hashv(&[&payload]).to_bytes();
    require!(
        payload_hash == envelope.payload_hash,
        MangoError::ExecutionQueuePayloadHashMismatch
    );

    let account_hash = hash_accounts(
        &ctx.remaining_accounts
            .iter()
            .map(|ai| AccountMeta {
                pubkey: *ai.key,
                is_signer: ai.is_signer,
                is_writable: ai.is_writable,
            })
            .collect::<Vec<_>>(),
    );
    require!(
        account_hash == envelope.accounts_hash,
        MangoError::ExecutionQueueAccountsHashMismatch
    );

    require!(
        envelope.expires_at_slot == 0 || clock.slot <= envelope.expires_at_slot,
        MangoError::ExecutionQueueEnvelopeExpired
    );
    require!(
        envelope.sequence >= queue.next_sequence_to_execute,
        MangoError::InvalidSequenceNumber
    );
    require!(
        !queue
            .items
            .iter()
            .any(|it| it.status == QueueItemStatus::Pending as u8
                && it.kind == QueueItemKind::CtmWrapped as u8
                && it.sequence == envelope.sequence),
        MangoError::ExecutionQueueDuplicateSequence
    );

    let msg_hash = canonical_envelope_message(ctx.accounts.group.key(), &envelope);
    verify_ed25519_preinstruction(
        ctx.accounts.instructions.as_ref(),
        queue.ctm_signer,
        msg_hash,
    )?;

    let slot = find_free_slot(&queue).ok_or_else(|| error!(MangoError::ExecutionQueueFull))?;
    let item = &mut queue.items[slot];
    *item = QueueItem::default();
    item.sequence = envelope.sequence;
    item.min_execute_slot = envelope.min_execute_slot;
    item.ingress_slot = clock.slot;
    item.kind = envelope.kind;
    item.status = QueueItemStatus::Pending as u8;
    item.payload_len = payload.len() as u16;
    item.payload_hash = envelope.payload_hash;
    item.accounts_hash = envelope.accounts_hash;
    item.payload[..payload.len()].copy_from_slice(&payload);

    queue.count += 1;
    queue.max_seen_sequence = queue.max_seen_sequence.max(envelope.sequence);

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        sequence: envelope.sequence,
        kind: envelope.kind,
        min_execute_slot: envelope.min_execute_slot,
    });

    Ok(())
}

pub fn execution_queue_enqueue_liquidity(
    ctx: Context<ExecutionQueueEnqueueLiquidity>,
    kind: u8,
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );
    require!(
        kind == QueueItemKind::LiquidityDeposit as u8
            || kind == QueueItemKind::LiquidityWithdraw as u8,
        MangoError::ExecutionQueueInvalidItemKind
    );

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    require!(!queue_is_full(&queue), MangoError::ExecutionQueueFull);

    let payload_hash = hashv(&[&payload]).to_bytes();
    let slot = find_free_slot(&queue).ok_or_else(|| error!(MangoError::ExecutionQueueFull))?;
    let min_execute_slot = clock.slot + queue.liquidity_delay_slots;
    {
        let item = &mut queue.items[slot];
        *item = QueueItem::default();
        item.sequence = 0;
        item.min_execute_slot = min_execute_slot;
        item.ingress_slot = clock.slot;
        item.kind = kind;
        item.status = QueueItemStatus::Pending as u8;
        item.payload_len = payload.len() as u16;
        item.payload_hash = payload_hash;
        item.accounts_hash = [0; 32];
        item.payload[..payload.len()].copy_from_slice(&payload);
    }

    queue.count += 1;

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        sequence: 0,
        kind,
        min_execute_slot,
    });

    Ok(())
}

pub fn execution_queue_execute(ctx: Context<ExecutionQueueExecute>, max_items: u16) -> Result<()> {
    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.maybe_activate_pending_ctm(clock.slot);

    require!(
        queue.paused_execute == 0,
        MangoError::ExecutionQueueExecutePaused
    );

    for _ in 0..max_items {
        let next_seq = queue.next_sequence_to_execute;
        let ctm_index = queue.items.iter().position(|it| {
            it.status == QueueItemStatus::Pending as u8
                && it.kind == QueueItemKind::CtmWrapped as u8
                && it.sequence == next_seq
        });

        if let Some(idx) = ctm_index {
            let (sequence, kind, status, min_execute_slot) = {
                let item = &queue.items[idx];
                (
                    item.sequence,
                    item.kind,
                    QueueItemStatus::Executed as u8,
                    item.min_execute_slot,
                )
            };
            if clock.slot < min_execute_slot {
                break;
            }

            {
                let item = &mut queue.items[idx];
                item.status = status;
                *item = QueueItem::default();
            }
            queue.next_sequence_to_execute = queue.next_sequence_to_execute.saturating_add(1);
            queue.gap_observed_slot = 0;
            queue.count = queue.count.saturating_sub(1);
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence,
                kind,
                status,
            });
            continue;
        }

        let can_skip_gap = queue.max_seen_sequence >= next_seq;
        if can_skip_gap {
            if queue.gap_observed_slot == 0 {
                queue.gap_observed_slot = clock.slot;
            }
            if clock.slot >= queue.gap_observed_slot.saturating_add(queue.gap_wait_slots) {
                emit!(QueueItemProcessed {
                    group: ctx.accounts.group.key(),
                    sequence: next_seq,
                    kind: QueueItemKind::CtmWrapped as u8,
                    status: QueueItemStatus::Skipped as u8,
                });
                queue.next_sequence_to_execute = queue.next_sequence_to_execute.saturating_add(1);
                queue.gap_observed_slot = 0;
                continue;
            }
        }

        let liq_index = queue
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.status == QueueItemStatus::Pending as u8
                    && (it.kind == QueueItemKind::LiquidityDeposit as u8
                        || it.kind == QueueItemKind::LiquidityWithdraw as u8)
                    && clock.slot >= it.min_execute_slot
            })
            .min_by_key(|(_, it)| it.ingress_slot)
            .map(|(i, _)| i);

        if let Some(idx) = liq_index {
            let kind = queue.items[idx].kind;
            let status = QueueItemStatus::Executed as u8;
            {
                let item = &mut queue.items[idx];
                item.status = status;
                *item = QueueItem::default();
            }
            queue.count = queue.count.saturating_sub(1);
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: 0,
                kind,
                status,
            });
            continue;
        }

        break;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_message_is_stable() {
        let group = Pubkey::new_unique();
        let env = CtmEnvelope {
            sequence: 42,
            min_execute_slot: 99,
            kind: QueueItemKind::CtmWrapped as u8,
            payload_hash: [7; 32],
            accounts_hash: [9; 32],
            expires_at_slot: 120,
        };

        let one = canonical_envelope_message(group, &env);
        let two = canonical_envelope_message(group, &env);
        assert_eq!(one, two);
    }

    #[test]
    fn accounts_hash_changes_with_order() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let first = hash_accounts(&[
            AccountMeta::new(a, false),
            AccountMeta::new_readonly(b, true),
        ]);
        let second = hash_accounts(&[
            AccountMeta::new_readonly(b, true),
            AccountMeta::new(a, false),
        ]);

        assert_ne!(first, second);
    }
}
