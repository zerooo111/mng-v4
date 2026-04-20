use anchor_lang::prelude::*;
use bytemuck::cast_ref;

use crate::accounts_ix::*;
use crate::error::*;
use crate::state::*;

fn book_has_orders_for_owner(book: &BookSide, owner: Pubkey) -> bool {
    book.nodes
        .nodes
        .iter()
        .filter_map(|node| node.as_leaf())
        .any(|leaf| leaf.owner == owner)
}

fn event_queue_has_activity_for_account(event_queue: &EventQueue, account: Pubkey) -> bool {
    event_queue
        .iter()
        .any(|event| match EventType::try_from(event.event_type) {
            Ok(EventType::Fill) => {
                let fill: &FillEvent = cast_ref(event);
                fill.maker == account || fill.taker == account
            }
            Ok(EventType::Out) => {
                let out: &OutEvent = cast_ref(event);
                out.owner == account
            }
            Ok(EventType::Liquidate) => false,
            Err(_) => true,
        })
}

pub fn perp_admin_repair_stale_orders(ctx: Context<PerpAdminRepairStaleOrders>) -> Result<()> {
    let perp_market = ctx.accounts.perp_market.load()?;
    let perp_market_index = perp_market.perp_market_index;
    let account_pk = ctx.accounts.account.key();

    let bids = ctx.accounts.bids.load()?;
    require_msg!(
        !book_has_orders_for_owner(&bids, account_pk),
        "account still has bids on book"
    );

    let asks = ctx.accounts.asks.load()?;
    require_msg!(
        !book_has_orders_for_owner(&asks, account_pk),
        "account still has asks on book"
    );

    let event_queue = ctx.accounts.event_queue.load()?;
    require_msg!(
        !event_queue_has_activity_for_account(&event_queue, account_pk),
        "account still has pending perp events"
    );

    let mut account = ctx.accounts.account.load_full_mut()?;
    require_msg!(
        account
            .all_perp_orders()
            .all(|oo| !oo.is_active_for_market(perp_market_index)),
        "account still has active perp order slots"
    );

    let perp_position = account.perp_position_mut(perp_market_index)?;
    require_msg!(
        perp_position.taker_base_lots == 0 && perp_position.taker_quote_lots == 0,
        "perp position still has taker fills"
    );

    let old_bids = perp_position.bids_base_lots;
    let old_asks = perp_position.asks_base_lots;
    perp_position.bids_base_lots = 0;
    perp_position.asks_base_lots = 0;

    msg!(
        "perp_admin_repair_stale_orders: account={} market_index={} bids {}->0 asks {}->0",
        account_pk,
        perp_market_index,
        old_bids,
        old_asks,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn detects_leaf_owned_by_account() {
        let owner = Pubkey::new_unique();
        let mut book = BookSide::zeroed();
        book.nodes.order_tree_type = OrderTreeType::Bids.into();
        book.nodes.nodes[7] =
            *LeafNode::new(0, 1, owner, 1, 0, PostOrderType::Limit, 0, -1, 0).as_ref();

        assert!(book_has_orders_for_owner(&book, owner));
        assert!(!book_has_orders_for_owner(&book, Pubkey::new_unique()));
    }

    #[test]
    fn detects_event_queue_activity_for_account() {
        let owner = Pubkey::new_unique();
        let mut event_queue = EventQueue::zeroed();
        event_queue
            .push_back(
                FillEvent::new(
                    Side::Bid,
                    false,
                    0,
                    0,
                    1,
                    owner,
                    11,
                    22,
                    I80F48::ZERO,
                    0,
                    Pubkey::new_unique(),
                    33,
                    I80F48::ZERO,
                    44,
                    55,
                )
                .into(),
            )
            .unwrap();

        assert!(event_queue_has_activity_for_account(&event_queue, owner));
        assert!(!event_queue_has_activity_for_account(
            &event_queue,
            Pubkey::new_unique()
        ));
    }
}
