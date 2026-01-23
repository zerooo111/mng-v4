use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use super::ed25519_helpers::find_ed25519_signature;

pub fn enqueue_perp_event(
    ctx: Context<EnqueuePerpEvent>,
    args: EnqueuePerpEventArgs,
) -> Result<()> {
    require!(
        args.event_type.to_perp_action().is_some(),
        MangoError::UnsupportedQueueEventType
    );

    let perp_market = ctx.accounts.perp_market.load()?;
    require_eq!(
        perp_market.perp_market_index,
        args.params.market_index,
        MangoError::InvalidQueueParams
    );

    let group = ctx.accounts.group.load()?;
    let continuum = group
        .continuum_key()
        .ok_or(MangoError::ContinuumKeyMissing)?;

    let mut queue = ctx.accounts.queue.load_mut()?;
    if queue.is_full() {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=queue_full",
            args.seq_no
        );
        return err!(MangoError::QueueIsFull);
    }

    let tail_seq = queue.tail_seq();
    if args.seq_no <= tail_seq {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=non_monotonic tail_seq={}",
            args.seq_no,
            tail_seq
        );
        return err!(MangoError::SequenceNumberTooLow);
    }

    let submitted_slot = Clock::get()?.slot;
    let params: CompactOrderParams = args.params.into();
    let mut event = QueueFifoEvent {
        seq_no: args.seq_no,
        submitted_slot,
        user: args.user,
        continuum,
        event_type: args.event_type as u8,
        padding: [0u8; 7],
        params,
        user_signature: SignatureBlob {
            bytes: args.user_signature,
            is_present: 1,
            padding: [0u8; 7],
        },
        continuum_signature: SignatureBlob {
            bytes: args.continuum_signature,
            is_present: 1,
            padding: [0u8; 7],
        },
    };

    let expected_message = event.intent_hash();
    let Some(intent_signature) =
        find_ed25519_signature(&ctx.accounts.instructions, &event.user, &expected_message)?
    else {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=missing_intent_signature",
            args.seq_no
        );
        return err!(MangoError::MissingIntentSignature);
    };

    event.user_signature.bytes = intent_signature;

    let Some(continuum_signature) =
        find_ed25519_signature(&ctx.accounts.instructions, &event.continuum, &event.payload_hash())?
    else {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=missing_continuum_signature",
            args.seq_no
        );
        return err!(MangoError::MissingContinuumSignature);
    };

    event.continuum_signature.bytes = continuum_signature;

    let index = queue.next_index();
    queue.entries[index] = event;
    queue.header.count = queue.header.count.saturating_add(1);

    msg!(
        "enqueue_perp_event: accepted seq_no={} queue_count={} tail_seq={}",
        args.seq_no,
        queue.header.count,
        tail_seq
    );

    Ok(())
}
