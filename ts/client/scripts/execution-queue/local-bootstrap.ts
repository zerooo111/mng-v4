import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import { createMint } from '@solana/spl-token';
import { Cluster, Connection, Keypair, PublicKey, SystemProgram, Transaction, TransactionInstruction } from '@solana/web3.js';
import { createHash } from 'crypto';
import * as dotenv from 'dotenv';
import fs from 'fs';
import os from 'os';
import path from 'path';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';

dotenv.config();

const CLUSTER: Cluster = (process.env.CLUSTER_OVERRIDE as Cluster) || 'devnet';
const CLUSTER_URL = process.env.CLUSTER_URL_OVERRIDE || 'http://127.0.0.1:8899';
const PAYER_KEYPAIR =
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  process.env.MB_PAYER_KEYPAIR ||
  path.join(os.homedir(), '.config/solana/id.json');
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;
const GROUP_NUM = Number(process.env.EXECUTION_QUEUE_GROUP_NUM || '7001');
const MANGO_ACCOUNT_NUM = Number(process.env.EXECUTION_QUEUE_ACCOUNT_NUM || '0');
const EXECUTION_QUEUE_BUFFER_KEYPAIR =
  process.env.EXECUTION_QUEUE_BUFFER_KEYPAIR || '/tmp/execution-queue-buffer-keypair.json';
const CTM_KEYPAIR = process.env.CTM_RELAYER_CTM_KEYPAIR;
const CTM_PUBKEY = process.env.CTM_PUBKEY;
const INSURANCE_MINT_OVERRIDE = process.env.INSURANCE_MINT;
const EXECUTION_QUEUE_BUFFER_SPACE = 8 + 32 + 4 + 4 + 1000 * 368;

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function anchorDiscriminator(ixName: string): Buffer {
  return createHash('sha256')
    .update(`global:${ixName}`)
    .digest()
    .subarray(0, 8);
}

async function main(): Promise<void> {
  const admin = readKeypair(PAYER_KEYPAIR);
  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(admin),
    AnchorProvider.defaultOptions(),
  );

  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];

  const client = await MangoClient.connect(provider, CLUSTER, programId, {
    idsSource: 'get-program-accounts',
  });

  const insuranceMint = INSURANCE_MINT_OVERRIDE
    ? new PublicKey(INSURANCE_MINT_OVERRIDE)
    : await createMint(
        provider.connection,
        admin,
        admin.publicKey,
        null,
        6,
      );

  const groupNumBuf = Buffer.alloc(4);
  groupNumBuf.writeUInt32LE(GROUP_NUM);
  const [groupPk] = PublicKey.findProgramAddressSync(
    [Buffer.from('Group'), admin.publicKey.toBuffer(), groupNumBuf],
    programId,
  );

  const groupInfo = await provider.connection.getAccountInfo(groupPk);
  if (!groupInfo) {
    try {
      await client.groupCreate(GROUP_NUM, true, 0, insuranceMint);
    } catch (err) {
      console.log(
        'groupCreate skipped/failed (likely already exists):',
        (err as Error).message,
      );
    }
  }
  const group = await client.getGroup(groupPk);

  const [executionQueue] = PublicKey.findProgramAddressSync(
    [Buffer.from('ExecutionQueue'), group.publicKey.toBuffer()],
    programId,
  );

  const ctmSigner = CTM_PUBKEY
    ? new PublicKey(CTM_PUBKEY)
    : CTM_KEYPAIR
      ? readKeypair(CTM_KEYPAIR).publicKey
      : admin.publicKey;

  const queueInfo = await provider.connection.getAccountInfo(executionQueue);
  let queueBufferKp: Keypair;
  if (fs.existsSync(EXECUTION_QUEUE_BUFFER_KEYPAIR)) {
    queueBufferKp = readKeypair(EXECUTION_QUEUE_BUFFER_KEYPAIR);
  } else {
    queueBufferKp = Keypair.generate();
    fs.writeFileSync(
      EXECUTION_QUEUE_BUFFER_KEYPAIR,
      JSON.stringify(Array.from(queueBufferKp.secretKey)),
    );
  }

  let queueBufferPk = queueBufferKp.publicKey;
  if (!queueInfo) {
    const queueBufferInfo = await provider.connection.getAccountInfo(queueBufferKp.publicKey);
    if (!queueBufferInfo) {
      const lamports = await provider.connection.getMinimumBalanceForRentExemption(
        EXECUTION_QUEUE_BUFFER_SPACE,
      );
      const createBufferIx = SystemProgram.createAccount({
        fromPubkey: admin.publicKey,
        newAccountPubkey: queueBufferKp.publicKey,
        lamports,
        space: EXECUTION_QUEUE_BUFFER_SPACE,
        programId,
      });
      const createBufferTx = new Transaction().add(createBufferIx);
      await provider.sendAndConfirm(createBufferTx, [queueBufferKp]);
    }

    const ix = new TransactionInstruction({
      programId,
      keys: [
        { pubkey: group.publicKey, isSigner: false, isWritable: true },
        { pubkey: executionQueue, isSigner: false, isWritable: true },
        { pubkey: admin.publicKey, isSigner: true, isWritable: true },
        { pubkey: admin.publicKey, isSigner: true, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        { pubkey: queueBufferKp.publicKey, isSigner: false, isWritable: true },
      ],
      data: Buffer.concat([anchorDiscriminator('execution_queue_init'), ctmSigner.toBuffer()]),
    });
    await client.sendAndConfirmTransaction([ix]);
  } else if (queueInfo.data.length >= 136) {
    queueBufferPk = new PublicKey(queueInfo.data.subarray(104, 136));
  }

  let mangoAccount = await client.getMangoAccountForOwner(
    group,
    admin.publicKey,
    MANGO_ACCOUNT_NUM,
  );
  if (!mangoAccount) {
    await client.createMangoAccount(
      group,
      MANGO_ACCOUNT_NUM,
      'ctm-local-user',
      8,
      4,
      4,
      8,
    );
    mangoAccount = await client.getMangoAccountForOwner(
      group,
      admin.publicKey,
      MANGO_ACCOUNT_NUM,
    );
  }

  if (!mangoAccount) {
    throw new Error('failed to create/load mango account');
  }

  console.log(JSON.stringify({
    clusterUrl: CLUSTER_URL,
    programId: programId.toBase58(),
    admin: admin.publicKey.toBase58(),
    ctmSigner: ctmSigner.toBase58(),
    group: group.publicKey.toBase58(),
    executionQueue: executionQueue.toBase58(),
    executionQueueBuffer: queueBufferPk.toBase58(),
    mangoAccount: mangoAccount.publicKey.toBase58(),
    insuranceMint: insuranceMint.toBase58(),
  }, null, 2));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
