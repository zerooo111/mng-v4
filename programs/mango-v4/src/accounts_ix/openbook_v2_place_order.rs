use crate::error::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use num_enum::IntoPrimitive;
use num_enum::TryFromPrimitive;
use openbook_v2::{
    program::OpenbookV2,
    state::{BookSide, Market, OpenOrdersAccount, PostOrderType, SelfTradeBehavior, Side},
};

#[derive(Copy, Clone, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum OpenbookV2PlaceOrderType {
    Limit = 0,
    ImmediateOrCancel = 1,
    PostOnly = 2,
    Market = 3,
    PostOnlySlide = 4,
}

impl OpenbookV2PlaceOrderType {
    pub fn to_external_post_order_type(&self) -> Result<PostOrderType> {
        match *self {
            Self::Market => Err(MangoError::SomeError.into()),
            Self::ImmediateOrCancel => Err(MangoError::SomeError.into()),
            Self::Limit => Ok(PostOrderType::Limit),
            Self::PostOnly => Ok(PostOrderType::PostOnly),
            Self::PostOnlySlide => Ok(PostOrderType::PostOnlySlide),
        }
    }
}

#[derive(Copy, Clone, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum OpenbookV2PostOrderType {
    Limit = 0,
    PostOnly = 2,
    PostOnlySlide = 4,
}

#[derive(Copy, Clone, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum OpenbookV2SelfTradeBehavior {
    DecrementTake = 0,
    CancelProvide = 1,
    AbortTransaction = 2,
}
impl OpenbookV2SelfTradeBehavior {
    pub fn to_external(&self) -> SelfTradeBehavior {
        match *self {
            OpenbookV2SelfTradeBehavior::DecrementTake => SelfTradeBehavior::DecrementTake,
            OpenbookV2SelfTradeBehavior::CancelProvide => SelfTradeBehavior::CancelProvide,
            OpenbookV2SelfTradeBehavior::AbortTransaction => SelfTradeBehavior::AbortTransaction,
        }
    }
}

#[derive(Copy, Clone, TryFromPrimitive, IntoPrimitive, AnchorSerialize, AnchorDeserialize)]
#[repr(u8)]
pub enum OpenbookV2Side {
    Bid = 0,
    Ask = 1,
}
impl OpenbookV2Side {
    pub fn to_external(&self) -> Side {
        match *self {
            Self::Bid => Side::Bid,
            Self::Ask => Side::Ask,
        }
    }
}

#[derive(Accounts)]
pub struct OpenbookV2PlaceOrder<'info> {
    /// CHECK: validated inline in the instruction body.
    pub group: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub account: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub open_orders: UncheckedAccount<'info>,

    pub openbook_v2_market: UncheckedAccount<'info>,

    /// CHECK: validated inline in the instruction body.
    pub openbook_v2_program: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub openbook_v2_market_external: UncheckedAccount<'info>,

    /// The bank that pays for the order. Bank oracle also expected in remaining_accounts
    //  payer_bank.vault == payer_vault is validated inline at #3
    //  bank.token_index is validated against the openbook market at #4
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub payer_bank: UncheckedAccount<'info>,
    /// The bank vault that pays for the order
    #[account(mut)]
    pub payer_vault: UncheckedAccount<'info>,

    /// The bank that receives the funds upon settlement. Bank oracle also expected in remaining_accounts
    //  bank.token_index is validated against the openbook market at #4
    #[account(mut)]
    /// CHECK: validated inline in the instruction body.
    pub receiver_bank: UncheckedAccount<'info>,
}
