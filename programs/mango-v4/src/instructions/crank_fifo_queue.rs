use anchor_lang::prelude::*;

use crate::accounts_ix::*;
use crate::error::*;
use crate::instructions::{
    perp_cancel_order_by_client_order_id_prevalidated, perp_place_order_prevalidated,
};
use crate::state::*;

pub fn crank_fifo_queue(ctx: Context<CrankFifoQueue>, max_events: u8) -> Result<()> {
    let mut queue = ctx.accounts.queue.load_mut()?;
    let saved_header = queue.header;

    let result = (|| -> Result<()> {
        let mut processed: u8 = 0;
        let mut last_executed = queue.header.last_executed_seq;

        while processed < max_events && queue.header.count > 0 {
            let head_index = queue.header.head as usize;
            let event = queue.entries[head_index];

            let expected_seq = last_executed.saturating_add(1);
            if event.seq_no != expected_seq {
                if event.seq_no > expected_seq {
                    let current_slot = Clock::get()?.slot;
                    let lag = current_slot.saturating_sub(event.submitted_slot);
                    if lag >= queue.header.max_lag_slots as u64 {
                        last_executed = event.seq_no.saturating_sub(1);
                        queue.header.last_executed_seq = last_executed;
                        continue;
                    } else {
                        break;
                    }
                } else {
                    // Skip stale events
                    queue.header.head = (queue.header.head + 1)
                        % crate::state::queue_fifo::FIFO_QUEUE_CAPACITY as u32;
                    queue.header.count = queue.header.count.saturating_sub(1);
                    continue;
                }
            }

            validate_event_targets(&ctx, &event)?;

            match event.queue_event_type()? {
                QueueEventType::PerpPlaceOrder => {
                    let (order, limit) = order_from_params(&event.params)?;
                    perp_place_order_prevalidated(
                        PerpPlaceOrderAccounts {
                            group: &ctx.accounts.group,
                            account: &ctx.accounts.account,
                            perp_market: &ctx.accounts.perp_market,
                            bids: &ctx.accounts.bids,
                            asks: &ctx.accounts.asks,
                            event_queue: &ctx.accounts.event_queue,
                            oracle: ctx.accounts.oracle.as_ref(),
                            owner_key: event.user,
                            remaining_accounts: ctx.remaining_accounts,
                        },
                        order,
                        limit,
                    )?;
                }
                QueueEventType::PerpCancelOrder => {
                    perp_cancel_order_by_client_order_id_prevalidated(
                        PerpCancelOrderByClientOrderIdAccounts {
                            account: &ctx.accounts.account,
                            perp_market: &ctx.accounts.perp_market,
                            bids: &ctx.accounts.bids,
                            asks: &ctx.accounts.asks,
                            owner_key: event.user,
                        },
                        event.params.client_order_id,
                    )?;
                }
                _ => return err!(MangoError::UnsupportedQueueEventType),
            }

            queue.header.head =
                (queue.header.head + 1) % crate::state::queue_fifo::FIFO_QUEUE_CAPACITY as u32;
            queue.header.count = queue.header.count.saturating_sub(1);
            last_executed = event.seq_no;
            queue.header.last_executed_seq = last_executed;
            processed = processed.saturating_add(1);
        }

        Ok(())
    })();

    if result.is_err() {
        queue.header = saved_header;
    } else {
        let last_executed_seq = queue.header.last_executed_seq;
        drop(queue);
        let mut perp_market = ctx.accounts.perp_market.load_mut()?;
        perp_market.queue_last_executed_seq = last_executed_seq;
    }

    result
}

fn validate_event_targets(ctx: &Context<CrankFifoQueue>, event: &QueueFifoEvent) -> Result<()> {
    let account = ctx.accounts.account.load()?;
    require!(
        account.fixed.is_owner_or_delegate(event.user),
        MangoError::SomeError
    );

    let perp_market = ctx.accounts.perp_market.load()?;
    require_eq!(
        perp_market.perp_market_index,
        event.params.market_index,
        MangoError::InvalidQueueParams
    );

    Ok(())
}

fn order_from_params(params: &CompactOrderParams) -> Result<(Order, u8)> {
    let side = params.side()?;
    let place_order_type = params.place_order_type()?;
    let self_trade_behavior = params.self_trade_behavior()?;
    let reduce_only = params.is_reduce_only();

    let order_params = match place_order_type {
        PlaceOrderType::Limit => OrderParams::Fixed {
            price_lots: params.price_lots,
            order_type: place_order_type.to_post_order_type()?,
        },
        PlaceOrderType::ImmediateOrCancel => OrderParams::ImmediateOrCancel {
            price_lots: params.price_lots,
        },
        PlaceOrderType::Market => OrderParams::ImmediateOrCancel {
            price_lots: i64::MAX,
        },
        PlaceOrderType::PostOnly | PlaceOrderType::PostOnlySlide => OrderParams::Fixed {
            price_lots: params.price_lots,
            order_type: place_order_type.to_post_order_type()?,
        },
    };

    let order = Order {
        side,
        max_base_lots: params.max_base_lots,
        max_quote_lots: params.max_quote_lots,
        client_order_id: params.client_order_id,
        reduce_only,
        time_in_force: params.tif_offset,
        self_trade_behavior,
        params: order_params,
    };

    Ok((order, params.limit))
}
