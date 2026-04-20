use crate::accounts_ix::*;
use crate::error::*;
use crate::health::{new_health_cache, ScanningAccountRetriever};
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::{invoke, invoke_signed};
use anchor_lang::solana_program::system_instruction;
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;
use anchor_lang::InstructionData;
use fixed::types::I80F48;

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
    pub market_index: u16,
    pub sequence: u64,
    pub kind: u8,
    pub min_execute_slot: u64,
}

#[event]
pub struct QueueItemProcessed {
    pub group: Pubkey,
    pub market_index: u16,
    pub sequence: u64,
    pub kind: u8,
    pub status: u8,
    /// `QueueFailureCode` value explaining why this item terminalized.
    /// `None = 0` for `Revealed` status. Populated on `Failed` / `Skipped`
    /// so indexers and the relayer can attribute no-ops without re-reading
    /// program logs. Appended to the event schema so existing borsh
    /// decoders that stop after `status` still parse earlier fields.
    pub failure_code: u8,
}

/// Canonical reasons an execution-queue item terminalized without a
/// successful dispatch. Populated on `QueueItemProcessed.failure_code`.
/// Values are stable and additive — new codes append at the end; the
/// numeric values must not be reused.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueFailureCode {
    None = 0,
    Expired = 1,
    DecodeFailed = 2,
    PayloadVariantInvalid = 3,
    PayloadKindMismatch = 4,
    HealthRegionBeginFailed = 5,
    InsufficientMargin = 6,
    BeingLiquidated = 7,
    Bankrupt = 8,
    OracleStale = 9,
    OracleConfidence = 10,
    PriceBandExceeded = 11,
    MarketReduceOnly = 12,
    TokenReduceOnly = 13,
    TokenForceClose = 14,
    AccountFrozen = 15,
    GroupHalted = 16,
    BankBorrowLimit = 17,
    BankNetBorrowsLimit = 18,
    BankDepositLimit = 19,
    GroupDepositLimit = 20,
    WouldSelfTrade = 21,
    RetriesExhausted = 22,
    GapSkipped = 23,
    Other = 255,
}

const ED25519_INSTRUCTION_HEADER_LEN: usize = 2;
const ED25519_SIGNATURE_OFFSETS_LEN: usize = 14;
const ED25519_SIGNATURE_LEN: usize = 64;
const ED25519_PUBKEY_LEN: usize = 32;
const ED25519_CURRENT_INSTRUCTION_INDEX: u16 = u16::MAX;
const QUEUE_PAYLOAD_VERSION_V1: u8 = 1;
const QUEUE_PAYLOAD_HEADER_LEN: usize = 4;
// One retry is sufficient: with the C-1 hash integrity fix, the cranker cannot
// provide wrong accounts to artificially fail dispatch. Non-transient failures
// (expired order, frozen account, paused market) won't resolve on retry.
pub(crate) const EXECUTION_QUEUE_MAX_RETRIES: u8 = 1;
pub(crate) const EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE: u16 = 32;
pub(crate) const DIRECT_SUBMIT_DELAY_SLOTS: u64 = 10;

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
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserIntentTargetKind {
    PerpMarket = 0,
    Token = 1,
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
pub(crate) enum QueuePayloadBody {
    PerpPlaceOrderV2(PerpPlaceOrderV2Payload),
    PerpCancelOrder(PerpCancelOrderPayload),
    PerpCancelOrderByClientOrderId(PerpCancelOrderByClientOrderIdPayload),
    PerpCancelAllOrders(PerpCancelAllOrdersPayload),
    PerpCancelAllOrdersBySide(PerpCancelAllOrdersBySidePayload),
    LiquidityDeposit(LiquidityDepositPayload),
    LiquidityWithdraw(LiquidityWithdrawPayload),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueueHealthRegionSpec {
    account_index: usize,
    health_accounts_start: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct DecodedQueuePayload {
    pub(crate) variant: QueuePayloadVariant,
    pub(crate) flags: u16,
    pub(crate) body: QueuePayloadBody,
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
pub(crate) enum TerminalCtmFailureReason {
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

pub(crate) fn split_dispatch_accounts<'a, 'info>(
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

/// Merge runtime flags for hash computation: OR flags for accounts that appear
/// in both remaining and fixed sets, output in remaining order.
fn merge_effective_runtime_flags_for_hash(
    remaining_accounts: &[AccountMeta],
    fixed_accounts: &[AccountMeta],
) -> Vec<AccountMeta> {
    use std::collections::HashMap;
    let mut merged = HashMap::<Pubkey, (bool, bool)>::new();
    for account in fixed_accounts.iter().chain(remaining_accounts.iter()) {
        let entry = merged
            .entry(account.pubkey)
            .or_insert((account.is_signer, account.is_writable));
        entry.0 |= account.is_signer;
        entry.1 |= account.is_writable;
    }
    remaining_accounts
        .iter()
        .map(|account| {
            let (is_signer, is_writable) = merged
                .get(&account.pubkey)
                .copied()
                .unwrap_or((account.is_signer, account.is_writable));
            AccountMeta {
                pubkey: account.pubkey,
                is_signer,
                is_writable,
            }
        })
        .collect()
}

pub(crate) fn hash_accounts(accounts: &[AccountMeta]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(accounts.len() * 34);
    for a in accounts {
        bytes.extend_from_slice(a.pubkey.as_ref());
        bytes.push(u8::from(a.is_signer));
        bytes.push(u8::from(a.is_writable));
    }
    hashv(&[&bytes]).to_bytes()
}

pub(crate) fn canonical_envelope_message(group: Pubkey, envelope: &CtmEnvelope) -> [u8; 32] {
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

pub(crate) fn canonical_user_intent_message_v1(
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

pub(crate) fn canonical_user_intent_message_v2(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    kind: u8,
    target_kind: UserIntentTargetKind,
    target_index: u16,
    payload_hash: &[u8; 32],
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-user-intent-v2",
        group.as_ref(),
        mango_account.as_ref(),
        user_owner.as_ref(),
        &[kind],
        &[target_kind as u8],
        &target_index.to_le_bytes(),
        payload_hash,
    ])
    .to_bytes()
}

/// v3 of the canonical user-intent hash, used by the v5 commit-reveal path.
///
/// Adds an 8-byte `client_order_id`, supplied randomly by the user per intent.
/// Purpose: payload space for common intents (cancels, standard-size places)
/// is small enough that an observer with a commit_hash could brute-force the
/// pre-image. Mixing in 2^64 random bits per intent makes brute force
/// infeasible, restoring commit-time privacy.
///
/// This hash also replaces the separate relayer-side `commit_hash` used in v4
/// and early v5 — the commit stored on-chain IS this hash, and the user's
/// ed25519 pre-ix signs this same hash. One hash binds user consent, replay
/// resistance (via client_order_id), and the reveal-time integrity check.
///
/// NOT included in the hash (by design):
///   * sequence — assigned by the relayer at commit time; replay protection
///     is provided by client_order_id uniqueness enforced downstream.
///   * min_execute_slot / expires_at_slot — treated as relayer policy, not
///     user-signed. A future v4 may bind these for stricter user control.
pub(crate) fn canonical_user_intent_message_v3(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    kind: u8,
    target_kind: UserIntentTargetKind,
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
        &[target_kind as u8],
        &target_index.to_le_bytes(),
        payload_hash,
        &client_order_id.to_le_bytes(),
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
        _ => err!(MangoError::ExecutionQueuePayloadVariantInvalid),
    }
}

pub(crate) fn queue_item_kind_for_payload_variant(variant: QueuePayloadVariant) -> u8 {
    match variant {
        QueuePayloadVariant::PerpPlaceOrderV2
        | QueuePayloadVariant::PerpCancelOrder
        | QueuePayloadVariant::PerpCancelOrderByClientOrderId
        | QueuePayloadVariant::PerpCancelAllOrders
        | QueuePayloadVariant::PerpCancelAllOrdersBySide => QueueItemKind::CtmWrapped as u8,
        QueuePayloadVariant::LiquidityDeposit => QueueItemKind::LiquidityDeposit as u8,
        QueuePayloadVariant::LiquidityWithdraw => QueueItemKind::LiquidityWithdraw as u8,
    }
}

pub(crate) fn queue_health_region_spec(
    variant: QueuePayloadVariant,
) -> Option<QueueHealthRegionSpec> {
    match variant {
        // Only place orders can worsen health; cancels release margin and are always safe.
        QueuePayloadVariant::PerpPlaceOrderV2 => Some(QueueHealthRegionSpec {
            account_index: 1,
            health_accounts_start: 8,
        }),
        _ => None,
    }
}

pub(crate) fn variant_uses_user_signature(variant: QueuePayloadVariant) -> bool {
    matches!(
        variant,
        QueuePayloadVariant::PerpPlaceOrderV2
            | QueuePayloadVariant::PerpCancelOrder
            | QueuePayloadVariant::PerpCancelOrderByClientOrderId
            | QueuePayloadVariant::PerpCancelAllOrders
            | QueuePayloadVariant::PerpCancelAllOrdersBySide
    )
}

pub(crate) fn variant_uses_queue_owner_signer(_variant: QueuePayloadVariant) -> bool {
    false
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

pub(crate) fn decode_queue_payload(payload: &[u8]) -> Result<DecodedQueuePayload> {
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

pub(crate) fn account_metas_from_infos(account_infos: &[AccountInfo]) -> Vec<AccountMeta> {
    account_infos
        .iter()
        .map(|ai| AccountMeta {
            pubkey: *ai.key,
            is_signer: ai.is_signer,
            is_writable: ai.is_writable,
        })
        .collect()
}

pub(crate) fn canonical_direct_dispatch_account_metas(
    group: Pubkey,
    execution_queue: Pubkey,
    dispatch_accounts: &[AccountMeta],
    user_owner: Option<Pubkey>,
) -> Vec<AccountMeta> {
    let mut effective_remaining = merge_effective_runtime_flags_for_hash(
        dispatch_accounts,
        &[
            AccountMeta {
                pubkey: group,
                is_signer: false,
                is_writable: true,
            },
            AccountMeta {
                pubkey: execution_queue,
                is_signer: false,
                is_writable: true,
            },
            AccountMeta {
                pubkey: tx_instructions::id(),
                is_signer: false,
                is_writable: false,
            },
        ],
    );
    if let Some(user_owner) = user_owner {
        if let Some(owner_meta) = effective_remaining
            .iter_mut()
            .find(|meta| meta.pubkey == user_owner)
        {
            owner_meta.is_signer = false;
            owner_meta.is_writable = false;
        }
    }
    effective_remaining
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

pub(crate) fn prevalidate_terminal_ctm_payload(
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
        _ => None,
    }
}

pub(crate) fn terminal_ctm_failure_msg(reason: TerminalCtmFailureReason) -> &'static str {
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

pub(crate) fn dispatch_queue_payload(
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

fn map_execution_queue_perp_health_validation_error(error: Error) -> Error {
    error!(MangoError::ExecutionQueuePerpHealthAccountsInvalid).context(error)
}

fn validate_perp_place_order_health_accounts(
    group_key: Pubkey,
    dispatch_accounts: &[AccountInfo],
    spec: QueueHealthRegionSpec,
) -> Result<()> {
    require!(
        dispatch_accounts.len() > spec.account_index,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    require!(
        dispatch_accounts.len() > 3,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    require!(
        dispatch_accounts.len() > spec.health_accounts_start,
        MangoError::ExecutionQueuePerpHealthAccountsInvalid
    );

    let account_loader =
        AccountLoader::<MangoAccountFixed>::try_from(&dispatch_accounts[spec.account_index])
            .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let account = account_loader
        .load_full()
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    require!(
        account.fixed.group == group_key,
        MangoError::ExecutionQueueInvalidUserAccount
    );

    let perp_market_loader = AccountLoader::<PerpMarket>::try_from(&dispatch_accounts[3])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let perp_market = perp_market_loader
        .load()
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;

    let health_accounts = &dispatch_accounts[spec.health_accounts_start..];
    let retriever = ScanningAccountRetriever::new(health_accounts, &group_key)
        .map_err(map_execution_queue_perp_health_validation_error)
        .context("create queue perp health account retriever")?;
    let queued_perp_market_index = perp_market.perp_market_index;

    for position in account.active_token_positions() {
        let token_index = position.token_index;
        retriever
            .has_scanned_bank_and_oracle_key(token_index)
            .map_err(map_execution_queue_perp_health_validation_error)
            .with_context(|| format!("missing bank/oracle for token position {}", token_index))?;
    }

    for perp_position in account.active_perp_positions() {
        let perp_market_index = perp_position.market_index;
        retriever
            .has_scanned_perp_market_and_oracle_key(perp_market_index)
            .map_err(map_execution_queue_perp_health_validation_error)
            .with_context(|| {
                format!(
                    "missing perp market/oracle for active perp position {}",
                    perp_market_index
                )
            })?;
    }

    retriever
        .has_scanned_perp_market_and_oracle_key(queued_perp_market_index)
        .map_err(map_execution_queue_perp_health_validation_error)
        .with_context(|| {
            format!(
                "missing perp market/oracle for queued perp market {}",
                queued_perp_market_index
            )
        })?;

    for serum3_orders in account.active_serum3_orders() {
        retriever
            .scanned_serum_oo(&serum3_orders.open_orders)
            .map(|_| ())
            .map_err(map_execution_queue_perp_health_validation_error)
            .with_context(|| format!("missing serum3 open orders {}", serum3_orders.open_orders))?;
    }

    for openbook_v2_orders in account.active_openbook_v2_orders() {
        retriever
            .scanned_openbook_oo(&openbook_v2_orders.open_orders)
            .map(|_| ())
            .map_err(map_execution_queue_perp_health_validation_error)
            .with_context(|| {
                format!(
                    "missing openbook v2 open orders {}",
                    openbook_v2_orders.open_orders
                )
            })?;
    }

    Ok(())
}

pub(crate) fn validate_queue_payload_dispatch_accounts(
    group_key: Pubkey,
    payload: &DecodedQueuePayload,
    dispatch_accounts: &[AccountInfo],
) -> Result<()> {
    if let Some(spec) = queue_health_region_spec(payload.variant) {
        if payload.variant == QueuePayloadVariant::PerpPlaceOrderV2 {
            validate_perp_place_order_health_accounts(group_key, dispatch_accounts, spec)?;
        }
    }
    Ok(())
}

/// Verify that the perp_market account at canonical position [3] in the
/// dispatch accounts has `perp_market_index == expected_market_index`.
/// This binds the instruction's `market_index` parameter to the actual
/// PerpMarket being dispatched against, so a relayer cannot route an order
/// for market A into market B's sub-queue. Liquidity payloads have no perp
/// market in the dispatch accounts and bypass this check (they go to the
/// global liquidity ring).
pub(crate) fn require_dispatch_market_index(
    dispatch_accounts: &[AccountInfo],
    expected_market_index: u16,
) -> Result<()> {
    require!(
        dispatch_accounts.len() > 3,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let perp_market_loader = AccountLoader::<PerpMarket>::try_from(&dispatch_accounts[3])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let perp_market = perp_market_loader
        .load()
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    require!(
        perp_market.perp_market_index == expected_market_index,
        MangoError::ExecutionQueueSubQueueMarketIndexMismatch
    );
    Ok(())
}

pub(crate) fn extract_user_owner_for_ctm_payload(
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

fn ed25519_ix_matches(
    ix: &Instruction,
    ix_index: usize,
    ctm_signer: Pubkey,
    message: &[u8],
) -> bool {
    if ix.program_id != ed25519_program::id() || ix.data.len() < ED25519_INSTRUCTION_HEADER_LEN {
        return false;
    }

    let signature_count = ix.data[0] as usize;
    if signature_count == 0 {
        return false;
    }

    let min_len = ED25519_INSTRUCTION_HEADER_LEN + signature_count * ED25519_SIGNATURE_OFFSETS_LEN;
    if ix.data.len() < min_len {
        return false;
    }

    for signature_index in 0..signature_count {
        let Some(offsets) = parse_ed25519_signature_offsets(&ix.data, signature_index) else {
            continue;
        };

        // Require the referenced signature blob to exist in this ed25519 instruction.
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

        if public_key_bytes != ctm_signer.as_ref() {
            continue;
        }

        if offsets.message_data_size as usize != message.len() {
            continue;
        }

        let Some(message_bytes) = extract_bytes(
            ix,
            ix_index,
            offsets.message_instruction_index,
            offsets.message_data_offset as usize,
            offsets.message_data_size as usize,
        ) else {
            continue;
        };

        if message_bytes == message {
            return true;
        }
    }

    false
}

fn has_ed25519_preinstruction(
    ixs: &AccountInfo,
    ctm_signer: Pubkey,
    message: &[u8],
) -> Result<bool> {
    let current_index = tx_instructions::load_current_index_checked(ixs)? as usize;
    for index in 0..current_index {
        let ix = tx_instructions::load_instruction_at_checked(index, ixs)?;
        if ed25519_ix_matches(&ix, index, ctm_signer, message) {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(crate) fn verify_ed25519_preinstruction(
    ixs: &AccountInfo,
    signer: Pubkey,
    message: &[u8],
) -> Result<()> {
    let found = has_ed25519_preinstruction(ixs, signer, message)?;
    require!(found, MangoError::CtmSignatureMissing);
    Ok(())
}

pub(crate) fn verify_user_ed25519_preinstruction(
    ixs: &AccountInfo,
    signer: Pubkey,
    msg_hashes: &[[u8; 32]],
) -> Result<()> {
    for msg_hash in msg_hashes {
        if has_ed25519_preinstruction(ixs, signer, msg_hash.as_ref())? {
            return Ok(());
        }

        // Frontend compatibility: some wallets sign utf8(hex(intent_hash)).
        let msg_hex_utf8 = canonical_user_intent_message_hex_utf8(*msg_hash);
        if has_ed25519_preinstruction(ixs, signer, msg_hex_utf8.as_ref())? {
            return Ok(());
        }
    }

    err!(MangoError::ExecutionQueueUserSignatureMissing)
}

pub(crate) fn queue_health_region_begin(
    dispatch_accounts: &[AccountInfo],
    spec: QueueHealthRegionSpec,
) -> Result<()> {
    require!(
        dispatch_accounts.len() > spec.account_index,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let account =
        AccountLoader::<MangoAccountFixed>::try_from(&dispatch_accounts[spec.account_index])
            .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let mut account = account.load_full_mut()?;

    // Self-heal stuck `in_health_region` flags. A prior tx that called
    // `queue_health_region_begin` and then errored mid-CPI (before
    // `queue_health_region_end` ran) leaves the account marked as "in
    // region" forever, which made every future reveal for that account
    // fail silently in the enqueue path. There is no re-entrancy risk
    // here — this helper is only invoked inside a single top-level
    // reveal_execute_market ix, which runs sequentially — so clearing
    // the flag at the start of a fresh begin is safe. The stored
    // `pre_init_health` would also be stale; wipe it too.
    if account.fixed.is_in_health_region() {
        msg!(
            "queue_health_region_begin: cleared stale in_health_region flag on {}",
            account.fixed.name()
        );
        account.fixed.set_in_health_region(false);
        account.fixed.health_region_begin_init_health = 0;
    }

    let group = account.fixed.group;
    let health_accounts = if dispatch_accounts.len() > spec.health_accounts_start {
        &dispatch_accounts[spec.health_accounts_start..]
    } else {
        &[]
    };
    let account_retriever = ScanningAccountRetriever::new(health_accounts, &group)
        .context("create account retriever")?;
    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
    let health_cache = new_health_cache(&account.borrow(), &account_retriever, now_ts)?;
    let pre_init_health = account.check_health_pre(&health_cache)?;
    account.fixed.set_in_health_region(true);
    account.fixed.health_region_begin_init_health = pre_init_health.ceil().to_num();
    Ok(())
}

pub(crate) fn queue_health_region_end(
    dispatch_accounts: &[AccountInfo],
    spec: QueueHealthRegionSpec,
) -> Result<()> {
    require!(
        dispatch_accounts.len() > spec.account_index,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let account =
        AccountLoader::<MangoAccountFixed>::try_from(&dispatch_accounts[spec.account_index])
            .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let mut account = account.load_full_mut()?;
    require!(account.fixed.is_in_health_region(), MangoError::SomeError);

    let group = account.fixed.group;
    let health_accounts = if dispatch_accounts.len() > spec.health_accounts_start {
        &dispatch_accounts[spec.health_accounts_start..]
    } else {
        &[]
    };
    let account_retriever = ScanningAccountRetriever::new(health_accounts, &group)
        .context("create account retriever")?;
    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
    let health_cache = new_health_cache(&account.borrow(), &account_retriever, now_ts)?;
    let pre_init_health = I80F48::from(account.fixed.health_region_begin_init_health);
    account.check_health_post(&health_cache, pre_init_health)?;
    account.fixed.set_in_health_region(false);
    account.fixed.health_region_begin_init_health = 0;
    Ok(())
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
    queue.require_v2()?;
    queue.global_header.gap_wait_slots = params.gap_wait_slots;
    queue.global_header.liquidity_delay_slots = params.liquidity_delay_slots;
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

/// Migrate a v1 single-queue ExecutionQueue to the v2 sub-queue layout.
///
/// **Preconditions** (all enforced):
/// - Caller is the queue admin (validated by `ExecutionQueueAdmin` constraint).
/// - The queue is currently at v1 (layout_version == 0). Calling on a v2
///   queue is a no-op success.
/// - Both `paused_ingress` and `paused_execute` are set. The queue must be
///   fully paused before migration to avoid concurrent state mutation.
/// - `total_count == 0`. All in-flight items must be drained or admin-dropped
///   before migration. We do not attempt to translate v1 ring buffer
///   contents into v2 sub-queue contents in-place — the legacy `next_sequence`
///   would map to sub-queue slot 0 but the per-market layout requires
///   determining which market each pending item belongs to, which the v1
///   layout doesn't track.
///
/// **What it does**:
/// 1. Re-initializes `global_header` with the v1 `gap_wait_slots` and
///    `liquidity_delay_slots` carried over.
/// 2. Zeroes the legacy v1 ring buffer region and rewrites it as the v2
///    sub-queue header array (16 slots, all inactive) followed by the v2
///    flat item array.
/// 3. Sets `layout_version = 2`.
///
/// After migration the queue is empty, paused, and ready to accept v2
/// instructions. Admin must explicitly unpause via `execution_queue_configure`.
pub fn execution_queue_migrate_v1_to_v2(ctx: Context<ExecutionQueueAdmin>) -> Result<()> {
    let mut queue = ctx.accounts.execution_queue.load_mut()?;

    if queue.layout_version == EXECUTION_QUEUE_LAYOUT_VERSION_V2 {
        // Already migrated; idempotent success.
        return Ok(());
    }
    require!(
        queue.layout_version == EXECUTION_QUEUE_LAYOUT_VERSION_V1,
        MangoError::ExecutionQueueSubQueueLayoutVersionMismatch
    );
    require!(
        queue.paused_ingress != 0 && queue.paused_execute != 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );
    require!(
        queue.global_header.total_count == 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );

    // Capture v1 tunables we want to carry over. The v1 header field layout
    // and the v2 ExecutionQueueGlobalHeader field layout are intentionally
    // different (v1 had per-queue sequence/count fields where v2 has only
    // tunables); the realloc below moves bytes around. We rebuild the
    // global header from scratch using the carried tunables.
    let carry_gap_wait_slots = queue.global_header.gap_wait_slots;
    let carry_liquidity_delay_slots = queue.global_header.liquidity_delay_slots;

    queue.global_header = ExecutionQueueGlobalHeader {
        gap_wait_slots: carry_gap_wait_slots,
        liquidity_delay_slots: carry_liquidity_delay_slots,
        liquidity_head: 0,
        liquidity_count: 0,
        total_count: 0,
        _padding: 0,
        reserved: [0; 32],
    };

    // Sub-queue headers all start inactive. They are bound to specific
    // markets on the first enqueue per market via get_or_allocate_sub_queue_slot.
    for header in queue.sub_queue_headers.iter_mut() {
        *header = SubQueueHeader {
            market_index: 0,
            active: 0,
            paused_ingress: 0,
            paused_execute: 0,
            _padding0: [0; 3],
            ctm_count: 0,
            _padding1: 0,
            next_sequence_to_execute: 0,
            max_seen_sequence: 0,
            gap_observed_slot: 0,
            first_failure_slot: 0,
            reserved: [0; 16],
        };
    }

    // Item arrays are zeroed at allocation by Solana; preconditions
    // (total_count == 0) guarantee no pending items remain. We rezero
    // explicitly to defend against partial-fail states from prior buggy
    // sequences.
    for item in queue.ctm_items.iter_mut() {
        *item = QueueItem::default();
    }

    queue.layout_version = EXECUTION_QUEUE_LAYOUT_VERSION_V2;
    queue._padding = [0; 4];
    Ok(())
}

pub fn execution_queue_drop_ctm(
    ctx: Context<ExecutionQueueAdmin>,
    market_index: u16,
    sequence: u64,
) -> Result<()> {
    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.require_v2()?;
    queue.maybe_activate_pending_ctm(clock.slot);
    require!(
        queue.paused_execute != 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );

    let item = *queue.ctm_item(market_index, sequence)?;
    require!(
        item.status == QueueItemStatus::Pending as u8 && item.sequence == sequence,
        MangoError::ExecutionQueueSequenceNotPending
    );

    queue.clear_ctm_item_at(market_index, sequence)?;
    emit!(QueueItemProcessed {
        group: ctx.accounts.group.key(),
        market_index,
        sequence,
        kind: item.kind,
        status: QueueItemStatus::Failed as u8,
        failure_code: 0,
    });
    Ok(())
}

pub fn execution_queue_enqueue_ctm(
    ctx: Context<ExecutionQueueEnqueueCtm>,
    market_index: u16,
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

    // Bind market_index to the actual perp_market in dispatch_accounts.
    // Done before any queue mutation so a mismatch fails fast.
    require_dispatch_market_index(dispatch_accounts, market_index)?;

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.require_v2()?;
    queue.maybe_activate_pending_ctm(clock.slot);

    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    // Per-market pause: respect the sub-queue's own pause flag if the slot
    // is already bound. Newly-bound slots inherit unpaused state at allocation.
    if let Some(sub) = queue
        .find_sub_queue_slot(market_index)
        .map(|i| &queue.sub_queue_headers[i])
    {
        require!(
            sub.paused_ingress == 0,
            MangoError::ExecutionQueueIngressPaused
        );
    }

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
    validate_queue_payload_dispatch_accounts(
        ctx.accounts.group.key(),
        &decoded_payload,
        dispatch_accounts,
    )?;
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

    let account_hash = hash_accounts(
        &dispatch_accounts
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
    // Per-market sequence floor: an unbound market starts at 0.
    let sequence_floor = queue
        .find_sub_queue_slot(market_index)
        .map(|i| queue.sub_queue_headers[i].next_sequence_to_execute)
        .unwrap_or(0);
    require!(
        envelope.sequence >= sequence_floor,
        MangoError::InvalidSequenceNumber
    );

    let msg_hash = canonical_envelope_message(ctx.accounts.group.key(), &envelope);
    verify_ed25519_preinstruction(
        ctx.accounts.instructions.as_ref(),
        queue.ctm_signer,
        msg_hash.as_ref(),
    )?;

    if variant_uses_user_signature(decoded_payload.variant) {
        let (mango_account_key, user_owner) =
            extract_user_owner_for_ctm_payload(ctx.accounts.group.key(), dispatch_accounts)?;
        let user_intent_hash_v2 = canonical_user_intent_message_v2(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            envelope.kind,
            UserIntentTargetKind::PerpMarket,
            market_index,
            &envelope.payload_hash,
        );
        let user_intent_hash_v1 = canonical_user_intent_message_v1(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            &envelope,
        );
        verify_user_ed25519_preinstruction(
            ctx.accounts.instructions.as_ref(),
            user_owner,
            &[user_intent_hash_v2, user_intent_hash_v1],
        )?;
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

    queue.push_ctm(market_index, item)?;
    // push_ctm bumps max_seen_sequence internally.

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index,
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
    market_index: u16,
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

    require_dispatch_market_index(dispatch_accounts, market_index)?;

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    queue.require_v2()?;
    queue.maybe_activate_pending_ctm(clock.slot);

    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    if let Some(sub) = queue
        .find_sub_queue_slot(market_index)
        .map(|i| &queue.sub_queue_headers[i])
    {
        require!(
            sub.paused_ingress == 0,
            MangoError::ExecutionQueueIngressPaused
        );
    }

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
    validate_queue_payload_dispatch_accounts(
        ctx.accounts.group.key(),
        &decoded_payload,
        dispatch_accounts,
    )?;
    let user_signature_keys = if variant_uses_user_signature(decoded_payload.variant) {
        Some(extract_user_owner_for_ctm_payload(
            ctx.accounts.group.key(),
            dispatch_accounts,
        )?)
    } else {
        None
    };
    let dispatch_account_metas = account_metas_from_infos(dispatch_accounts);
    let account_hash = hash_accounts(&canonical_direct_dispatch_account_metas(
        ctx.accounts.group.key(),
        ctx.accounts.execution_queue.key(),
        &dispatch_account_metas,
        user_signature_keys.map(|(_, user_owner)| user_owner),
    ));
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
    if let Some((mango_account_key, user_owner)) = user_signature_keys {
        let user_intent_hash_v2 = canonical_user_intent_message_v2(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            envelope.kind,
            UserIntentTargetKind::PerpMarket,
            market_index,
            &envelope.payload_hash,
        );
        let user_intent_hash_v1 = canonical_user_intent_message_v1(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            &envelope,
        );
        verify_user_ed25519_preinstruction(
            ctx.accounts.instructions.as_ref(),
            user_owner,
            &[user_intent_hash_v2, user_intent_hash_v1],
        )?;
    }

    // Per-market sequence allocator: max_seen_sequence + 1.
    let prev_max_seen = queue
        .find_sub_queue_slot(market_index)
        .map(|i| queue.sub_queue_headers[i].max_seen_sequence)
        .unwrap_or(0);
    let sequence = prev_max_seen.saturating_add(1);
    require!(
        queue.can_enqueue_ctm_sequence(market_index, sequence),
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

    queue.push_ctm(market_index, item)?;

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index,
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
    queue.require_v2()?;
    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );

    let payload_hash = hashv(&[&payload]).to_bytes();
    let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
    let min_execute_slot = clock.slot + queue.global_header.liquidity_delay_slots;
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

    // Liquidity items are not market-keyed; emit market_index = u16::MAX as
    // a sentinel meaning "global liquidity ring".
    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index: u16::MAX,
        sequence: 0,
        kind,
        min_execute_slot,
    });

    Ok(())
}

pub fn execution_queue_execute(
    ctx: Context<ExecutionQueueExecute>,
    market_index: u16,
    max_items: u16,
) -> Result<()> {
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (dispatch_accounts, invoke_accounts) =
        split_dispatch_accounts(ctx.remaining_accounts, ctx.accounts.execution_queue.key())?;

    // Bind market_index to the dispatched perp_market. Liquidity items are
    // identified by sentinel u16::MAX (which has no perp_market in
    // dispatch_accounts) and skip this check.
    if market_index != u16::MAX {
        require_dispatch_market_index(dispatch_accounts, market_index)?;
    }

    let clock = Clock::get()?;
    let group_key = ctx.accounts.group.key();
    let execution_queue_key = ctx.accounts.execution_queue.key();
    let execution_queue_bump: u8;
    {
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.require_v2()?;
        execution_queue_bump = queue.bump;
        queue.maybe_activate_pending_ctm(clock.slot);
        require!(
            queue.paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
        if market_index != u16::MAX {
            if let Some(sub) = queue
                .find_sub_queue_slot(market_index)
                .map(|i| &queue.sub_queue_headers[i])
            {
                require!(
                    sub.paused_execute == 0,
                    MangoError::ExecutionQueueExecutePaused
                );
            }
        }
    }

    let provided_accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    for _ in 0..max_items {
        let mut candidate: Option<ExecutableCandidate> = None;
        let mut blocked = false;

        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            // Snapshot per-market state once for this iteration. If the
            // market is unbound (no slot allocated), all CTM-related fields
            // are zero and we fall through to liquidity processing.
            let sub_slot_opt = if market_index == u16::MAX {
                None
            } else {
                queue.find_sub_queue_slot(market_index)
            };
            let (sub_ctm_count, sub_next_seq, sub_max_seen, sub_gap_obs) =
                if let Some(slot) = sub_slot_opt {
                    let h = &queue.sub_queue_headers[slot];
                    (
                        h.ctm_count,
                        h.next_sequence_to_execute,
                        h.max_seen_sequence,
                        h.gap_observed_slot,
                    )
                } else {
                    (0u32, 0u64, 0u64, 0u64)
                };
            let gap_wait_slots = queue.global_header.gap_wait_slots;

            if let Some(item) = market_index_safe_current_ctm_head(&queue, market_index).copied() {
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
            } else if sub_ctm_count > 0 && sub_max_seen >= sub_next_seq {
                let mut new_gap_obs = sub_gap_obs;
                if new_gap_obs == 0 {
                    new_gap_obs = clock.slot;
                    if let Some(slot) = sub_slot_opt {
                        queue.sub_queue_headers[slot].gap_observed_slot = new_gap_obs;
                    }
                }
                if clock.slot >= new_gap_obs.saturating_add(gap_wait_slots) {
                    if let Some(slot) = sub_slot_opt {
                        let mut skipped = 0u16;
                        while skipped < EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE
                            && queue.sub_queue_headers[slot].ctm_count > 0
                            && queue.sub_queue_headers[slot].max_seen_sequence
                                >= queue.sub_queue_headers[slot].next_sequence_to_execute
                            && market_index_safe_current_ctm_head(&queue, market_index).is_none()
                        {
                            let skip_seq = queue.sub_queue_headers[slot].next_sequence_to_execute;
                            emit!(QueueItemProcessed {
                                group: group_key,
                                market_index,
                                sequence: skip_seq,
                                kind: QueueItemKind::CtmWrapped as u8,
                                status: QueueItemStatus::Skipped as u8,
                                failure_code: 0,
                            });
                            queue.sub_queue_headers[slot].next_sequence_to_execute =
                                skip_seq.saturating_add(1);
                            skipped = skipped.saturating_add(1);
                        }
                        queue.sub_queue_headers[slot].gap_observed_slot = 0;
                    }
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
                    queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                } else {
                    let _ = queue.pop_liquidity_head();
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                    failure_code: 0,
                });
                continue;
            }
        };

        let payload_kind = queue_item_kind_for_payload_variant(decoded_payload.variant);
        if payload_kind != candidate.kind {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
            } else {
                let _ = queue.pop_liquidity_head();
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let item_health_region = queue_health_region_spec(decoded_payload.variant);

        if candidate.is_ctm {
            if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                    failure_code: 0,
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

        if item_health_region.is_some()
            && candidate.is_ctm
            && candidate.retries >= EXECUTION_QUEUE_MAX_RETRIES
        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            queue.clear_ctm_item_at(market_index, candidate.sequence)?;
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        if let Some(spec) = item_health_region {
            if queue_health_region_begin(dispatch_accounts, spec).is_err() {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                if candidate.is_ctm {
                    let retries =
                        queue.increment_ctm_retry(market_index, candidate.sequence, clock.slot)?;
                    if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                        queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                        emit!(QueueItemProcessed {
                            group: group_key,
                            market_index,
                            sequence: candidate.sequence,
                            kind: candidate.kind,
                            status: QueueItemStatus::Failed as u8,
                            failure_code: 0,
                        });
                        continue;
                    }
                } else {
                    let _ = queue.pop_liquidity_head();
                }
                break;
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
        let dispatch_result = if let Some(spec) = item_health_region {
            match dispatch_result {
                Ok(()) => queue_health_region_end(dispatch_accounts, spec),
                Err(err) => Err(err),
            }
        } else {
            dispatch_result
        };

        if dispatch_result.is_ok() {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
            } else {
                let _ = queue.pop_liquidity_head();
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Executed as u8,
                failure_code: 0,
            });
            continue;
        }

        // Dispatch failed. Handle retries and clearing.
        if candidate.is_ctm {
            if item_health_region.is_some() {
                return dispatch_result;
            }
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let retries =
                queue.increment_ctm_retry(market_index, candidate.sequence, clock.slot)?;
            if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                    failure_code: 0,
                });
                continue;
            }
            break;
        }

        // Liquidity item failure: retry up to max, then discard.
        let next_retry = candidate.retries.saturating_add(1);
        if next_retry >= EXECUTION_QUEUE_MAX_RETRIES {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let _ = queue.pop_liquidity_head();
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.rotate_liquidity_head_with_retry(clock.slot, next_retry, 1)?;
        break;
    }

    Ok(())
}

/// Helper used by execute paths: returns the current CTM head for a market
/// safely (returns None if the market sentinel is u16::MAX or the market is
/// unbound).
fn market_index_safe_current_ctm_head<'a>(
    queue: &'a std::cell::RefMut<'_, ExecutionQueue>,
    market_index: u16,
) -> Option<&'a QueueItem> {
    if market_index == u16::MAX {
        return None;
    }
    queue.current_ctm_head(market_index)
}

/// Multi-lane execute: process items from multiple lane account sets in one tx.
/// `lane_count` account groups of `accounts_per_lane` accounts each are packed
/// into remaining_accounts. The function pre-hashes each group and matches
/// queue items against any lane, switching health regions as needed.
pub fn execution_queue_execute_multi(
    ctx: Context<ExecutionQueueExecute>,
    market_index: u16,
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

    // Bind market_index against the first lane's perp_market. All lanes for
    // a single execute_multi call must target the same market because we
    // operate on a single sub-queue per call.
    require_dispatch_market_index(lane_slices[0], market_index)?;
    for lane in lane_slices.iter().skip(1) {
        require_dispatch_market_index(lane, market_index)?;
    }

    require!(
        lane_hashes.len() == lane_count,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let execution_queue_bump: u8;
    {
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.require_v2()?;
        execution_queue_bump = queue.bump;
        queue.maybe_activate_pending_ctm(clock.slot);
        require!(
            queue.paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
        if let Some(sub) = queue
            .find_sub_queue_slot(market_index)
            .map(|i| &queue.sub_queue_headers[i])
        {
            require!(
                sub.paused_execute == 0,
                MangoError::ExecutionQueueExecutePaused
            );
        }
    }

    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);

    // HLT: Precompute lane hashes once before the item loop.
    let precomputed_lane_hashes: Vec<[u8; 32]> = lane_slices
        .iter()
        .map(|lane| hash_accounts(&account_metas_from_infos(lane)))
        .collect();

    // Caller-supplied `lane_hashes` must agree with the accounts actually
    // passed in `remaining_accounts`. The program still uses the precomputed
    // hashes for lane matching; the equality check tightens the ABI so
    // callers can't silently disagree with the program about the lane
    // layout.
    for (i, declared) in lane_hashes.iter().enumerate() {
        require!(
            *declared == precomputed_lane_hashes[i],
            MangoError::ExecutionQueueAccountsHashMismatch
        );
    }

    for _ in 0..max_items {
        let mut candidate: Option<ExecutableCandidate> = None;
        let mut blocked = false;

        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let sub_slot_opt = queue.find_sub_queue_slot(market_index);
            let (sub_ctm_count, sub_next_seq, sub_max_seen, sub_gap_obs) =
                if let Some(slot) = sub_slot_opt {
                    let h = &queue.sub_queue_headers[slot];
                    (
                        h.ctm_count,
                        h.next_sequence_to_execute,
                        h.max_seen_sequence,
                        h.gap_observed_slot,
                    )
                } else {
                    (0u32, 0u64, 0u64, 0u64)
                };
            let gap_wait_slots = queue.global_header.gap_wait_slots;

            if let Some(item) = market_index_safe_current_ctm_head(&queue, market_index).copied() {
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
            } else if sub_ctm_count > 0 && sub_max_seen >= sub_next_seq {
                let mut new_gap_obs = sub_gap_obs;
                if new_gap_obs == 0 {
                    new_gap_obs = clock.slot;
                    if let Some(slot) = sub_slot_opt {
                        queue.sub_queue_headers[slot].gap_observed_slot = new_gap_obs;
                    }
                }
                if clock.slot >= new_gap_obs.saturating_add(gap_wait_slots) {
                    if let Some(slot) = sub_slot_opt {
                        let mut skipped = 0u16;
                        while skipped < EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE
                            && queue.sub_queue_headers[slot].ctm_count > 0
                            && queue.sub_queue_headers[slot].max_seen_sequence
                                >= queue.sub_queue_headers[slot].next_sequence_to_execute
                            && market_index_safe_current_ctm_head(&queue, market_index).is_none()
                        {
                            let skip_seq = queue.sub_queue_headers[slot].next_sequence_to_execute;
                            emit!(QueueItemProcessed {
                                group: group_key,
                                market_index,
                                sequence: skip_seq,
                                kind: QueueItemKind::CtmWrapped as u8,
                                status: QueueItemStatus::Skipped as u8,
                                failure_code: 0,
                            });
                            queue.sub_queue_headers[slot].next_sequence_to_execute =
                                skip_seq.saturating_add(1);
                            skipped = skipped.saturating_add(1);
                        }
                        queue.sub_queue_headers[slot].gap_observed_slot = 0;
                    }
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

        // C-1 fix: Use precomputed lane hashes (HLT).
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

        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(p) => p,
            Err(_) => {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                if candidate.is_ctm {
                    queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                    failure_code: 0,
                });
                continue;
            }
        };

        let payload_kind = queue_item_kind_for_payload_variant(decoded_payload.variant);
        if payload_kind != candidate.kind {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let item_health_region = queue_health_region_spec(decoded_payload.variant);

        if candidate.is_ctm {
            if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                    failure_code: 0,
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

        if item_health_region.is_some()
            && candidate.is_ctm
            && candidate.retries >= EXECUTION_QUEUE_MAX_RETRIES
        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            queue.clear_ctm_item_at(market_index, candidate.sequence)?;
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        if let Some(spec) = item_health_region {
            if queue_health_region_begin(dispatch_accounts, spec).is_err() {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                if candidate.is_ctm {
                    let retries =
                        queue.increment_ctm_retry(market_index, candidate.sequence, clock.slot)?;
                    if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                        queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                        emit!(QueueItemProcessed {
                            group: group_key,
                            market_index,
                            sequence: candidate.sequence,
                            kind: candidate.kind,
                            status: QueueItemStatus::Failed as u8,
                            failure_code: 0,
                        });
                        continue;
                    }
                }
                break;
            }
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            dispatch_accounts,
            group_key,
            execution_queue_key,
            execution_queue_bump,
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
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            if candidate.is_ctm {
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Executed as u8,
                failure_code: 0,
            });
            continue;
        }

        if candidate.is_ctm {
            if item_health_region.is_some() {
                return dispatch_result;
            }
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let retries =
                queue.increment_ctm_retry(market_index, candidate.sequence, clock.slot)?;
            if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                queue.clear_ctm_item_at(market_index, candidate.sequence)?;
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatus::Failed as u8,
                    failure_code: 0,
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
    fn canonical_user_intent_messages_are_stable_and_bind_owner() {
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

        let v1_one = canonical_user_intent_message_v1(group, mango_account, owner_a, &env);
        let v1_two = canonical_user_intent_message_v1(group, mango_account, owner_a, &env);
        let v1_different_owner =
            canonical_user_intent_message_v1(group, mango_account, owner_b, &env);
        let v2_one = canonical_user_intent_message_v2(
            group,
            mango_account,
            owner_a,
            env.kind,
            UserIntentTargetKind::PerpMarket,
            7,
            &env.payload_hash,
        );
        let v2_two = canonical_user_intent_message_v2(
            group,
            mango_account,
            owner_a,
            env.kind,
            UserIntentTargetKind::PerpMarket,
            7,
            &env.payload_hash,
        );
        let v2_different_owner = canonical_user_intent_message_v2(
            group,
            mango_account,
            owner_b,
            env.kind,
            UserIntentTargetKind::PerpMarket,
            7,
            &env.payload_hash,
        );

        assert_eq!(v1_one, v1_two);
        assert_ne!(v1_one, v1_different_owner);
        assert_eq!(v2_one, v2_two);
        assert_ne!(v2_one, v2_different_owner);
    }

    #[test]
    fn canonical_user_intent_v2_does_not_bind_accounts_hash() {
        let group = Pubkey::new_unique();
        let mango_account = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let env_a = CtmEnvelope {
            sequence: 42,
            min_execute_slot: 99,
            kind: QueueItemKind::CtmWrapped as u8,
            payload_hash: [7; 32],
            accounts_hash: [9; 32],
            expires_at_slot: 120,
        };
        let env_b = CtmEnvelope {
            accounts_hash: [11; 32],
            ..env_a
        };

        let v1_a = canonical_user_intent_message_v1(group, mango_account, owner, &env_a);
        let v1_b = canonical_user_intent_message_v1(group, mango_account, owner, &env_b);
        let v2_a = canonical_user_intent_message_v2(
            group,
            mango_account,
            owner,
            env_a.kind,
            UserIntentTargetKind::PerpMarket,
            7,
            &env_a.payload_hash,
        );
        let v2_b = canonical_user_intent_message_v2(
            group,
            mango_account,
            owner,
            env_b.kind,
            UserIntentTargetKind::PerpMarket,
            7,
            &env_b.payload_hash,
        );

        assert_ne!(v1_a, v1_b);
        assert_eq!(v2_a, v2_b);
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

    #[test]
    fn canonical_direct_dispatch_account_metas_scrubs_user_owner_runtime_uplift() {
        let group = Pubkey::new_unique();
        let execution_queue = Pubkey::new_unique();
        let mango_account = Pubkey::new_unique();
        let user_owner = Pubkey::new_unique();
        let perp_market = Pubkey::new_unique();
        let dispatch_accounts = vec![
            AccountMeta::new_readonly(group, false),
            AccountMeta::new(mango_account, false),
            AccountMeta::new(user_owner, true),
            AccountMeta::new(perp_market, false),
        ];

        let canonical = canonical_direct_dispatch_account_metas(
            group,
            execution_queue,
            &dispatch_accounts,
            Some(user_owner),
        );

        assert_eq!(canonical[0].pubkey, group);
        assert!(!canonical[0].is_signer);
        assert!(canonical[0].is_writable);
        assert_eq!(canonical[2].pubkey, user_owner);
        assert!(!canonical[2].is_signer);
        assert!(!canonical[2].is_writable);
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
