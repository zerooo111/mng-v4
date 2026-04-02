use anchor_lang::prelude::Pubkey;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, HarnessError>;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error(
        "queue payload too short for {label}: expected at least {expected} bytes, got {actual}"
    )]
    PayloadTooShort {
        label: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("unsupported queue payload version: {0}")]
    UnsupportedPayloadVersion(u8),
    #[error("unsupported queue payload flags: {0}")]
    UnsupportedPayloadFlags(u16),
    #[error("unsupported queue payload variant: {0}")]
    UnsupportedPayloadVariant(u8),
    #[error("unsupported queue payload operation: {0}")]
    UnsupportedOperation(&'static str),
    #[error("invalid queue payload side: {0}")]
    InvalidSide(u8),
    #[error("invalid queue payload order type: {0}")]
    InvalidPlaceOrderType(u8),
    #[error("invalid queue payload self trade behavior: {0}")]
    InvalidSelfTradeBehavior(u8),
    #[error("{field} must be nonnegative, got {value}")]
    NegativeValue { field: &'static str, value: i64 },
    #[error("market {0} is not registered")]
    UnknownMarket(u16),
    #[error("market {0} is already registered")]
    MarketAlreadyRegistered(u16),
    #[error("account {0} is not registered")]
    UnknownAccount(Pubkey),
    #[error("account {0} is already registered")]
    AccountAlreadyRegistered(Pubkey),
    #[error(
        "account {account} is already registered with owner {existing_owner}, not {requested_owner}"
    )]
    AccountOwnerMismatch {
        account: Pubkey,
        existing_owner: Pubkey,
        requested_owner: Pubkey,
    },
    #[error("unknown event type: {0}")]
    UnknownEventType(u8),
    #[error("invalid pubkey for {field}: {value}")]
    InvalidPubkey { field: &'static str, value: String },
    #[error("invalid integer for {field}: {value}")]
    InvalidInteger { field: &'static str, value: String },
    #[error("invalid fixed-point for {field}: {value}")]
    InvalidFixedPoint { field: &'static str, value: String },
    #[error("duplicate order id {order_id} on market {market_index}")]
    DuplicateOrderId { market_index: u16, order_id: u128 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Anchor(#[from] anchor_lang::error::Error),
}
