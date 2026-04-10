use crate::accounts_ix::*;
use crate::accounts_zerocopy::*;
use crate::error::*;
use crate::health::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE;
use anchor_lang::solana_program::hash::{hashv, Hasher};
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::{invoke, invoke_signed};
use anchor_lang::solana_program::system_instruction;
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;
use anchor_lang::InstructionData;

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

const ED25519_INSTRUCTION_HEADER_LEN: usize = 2;
const ED25519_SIGNATURE_OFFSETS_LEN: usize = 14;
const ED25519_SIGNATURE_LEN: usize = 64;
const ED25519_PUBKEY_LEN: usize = 32;
const ED25519_CURRENT_INSTRUCTION_INDEX: u16 = u16::MAX;
const QUEUE_PAYLOAD_VERSION_V1: u8 = 1;
const QUEUE_PAYLOAD_HEADER_LEN: usize = 4;
const PERP_BATCH_INTENT_MAX_OPS: usize = 8;
// One retry is sufficient: with the C-1 hash integrity fix, the cranker cannot
// provide wrong accounts to artificially fail dispatch. Non-transient failures
// (expired order, frozen account, paused market) won't resolve on retry.
const EXECUTION_QUEUE_MAX_RETRIES: u8 = 1;
const EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE: u16 = 32;
const DIRECT_SUBMIT_DELAY_SLOTS: u64 = 10;

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuePayloadVariant {
    PerpPlaceOrderV2 = 0,
    PerpCancelOrder = 1,
    PerpCancelOrderByClientOrderId = 2,
    PerpCancelAllOrders = 3,
    PerpCancelAllOrdersBySide = 4,
    LiquidityDeposit = 5,
    LiquidityWithdraw = 6,
    PerpCancelOrderBySlot = 7,
    PerpBatchIntent = 8,
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerpBatchSubOpVariant {
    PerpCancelOrderBySlot = 0,
    PerpPlaceOrderV2 = 1,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpPlaceOrderV2Payload {
    pub side: Side,
    pub price_lots: i64,
    pub max_base_lots: i64,
    pub max_quote_lots: i64,
    pub client_order_id: u64,
    pub order_type: PlaceOrderType,
    pub self_trade_behavior: SelfTradeBehavior,
    pub reduce_only: bool,
    pub expiry_timestamp: u64,
    pub limit: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpCancelOrderPayload {
    pub order_id: u128,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpCancelOrderBySlotPayload {
    pub slot: u8,
    pub expected_order_id: u128,
}

#[derive(Clone, Debug)]
pub enum PerpBatchSubOp {
    PerpCancelOrderBySlot(PerpCancelOrderBySlotPayload),
    PerpPlaceOrderV2(PerpPlaceOrderV2Payload),
}

#[derive(Clone, Debug)]
pub struct PerpBatchIntentPayload {
    pub operations: Vec<PerpBatchSubOp>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpCancelOrderByClientOrderIdPayload {
    pub client_order_id: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpCancelAllOrdersPayload {
    pub limit: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct PerpCancelAllOrdersBySidePayload {
    pub side_option: Option<Side>,
    pub limit: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct LiquidityDepositPayload {
    pub amount: u64,
    pub reduce_only: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct LiquidityWithdrawPayload {
    pub amount: u64,
    pub allow_borrow: bool,
}

#[derive(Clone, Debug)]
enum QueuePayloadBody {
    PerpPlaceOrderV2(PerpPlaceOrderV2Payload),
    PerpCancelOrder(PerpCancelOrderPayload),
    PerpCancelOrderBySlot(PerpCancelOrderBySlotPayload),
    PerpBatchIntent(PerpBatchIntentPayload),
    PerpCancelOrderByClientOrderId(PerpCancelOrderByClientOrderIdPayload),
    PerpCancelAllOrders(PerpCancelAllOrdersPayload),
    PerpCancelAllOrdersBySide(PerpCancelAllOrdersBySidePayload),
    LiquidityDeposit(LiquidityDepositPayload),
    LiquidityWithdraw(LiquidityWithdrawPayload),
}

#[derive(Clone, Debug)]
struct DecodedQueuePayload {
    variant: QueuePayloadVariant,
    flags: u16,
    body: QueuePayloadBody,
}

#[derive(Clone, Debug)]
struct ExecutableCandidate {
    sequence: u64,
    kind: u8,
    payload: Vec<u8>,
    accounts_hash: [u8; 32],
    retries: u8,
    is_ctm: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalCtmFailureReason {
    Expired,
    InvalidNumericInput,
}

#[derive(Clone, Copy, Debug)]
struct Ed25519SignatureOffsets {
    signature_offset: u16,
    signature_instruction_index: u16,
    public_key_offset: u16,
    public_key_instruction_index: u16,
    message_data_offset: u16,
    message_data_size: u16,
    message_instruction_index: u16,
}

struct Ed25519MatchRequest<'a> {
    signer: Pubkey,
    message: &'a [u8],
    found: bool,
}

fn split_dispatch_accounts<'a, 'info>(
    remaining_accounts: &'a [AccountInfo<'info>],
    execution_queue_key: Pubkey,
) -> Result<(&'a [AccountInfo<'info>], &'a [AccountInfo<'info>])> {
    let without_alias = if remaining_accounts
        .last()
        .map(|ai| *ai.key == execution_queue_key)
        .unwrap_or(false)
    {
        &remaining_accounts[..remaining_accounts.len() - 1]
    } else {
        remaining_accounts
    };
    require!(
        !without_alias.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let dispatch_program_ai = without_alias.last();
    if dispatch_program_ai
        .map(|ai| *ai.key == crate::id())
        .unwrap_or(false)
    {
        Ok((&without_alias[..without_alias.len() - 1], without_alias))
    } else {
        Ok((without_alias, without_alias))
    }
}

#[cfg(test)]
fn hash_accounts(accounts: &[AccountMeta]) -> [u8; 32] {
    let mut hasher = Hasher::default();
    for a in accounts {
        hasher.hash(a.pubkey.as_ref());
        hasher.hash(&[u8::from(a.is_signer), u8::from(a.is_writable)]);
    }
    hasher.result().to_bytes()
}

fn hash_account_infos(accounts: &[AccountInfo]) -> [u8; 32] {
    let mut hasher = Hasher::default();
    for ai in accounts {
        hasher.hash(ai.key.as_ref());
        hasher.hash(&[u8::from(ai.is_signer), u8::from(ai.is_writable)]);
    }
    hasher.result().to_bytes()
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

fn canonical_user_intent_message(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    envelope: &CtmEnvelope,
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-user-intent-v1",
        group.as_ref(),
        mango_account.as_ref(),
        user_owner.as_ref(),
        &[envelope.kind],
        &envelope.payload_hash,
        &envelope.accounts_hash,
    ])
    .to_bytes()
}

fn canonical_user_intent_message_hex_utf8(msg_hash: [u8; 32]) -> [u8; 64] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 64];
    let mut i = 0usize;
    while i < 32 {
        let b = msg_hash[i];
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0x0f) as usize];
        i += 1;
    }
    out
}

fn queue_payload_variant_from_u8(value: u8) -> Result<QueuePayloadVariant> {
    match value {
        0 => Ok(QueuePayloadVariant::PerpPlaceOrderV2),
        1 => Ok(QueuePayloadVariant::PerpCancelOrder),
        2 => Ok(QueuePayloadVariant::PerpCancelOrderByClientOrderId),
        3 => Ok(QueuePayloadVariant::PerpCancelAllOrders),
        4 => Ok(QueuePayloadVariant::PerpCancelAllOrdersBySide),
        5 => Ok(QueuePayloadVariant::LiquidityDeposit),
        6 => Ok(QueuePayloadVariant::LiquidityWithdraw),
        7 => Ok(QueuePayloadVariant::PerpCancelOrderBySlot),
        8 => Ok(QueuePayloadVariant::PerpBatchIntent),
        _ => err!(MangoError::ExecutionQueuePayloadVariantInvalid),
    }
}

fn queue_item_kind_for_payload_variant(variant: QueuePayloadVariant) -> u8 {
    match variant {
        QueuePayloadVariant::PerpPlaceOrderV2
        | QueuePayloadVariant::PerpCancelOrder
        | QueuePayloadVariant::PerpCancelOrderBySlot
        | QueuePayloadVariant::PerpBatchIntent
        | QueuePayloadVariant::PerpCancelOrderByClientOrderId
        | QueuePayloadVariant::PerpCancelAllOrders
        | QueuePayloadVariant::PerpCancelAllOrdersBySide => QueueItemKind::CtmWrapped as u8,
        QueuePayloadVariant::LiquidityDeposit => QueueItemKind::LiquidityDeposit as u8,
        QueuePayloadVariant::LiquidityWithdraw => QueueItemKind::LiquidityWithdraw as u8,
    }
}

fn variant_requires_atomic_failure_rollback(variant: QueuePayloadVariant) -> bool {
    matches!(
        variant,
        QueuePayloadVariant::PerpPlaceOrderV2 | QueuePayloadVariant::PerpBatchIntent
    )
}

fn variant_uses_user_signature(variant: QueuePayloadVariant) -> bool {
    matches!(
        variant,
        QueuePayloadVariant::PerpPlaceOrderV2
            | QueuePayloadVariant::PerpCancelOrder
            | QueuePayloadVariant::PerpCancelOrderBySlot
            | QueuePayloadVariant::PerpBatchIntent
            | QueuePayloadVariant::PerpCancelOrderByClientOrderId
            | QueuePayloadVariant::PerpCancelAllOrders
            | QueuePayloadVariant::PerpCancelAllOrdersBySide
    )
}

fn variant_uses_queue_owner_signer(_variant: QueuePayloadVariant) -> bool {
    false
}

fn decode_perp_batch_intent_payload(body: &[u8]) -> Result<PerpBatchIntentPayload> {
    require!(
        !body.is_empty(),
        MangoError::ExecutionQueuePayloadDecodeFailed
    );

    let op_count = body[0] as usize;
    require!(
        op_count > 0 && op_count <= PERP_BATCH_INTENT_MAX_OPS,
        MangoError::ExecutionQueuePayloadDecodeFailed
    );

    let mut remaining = &body[1..];
    let mut operations = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        let op_variant = *remaining
            .first()
            .ok_or_else(|| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?;
        remaining = &remaining[1..];
        match op_variant {
            value if value == PerpBatchSubOpVariant::PerpCancelOrderBySlot as u8 => {
                let op = PerpCancelOrderBySlotPayload::deserialize(&mut remaining)
                    .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?;
                operations.push(PerpBatchSubOp::PerpCancelOrderBySlot(op));
            }
            value if value == PerpBatchSubOpVariant::PerpPlaceOrderV2 as u8 => {
                let op = PerpPlaceOrderV2Payload::deserialize(&mut remaining)
                    .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?;
                operations.push(PerpBatchSubOp::PerpPlaceOrderV2(op));
            }
            _ => return err!(MangoError::ExecutionQueuePayloadDecodeFailed),
        }
    }

    require!(
        remaining.is_empty(),
        MangoError::ExecutionQueuePayloadDecodeFailed
    );

    Ok(PerpBatchIntentPayload { operations })
}

fn decode_payload_body(variant: QueuePayloadVariant, body: &[u8]) -> Result<QueuePayloadBody> {
    let decoded = match variant {
        QueuePayloadVariant::PerpPlaceOrderV2 => QueuePayloadBody::PerpPlaceOrderV2(
            PerpPlaceOrderV2Payload::try_from_slice(body)
                .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
        ),
        QueuePayloadVariant::PerpCancelOrder => QueuePayloadBody::PerpCancelOrder(
            PerpCancelOrderPayload::try_from_slice(body)
                .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
        ),
        QueuePayloadVariant::PerpCancelOrderBySlot => QueuePayloadBody::PerpCancelOrderBySlot(
            PerpCancelOrderBySlotPayload::try_from_slice(body)
                .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
        ),
        QueuePayloadVariant::PerpBatchIntent => {
            QueuePayloadBody::PerpBatchIntent(decode_perp_batch_intent_payload(body)?)
        }
        QueuePayloadVariant::PerpCancelOrderByClientOrderId => {
            QueuePayloadBody::PerpCancelOrderByClientOrderId(
                PerpCancelOrderByClientOrderIdPayload::try_from_slice(body)
                    .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
            )
        }
        QueuePayloadVariant::PerpCancelAllOrders => QueuePayloadBody::PerpCancelAllOrders(
            PerpCancelAllOrdersPayload::try_from_slice(body)
                .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
        ),
        QueuePayloadVariant::PerpCancelAllOrdersBySide => {
            QueuePayloadBody::PerpCancelAllOrdersBySide(
                PerpCancelAllOrdersBySidePayload::try_from_slice(body)
                    .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
            )
        }
        QueuePayloadVariant::LiquidityDeposit => QueuePayloadBody::LiquidityDeposit(
            LiquidityDepositPayload::try_from_slice(body)
                .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
        ),
        QueuePayloadVariant::LiquidityWithdraw => QueuePayloadBody::LiquidityWithdraw(
            LiquidityWithdrawPayload::try_from_slice(body)
                .map_err(|_| error!(MangoError::ExecutionQueuePayloadDecodeFailed))?,
        ),
    };
    Ok(decoded)
}

fn decode_queue_payload(payload: &[u8]) -> Result<DecodedQueuePayload> {
    require!(
        payload.len() >= QUEUE_PAYLOAD_HEADER_LEN,
        MangoError::ExecutionQueuePayloadDecodeFailed
    );

    let version = payload[0];
    require!(
        version == QUEUE_PAYLOAD_VERSION_V1,
        MangoError::ExecutionQueuePayloadVersionUnsupported
    );

    let variant = queue_payload_variant_from_u8(payload[1])?;
    let flags = u16::from_le_bytes([payload[2], payload[3]]);
    require!(flags == 0, MangoError::ExecutionQueuePayloadDecodeFailed);

    let body = decode_payload_body(variant, &payload[QUEUE_PAYLOAD_HEADER_LEN..])?;
    Ok(DecodedQueuePayload {
        variant,
        flags,
        body,
    })
}

fn account_metas_from_infos(account_infos: &[AccountInfo]) -> Vec<AccountMeta> {
    account_infos
        .iter()
        .map(|ai| AccountMeta {
            pubkey: *ai.key,
            is_signer: ai.is_signer,
            is_writable: ai.is_writable,
        })
        .collect()
}

fn build_dispatch_ix_data(payload: &DecodedQueuePayload) -> Vec<u8> {
    match &payload.body {
        QueuePayloadBody::PerpPlaceOrderV2(p) => crate::instruction::PerpPlaceOrderV2 {
            side: p.side,
            price_lots: p.price_lots,
            max_base_lots: p.max_base_lots,
            max_quote_lots: p.max_quote_lots,
            client_order_id: p.client_order_id,
            order_type: p.order_type,
            self_trade_behavior: p.self_trade_behavior,
            reduce_only: p.reduce_only,
            expiry_timestamp: p.expiry_timestamp,
            limit: p.limit,
        }
        .data(),
        QueuePayloadBody::PerpCancelOrder(p) => crate::instruction::PerpCancelOrder {
            order_id: p.order_id,
        }
        .data(),
        QueuePayloadBody::PerpCancelOrderBySlot(_) | QueuePayloadBody::PerpBatchIntent(_) => {
            Vec::new()
        }
        QueuePayloadBody::PerpCancelOrderByClientOrderId(p) => {
            crate::instruction::PerpCancelOrderByClientOrderId {
                client_order_id: p.client_order_id,
            }
            .data()
        }
        QueuePayloadBody::PerpCancelAllOrders(p) => {
            crate::instruction::PerpCancelAllOrders { limit: p.limit }.data()
        }
        QueuePayloadBody::PerpCancelAllOrdersBySide(p) => {
            crate::instruction::PerpCancelAllOrdersBySide {
                side_option: p.side_option,
                limit: p.limit,
            }
            .data()
        }
        QueuePayloadBody::LiquidityDeposit(p) => crate::instruction::TokenDeposit {
            amount: p.amount,
            reduce_only: p.reduce_only,
        }
        .data(),
        QueuePayloadBody::LiquidityWithdraw(p) => crate::instruction::TokenWithdraw {
            amount: p.amount,
            allow_borrow: p.allow_borrow,
        }
        .data(),
    }
}

fn build_perp_place_order_from_queue_payload(payload: &PerpPlaceOrderV2Payload) -> Result<Order> {
    require_gte!(payload.price_lots, 0);

    let time_in_force = match Order::tif_from_expiry(payload.expiry_timestamp) {
        Some(t) => t,
        None => {
            msg!("Order is already expired");
            return err!(MangoError::SomeError);
        }
    };

    Ok(Order {
        side: payload.side,
        max_base_lots: payload.max_base_lots,
        max_quote_lots: payload.max_quote_lots,
        client_order_id: payload.client_order_id,
        reduce_only: payload.reduce_only,
        time_in_force,
        self_trade_behavior: payload.self_trade_behavior,
        params: match payload.order_type {
            PlaceOrderType::Market => OrderParams::Market,
            PlaceOrderType::ImmediateOrCancel => OrderParams::ImmediateOrCancel {
                price_lots: payload.price_lots,
            },
            _ => OrderParams::Fixed {
                price_lots: payload.price_lots,
                order_type: payload.order_type.to_post_order_type()?,
            },
        },
    })
}

fn dispatch_perp_place_order_v2<'info>(
    payload: &PerpPlaceOrderV2Payload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    let order = build_perp_place_order_from_queue_payload(payload)?;
    super::perp_place_order::perp_place_order_from_account_infos(
        dispatch_accounts,
        order,
        payload.limit,
    )?;
    Ok(())
}

fn prevalidate_terminal_ctm_payload(
    payload: &DecodedQueuePayload,
    now_ts: u64,
) -> Option<TerminalCtmFailureReason> {
    match &payload.body {
        QueuePayloadBody::PerpPlaceOrderV2(place) => {
            if place.price_lots < 0 {
                return Some(TerminalCtmFailureReason::InvalidNumericInput);
            }
            if place.expiry_timestamp != 0 && place.expiry_timestamp <= now_ts {
                return Some(TerminalCtmFailureReason::Expired);
            }
            None
        }
        QueuePayloadBody::PerpBatchIntent(batch) => {
            for operation in &batch.operations {
                let PerpBatchSubOp::PerpPlaceOrderV2(place) = operation else {
                    continue;
                };
                if place.price_lots < 0 {
                    return Some(TerminalCtmFailureReason::InvalidNumericInput);
                }
                if place.expiry_timestamp != 0 && place.expiry_timestamp <= now_ts {
                    return Some(TerminalCtmFailureReason::Expired);
                }
            }
            None
        }
        _ => None,
    }
}

fn terminal_ctm_failure_msg(reason: TerminalCtmFailureReason) -> &'static str {
    match reason {
        TerminalCtmFailureReason::Expired => "execution_queue: cleared terminal expired CTM",
        TerminalCtmFailureReason::InvalidNumericInput => {
            "execution_queue: cleared terminal invalid CTM payload"
        }
    }
}

fn dispatch_perp_cancel_all_orders<'info>(
    payload: &PerpCancelAllOrdersPayload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    super::perp_cancel_all_orders::perp_cancel_all_orders_from_account_infos(
        dispatch_accounts,
        payload.limit,
    )?;
    Ok(())
}

fn dispatch_perp_cancel_all_orders_by_side<'info>(
    payload: &PerpCancelAllOrdersBySidePayload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    super::perp_cancel_all_orders_by_side::perp_cancel_all_orders_by_side_from_account_infos(
        dispatch_accounts,
        payload.side_option,
        payload.limit,
    )?;
    Ok(())
}

fn dispatch_perp_cancel_order<'info>(
    payload: &PerpCancelOrderPayload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    super::perp_cancel_order::perp_cancel_order_from_account_infos(
        dispatch_accounts,
        payload.order_id,
    )?;
    Ok(())
}

fn dispatch_perp_cancel_order_by_slot<'info>(
    payload: &PerpCancelOrderBySlotPayload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    require!(
        dispatch_accounts.len() >= 6,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let group = AccountLoader::try_from(&dispatch_accounts[0])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let account = AccountLoader::try_from(&dispatch_accounts[1])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let owner = *dispatch_accounts[2].key;
    let perp_market = AccountLoader::try_from(&dispatch_accounts[3])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let bids = AccountLoader::try_from(&dispatch_accounts[4])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let asks = AccountLoader::try_from(&dispatch_accounts[5])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;

    super::perp_cancel_order::validate_perp_cancel_order_queue_accounts(
        &group,
        &account,
        &perp_market,
        &bids,
        &asks,
        IxGate::PerpCancelOrder,
        true,
    )?;

    let account_pk = account.key();
    let perp_market_index = perp_market.load()?.perp_market_index;
    let mut account = account.load_full_mut()?;
    require!(
        account.fixed.is_owner_or_delegate(owner),
        MangoError::SomeError
    );
    let mut book = Orderbook {
        bids: bids.load_mut()?,
        asks: asks.load_mut()?,
    };

    super::perp_cancel_order::perp_cancel_order_by_slot_with_loaded_context(
        &mut account.borrow_mut(),
        &account_pk,
        &mut book,
        perp_market_index,
        payload.slot,
        payload.expected_order_id,
    )
}

fn dispatch_perp_batch_intent<'info>(
    payload: &PerpBatchIntentPayload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    require!(
        dispatch_accounts.len() >= 8,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    require!(
        !payload.operations.is_empty(),
        MangoError::ExecutionQueuePayloadDecodeFailed
    );

    let group = AccountLoader::try_from(&dispatch_accounts[0])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let account = AccountLoader::try_from(&dispatch_accounts[1])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let owner = *dispatch_accounts[2].key;
    let perp_market = AccountLoader::try_from(&dispatch_accounts[3])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let bids = AccountLoader::try_from(&dispatch_accounts[4])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let asks = AccountLoader::try_from(&dispatch_accounts[5])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let event_queue = AccountLoader::try_from(&dispatch_accounts[6])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let oracle = &dispatch_accounts[7];

    super::perp_place_order::validate_perp_place_order_queue_accounts(
        &group,
        &account,
        &perp_market,
        &bids,
        &asks,
        &event_queue,
        oracle,
    )?;

    let clock = Clock::get()?;
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    let now_slot = clock.slot;
    let group_key = group.key();
    let (sidecar_ai_opt, health_accounts) =
        split_optional_risk_sidecar_account(group_key, account.key(), &dispatch_accounts[8..]);
    let has_place = payload
        .operations
        .iter()
        .any(|op| matches!(op, PerpBatchSubOp::PerpPlaceOrderV2(_)));

    let group_data = group.load()?;
    let buyback_fees_expiry_interval = group_data.buyback_fees_expiry_interval;
    drop(group_data);

    let oracle_price = {
        let mut perp_market = perp_market.load_mut()?;
        let book = Orderbook {
            bids: bids.load_mut()?,
            asks: asks.load_mut()?,
        };

        let oracle_ref = &AccountInfoRef::borrow(oracle)?;
        let oracle_state = perp_market.oracle_state(
            &OracleAccountInfos::from_reader(oracle_ref),
            None,
        )?;
        let oracle_price = oracle_state.price;
        perp_market.update_funding_and_stable_price(&book, &oracle_state, now_ts)?;
        oracle_price
    };

    let (perp_market_index, settle_token_index) = {
        let perp_market = perp_market.load()?;
        (
            perp_market.perp_market_index,
            perp_market.settle_token_index,
        )
    };

    let account_pk = account.key();
    {
        let mut account = account.load_full_mut()?;
        require!(
            account.fixed.is_owner_or_delegate(owner),
            MangoError::SomeError
        );
        account.ensure_perp_position(perp_market_index, settle_token_index)?;
    }

    let account_for_hash = account.load_full()?;
    let account_hash_ref = account_for_hash.borrow();
    let pre_account_state_hash = risk_sidecar_account_state_hash(&account_hash_ref)?;
    let pre_health_accounts_state_hash =
        risk_sidecar_health_accounts_state_hash(&account_hash_ref, health_accounts, now_slot, now_ts)?;
    drop(account_hash_ref);
    drop(account_for_hash);
    let mut sidecar_account_opt =
        load_risk_sidecar_account(group_key, account_pk, sidecar_ai_opt)?;
    let mut account = account.load_full_mut()?;

    let mut pre_health_opt = None;
    if has_place && !account.fixed.is_in_health_region() {
        let mut account_ref = account.borrow_mut();
        let session = if let Some(snapshot) = sidecar_account_opt
            .as_ref()
            .map(|sidecar| {
                matching_risk_sidecar_snapshot(
                    sidecar,
                    group_key,
                    account_pk,
                    pre_account_state_hash,
                    pre_health_accounts_state_hash,
                )
            })
            .transpose()?
            .flatten()
        {
            ExactRiskSidecarSession::prepare_from_snapshot(account_pk, &mut account_ref, &snapshot)
                .context("pre init health from risk sidecar")?
        } else {
            ExactRiskSidecarSession::prepare_from_fixed_accounts(
                account_pk,
                &mut account_ref,
                health_accounts,
                now_slot,
                now_ts,
            )
            .context("pre init health")?
        };
        session.require_token_info(settle_token_index)?;
        pre_health_opt = Some(session);
    }

    {
        let mut perp_market = perp_market.load_mut()?;
        let mut book = Orderbook {
            bids: bids.load_mut()?,
            asks: asks.load_mut()?,
        };
        let mut event_queue = event_queue.load_mut()?;

        for operation in &payload.operations {
            match operation {
                PerpBatchSubOp::PerpCancelOrderBySlot(cancel) => {
                    super::perp_cancel_order::perp_cancel_order_by_slot_with_loaded_context(
                        &mut account.borrow_mut(),
                        &account_pk,
                        &mut book,
                        perp_market_index,
                        cancel.slot,
                        cancel.expected_order_id,
                    )?;
                }
                PerpBatchSubOp::PerpPlaceOrderV2(place) => {
                    let order = build_perp_place_order_from_queue_payload(place)?;
                    super::perp_place_order::perp_place_order_with_loaded_context(
                        &mut account.borrow_mut(),
                        &account_pk,
                        &mut perp_market,
                        &mut book,
                        &mut event_queue,
                        oracle_price,
                        order,
                        now_ts,
                        buyback_fees_expiry_interval,
                        place.limit,
                    )?;
                }
            }
        }

        if let Some(risk_session) = pre_health_opt.as_mut() {
            let mut account_ref = account.borrow_mut();
            risk_session.recompute_perp_and_check_post(&mut account_ref, &perp_market)?;
        }
    }

    let snapshot_opt = if let Some(risk_session) = pre_health_opt.take() {
        Some(risk_session.into_snapshot(&account.borrow()))
    } else {
        None
    };
    drop(account);

    if let (Some(snapshot), Some(sidecar_account)) = (snapshot_opt.as_ref(), sidecar_account_opt.as_mut()) {
        if sidecar_account.to_account_info().is_writable {
            let account_loader = AccountLoader::try_from(&dispatch_accounts[1])
                .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
            let account_for_hash = account_loader.load_full()?;
            let account_hash_ref = account_for_hash.borrow();
            let post_account_state_hash = risk_sidecar_account_state_hash(&account_hash_ref)?;
            let post_health_accounts_state_hash =
                risk_sidecar_health_accounts_state_hash(&account_hash_ref, health_accounts, now_slot, now_ts)?;
            refresh_risk_sidecar_account(
                sidecar_account,
                snapshot,
                post_account_state_hash,
                post_health_accounts_state_hash,
                now_slot,
                now_ts,
                false,
            )?;
        }
    }

    Ok(())
}

fn dispatch_perp_cancel_order_by_client_order_id<'info>(
    payload: &PerpCancelOrderByClientOrderIdPayload,
    dispatch_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    super::perp_cancel_order_by_client_order_id::perp_cancel_order_by_client_order_id_from_account_infos(
        dispatch_accounts,
        payload.client_order_id,
    )?;
    Ok(())
}

fn dispatch_queue_payload(
    payload: &DecodedQueuePayload,
    dispatch_accounts: &[AccountInfo],
    invoke_accounts: &[AccountInfo],
    group_key: Pubkey,
    execution_queue_key: Pubkey,
    execution_queue_bump: u8,
) -> Result<()> {
    match &payload.body {
        QueuePayloadBody::PerpPlaceOrderV2(place) => {
            return dispatch_perp_place_order_v2(place, dispatch_accounts);
        }
        QueuePayloadBody::PerpCancelOrder(cancel) => {
            return dispatch_perp_cancel_order(cancel, dispatch_accounts);
        }
        QueuePayloadBody::PerpCancelOrderBySlot(cancel) => {
            return dispatch_perp_cancel_order_by_slot(cancel, dispatch_accounts);
        }
        QueuePayloadBody::PerpBatchIntent(batch) => {
            return dispatch_perp_batch_intent(batch, dispatch_accounts);
        }
        QueuePayloadBody::PerpCancelOrderByClientOrderId(cancel) => {
            return dispatch_perp_cancel_order_by_client_order_id(cancel, dispatch_accounts);
        }
        QueuePayloadBody::PerpCancelAllOrders(cancel) => {
            return dispatch_perp_cancel_all_orders(cancel, dispatch_accounts);
        }
        QueuePayloadBody::PerpCancelAllOrdersBySide(cancel) => {
            return dispatch_perp_cancel_all_orders_by_side(cancel, dispatch_accounts);
        }
        _ => {}
    }

    require!(
        invoke_accounts
            .last()
            .map(|ai| *ai.key == crate::id())
            .unwrap_or(false),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let mut metas = account_metas_from_infos(dispatch_accounts);
    if variant_uses_queue_owner_signer(payload.variant) {
        require!(
            metas.len() >= 3,
            MangoError::ExecutionQueueDispatchAccountLayoutInvalid
        );
        require!(
            metas[2].pubkey == execution_queue_key,
            MangoError::ExecutionQueueOwnerMustBeQueueAuthority
        );
        metas[2].is_signer = true;
    }

    let dispatch_ix = Instruction {
        program_id: crate::id(),
        accounts: metas,
        data: build_dispatch_ix_data(payload),
    };

    if variant_uses_queue_owner_signer(payload.variant) {
        let seeds: &[&[u8]] = &[
            b"ExecutionQueue".as_ref(),
            group_key.as_ref(),
            &[execution_queue_bump],
        ];
        invoke_signed(&dispatch_ix, invoke_accounts, &[seeds])
            .map_err(|_| error!(MangoError::ExecutionQueueDispatchFailed))
    } else {
        invoke(&dispatch_ix, invoke_accounts)
            .map_err(|_| error!(MangoError::ExecutionQueueDispatchFailed))
    }
}

fn extract_user_owner_for_ctm_payload(
    group_key: Pubkey,
    remaining_accounts: &[AccountInfo],
) -> Result<(Pubkey, Pubkey)> {
    require!(
        remaining_accounts.len() >= 2,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let mango_account_ai = &remaining_accounts[1];
    let loader: AccountLoader<MangoAccountFixed> = AccountLoader::try_from(mango_account_ai)
        .map_err(|_| error!(MangoError::ExecutionQueueInvalidUserAccount))?;
    let account = loader
        .load()
        .map_err(|_| error!(MangoError::ExecutionQueueInvalidUserAccount))?;
    require!(
        account.group == group_key,
        MangoError::ExecutionQueueInvalidUserAccount
    );
    Ok((*mango_account_ai.key, account.owner))
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn parse_ed25519_signature_offsets(
    data: &[u8],
    signature_index: usize,
) -> Option<Ed25519SignatureOffsets> {
    let start = ED25519_INSTRUCTION_HEADER_LEN + signature_index * ED25519_SIGNATURE_OFFSETS_LEN;
    Some(Ed25519SignatureOffsets {
        signature_offset: read_u16(data, start)?,
        signature_instruction_index: read_u16(data, start + 2)?,
        public_key_offset: read_u16(data, start + 4)?,
        public_key_instruction_index: read_u16(data, start + 6)?,
        message_data_offset: read_u16(data, start + 8)?,
        message_data_size: read_u16(data, start + 10)?,
        message_instruction_index: read_u16(data, start + 12)?,
    })
}

fn extract_ix_data<'a>(
    ix: &'a Instruction,
    expected_ix_index: usize,
    ix_index: u16,
) -> Option<&'a [u8]> {
    if ix_index == ED25519_CURRENT_INSTRUCTION_INDEX || ix_index as usize == expected_ix_index {
        Some(ix.data.as_slice())
    } else {
        None
    }
}

fn extract_bytes<'a>(
    ix: &'a Instruction,
    expected_ix_index: usize,
    ix_index: u16,
    offset: usize,
    len: usize,
) -> Option<&'a [u8]> {
    let data = extract_ix_data(ix, expected_ix_index, ix_index)?;
    data.get(offset..offset + len)
}

fn all_ed25519_requests_found(requests: &[Ed25519MatchRequest]) -> bool {
    requests.iter().all(|request| request.found)
}

fn record_ed25519_ix_matches(
    ix: &Instruction,
    ix_index: usize,
    requests: &mut [Ed25519MatchRequest],
) {
    if ix.program_id != ed25519_program::id() || ix.data.len() < ED25519_INSTRUCTION_HEADER_LEN {
        return;
    }

    let signature_count = ix.data[0] as usize;
    if signature_count == 0 {
        return;
    }

    let min_len = ED25519_INSTRUCTION_HEADER_LEN + signature_count * ED25519_SIGNATURE_OFFSETS_LEN;
    if ix.data.len() < min_len {
        return;
    }

    for signature_index in 0..signature_count {
        let Some(offsets) = parse_ed25519_signature_offsets(&ix.data, signature_index) else {
            continue;
        };

        if extract_bytes(
            ix,
            ix_index,
            offsets.signature_instruction_index,
            offsets.signature_offset as usize,
            ED25519_SIGNATURE_LEN,
        )
        .is_none()
        {
            continue;
        }

        let Some(public_key_bytes) = extract_bytes(
            ix,
            ix_index,
            offsets.public_key_instruction_index,
            offsets.public_key_offset as usize,
            ED25519_PUBKEY_LEN,
        ) else {
            continue;
        };

        let Some(message_bytes) = extract_bytes(
            ix,
            ix_index,
            offsets.message_instruction_index,
            offsets.message_data_offset as usize,
            offsets.message_data_size as usize,
        ) else {
            continue;
        };

        for request in requests.iter_mut().filter(|request| !request.found) {
            if public_key_bytes == request.signer.as_ref() && message_bytes == request.message {
                request.found = true;
            }
        }

        if all_ed25519_requests_found(requests) {
            return;
        }
    }
}

fn scan_ed25519_preinstructions(
    ixs: &AccountInfo,
    requests: &mut [Ed25519MatchRequest],
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let current_index = tx_instructions::load_current_index_checked(ixs)? as usize;
    for index in 0..current_index {
        let ix = tx_instructions::load_instruction_at_checked(index, ixs)?;
        record_ed25519_ix_matches(&ix, index, requests);
        if all_ed25519_requests_found(requests) {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
fn ed25519_ix_matches(ix: &Instruction, ix_index: usize, signer: Pubkey, message: &[u8]) -> bool {
    let mut requests = [Ed25519MatchRequest {
        signer,
        message,
        found: false,
    }];
    record_ed25519_ix_matches(ix, ix_index, &mut requests);
    requests[0].found
}

pub fn execution_queue_create(_ctx: Context<ExecutionQueueCreate>) -> Result<()> {
    Ok(())
}

pub fn execution_queue_resize(ctx: Context<ExecutionQueueResize>) -> Result<()> {
    let execution_queue_ai = ctx.accounts.execution_queue.to_account_info();
    let current_len = execution_queue_ai.data_len();
    let target_len = EXECUTION_QUEUE_ACCOUNT_SPACE;

    require!(current_len > 0, MangoError::SomeError);
    if current_len >= target_len {
        return Ok(());
    }

    let next_len = (current_len + MAX_PERMITTED_DATA_INCREASE).min(target_len);
    let rent = Rent::get()?;
    let needed_lamports = rent
        .minimum_balance(next_len)
        .saturating_sub(execution_queue_ai.lamports());
    if needed_lamports > 0 {
        invoke(
            &system_instruction::transfer(
                &ctx.accounts.payer.key(),
                execution_queue_ai.key,
                needed_lamports,
            ),
            &[
                ctx.accounts.payer.to_account_info(),
                execution_queue_ai.clone(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;
    }

    execution_queue_ai.realloc(next_len, true)?;
    Ok(())
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
    queue.header.gap_wait_slots = params.gap_wait_slots;
    queue.header.liquidity_delay_slots = params.liquidity_delay_slots;
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

pub fn execution_queue_drop_ctm(ctx: Context<ExecutionQueueAdmin>, sequence: u64) -> Result<()> {
    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.maybe_activate_pending_ctm(clock.slot);
    require!(
        queue.paused_execute != 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );

    let item = *queue.ctm_item(sequence);
    require!(
        item.status == QueueItemStatus::Pending as u8 && item.sequence == sequence,
        MangoError::ExecutionQueueSequenceNotPending
    );

    queue.clear_ctm_item_at(sequence);
    emit!(QueueItemProcessed {
        group: ctx.accounts.group.key(),
        sequence,
        kind: item.kind,
        status: QueueItemStatus::Failed as u8,
    });
    Ok(())
}

pub fn execution_queue_enqueue_ctm(
    ctx: Context<ExecutionQueueEnqueueCtm>,
    envelope: CtmEnvelope,
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        envelope.kind == QueueItemKind::CtmWrapped as u8,
        MangoError::ExecutionQueueInvalidItemKind
    );
    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );

    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (dispatch_accounts, _) =
        split_dispatch_accounts(ctx.remaining_accounts, ctx.accounts.execution_queue.key())?;

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.maybe_activate_pending_ctm(clock.slot);

    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );

    let payload_hash = hashv(&[&payload]).to_bytes();
    require!(
        payload_hash == envelope.payload_hash,
        MangoError::ExecutionQueuePayloadHashMismatch
    );
    let decoded_payload = decode_queue_payload(&payload)?;
    require!(
        decoded_payload.flags == 0,
        MangoError::ExecutionQueuePayloadDecodeFailed
    );
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    require!(
        prevalidate_terminal_ctm_payload(&decoded_payload, now_ts).is_none(),
        MangoError::SomeError
    );
    require!(
        queue_item_kind_for_payload_variant(decoded_payload.variant)
            == QueueItemKind::CtmWrapped as u8,
        MangoError::ExecutionQueuePayloadKindMismatch
    );
    if variant_uses_queue_owner_signer(decoded_payload.variant) {
        require!(
            dispatch_accounts.len() >= 3,
            MangoError::ExecutionQueueDispatchAccountLayoutInvalid
        );
        require!(
            *dispatch_accounts[2].key == ctx.accounts.execution_queue.key(),
            MangoError::ExecutionQueueOwnerMustBeQueueAuthority
        );
    }

    let account_hash = hash_account_infos(dispatch_accounts);
    require!(
        account_hash == envelope.accounts_hash,
        MangoError::ExecutionQueueAccountsHashMismatch
    );

    require!(
        envelope.expires_at_slot == 0 || clock.slot <= envelope.expires_at_slot,
        MangoError::ExecutionQueueEnvelopeExpired
    );
    require!(
        envelope.sequence >= queue.header.next_sequence_to_execute,
        MangoError::InvalidSequenceNumber
    );

    let uses_user_signature = variant_uses_user_signature(decoded_payload.variant);
    let ctm_message = canonical_envelope_message(ctx.accounts.group.key(), &envelope);
    let user_intent_messages = if uses_user_signature {
        let (mango_account_key, user_owner) =
            extract_user_owner_for_ctm_payload(ctx.accounts.group.key(), dispatch_accounts)?;
        let user_intent_hash = canonical_user_intent_message(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            &envelope,
        );
        let user_intent_hex_utf8 = canonical_user_intent_message_hex_utf8(user_intent_hash);
        Some((user_owner, user_intent_hash, user_intent_hex_utf8))
    } else {
        None
    };
    let mut signature_requests = Vec::with_capacity(if uses_user_signature { 3 } else { 1 });
    signature_requests.push(Ed25519MatchRequest {
        signer: queue.ctm_signer,
        message: ctm_message.as_ref(),
        found: false,
    });
    if let Some((user_owner, user_intent_hash, user_intent_hex_utf8)) =
        user_intent_messages.as_ref()
    {
        signature_requests.push(Ed25519MatchRequest {
            signer: *user_owner,
            message: user_intent_hash.as_ref(),
            found: false,
        });
        signature_requests.push(Ed25519MatchRequest {
            signer: *user_owner,
            message: user_intent_hex_utf8.as_ref(),
            found: false,
        });
    }
    scan_ed25519_preinstructions(ctx.accounts.instructions.as_ref(), &mut signature_requests)?;
    require!(signature_requests[0].found, MangoError::CtmSignatureMissing);
    if uses_user_signature {
        require!(
            signature_requests[1].found || signature_requests[2].found,
            MangoError::ExecutionQueueUserSignatureMissing
        );
    }

    let mut item = QueueItem::default();
    item.sequence = envelope.sequence;
    item.min_execute_slot = envelope.min_execute_slot;
    item.ingress_slot = clock.slot;
    item.kind = envelope.kind;
    item.status = QueueItemStatus::Pending as u8;
    item.payload_len = payload.len() as u16;
    item.payload_hash = envelope.payload_hash;
    item.accounts_hash = envelope.accounts_hash;
    item.payload[..payload.len()].copy_from_slice(&payload);

    queue.push_ctm(item)?;
    queue.header.max_seen_sequence = queue.header.max_seen_sequence.max(envelope.sequence);

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        sequence: envelope.sequence,
        kind: envelope.kind,
        min_execute_slot: envelope.min_execute_slot,
    });

    Ok(())
}

/// C-4 fix: Direct-submit fallback for liveness.
/// Allows users to enqueue intents without the CTM co-signature, but with a
/// mandatory 10-slot delayed execution to prevent race conditions with properly
/// sequenced relayer transactions.
pub fn execution_queue_enqueue_direct(
    ctx: Context<ExecutionQueueEnqueueCtm>,
    envelope: CtmEnvelope,
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        envelope.kind == QueueItemKind::CtmWrapped as u8,
        MangoError::ExecutionQueueInvalidItemKind
    );
    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );

    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (dispatch_accounts, _) =
        split_dispatch_accounts(ctx.remaining_accounts, ctx.accounts.execution_queue.key())?;

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.maybe_activate_pending_ctm(clock.slot);

    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );

    let payload_hash = hashv(&[&payload]).to_bytes();
    require!(
        payload_hash == envelope.payload_hash,
        MangoError::ExecutionQueuePayloadHashMismatch
    );
    let decoded_payload = decode_queue_payload(&payload)?;
    require!(
        decoded_payload.flags == 0,
        MangoError::ExecutionQueuePayloadDecodeFailed
    );
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    require!(
        prevalidate_terminal_ctm_payload(&decoded_payload, now_ts).is_none(),
        MangoError::SomeError
    );
    require!(
        queue_item_kind_for_payload_variant(decoded_payload.variant)
            == QueueItemKind::CtmWrapped as u8,
        MangoError::ExecutionQueuePayloadKindMismatch
    );

    let account_hash = hash_account_infos(dispatch_accounts);
    require!(
        account_hash == envelope.accounts_hash,
        MangoError::ExecutionQueueAccountsHashMismatch
    );

    require!(
        envelope.expires_at_slot == 0 || clock.slot <= envelope.expires_at_slot,
        MangoError::ExecutionQueueEnvelopeExpired
    );

    // Direct-submit: NO CTM signer verification required.
    // Instead, verify only the user's Ed25519 intent signature.
    if variant_uses_user_signature(decoded_payload.variant) {
        let (mango_account_key, user_owner) =
            extract_user_owner_for_ctm_payload(ctx.accounts.group.key(), dispatch_accounts)?;
        let user_intent_hash = canonical_user_intent_message(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            &envelope,
        );
        let user_intent_hex_utf8 = canonical_user_intent_message_hex_utf8(user_intent_hash);
        let mut signature_requests = vec![
            Ed25519MatchRequest {
                signer: user_owner,
                message: user_intent_hash.as_ref(),
                found: false,
            },
            Ed25519MatchRequest {
                signer: user_owner,
                message: user_intent_hex_utf8.as_ref(),
                found: false,
            },
        ];
        scan_ed25519_preinstructions(ctx.accounts.instructions.as_ref(), &mut signature_requests)?;
        require!(
            signature_requests[0].found || signature_requests[1].found,
            MangoError::ExecutionQueueUserSignatureMissing
        );
    }

    // Assign sequence: use max_seen_sequence + 1 to avoid collisions with relayer sequences
    let sequence = queue.header.max_seen_sequence.saturating_add(1);
    require!(
        queue.can_enqueue_ctm_sequence(sequence),
        MangoError::ExecutionQueueFull
    );

    // Force minimum execution delay of DIRECT_SUBMIT_DELAY_SLOTS
    let forced_min_execute_slot = clock.slot.saturating_add(DIRECT_SUBMIT_DELAY_SLOTS);
    let min_execute_slot = envelope.min_execute_slot.max(forced_min_execute_slot);

    let mut item = QueueItem::default();
    item.sequence = sequence;
    item.min_execute_slot = min_execute_slot;
    item.ingress_slot = clock.slot;
    item.kind = envelope.kind;
    item.status = QueueItemStatus::Pending as u8;
    item.payload_len = payload.len() as u16;
    item.payload_hash = payload_hash;
    item.accounts_hash = account_hash;
    item.payload[..payload.len()].copy_from_slice(&payload);

    queue.push_ctm(item)?;
    if sequence > queue.header.max_seen_sequence {
        queue.header.max_seen_sequence = sequence;
    }

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        sequence,
        kind: envelope.kind,
        min_execute_slot,
    });

    Ok(())
}

pub fn execution_queue_enqueue_liquidity(
    ctx: Context<ExecutionQueueEnqueueLiquidity>,
    kind: u8,
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (dispatch_accounts, _) =
        split_dispatch_accounts(ctx.remaining_accounts, ctx.accounts.execution_queue.key())?;

    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );
    require!(
        kind == QueueItemKind::LiquidityDeposit as u8
            || kind == QueueItemKind::LiquidityWithdraw as u8,
        MangoError::ExecutionQueueInvalidItemKind
    );
    let decoded_payload = decode_queue_payload(&payload)?;
    require!(
        decoded_payload.flags == 0,
        MangoError::ExecutionQueuePayloadDecodeFailed
    );
    require!(
        queue_item_kind_for_payload_variant(decoded_payload.variant) == kind,
        MangoError::ExecutionQueuePayloadKindMismatch
    );

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );

    let payload_hash = hashv(&[&payload]).to_bytes();
    let accounts_hash = hash_account_infos(dispatch_accounts);
    let min_execute_slot = clock.slot + queue.header.liquidity_delay_slots;
    let mut item = QueueItem::default();
    item.sequence = 0;
    item.min_execute_slot = min_execute_slot;
    item.ingress_slot = clock.slot;
    item.kind = kind;
    item.status = QueueItemStatus::Pending as u8;
    item.payload_len = payload.len() as u16;
    item.payload_hash = payload_hash;
    item.accounts_hash = accounts_hash;
    item.payload[..payload.len()].copy_from_slice(&payload);

    queue.push_liquidity(item)?;

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        sequence: 0,
        kind,
        min_execute_slot,
    });

    Ok(())
}

pub fn execution_queue_execute(ctx: Context<ExecutionQueueExecute>, max_items: u16) -> Result<()> {
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (dispatch_accounts, invoke_accounts) =
        split_dispatch_accounts(ctx.remaining_accounts, ctx.accounts.execution_queue.key())?;

    let clock = Clock::get()?;
    let group_key = ctx.accounts.group.key();
    let execution_queue_key = ctx.accounts.execution_queue.key();
    let execution_queue_bump: u8;
    {
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        execution_queue_bump = queue.bump;
        queue.maybe_activate_pending_ctm(clock.slot);
        require!(
            queue.paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
    }

    let provided_accounts_hash = hash_account_infos(dispatch_accounts);
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    for _ in 0..max_items {
        let mut candidate: Option<ExecutableCandidate> = None;
        let mut blocked = false;

        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if let Some(item) = queue.current_ctm_head().copied() {
                if clock.slot < item.min_execute_slot {
                    blocked = true;
                } else {
                    let payload_len = item.payload_len as usize;
                    candidate = Some(ExecutableCandidate {
                        sequence: item.sequence,
                        kind: item.kind,
                        payload: item.payload[..payload_len].to_vec(),
                        accounts_hash: item.accounts_hash,
                        retries: item.retries,
                        is_ctm: true,
                    });
                }
            } else if queue.header.ctm_count > 0
                && queue.header.max_seen_sequence >= queue.header.next_sequence_to_execute
            {
                if queue.header.gap_observed_slot == 0 {
                    queue.header.gap_observed_slot = clock.slot;
                }
                if clock.slot
                    >= queue
                        .header
                        .gap_observed_slot
                        .saturating_add(queue.header.gap_wait_slots)
                {
                    let mut skipped = 0u16;
                    while skipped < EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE
                        && queue.header.ctm_count > 0
                        && queue.header.max_seen_sequence >= queue.header.next_sequence_to_execute
                        && queue.current_ctm_head().is_none()
                    {
                        emit!(QueueItemProcessed {
                            group: ctx.accounts.group.key(),
                            sequence: queue.header.next_sequence_to_execute,
                            kind: QueueItemKind::CtmWrapped as u8,
                            status: QueueItemStatus::Skipped as u8,
                        });
                        queue.header.next_sequence_to_execute =
                            queue.header.next_sequence_to_execute.saturating_add(1);
                        skipped = skipped.saturating_add(1);
                    }
                    queue.header.gap_observed_slot = 0;
                    continue;
                }
                blocked = true;
            } else if let Some(item) = queue.liquidity_head_item().copied() {
                if item.status != QueueItemStatus::Pending as u8 {
                    let _ = queue.pop_liquidity_head();
                    continue;
                }
                if clock.slot < item.min_execute_slot {
                    blocked = true;
                } else {
                    let payload_len = item.payload_len as usize;
                    candidate = Some(ExecutableCandidate {
                        sequence: item.sequence,
                        kind: item.kind,
                        payload: item.payload[..payload_len].to_vec(),
                        accounts_hash: item.accounts_hash,
                        retries: item.retries,
                        is_ctm: false,
                    });
                }
            }
        }

        if blocked {
            break;
        }

        let Some(candidate) = candidate else {
            break;
        };

        // H-8 fix: Strict head-only FIFO — if hash doesn't match, stop.
        if candidate.accounts_hash != [0; 32] && provided_accounts_hash != candidate.accounts_hash {
            break;
        }

        // Payload hash was verified at enqueue time; skip redundant re-hash.
        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(p) => p,
            Err(_) => {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                if candidate.is_ctm {
                    queue.clear_ctm_item_at(candidate.sequence);
                } else {
                    let _ = queue.pop_liquidity_head();
                }
                emit!(QueueItemProcessed {
                    group: ctx.accounts.group.key(),
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                });
                continue;
            }
        };

        let payload_kind = queue_item_kind_for_payload_variant(decoded_payload.variant);
        if payload_kind != candidate.kind {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(candidate.sequence);
            } else {
                let _ = queue.pop_liquidity_head();
            }
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            continue;
        }

        let requires_atomic_failure_rollback =
            variant_requires_atomic_failure_rollback(decoded_payload.variant);

        if candidate.is_ctm {
            if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                queue.clear_ctm_item_at(candidate.sequence);
                emit!(QueueItemProcessed {
                    group: ctx.accounts.group.key(),
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                });
                msg!(
                    "{} seq={} reason={:?}",
                    terminal_ctm_failure_msg(reason),
                    candidate.sequence,
                    reason
                );
                continue;
            }
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            invoke_accounts,
            group_key,
            execution_queue_key,
            execution_queue_bump,
        );

        if dispatch_result.is_ok() {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(candidate.sequence);
            } else {
                let _ = queue.pop_liquidity_head();
            }
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Executed as u8,
            });
            continue;
        }

        // Dispatch failed. Handle retries and clearing.
        if candidate.is_ctm {
            if requires_atomic_failure_rollback {
                // Health-gated items (PerpPlaceOrderV2) mutate perp book state
                // directly before the post-check, so a failed dispatch must roll
                // back the whole tx to preserve orderbook/account atomicity.
                return dispatch_result;
            }
            // Non-health-gated CTM (cancel orders, etc.): retry up to max, then discard.
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let retries = queue.increment_ctm_retry(candidate.sequence, clock.slot);
            if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                queue.clear_ctm_item_at(candidate.sequence);
                emit!(QueueItemProcessed {
                    group: ctx.accounts.group.key(),
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                });
                continue;
            }
            // Item stays in queue with incremented retry; stop this execute call.
            break;
        }

        // Liquidity item failure: retry up to max, then discard.
        let next_retry = candidate.retries.saturating_add(1);
        if next_retry >= EXECUTION_QUEUE_MAX_RETRIES {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let _ = queue.pop_liquidity_head();
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            continue;
        }

        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.rotate_liquidity_head_with_retry(clock.slot, next_retry, 1)?;
        break;
    }

    Ok(())
}

/// Multi-lane execute: process items from multiple lane account sets in one tx.
/// `lane_count` account groups of `accounts_per_lane` accounts each are packed
/// into remaining_accounts. The function pre-hashes each group and matches
/// queue items against any lane, switching health regions as needed.
pub fn execution_queue_execute_multi(
    ctx: Context<ExecutionQueueExecute>,
    max_items: u16,
    lane_count: u8,
    accounts_per_lane: u16,
    lane_hashes: Vec<[u8; 32]>,
) -> Result<()> {
    let lane_count = lane_count as usize;
    let apl = accounts_per_lane as usize;
    require!(
        lane_count >= 1 && lane_count <= 20,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let total_lane_accounts = lane_count * apl;
    require!(
        ctx.remaining_accounts.len() >= total_lane_accounts,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let clock = Clock::get()?;
    let group_key = ctx.accounts.group.key();
    let execution_queue_key = ctx.accounts.execution_queue.key();

    // Split remaining_accounts into lane groups
    let lane_slices: Vec<&[AccountInfo]> = (0..lane_count)
        .map(|i| &ctx.remaining_accounts[i * apl..(i + 1) * apl])
        .collect();

    // Use lane hashes passed as instruction data (computed by the executor
    // with the original pre-runtime flags, avoiding Solana flag OR mismatch).
    require!(
        lane_hashes.len() == lane_count,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    // Note: lane_hashes parameter is deprecated and ignored. Hashes are now
    // computed from actual remaining_accounts to prevent account substitution (C-1 fix).
    let execution_queue_bump: u8;
    {
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        execution_queue_bump = queue.bump;
        queue.maybe_activate_pending_ctm(clock.slot);
        require!(
            queue.paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
    }

    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);

    // HLT: Precompute lane hashes once before the item loop.
    // This avoids O(lane_count × accounts_per_lane) SHA256 recomputation
    // per item, replacing it with O(lane_count) byte comparisons.
    let precomputed_lane_hashes: Vec<[u8; 32]> = lane_slices
        .iter()
        .map(|lane| hash_account_infos(lane))
        .collect();

    for _ in 0..max_items {
        let mut candidate: Option<ExecutableCandidate> = None;
        let mut blocked = false;

        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if let Some(item) = queue.current_ctm_head().copied() {
                if clock.slot < item.min_execute_slot {
                    blocked = true;
                } else {
                    let payload_len = item.payload_len as usize;
                    candidate = Some(ExecutableCandidate {
                        sequence: item.sequence,
                        kind: item.kind,
                        payload: item.payload[..payload_len].to_vec(),
                        accounts_hash: item.accounts_hash,
                        retries: item.retries,
                        is_ctm: true,
                    });
                }
            } else if queue.header.ctm_count > 0
                && queue.header.max_seen_sequence >= queue.header.next_sequence_to_execute
            {
                if queue.header.gap_observed_slot == 0 {
                    queue.header.gap_observed_slot = clock.slot;
                }
                if clock.slot
                    >= queue
                        .header
                        .gap_observed_slot
                        .saturating_add(queue.header.gap_wait_slots)
                {
                    let mut skipped = 0u16;
                    while skipped < EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE
                        && queue.header.ctm_count > 0
                        && queue.header.max_seen_sequence >= queue.header.next_sequence_to_execute
                        && queue.current_ctm_head().is_none()
                    {
                        emit!(QueueItemProcessed {
                            group: group_key,
                            sequence: queue.header.next_sequence_to_execute,
                            kind: QueueItemKind::CtmWrapped as u8,
                            status: QueueItemStatus::Skipped as u8,
                        });
                        queue.header.next_sequence_to_execute =
                            queue.header.next_sequence_to_execute.saturating_add(1);
                        skipped = skipped.saturating_add(1);
                    }
                    queue.header.gap_observed_slot = 0;
                    continue;
                }
                blocked = true;
            }
        }

        if blocked {
            break;
        }

        let Some(candidate) = candidate else {
            break;
        };

        // C-1 fix: Use precomputed lane hashes (HLT) instead of recomputing per item.
        // H-8 fix: Strict head-only FIFO — no scan-ahead for non-head items.
        let matched_lane = if candidate.accounts_hash == [0; 32] {
            Some(0)
        } else {
            precomputed_lane_hashes
                .iter()
                .position(|h| *h == candidate.accounts_hash)
        };

        let lane_idx = match matched_lane {
            Some(li) => li,
            None => break,
        };

        let dispatch_accounts = lane_slices[lane_idx];

        // Decode payload
        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(p) => p,
            Err(_) => {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                if candidate.is_ctm {
                    queue.clear_ctm_item_at(candidate.sequence);
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                });
                continue;
            }
        };

        let payload_kind = queue_item_kind_for_payload_variant(decoded_payload.variant);
        if payload_kind != candidate.kind {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(candidate.sequence);
            }
            emit!(QueueItemProcessed {
                group: group_key,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            continue;
        }

        let requires_atomic_failure_rollback =
            variant_requires_atomic_failure_rollback(decoded_payload.variant);

        if candidate.is_ctm {
            if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                queue.clear_ctm_item_at(candidate.sequence);
                emit!(QueueItemProcessed {
                    group: group_key,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                });
                msg!(
                    "{} seq={} reason={:?}",
                    terminal_ctm_failure_msg(reason),
                    candidate.sequence,
                    reason
                );
                continue;
            }
        }

        // Dispatch
        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            dispatch_accounts, // invoke_accounts = dispatch for perp direct calls
            group_key,
            execution_queue_key,
            execution_queue_bump,
        );

        if dispatch_result.is_ok() {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(candidate.sequence);
            }
            emit!(QueueItemProcessed {
                group: group_key,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Executed as u8,
            });
            continue;
        }

        if candidate.is_ctm {
            if requires_atomic_failure_rollback {
                // Atomic CTM failures must roll back the entire tx because the
                // dispatch path may have already mutated the shared hot context.
                return dispatch_result;
            }
            // Non-health-gated CTM: retry up to max, then discard.
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let retries = queue.increment_ctm_retry(candidate.sequence, clock.slot);
            if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                queue.clear_ctm_item_at(candidate.sequence);
                emit!(QueueItemProcessed {
                    group: group_key,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                });
                continue;
            }
            break;
        }
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
    fn canonical_user_intent_message_is_stable_and_binds_owner() {
        let group = Pubkey::new_unique();
        let mango_account = Pubkey::new_unique();
        let owner_a = Pubkey::new_unique();
        let owner_b = Pubkey::new_unique();
        let env = CtmEnvelope {
            sequence: 42,
            min_execute_slot: 99,
            kind: QueueItemKind::CtmWrapped as u8,
            payload_hash: [7; 32],
            accounts_hash: [9; 32],
            expires_at_slot: 120,
        };

        let one = canonical_user_intent_message(group, mango_account, owner_a, &env);
        let two = canonical_user_intent_message(group, mango_account, owner_a, &env);
        let different_owner = canonical_user_intent_message(group, mango_account, owner_b, &env);

        assert_eq!(one, two);
        assert_ne!(one, different_owner);
    }

    #[test]
    fn canonical_user_intent_hex_utf8_is_lowercase_hex() {
        let msg_hash = [0xabu8; 32];
        let hex = canonical_user_intent_message_hex_utf8(msg_hash);
        assert_eq!(hex.len(), 64);
        assert_eq!(&hex[..4], b"abab");
        assert_eq!(&hex[60..], b"abab");
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

    fn write_u16_le(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn test_ed25519_ix(
        signer: Pubkey,
        message: [u8; 32],
        public_key_instruction_index: u16,
        message_instruction_index: u16,
    ) -> Instruction {
        let header_len = ED25519_INSTRUCTION_HEADER_LEN + ED25519_SIGNATURE_OFFSETS_LEN;
        let signature_offset = header_len;
        let public_key_offset = signature_offset + ED25519_SIGNATURE_LEN;
        let message_offset = public_key_offset + ED25519_PUBKEY_LEN;

        let mut data = vec![0u8; message_offset + message.len()];
        data[0] = 1; // num signatures
        data[1] = 0; // padding

        write_u16_le(&mut data, 2, signature_offset as u16);
        write_u16_le(&mut data, 4, ED25519_CURRENT_INSTRUCTION_INDEX);
        write_u16_le(&mut data, 6, public_key_offset as u16);
        write_u16_le(&mut data, 8, public_key_instruction_index);
        write_u16_le(&mut data, 10, message_offset as u16);
        write_u16_le(&mut data, 12, message.len() as u16);
        write_u16_le(&mut data, 14, message_instruction_index);

        data[signature_offset..signature_offset + ED25519_SIGNATURE_LEN]
            .copy_from_slice(&[7u8; 64]);
        data[public_key_offset..public_key_offset + ED25519_PUBKEY_LEN]
            .copy_from_slice(signer.as_ref());
        data[message_offset..message_offset + message.len()].copy_from_slice(&message);

        Instruction {
            program_id: ed25519_program::id(),
            accounts: vec![],
            data,
        }
    }

    #[test]
    fn ed25519_match_requires_structured_signature_for_expected_signer_and_message() {
        let signer = Pubkey::new_unique();
        let msg_hash = [11u8; 32];
        let ix = test_ed25519_ix(
            signer,
            msg_hash,
            ED25519_CURRENT_INSTRUCTION_INDEX,
            ED25519_CURRENT_INSTRUCTION_INDEX,
        );

        assert!(ed25519_ix_matches(&ix, 0, signer, msg_hash.as_ref()));
    }

    #[test]
    fn ed25519_match_rejects_non_self_referential_message_offsets() {
        let signer = Pubkey::new_unique();
        let msg_hash = [13u8; 32];
        let ix = test_ed25519_ix(signer, msg_hash, ED25519_CURRENT_INSTRUCTION_INDEX, 7);

        assert!(!ed25519_ix_matches(&ix, 0, signer, msg_hash.as_ref()));
    }

    #[test]
    fn ed25519_match_rejects_wrong_message() {
        let signer = Pubkey::new_unique();
        let msg_hash = [17u8; 32];
        let ix = test_ed25519_ix(
            signer,
            [19u8; 32],
            ED25519_CURRENT_INSTRUCTION_INDEX,
            ED25519_CURRENT_INSTRUCTION_INDEX,
        );

        assert!(!ed25519_ix_matches(&ix, 0, signer, msg_hash.as_ref()));
    }

    #[test]
    fn execution_queue_account_space_matches_state_layout() {
        assert_eq!(
            EXECUTION_QUEUE_ACCOUNT_SPACE,
            8 + std::mem::size_of::<ExecutionQueue>()
        );
    }
}
