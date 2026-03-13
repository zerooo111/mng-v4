# Startup Bugs Log

## 2026-03-13

### 1) Stale generated config after validator reset can restart services against dead accounts
- Symptom:
  - The validator was healthy, but manually restarted harness/relayer still failed against missing queue/group accounts.
  - Relayer logs showed `AccountNotFound` for queue/group addresses from an older run config.
- Root cause:
  - Validator reset removed ledger state, but generated run artifacts like `execution-queue-e2e-<group>.json`, lane config, and buffer layout could still be reused accidentally before bootstrap rebuilt them.
- Fix:
  - On `RESET_VALIDATOR=1`, startup now removes the generated E2E config and related derived runtime files before bootstrap.
- Restart rule:
  - After a validator reset, always regenerate the run config before starting harness/relayer.

### 2) Bootstrap was not idempotent for reused group numbers
- Symptom:
  - Bootstrap failed in `tokenRegister` with `Allocate: account ... already in use`.
  - The colliding account was the bank PDA for token index `0`.
- Root cause:
  - `local-perp-e2e-bootstrap.ts` created fresh `usdcMint` and `solMint` before checking whether the target group already existed.
  - On a reused `GROUP_NUM`, bootstrap then tried to register a new mint into token index `0`, colliding with the existing bank PDA for the already-registered mint.
- Fix:
  - Bootstrap now resolves existing onchain mint/oracle state before creating fresh local mints.
  - Existing groups reuse the registered USDC bank mint and the existing perp oracle when present.
- Restart rule:
  - Reusing the same `GROUP_NUM` on a live validator must reuse onchain mint/oracle state, not create fresh mints up front.

### 3) Refactored SBF binary exposed stack-unsafe startup instructions
- Symptom:
  - Fresh local bootstrap failed first in `GroupCreate`, then in `TokenRegister`, with `ProgramFailedToComplete`.
  - Validator logs showed BPF access violations in stack frames during those instructions.
- Root cause:
  - The rebuilt program still contained stack-heavy Anchor `Accounts` contexts and instruction bodies that constructed full `Group`/`Bank`/`MintInfo`-scale values on the stack before writing them into zero-copy accounts.
- Fix:
  - Boxed large mint/token `Account` fields in startup-critical `Accounts` structs.
  - Rewrote token registration to initialize zero-copy `Bank` and `MintInfo` accounts in place instead of assigning giant temporary structs.
- Restart rule:
  - After any large account-layout refactor, validate local bootstrap on the rebuilt SBF binary immediately; `cargo check` alone is not enough because BPF stack violations surface only in deployed execution.

### 4) Bootstrap-only bank defaults still need to satisfy `Bank::verify()`
- Symptom:
  - After stack and discriminator issues were fixed, `TokenRegisterBootstrap` still failed with normal Anchor validation errors during local bootstrap.
  - Two concrete failures appeared:
    - `RequireGteViolated` at `bank.rs:426` because `interest_curve_scaling` was left at `0`
    - `RequireEqViolated` at `bank.rs:436` because `disable_asset_liquidation=1` while `maint_asset_weight=1`
- Root cause:
  - The compact bootstrap path initialized only a subset of `Bank` fields and diverged from the minimum valid defaults used by the normal token registration flows.
- Fix:
  - Align bootstrap-only defaults with a valid bank configuration before calling `bank.verify()`, especially interest curve fields and liquidation/asset-weight compatibility flags.
- Restart rule:
  - Whenever `TokenRegisterBootstrap` changes, confirm the first failing tx with `solana confirm -v` and map the exact `bank.rs` invariant before changing more fields.

### 5) `try_from_unchecked(...).load_init()` does not make bootstrap zero-copy accounts discoverable by Anchor clients
- Symptom:
  - `TokenRegisterBootstrap` returned success, but `program.account.bank.all()` and `program.account.mintInfo.all()` returned no accounts for the group.
  - Raw onchain accounts existed at the expected sizes, but their first 8 bytes were all zero.
- Root cause:
  - The bootstrap path switched `bank` and `mint_info` to `UncheckedAccount` and initialized them through `AccountLoader::try_from_unchecked(...).load_init()`, but that path did not leave the Anchor account discriminator set for client-side account scans.
- Fix:
  - After in-place initialization, explicitly write `Bank::discriminator()` and `MintInfo::discriminator()` into bytes `0..8` of the respective account data.
- Restart rule:
  - After any unchecked zero-copy init path is introduced, verify both onchain success and that `program.account.<type>.all()` can rediscover the created accounts.

### 6) Writing discriminators after `load_init()` needs the loader borrows released first
- Symptom:
  - After adding explicit discriminator writes, `TokenRegisterBootstrap` failed with `AccountBorrowFailed`.
  - The tx error was: `instruction tries to borrow reference for an account which is already borrowed`.
- Root cause:
  - The instruction tried to call `try_borrow_mut_data()` on `bank` / `mint_info` while `load_init()`'s mutable zero-copy borrows were still live.
- Fix:
  - Explicitly release the zero-copy borrows before writing the discriminator bytes.
- Restart rule:
  - When mixing `AccountLoader` zero-copy refs with raw `AccountInfo` data access in the same instruction, always end or drop the loader borrows before borrowing account data again.

### 7) `PRELOAD_PROGRAM_IN_VALIDATOR=0` was ignored by validator startup
- Symptom:
  - Trying to switch back to deploy mode still launched the validator with the program preloaded, and `solana program deploy` then failed with:
    - `Program's authority Some(11111111111111111111111111111111) does not match authority provided ...`
- Root cause:
  - `deploy_program()` honored `PRELOAD_PROGRAM_IN_VALIDATOR`, but `start_validator()` always passed `--bpf-program`, so the validator was still seeded with an immutable preloaded program even when deploy mode was requested.
- Fix:
  - Gate the validator `--bpf-program` arguments on `PRELOAD_PROGRAM_IN_VALIDATOR`.
- Restart rule:
  - If deploy mode is intended for debugging CPI/runtime behavior, verify both the deploy step and validator launch path honor the same preload flag.
