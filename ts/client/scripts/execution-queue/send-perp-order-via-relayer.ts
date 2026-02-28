import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import { Cluster, Connection, Keypair, PublicKey } from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { MangoClient } from '../../src/client';
import { MANGO_V4_ID } from '../../src/constants';
import {
  PerpOrderSide,
  PerpOrderType,
  PerpSelfTradeBehavior,
} from '../../src/accounts/perp';
import {
  buildExecutionQueueUserIntent,
  encodePerpPlaceOrderV2QueuePayload,
  signExecutionQueueIntentMessage,
} from '../../src/executionQueue';
import { I64_MAX_BN } from '../../src/utils';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const USER_KEYPAIR =
  process.env.USER_KEYPAIR_OVERRIDE || process.env.MB_PAYER_KEYPAIR;
const RELAYER_ADDR = process.env.CTM_RELAYER_ADDR || '127.0.0.1:9090';
const EXECUTION_QUEUE_PK = process.env.EXECUTION_QUEUE_PK;
const MANGO_ACCOUNT_PK = process.env.MANGO_ACCOUNT_PK;
const PERP_MARKET_INDEX = Number(process.env.PERP_MARKET_INDEX ?? '0');
const PRICE = Number(process.env.PERP_ORDER_PRICE ?? '100');
const QUANTITY = Number(process.env.PERP_ORDER_QUANTITY ?? '0.01');
const MAX_QUOTE_QTY = process.env.PERP_ORDER_MAX_QUOTE_QTY
  ? Number(process.env.PERP_ORDER_MAX_QUOTE_QTY)
  : undefined;
const CLIENT_ORDER_ID = process.env.PERP_ORDER_CLIENT_ORDER_ID
  ? Number(process.env.PERP_ORDER_CLIENT_ORDER_ID)
  : Date.now();
const ORDER_TYPE = process.env.PERP_ORDER_TYPE || 'limit';
const REDUCE_ONLY = (process.env.PERP_ORDER_REDUCE_ONLY || 'false') === 'true';
const EXPIRY_TIMESTAMP = Number(process.env.PERP_ORDER_EXPIRY_TIMESTAMP ?? '0');
const LIMIT = Number(process.env.PERP_ORDER_LIMIT ?? '10');
const MIN_EXECUTE_SLOT = process.env.MIN_EXECUTE_SLOT || '0';
const EXPIRES_AT_SLOT = process.env.EXPIRES_AT_SLOT || '0';
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parseOrderType(): PerpOrderType {
  switch (ORDER_TYPE) {
    case 'limit':
      return PerpOrderType.limit;
    case 'ioc':
      return PerpOrderType.immediateOrCancel;
    case 'postOnly':
      return PerpOrderType.postOnly;
    case 'market':
      return PerpOrderType.market;
    case 'postOnlySlide':
      return PerpOrderType.postOnlySlide;
    default:
      throw new Error(`unsupported PERP_ORDER_TYPE: ${ORDER_TYPE}`);
  }
}

function executionQueueRemainingAccountsFromMangoIx(
  executionQueue: PublicKey,
  keys: { pubkey: PublicKey; isWritable: boolean; isSigner: boolean }[],
) {
  if (keys.length < 3) {
    throw new Error('expected at least 3 metas in perp instruction');
  }
  const remaining = keys.map((k) => ({
    pubkey: k.pubkey,
    isWritable: k.isWritable,
    isSigner: k.isSigner,
  }));
  remaining[2] = {
    pubkey: executionQueue,
    isWritable: remaining[2].isWritable,
    isSigner: false,
  };
  return remaining;
}

async function main(): Promise<void> {
  if (!CLUSTER_URL || !USER_KEYPAIR || !EXECUTION_QUEUE_PK || !MANGO_ACCOUNT_PK) {
    throw new Error(
      'CLUSTER_URL_OVERRIDE/MB_CLUSTER_URL, MB_PAYER_KEYPAIR, EXECUTION_QUEUE_PK, MANGO_ACCOUNT_PK are required',
    );
  }

  const user = readKeypair(USER_KEYPAIR);
  const provider = new AnchorProvider(
    new Connection(CLUSTER_URL, AnchorProvider.defaultOptions()),
    new Wallet(user),
    AnchorProvider.defaultOptions(),
  );
  const client = await MangoClient.connect(
    provider,
    CLUSTER,
    PROGRAM_ID_OVERRIDE
      ? new PublicKey(PROGRAM_ID_OVERRIDE)
      : MANGO_V4_ID[CLUSTER],
    { idsSource: 'get-program-accounts' },
  );
  const mangoAccount = await client.getMangoAccount(new PublicKey(MANGO_ACCOUNT_PK));
  const group = await client.getGroup(mangoAccount.group);
  const executionQueue = new PublicKey(EXECUTION_QUEUE_PK);

  const orderType = parseOrderType();
  const side = PRICE > 0 ? PerpOrderSide.bid : PerpOrderSide.ask;
  const selfTradeBehavior = PerpSelfTradeBehavior.decrementTake;
  const perpMarket = group.getPerpMarketByMarketIndex(PERP_MARKET_INDEX);

  const placeIx = await client.perpPlaceOrderV2Ix(
    group,
    mangoAccount,
    PERP_MARKET_INDEX,
    side,
    Math.abs(PRICE),
    QUANTITY,
    MAX_QUOTE_QTY,
    CLIENT_ORDER_ID,
    orderType,
    selfTradeBehavior,
    REDUCE_ONLY,
    EXPIRY_TIMESTAMP,
    LIMIT,
  );

  const remainingAccounts = executionQueueRemainingAccountsFromMangoIx(
    executionQueue,
    placeIx.keys,
  );

  const payload = encodePerpPlaceOrderV2QueuePayload({
    side,
    priceLots: BigInt(perpMarket.uiPriceToLots(Math.abs(PRICE)).toString()),
    maxBaseLots: BigInt(perpMarket.uiBaseToLots(QUANTITY).toString()),
    maxQuoteLots: MAX_QUOTE_QTY
      ? BigInt(perpMarket.uiQuoteToLots(MAX_QUOTE_QTY).toString())
      : BigInt(I64_MAX_BN.toString()),
    clientOrderId: CLIENT_ORDER_ID,
    orderType,
    selfTradeBehavior,
    reduceOnly: REDUCE_ONLY,
    expiryTimestamp: EXPIRY_TIMESTAMP,
    limit: LIMIT,
  });

  const intent = await buildExecutionQueueUserIntent({
    group: group.publicKey,
    executionQueue,
    mangoAccount: mangoAccount.publicKey,
    userOwner: user.publicKey,
    payload,
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
        group: group.publicKey.toBase58(),
        execution_queue: executionQueue.toBase58(),
        market: new BN(PERP_MARKET_INDEX).toString(),
        payload,
        remaining_accounts: remainingAccounts.map((a) => ({
          pubkey: a.pubkey.toBase58(),
          is_signer: !!a.isSigner,
          is_writable: !!a.isWritable,
        })),
        min_execute_slot: MIN_EXECUTE_SLOT,
        expires_at_slot: EXPIRES_AT_SLOT,
        user_owner: user.publicKey.toBase58(),
        mango_account: mangoAccount.publicKey.toBase58(),
        user_signature: Buffer.from(userSignature),
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

  console.log('Relayer accepted intent:', {
    sequence: response.sequence,
    txSignature: response.tx_signature,
  });
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
