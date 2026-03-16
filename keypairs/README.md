# Persistent Keypairs

These keypairs are intentionally stored outside `.localnet/` and `.devnet/`.

Rules:
- startup/bootstrap scripts only read keypairs from this directory
- startup/bootstrap scripts do not generate new keypairs here automatically
- startup/bootstrap scripts do not auto-fund these keypairs

Default files:
- `execution-queue-maker.json`
- `execution-queue-taker.json`
- `quoter-bots/quoter-bot-0.json` through `quoter-bots/quoter-bot-9.json`

Supported overrides:
- `KEYPAIRS_DIR`
- `E2E_MAKER_KEYPAIR_PATH`
- `E2E_TAKER_KEYPAIR_PATH`
- `QUOTER_BOT_KEYPAIR_DIR`
