/**
 * provision-fresh-taker.ts
 * Create a mango account for a fresh keypair and deposit USDC.
 */
import { Connection, Keypair, PublicKey } from '@solana/web3.js';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { MangoClient } from '../../src/client';
import * as fs from 'fs';

// Hardcoded defaults are baked to a pre-reset deployment. Overridable via
// env so the script tracks the current devnet bootstrap without edits.
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE ||
  'https://devnet.helius-rpc.com/?api-key=61e8475f-abea-4774-bc59-9b8ba4df20a0';
const PROGRAM_ID = new PublicKey(
  process.env.CTM_RELAYER_PROGRAM_ID || 'Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt',
);
const GROUP_PK = new PublicKey(
  process.env.V4_GROUP ||
    process.env.CONTINUUM_HARNESS_GROUP_PK ||
    'Cj8vUC2nWbREhofnD3iWk4j8CD9Fo6j9c33M5ZFKLVPB',
);
const USDC_MINT = new PublicKey(
  process.env.CONTINUUM_HARNESS_USDC_MINT || 'BBf5TvMhDG3rA8WuNV8xFxZoi2qZ9d5QTouDkeR1ha66',
);
const KEYPAIR_PATH = process.env.TAKER_KEYPAIR_PATH || '/home/hetalkenaudekar/mng-v4/keypairs/taker-fresh.json';
const DEPOSIT_AMOUNT = Number(process.env.DEPOSIT_AMOUNT || '5000');
const ACCOUNT_NUM_OVERRIDE = Number(process.env.ACCOUNT_NUM ?? '0');

async function main() {
  const keypair = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(KEYPAIR_PATH, 'utf-8')))
  );
  console.log('owner:', keypair.publicKey.toBase58());

  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const provider = new AnchorProvider(connection, new Wallet(keypair), { commitment: 'confirmed' });
  const client = await MangoClient.connect(provider, 'devnet', PROGRAM_ID, { idsSource: 'get-program-accounts' });
  const group = await client.getGroup(GROUP_PK);

  // Derive mango account PDA: seeds = ["MangoAccount", group, owner, account_num_le_u32]
  const ACCOUNT_NUM = ACCOUNT_NUM_OVERRIDE;
  const accountNumBuf = Buffer.alloc(4);
  accountNumBuf.writeUInt32LE(ACCOUNT_NUM, 0);
  const [mangoAccountPk] = PublicKey.findProgramAddressSync(
    [Buffer.from('MangoAccount'), GROUP_PK.toBytes(), keypair.publicKey.toBytes(), accountNumBuf],
    PROGRAM_ID,
  );
  console.log('mango account PDA:', mangoAccountPk.toBase58());

  // Step 1: create mango account if it doesn't exist
  const existing = await connection.getAccountInfo(mangoAccountPk);
  if (!existing) {
    console.log('creating mango account...');
    const sig = await client.createMangoAccount(group, ACCOUNT_NUM, 'taker', 8, 4, 4, 32);
    console.log('create tx:', sig);
    // wait for confirmation
    await new Promise(r => setTimeout(r, 4000));
  } else {
    console.log('mango account already exists');
  }

  // Step 2: deposit USDC
  const mangoAccount = await client.getMangoAccount(mangoAccountPk, true);
  console.log(`depositing ${DEPOSIT_AMOUNT} USDC...`);
  const sig = await client.tokenDeposit(group, mangoAccount, USDC_MINT, DEPOSIT_AMOUNT);
  console.log('deposit tx:', sig);
  console.log('done. mango_account:', mangoAccountPk.toBase58());
}

main().catch(e => { console.error(e); process.exit(1); });
