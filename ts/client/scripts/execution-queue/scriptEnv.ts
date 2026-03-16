import path from 'path';

export type StackCluster = 'localnet' | 'devnet';

const DEFAULT_LOCALNET_RPC_URL = 'http://127.0.0.1:8899';
const DEFAULT_DEVNET_RPC_URL = 'https://api.devnet.solana.com';
const DEFAULT_RUNTIME_ROOT = '/home/ec2-user/stagin4/mng-v4';
const DEFAULT_KEYPAIRS_DIR = path.resolve(DEFAULT_RUNTIME_ROOT, 'keypairs');

export function stackCluster(): StackCluster {
  return process.env.STACK_CLUSTER === 'devnet' ? 'devnet' : 'localnet';
}

export function defaultClusterUrl(): string {
  return stackCluster() === 'devnet' ? DEFAULT_DEVNET_RPC_URL : DEFAULT_LOCALNET_RPC_URL;
}

export function runtimeRunDir(): string {
  return path.resolve(
    DEFAULT_RUNTIME_ROOT,
    stackCluster() === 'devnet' ? '.devnet/run' : '.localnet/run',
  );
}

export function runtimeConfigPath(fileName: string): string {
  return path.resolve(runtimeRunDir(), fileName);
}

export function keypairsDir(): string {
  return path.resolve(process.env.KEYPAIRS_DIR || DEFAULT_KEYPAIRS_DIR);
}

export function keypairPath(fileName: string): string {
  return path.resolve(keypairsDir(), fileName);
}
