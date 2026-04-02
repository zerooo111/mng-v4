use crate::error::{HarnessError, Result};
use mango_v4::state::{Order, OrderParams, PlaceOrderType, SelfTradeBehavior, Side};

const QUEUE_PAYLOAD_VERSION_V1: u8 = 1;
const QUEUE_PAYLOAD_HEADER_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum QueuePayloadVariant {
    PerpPlaceOrderV2 = 0,
    PerpCancelOrder = 1,
    PerpCancelOrderByClientOrderId = 2,
    PerpCancelAllOrders = 3,
    PerpCancelAllOrdersBySide = 4,
    LiquidityDeposit = 5,
    LiquidityWithdraw = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerpPlaceOrderPayload {
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

impl PerpPlaceOrderPayload {
    pub fn to_order(self, now_ts: u64) -> Result<Option<(Order, u8)>> {
        if self.price_lots < 0 {
            return Err(HarnessError::NegativeValue {
                field: "price_lots",
                value: self.price_lots,
            });
        }
        if self.max_base_lots < 0 {
            return Err(HarnessError::NegativeValue {
                field: "max_base_lots",
                value: self.max_base_lots,
            });
        }
        if self.max_quote_lots < 0 {
            return Err(HarnessError::NegativeValue {
                field: "max_quote_lots",
                value: self.max_quote_lots,
            });
        }

        let Some(time_in_force) = time_in_force_from_expiry(self.expiry_timestamp, now_ts) else {
            return Ok(None);
        };

        let params = match self.order_type {
            PlaceOrderType::Market => OrderParams::Market {},
            PlaceOrderType::ImmediateOrCancel => OrderParams::ImmediateOrCancel {
                price_lots: self.price_lots,
            },
            _ => OrderParams::Fixed {
                price_lots: self.price_lots,
                order_type: self.order_type.to_post_order_type()?,
            },
        };

        Ok(Some((
            Order {
                side: self.side,
                max_base_lots: self.max_base_lots,
                max_quote_lots: self.max_quote_lots,
                client_order_id: self.client_order_id,
                reduce_only: self.reduce_only,
                time_in_force,
                self_trade_behavior: self.self_trade_behavior,
                params,
            },
            self.limit,
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerpCancelOrderPayload {
    pub order_id: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerpCancelOrderByClientOrderIdPayload {
    pub client_order_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerpCancelAllPayload {
    pub limit: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerpCancelAllBySidePayload {
    pub side: Option<Side>,
    pub limit: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiquidityDepositPayload {
    pub amount: u64,
    pub reduce_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiquidityWithdrawPayload {
    pub amount: u64,
    pub allow_borrow: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePayload {
    PerpPlaceOrderV2(PerpPlaceOrderPayload),
    PerpCancelOrder(PerpCancelOrderPayload),
    PerpCancelOrderByClientOrderId(PerpCancelOrderByClientOrderIdPayload),
    PerpCancelAllOrders(PerpCancelAllPayload),
    PerpCancelAllOrdersBySide(PerpCancelAllBySidePayload),
    LiquidityDeposit(LiquidityDepositPayload),
    LiquidityWithdraw(LiquidityWithdrawPayload),
}

impl QueuePayload {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        ensure_min_length(payload, QUEUE_PAYLOAD_HEADER_LEN, "queue payload")?;

        let version = payload[0];
        if version != QUEUE_PAYLOAD_VERSION_V1 {
            return Err(HarnessError::UnsupportedPayloadVersion(version));
        }

        let variant = payload[1];
        let flags = u16::from_le_bytes([payload[2], payload[3]]);
        if flags != 0 {
            return Err(HarnessError::UnsupportedPayloadFlags(flags));
        }

        let body = &payload[QUEUE_PAYLOAD_HEADER_LEN..];
        match variant {
            x if x == QueuePayloadVariant::PerpPlaceOrderV2 as u8 => {
                ensure_min_length(body, 45, "perp place-order payload body")?;
                Ok(Self::PerpPlaceOrderV2(PerpPlaceOrderPayload {
                    side: decode_side(body[0])?,
                    price_lots: read_i64(body, 1)?,
                    max_base_lots: read_i64(body, 9)?,
                    max_quote_lots: read_i64(body, 17)?,
                    client_order_id: read_u64(body, 25)?,
                    order_type: decode_place_order_type(body[33])?,
                    self_trade_behavior: decode_self_trade_behavior(body[34])?,
                    reduce_only: body[35] == 1,
                    expiry_timestamp: read_u64(body, 36)?,
                    limit: body[44],
                }))
            }
            x if x == QueuePayloadVariant::PerpCancelOrder as u8 => {
                ensure_min_length(body, 16, "perp cancel-order payload body")?;
                Ok(Self::PerpCancelOrder(PerpCancelOrderPayload {
                    order_id: read_u128(body, 0)?,
                }))
            }
            x if x == QueuePayloadVariant::PerpCancelOrderByClientOrderId as u8 => {
                ensure_min_length(body, 8, "perp cancel-order-by-client-id payload body")?;
                Ok(Self::PerpCancelOrderByClientOrderId(
                    PerpCancelOrderByClientOrderIdPayload {
                        client_order_id: read_u64(body, 0)?,
                    },
                ))
            }
            x if x == QueuePayloadVariant::PerpCancelAllOrders as u8 => {
                ensure_min_length(body, 1, "perp cancel-all payload body")?;
                Ok(Self::PerpCancelAllOrders(PerpCancelAllPayload {
                    limit: body[0],
                }))
            }
            x if x == QueuePayloadVariant::PerpCancelAllOrdersBySide as u8 => {
                ensure_min_length(body, 2, "perp cancel-all-by-side payload body")?;
                let has_side = body[0] == 1;
                ensure_min_length(
                    body,
                    if has_side { 3 } else { 2 },
                    "perp cancel-all-by-side payload body",
                )?;
                Ok(Self::PerpCancelAllOrdersBySide(
                    PerpCancelAllBySidePayload {
                        side: if has_side {
                            Some(decode_side(body[1])?)
                        } else {
                            None
                        },
                        limit: body[if has_side { 2 } else { 1 }],
                    },
                ))
            }
            x if x == QueuePayloadVariant::LiquidityDeposit as u8 => {
                ensure_min_length(body, 9, "liquidity deposit payload body")?;
                Ok(Self::LiquidityDeposit(LiquidityDepositPayload {
                    amount: read_u64(body, 0)?,
                    reduce_only: body[8] == 1,
                }))
            }
            x if x == QueuePayloadVariant::LiquidityWithdraw as u8 => {
                ensure_min_length(body, 9, "liquidity withdraw payload body")?;
                Ok(Self::LiquidityWithdraw(LiquidityWithdrawPayload {
                    amount: read_u64(body, 0)?,
                    allow_borrow: body[8] == 1,
                }))
            }
            other => Err(HarnessError::UnsupportedPayloadVariant(other)),
        }
    }
}

fn time_in_force_from_expiry(expiry_timestamp: u64, now_ts: u64) -> Option<u16> {
    if expiry_timestamp == 0 {
        return Some(0);
    }
    let tif = expiry_timestamp.saturating_sub(now_ts).min(u16::MAX as u64);
    if tif == 0 {
        None
    } else {
        Some(tif as u16)
    }
}

fn decode_side(value: u8) -> Result<Side> {
    match value {
        0 => Ok(Side::Bid),
        1 => Ok(Side::Ask),
        other => Err(HarnessError::InvalidSide(other)),
    }
}

fn decode_place_order_type(value: u8) -> Result<PlaceOrderType> {
    match value {
        0 => Ok(PlaceOrderType::Limit),
        1 => Ok(PlaceOrderType::ImmediateOrCancel),
        2 => Ok(PlaceOrderType::PostOnly),
        3 => Ok(PlaceOrderType::Market),
        4 => Ok(PlaceOrderType::PostOnlySlide),
        other => Err(HarnessError::InvalidPlaceOrderType(other)),
    }
}

fn decode_self_trade_behavior(value: u8) -> Result<SelfTradeBehavior> {
    match value {
        0 => Ok(SelfTradeBehavior::DecrementTake),
        1 => Ok(SelfTradeBehavior::CancelProvide),
        2 => Ok(SelfTradeBehavior::AbortTransaction),
        other => Err(HarnessError::InvalidSelfTradeBehavior(other)),
    }
}

fn ensure_min_length(buffer: &[u8], expected: usize, label: &'static str) -> Result<()> {
    if buffer.len() < expected {
        return Err(HarnessError::PayloadTooShort {
            label,
            expected,
            actual: buffer.len(),
        });
    }
    Ok(())
}

fn read_u64(buffer: &[u8], offset: usize) -> Result<u64> {
    ensure_min_length(buffer, offset + 8, "u64 field")?;
    Ok(u64::from_le_bytes(
        buffer[offset..offset + 8].try_into().unwrap(),
    ))
}

fn read_i64(buffer: &[u8], offset: usize) -> Result<i64> {
    ensure_min_length(buffer, offset + 8, "i64 field")?;
    Ok(i64::from_le_bytes(
        buffer[offset..offset + 8].try_into().unwrap(),
    ))
}

fn read_u128(buffer: &[u8], offset: usize) -> Result<u128> {
    ensure_min_length(buffer, offset + 16, "u128 field")?;
    Ok(u128::from_le_bytes(
        buffer[offset..offset + 16].try_into().unwrap(),
    ))
}
