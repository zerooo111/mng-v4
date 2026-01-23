# Order intent message layouts

This document describes the signed payload layouts used by `perp_enqueue_operation`.
Both messages are **Anchor/Borsh-serialized** with the fields listed below in order.

## OrderIntent (user-signed)

Signed by the order owner. This payload authorizes the order parameters and the
account/market keys that must be reused when enqueuing the operation.

Fields (in order):

1. `version` (`u8`) - schema version (currently `0`).
2. `group` (`Pubkey`)
3. `account` (`Pubkey`)
4. `owner` (`Pubkey`)
5. `perp_market` (`Pubkey`)
6. `event_type` (`QueueEventType` encoded as `u8`)
7. `params` (`CompactOrderParamsPayload`)
8. `expiry_ts` (`u64`) - UNIX timestamp (seconds). `0` disables expiry.

### CompactOrderParamsPayload

Fields (in order):

1. `market_index` (`u16`)
2. `side` (`u8`)
3. `order_type` (`u8`)
4. `self_trade_behavior` (`u8`)
5. `reduce_only` (`u8`)
6. `limit` (`u8`)
7. `tif_offset` (`u16`)
8. `max_base_lots` (`i64`)
9. `max_quote_lots` (`i64`)
10. `price_lots` (`i64`)
11. `client_order_id` (`u64`)

## QueuedOrderIntent (sequencer-signed)

Signed by the sequencer/continuum key. This payload binds an `OrderIntent`
hash to a queue sequence number and expiry.

Fields (in order):

1. `version` (`u8`) - schema version (currently `0`).
2. `order_intent_hash` (`[u8; 32]`) - `hashv` over the serialized `OrderIntent`.
3. `seq_no` (`u64`)
4. `expiry_ts` (`u64`) - UNIX timestamp (seconds). `0` disables expiry.
