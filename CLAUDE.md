# Project rules

## Keypair management

- **Always store every keypair you generate in `/home/hetalkenaudekar/secure/keypairs/`** (mode 700).
  This includes program keypairs, group keypairs, oracle keypairs, mango account keypairs — anything created by `solana-keygen new`, `Keypair::new()`, or any other generator.
- **Never use ephemeral keypairs.** If you need a keypair for any operation, generate it, persist it to `/home/hetalkenaudekar/secure/keypairs/<descriptive-name>.json`, then use the file. Never use `Keypair::new()` and discard.
- After saving a keypair, also `chmod 600` it.
- File names must be self-describing: e.g. `mango_v4_program_devnet.json`, `group_3FDdg3kMY_admin.json`. Avoid generic names like `keypair.json` or `kp1.json`.
- If a script generates intermediate keypairs (e.g. Solana's deploy buffer ephemeral signer), capture and persist the seed before the script exits — Solana's `solana program deploy` prints a 12-word seed phrase on failure exactly so you can recover the buffer; treat that as a *minimum* baseline, never the only copy.

**Reason:** the v4 program at `5KaJhG2AxyFbyNorYLtUUmrKXZMMGGWDQUzetQgS3LqB` was deployed without the program keypair being persisted, which made an in-place upgrade impossible when the binary needed a fix and the deployer wallet didn't have enough SOL for a fresh buffer. The 34 SOL of program rent had to be reclaimed via `solana program close`, and the address was permanently retired.

**How to apply:** before running any tool that emits a keypair (`solana-keygen new`, custom Rust binaries that `let kp = Keypair::new();`, any TypeScript that calls `new Keypair()`), make sure the output path lands in `/home/hetalkenaudekar/secure/keypairs/`. After the operation, `ls /home/hetalkenaudekar/secure/keypairs/` and confirm the new file is there with sane perms.
