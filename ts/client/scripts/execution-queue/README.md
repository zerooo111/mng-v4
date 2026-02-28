# Execution Queue Relayer + Cranker

## CTM Sequencer Relayer (gRPC :9090)

Runs a gRPC server that accepts user-signed intents, assigns a **market-specific sequence number**, applies CTM envelope signing, and submits `execution_queue_enqueue_ctm`.

### Start

```bash
CTM_RELAYER_BIND_ADDR=0.0.0.0:9090 \
CLUSTER_OVERRIDE=devnet \
CLUSTER_URL_OVERRIDE=https://api.devnet.solana.com \
CTM_RELAYER_PAYER_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_CTM_KEYPAIR=~/.config/solana/id.json \
CTM_RELAYER_PROGRAM_ID=<optional-program-id-override> \
yarn ctm-sequencer-relayer
```

### gRPC API

Proto: `ts/client/scripts/execution-queue/ctm_sequencer.proto`

`SubmitIntent` request fields:
- `group`, `execution_queue`, `market`
- `payload` (queue payload bytes)
- `remaining_accounts` (account metas for dispatch)
- `min_execute_slot`, `expires_at_slot`
- `user_owner`, `mango_account`
- `user_signature` (ed25519 signature over canonical user-intent message)

## Execution Queue Cranker

Periodically submits `execution_queue_execute` using configured account-meta lanes.

### Start

```bash
EXECUTION_QUEUE_GROUP_PK=<group-pk> \
EXECUTION_QUEUE_PK=<execution-queue-pk> \
EXECUTION_QUEUE_CRANKER_KEYPAIR=~/.config/solana/id.json \
EXECUTION_QUEUE_PROGRAM_ID=<optional-program-id-override> \
EXECUTION_QUEUE_CRANK_MAX_ITEMS=4 \
EXECUTION_QUEUE_CRANK_INTERVAL_MS=1500 \
EXECUTION_QUEUE_CRANK_LANES_JSON='[
  {
    "name":"btc-perp",
    "remainingAccounts":[
      {"pubkey":"<group>","isWritable":false},
      {"pubkey":"<mango-account>","isWritable":true},
      {"pubkey":"<execution-queue>","isWritable":false},
      {"pubkey":"<perp-market>","isWritable":true},
      {"pubkey":"<bids>","isWritable":true},
      {"pubkey":"<asks>","isWritable":true},
      {"pubkey":"<event-queue>","isWritable":true},
      {"pubkey":"<oracle>","isWritable":false}
    ]
  }
]' \
yarn execution-queue-cranker
```

Notes:
- Lane account metas must match the `accounts_hash` for queued items.
- Multiple lanes can be configured (one per market/account-meta set).
