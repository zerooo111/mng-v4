use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;

use super::ed25519_helpers::find_two_ed25519_signatures;

pub fn perp_enqueue_operation(
    ctx: Context<PerpEnqueueOperation>,
    args: PerpEnqueueOperationArgs,
) -> Result<()> {
    let order_intent = args.order_intent;
    let queued_order_intent = args.queued_order_intent;

    require!(
        order_intent.event_type.to_perp_action().is_some(),
        MangoError::UnsupportedQueueEventType
    );

    require_keys_eq!(
        order_intent.group,
        ctx.accounts.group.key(),
        MangoError::IntentGroupMismatch
    );
    require_keys_eq!(
        order_intent.account,
        ctx.accounts.account.key(),
        MangoError::IntentAccountMismatch
    );
    require_keys_eq!(
        order_intent.perp_market,
        ctx.accounts.perp_market.key(),
        MangoError::IntentMarketMismatch
    );

    let perp_market = ctx.accounts.perp_market.load()?;
    require_eq!(
        perp_market.perp_market_index,
        order_intent.params.market_index,
        MangoError::IntentMarketMismatch
    );

    let group = ctx.accounts.group.load()?;
    let continuum = group
        .continuum_key()
        .ok_or(MangoError::ContinuumKeyMissing)?;

    let order_intent_hash = order_intent.intent_hash()?;
    require!(
        order_intent_hash == queued_order_intent.order_intent_hash,
        MangoError::OrderIntentHashMismatch
    );

    let now_ts = Clock::get()?.unix_timestamp.max(0) as u64;
    if order_intent.expiry_ts != 0 {
        require!(
            now_ts <= order_intent.expiry_ts,
            MangoError::OrderIntentExpired
        );
    }
    if queued_order_intent.expiry_ts != 0 {
        require!(
            now_ts <= queued_order_intent.expiry_ts,
            MangoError::QueuedOrderIntentExpired
        );
    }

    let mut queue = ctx.accounts.queue.load_mut()?;
    if queue.is_full() {
        msg!(
            "perp_enqueue_operation: rejected seq_no={} reason=queue_full",
            queued_order_intent.seq_no
        );
        return err!(MangoError::QueueIsFull);
    }

    let tail_seq = queue.tail_seq();
    if queued_order_intent.seq_no <= tail_seq {
        msg!(
            "perp_enqueue_operation: rejected seq_no={} reason=non_monotonic tail_seq={}",
            queued_order_intent.seq_no,
            tail_seq
        );
        return err!(MangoError::SequenceNumberTooLow);
    }

    let order_intent_bytes = order_intent.message_bytes()?;
    let queued_order_intent_bytes = queued_order_intent.message_bytes()?;
    let (user_signature, continuum_signature) = find_two_ed25519_signatures(
        &ctx.accounts.instructions,
        &order_intent.owner,
        &order_intent_bytes,
        &continuum,
        &queued_order_intent_bytes,
    )?;

    let Some(user_signature) = user_signature else {
        msg!(
            "perp_enqueue_operation: rejected seq_no={} reason=missing_intent_signature",
            queued_order_intent.seq_no
        );
        return err!(MangoError::MissingIntentSignature);
    };

    let Some(continuum_signature) = continuum_signature else {
        msg!(
            "perp_enqueue_operation: rejected seq_no={} reason=missing_continuum_signature",
            queued_order_intent.seq_no
        );
        return err!(MangoError::MissingContinuumSignature);
    };

    let submitted_slot = Clock::get()?.slot;
    let params: CompactOrderParams = order_intent.params.into();
    let event = QueueFifoEvent {
        seq_no: queued_order_intent.seq_no,
        submitted_slot,
        user: order_intent.owner,
        continuum,
        event_type: order_intent.event_type as u8,
        padding: [0u8; 7],
        params,
        user_signature: SignatureBlob {
            bytes: user_signature,
            is_present: 1,
            padding: [0u8; 7],
        },
        continuum_signature: SignatureBlob {
            bytes: continuum_signature,
            is_present: 1,
            padding: [0u8; 7],
        },
    };

    let index = queue.next_index();
    queue.entries[index] = event;
    queue.header.count = queue.header.count.saturating_add(1);

    msg!(
        "perp_enqueue_operation: accepted seq_no={} queue_count={} tail_seq={}",
        queued_order_intent.seq_no,
        queue.header.count,
        tail_seq
    );

    Ok(())
}
