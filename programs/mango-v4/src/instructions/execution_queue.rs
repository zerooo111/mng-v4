use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::{invoke, invoke_signed};
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;
use anchor_lang::Discriminator;
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
const EXECUTION_QUEUE_MAX_RETRIES: u8 = 5;

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

#[derive(Clone, Debug)]
struct DecodedQueuePayload {
    variant: QueuePayloadVariant,
    flags: u16,
    body: QueuePayloadBody,
}

#[derive(Clone, Debug)]
struct ExecutableCandidate {
    idx: usize,
    sequence: u64,
    kind: u8,
    payload: Vec<u8>,
    payload_hash: [u8; 32],
    accounts_hash: [u8; 32],
    retries: u8,
    is_ctm_lane: bool,
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

fn find_free_slot(queue: &ExecutionQueue, buffer: &ExecutionQueueBuffer) -> Option<usize> {
    buffer.items[..queue.capacity as usize]
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

fn variant_uses_queue_owner_signer(variant: QueuePayloadVariant) -> bool {
    matches!(
        variant,
        QueuePayloadVariant::PerpPlaceOrderV2
            | QueuePayloadVariant::PerpCancelOrder
            | QueuePayloadVariant::PerpCancelOrderByClientOrderId
            | QueuePayloadVariant::PerpCancelAllOrders
            | QueuePayloadVariant::PerpCancelAllOrdersBySide
    )
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

fn dispatch_queue_payload(
    payload: &DecodedQueuePayload,
    dispatch_accounts: &[AccountInfo],
    invoke_accounts: &[AccountInfo],
    group_key: Pubkey,
    execution_queue_key: Pubkey,
    execution_queue_bump: u8,
) -> Result<()> {
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
    msg_hash: [u8; 32],
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

        if offsets.message_data_size as usize != msg_hash.len() {
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

        if message_bytes == msg_hash.as_ref() {
            return true;
        }
    }

    false
}

fn has_ed25519_preinstruction(
    ixs: &AccountInfo,
    ctm_signer: Pubkey,
    msg_hash: [u8; 32],
) -> Result<bool> {
    let current_index = tx_instructions::load_current_index_checked(ixs)? as usize;
    for index in 0..current_index {
        let ix = tx_instructions::load_instruction_at_checked(index, ixs)?;
        if ed25519_ix_matches(&ix, index, ctm_signer, msg_hash) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn verify_ed25519_preinstruction(
    ixs: &AccountInfo,
    signer: Pubkey,
    msg_hash: [u8; 32],
) -> Result<()> {
    let found = has_ed25519_preinstruction(ixs, signer, msg_hash)?;
    require!(found, MangoError::CtmSignatureMissing);
    Ok(())
}

fn verify_user_ed25519_preinstruction(
    ixs: &AccountInfo,
    signer: Pubkey,
    msg_hash: [u8; 32],
) -> Result<()> {
    let found = has_ed25519_preinstruction(ixs, signer, msg_hash)?;
    require!(found, MangoError::ExecutionQueueUserSignatureMissing);
    Ok(())
}

pub fn execution_queue_init(ctx: Context<ExecutionQueueInit>, ctm_signer: Pubkey) -> Result<()> {
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let buffer_ai = &ctx.remaining_accounts[ctx.remaining_accounts.len() - 1];
    let buffer_loader: AccountLoader<ExecutionQueueBuffer> =
        AccountLoader::try_from_unchecked(&crate::id(), buffer_ai)
            .map_err(|_| error!(MangoError::SomeError))?;
    let mut buffer = buffer_loader.load_init()?;
    let capacity = EXECUTION_QUEUE_CAPACITY as u32;
    buffer.init(ctx.accounts.execution_queue.key(), capacity);
    drop(buffer);
    {
        let mut data = buffer_ai.try_borrow_mut_data()?;
        data[..8].copy_from_slice(&ExecutionQueueBuffer::discriminator());
    }

    let mut queue = ctx.accounts.execution_queue.load_init()?;
    queue.init(
        ctx.accounts.group.key(),
        ctx.accounts.admin.key(),
        ctm_signer,
        buffer_ai.key(),
        capacity,
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
        envelope.kind == QueueItemKind::CtmWrapped as u8,
        MangoError::ExecutionQueueInvalidItemKind
    );
    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );

    require!(
        ctx.remaining_accounts.len() >= 2,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (without_buffer, buffer_tail) =
        ctx.remaining_accounts.split_at(ctx.remaining_accounts.len() - 1);
    let (dispatch_accounts, program_tail) = without_buffer.split_at(without_buffer.len() - 1);
    let dispatch_program_ai = &program_tail[0];
    require!(
        *dispatch_program_ai.key == crate::id(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let buffer_ai = &buffer_tail[0];
    let buffer_loader: AccountLoader<ExecutionQueueBuffer> =
        AccountLoader::try_from_unchecked(&crate::id(), buffer_ai)
            .map_err(|_| error!(MangoError::SomeError))?;

    let clock = Clock::get()?;
    let mut queue = ctx.accounts.execution_queue.load_mut()?;
    require!(
        queue.buffer == *buffer_ai.key,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let mut buffer = buffer_loader.load_mut()?;
    require!(
        buffer.execution_queue == ctx.accounts.execution_queue.key(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
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
        envelope.sequence >= queue.next_sequence_to_execute,
        MangoError::InvalidSequenceNumber
    );
    require!(
        !buffer
            .items[..queue.capacity as usize]
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

    if variant_uses_queue_owner_signer(decoded_payload.variant) {
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

    let slot = find_free_slot(&queue, &buffer).ok_or_else(|| error!(MangoError::ExecutionQueueFull))?;
    let item = &mut buffer.items[slot];
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
        ctx.remaining_accounts.len() >= 2,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (without_buffer, buffer_tail) =
        ctx.remaining_accounts.split_at(ctx.remaining_accounts.len() - 1);
    let (dispatch_accounts, program_tail) = without_buffer.split_at(without_buffer.len() - 1);
    let dispatch_program_ai = &program_tail[0];
    require!(
        *dispatch_program_ai.key == crate::id(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let buffer_ai = &buffer_tail[0];
    let buffer_loader: AccountLoader<ExecutionQueueBuffer> =
        AccountLoader::try_from_unchecked(&crate::id(), buffer_ai)
            .map_err(|_| error!(MangoError::SomeError))?;

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
        queue.buffer == *buffer_ai.key,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let mut buffer = buffer_loader.load_mut()?;
    require!(
        buffer.execution_queue == ctx.accounts.execution_queue.key(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    require!(
        queue.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    require!(!queue_is_full(&queue), MangoError::ExecutionQueueFull);

    let payload_hash = hashv(&[&payload]).to_bytes();
    let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
    let slot = find_free_slot(&queue, &buffer).ok_or_else(|| error!(MangoError::ExecutionQueueFull))?;
    let min_execute_slot = clock.slot + queue.liquidity_delay_slots;
    {
        let item = &mut buffer.items[slot];
        *item = QueueItem::default();
        item.sequence = 0;
        item.min_execute_slot = min_execute_slot;
        item.ingress_slot = clock.slot;
        item.kind = kind;
        item.status = QueueItemStatus::Pending as u8;
        item.payload_len = payload.len() as u16;
        item.payload_hash = payload_hash;
        item.accounts_hash = accounts_hash;
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
    require!(
        ctx.remaining_accounts.len() >= 2,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let (without_buffer, buffer_tail) =
        ctx.remaining_accounts.split_at(ctx.remaining_accounts.len() - 1);
    let (dispatch_accounts, program_tail) = without_buffer.split_at(without_buffer.len() - 1);
    let dispatch_program_ai = &program_tail[0];
    require!(
        *dispatch_program_ai.key == crate::id(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    let buffer_ai = &buffer_tail[0];
    let buffer_loader: AccountLoader<ExecutionQueueBuffer> =
        AccountLoader::try_from_unchecked(&crate::id(), buffer_ai)
            .map_err(|_| error!(MangoError::SomeError))?;

    let clock = Clock::get()?;
    let group_key = ctx.accounts.group.key();
    let execution_queue_key = ctx.accounts.execution_queue.key();
    let execution_queue_bump: u8;
    {
        let mut queue = ctx.accounts.execution_queue.load_mut()?;
        require!(
            queue.buffer == *buffer_ai.key,
            MangoError::ExecutionQueueDispatchAccountLayoutInvalid
        );
        let buffer = buffer_loader.load()?;
        require!(
            buffer.execution_queue == ctx.accounts.execution_queue.key(),
            MangoError::ExecutionQueueDispatchAccountLayoutInvalid
        );
        execution_queue_bump = queue.bump;
        queue.maybe_activate_pending_ctm(clock.slot);
        require!(
            queue.paused_execute == 0,
            MangoError::ExecutionQueueExecutePaused
        );
    }

    let provided_accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));

    for _ in 0..max_items {
        let mut candidate: Option<ExecutableCandidate> = None;
        let mut blocked_on_min_slot = false;

        {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let buffer = buffer_loader.load()?;
            let cap = (queue.capacity as usize).min(EXECUTION_QUEUE_CAPACITY);
            let next_seq = queue.next_sequence_to_execute;
            let ctm_index = buffer.items[..cap].iter().position(|it| {
                it.status == QueueItemStatus::Pending as u8
                    && it.kind == QueueItemKind::CtmWrapped as u8
                    && it.sequence == next_seq
            });

            if let Some(idx) = ctm_index {
                let item = &buffer.items[idx];
                if clock.slot < item.min_execute_slot {
                    blocked_on_min_slot = true;
                } else {
                    let payload_len = item.payload_len as usize;
                    candidate = Some(ExecutableCandidate {
                        idx,
                        sequence: item.sequence,
                        kind: item.kind,
                        payload: item.payload[..payload_len].to_vec(),
                        payload_hash: item.payload_hash,
                        accounts_hash: item.accounts_hash,
                        retries: item.retries,
                        is_ctm_lane: true,
                    });
                }
            } else {
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
                        queue.next_sequence_to_execute =
                            queue.next_sequence_to_execute.saturating_add(1);
                        queue.gap_observed_slot = 0;
                        continue;
                    }
                }

                let liq_index = buffer
                    .items[..cap]
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
                    let item = &buffer.items[idx];
                    let payload_len = item.payload_len as usize;
                    candidate = Some(ExecutableCandidate {
                        idx,
                        sequence: item.sequence,
                        kind: item.kind,
                        payload: item.payload[..payload_len].to_vec(),
                        payload_hash: item.payload_hash,
                        accounts_hash: item.accounts_hash,
                        retries: item.retries,
                        is_ctm_lane: false,
                    });
                }
            }
        }

        if blocked_on_min_slot {
            break;
        }

        let Some(candidate) = candidate else {
            break;
        };

        let computed_payload_hash = hashv(&[&candidate.payload]).to_bytes();
        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(p) => p,
            Err(_) => {
                let mut queue = ctx.accounts.execution_queue.load_mut()?;
                let mut buffer = buffer_loader.load_mut()?;
                let item = &mut buffer.items[candidate.idx];
                if item.status == QueueItemStatus::Pending as u8 {
                    *item = QueueItem::default();
                    queue.count = queue.count.saturating_sub(1);
                    if candidate.is_ctm_lane {
                        queue.next_sequence_to_execute =
                            queue.next_sequence_to_execute.saturating_add(1);
                        queue.gap_observed_slot = 0;
                    }
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
        if computed_payload_hash != candidate.payload_hash || payload_kind != candidate.kind {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let mut buffer = buffer_loader.load_mut()?;
            let item = &mut buffer.items[candidate.idx];
            if item.status == QueueItemStatus::Pending as u8 {
                *item = QueueItem::default();
                queue.count = queue.count.saturating_sub(1);
                if candidate.is_ctm_lane {
                    queue.next_sequence_to_execute =
                        queue.next_sequence_to_execute.saturating_add(1);
                    queue.gap_observed_slot = 0;
                }
            }
            emit!(QueueItemProcessed {
                group: ctx.accounts.group.key(),
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatus::Failed as u8,
            });
            continue;
        }

        if candidate.accounts_hash != [0; 32]
            && provided_accounts_hash != candidate.accounts_hash
        {
            // The caller provided a dispatch lane that doesn't match the head item's account hash.
            // Leave the queue untouched so another execute call with the correct lane can process it.
            break;
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            without_buffer,
            group_key,
            execution_queue_key,
            execution_queue_bump,
        );
        if dispatch_result.is_ok() {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let mut buffer = buffer_loader.load_mut()?;
            let item = &mut buffer.items[candidate.idx];
            if item.status == QueueItemStatus::Pending as u8 {
                *item = QueueItem::default();
                queue.count = queue.count.saturating_sub(1);
                if candidate.is_ctm_lane {
                    queue.next_sequence_to_execute =
                        queue.next_sequence_to_execute.saturating_add(1);
                    queue.gap_observed_slot = 0;
                }
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
        if next_retry >= EXECUTION_QUEUE_MAX_RETRIES {
            let mut queue = ctx.accounts.execution_queue.load_mut()?;
            let mut buffer = buffer_loader.load_mut()?;
            let item = &mut buffer.items[candidate.idx];
            if item.status == QueueItemStatus::Pending as u8 {
                *item = QueueItem::default();
                queue.count = queue.count.saturating_sub(1);
                if candidate.is_ctm_lane {
                    queue.next_sequence_to_execute =
                        queue.next_sequence_to_execute.saturating_add(1);
                    queue.gap_observed_slot = 0;
                }
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
        let mut buffer = buffer_loader.load_mut()?;
        let item = &mut buffer.items[candidate.idx];
        if item.status == QueueItemStatus::Pending as u8 {
            if item.first_failure_slot == 0 {
                item.first_failure_slot = clock.slot;
            }
            item.retries = next_retry;
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

        assert!(ed25519_ix_matches(&ix, 0, signer, msg_hash));
    }

    #[test]
    fn ed25519_match_rejects_non_self_referential_message_offsets() {
        let signer = Pubkey::new_unique();
        let msg_hash = [13u8; 32];
        let ix = test_ed25519_ix(signer, msg_hash, ED25519_CURRENT_INSTRUCTION_INDEX, 7);

        assert!(!ed25519_ix_matches(&ix, 0, signer, msg_hash));
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

        assert!(!ed25519_ix_matches(&ix, 0, signer, msg_hash));
    }
}
