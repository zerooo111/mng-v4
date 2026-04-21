import type { ContinuumStateEngine } from './continuumHarnessTsEngine';
import { ContinuumStateEngine as TsContinuumStateEngine } from './continuumHarnessTsEngine';

export type HarnessBackendKind = 'ts-backend' | 'rust-backend';

export type ContinuumHarnessBackend = Pick<
  ContinuumStateEngine,
  | 'bootstrapFromOnchainSnapshot'
  | 'findIntent'
  | 'getValidatedLocalPayload'
  | 'getAllTrades'
  | 'getBalances'
  | 'getCandles'
  | 'getMarketState'
  | 'getOrders'
  | 'getQueueState'
  | 'getSnapshot'
  | 'getTrades'
  | 'getTradesFiltered'
  | 'getUserState'
  | 'ingestQueueEnqueued'
  | 'ingestQueueProcessed'
  | 'ingestPerpFill'
  | 'ingestRelayIntent'
  | 'ingestRelayIntentStatus'
  | 'listDivergences'
  | 'listIntents'
  | 'reportExternalDivergence'
  | 'subscribe'
>;

type RustBackendModule = {
  createContinuumHarnessBackend?: () =>
    | ContinuumHarnessBackend
    | Promise<ContinuumHarnessBackend>;
  createRustBackend?: () =>
    | ContinuumHarnessBackend
    | Promise<ContinuumHarnessBackend>;
  createRustContinuumStateEngine?: () =>
    | ContinuumHarnessBackend
    | Promise<ContinuumHarnessBackend>;
  ContinuumStateEngine?: new () => ContinuumHarnessBackend;
  RustContinuumStateEngine?: new () => ContinuumHarnessBackend;
  default?:
    | ContinuumHarnessBackend
    | (new () => ContinuumHarnessBackend)
    | ((
        ...args: never[]
      ) => ContinuumHarnessBackend | Promise<ContinuumHarnessBackend>);
};

export function parseHarnessBackendKind(
  value: string | undefined | null,
): HarnessBackendKind {
  const normalized = (value || 'rust-backend').trim().toLowerCase();
  if (normalized === 'ts-backend' || normalized === 'rust-backend') {
    return normalized;
  }
  throw new Error(
    `unsupported CONTINUUM_HARNESS_BACKEND "${value}", expected "ts-backend" or "rust-backend"`,
  );
}

export async function createContinuumHarnessBackend(
  kind: HarnessBackendKind,
  opts?: {
    rustBindingModule?: string;
  },
): Promise<ContinuumHarnessBackend> {
  if (kind === 'ts-backend') {
    return new TsContinuumStateEngine();
  }
  return await createRustBackend(opts);
}

async function createRustBackend(opts?: {
  rustBindingModule?: string;
}): Promise<ContinuumHarnessBackend> {
  const moduleName =
    opts?.rustBindingModule ||
    process.env.CONTINUUM_HARNESS_RUST_BINDING_MODULE ||
    './continuumHarnessRustBackend';

  let loaded: RustBackendModule;
  try {
    loaded = (await import(moduleName)) as RustBackendModule;
  } catch (err: any) {
    throw new Error(
      `failed to load rust-backend module "${moduleName}": ${err?.message || `${err}`}`,
    );
  }

  for (const candidate of [
    loaded.createContinuumHarnessBackend,
    loaded.createRustBackend,
    loaded.createRustContinuumStateEngine,
  ]) {
    if (typeof candidate !== 'function') {
      continue;
    }
    const backend = await candidate();
    assertValidBackend(moduleName, backend);
    return backend;
  }

  for (const candidate of [
    loaded.ContinuumStateEngine,
    loaded.RustContinuumStateEngine,
  ]) {
    if (typeof candidate !== 'function') {
      continue;
    }
    const backend = new candidate();
    assertValidBackend(moduleName, backend);
    return backend;
  }

  if (loaded.default) {
    if (typeof loaded.default === 'function') {
      try {
        const backend = new (loaded.default as new () => ContinuumHarnessBackend)();
        assertValidBackend(moduleName, backend);
        return backend;
      } catch {
        const backend = await (
          loaded.default as () =>
            | ContinuumHarnessBackend
            | Promise<ContinuumHarnessBackend>
        )();
        assertValidBackend(moduleName, backend);
        return backend;
      }
    }
    assertValidBackend(moduleName, loaded.default as ContinuumHarnessBackend);
    return loaded.default as ContinuumHarnessBackend;
  }

  throw new Error(
    `rust-backend module "${moduleName}" did not expose a supported backend factory or constructor`,
  );
}

function assertValidBackend(
  moduleName: string,
  backend: ContinuumHarnessBackend | undefined | null,
): asserts backend is ContinuumHarnessBackend {
  if (!backend) {
    throw new Error(
      `rust-backend module "${moduleName}" returned an empty backend instance`,
    );
  }
  for (const method of [
    'bootstrapFromOnchainSnapshot',
    'findIntent',
    'getValidatedLocalPayload',
    'getAllTrades',
    'getBalances',
    'getCandles',
    'getMarketState',
    'getOrders',
    'getQueueState',
    'getSnapshot',
    'getTrades',
    'getTradesFiltered',
    'getUserState',
    'ingestQueueEnqueued',
    'ingestQueueProcessed',
    'ingestRelayIntent',
    'ingestRelayIntentStatus',
    'listDivergences',
    'listIntents',
    'reportExternalDivergence',
    'subscribe',
  ] as const) {
    if (typeof backend[method] !== 'function') {
      throw new Error(
        `rust-backend module "${moduleName}" is missing required method "${method}"`,
      );
    }
  }
}
