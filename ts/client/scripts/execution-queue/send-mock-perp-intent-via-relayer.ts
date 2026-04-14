import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import {
  buildExecutionQueueUserIntent,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import {
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import { defaultClusterUrl } from './scriptEnv';

dotenv.config();

const CLUSTER: Cluster = (process.env.CLUSTER_OVERRIDE as Cluster) || 'devnet';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL || defaultClusterUrl();
const USER_KEYPAIR =
  process.env.USER_KEYPAIR_OVERRIDE || process.env.MB_PAYER_KEYPAIR;
const RELAYER_ADDR = process.env.CTM_RELAYER_ADDR || '127.0.0.1:9090';
const GROUP_PK = process.env.EXECUTION_QUEUE_GROUP_PK;
const EXECUTION_QUEUE_PK = process.env.EXECUTION_QUEUE_PK;
const MANGO_ACCOUNT_PK = process.env.MANGO_ACCOUNT_PK;
const MARKET = process.env.MARKET_OVERRIDE || '0';
const MIN_EXECUTE_SLOT = process.env.MIN_EXECUTE_SLOT || '0';
const EXPIRES_AT_SLOT = process.env.EXPIRES_AT_SLOT || '0';
const PRICE_LOTS = BigInt(process.env.MOCK_PRICE_LOTS || '1000');
const MAX_BASE_LOTS = BigInt(process.env.MOCK_MAX_BASE_LOTS || '1');
const MAX_QUOTE_LOTS = BigInt(process.env.MOCK_MAX_QUOTE_LOTS || '100000');
const CLIENT_ORDER_ID = Number(process.env.PERP_ORDER_CLIENT_ORDER_ID || Date.now());
const LIMIT = Number(process.env.PERP_ORDER_LIMIT || '10');

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

async function main(): Promise<void> {
  if (!USER_KEYPAIR || !GROUP_PK || !EXECUTION_QUEUE_PK || !MANGO_ACCOUNT_PK) {
    throw new Error(
      'USER_KEYPAIR_OVERRIDE/MB_PAYER_KEYPAIR, EXECUTION_QUEUE_GROUP_PK, EXECUTION_QUEUE_PK, MANGO_ACCOUNT_PK are required',
    );
  }

  const user = readKeypair(USER_KEYPAIR);
  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(user),
    AnchorProvider.defaultOptions(),
  );
  const slot = await provider.connection.getSlot('confirmed');
  const group = new PublicKey(GROUP_PK);
  const executionQueue = new PublicKey(EXECUTION_QUEUE_PK);
  const mangoAccount = new PublicKey(MANGO_ACCOUNT_PK);

  const remainingAccounts = [
    { pubkey: group, isWritable: false, isSigner: false },
    { pubkey: mangoAccount, isWritable: true, isSigner: false },
    { pubkey: executionQueue, isWritable: false, isSigner: false },
  ];

  const payload = encodePerpPlaceOrderV2QueuePayload({
    side: PerpOrderSide.bid,
    priceLots: PRICE_LOTS,
    maxBaseLots: MAX_BASE_LOTS,
    maxQuoteLots: MAX_QUOTE_LOTS,
    clientOrderId: CLIENT_ORDER_ID,
    orderType: PerpOrderType.limit,
    selfTradeBehavior: PerpSelfTradeBehavior.decrementTake,
    reduceOnly: false,
    expiryTimestamp: 0,
    limit: LIMIT,
  });

  const intent = await buildExecutionQueueUserIntent({
    group,
    executionQueue,
    mangoAccount,
    userOwner: user.publicKey,
    payload,
    target: { kind: 0, index: Number(MARKET) },
    remainingAccounts,
  });
  const userSignature = signExecutionQueueIntentMessage(
    user.secretKey,
    intent.userIntentMessage,
  );

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;
  const relayerClient = new proto.ctmsequencer.CtmSequencerRelayer(
    RELAYER_ADDR,
    grpc.credentials.createInsecure(),
  );

  const response = await new Promise<any>((resolve, reject) => {
    relayerClient.submitIntent(
      {
        group: group.toBase58(),
        execution_queue: executionQueue.toBase58(),
        market: MARKET,
        payload,
        remaining_accounts: remainingAccounts.map((a) => ({
          pubkey: a.pubkey.toBase58(),
          is_signer: a.isSigner,
          is_writable: a.isWritable,
        })),
        min_execute_slot: new BN(Math.max(Number(MIN_EXECUTE_SLOT), slot + 2)).toString(),
        expires_at_slot: EXPIRES_AT_SLOT,
        user_owner: user.publicKey.toBase58(),
        mango_account: mangoAccount.toBase58(),
        user_signature: Buffer.from(userSignature),
        intent_version: 2,
        target_kind: 0,
        target_index: Number(MARKET),
      },
      (err: Error | null, res: any) => {
        if (err) {
          reject(err);
          return;
        }
        resolve(res);
      },
    );
  });

  console.log(
    JSON.stringify(
      {
        sequence: response.sequence,
        txSignature: response.tx_signature,
      },
      null,
      2,
    ),
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
