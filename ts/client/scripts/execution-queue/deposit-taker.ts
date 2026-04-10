import { Connection, Keypair, PublicKey } from '@solana/web3.js';
import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { MangoClient } from '../../src/client';
import * as fs from 'fs';

const CLUSTER_URL = 'https://devnet.helius-rpc.com/?api-key=61e8475f-abea-4774-bc59-9b8ba4df20a0';
const PROGRAM_ID = new PublicKey('Bgjnb7rn2T157TSradRsENVcW86Ss58oMvGGvBgQxTEt');
const GROUP_PK = new PublicKey('Cj8vUC2nWbREhofnD3iWk4j8CD9Fo6j9c33M5ZFKLVPB');
const USDC_MINT = new PublicKey('BBf5TvMhDG3rA8WuNV8xFxZoi2qZ9d5QTouDkeR1ha66');
const MANGO_ACCOUNT_PK = new PublicKey('GkJa12gfjmcNKZYmc4k9Qc2APzUtLYGW3k4v7wKTAqjW');
const KEYPAIR_PATH = '/home/hetalkenaudekar/mng-v4/keypairs/execution-queue-taker.json';
const DEPOSIT_AMOUNT = 5000;

async function main() {
  const keypair = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(KEYPAIR_PATH, 'utf-8')))
  );
  const connection = new Connection(CLUSTER_URL, 'confirmed');
  const provider = new AnchorProvider(connection, new Wallet(keypair), { commitment: 'confirmed' });
  const client = await MangoClient.connect(provider, 'devnet', PROGRAM_ID, { idsSource: 'get-program-accounts' });
  const group = await client.getGroup(GROUP_PK);
  const mangoAccount = await client.getMangoAccount(MANGO_ACCOUNT_PK);
  
  console.log(`Depositing ${DEPOSIT_AMOUNT} USDC to ${MANGO_ACCOUNT_PK.toBase58()}`);
  const sig = await client.tokenDeposit(group, mangoAccount, USDC_MINT, DEPOSIT_AMOUNT);
  console.log('tx:', sig);
}

main().catch(e => { console.error(e); process.exit(1); });
