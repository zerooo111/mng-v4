import { PublicKey } from '@solana/web3.js';
import { HarnessBackendKind } from './continuumHarnessBackend';
import {
  AccountProjectedState,
  EngineSnapshot,
  MarginSummaryAccount,
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
  backend?: HarnessBackendKind;
  instance_id?: string;
  pid?: number;
  bind_addr?: string;
  process_started_ts_ms?: number;
  cwd?: string;
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

export type HarnessMarketMetadata = {
  market_index: number;
  name: string;
  base_symbol: string;
  quote_symbol: string;
  base_mint: string;
  quote_mint: string;
  perp_market: string;
  oracle: string;
  bids: string;
  asks: string;
  event_queue: string;
  base_decimals: number;
  quote_decimals: number;
  base_lot_size: string;
  quote_lot_size: string;
  open_interest: string;
};

export type OrderbookLevelView = {
  price_lots: string;
  base_lots: string;
  price_ui: number | null;
  qty_ui: number | null;
};

export type OrderbookSummaryView = {
  depth: number;
  bids: OrderbookLevelView[];
  asks: OrderbookLevelView[];
};

export type MarketTradeSummary = {
  market: string;
  view: QueueView;
  window_ms: number;
  trade_count: number;
  last_trade_ts_ms: number | null;
  last_price_lots: string | null;
  last_price_ui: number | null;
  open_price_lots: string | null;
  open_price_ui: number | null;
  high_price_lots: string | null;
  high_price_ui: number | null;
  low_price_lots: string | null;
  low_price_ui: number | null;
  change_24h_pct: number | null;
  volume_base_lots: string;
  volume_quote_lots: string;
  volume_base_ui: number | null;
  volume_quote_ui: number | null;
};

export type MarketRuntimeMetrics = {
  market: string;
  oracle_price_ui: number | null;
  mark_price_ui: number | null;
  funding_rate_daily_pct: number | null;
  funding_rate_hourly_pct: number | null;
  open_interest_base_lots: string | null;
  open_interest_base_ui: number | null;
  best_bid_ui: number | null;
  best_ask_ui: number | null;
  updated_ts_ms: number;
};

export type MarketListItem = {
  market: string;
  view: QueueView;
  metadata: HarnessMarketMetadata | null;
  data: MarketState;
  orderbook_summary: OrderbookSummaryView;
  trade_summary: MarketTradeSummary;
  metrics: MarketRuntimeMetrics | null;
};

export type StubbedAccountMetrics = {
  status: 'stub';
  source: 'pending-subtree';
  updated_ts_ms: number;
  fields: {
    margin_used: null;
    health_init: null;
    health_maint: null;
    pnl_realized: null;
    pnl_unrealized: null;
    equity: null;
    liquidation_price_by_market: null;
  };
};

export type FrontendAccountMetrics =
  | StubbedAccountMetrics
  | {
      status: 'empty' | 'ok';
      source:
        | 'onchain-mango-health'
        | 'rust-replay-perp-token-health'
        | 'rust-replay-perp-token-health-partial';
      updated_ts_ms: number;
      account_count: number;
      mango_account: string | null;
      totals: {
        equity_native_quote: string;
        pnl_native_quote: string;
        assets_native_quote: string;
        liabs_native_quote: string;
        init_health_native_quote: string;
        maint_health_native_quote: string;
        margin_usage_fraction: number;
      };
      accounts: MarginSummaryAccount[];
      fields: {
        margin_used: number;
        health_init: string;
        health_maint: string;
        pnl_realized: null;
        pnl_unrealized: string;
        equity: string;
        liquidation_price_by_market: null;
      };
    };

export type FrontendOwnerSlice = {
  owner: string;
  mango_account: string | null;
  view: QueueView;
  positions_scope: 'owner_aggregate';
  positions: UserState['per_market'];
  open_orders: MarketState['open_orders'];
  trades: MarketTrade[];
  account_metrics: FrontendAccountMetrics;
};

export type FrontendMarketSlice = {
  market: string;
  view: QueueView;
  metadata: HarnessMarketMetadata | null;
  metrics: MarketRuntimeMetrics | null;
  trade_summary: MarketTradeSummary;
  orderbook_summary: OrderbookSummaryView;
  orderbook: MarketState | null;
};

export type HarnessMarketStateResponse = {
  view: QueueView;
  metadata: HarnessMarketMetadata | null;
  orderbook_summary: OrderbookSummaryView;
  trade_summary: MarketTradeSummary;
  metrics: MarketRuntimeMetrics | null;
  data: MarketState;
};

export type HarnessMarketsResponse = {
  view: QueueView;
  items: MarketListItem[];
};

export type HarnessTradesResponse = {
  view: QueueView;
  market: string | null;
  owner: string | null;
  data: MarketTrade[];
};

export type HarnessTradeSummaryResponse =
  | {
      view: QueueView;
      market: string;
      owner: string | null;
      data: MarketTradeSummary;
    }
  | {
      view: QueueView;
      market: null;
      owner: string | null;
      items: MarketTradeSummary[];
    };

export type HarnessStateFull = EngineSnapshot & {
  market_metadata?: Record<string, HarnessMarketMetadata | null>;
};

export type HarnessStateFullMarket = {
  view: QueueView;
  generated_ts_ms: number;
  market: MarketState | null;
  market_metadata?: HarnessMarketMetadata | null;
  queue: QueueState | null;
  users: Record<string, UserState>;
  accounts?: Record<string, AccountProjectedState>;
};

export type RelayIntentAcceptedRequest = {
  event_type: 'relay_intent_accepted';
  ts_ms?: number;
  group: string;
  execution_queue: string;
  market: string;
  intent_version?: number;
  target_kind?: number;
  target_index?: number;
  accounts_hash?: string;
  remaining_accounts_source?: string;
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
  account_num?: number;
  ui_amount?: number;
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
  auto_created_mango_account: boolean;
  unsafe_account_create_tx_signature: string | null;
};

export type HarnessDepositContextResponse = {
  owner: string;
  group: string;
  program_id: string;
  quote_mint: string;
  quote_decimals: number;
  quote_bank: string;
  quote_vault: string;
  quote_oracle: string;
  mango_account: string;
  mango_account_exists: boolean;
  account_num: number;
  health_remaining_accounts: string[];
  default_ui_amount: number;
};

export type HarnessFundingRateEntry = {
  ts_ms: number;
  hourly_pct: number | null;
  daily_pct: number | null;
  long_funding: string;
  short_funding: string;
  oracle_price_ui: number | null;
};

export type HarnessMonitoringMarketItem = {
  market: string;
  market_name: string;
  last_collected_ms: number | null;
  volume_24h_base_lots: string;
  volume_24h_quote_lots: string;
  volume_24h_base_ui: number | null;
  volume_24h_quote_ui: number | null;
  trade_count_24h: number | null;
  change_24h_pct: number | null;
  last_price_ui: number | null;
  cumulative_volume_base_lots: string;
  cumulative_volume_quote_lots: string;
  cumulative_volume_base_ui: number | null;
  cumulative_volume_quote_ui: number | null;
  open_interest_base_lots: string | null;
  open_interest_base_ui: number | null;
  fees_accrued_native: string | null;
  fees_settled_native: string | null;
  active_unique_users: number;
  all_time_unique_users: number;
  current_funding_hourly_pct: number | null;
  current_funding_daily_pct: number | null;
  current_oracle_price_ui: number | null;
  funding_rate_history: HarnessFundingRateEntry[];
};

export type HarnessMonitoringResponse = {
  generated_ts_ms: number;
  collection_interval_ms: number;
  stats_file: string;
  last_collection_ms: number | null;
  markets: HarnessMonitoringMarketItem[];
};

function normalizeBaseUrl(baseUrl: string): string {
  return baseUrl.endsWith('/') ? baseUrl.slice(0, -1) : baseUrl;
}

function toBase58(value?: PublicKey | string | null): string | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  return value instanceof PublicKey ? value.toBase58() : value;
}

function withQuery(
  path: string,
  query: Record<string, string | undefined | null>,
): string {
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

  async getMarkets(params?: {
    markets?: Array<string | number>;
    view?: QueueView;
    depth?: number;
    book?: 'summary' | 'full';
  }): Promise<MarketListItem[]> {
    const response = await this.request<HarnessMarketsResponse>(
      'GET',
      withQuery('/state/markets', {
        view: params?.view || 'optimistic',
        depth:
          params?.depth !== undefined
            ? Math.max(1, Math.floor(params.depth)).toString()
            : undefined,
        book: params?.book,
        markets:
          params?.markets && params.markets.length
            ? params.markets.map((market) => String(market)).join(',')
            : undefined,
      }),
    );
    return response.items;
  }

  async getMarketStateDetails(
    market: string | number,
    params?: {
      view?: QueueView;
      depth?: number;
    },
  ): Promise<HarnessMarketStateResponse> {
    return await this.request<HarnessMarketStateResponse>(
      'GET',
      withQuery(`/state/markets/${encodeURIComponent(String(market))}`, {
        view: params?.view || 'optimistic',
        depth:
          params?.depth !== undefined
            ? Math.max(1, Math.floor(params.depth)).toString()
            : undefined,
      }),
    );
  }

  async getMarketState(
    market: string | number,
    view: QueueView = 'optimistic',
  ): Promise<MarketState> {
    const response = await this.getMarketStateDetails(market, { view });
    return response.data;
  }

  async getUserState(
    owner: PublicKey | string,
    view: QueueView = 'optimistic',
  ): Promise<UserState> {
    const ownerPk = toBase58(owner)!;
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
    const owner = toBase58(params.owner);
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
    const ownerPk = toBase58(owner)!;
    const response = await this.request<{ view: QueueView; data: UserBalances }>(
      'GET',
      withQuery(`/state/balances/${encodeURIComponent(ownerPk)}`, { view }),
    );
    return response.data;
  }

  async queryTrades(params: {
    market?: string | number;
    owner?: PublicKey | string;
    view?: QueueView;
    limit?: number;
  }): Promise<MarketTrade[]> {
    const response = await this.request<HarnessTradesResponse>(
      'GET',
      withQuery('/state/trades', {
        view: params.view || 'optimistic',
        market:
          params.market !== undefined ? String(params.market) : undefined,
        owner: toBase58(params.owner),
        limit:
          params.limit !== undefined
            ? Math.max(0, Math.floor(params.limit)).toString()
            : undefined,
      }),
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

  async getTradeSummary(params: {
    market: string | number;
    owner?: PublicKey | string;
    view?: QueueView;
  }): Promise<MarketTradeSummary> {
    const response = await this.request<Extract<HarnessTradeSummaryResponse, { data: MarketTradeSummary }>>(
      'GET',
      withQuery('/state/trades/summary', {
        view: params.view || 'optimistic',
        market: String(params.market),
        owner: toBase58(params.owner),
      }),
    );
    return response.data;
  }

  async listTradeSummaries(params?: {
    owner?: PublicKey | string;
    view?: QueueView;
  }): Promise<MarketTradeSummary[]> {
    const response = await this.request<Extract<HarnessTradeSummaryResponse, { items: MarketTradeSummary[] }>>(
      'GET',
      withQuery('/state/trades/summary', {
        view: params?.view || 'optimistic',
        owner: toBase58(params?.owner),
      }),
    );
    return response.items;
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

  async getFullState(view: QueueView = 'optimistic'): Promise<HarnessStateFull> {
    return await this.request<HarnessStateFull>(
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

  async getDepositContext(
    owner: PublicKey | string,
    params?: {
      mangoAccount?: PublicKey | string;
      accountNum?: number;
    },
  ): Promise<HarnessDepositContextResponse> {
    const ownerPk = toBase58(owner)!;
    return await this.request<HarnessDepositContextResponse>(
      'GET',
      withQuery(`/state/deposit-context/${encodeURIComponent(ownerPk)}`, {
        mango_account: toBase58(params?.mangoAccount),
        account_num:
          params?.accountNum !== undefined ? String(params.accountNum) : undefined,
      }),
    );
  }

  async getMonitoring(params?: {
    fullHistory?: boolean;
  }): Promise<HarnessMonitoringResponse> {
    return await this.request<HarnessMonitoringResponse>(
      'GET',
      withQuery('/monitoring', {
        full_history: params?.fullHistory ? 'true' : undefined,
      }),
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
