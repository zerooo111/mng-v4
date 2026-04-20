use super::execution_queue::{
    account_metas_from_infos, canonical_direct_dispatch_account_metas, canonical_envelope_message,
    canonical_user_intent_message_v1, canonical_user_intent_message_v2, decode_queue_payload,
    dispatch_queue_payload, extract_user_owner_for_ctm_payload, hash_accounts,
    prevalidate_terminal_ctm_payload, queue_health_region_begin, queue_health_region_end,
    queue_health_region_spec, queue_item_kind_for_payload_variant, require_dispatch_market_index,
    split_dispatch_accounts, terminal_ctm_failure_msg, validate_queue_payload_dispatch_accounts,
    variant_uses_user_signature, verify_ed25519_preinstruction, verify_user_ed25519_preinstruction,
    CtmEnvelope, DecodedQueuePayload, QueueItemEnqueued, QueueItemProcessed, QueuePayloadBody,
    UserIntentTargetKind, DIRECT_SUBMIT_DELAY_SLOTS, EXECUTION_QUEUE_GAP_SKIP_LIMIT_PER_EXECUTE,
    EXECUTION_QUEUE_MAX_RETRIES,
};
use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::system_instruction;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV3MarketRootCreateParams {
    pub page_size: u16,
    pub num_pages: u16,
    pub soft_limit: u16,
    pub recipe_version: u16,
    pub gap_wait_slots: u16,
    pub max_compaction_distance: u8,
    pub min_expiry_buffer_slots: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV3MarketRootConfigParams {
    pub soft_limit: u16,
    pub recipe_version: u16,
    pub gap_wait_slots: u16,
    pub max_compaction_distance: u8,
    pub min_expiry_buffer_slots: u8,
    pub pause_ingress: bool,
    pub pause_execute: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV3LiquidityRootCreateParams {
    pub page_size: u16,
    pub num_pages: u16,
    pub liquidity_delay_slots: u16,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ExecutionQueueV3LiquidityRootConfigParams {
    pub liquidity_delay_slots: u16,
    pub pause_ingress: bool,
    pub pause_execute: bool,
}

fn validate_page_geometry(page_size: u16, num_pages: u16, soft_limit: u16) -> Result<u32> {
    require!(page_size > 0, MangoError::ExecutionQueueV3InvalidPageSize);
    require!(
        page_size as usize <= EXECUTION_QUEUE_V3_MAX_PAGE_SIZE,
        MangoError::ExecutionQueueV3InvalidPageSize
    );
    require!(num_pages > 0, MangoError::ExecutionQueueV3InvalidNumPages);
    require!(
        num_pages <= EXECUTION_QUEUE_V3_MAX_NUM_PAGES,
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

fn validate_page_assignment(
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

fn resize_execution_queue_page_v3<'info>(
    queue_page: &UncheckedAccount<'info>,
    payer: &Signer<'info>,
    system_program: &Program<'info, System>,
) -> Result<()> {
    let queue_page_ai = queue_page.to_account_info();
    let current_len = queue_page_ai.data_len();
    let target_len = EXECUTION_QUEUE_PAGE_V3_SPACE;

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

fn place_order_match_kind(order_type: PlaceOrderType) -> u8 {
    match order_type {
        PlaceOrderType::Market => QueueItemMatchKindV3::Market as u8,
        PlaceOrderType::ImmediateOrCancel => QueueItemMatchKindV3::Ioc as u8,
        PlaceOrderType::Limit => QueueItemMatchKindV3::Limit as u8,
        PlaceOrderType::PostOnly | PlaceOrderType::PostOnlySlide => {
            QueueItemMatchKindV3::PostOnly as u8
        }
    }
}

fn side_match_side(side: Side) -> u8 {
    match side {
        Side::Bid => QueueItemMatchSideV3::Bid as u8,
        Side::Ask => QueueItemMatchSideV3::Ask as u8,
    }
}

fn aggressive_limit_price_lots(side: Side, order_type: PlaceOrderType, price_lots: i64) -> i64 {
    match order_type {
        PlaceOrderType::Market => match side {
            Side::Bid => i64::MAX,
            Side::Ask => i64::MIN,
        },
        _ => price_lots,
    }
}

fn set_canonical_perp_account_recipe(item: &mut QueueItemV3, market_index: u16) {
    item.recipe_kind = QueueAccountRecipeKindV3::CanonicalPerpV1 as u8;
    item.recipe_len = EXECUTION_QUEUE_V3_CANONICAL_PERP_ACCOUNT_RECIPE_V1_LEN;
    item.account_recipe = CanonicalPerpAccountRecipeV3::new(market_index, item.op_class).encode();
}

fn populate_market_item_metadata(
    item: &mut QueueItemV3,
    decoded_payload: &DecodedQueuePayload,
    market_index: u16,
    mango_account_hint: Pubkey,
) -> Result<()> {
    item.market_index = market_index;
    item.mango_account_hint = mango_account_hint;
    require!(
        mango_account_hint != Pubkey::default(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    match &decoded_payload.body {
        QueuePayloadBody::PerpPlaceOrderV2(place) => {
            let mut flags =
                queue_item_flags_v3::HEALTH_GATED | queue_item_flags_v3::PRECHECK_REQUIRED;
            if !matches!(
                place.order_type,
                PlaceOrderType::PostOnly | PlaceOrderType::PostOnlySlide
            ) {
                flags |= queue_item_flags_v3::AGGRESSIVE_CANDIDATE;
            }
            item.flags = flags;
            item.op_class = QueueItemOpClassV3::GenericPerpPlace as u8;
            item.match_kind = place_order_match_kind(place.order_type);
            item.match_side = side_match_side(place.side);
            item.match_limit_price_lots =
                aggressive_limit_price_lots(place.side, place.order_type, place.price_lots);
            set_canonical_perp_account_recipe(item, market_index);
        }
        QueuePayloadBody::PerpCancelOrder(_) => {
            item.op_class = QueueItemOpClassV3::GenericCancel as u8;
            set_canonical_perp_account_recipe(item, market_index);
        }
        QueuePayloadBody::PerpCancelOrderByClientOrderId(_) => {
            item.op_class = QueueItemOpClassV3::GenericCancelByClientOrderId as u8;
            set_canonical_perp_account_recipe(item, market_index);
        }
        QueuePayloadBody::PerpCancelAllOrders(_)
        | QueuePayloadBody::PerpCancelAllOrdersBySide(_) => {
            item.op_class = QueueItemOpClassV3::GenericCancelAll as u8;
            set_canonical_perp_account_recipe(item, market_index);
        }
        QueuePayloadBody::LiquidityDeposit(_) | QueuePayloadBody::LiquidityWithdraw(_) => {}
    }
    Ok(())
}

fn liquidity_op_class(kind: u8) -> u8 {
    match kind {
        k if k == QueueItemKind::LiquidityDeposit as u8 => {
            QueueItemOpClassV3::LiquidityDeposit as u8
        }
        k if k == QueueItemKind::LiquidityWithdraw as u8 => {
            QueueItemOpClassV3::LiquidityWithdraw as u8
        }
        _ => QueueItemOpClassV3::LiquidityDeposit as u8,
    }
}

fn write_common_item_fields(
    item: &mut QueueItemV3,
    sequence: u64,
    min_execute_slot: u64,
    ingress_slot: u64,
    expires_at_slot: u64,
    kind: u8,
    payload_hash: [u8; 32],
    accounts_hash: [u8; 32],
    payload: &[u8],
) {
    item.sequence = sequence;
    item.min_execute_slot = min_execute_slot;
    item.ingress_slot = ingress_slot;
    item.expires_at_slot = expires_at_slot;
    item.kind = kind;
    item.status = QueueItemStatusV3::Pending as u8;
    item.payload_hash = payload_hash;
    item.accounts_hash = accounts_hash;
    item.payload_len = payload.len() as u16;
    item.payload[..payload.len()].copy_from_slice(payload);
}

#[derive(Clone, Debug)]
struct ExecutableQueueItemV3 {
    sequence: u64,
    kind: u8,
    payload: Vec<u8>,
    accounts_hash: [u8; 32],
    retries: u8,
    min_execute_slot: u64,
    expires_at_slot: u64,
}

fn pending_head_item_on_page(
    queue_page: &ExecutionQueuePageV3,
    page_size: u16,
    sequence: u64,
) -> Option<QueueItemV3> {
    let offset = (sequence % page_size as u64) as usize;
    let item = queue_page.items.get(offset)?.clone();
    if item.status == QueueItemStatusV3::Pending as u8 && item.sequence == sequence {
        Some(item)
    } else {
        None
    }
}

fn normalize_market_head_within_page(
    queue_root: &mut PerpMarketQueueRootV3,
    queue_page: &ExecutionQueuePageV3,
) {
    while queue_root.live_count > 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
        && queue_root.head_abs_page_no() == queue_page.assigned_abs_page_no
    {
        let offset = queue_root.head_page_offset() as usize;
        let head = &queue_page.items[offset];
        if head.status == QueueItemStatusV3::Pending as u8
            && head.sequence == queue_root.next_sequence_to_execute
        {
            break;
        }
        queue_root.next_sequence_to_execute = queue_root.next_sequence_to_execute.saturating_add(1);
        queue_root.gap_observed_slot = 0;
    }

    if queue_root.live_count == 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
    {
        queue_root.next_sequence_to_execute = queue_root.max_seen_sequence.saturating_add(1);
        queue_root.gap_observed_slot = 0;
    }
}

fn clear_market_head_and_advance(
    queue_root: &mut PerpMarketQueueRootV3,
    queue_page: &mut ExecutionQueuePageV3,
) -> Result<()> {
    let offset = queue_root.head_page_offset();
    queue_page.clear_item(queue_root.page_size, offset)?;
    queue_root.live_count = queue_root.live_count.saturating_sub(1);
    queue_root.next_sequence_to_execute = queue_root.next_sequence_to_execute.saturating_add(1);
    queue_root.gap_observed_slot = 0;
    normalize_market_head_within_page(queue_root, queue_page);
    Ok(())
}

fn skip_market_head_gaps(
    queue_root: &mut PerpMarketQueueRootV3,
    queue_page: &ExecutionQueuePageV3,
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
        if head.status == QueueItemStatusV3::Pending as u8 && head.sequence == next_seq {
            break;
        }

        emit!(QueueItemProcessed {
            group,
            market_index,
            sequence: next_seq,
            kind: QueueItemKind::CtmWrapped as u8,
            status: QueueItemStatusV3::Skipped as u8,
            failure_code: 0,
        });
        queue_root.next_sequence_to_execute = next_seq.saturating_add(1);
        queue_root.gap_observed_slot = 0;
        skipped = skipped.saturating_add(1);
    }

    normalize_market_head_within_page(queue_root, queue_page);
}

fn normalize_liquidity_head_within_page(
    queue_root: &mut LiquidityQueueRootV3,
    queue_page: &ExecutionQueuePageV3,
) {
    while queue_root.live_count > 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
        && queue_root.head_abs_page_no() == queue_page.assigned_abs_page_no
    {
        let offset = queue_root.head_page_offset() as usize;
        let head = &queue_page.items[offset];
        if head.status == QueueItemStatusV3::Pending as u8
            && head.sequence == queue_root.next_sequence_to_execute
        {
            break;
        }
        queue_root.next_sequence_to_execute = queue_root.next_sequence_to_execute.saturating_add(1);
        queue_root.gap_observed_slot = 0;
    }

    if queue_root.live_count == 0
        && queue_root.max_seen_sequence >= queue_root.next_sequence_to_execute
    {
        queue_root.next_sequence_to_execute = queue_root.max_seen_sequence.saturating_add(1);
        queue_root.gap_observed_slot = 0;
    }
}

fn clear_liquidity_head_and_advance(
    queue_root: &mut LiquidityQueueRootV3,
    queue_page: &mut ExecutionQueuePageV3,
) -> Result<()> {
    let offset = queue_root.head_page_offset();
    queue_page.clear_item(queue_root.page_size, offset)?;
    queue_root.live_count = queue_root.live_count.saturating_sub(1);
    queue_root.next_sequence_to_execute = queue_root.next_sequence_to_execute.saturating_add(1);
    queue_root.gap_observed_slot = 0;
    normalize_liquidity_head_within_page(queue_root, queue_page);
    Ok(())
}

fn skip_liquidity_head_gaps(
    queue_root: &mut LiquidityQueueRootV3,
    queue_page: &ExecutionQueuePageV3,
    _group: Pubkey,
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
        if head.status == QueueItemStatusV3::Pending as u8 && head.sequence == next_seq {
            break;
        }
        queue_root.next_sequence_to_execute = next_seq.saturating_add(1);
        queue_root.gap_observed_slot = 0;
        skipped = skipped.saturating_add(1);
    }

    normalize_liquidity_head_within_page(queue_root, queue_page);
}

pub fn execution_queue_v3_init_authority_state(
    ctx: Context<ExecutionQueueV3InitAuthorityState>,
    ctm_signer: Pubkey,
) -> Result<()> {
    let bump = *ctx
        .bumps
        .get("authority_state")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let authority_state = &mut ctx.accounts.authority_state;
    authority_state.init(
        ctx.accounts.group.key(),
        ctx.accounts.admin.key(),
        ctm_signer,
        bump,
    );
    Ok(())
}

pub fn execution_queue_v3_set_ctm_pending(
    ctx: Context<ExecutionQueueV3AuthorityAdmin>,
    pending_ctm_signer: Pubkey,
    activate_at_slot: u64,
) -> Result<()> {
    let authority_state = &mut ctx.accounts.authority_state;
    authority_state.set_pending_ctm_signer(pending_ctm_signer, activate_at_slot);
    Ok(())
}

pub fn execution_queue_v3_init_market_root(
    ctx: Context<ExecutionQueueV3InitMarketRoot>,
    market_index: u16,
    shard_id: u8,
    params: ExecutionQueueV3MarketRootCreateParams,
) -> Result<()> {
    require!(
        shard_id == 0,
        MangoError::ExecutionQueueV3UnsupportedShardId
    );
    validate_page_geometry(params.page_size, params.num_pages, params.soft_limit)?;

    let bump = *ctx
        .bumps
        .get("queue_root")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let queue_root = &mut ctx.accounts.queue_root;
    queue_root.init(
        ctx.accounts.group.key(),
        ctx.accounts.authority_state.key(),
        market_index,
        shard_id,
        bump,
        params.page_size,
        params.num_pages,
        params.soft_limit,
        params.recipe_version,
        params.gap_wait_slots,
        params.max_compaction_distance,
        params.min_expiry_buffer_slots,
    );
    Ok(())
}

pub fn execution_queue_v3_configure_market_root(
    ctx: Context<ExecutionQueueV3MarketRootAdmin>,
    params: ExecutionQueueV3MarketRootConfigParams,
) -> Result<()> {
    let queue_root = &mut ctx.accounts.queue_root;
    validate_page_geometry(
        queue_root.page_size,
        queue_root.num_pages,
        params.soft_limit,
    )?;
    queue_root.soft_limit = params.soft_limit;
    queue_root.recipe_version = params.recipe_version;
    queue_root.gap_wait_slots = params.gap_wait_slots;
    queue_root.max_compaction_distance = params.max_compaction_distance;
    queue_root.min_expiry_buffer_slots = params.min_expiry_buffer_slots;
    queue_root.paused_ingress = u8::from(params.pause_ingress);
    queue_root.paused_execute = u8::from(params.pause_execute);
    Ok(())
}

pub fn execution_queue_v3_init_liquidity_root(
    ctx: Context<ExecutionQueueV3InitLiquidityRoot>,
    params: ExecutionQueueV3LiquidityRootCreateParams,
) -> Result<()> {
    validate_page_geometry(params.page_size, params.num_pages, 0)?;

    let bump = *ctx
        .bumps
        .get("queue_root")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let queue_root = &mut ctx.accounts.queue_root;
    queue_root.init(
        ctx.accounts.group.key(),
        ctx.accounts.authority_state.key(),
        bump,
        params.page_size,
        params.num_pages,
        params.liquidity_delay_slots,
    );
    Ok(())
}

pub fn execution_queue_v3_configure_liquidity_root(
    ctx: Context<ExecutionQueueV3LiquidityRootAdmin>,
    params: ExecutionQueueV3LiquidityRootConfigParams,
) -> Result<()> {
    let queue_root = &mut ctx.accounts.queue_root;
    queue_root.liquidity_delay_slots = params.liquidity_delay_slots;
    queue_root.paused_ingress = u8::from(params.pause_ingress);
    queue_root.paused_execute = u8::from(params.pause_execute);
    Ok(())
}

pub fn execution_queue_v3_create_market_page(
    _ctx: Context<ExecutionQueueV3CreateMarketPage>,
    _page_slot: u16,
) -> Result<()> {
    Ok(())
}

pub fn execution_queue_v3_resize_market_page(
    ctx: Context<ExecutionQueueV3ResizeMarketPage>,
    _page_slot: u16,
) -> Result<()> {
    resize_execution_queue_page_v3(
        &ctx.accounts.queue_page,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
    )
}

pub fn execution_queue_v3_init_market_page(
    ctx: Context<ExecutionQueueV3InitMarketPage>,
    page_slot: u16,
    assigned_abs_page_no: u64,
) -> Result<()> {
    let queue_root = &ctx.accounts.queue_root;
    validate_page_assignment(page_slot, assigned_abs_page_no, queue_root.num_pages)?;

    let bump = *ctx
        .bumps
        .get("queue_page")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let mut queue_page = ctx.accounts.queue_page.load_init()?;
    queue_page.init(
        ctx.accounts.queue_root.key(),
        page_slot,
        assigned_abs_page_no,
        queue_root.page_size,
        bump,
    );
    Ok(())
}

pub fn execution_queue_v3_create_liquidity_page(
    _ctx: Context<ExecutionQueueV3CreateLiquidityPage>,
    _page_slot: u16,
) -> Result<()> {
    Ok(())
}

pub fn execution_queue_v3_resize_liquidity_page(
    ctx: Context<ExecutionQueueV3ResizeLiquidityPage>,
    _page_slot: u16,
) -> Result<()> {
    resize_execution_queue_page_v3(
        &ctx.accounts.queue_page,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
    )
}

pub fn execution_queue_v3_init_liquidity_page(
    ctx: Context<ExecutionQueueV3InitLiquidityPage>,
    page_slot: u16,
    assigned_abs_page_no: u64,
) -> Result<()> {
    let queue_root = &ctx.accounts.queue_root;
    validate_page_assignment(page_slot, assigned_abs_page_no, queue_root.num_pages)?;

    let bump = *ctx
        .bumps
        .get("queue_page")
        .ok_or_else(|| error!(MangoError::SomeError))?;
    let mut queue_page = ctx.accounts.queue_page.load_init()?;
    queue_page.init(
        ctx.accounts.queue_root.key(),
        page_slot,
        assigned_abs_page_no,
        queue_root.page_size,
        bump,
    );
    Ok(())
}

pub fn execution_queue_v3_close_market_page(
    ctx: Context<ExecutionQueueV3CloseMarketPage>,
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

pub fn execution_queue_v3_close_liquidity_page(
    ctx: Context<ExecutionQueueV3CloseLiquidityPage>,
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

pub fn execution_queue_v3_enqueue_market(
    ctx: Context<ExecutionQueueV3EnqueueMarket>,
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

    let queue_root_key = ctx.accounts.queue_root.key();
    let (dispatch_accounts, _) = split_dispatch_accounts(ctx.remaining_accounts, queue_root_key)?;
    require!(
        ctx.accounts.queue_root.market_index == market_index,
        MangoError::ExecutionQueueSubQueueMarketIndexMismatch
    );
    require!(
        ctx.accounts.queue_root.shard_id == 0,
        MangoError::ExecutionQueueV3UnsupportedShardId
    );
    require_dispatch_market_index(dispatch_accounts, market_index)?;

    let clock = Clock::get()?;
    ctx.accounts
        .authority_state
        .maybe_activate_pending_ctm(clock.slot);
    require!(
        ctx.accounts.queue_root.paused_ingress == 0,
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
    validate_queue_payload_dispatch_accounts(
        ctx.accounts.group.key(),
        &decoded_payload,
        dispatch_accounts,
    )?;

    let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
    require!(
        accounts_hash == envelope.accounts_hash,
        MangoError::ExecutionQueueAccountsHashMismatch
    );
    require!(
        envelope.expires_at_slot == 0 || clock.slot <= envelope.expires_at_slot,
        MangoError::ExecutionQueueEnvelopeExpired
    );

    ctx.accounts
        .queue_root
        .validate_enqueue_sequence(envelope.sequence)?;
    let abs_page_no = ctx
        .accounts
        .queue_root
        .abs_page_no_for_sequence(envelope.sequence);
    let page_slot = ctx
        .accounts
        .queue_root
        .page_slot_for_sequence(envelope.sequence);
    let page_offset = ctx
        .accounts
        .queue_root
        .page_offset_for_sequence(envelope.sequence);
    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.prepare_for_write_target(
            queue_root_key,
            page_slot,
            abs_page_no,
            ctx.accounts.queue_root.page_size,
        )?;
    }

    let msg_hash = canonical_envelope_message(ctx.accounts.group.key(), &envelope);
    verify_ed25519_preinstruction(
        ctx.accounts.instructions.as_ref(),
        ctx.accounts.authority_state.ctm_signer,
        msg_hash.as_ref(),
    )?;

    let mut mango_account_hint = Pubkey::default();
    if variant_uses_user_signature(decoded_payload.variant) {
        let (mango_account_key, user_owner) =
            extract_user_owner_for_ctm_payload(ctx.accounts.group.key(), dispatch_accounts)?;
        mango_account_hint = mango_account_key;
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

    let mut item = QueueItemV3::default();
    write_common_item_fields(
        &mut item,
        envelope.sequence,
        envelope.min_execute_slot,
        clock.slot,
        envelope.expires_at_slot,
        envelope.kind,
        envelope.payload_hash,
        envelope.accounts_hash,
        &payload,
    );
    populate_market_item_metadata(
        &mut item,
        &decoded_payload,
        market_index,
        mango_account_hint,
    )?;
    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.write_pending_item(ctx.accounts.queue_root.page_size, page_offset, item)?;
    }
    ctx.accounts.queue_root.note_enqueue(envelope.sequence);

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index,
        sequence: envelope.sequence,
        kind: envelope.kind,
        min_execute_slot: envelope.min_execute_slot,
    });
    Ok(())
}

pub fn execution_queue_v3_enqueue_market_direct(
    ctx: Context<ExecutionQueueV3EnqueueMarket>,
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

    let queue_root_key = ctx.accounts.queue_root.key();
    let market_index = ctx.accounts.queue_root.market_index;
    let (dispatch_accounts, _) = split_dispatch_accounts(ctx.remaining_accounts, queue_root_key)?;
    require_dispatch_market_index(dispatch_accounts, market_index)?;

    let clock = Clock::get()?;
    require!(
        ctx.accounts.queue_root.paused_ingress == 0,
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
    let accounts_hash = hash_accounts(&canonical_direct_dispatch_account_metas(
        ctx.accounts.group.key(),
        queue_root_key,
        &dispatch_account_metas,
        user_signature_keys.map(|(_, user_owner)| user_owner),
    ));
    require!(
        accounts_hash == envelope.accounts_hash,
        MangoError::ExecutionQueueAccountsHashMismatch
    );
    require!(
        envelope.expires_at_slot == 0 || clock.slot <= envelope.expires_at_slot,
        MangoError::ExecutionQueueEnvelopeExpired
    );

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

    let sequence = ctx.accounts.queue_root.next_enqueue_sequence();
    ctx.accounts
        .queue_root
        .validate_enqueue_sequence(sequence)?;
    let abs_page_no = ctx.accounts.queue_root.abs_page_no_for_sequence(sequence);
    let page_slot = ctx.accounts.queue_root.page_slot_for_sequence(sequence);
    let page_offset = ctx.accounts.queue_root.page_offset_for_sequence(sequence);
    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.prepare_for_write_target(
            queue_root_key,
            page_slot,
            abs_page_no,
            ctx.accounts.queue_root.page_size,
        )?;
    }

    let min_execute_slot = envelope
        .min_execute_slot
        .max(clock.slot.saturating_add(DIRECT_SUBMIT_DELAY_SLOTS));
    let mut item = QueueItemV3::default();
    write_common_item_fields(
        &mut item,
        sequence,
        min_execute_slot,
        clock.slot,
        envelope.expires_at_slot,
        envelope.kind,
        payload_hash,
        accounts_hash,
        &payload,
    );
    populate_market_item_metadata(
        &mut item,
        &decoded_payload,
        market_index,
        user_signature_keys
            .map(|(mango_account_key, _)| mango_account_key)
            .unwrap_or_default(),
    )?;
    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.write_pending_item(ctx.accounts.queue_root.page_size, page_offset, item)?;
    }
    ctx.accounts.queue_root.note_enqueue(sequence);

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index,
        sequence,
        kind: envelope.kind,
        min_execute_slot,
    });
    Ok(())
}

pub fn execution_queue_v3_enqueue_liquidity(
    ctx: Context<ExecutionQueueV3EnqueueLiquidity>,
    kind: u8,
    payload: Vec<u8>,
) -> Result<()> {
    require!(
        payload.len() <= EXECUTION_QUEUE_PAYLOAD_MAX,
        MangoError::ExecutionQueuePayloadTooLarge
    );
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );
    require!(
        kind == QueueItemKind::LiquidityDeposit as u8
            || kind == QueueItemKind::LiquidityWithdraw as u8,
        MangoError::ExecutionQueueInvalidItemKind
    );

    let queue_root_key = ctx.accounts.queue_root.key();
    let (dispatch_accounts, _) = split_dispatch_accounts(ctx.remaining_accounts, queue_root_key)?;
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
    require!(
        ctx.accounts.queue_root.paused_ingress == 0,
        MangoError::ExecutionQueueIngressPaused
    );
    let sequence = ctx.accounts.queue_root.next_enqueue_sequence();
    ctx.accounts
        .queue_root
        .validate_enqueue_sequence(sequence)?;
    let abs_page_no = ctx.accounts.queue_root.abs_page_no_for_sequence(sequence);
    let page_slot = ctx.accounts.queue_root.page_slot_for_sequence(sequence);
    let page_offset = ctx.accounts.queue_root.page_offset_for_sequence(sequence);
    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.prepare_for_write_target(
            queue_root_key,
            page_slot,
            abs_page_no,
            ctx.accounts.queue_root.page_size,
        )?;
    }

    let payload_hash = hashv(&[&payload]).to_bytes();
    let accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));
    let min_execute_slot = clock
        .slot
        .saturating_add(ctx.accounts.queue_root.liquidity_delay_slots as u64);

    let mut item = QueueItemV3::default();
    write_common_item_fields(
        &mut item,
        sequence,
        min_execute_slot,
        clock.slot,
        0,
        kind,
        payload_hash,
        accounts_hash,
        &payload,
    );
    item.market_index = u16::MAX;
    item.op_class = liquidity_op_class(kind);
    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.write_pending_item(ctx.accounts.queue_root.page_size, page_offset, item)?;
    }
    ctx.accounts.queue_root.note_enqueue(sequence);

    emit!(QueueItemEnqueued {
        group: ctx.accounts.group.key(),
        market_index: u16::MAX,
        sequence,
        kind,
        min_execute_slot,
    });
    Ok(())
}

pub fn execution_queue_v3_execute_market(
    ctx: Context<ExecutionQueueV3ExecuteMarket>,
    max_items: u16,
) -> Result<()> {
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let queue_root_key = ctx.accounts.queue_root.key();
    let (dispatch_accounts, invoke_accounts) =
        split_dispatch_accounts(ctx.remaining_accounts, queue_root_key)?;
    let market_index = ctx.accounts.queue_root.market_index;
    require_dispatch_market_index(dispatch_accounts, market_index)?;

    let clock = Clock::get()?;
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    let group_key = ctx.accounts.group.key();
    require!(
        ctx.accounts.queue_root.paused_execute == 0,
        MangoError::ExecutionQueueExecutePaused
    );

    let provided_accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));

    for _ in 0..max_items {
        if ctx.accounts.queue_root.live_count == 0 {
            break;
        }

        let head_abs_page_no = ctx.accounts.queue_root.head_abs_page_no();
        let head_page_slot = ctx.accounts.queue_root.head_page_slot();
        let next_sequence = ctx.accounts.queue_root.next_sequence_to_execute;
        let page_size = ctx.accounts.queue_root.page_size;
        let gap_wait_slots = ctx.accounts.queue_root.gap_wait_slots as u64;

        {
            let queue_page = ctx.accounts.queue_page.load()?;
            require!(
                queue_page.page_state == QueuePageStateV3::Active as u8,
                MangoError::ExecutionQueueV3PageInactive
            );
            if queue_page.page_slot != head_page_slot
                || queue_page.assigned_abs_page_no != head_abs_page_no
            {
                break;
            }
            queue_page.validate_target(queue_root_key, head_page_slot, head_abs_page_no)?;
        }

        let head_item = {
            let queue_page = ctx.accounts.queue_page.load()?;
            pending_head_item_on_page(&queue_page, page_size, next_sequence)
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
                skip_market_head_gaps(&mut ctx.accounts.queue_root, &queue_page, group_key, market_index);
                continue;
            }
            break;
        };

        if clock.slot < head_item.min_execute_slot {
            break;
        }

        let candidate = ExecutableQueueItemV3 {
            sequence: head_item.sequence,
            kind: head_item.kind,
            payload: head_item.payload[..head_item.payload_len as usize].to_vec(),
            accounts_hash: head_item.accounts_hash,
            retries: head_item.retries,
            min_execute_slot: head_item.min_execute_slot,
            expires_at_slot: head_item.expires_at_slot,
        };

        require!(
            candidate.min_execute_slot <= clock.slot,
            MangoError::SomeError
        );

        if candidate.expires_at_slot != 0 && clock.slot > candidate.expires_at_slot {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        require!(
            candidate.accounts_hash == [0; 32] || provided_accounts_hash == candidate.accounts_hash,
            MangoError::ExecutionQueueExecuteAccountsHashMismatch
        );

        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(payload) => payload,
            Err(_) => {
                {
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatusV3::Failed as u8,
                    failure_code: 0,
                });
                continue;
            }
        };

        if queue_item_kind_for_payload_variant(decoded_payload.variant) != candidate.kind {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let item_health_region = queue_health_region_spec(decoded_payload.variant);
        if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
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

        if item_health_region.is_some() && candidate.retries >= EXECUTION_QUEUE_MAX_RETRIES {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        if let Some(spec) = item_health_region {
            if queue_health_region_begin(dispatch_accounts, spec).is_err() {
                let retries = {
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    queue_page.increment_retry(
                        ctx.accounts.queue_root.page_size,
                        ctx.accounts.queue_root.head_page_offset(),
                        clock.slot,
                    )?
                };
                if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                    {
                        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                        clear_market_head_and_advance(
                            &mut ctx.accounts.queue_root,
                            &mut queue_page,
                        )?;
                    }
                    emit!(QueueItemProcessed {
                        group: group_key,
                        market_index,
                        sequence: candidate.sequence,
                        kind: candidate.kind,
                        status: QueueItemStatusV3::Failed as u8,
                        failure_code: 0,
                    });
                    continue;
                }
                break;
            }
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            invoke_accounts,
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
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Executed as u8,
                failure_code: 0,
            });
            continue;
        }

        if item_health_region.is_some() {
            return dispatch_result;
        }

        let retries = {
            let mut queue_page = ctx.accounts.queue_page.load_mut()?;
            queue_page.increment_retry(
                ctx.accounts.queue_root.page_size,
                ctx.accounts.queue_root.head_page_offset(),
                clock.slot,
            )?
        };
        if retries >= EXECUTION_QUEUE_MAX_RETRIES {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }
        break;
    }

    Ok(())
}

pub fn execution_queue_v3_execute_market_multi(
    ctx: Context<ExecutionQueueV3ExecuteMarket>,
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

    let market_index = ctx.accounts.queue_root.market_index;
    let clock = Clock::get()?;
    let now_ts: u64 = clock.unix_timestamp.try_into().unwrap_or(0);
    let group_key = ctx.accounts.group.key();
    let queue_root_key = ctx.accounts.queue_root.key();
    require!(
        ctx.accounts.queue_root.paused_execute == 0,
        MangoError::ExecutionQueueExecutePaused
    );

    let lane_slices: Vec<&[AccountInfo]> = (0..lane_count)
        .map(|i| &ctx.remaining_accounts[i * apl..(i + 1) * apl])
        .collect();

    require_dispatch_market_index(lane_slices[0], market_index)?;
    for lane in lane_slices.iter().skip(1) {
        require_dispatch_market_index(lane, market_index)?;
    }
    require!(
        lane_hashes.len() == lane_count,
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let precomputed_lane_hashes: Vec<[u8; 32]> = lane_slices
        .iter()
        .map(|lane| hash_accounts(&account_metas_from_infos(lane)))
        .collect();

    for _ in 0..max_items {
        if ctx.accounts.queue_root.live_count == 0 {
            break;
        }

        let head_abs_page_no = ctx.accounts.queue_root.head_abs_page_no();
        let head_page_slot = ctx.accounts.queue_root.head_page_slot();
        let next_sequence = ctx.accounts.queue_root.next_sequence_to_execute;
        let page_size = ctx.accounts.queue_root.page_size;
        let gap_wait_slots = ctx.accounts.queue_root.gap_wait_slots as u64;

        {
            let queue_page = ctx.accounts.queue_page.load()?;
            require!(
                queue_page.page_state == QueuePageStateV3::Active as u8,
                MangoError::ExecutionQueueV3PageInactive
            );
            if queue_page.page_slot != head_page_slot
                || queue_page.assigned_abs_page_no != head_abs_page_no
            {
                break;
            }
            queue_page.validate_target(queue_root_key, head_page_slot, head_abs_page_no)?;
        }

        let head_item = {
            let queue_page = ctx.accounts.queue_page.load()?;
            pending_head_item_on_page(&queue_page, page_size, next_sequence)
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
                skip_market_head_gaps(&mut ctx.accounts.queue_root, &queue_page, group_key, market_index);
                continue;
            }
            break;
        };

        if clock.slot < head_item.min_execute_slot {
            break;
        }

        let candidate = ExecutableQueueItemV3 {
            sequence: head_item.sequence,
            kind: head_item.kind,
            payload: head_item.payload[..head_item.payload_len as usize].to_vec(),
            accounts_hash: head_item.accounts_hash,
            retries: head_item.retries,
            min_execute_slot: head_item.min_execute_slot,
            expires_at_slot: head_item.expires_at_slot,
        };

        require!(
            candidate.min_execute_slot <= clock.slot,
            MangoError::SomeError
        );

        if candidate.expires_at_slot != 0 && clock.slot > candidate.expires_at_slot {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let matched_lane = if candidate.accounts_hash == [0; 32] {
            Some(0usize)
        } else {
            precomputed_lane_hashes
                .iter()
                .position(|hash| *hash == candidate.accounts_hash)
        };
        let Some(lane_idx) = matched_lane else {
            break;
        };
        let dispatch_accounts = lane_slices[lane_idx];

        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(payload) => payload,
            Err(_) => {
                {
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatusV3::Failed as u8,
                    failure_code: 0,
                });
                continue;
            }
        };

        if queue_item_kind_for_payload_variant(decoded_payload.variant) != candidate.kind {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let item_health_region = queue_health_region_spec(decoded_payload.variant);
        if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded_payload, now_ts) {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
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

        if item_health_region.is_some() && candidate.retries >= EXECUTION_QUEUE_MAX_RETRIES {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        if let Some(spec) = item_health_region {
            if queue_health_region_begin(dispatch_accounts, spec).is_err() {
                let retries = {
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    queue_page.increment_retry(
                        ctx.accounts.queue_root.page_size,
                        ctx.accounts.queue_root.head_page_offset(),
                        clock.slot,
                    )?
                };
                if retries >= EXECUTION_QUEUE_MAX_RETRIES {
                    {
                        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                        clear_market_head_and_advance(
                            &mut ctx.accounts.queue_root,
                            &mut queue_page,
                        )?;
                    }
                    emit!(QueueItemProcessed {
                        group: group_key,
                        market_index,
                        sequence: candidate.sequence,
                        kind: candidate.kind,
                        status: QueueItemStatusV3::Failed as u8,
                        failure_code: 0,
                    });
                    continue;
                }
                break;
            }
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            dispatch_accounts,
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
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Executed as u8,
                failure_code: 0,
            });
            continue;
        }

        if item_health_region.is_some() {
            return dispatch_result;
        }

        let retries = {
            let mut queue_page = ctx.accounts.queue_page.load_mut()?;
            queue_page.increment_retry(
                ctx.accounts.queue_root.page_size,
                ctx.accounts.queue_root.head_page_offset(),
                clock.slot,
            )?
        };
        if retries >= EXECUTION_QUEUE_MAX_RETRIES {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_market_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }
        break;
    }

    Ok(())
}

pub fn execution_queue_v3_drop_market(
    ctx: Context<ExecutionQueueV3MarketPageAdmin>,
    sequence: u64,
) -> Result<()> {
    let clock = Clock::get()?;
    ctx.accounts
        .authority_state
        .maybe_activate_pending_ctm(clock.slot);
    require!(
        ctx.accounts.queue_root.paused_execute != 0,
        MangoError::ExecutionQueueAdminActionRequiresPause
    );

    let head_abs_page_no = ctx.accounts.queue_root.abs_page_no_for_sequence(sequence);
    let head_page_slot = ctx.accounts.queue_root.page_slot_for_sequence(sequence);
    {
        let queue_page = ctx.accounts.queue_page.load()?;
        queue_page.validate_target(
            ctx.accounts.queue_root.key(),
            head_page_slot,
            head_abs_page_no,
        )?;
    }

    let offset = ctx.accounts.queue_root.page_offset_for_sequence(sequence) as usize;
    let item = {
        let queue_page = ctx.accounts.queue_page.load()?;
        queue_page.items[offset]
    };
    require!(
        item.status == QueueItemStatusV3::Pending as u8 && item.sequence == sequence,
        MangoError::ExecutionQueueSequenceNotPending
    );

    {
        let mut queue_page = ctx.accounts.queue_page.load_mut()?;
        queue_page.clear_item(ctx.accounts.queue_root.page_size, offset as u16)?;
    }
    ctx.accounts.queue_root.live_count = ctx.accounts.queue_root.live_count.saturating_sub(1);
    if sequence == ctx.accounts.queue_root.next_sequence_to_execute {
        ctx.accounts.queue_root.next_sequence_to_execute = ctx
            .accounts
            .queue_root
            .next_sequence_to_execute
            .saturating_add(1);
        ctx.accounts.queue_root.gap_observed_slot = 0;
        let queue_page = ctx.accounts.queue_page.load()?;
        normalize_market_head_within_page(&mut ctx.accounts.queue_root, &queue_page);
    }

    emit!(QueueItemProcessed {
        group: ctx.accounts.group.key(),
        market_index: ctx.accounts.queue_root.market_index,
        sequence,
        kind: item.kind,
        status: QueueItemStatusV3::Failed as u8,
        failure_code: 0,
    });
    Ok(())
}

pub fn execution_queue_v3_execute_liquidity(
    ctx: Context<ExecutionQueueV3ExecuteLiquidity>,
    max_items: u16,
) -> Result<()> {
    require!(
        !ctx.remaining_accounts.is_empty(),
        MangoError::ExecutionQueueDispatchAccountLayoutInvalid
    );

    let queue_root_key = ctx.accounts.queue_root.key();
    let (dispatch_accounts, invoke_accounts) =
        split_dispatch_accounts(ctx.remaining_accounts, queue_root_key)?;
    let clock = Clock::get()?;
    let group_key = ctx.accounts.group.key();
    require!(
        ctx.accounts.queue_root.paused_execute == 0,
        MangoError::ExecutionQueueExecutePaused
    );

    let provided_accounts_hash = hash_accounts(&account_metas_from_infos(dispatch_accounts));

    for _ in 0..max_items {
        if ctx.accounts.queue_root.live_count == 0 {
            break;
        }

        let head_abs_page_no = ctx.accounts.queue_root.head_abs_page_no();
        let head_page_slot = ctx.accounts.queue_root.head_page_slot();
        let next_sequence = ctx.accounts.queue_root.next_sequence_to_execute;
        let page_size = ctx.accounts.queue_root.page_size;
        let gap_wait_slots = EXECUTION_QUEUE_V3_DEFAULT_GAP_WAIT_SLOTS as u64;

        {
            let queue_page = ctx.accounts.queue_page.load()?;
            require!(
                queue_page.page_state == QueuePageStateV3::Active as u8,
                MangoError::ExecutionQueueV3PageInactive
            );
            if queue_page.page_slot != head_page_slot
                || queue_page.assigned_abs_page_no != head_abs_page_no
            {
                break;
            }
            queue_page.validate_target(queue_root_key, head_page_slot, head_abs_page_no)?;
        }

        let head_item = {
            let queue_page = ctx.accounts.queue_page.load()?;
            pending_head_item_on_page(&queue_page, page_size, next_sequence)
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
                skip_liquidity_head_gaps(&mut ctx.accounts.queue_root, &queue_page, group_key);
                continue;
            }
            break;
        };

        if clock.slot < head_item.min_execute_slot {
            break;
        }

        let candidate = ExecutableQueueItemV3 {
            sequence: head_item.sequence,
            kind: head_item.kind,
            payload: head_item.payload[..head_item.payload_len as usize].to_vec(),
            accounts_hash: head_item.accounts_hash,
            retries: head_item.retries,
            min_execute_slot: head_item.min_execute_slot,
            expires_at_slot: head_item.expires_at_slot,
        };

        require!(
            candidate.min_execute_slot <= clock.slot,
            MangoError::SomeError
        );
        require!(
            candidate.accounts_hash == [0; 32] || provided_accounts_hash == candidate.accounts_hash,
            MangoError::ExecutionQueueExecuteAccountsHashMismatch
        );

        let decoded_payload = match decode_queue_payload(&candidate.payload) {
            Ok(payload) => payload,
            Err(_) => {
                {
                    let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                    clear_liquidity_head_and_advance(
                        &mut ctx.accounts.queue_root,
                        &mut queue_page,
                    )?;
                }
                emit!(QueueItemProcessed {
                    group: group_key,
                    market_index: u16::MAX,
                    sequence: candidate.sequence,
                    kind: candidate.kind,
                    status: QueueItemStatusV3::Failed as u8,
                    failure_code: 0,
                });
                continue;
            }
        };

        if queue_item_kind_for_payload_variant(decoded_payload.variant) != candidate.kind {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_liquidity_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index: u16::MAX,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        let dispatch_result = dispatch_queue_payload(
            &decoded_payload,
            dispatch_accounts,
            invoke_accounts,
            group_key,
            queue_root_key,
            ctx.accounts.queue_root.bump,
        );

        if dispatch_result.is_ok() {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_liquidity_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index: u16::MAX,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Executed as u8,
                failure_code: 0,
            });
            continue;
        }

        let next_retry = candidate.retries.saturating_add(1);
        if next_retry >= EXECUTION_QUEUE_MAX_RETRIES {
            {
                let mut queue_page = ctx.accounts.queue_page.load_mut()?;
                clear_liquidity_head_and_advance(&mut ctx.accounts.queue_root, &mut queue_page)?;
            }
            emit!(QueueItemProcessed {
                group: group_key,
                market_index: u16::MAX,
                sequence: candidate.sequence,
                kind: candidate.kind,
                status: QueueItemStatusV3::Failed as u8,
                failure_code: 0,
            });
            continue;
        }

        {
            let mut queue_page = ctx.accounts.queue_page.load_mut()?;
            queue_page.set_retry_backoff(
                ctx.accounts.queue_root.page_size,
                ctx.accounts.queue_root.head_page_offset(),
                clock.slot,
                next_retry,
                1,
            )?;
        }
        break;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_page_geometry_accepts_recommended_market_defaults() {
        let capacity = validate_page_geometry(128, 16, 256).unwrap();
        assert_eq!(capacity, 2048);
    }

    #[test]
    fn validate_page_geometry_rejects_out_of_range_values() {
        assert!(validate_page_geometry(0, 16, 0).is_err());
        assert!(validate_page_geometry(129, 16, 0).is_err());
        assert!(validate_page_geometry(64, 0, 0).is_err());
        assert!(validate_page_geometry(64, 65, 0).is_err());
        assert!(validate_page_geometry(64, 4, 300).is_err());
    }

    #[test]
    fn validate_page_assignment_requires_ring_slot_match() {
        assert!(validate_page_assignment(3, 19, 16).is_ok());
        assert!(validate_page_assignment(3, 18, 16).is_err());
        assert!(validate_page_assignment(16, 16, 16).is_err());
    }
}
