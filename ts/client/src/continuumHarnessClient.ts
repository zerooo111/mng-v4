import { PublicKey } from '@solana/web3.js';
import {
  EngineSnapshot,
  MarketCandle,
  MarketTrade,
  MarketState,
  QueueState,
  QueueView,
  UserBalances,
  UserState,
} from './continuumHarness';

export type HarnessApiError = {
  status: number;
  message: string;
  body: string;
};

export type HarnessHealth = {
  ok: boolean;
  mode: string;
  intents_total: number;
  divergences_total: number;
  markets_total: number;
  users_total: number;
  queue_views_total: number;
  sse_clients: number;
  airdrop_enabled?: boolean;
  airdrop_deposit_enabled?: boolean;
  generated_ts_ms: number;
};

export type HarnessLiveness = {
  ok: boolean;
  ts_ms: number;
};

export type HarnessStateFullMarket = {
  view: QueueView;
  generated_ts_ms: number;
  market: MarketState | null;
  queue: QueueState | null;
  users: Record<string, UserState>;
};

export type RelayIntentAcceptedRequest = {
  event_type: 'relay_intent_accepted';
  ts_ms?: number;
  group: string;
  execution_queue: string;
  market: string;
  sequence: string;
  kind?: number;
  payload_b64: string;
  remaining_accounts?: Array<{
    pubkey: string;
    is_signer: boolean;
    is_writable: boolean;
  }>;
  min_execute_slot?: string;
  expires_at_slot?: string;
  user_owner: string;
  mango_account: string;
  enqueue_tx_signature: string;
};

export type HarnessAirdropRequest = {
  owner: string;
  ui_amount?: number;
};

export type HarnessAirdropResponse = {
  ok: boolean;
  owner: string;
  mint: string;
  destination_token_account: string;
  ui_amount: number;
  raw_amount: string;
  tx_signature: string;
};

export type HarnessAirdropDepositRequest = {
  owner: string;
  mango_account?: string;
};

export type HarnessAirdropDepositResponse = {
  ok: boolean;
  owner: string;
  mango_account: string;
  group: string;
  mint: string;
  ui_amount: number;
  raw_amount: string;
  unsafe_deposit_tx_signature: string;
  execution_path: 'unsafe_deposit' | 'token_deposit_into_existing_fallback';
};

function normalizeBaseUrl(baseUrl: string): string {
  return baseUrl.endsWith('/') ? baseUrl.slice(0, -1) : baseUrl;
}

function withQuery(path: string, query: Record<string, string | undefined>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== null) {
      params.set(key, value);
    }
  }
  const serialized = params.toString();
  return serialized.length ? `${path}?${serialized}` : path;
}

export class ContinuumHarnessClient {
  constructor(
    readonly baseUrl: string,
    readonly opts?: {
      relayIngestBearerToken?: string;
      fetchImpl?: typeof fetch;
    },
  ) {}

  private get fetchImpl(): typeof fetch {
    if (this.opts?.fetchImpl) {
      return this.opts.fetchImpl;
    }
    return fetch;
  }

  private async request<T>(
    method: 'GET' | 'POST',
    path: string,
    body?: unknown,
    headers?: Record<string, string>,
  ): Promise<T> {
    const url = `${normalizeBaseUrl(this.baseUrl)}${path}`;
    const response = await this.fetchImpl(url, {
      method,
      headers: {
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(headers || {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });

    const text = await response.text();
    if (!response.ok) {
      const err: HarnessApiError = {
        status: response.status,
        message: `harness request failed ${method} ${path}`,
        body: text,
      };
      throw err;
    }

    if (!text.length) {
      return undefined as unknown as T;
    }
    return JSON.parse(text) as T;
  }

  async healthz(): Promise<HarnessHealth> {
    return await this.request<HarnessHealth>('GET', '/healthz');
  }

  async livez(): Promise<HarnessLiveness> {
    return await this.request<HarnessLiveness>('GET', '/livez');
  }

  async getMarketState(market: string | number, view: QueueView = 'optimistic'): Promise<MarketState> {
    const response = await this.request<{ view: QueueView; data: MarketState }>(
      'GET',
      withQuery(`/state/markets/${encodeURIComponent(String(market))}`, { view }),
    );
    return response.data;
  }

  async getUserState(owner: PublicKey | string, view: QueueView = 'optimistic'): Promise<UserState> {
    const ownerPk = owner instanceof PublicKey ? owner.toBase58() : owner;
    const response = await this.request<{ view: QueueView; data: UserState }>(
      'GET',
      withQuery(`/state/users/${encodeURIComponent(ownerPk)}`, { view }),
    );
    return response.data;
  }

  async getOrders(params: {
    market: string | number;
    owner?: PublicKey | string;
    view?: QueueView;
  }) {
    const owner =
      params.owner instanceof PublicKey ? params.owner.toBase58() : params.owner;
    const response = await this.request<{
      view: QueueView;
      market: string;
      owner: string | null;
      data: MarketState['open_orders'];
    }>(
      'GET',
      withQuery(`/state/orders/${encodeURIComponent(String(params.market))}`, {
        view: params.view || 'optimistic',
        owner,
      }),
    );
    return response.data;
  }

  async getQueueState(market: string | number): Promise<QueueState> {
    const response = await this.request<{ market: string; data: QueueState }>(
      'GET',
      `/state/queue/${encodeURIComponent(String(market))}`,
    );
    return response.data;
  }

  async getBalances(
    owner: PublicKey | string,
    view: QueueView = 'optimistic',
  ): Promise<UserBalances> {
    const ownerPk = owner instanceof PublicKey ? owner.toBase58() : owner;
    const response = await this.request<{ view: QueueView; data: UserBalances }>(
      'GET',
      withQuery(`/state/balances/${encodeURIComponent(ownerPk)}`, { view }),
    );
    return response.data;
  }

  async getTrades(params: {
    market: string | number;
    view?: QueueView;
    limit?: number;
  }): Promise<MarketTrade[]> {
    const response = await this.request<{
      view: QueueView;
      market: string;
      data: MarketTrade[];
    }>(
      'GET',
      withQuery(`/state/trades/${encodeURIComponent(String(params.market))}`, {
        view: params.view || 'optimistic',
        limit:
          params.limit !== undefined ? Math.max(0, Math.floor(params.limit)).toString() : undefined,
      }),
    );
    return response.data;
  }

  async getCandles(params: {
    market: string | number;
    view?: QueueView;
    resolutionSec?: number;
    limit?: number;
  }): Promise<MarketCandle[]> {
    const response = await this.request<{
      view: QueueView;
      market: string;
      resolution_sec: number;
      data: MarketCandle[];
    }>(
      'GET',
      withQuery(`/state/candles/${encodeURIComponent(String(params.market))}`, {
        view: params.view || 'optimistic',
        resolution_sec:
          params.resolutionSec !== undefined
            ? Math.max(1, Math.floor(params.resolutionSec)).toString()
            : undefined,
        limit:
          params.limit !== undefined ? Math.max(0, Math.floor(params.limit)).toString() : undefined,
      }),
    );
    return response.data;
  }

  async getFullState(view: QueueView = 'optimistic'): Promise<EngineSnapshot> {
    return await this.request<EngineSnapshot>(
      'GET',
      withQuery('/state/full', { view }),
    );
  }

  async getFullStateForMarket(
    market: string | number,
    view: QueueView = 'optimistic',
  ): Promise<HarnessStateFullMarket> {
    return await this.request<HarnessStateFullMarket>(
      'GET',
      withQuery('/state/full', { market: String(market), view }),
    );
  }

  async ingestRelayIntent(intent: RelayIntentAcceptedRequest): Promise<{ ok: boolean; key: string }> {
    const headers: Record<string, string> = {};
    if (this.opts?.relayIngestBearerToken?.length) {
      headers.Authorization = `Bearer ${this.opts.relayIngestBearerToken}`;
    }
    return await this.request<{ ok: boolean; key: string }>(
      'POST',
      '/ingest/relay-intent',
      intent,
      headers,
    );
  }

  async airdropUsdc(
    params: HarnessAirdropRequest,
  ): Promise<HarnessAirdropResponse> {
    return await this.request<HarnessAirdropResponse>('POST', '/airdrop', params);
  }

  async airdropDepositUsdc(
    params: HarnessAirdropDepositRequest,
  ): Promise<HarnessAirdropDepositResponse> {
    return await this.request<HarnessAirdropDepositResponse>(
      'POST',
      '/airdrop-deposit',
      params,
    );
  }

  async adminReplay(): Promise<{
    ok: boolean;
    optimistic_generated_ts_ms: number;
    confirmed_generated_ts_ms: number;
  }> {
    return await this.request<{
      ok: boolean;
      optimistic_generated_ts_ms: number;
      confirmed_generated_ts_ms: number;
    }>('POST', '/admin/replay', {});
  }
}
