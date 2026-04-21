# Devnet metadata — 2026-04-21 fresh deploy

End-to-end verified: commit → reveal → execute → on-book at 2026-04-21 11:52 UTC.

## Connection

```
cluster        = devnet
rpc_primary    = https://devnet.helius-rpc.com/?api-key=61e8475f-abea-4774-bc59-9b8ba4df20a0
rpc_secondary  = https://fermila-develope-edb9.devnet.rpcpool.com/c3f2c0dd-6bb9-46bb-bd56-e178ed73783e
relayer_grpc   = 127.0.0.1:9090           # gRPC, SubmitIntent
relayer_http   = 127.0.0.1:9092           # HTTP (legacy harness-facing)
engine_http    = 127.0.0.1:9093           # HTTP (pipeline introspection)
harness_http   = 127.0.0.1:9091           # read-path / airdrop / /trace
fanout_sse     = 127.0.0.1:9094           # realtime event fan-out (SSE)
```

## Program / group / identity

```
program_id       = 9MkHgbHZ24xQ9YUdqUfpsakH2MBUsP8tZWtNJ4pEL8qZ
group            = CwUdR42yS8881q8tmesKuVko1nqsUeqmcEbcgqWVnUxf
group_num        = 3
authority_state  = 9fXSwzrK9NmnrQ1LUcZpmkgdAqffLBRvFWaqZ3YD3Tkx
admin            = CyJSpqonriELcXeSQXnZ17AQsb77ZsHWFdttMmBstq8s
```

## USDC bank

```
usdc_mint        = GUdEQz4upmnxL57Xh6qKsTKU1KagpFSsgRfauuV76G7i
usdc_oracle      = Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX   # Pyth sponsored USDC/USD
```

Use the harness `/airdrop` endpoint to fund any account with USDC of the above mint:
```
POST http://127.0.0.1:9091/airdrop
{"owner":"<your pubkey>","ui_amount":5000}
```
Default max is 100_000 (`CONTINUUM_HARNESS_AIRDROP_MAX_UI_AMOUNT`).

## Perp markets (5)

Every market is a v5 per-market queue — the relayer derives the queue PDA from
`(program_id, group, market_index_le_u16)` so it's listed only for convenience.

| idx | symbol          | base dec | base lot | tick $    | perp_market                                   | bids                                          | asks                                          | event_queue                                   | oracle                                          | queue_pda                                     |
|----:|-----------------|---------:|---------:|----------:|-----------------------------------------------|-----------------------------------------------|-----------------------------------------------|-----------------------------------------------|-------------------------------------------------|-----------------------------------------------|
|   0 | SOL-PERP        | 5        | 100      | 0.001     | goDzPiPaTD1b2E6eYT1WpBRQfnhz5Bb2agwAVoijqtN   | 8xYW6Sykoy8Xu5rErwkbPaGWAh3xeWGj9ygTDKVqMeLT  | G7W55AyJVmQhFhEyPMyhdPTd9KoL7f3fopfuAh8PXy7w  | FwViY7JQvTtSn4VEUpF51UWJ2gm1uMsev9FnCSkGQRQM  | 7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE    | GGUVjU3iEkREsWBrbYAXUUChrshZNjqxbtSJbC5eyBNu  |
|   1 | ETH-PERP        | 6        | 100      | 0.01      | B96Z21bMxXGWtgh6kBe6KEGKhD1uixKFs9Ph5iwiDChV  | FJi5Hoo5K3RWVQcNLc1qL3RmpGF2PpG2BLHFQRE1GB6r  | 9DPDa5zUU79GQtf5LQ6sKAMq4h3RfqRz9vjpYqz9YBdW  | 4UuRceXvB5M1DnDb35UQ6HZEzN57vvQWvJ2QnyAS2nF1  | 42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC    | 8kVA5nsogRgNJFXMfdgbuYE5G3sJUmjBuZmkjkWwc5Yj  |
|   2 | BTC-PERP        | 6        | 100      | 0.01      | FNrXjqWk2ZY63hFjgjpU5sBg5q4ToC9AGSFf6Q1yfd93  | HAR1Hdr5E5fvDtiAwPT6qBPxeq2CeCKuFtReaMmunPYL  | G5hL1MjL9UbBitTw8nxvesTwNq8zu5XVFYeeP8bCkH5p  | N81RPZmxwk7BC9js55csBUmBFwF73M85ZuoEwQfuibj   | 4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo    | GBJWFY8VuMiNieVM4L6bwBszeeA8VFZeyohpfF3D94DH  |
|   3 | ZEC-PERP        | 5        | 100      | 0.001     | AoQ85SrYj3nsupAuV1W7FJF51bHfQGcuWerQpd1oKnWH  | CvRKNgGSoFxa7LMub3T1ruSzWpb97spuBz4qgDCjKHNT  | EmSetuvVbt9Jft8UvQpMJFaw7DeJtUcJNusAXg9zD78E  | 3t67whuicpRpW3PJFbBkMBUpDwapcw4nCri7nX1UVhgS  | HzdKMXqocYWqy7mh8AKDoZFJinjeGMfBKmGAxGbasc28    | GFiPdSaJP6eAPpDLuze7CaL1FsAr3TpitiHQcqmS84cq  |
|   4 | FARTCOIN-PERP   | 6        | 10000    | 0.0001    | Ai69s48uE8ZZ1bvraVwN9nwzZEguEMrtNyYhEcs1MJsp  | 9hbLfYA4FEZFZPQWoJiwHEDFG2CBvbQJ5LvLQNCQX2XT  | GivJunzYJFSEhWRavZrHQk12Hda1e9tBeRcbUifsofqa  | 4oAAKBCtpwnfv8d9aCdXGzL1vcDS1juJ8w1qh8ox3NPo  | 2t8eUbYKjidMs3uSeYM9jXM9uudYZwGkSeTB4TKjmvnC    | AXUarLCHmA4AewXzVBz72fwgSQsiQoNBTUSh6ZCFs6Ct  |

All 5 oracles are Pyth sponsored PriceUpdateV2 feeds (shard 0), derived from feed IDs via
`findProgramAddressSync([u16_le(0), feed_id_bytes], pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT)`:

- SOL/USD:      `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`
- ETH/USD:      `ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace`
- BTC/USD:      `e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43`
- ZEC/USD:      `be9b59d178f0d6a97ab4c343bff2aa69caa1eaae3e9048a65788c529b125bb24`
- FARTCOIN/USD: `58cd29ef0e714c5affc44f269b2c1899a52da4169d7acc147b9da692e6953608`

## Market quote conversions

```
tick_quote_per_base = quote_lot_size * 10^(base_dec - USDC_DEC) / base_lot_size
min_base_size_ui    = base_lot_size / 10^base_dec
```

All markets use `quote_lot_size = 1` and `USDC_DEC = 6`, so:

- SOL, ZEC    (base_dec=5, base_lot=100):   tick $0.001, min 0.001 base
- ETH, BTC    (base_dec=6, base_lot=100):   tick $0.01, min 0.0001 base
- FARTCOIN    (base_dec=6, base_lot=10000): tick $0.0001, min 0.01 base

## Provisioning a maker/taker bot

```bash
# 1. generate + fund a keypair
solana-keygen new --no-bip39-passphrase --silent -o ~/keys/maker-sol.json
solana transfer <maker-pubkey> 0.5 --allow-unfunded-recipient \
  --keypair <admin.json> --url https://api.devnet.solana.com

# 2. airdrop USDC via harness
curl -X POST http://127.0.0.1:9091/airdrop -H 'content-type: application/json' \
  -d '{"owner":"<maker-pubkey>","ui_amount":5000}'

# 3. create mango account + deposit (exp2 tree, ACCOUNT_NUM starts at 0)
cd /home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4
CLUSTER_URL_OVERRIDE="https://devnet.helius-rpc.com/?api-key=…" \
CTM_RELAYER_PROGRAM_ID=9MkHgbHZ24xQ9YUdqUfpsakH2MBUsP8tZWtNJ4pEL8qZ \
V4_GROUP=CwUdR42yS8881q8tmesKuVko1nqsUeqmcEbcgqWVnUxf \
CONTINUUM_HARNESS_USDC_MINT=GUdEQz4upmnxL57Xh6qKsTKU1KagpFSsgRfauuV76G7i \
TAKER_KEYPAIR_PATH=~/keys/maker-sol.json \
ACCOUNT_NUM=0 DEPOSIT_AMOUNT=1000 \
./node_modules/.bin/ts-node ts/client/scripts/execution-queue/provision-fresh-taker.ts

# Output includes the mango account pubkey (PDA).
```

## Submitting an order via the relayer (gRPC)

See `ts/client/scripts/execution-queue/send-perp-order-via-relayer.ts`. Key
env vars:

```
USER_KEYPAIR_OVERRIDE        = path to signing keypair (bot's own key)
CTM_RELAYER_ADDR             = 127.0.0.1:9090
CTM_RELAYER_PROGRAM_ID       = 9MkHgbHZ24xQ9YUdqUfpsakH2MBUsP8tZWtNJ4pEL8qZ
EXECUTION_QUEUE_PK           = <queue_pda from the table above for your market>
MANGO_ACCOUNT_PK             = <bot's mango account>
PERP_MARKET_INDEX            = 0..4
PERP_ORDER_PRICE, QUANTITY, TYPE=limit, CLIENT_ORDER_ID, …
```

The relayer:
- mirrors group/banks/perps from chain at startup (first submit triggers if cold)
- commits the intent on-chain (returns a sequence + tx signature)
- reveals it in the background → program executes against the book
- all within 1-3 Solana slots of each other under normal load

## Observability

- `GET /state/queue/{m}?source=onchain_v5`   — live per-market queue state
- `GET /state/book/{m}?depth=N`              — fresh per-market orderbook
- `GET /trace?market=M&sequence=S`           — pipeline trace for a seq
- `GET /trace?market=M&client_order_id=C`    — lookup by bot-chosen CID
- `GET /trace?tx_signature=<sig>`            — lookup by on-chain tx

## Known operational caveats

- **ALT not rebuilt yet.** `V5_REVEAL_ALT_ADDRESS` is empty — reveals fall
  back to legacy (non-versioned) transactions. Throughput capped at
  batch-size=2 until we bootstrap a new ALT. No correctness impact.
- **Mango accounts under the old program are orphaned.** Users need fresh
  accounts against `group=CwUdR42…`. The old mints / USDC balances are
  dead — re-airdrop.
- **Pyth `maxStalenessSlots=600`** (~5 min). If the Pyth pusher misses
  several updates in a row a market's oracle may go stale. Monitor via
  `/state/book/{m}` (`oracle_price_ui` stops updating).
