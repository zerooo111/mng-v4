# Build & Localnet Guide

Notes collected while getting v4 built and stress-tested in this sandbox.
Re-read this before spending time on "new" build errors — most are
environmental, not code.

## Environment snapshot

- Cargo 1.70.0, rustc 1.70.0 (`rustup` only for `hetalkenaudekar`, not `dm`).
- Solana CLI 1.18.26 (Agave).
- `solana-cargo-build-sbf` 1.18.26 (uses `platform-tools` v1.41).
- Existing complete mango-v4 checkout lives at `/home/hetalkenaudekar/mng-v4`.
- `cargo build-sbf` needs network only for the initial `platform-tools` download — cached results can be reused.

## 1. Missing workspace dependencies

The `exp2/mng-v4` staging checkout does not include the local-patch trees
referenced by the workspace `[patch.crates-io]` section or the
`openbook-v2-patched` path. Link them from the reference checkout:

```bash
cd /home/hetalkenaudekar/stagin4/experimental/exp2/mng-v4/third-party
for d in anchor-attribute-program-0.28.0-local anchor-syn-0.28.0-local \
         globset-0.4.16-local num_enum-0.7.6-local num_enum_derive-0.7.6-local \
         openbook-v2-patched proc-macro-crate-1.3.1-local protobuf-src-noop \
         quote-1.0.45-local solana-program-1.16.14-local syn-2.0.117-local \
         unicode-ident-1.0.24-local zmij-1.0.21-local; do
    [ -e "$d" ] || ln -s /home/hetalkenaudekar/mng-v4/third-party/$d $d
done
```

After this, `cargo check -p mango-v4 --lib --features enable-gpl` should
succeed with warnings only.

> Plain `cargo check -p mango-v4 --lib` fails with a deliberate
> `compile_error!` — you must pass `--features enable-gpl` (or `client`).

## 2. Building the SBF program

Two gotchas when the `$HOME` running cargo-build-sbf differs from the one
that owns the Solana install:

### a) Platform-tools install dir is not writable

`cargo build-sbf` unlinks and relinks
`$SBF_SDK_PATH/dependencies/platform-tools`. If the SDK path is owned by
another user (e.g. the Solana install dir under `/home/hetalkenaudekar/.local/share/solana/...`),
the unlink fails with `Permission denied`.

**Fix:** build as the user that owns the SDK via sudo, and redirect the
Cargo target dir to a place that user can write:

```bash
sudo -n -u hetalkenaudekar bash -lc \
  'CARGO_TARGET_DIR=/tmp/v4-target cargo build-sbf \
      --manifest-path programs/mango-v4/Cargo.toml \
      --features enable-gpl'
```

`-lc` is important — it loads the login profile so `cargo`/`rustup` are on
PATH. Without it you get `Failed to execute rustup: No such file`.

### b) Artifact ownership

The resulting `.so` lands in `/tmp/v4-target/deploy/` owned by
`hetalkenaudekar`. To use it from `dm`:

```bash
sudo rm -f /tmp/mango_v4.so
sudo cp /tmp/v4-target/deploy/mango_v4.so /tmp/mango_v4.so
sudo chown dm /tmp/mango_v4.so
```

## 3. Running a localnet validator

Conflicts to watch:

- A pre-existing `solana-test-validator` may already be running on a
  different port with the *old* `.so`. Do not kill another user's
  validator; start your own on a different port.
- `--log` and `--quiet` are mutually exclusive.
- `--log` expects no argument (it enables stderr logging; pipe stderr if
  you want a file).

Working invocation for v4:

```bash
solana-test-validator --reset \
  --ledger /tmp/v4-ledger \
  --rpc-port 38899 --faucet-port 39010 --gossip-port 40000 \
  --bind-address 127.0.0.1 \
  --bpf-program Hjz5uX54acR4mhiNAih5Qd8yvxZTL5Zt4caFswrqP2Zu /tmp/mango_v4.so \
  > /tmp/v4-validator.out 2>&1 &
```

`Hjz5uX54acR4mhiNAih5Qd8yvxZTL5Zt4caFswrqP2Zu` is the canonical program ID
declared in `programs/mango-v4/src/lib.rs` via `declare_id!`. If you deploy
at any other ID, CPIs in the program will fail — `--bpf-program <ADDRESS>
<SO>` makes the validator load the program at that specific address.

## 4. Relayer / v4-stress crates

Rust toolchain 1.70 cannot compile anything declaring `edition = "2024"`.
When adding a new workspace crate, pin transitive deps away from that
cliff — for `clap`, 4.x pulls `clap_lex 1.x` which needs edition 2024. Use
`clap = "3.1.8"` to match the rest of the workspace.

## 5. Synthetic accounts for tests

Two pubkeys that silently break a hand-crafted test tx:

- **`Pubkey::default()` = 32 zero bytes = System Program ID.** Solana
  demotes any `AccountMeta::new(Pubkey::default(), false)` to readonly on
  the wire. Any offchain `accounts_hash` that assumed `is_writable=true`
  will disagree with the on-chain recompute → commit_hash mismatch in v4.
- **`SysvarC1ock111...` / other sysvar prefixes** are similarly special.

Use a non-reserved prefix byte (e.g. `0x42`) when seeding synthetic
pubkeys for v4 dispatch-account slots.

## 6. Rebuilding after a code change

Because the Solana validator caches the loaded program, a rebuild requires
a validator restart when the program ID is the same:

```bash
# rebuild
sudo -n -u hetalkenaudekar bash -lc 'CARGO_TARGET_DIR=/tmp/v4-target \
    cargo build-sbf --manifest-path programs/mango-v4/Cargo.toml --features enable-gpl'

# replace artifact
sudo rm -f /tmp/mango_v4.so
sudo cp /tmp/v4-target/deploy/mango_v4.so /tmp/mango_v4.so
sudo chown dm /tmp/mango_v4.so

# restart validator
ps aux | grep "ledger /tmp/v4-ledger" | grep -v grep | awk '{print $2}' | xargs -r kill
sleep 2
solana-test-validator --reset --ledger /tmp/v4-ledger --rpc-port 38899 --faucet-port 39010 \
  --gossip-port 40000 --bind-address 127.0.0.1 \
  --bpf-program Hjz5uX54acR4mhiNAih5Qd8yvxZTL5Zt4caFswrqP2Zu /tmp/mango_v4.so \
  > /tmp/v4-validator.out 2>&1 &
sleep 10
```

## 7. Debugging on-chain `msg!` output

`send_and_confirm_transaction` doesn't surface program logs on failure —
you only see the error code. To get logs, call `simulate_transaction`
first (harmless because it doesn't land) and inspect
`sim.value.logs`. The `v4-stress` harness has a `preflight` block that
demonstrates this pattern.
