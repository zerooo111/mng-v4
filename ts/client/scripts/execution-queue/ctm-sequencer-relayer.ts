import { AnchorProvider, Wallet } from '@coral-xyz/anchor';
import {
  AccountMeta,
  Cluster,
  Connection,
  Keypair,
  PublicKey,
} from '@solana/web3.js';
import * as dotenv from 'dotenv';
import fs from 'fs';
import path from 'path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import nacl from 'tweetnacl';
import { MANGO_V4_ID } from '../../src/constants';
import {
  buildExecutionQueueEnqueueCtmWithIntentIxs,
  IntentSigner,
} from '../../src/executionQueue';
import { sendTransaction } from '../../src/utils/rpc';

dotenv.config();

const CLUSTER: Cluster =
  (process.env.CLUSTER_OVERRIDE as Cluster) || 'mainnet-beta';
const CLUSTER_URL =
  process.env.CLUSTER_URL_OVERRIDE || process.env.MB_CLUSTER_URL;
const RELAYER_BIND_ADDR = process.env.CTM_RELAYER_BIND_ADDR || '0.0.0.0:9090';
const RELAYER_PAYER_KEYPAIR =
  process.env.CTM_RELAYER_PAYER_KEYPAIR ||
  process.env.USER_KEYPAIR_OVERRIDE ||
  process.env.MB_PAYER_KEYPAIR;
const RELAYER_CTM_KEYPAIR =
  process.env.CTM_RELAYER_CTM_KEYPAIR || RELAYER_PAYER_KEYPAIR;
const RELAYER_SEQUENCE_STATE_PATH =
  process.env.CTM_RELAYER_SEQUENCE_STATE_PATH || '/tmp/ctm-sequences.json';
const RELAYER_DEFAULT_MIN_EXECUTE_SLOT_OFFSET = BigInt(
  process.env.CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET ?? '20',
);
const RELAYER_DEFAULT_EXPIRES_AT_SLOT = BigInt(
  process.env.CTM_RELAYER_DEFAULT_EXPIRES_AT_SLOT ?? '0',
);
const EXECUTION_QUEUE_BUFFER_PK = process.env.EXECUTION_QUEUE_BUFFER_PK;
const RELAYER_PRIORITIZATION_FEE = Number(
  process.env.CTM_RELAYER_PRIORITIZATION_FEE ?? '0',
);
const PROGRAM_ID_OVERRIDE = process.env.CTM_RELAYER_PROGRAM_ID;

type SequenceState = Record<string, string>;

type SubmitIntentRequest = {
  group: string;
  execution_queue: string;
  market: string;
  payload: Buffer;
  remaining_accounts: Array<{
    pubkey: string;
    is_signer: boolean;
    is_writable: boolean;
  }>;
  min_execute_slot: string;
  expires_at_slot: string;
  user_owner: string;
  mango_account: string;
  user_signature: Buffer;
};

type SubmitIntentResponse = {
  sequence: string;
  tx_signature: string;
  user_intent_message: Buffer;
  ctm_envelope_message: Buffer;
};

class SequenceStore {
  private readonly sequences: Map<string, bigint>;
  private pending: Promise<void> = Promise.resolve();

  constructor(private readonly statePath: string) {
    this.sequences = this.load();
  }

  private load(): Map<string, bigint> {
    if (!fs.existsSync(this.statePath)) {
      return new Map();
    }
    const data = JSON.parse(fs.readFileSync(this.statePath, 'utf-8')) as SequenceState;
    const out = new Map<string, bigint>();
    for (const [k, v] of Object.entries(data)) {
      out.set(k, BigInt(v));
    }
    return out;
  }

  async withNextSequence<T>(
    key: string,
    submit: (sequence: bigint) => Promise<T>,
  ): Promise<{ sequence: bigint; value: T }> {
    const waitFor = this.pending;
    let releasePending!: () => void;
    this.pending = new Promise<void>((resolve) => {
      releasePending = resolve;
    });
    await waitFor;

    try {
      const sequence = this.sequences.get(key) ?? 0n;
      const value = await submit(sequence);
      this.sequences.set(key, sequence + 1n);
      this.persist();
      return { sequence, value };
    } finally {
      releasePending();
    }
  }

  private persist(): void {
    const dir = path.dirname(this.statePath);
    fs.mkdirSync(dir, { recursive: true });
    const serialized: SequenceState = {};
    for (const [k, v] of this.sequences.entries()) {
      serialized[k] = v.toString();
    }
    fs.writeFileSync(this.statePath, JSON.stringify(serialized, null, 2));
  }
}

function readKeypair(rawPathOrJson: string): Keypair {
  const maybeFile = path.resolve(rawPathOrJson);
  const raw = fs.existsSync(maybeFile)
    ? fs.readFileSync(maybeFile, 'utf-8')
    : rawPathOrJson;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function parseU64(input: string | number | undefined | null): bigint {
  if (input === undefined || input === null) {
    return 0n;
  }
  if (typeof input === 'number') {
    return BigInt(input);
  }
  return input.length ? BigInt(input) : 0n;
}

function parseRemainingAccounts(
  remainingAccounts: SubmitIntentRequest['remaining_accounts'],
): AccountMeta[] {
  return (remainingAccounts ?? []).map((a) => ({
    pubkey: new PublicKey(a.pubkey),
    isSigner: !!a.is_signer,
    isWritable: !!a.is_writable,
  }));
}

async function main(): Promise<void> {
  if (!CLUSTER_URL) {
    throw new Error('CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required');
  }
  if (!RELAYER_PAYER_KEYPAIR) {
    throw new Error('CTM_RELAYER_PAYER_KEYPAIR (or MB_PAYER_KEYPAIR) is required');
  }
  if (!RELAYER_CTM_KEYPAIR) {
    throw new Error('CTM_RELAYER_CTM_KEYPAIR is required');
  }
  if (!EXECUTION_QUEUE_BUFFER_PK) {
    throw new Error('EXECUTION_QUEUE_BUFFER_PK is required');
  }

  const payer = readKeypair(RELAYER_PAYER_KEYPAIR);
  const ctm = readKeypair(RELAYER_CTM_KEYPAIR);
  const executionQueueBuffer = new PublicKey(EXECUTION_QUEUE_BUFFER_PK);
  const connection = new Connection(CLUSTER_URL, AnchorProvider.defaultOptions());
  const provider = new AnchorProvider(
    connection,
    new Wallet(payer),
    AnchorProvider.defaultOptions(),
  );
  const programId = PROGRAM_ID_OVERRIDE
    ? new PublicKey(PROGRAM_ID_OVERRIDE)
    : MANGO_V4_ID[CLUSTER];
  const sequenceStore = new SequenceStore(RELAYER_SEQUENCE_STATE_PATH);
  const ctmSigner: IntentSigner = { kind: 'keypair', privateKey: ctm.secretKey };

  const protoPath = path.resolve(__dirname, 'ctm_sequencer.proto');
  const pkgDef = protoLoader.loadSync(protoPath, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(pkgDef) as any;

  const serviceImpl = {
    submitIntent: async (
      call: grpc.ServerUnaryCall<SubmitIntentRequest, SubmitIntentResponse>,
      callback: grpc.sendUnaryData<SubmitIntentResponse>,
    ) => {
      try {
        const req = call.request;
        const group = new PublicKey(req.group);
        const executionQueue = new PublicKey(req.execution_queue);
        const userOwner = new PublicKey(req.user_owner);
        const mangoAccount = new PublicKey(req.mango_account);
        const payload = Buffer.from(req.payload ?? []);
        const remainingAccounts = parseRemainingAccounts(req.remaining_accounts);

        const nowSlot = BigInt(await connection.getSlot('processed'));
        const requestedMinSlot = parseU64(req.min_execute_slot);
        const minExecuteSlot =
          requestedMinSlot > 0n
            ? requestedMinSlot
            : nowSlot + RELAYER_DEFAULT_MIN_EXECUTE_SLOT_OFFSET;
        const requestedExpiresSlot = parseU64(req.expires_at_slot);
        const expiresAtSlot =
          requestedExpiresSlot > 0n
            ? requestedExpiresSlot
            : RELAYER_DEFAULT_EXPIRES_AT_SLOT;

        const sequenceKey = `${group.toBase58()}:${req.market}`;
        const userSigner: IntentSigner = {
          kind: 'presigned',
          publicKey: userOwner,
          signature: Buffer.from(req.user_signature ?? []),
        };

        const { sequence, value } = await sequenceStore.withNextSequence(
          sequenceKey,
          async (sequence) => {
            const built = await buildExecutionQueueEnqueueCtmWithIntentIxs({
              programId,
              group,
              executionQueue,
              executionQueueBuffer,
              remainingAccounts,
              payload,
              sequence,
              minExecuteSlot,
              expiresAtSlot,
              userOwner,
              mangoAccount,
              userSigner,
              ctmSigner,
            });

            const userSigOk = nacl.sign.detached.verify(
              new Uint8Array(built.userIntentMessage),
              new Uint8Array(userSigner.signature),
              userOwner.toBytes(),
            );
            if (!userSigOk) {
              throw new Error('user intent signature verification failed');
            }

            const status = await sendTransaction(provider, built.instructions, [], {
              prioritizationFee: RELAYER_PRIORITIZATION_FEE,
            });
            return { built, status };
          },
        );

        callback(null, {
          sequence: sequence.toString(),
          tx_signature: value.status.signature,
          user_intent_message: value.built.userIntentMessage,
          ctm_envelope_message: value.built.ctmEnvelopeMessage,
        });
      } catch (err: any) {
        callback(
          {
            code: grpc.status.INVALID_ARGUMENT,
            message: err?.message || `${err}`,
          },
          null,
        );
      }
    },
  };

  const server = new grpc.Server();
  server.addService(proto.ctmsequencer.CtmSequencerRelayer.service, serviceImpl);

  server.bindAsync(
    RELAYER_BIND_ADDR,
    grpc.ServerCredentials.createInsecure(),
    (err) => {
      if (err) {
        throw err;
      }
      console.log(
        `CTM relayer listening on ${RELAYER_BIND_ADDR}, ctm=${ctm.publicKey.toBase58()}`,
      );
      server.start();
    },
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
