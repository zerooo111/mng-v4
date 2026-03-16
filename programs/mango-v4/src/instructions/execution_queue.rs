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
const EXECUTION_QUEUE_MAX_RETRIES: u8 = 5;
const EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE: u16 = 32;

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
enum QueuePayloadBody {
    PerpPlaceOrderV2(PerpPlaceOrderV2Payload),
    PerpCancelOrder(PerpCancelOrderPayload),
    PerpCancelOrderByClientOrderId(PerpCancelOrderByClientOrderIdPayload),
    PerpCancelAllOrders(PerpCancelAllOrdersPayload),
    PerpCancelAllOrdersBySide(PerpCancelAllOrdersBySidePayload),
    LiquidityDeposit(LiquidityDepositPayload),
    LiquidityWithdraw(LiquidityWithdrawPayload),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueueHealthRegionSpec {
    account_index: usize,
    health_accounts_start: usize,
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
        _ => err!(MangoError::ExecutionQueuePayloadVariantInvalid),
    }
}

fn queue_item_kind_for_payload_variant(variant: QueuePayloadVariant) -> u8 {
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

fn queue_health_region_spec(variant: QueuePayloadVariant) -> Option<QueueHealthRegionSpec> {
    match variant {
        // Only place orders can worsen health; cancels release margin and are always safe.
        QueuePayloadVariant::PerpPlaceOrderV2 => Some(QueueHealthRegionSpec {
            account_index: 1,
            health_accounts_start: 8,
        }),
        _ => None,
    }
}

fn variant_uses_user_signature(variant: QueuePayloadVariant) -> bool {
    matches!(
        variant,
        QueuePayloadVariant::PerpPlaceOrderV2
            | QueuePayloadVariant::PerpCancelOrder
            | QueuePayloadVariant::PerpCancelOrderByClientOrderId
            | QueuePayloadVariant::PerpCancelAllOrders
            | QueuePayloadVariant::PerpCancelAllOrdersBySide
    )
}

fn variant_uses_queue_owner_signer(_variant: QueuePayloadVariant) -> bool {
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

fn verify_ed25519_preinstruction(ixs: &AccountInfo, signer: Pubkey, message: &[u8]) -> Result<()> {
    let found = has_ed25519_preinstruction(ixs, signer, message)?;
    require!(found, MangoError::CtmSignatureMissing);
    Ok(())
}

fn verify_user_ed25519_preinstruction(
    ixs: &AccountInfo,
    signer: Pubkey,
    msg_hash: [u8; 32],
) -> Result<()> {
    if has_ed25519_preinstruction(ixs, signer, msg_hash.as_ref())? {
        return Ok(());
    }

    // Frontend compatibility: some wallets sign utf8(hex(intent_hash)).
    let msg_hex_utf8 = canonical_user_intent_message_hex_utf8(msg_hash);
    let found_hex = has_ed25519_preinstruction(ixs, signer, msg_hex_utf8.as_ref())?;
    require!(found_hex, MangoError::ExecutionQueueUserSignatureMissing);
    Ok(())
}

fn queue_health_region_begin(
    dispatch_accounts: &[AccountInfo],
    spec: QueueHealthRegionSpec,
) -> Result<()> {
    require!(
        dispatch_accounts.len() > spec.account_index,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let account = AccountLoader::<MangoAccountFixed>::try_from(&dispatch_accounts[spec.account_index])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let mut account = account.load_full_mut()?;
    require!(
        !account.fixed.is_in_health_region(),
        MangoError::SomeError
    );
    account.fixed.set_in_health_region(true);

    let group = account.fixed.group;
    let health_accounts = if dispatch_accounts.len() > spec.health_accounts_start {
        &dispatch_accounts[spec.health_accounts_start..]
    } else {
        &[]
    };
    let account_retriever =
        ScanningAccountRetriever::new(health_accounts, &group).context("create account retriever")?;
    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
    let health_cache = new_health_cache(&account.borrow(), &account_retriever, now_ts)?;
    let pre_init_health = account.check_health_pre(&health_cache)?;
    account.fixed.health_region_begin_init_health = pre_init_health.ceil().to_num();
    Ok(())
}

fn queue_health_region_end(
    dispatch_accounts: &[AccountInfo],
    spec: QueueHealthRegionSpec,
) -> Result<()> {
    require!(
        dispatch_accounts.len() > spec.account_index,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let account = AccountLoader::<MangoAccountFixed>::try_from(&dispatch_accounts[spec.account_index])
        .map_err(|_| error!(MangoError::ExecutionQueueDispatchAccountLayoutInvalid))?;
    let mut account = account.load_full_mut()?;
    require!(account.fixed.is_in_health_region(), MangoError::SomeError);
    account.fixed.set_in_health_region(false);

    let group = account.fixed.group;
    let health_accounts = if dispatch_accounts.len() > spec.health_accounts_start {
        &dispatch_accounts[spec.health_accounts_start..]
    } else {
        &[]
    };
    let account_retriever =
        ScanningAccountRetriever::new(health_accounts, &group).context("create account retriever")?;
    let now_ts: u64 = Clock::get()?.unix_timestamp.try_into().unwrap();
    let health_cache = new_health_cache(&account.borrow(), &account_retriever, now_ts)?;
    let pre_init_health = I80F48::from(account.fixed.health_region_begin_init_health);
    account.check_health_post(&health_cache, pre_init_health)?;
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
    require!(
        envelope.sequence >= queue.header.next_sequence_to_execute,
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
        let user_intent_hash = canonical_user_intent_message(
            ctx.accounts.group.key(),
            mango_account_key,
            user_owner,
            &envelope,
        );
        verify_user_ed25519_preinstruction(
            ctx.accounts.instructions.as_ref(),
            user_owner,
            user_intent_hash,
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
    let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
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

    let provided_accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
    let mut active_health_region: Option<QueueHealthRegionSpec> = None;

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

        let Some(mut candidate) = candidate else {
            break;
        };

        // Lane check: if head lane doesn't match, scan forward for a matching item.
        if candidate.accounts_hash != [0; 32] && provided_accounts_hash != candidate.accounts_hash {
            if candidate.is_ctm {
                let queue = ctx.accounts.execution_queue.load_mut()?;
                if let Some((match_seq, match_item)) = queue.find_matching_ctm_item(
                    &provided_accounts_hash,
                    EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE * 4,
                ) {
                    let payload_len = match_item.payload_len as usize;
                    candidate = ExecutableCandidate {
                        sequence: match_seq,
                        kind: match_item.kind,
                        payload: match_item.payload[..payload_len].to_vec(),
                        accounts_hash: match_item.accounts_hash,
                        retries: match_item.retries,
                        is_ctm: true,
                    };
                    drop(queue);
                } else {
                    drop(queue);
                    break;
                }
            } else {
                break;
            }
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

        if active_health_region.is_none() {
            if let Some(spec) = queue_health_region_spec(decoded_payload.variant) {
                queue_health_region_begin(dispatch_accounts, spec)?;
                active_health_region = Some(spec);
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

        let next_retry = candidate.retries.saturating_add(1);
        if candidate.is_ctm || next_retry >= EXECUTION_QUEUE_MAX_RETRIES {
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

        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        queue.rotate_liquidity_head_with_retry(clock.slot, next_retry, 1)?;
        break;
    }

    if let Some(spec) = active_health_region {
        queue_health_region_end(dispatch_accounts, spec)?;
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

    let mut active_health_region: Option<(usize, QueueHealthRegionSpec)> = None;

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

        // Find which lane matches this item using executor-provided hashes
        let matched_lane = if candidate.accounts_hash == [0; 32] {
            Some(0)
        } else {
            lane_hashes
                .iter()
                .position(|h| *h == candidate.accounts_hash)
        };

        let (lane_idx, candidate) = if let Some(li) = matched_lane {
            (li, candidate)
        } else {
            // Head doesn't match any lane — scan for a non-head matching item
            let queue = ctx.accounts.execution_queue.load_mut()?;
            let mut found_candidate: Option<(usize, ExecutableCandidate)> = None;
            for (li, lh) in lane_hashes.iter().enumerate() {
                if let Some((match_seq, match_item)) = queue.find_matching_ctm_item(
                    lh,
                    EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE * 4,
                ) {
                    let payload_len = match_item.payload_len as usize;
                    found_candidate = Some((li, ExecutableCandidate {
                        sequence: match_seq,
                        kind: match_item.kind,
                        payload: match_item.payload[..payload_len].to_vec(),
                        accounts_hash: match_item.accounts_hash,
                        retries: match_item.retries,
                        is_ctm: true,
                    }));
                    break;
                }
            }
            drop(queue);
            match found_candidate {
                Some((li, c)) => (li, c),
                None => break,
            }
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

        // Health region management: switch if lane changed
        if let Some(spec) = queue_health_region_spec(decoded_payload.variant) {
            match active_health_region {
                Some((active_lane, active_spec)) if active_lane != lane_idx => {
                    // End current health region and start new one for this lane
                    queue_health_region_end(lane_slices[active_lane], active_spec)?;
                    queue_health_region_begin(dispatch_accounts, spec)?;
                    active_health_region = Some((lane_idx, spec));
                }
                None => {
                    queue_health_region_begin(dispatch_accounts, spec)?;
                    active_health_region = Some((lane_idx, spec));
                }
                _ => {} // Same lane, keep existing health region
            }
        } else if let Some((active_lane, active_spec)) = active_health_region {
            // Current item doesn't need health region but one is active — end it
            queue_health_region_end(lane_slices[active_lane], active_spec)?;
            active_health_region = None;
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

        // Failed — clear CTM items, no retries for CTM
        if candidate.is_ctm {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            queue.clear_ctm_item_at(candidate.sequence);
            emit!(QueueItemProcessed {
                group: group_key,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            continue;
        }
    }

    if let Some((active_lane, spec)) = active_health_region {
        queue_health_region_end(lane_slices[active_lane], spec)?;
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
