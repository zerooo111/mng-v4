use crate::accounts_ix::*;
use crate::error::*;
use crate::error_msg;
use crate::require_msg;
use crate::state::BookSide;
use anchor_lang::prelude::*;

fn book_is_empty(book: &BookSide) -> bool {
    book.roots.iter().all(|root| root.leaf_count == 0)
}

#[allow(clippy::too_many_arguments)]
pub fn perp_close_market(ctx: Context<PerpCloseMarket>) -> Result<()> {
    let perp_market = ctx.accounts.perp_market.load()?;
    require_msg!(
        perp_market.open_interest == 0,
        "perp market still has open interest"
    );

    let bids = ctx.accounts.bids.load()?;
    require_msg!(book_is_empty(&bids), "perp market bids are not empty");

    let asks = ctx.accounts.asks.load()?;
    require_msg!(book_is_empty(&asks), "perp market asks are not empty");

    let event_queue = ctx.accounts.event_queue.load()?;
    require_msg!(
        event_queue.is_empty(),
        "perp market event queue is not empty"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn book_is_empty_checks_both_roots() {
        let mut book = BookSide::zeroed();
        assert!(book_is_empty(&book));

        book.roots[0].leaf_count = 1;
        assert!(!book_is_empty(&book));

        book.roots[0].leaf_count = 0;
        book.roots[1].leaf_count = 2;
        assert!(!book_is_empty(&book));
    }
}
