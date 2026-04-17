use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as tx_instructions;

#[derive(Accounts)]
pub struct ExecutionQueueV3InitAuthorityState<'info> {
    #[account(
        mut,
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        init,
        payer = payer,
        space = EXECUTION_QUEUE_AUTHORITY_STATE_SPACE,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3AuthorityAdmin<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(market_index: u16, shard_id: u8)]
pub struct ExecutionQueueV3InitMarketRoot<'info> {
    #[account(
        mut,
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        init,
        payer = payer,
        space = EXECUTION_QUEUE_PERP_MARKET_ROOT_V3_SPACE,
        seeds = [
            b"perp-queue-root".as_ref(),
            group.key().as_ref(),
            market_index.to_le_bytes().as_ref(),
            &[shard_id],
        ],
        bump,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3MarketRootAdmin<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3InitLiquidityRoot<'info> {
    #[account(
        mut,
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        init,
        payer = payer,
        space = EXECUTION_QUEUE_LIQUIDITY_ROOT_V3_SPACE,
        seeds = [b"liq-queue-root".as_ref(), group.key().as_ref()],
        bump,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3LiquidityRootAdmin<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(page_slot: u16)]
pub struct ExecutionQueueV3CreateMarketPage<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(
        init,
        payer = payer,
        space = EXECUTION_QUEUE_PAGE_V3_CREATE_SPACE,
        seeds = [
            b"queue-page".as_ref(),
            queue_root.key().as_ref(),
            page_slot.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    /// CHECK: chunked grow path before zero-copy init
    pub queue_page: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(page_slot: u16)]
pub struct ExecutionQueueV3ResizeMarketPage<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(
        mut,
        seeds = [
            b"queue-page".as_ref(),
            queue_root.key().as_ref(),
            page_slot.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    /// CHECK: chunked grow path before zero-copy init
    pub queue_page: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(page_slot: u16)]
pub struct ExecutionQueueV3InitMarketPage<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(
        zero,
        seeds = [
            b"queue-page".as_ref(),
            queue_root.key().as_ref(),
            page_slot.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
}

#[derive(Accounts)]
#[instruction(page_slot: u16)]
pub struct ExecutionQueueV3CreateLiquidityPage<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(
        init,
        payer = payer,
        space = EXECUTION_QUEUE_PAGE_V3_CREATE_SPACE,
        seeds = [
            b"queue-page".as_ref(),
            queue_root.key().as_ref(),
            page_slot.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    /// CHECK: chunked grow path before zero-copy init
    pub queue_page: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(page_slot: u16)]
pub struct ExecutionQueueV3ResizeLiquidityPage<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(
        mut,
        seeds = [
            b"queue-page".as_ref(),
            queue_root.key().as_ref(),
            page_slot.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    /// CHECK: chunked grow path before zero-copy init
    pub queue_page: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(page_slot: u16)]
pub struct ExecutionQueueV3InitLiquidityPage<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(
        zero,
        seeds = [
            b"queue-page".as_ref(),
            queue_root.key().as_ref(),
            page_slot.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3CloseMarketPage<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(mut, has_one = queue_root, close = receiver)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
    #[account(mut)]
    pub receiver: Signer<'info>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3CloseLiquidityPage<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(mut, has_one = queue_root, close = receiver)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
    #[account(mut)]
    pub receiver: Signer<'info>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3EnqueueMarket<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        mut,
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(mut, has_one = queue_root)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
    /// CHECK: fixed instructions sysvar account
    #[account(address = tx_instructions::ID)]
    pub instructions: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3EnqueueLiquidity<'info> {
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(mut, has_one = queue_root)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3ExecuteMarket<'info> {
    #[account(mut)]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(mut, has_one = queue_root)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3ExecuteLiquidity<'info> {
    #[account(mut)]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, LiquidityQueueRootV3>,
    #[account(mut, has_one = queue_root)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
}

#[derive(Accounts)]
pub struct ExecutionQueueV3MarketPageAdmin<'info> {
    #[account(
        constraint = group.load()?.admin == admin.key() @ MangoError::SomeError,
    )]
    pub group: AccountLoader<'info, Group>,
    #[account(
        has_one = group,
        seeds = [b"queue-authority".as_ref(), group.key().as_ref()],
        bump = authority_state.bump,
    )]
    pub authority_state: Account<'info, ExecutionQueueAuthorityState>,
    #[account(
        mut,
        has_one = group,
        has_one = authority_state,
    )]
    pub queue_root: Account<'info, PerpMarketQueueRootV3>,
    #[account(mut, has_one = queue_root)]
    pub queue_page: AccountLoader<'info, ExecutionQueuePageV3>,
    pub admin: Signer<'info>,
}
