import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { Connection, Keypair, PublicKey, TransactionInstruction, TransactionMessage, VersionedTransaction } from '@solana/web3.js';
import fs from 'fs';
import { anchorInstructionDiscriminator } from '../../src/executionQueue';

const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || 'https://fermila-develope-edb9.devnet.rpcpool.com/c3f2c0dd-6bb9-46bb-bd56-e178ed73783e';
const ADMIN_KEYPAIR_PATH = process.env.ADMIN_KEYPAIR || '/home/hetalkenaudekar/.config/solana/id.json';
const GROUP = new PublicKey('7SqdQ3xMta4EhztpX1VRtBPitX1cfkyXtWhq29nva9mi');
const EXECUTION_QUEUE = new PublicKey('2gnDrvJgs3MuMVFte8Xx7KKTyeoqqLGGmcEgRpPVvCpL');
const PROGRAM_ID = new PublicKey('4CGsiGHZXSnweudEcN235xkLz4twT2DJB35hS7t89cUm');

function u64ToLe(v: bigint): Buffer {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(v);
  return buf;
}

async function main() {
  const adminKey = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(ADMIN_KEYPAIR_PATH, 'utf-8')))
  );
  const connection = new Connection(CLUSTER_URL, 'confirmed');

  // Read current queue state
  const queueAccount = await connection.getAccountInfo(EXECUTION_QUEUE);
  if (!queueAccount) throw new Error('queue account not found');
  const data = queueAccount.data;
  const gapWaitSlots = data.readBigUInt64LE(184);
  const liquidityDelaySlots = data.readBigUInt64LE(192);

  console.log(`Current state: gapWaitSlots=${gapWaitSlots} liquidityDelaySlots=${liquidityDelaySlots}`);
  console.log(`pauseIngress=${data.readUInt8(145)} pauseExecute=${data.readUInt8(146)}`);

  // Build configure ix: unpause ingress, keep execute unpaused
  const discriminator = anchorInstructionDiscriminator('execution_queue_configure');
  const ixData = Buffer.concat([
    discriminator,
    u64ToLe(gapWaitSlots),
    u64ToLe(liquidityDelaySlots),
    Buffer.from([0]),  // pauseIngress = false
    Buffer.from([0]),  // pauseExecute = false
  ]);

  const ix = new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: GROUP, isSigner: false, isWritable: false },
      { pubkey: EXECUTION_QUEUE, isSigner: false, isWritable: true },
      { pubkey: adminKey.publicKey, isSigner: true, isWritable: false },
    ],
    data: ixData,
  });

  const { blockhash } = await connection.getLatestBlockhash();
  const message = new TransactionMessage({
    payerKey: adminKey.publicKey,
    recentBlockhash: blockhash,
    instructions: [ix],
  }).compileToV0Message();

  const tx = new VersionedTransaction(message);
  tx.sign([adminKey]);
  const sig = await connection.sendTransaction(tx);
  console.log(`Unpause tx sent: ${sig}`);
  await connection.confirmTransaction(sig, 'confirmed');
  console.log('Confirmed. Queue ingress unpaused.');
}

main().catch(e => { console.error(e); process.exit(1); });
