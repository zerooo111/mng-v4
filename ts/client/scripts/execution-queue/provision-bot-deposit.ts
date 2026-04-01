/**
 * provision-bot-deposit.ts — Create Mango accounts and deposit for bot keypairs.
 *
 * Reads keypairs from QUOTER_KEYS_DIR, fetches deposit-context from the harness,
 * then sends accountCreate + tokenDeposit transactions for each bot.
 *
 * Usage:
 *   QUOTER_KEYS_DIR=.devnet/run/quoter-keys \
 *   HARNESS_URL=http://127.0.0.1:9091 \
 *   npx ts-node ts/client/scripts/execution-queue/provision-bot-deposit.ts
 */
import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from '@solana/web3.js';
import { AnchorProvider, BN, Program, Wallet } from '@coral-xyz/anchor';
import {
  getAssociatedTokenAddress,
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
} from '@solana/spl-token';
import fs from 'fs';
import path from 'path';
import axios from 'axios';

const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE ||
  'https://fermila-develope-edb9.devnet.rpcpool.com/c3f2c0dd-6bb9-46bb-bd56-e178ed73783e';
const HARNESS_URL = process.env.HARNESS_URL || 'http://127.0.0.1:9091';
const KEYS_DIR = process.env.QUOTER_KEYS_DIR || '.devnet/run/quoter-keys';
const DEPOSIT_UI_AMOUNT = Number(process.env.DEPOSIT_UI_AMOUNT || '5000');

interface DepositContext {
  owner: string;
  group: string;
  program_id: string;
  quote_mint: string;
  quote_decimals: number;
  quote_bank: string;
  quote_vault: string;
  quote_oracle: string;
  mango_account: string;
  mango_account_exists: boolean;
  account_num: number;
  health_remaining_accounts: string[];
  default_ui_amount: number;
}

function toAccountNumLeBytes(value: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  return bytes;
}

async function provisionBot(
  keypairPath: string,
  connection: Connection,
): Promise<void> {
  const raw = JSON.parse(fs.readFileSync(keypairPath, 'utf-8'));
  const keypair = Keypair.fromSecretKey(Uint8Array.from(raw));
  const owner = keypair.publicKey;
  const name = path.basename(keypairPath, '.json');

  console.log(`\n--- ${name} (${owner.toBase58()}) ---`);

  // Fetch deposit context from harness
  const { data: ctx } = await axios.get<DepositContext>(
    `${HARNESS_URL}/state/deposit-context/${owner.toBase58()}`,
  );

  const wallet = new Wallet(keypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: 'confirmed',
  });

  const { IDL } = await import('../../src/mango_v4');
  const program = new Program(
    IDL as any,
    new PublicKey(ctx.program_id),
    provider,
  );

  const groupPk = new PublicKey(ctx.group);
  const mangoAccountPk = new PublicKey(ctx.mango_account);
  const quoteMintPk = new PublicKey(ctx.quote_mint);

  // Step 1: Create Mango account if needed
  if (!ctx.mango_account_exists) {
    console.log(`  Creating Mango account...`);
    const createIx = await program.methods
      .accountCreate(ctx.account_num, 8, 4, 4, 32, 'bot')
      .accounts({
        group: groupPk,
        owner: owner,
        payer: owner,
      })
      .instruction();
    const createTx = new Transaction().add(createIx);
    const sig = await sendAndConfirmTransaction(connection, createTx, [keypair], {
      commitment: 'confirmed',
    });
    console.log(`  Account created: ${sig}`);
  } else {
    console.log(`  Mango account exists: ${mangoAccountPk.toBase58()}`);
  }

  // Step 2: Deposit
  const ata = await getAssociatedTokenAddress(quoteMintPk, owner);
  const balanceResp = await connection.getTokenAccountBalance(ata, 'confirmed');
  const walletBalance = new BN(balanceResp.value.amount);
  const depositNative = BN.min(
    walletBalance,
    new BN(DEPOSIT_UI_AMOUNT).mul(new BN(10).pow(new BN(ctx.quote_decimals))),
  );

  if (depositNative.lte(new BN(0))) {
    console.log(`  No balance to deposit`);
    return;
  }

  console.log(
    `  Depositing ${depositNative.toString()} native (${(
      Number(depositNative.toString()) / Math.pow(10, ctx.quote_decimals)
    ).toFixed(2)} UI)...`,
  );

  const depositIx = await program.methods
    .tokenDeposit(depositNative, false)
    .accounts({
      group: groupPk,
      account: mangoAccountPk,
      owner: owner,
      bank: new PublicKey(ctx.quote_bank),
      vault: new PublicKey(ctx.quote_vault),
      oracle: new PublicKey(ctx.quote_oracle),
      tokenAccount: ata,
      tokenAuthority: owner,
    })
    .remainingAccounts(
      ctx.health_remaining_accounts.map((pk) => ({
        pubkey: new PublicKey(pk),
        isSigner: false,
        isWritable: false,
      })),
    )
    .instruction();

  const depositTx = new Transaction().add(depositIx);
  const sig = await sendAndConfirmTransaction(connection, depositTx, [keypair], {
    commitment: 'confirmed',
  });
  console.log(`  Deposited: ${sig}`);
}

async function main() {
  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const files = fs
    .readdirSync(KEYS_DIR)
    .filter((f) => f.endsWith('.json'))
    .sort();

  console.log(`Found ${files.length} keypairs in ${KEYS_DIR}`);

  for (const file of files) {
    try {
      await provisionBot(path.join(KEYS_DIR, file), connection);
    } catch (err: any) {
      console.error(`  FAILED: ${err.message || err}`);
    }
  }

  console.log('\nDone.');
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
