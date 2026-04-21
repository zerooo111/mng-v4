# Project rules

## No silent drops: every transaction MUST have an auditable trace

**Rule:** Across the full ingress → accepted → enqueued → committed → executed → head-advanced → state-updated pipeline, every sequence number / transaction must leave a structured, queryable trail. Given a `(market, sequence)` or `client_order_id` or `tx_signature`, it must be possible to determine within seconds: current status, every stage reached, every stage NOT reached, and the exact reason if anything failed. If a code path can cause a transaction to disappear without a recorded cause, that is a bug — fix the code path, don't mask it.

**Forbidden patterns:**
- Catching an error and returning early without emitting a status event / log line that ties back to the seq.
- Mutating reveal_store / queue state without a matching WAL or log record.
- Treating preflight / send failures as "retry later" without recording the attempt with timestamp, blockhash, and error text.
- `dispatch failed (terminal=false)` paths that silently autodrop — those MUST surface in both the relayer log AND the harness trace endpoint so the operator can see "seq X was admin-dropped at slot Y because <reason>".
- Any branch that drops an intent before a `relay_intent_status` event is emitted.

**Required emission points per seq:**
1. Ingress receive → `relay_intent_status status=accepted|rejected` (with `reason` if rejected)
2. Commit tx build → `v5_commit_debug seq=... commit_hash_hex=...`
3. Commit send outcome → `relay_intent_status status=submitted` with `tx_signature`, or `status=rejected` with `reason` including the preflight error code
4. Reveal dispatch → `v5_reveal_debug first_seq=... last_seq=...`
5. Reveal tx outcome (program-level) → parseable from on-chain logs (`dispatch failed` / `dispatch_ok` / `head advanced`)
6. Autodrop → `v5_autodrop admin drop sent seq=...`
7. Head advance → `v5_reveal queue state head=... last_fired=...`

**How to apply:** before merging any relayer/harness change that touches the submit / commit / reveal / drop paths, run `GET /trace/sequence/<market>/<seq>` against a recent live seq and confirm every stage emits at least one matching record. If a path can be exercised where the trace shows nothing for some stage, that path is broken — add the missing emission.

**Reason:** 2026-04-21 devnet incident — bot's cancel_all "failed" with no visible error anywhere. On investigation, no relay_intent_status with `kind=cancel` existed; no on-chain cancel_all tx; no harness log. The failure was invisible at every observability point because whatever path the bot was taking never emitted a status event. Cost ~1 hour of orthogonal debugging before we figured out the cancel wasn't actually being submitted. A trace endpoint that answered "no record for that order id, probably never sent" would have closed the case in 10 seconds.

## Keypair management

- **Always store every keypair you generate in `/home/hetalkenaudekar/secure/keypairs/`** (mode 700).
  This includes program keypairs, group keypairs, oracle keypairs, mango account keypairs — anything created by `solana-keygen new`, `Keypair::new()`, or any other generator.
- **Never use ephemeral keypairs.** If you need a keypair for any operation, generate it, persist it to `/home/hetalkenaudekar/secure/keypairs/<descriptive-name>.json`, then use the file. Never use `Keypair::new()` and discard.
- After saving a keypair, also `chmod 600` it.
- File names must be self-describing: e.g. `mango_v4_program_devnet.json`, `group_3FDdg3kMY_admin.json`. Avoid generic names like `keypair.json` or `kp1.json`.
- If a script generates intermediate keypairs (e.g. Solana's deploy buffer ephemeral signer), capture and persist the seed before the script exits — Solana's `solana program deploy` prints a 12-word seed phrase on failure exactly so you can recover the buffer; treat that as a *minimum* baseline, never the only copy.

**Reason:** the v4 program at `5KaJhG2AxyFbyNorYLtUUmrKXZMMGGWDQUzetQgS3LqB` was deployed without the program keypair being persisted, which made an in-place upgrade impossible when the binary needed a fix and the deployer wallet didn't have enough SOL for a fresh buffer. The 34 SOL of program rent had to be reclaimed via `solana program close`, and the address was permanently retired.

**How to apply:** before running any tool that emits a keypair (`solana-keygen new`, custom Rust binaries that `let kp = Keypair::new();`, any TypeScript that calls `new Keypair()`), make sure the output path lands in `/home/hetalkenaudekar/secure/keypairs/`. After the operation, `ls /home/hetalkenaudekar/secure/keypairs/` and confirm the new file is there with sane perms.

## Relayer binary: build from `experimental/exp2`, NOT from `~/mng-v4`

The systemd service `stagin4-devnet-relayer.service` executes the binary at
`/home/hetalkenaudekar/stagin4/mng-v4/target/release/service-mango-execution-engine`,
which is a symlink chain landing in `/home/hetalkenaudekar/mng-v4/target/release/`.
**Do not infer from that path that `/home/hetalkenaudekar/mng-v4` is the source
of truth.** It is a separate, stripped-down copy that has only `main.rs` and
**no v5 pipeline** — every enqueue it builds goes through the legacy
`ExecutionQueueEnqueueCtm` ix which the on-chain program rejects with
`Custom(6126) LegacyQueuesDisabled`.

The **authoritative relayer source tree** is:
`/home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4/`
(has `v5_pipeline.rs`, `v5_precheck.rs`, `v4_pipeline.rs`, `v4_builders.rs`,
`v4_reveal_packer.rs`, `v4_reveal_wal.rs` in `bin/service-mango-execution-engine/src/`).

**Build + deploy flow:**
```bash
cd /home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4
cargo build --release -p service-mango-execution-engine
# Binary lands at experimental/exp2/mng-v4/target/release/service-mango-execution-engine
sudo cp target/release/service-mango-execution-engine \
        /home/hetalkenaudekar/mng-v4/target/release/service-mango-execution-engine
sudo systemctl restart stagin4-devnet-relayer.service
```

Building at `/home/hetalkenaudekar/mng-v4` and restarting will silently regress
the relayer to no-v5-routing (symptom: every intent returns `Custom(6126)` at
chain preflight). The last known-good v5-enabled binary (29.7 MB, 2026-04-20
18:54) is kept at `experimental/exp2/mng-v4/target/release/` as a rollback.

**Reason:** 2026-04-20 incident where rebuilds from `~/mng-v4` silently
removed v5 routing, 100 % intents hit `LegacyQueuesDisabled`. Two workspace
trees exist and the production symlink points at the wrong one; fix the
build source, not the symlink.

## Program id (`declare_id!`) must match deployed

`programs/mango-v4/src/lib.rs:49` must contain `declare_id!("9rpAcg1jNmUydb4QoeCeJBGf8JfRuxLciRbS7AHGnXEq");`
— the **deployed devnet program id**. If it diverges (historically a revert
to `Hjz5uX54…` has crept in), every zerocopy `load::<Bank>()` /
`load::<PerpMarket>()` returns `AccountOwnedByWrongProgram` and the relayer's
group mirror bootstrap comes back empty ("found no bank/perp accounts"),
rejecting every submit. Check with:
```bash
grep declare_id programs/mango-v4/src/lib.rs
```
Must print `9rpAcg1j...`. Same rule applies for any mainnet deploy — align
source `declare_id!` with the actually-deployed program id before building
the relayer; Anchor's owner check is compile-time.

## Oracle `stable_price` cache freshness

PerpMarket accounts must NOT be seeded into `static_account_cache` at startup
(that cache never expires). The ingress pre-check reads
`target_market.stable_price_model.stable_price`; a frozen cached copy ages
out of the ±`maint_base_*_weight` band and starts rejecting every
near-spot limit order after ~80 min on devnet. Let PerpMarket flow through
the TTL'd `margin_account_cache` instead (see `load_group_static_account_mirror`
comment in `main.rs`). Mainnet `CTM_RELAYER_MARGIN_CACHE_TTL_MS` should be
≤ 500 ms; devnet currently 10000 ms.
