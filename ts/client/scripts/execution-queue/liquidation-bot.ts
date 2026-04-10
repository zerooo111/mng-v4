/**
 * Lightweight liquidation bot for Fermi DEX devnet.
 *
 * Polls all mango accounts, checks maint_health, and liquidates
 * accounts with negative maintenance health via perpLiqBaseOrPositivePnl.
 *
 * Liquidation is direct on-chain (not through the execution queue).
 */
import { Connection, Keypair, PublicKey } from '@solana/web3.js';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { MangoClient, MangoAccount, Group, PerpMarketIndex } from '../../src';
import * as fs from 'fs';

// ── Config ──────────────────────────────────────────────────────
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE ||
  'https://devnet.helius-rpc.com/?api-key=61e8475f-abea-4774-bc59-9b8ba4df20a0';
const PROGRAM_ID = new PublicKey(
  process.env.PROGRAM_ID || 'Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt',
);
const GROUP_PK = new PublicKey(
  process.env.EXECUTION_QUEUE_GROUP_PK ||
    'Cj8vUC2nWbREhofnD3iWk4j8CD9Fo6j9c33M5ZFKLVPB',
);
const LIQOR_KEYPAIR_PATH =
  process.env.LIQOR_KEYPAIR_PATH ||
  '/home/hetalkenaudekar/.config/solana/id.json';
const LIQOR_MANGO_ACCOUNT =
  process.env.LIQOR_MANGO_ACCOUNT || '';
const POLL_INTERVAL_MS = Number(process.env.LIQ_POLL_INTERVAL_MS || '5000');
const MAX_BASE_TRANSFER = Number(
  process.env.LIQ_MAX_BASE_TRANSFER || '1000000',
);
const MAX_PNL_TRANSFER = Number(
  process.env.LIQ_MAX_PNL_TRANSFER || '1000000000',
);
const PERP_MARKET_INDEX = Number(
  process.env.LIQ_PERP_MARKET_INDEX || '0',
) as PerpMarketIndex;

// ── Helpers ─────────────────────────────────────────────────────
function log(msg: string): void {
  console.log(
    JSON.stringify({ ts: new Date().toISOString(), msg }),
  );
}

function logJson(data: Record<string, unknown>): void {
  console.log(
    JSON.stringify({ ts: new Date().toISOString(), ...data }),
  );
}

// ── Main ────────────────────────────────────────────────────────
async function main(): Promise<void> {
  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const liqorKeypair = Keypair.fromSecretKey(
    Uint8Array.from(
      JSON.parse(fs.readFileSync(LIQOR_KEYPAIR_PATH, 'utf-8')),
    ),
  );
  const provider = new AnchorProvider(
    connection,
    new Wallet(liqorKeypair),
    { commitment: 'confirmed' },
  );
  const client = await MangoClient.connect(
    provider,
    'devnet',
    PROGRAM_ID,
    { idsSource: 'get-program-accounts' },
  );
  const group = await client.getGroup(GROUP_PK);

  // Resolve or create liqor mango account
  let liqorAccount: MangoAccount;
  if (LIQOR_MANGO_ACCOUNT) {
    liqorAccount = await client.getMangoAccount(
      new PublicKey(LIQOR_MANGO_ACCOUNT),
    );
  } else {
    // Find or create liqor account for this keypair
    const existing = await client.getMangoAccountsForOwner(
      group,
      liqorKeypair.publicKey,
    );
    if (existing.length > 0) {
      liqorAccount = existing[0];
      log(
        `Using existing liqor account: ${liqorAccount.publicKey.toBase58()}`,
      );
    } else {
      log('No liqor mango account found — create one first');
      process.exit(1);
    }
  }

  log(
    `Liquidation bot started: liqor=${liqorAccount.publicKey.toBase58()} poll=${POLL_INTERVAL_MS}ms`,
  );

  let totalLiquidations = 0;
  let totalScans = 0;
  let lastGroupReloadMs = Date.now();
  const GROUP_RELOAD_INTERVAL_MS = 60_000;

  // ── Poll loop ───────────────────────────────────────────────
  while (true) {
    try {
      // Reload group periodically for fresh oracle prices
      if (Date.now() - lastGroupReloadMs > GROUP_RELOAD_INTERVAL_MS) {
        await group.reloadAll(client);
        lastGroupReloadMs = Date.now();
      }

      // Fetch all mango accounts
      const allAccounts = await client.getAllMangoAccounts(group, true);
      totalScans++;

      let liquidatable = 0;
      for (const account of allAccounts) {
        // Skip liqor's own account
        if (account.publicKey.equals(liqorAccount.publicKey)) {
          continue;
        }

        try {
          const maintHealth = account.getHealth(group, 'Maint');
          if (maintHealth.isNeg()) {
            liquidatable++;
            const equity = account.getEquity(group);
            const initHealth = account.getHealth(group, 'Init');

            logJson({
              msg: 'liquidatable_account_found',
              account: account.publicKey.toBase58(),
              owner: account.owner.toBase58(),
              maint_health: (maintHealth.toNumber() / 1e6).toFixed(2),
              init_health: (initHealth.toNumber() / 1e6).toFixed(2),
              equity: (equity.toNumber() / 1e6).toFixed(2),
            });

            // Phase 1: Force cancel open perp orders
            const perpOrders = account.perpOrdersActive();
            if (perpOrders.length > 0) {
              try {
                const perpMarket = group.getPerpMarketByMarketIndex(
                  PERP_MARKET_INDEX,
                );
                const healthAccounts =
                  await client.buildHealthRemainingAccounts(
                    group,
                    [account],
                    [],
                    [perpMarket],
                  );
                const sig = await client.program.methods
                  .perpLiqForceCancelOrders(255)
                  .accounts({
                    group: group.publicKey,
                    account: account.publicKey,
                    perpMarket: perpMarket.publicKey,
                    bids: perpMarket.bids,
                    asks: perpMarket.asks,
                  })
                  .remainingAccounts(
                    healthAccounts.map((pk) => ({
                      pubkey: pk,
                      isSigner: false,
                      isWritable: false,
                    })),
                  )
                  .rpc();
                logJson({
                  msg: 'force_cancel_orders',
                  account: account.publicKey.toBase58(),
                  orders_cancelled: perpOrders.length,
                  tx: sig,
                });
              } catch (err: any) {
                logJson({
                  msg: 'force_cancel_failed',
                  account: account.publicKey.toBase58(),
                  error: err.message?.slice(0, 150),
                });
              }
            }

            // Phase 2: Liquidate perp base position or positive PnL
            const perpPositions = account.perpActive();
            for (const perpPos of perpPositions) {
              if (perpPos.basePositionLots.toNumber() === 0) continue;

              try {
                const freshLiqor = await client.getMangoAccount(
                  liqorAccount.publicKey,
                );
                const freshLiqee = await client.getMangoAccount(
                  account.publicKey,
                );
                const perpMarket = group.getPerpMarketByMarketIndex(
                  perpPos.marketIndex as PerpMarketIndex,
                );
                const settleBank =
                  group.getFirstBankForPerpSettlement();
                const healthAccounts =
                  await client.buildHealthRemainingAccounts(
                    group,
                    [freshLiqor, freshLiqee],
                    [settleBank],
                    [perpMarket],
                  );

                const { BN } = await import('@coral-xyz/anchor');
                const sig = await client.program.methods
                  .perpLiqBaseOrPositivePnl(
                    new BN(MAX_BASE_TRANSFER),
                    new BN(MAX_PNL_TRANSFER),
                  )
                  .accounts({
                    group: group.publicKey,
                    perpMarket: perpMarket.publicKey,
                    oracle: perpMarket.oracle,
                    liqor: freshLiqor.publicKey,
                    liqorOwner: liqorKeypair.publicKey,
                    liqee: freshLiqee.publicKey,
                    settleBank: settleBank.publicKey,
                    settleVault: settleBank.vault,
                    settleOracle: settleBank.oracle,
                  })
                  .remainingAccounts(
                    healthAccounts.map((pk) => ({
                      pubkey: pk,
                      isSigner: false,
                      isWritable:
                        pk.equals(settleBank.publicKey),
                    })),
                  )
                  .rpc();

                totalLiquidations++;
                logJson({
                  msg: 'perp_liquidation_success',
                  liqee: freshLiqee.publicKey.toBase58(),
                  market_index: perpPos.marketIndex,
                  base_lots: perpPos.basePositionLots.toNumber(),
                  tx: sig,
                  total_liquidations: totalLiquidations,
                });
              } catch (err: any) {
                logJson({
                  msg: 'perp_liquidation_failed',
                  liqee: account.publicKey.toBase58(),
                  market_index: perpPos.marketIndex,
                  error: err.message?.slice(0, 200),
                });
              }
            }
          }
        } catch {
          // Skip accounts that fail health computation
        }
      }

      if (totalScans % 10 === 0 || liquidatable > 0) {
        logJson({
          msg: 'scan_complete',
          accounts_scanned: allAccounts.length,
          liquidatable,
          total_liquidations: totalLiquidations,
          total_scans: totalScans,
        });
      }
    } catch (err: any) {
      logJson({
        msg: 'scan_error',
        error: err.message?.slice(0, 200),
      });
    }

    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
}

main().catch((err) => {
  console.error('Fatal:', err);
  process.exit(1);
});
