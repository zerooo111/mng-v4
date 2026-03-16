# Mar 16 Updates

## Devnet Deployment

- deployed program id: `9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF`
- cluster: `devnet`
- primary RPC used for testing: `https://devnet.helius-rpc.com/?api-key=6475b77b-7c5f-48b4-88f0-4879d4af73cb`
- persisted config: `.devnet/run/execution-queue-e2e-9120.json`
- group: `9VYm4QaBhEPEiFfyGxXEDpN7ZTh2muajTDebKrDL4f5k`
- execution queue: `HfaFVCt5FnLQfLETidHYopgQ2RqpW5JhR66yfQdtYrFP`

## Safety Changes

- startup/bootstrap paths no longer auto-fund maker/taker/bots
- startup/bootstrap paths no longer hot-generate keypairs
- maker/taker and quoter bots now use persistent saved keypairs from `keypairs/`
- devnet restarts can be run with `SKIP_BOOTSTRAP=1` against the saved config without replaying expensive setup

## Harness / Relayer Wiring

The devnet launcher and harness are updated to the current deployment and current user-state logic:

- `startup_local.sh` and `startup_all_local.sh` pass:
  - devnet RPC override
  - `PROGRAM_ID`
  - group/config-derived values
  - `CONTINUUM_HARNESS_PROGRAM_ID`
- `continuum-state-harness.ts` uses `CONTINUUM_HARNESS_PROGRAM_ID` when provided
- the harness uses the current `ContinuumStateEngine` / `UserState` model from `ts/client/src/continuumHarness.ts`
- confirmed harness user-state reads now expose:
  - `perp_positions`
  - `margin_summary`
  - confirmed collateral balances

## Devnet E2E

The full Rust path works on devnet for the clean bounded E2E:

- bootstrap resumed successfully from saved config
- relayer accepted and executed the place/cancel flow
- queue drained in the clean E2E case
- maker/taker ended with opposing perp positions
- cancel-all cleared open orders

Current conclusion: devnet E2E correctness is good.

## Devnet TPS Result

First bounded devnet TPS pass used:

- `2` persistent quoter bots
- `200 ms` quoter interval
- nonblocking submit enabled
- short bounded runtime

Measured result:

- `avgPlaceTps=2.2549`
- `avgIntentTps=7.1052`
- `peakWindowPlaceTps=8.6300`
- `peakWindowIntentTps=18.3387`
- `tickErrorCount=147`

Observed limiter:

- external RPC throttling on devnet provider
- relayer metrics at report time:
  - `execution_engine_requests_total=319`
  - `execution_engine_requests_ok_total=172`
  - `execution_engine_requests_error_total=147`
  - `execution_engine_submit_avg_ms=1818.846`
  - `execution_engine_submit_prepare_avg_ms=0.392`
  - `execution_engine_submit_send_avg_ms=1726.524`
- failures were dominated by `429 Too Many Requests`

Secondary devnet issue after the burst:

- queue remained sparse instead of draining cleanly
- observed state:
  - `count=167`
  - `ctmCount=167`
  - `gapSpan=181`
  - `next=4`
  - `head-not-found`
- harness logs showed repeated `QueueItemProcessed` for the same head range (`4` skipped, `5..16` processed) across many execute signatures
- the relayer/executor had to be stopped manually to avoid wasting SOL

## Devnet PnL / Settlement Test

A dedicated devnet PnL correctness script was added:

- `ts/client/scripts/execution-queue/local-perp-pnl-test.ts`

What it checks:

- crossed perp fill at a controlled off-oracle price
- nonzero opposite unsettled PnL on maker and taker
- `perpSettlePnl` between profitable and unprofitable accounts
- post-settle collateral and residual PnL movement

Latest successful result:

- the relayer trade leg hit the known devnet duplicate-sequence reconciliation edge
- the script fell back to direct `perpPlaceOrder` for the trade leg
- PnL and settle correctness still succeeded

Measured values:

- oracle price: `100`
- fill price: `95`
- maker after trade:
  - unsettled PnL `+7.2708935321 USDC`
- taker after trade:
  - unsettled PnL `-7.5638935460 USDC`
- settle signature:
  - `C1Z1XMfPiwjSeAKUVtEDNQgx3gMwieuhezdrhGQCXAW6Xt8VcoecN3dnvRK93NWJmgHWoEewTsWx5yKesHiXjXQ`
- maker after settle:
  - unsettled PnL `0`
  - confirmed USDC balance `20007.270893532106`
- taker after settle:
  - unsettled PnL `-0.2930000139`
  - confirmed USDC balance `19992.729106467894`

Current conclusion:

- onchain matching works
- onchain unsettled PnL calculation works
- `perpSettlePnl` works
- harness confirmed user-state and margin summaries reflect the new devnet state

## Current Known Devnet Issues

1. TPS is provider-limited before it becomes onchain-limited.
2. The relayer still hits duplicate-sequence reconciliation on devnet after restart/state drift.
3. After a throttled burst, the queue/executor can get stuck replaying gap-skip work without durable head advancement.
4. Aggressive devnet load should be avoided until the relayer/executor devnet profile is tuned.

## Current Operational State

- devnet stack was stopped after testing to avoid further SOL burn
- maker/taker devnet state remains valid
- harness wiring and saved config are ready for the next controlled devnet pass
