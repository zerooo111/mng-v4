use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anchor_lang::{AnchorDeserialize, InstructionData};
use anyhow::{anyhow, Context, Result};
use fixed::types::I80F48;
use mango_v4::{
    accounts_zerocopy::{KeyedAccountSharedData, LoadZeroCopy},
    error::MangoError,
    health::{new_health_cache, FixedOrderAccountRetriever},
    instructions::{
        CtmEnvelope, ExecutionQueueConfigParams, PerpCancelOrderBySlotPayload,
        PerpPlaceOrderV2Payload, QueuePayloadVariant,
    },
    state::{
        pyth_mainnet_sol_oracle, pyth_mainnet_usdc_oracle, Bank, EventQueue, EventType,
        FillEvent, MangoAccountValue, OutEvent, PerpMarket, PerpMarketIndex, Side,
        EXECUTION_QUEUE_COUNT_OFFSET, EXECUTION_QUEUE_CTM_CAPACITY,
        EXECUTION_QUEUE_CTM_ITEMS_OFFSET, EXECUTION_QUEUE_HEAD_OFFSET,
        EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET, EXECUTION_QUEUE_ITEM_KIND_OFFSET,
        EXECUTION_QUEUE_ITEM_PAYLOAD_LEN_OFFSET, EXECUTION_QUEUE_ITEM_PAYLOAD_OFFSET,
        EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET, EXECUTION_QUEUE_ITEM_SIZE,
        EXECUTION_QUEUE_ITEM_STATUS_OFFSET, EXECUTION_QUEUE_LIQUIDITY_CAPACITY,
        EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET,
        EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET,
    },
};
use serde::{Deserialize, Serialize};
use solana_client::{
    client_error::ClientErrorKind,
    nonblocking::rpc_client::RpcClient,
    rpc_config::RpcSendTransactionConfig,
    rpc_request::{RpcError, RpcResponseErrorData},
};
use solana_program::{hash::hashv, pubkey};
use solana_sdk::{
    account::ReadableAccount,
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    ed25519_program,
    instruction::{AccountMeta, Instruction, InstructionError},
    message::v0::Message as MessageV0,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    signer::keypair::read_keypair_file,
    sysvar,
    transaction::{TransactionError, VersionedTransaction},
};
use tokio::{
    fs,
    sync::{mpsc, Mutex, OwnedSemaphorePermit, RwLock, Semaphore},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tonic::{transport::Server, Code, Request, Response, Status};
use tracing::{debug, info, warn};
use warp::Filter;

use parking_lot::Mutex as PlMutex;
use rust_harness::{ContinuumStateEngine, EngineSnapshot, QueueView, UndoToken};

pub mod proto {
    tonic::include_proto!("ctmsequencer");
}

use proto::{
    ctm_sequencer_relayer_server::{CtmSequencerRelayer, CtmSequencerRelayerServer},
    AccountMeta as AccountMetaProto, SubmitIntentRequest, SubmitIntentResponse,
};

const SPL_MEMO_PROGRAM_ID: Pubkey = pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
const QUEUE_PAYLOAD_VERSION_V1: u8 = 1;
const QUEUE_PAYLOAD_HEADER_LEN: usize = 4;
const PERP_PLACE_ORDER_V2_PAYLOAD_LEN: usize = 45;
const PERP_CANCEL_ORDER_BY_SLOT_PAYLOAD_LEN: usize = 17;
const PERP_BATCH_INTENT_MAX_OPS: usize = 8;
static NEXT_RELAY_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct Config {
    cluster_url: String,
    bind_addr: SocketAddr,
    http_bind_addr: Option<SocketAddr>,
    enable_health_check: bool,
    payer: Arc<Keypair>,
    executor_admin: Arc<Keypair>,
    ctm: Arc<Keypair>,
    program_id: Pubkey,
    executor_group: Option<Pubkey>,
    executor_queue: Option<Pubkey>,
    executor_lane_config_path: Option<PathBuf>,
    executor_lane_cache_path: Option<PathBuf>,
    executor_relay_event_log_path: Option<PathBuf>,
    sequence_state_path: PathBuf,
    min_execute_slot_offset: u64,
    verify_user_signature: bool,
    blockhash_refresh_ms: u64,
    skip_preflight: bool,
    submit_rpc_max_retries: Option<usize>,
    queue_wait_timeout_ms: u64,
    max_inflight: usize,
    queue_soft_limit: u32,
    queue_gap_soft_limit: u64,
    sequence_pending_soft_limit: usize,
    sequence_submitted_stale_ms: u64,
    sequence_submit_watch_poll_ms: u64,
    prioritization_fee: u64,
    event_sink_url: Option<String>,
    harness_base_url: Option<String>,
    harness_health_timeout_ms: u64,
    harness_health_cache_ms: u64,
    harness_health_max_age_ms: u64,
    harness_reject_market_drift: bool,
    executor_enabled: bool,
    executor_interval_ms: u64,
    executor_busy_interval_ms: u64,
    executor_head_lock_ms: u64,
    executor_head_refresh_ms: u64,
    executor_pending_timeout_ms: u64,
    executor_status_poll_ms: u64,
    executor_target_lane_fanout: usize,
    executor_head_scan_items: usize,
    executor_max_pending_txs: usize,
    executor_pipeline_max_per_head: usize,
    executor_same_head_send_interval_ms: u64,
    executor_rpc_max_retries: Option<usize>,
    executor_max_items: u16,
    executor_prioritization_fee: u64,
    executor_skip_preflight: bool,
    executor_match_head_only: bool,
    executor_safe_speculative: bool,
    executor_failure_threshold: u64,
    executor_failure_backoff_ms: u64,
    executor_include_legacy_fixed_hash: bool,
    executor_dynamic_lanes_refresh_ms: u64,
    executor_dynamic_lanes_max_events: usize,
    executor_auto_drop_expired_heads: bool,
    executor_admin_tx_timeout_ms: u64,
    executor_expired_head_drop_batch_max: usize,
    executor_expired_head_drop_grace_secs: u64,
    executor_gap_recovery_drop_batch_max: usize,
    /// Maximum consecutive failures for the same sequence before the executor
    /// proactively fires an admin-drop tx. This handles the case where the
    /// normal auto-drop path (via signature status + log inspection) fails
    /// due to RPC rate-limiting or other transient errors.
    executor_max_sequence_failures: u32,
    /// When a head item has had no matching lane for longer than this many
    /// Solana slots (~400ms each), the executor admin-drops it as a health
    /// failure rather than spinning forever.  Set 0 to disable.
    executor_no_lane_match_drop_slots: u64,
    /// Maximum number of consecutive pending CTM items with the same head hash
    /// to drop in one no-lane-match recovery transaction.
    executor_no_lane_match_drop_batch_max: usize,
    /// When true, the executor optimistically advances the cached queue head
    /// after each successful send_transaction, without waiting for on-chain
    /// confirmation.  This allows back-to-back tx sends at RPC send latency
    /// (~50ms) rather than send+confirm latency (~400ms+).  If the optimistic
    /// assumption is wrong (tx dropped), the next periodic inspect corrects it.
    executor_optimistic_advance: bool,
    /// Interval at which the engine consumes perp event queue events. When 0,
    /// the periodic consumer is disabled. Default 2000ms (2s). Each pass
    /// consumes up to executor_perp_consume_limit events (capped at 8 by the
    /// on-chain program). Required to free OO slots after fills.
    executor_perp_consume_interval_ms: u64,
    /// Max events to consume per perp_consume_events instruction. Capped at 8
    /// by the on-chain program. Default 8.
    executor_perp_consume_limit: usize,
    /// URL to probe for bridge liveness on the /metrics health gauge. Set to
    /// empty to disable bridge probing entirely.
    bridge_health_url: Option<String>,
    /// Per-request HTTP timeout for the bridge health probe, in milliseconds.
    bridge_health_probe_timeout_ms: u64,
    /// How often the background health prober runs, in milliseconds.
    health_probe_interval_ms: u64,
    /// How often the background relayer-balance poller runs, in milliseconds.
    /// Set to 0 to disable.
    balance_poll_interval_ms: u64,
    /// Maximum age (ms) of the executor heartbeat before the executor is
    /// reported unhealthy on /metrics.
    executor_stale_threshold_ms: u64,
    /// Maximum age (ms) of the perp event consumer heartbeat before the event
    /// cranker is reported unhealthy on /metrics.
    event_cranker_stale_threshold_ms: u64,
    /// URL the "processed" latency prober polls to read the on-chain
    /// reconciliation watermark. Empty disables that tracker.
    latency_processed_url: Option<String>,
    /// JSON path for the processed watermark inside the response (default
    /// `data.last_processed_sequence` for the queue endpoint).
    latency_processed_field: String,
    /// URL the "optimistic" latency prober polls to read the harness
    /// orderbook optimistic watermark. Empty disables that tracker.
    latency_optimistic_url: Option<String>,
    /// JSON path for the optimistic watermark inside the response (default
    /// `data.watermarks.optimistic_seq` for the markets endpoint).
    latency_optimistic_field: String,
    /// How often each latency prober hits the harness, in milliseconds.
    /// Shared by both processed and optimistic probers.
    latency_probe_interval_ms: u64,
    /// HTTP timeout per latency-probe request, in milliseconds.
    latency_probe_timeout_ms: u64,
    /// Maximum number of in-flight pending sequences in each latency tracker
    /// before the oldest are evicted.
    latency_pending_max_size: usize,
    /// Maximum age (ms) of a pending entry before it is force-expired and
    /// dropped from the tracker (a.k.a. the "watermark didn't catch up"
    /// timeout).
    latency_pending_max_age_ms: u64,
    /// Sliding-window capacity for completed latency samples used to compute
    /// p50/p95. Shared across all three latency series.
    latency_samples_capacity: usize,
    /// Bounded mpsc channel capacity for the background submitter pool.
    /// Controls how many in-flight enqueue txs can be queued waiting for an
    /// RPC submit slot. When the channel is full, new submit_intent calls
    /// are rejected with RESOURCE_EXHAUSTED until the queue drains.
    bg_submit_channel_cap: usize,
    /// Number of background submitter worker tasks. Each worker serially
    /// drains one PendingSubmit at a time and calls
    /// `rpc.send_transaction_with_config`. Total parallelism = workers.
    bg_submit_workers: usize,
    /// Maximum retries per PendingSubmit on transient RPC failures.
    /// Hard failures (signature verify, account not found, program reject,
    /// etc.) skip retries and immediately roll back the local sequence.
    bg_submit_max_retries: u32,
    /// Base backoff in milliseconds for the exponential retry on transient
    /// RPC failures. Doubled per attempt, capped at 6 doublings.
    bg_submit_retry_base_ms: u64,
    /// Phase 3.5: TTL (in milliseconds) for the margin-check account
    /// cache. Set to 0 to disable caching entirely (every margin check
    /// hits RPC). 500 ms is the recommended starting point — short
    /// enough to bound oracle staleness, long enough to give ~100% cache
    /// hit rate at sustained tps.
    margin_cache_ttl_ms: u64,
    /// Maximum number of cached margin-check accounts. Soft cap; the
    /// oldest entries are evicted when the cache exceeds this size.
    margin_cache_max_entries: usize,
    /// Phase 2: enable in-process optimistic state via the embedded
    /// rust-harness crate. Defaults to false so the legacy HTTP path is
    /// preserved until soaked.
    local_state_enabled: bool,
    /// URL the relayer hits at startup to bootstrap its in-process state
    /// from the legacy harness. Only consulted when local_state_enabled is
    /// true. The relayer makes ONE GET request here at startup and never
    /// again — Phase 5 replaces this with a Rust on-chain reader.
    local_state_bootstrap_url: String,
    /// Per-request HTTP timeout for the bootstrap fetch (ms).
    local_state_bootstrap_timeout_ms: u64,
}

impl Config {
    fn from_env(enable_health_check_flag: bool) -> Result<Self> {
        let cluster_url = std::env::var("CLUSTER_URL_OVERRIDE")
            .or_else(|_| std::env::var("MB_CLUSTER_URL"))
            .context("CLUSTER_URL_OVERRIDE or MB_CLUSTER_URL is required")?;
        let bind_addr = parse_socket_addr(
            std::env::var("CTM_RELAYER_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:9090".into()),
        )?;
        let http_bind_addr = match std::env::var("CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR") {
            Ok(value) if !value.trim().is_empty() => Some(parse_socket_addr(value)?),
            _ => None,
        };
        let enable_health_check =
            enable_health_check_flag || parse_bool_env("CTM_RELAYER_ENABLE_HEALTH_CHECK", false);
        let payer = Arc::new(read_keypair_env(
            "CTM_RELAYER_PAYER_KEYPAIR",
            "MB_PAYER_KEYPAIR",
        )?);
        let executor_admin = Arc::new(
            read_keypair_env("EXECUTION_QUEUE_ADMIN_KEYPAIR", "CTM_RELAYER_PAYER_KEYPAIR")
                .or_else(|_| read_keypair_env("CTM_RELAYER_PAYER_KEYPAIR", "MB_PAYER_KEYPAIR"))?,
        );
        let ctm = Arc::new(
            read_keypair_env("CTM_RELAYER_CTM_KEYPAIR", "CTM_RELAYER_PAYER_KEYPAIR")
                .or_else(|_| read_keypair_env("CTM_RELAYER_PAYER_KEYPAIR", "MB_PAYER_KEYPAIR"))?,
        );
        let program_id = std::env::var("CTM_RELAYER_PROGRAM_ID")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| Pubkey::from_str(&v))
            .transpose()?
            .unwrap_or(mango_v4::id());
        let executor_group = parse_optional_pubkey_env("EXECUTION_QUEUE_GROUP_PK")?;
        let executor_queue = parse_optional_pubkey_env("EXECUTION_QUEUE_PK")?;
        let _execution_queue_buffer = parse_optional_pubkey_env("EXECUTION_QUEUE_BUFFER_PK")?
            .or(executor_queue)
            .unwrap_or_default();
        if executor_group.is_some() != executor_queue.is_some() {
            return Err(anyhow!(
                "EXECUTION_QUEUE_GROUP_PK and EXECUTION_QUEUE_PK must be set together"
            ));
        }
        let executor_lane_config_path = std::env::var("EXECUTION_QUEUE_CRANK_LANES_JSON_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        let executor_lane_cache_path = std::env::var("EXECUTION_QUEUE_LANE_CACHE_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        let executor_relay_event_log_path =
            std::env::var("EXECUTION_QUEUE_CRANK_RELAY_EVENT_LOG_PATH")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from);
        let sequence_state_path = PathBuf::from(
            std::env::var("CTM_RELAYER_SEQUENCE_STATE_PATH")
                .unwrap_or_else(|_| "/tmp/ctm-sequences.json".into()),
        );
        let min_execute_slot_offset = parse_u64_env("CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET", 1)?;
        let verify_user_signature = parse_bool_env("CTM_RELAYER_VERIFY_USER_SIGNATURE", true);
        let blockhash_refresh_ms = parse_u64_env("CTM_RELAYER_BLOCKHASH_CACHE_MS", 250)?;
        let skip_preflight = matches!(
            std::env::var("CTM_RELAYER_SUBMIT_MODE").ok().as_deref(),
            Some("fast")
        );
        let submit_rpc_max_retries = parse_optional_usize_env("CTM_RELAYER_RPC_MAX_RETRIES")?;
        let queue_wait_timeout_ms = parse_u64_env("CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS", 500)?;
        let max_inflight = parse_u64_env("CTM_RELAYER_MAX_INFLIGHT", 128)? as usize;
        let queue_soft_limit = parse_u64_env("CTM_RELAYER_QUEUE_SOFT_LIMIT", 0)? as u32;
        let queue_gap_soft_limit = parse_u64_env("CTM_RELAYER_QUEUE_GAP_SOFT_LIMIT", 256)?;
        let sequence_pending_soft_limit =
            parse_u64_env("CTM_RELAYER_SEQUENCE_PENDING_SOFT_LIMIT", 0)? as usize;
        let sequence_submitted_stale_ms =
            parse_u64_env("CTM_RELAYER_SEQUENCE_SUBMITTED_STALE_MS", 10_000)?;
        let sequence_submit_watch_poll_ms =
            parse_u64_env("CTM_RELAYER_SEQUENCE_SUBMIT_WATCH_POLL_MS", 250)?;
        let prioritization_fee = parse_u64_env("CTM_RELAYER_PRIORITIZATION_FEE", 0)?;
        let event_sink_url = std::env::var("CTM_RELAYER_EVENT_SINK_URL")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let harness_base_url = std::env::var("CTM_RELAYER_HARNESS_BASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim_end_matches('/').to_string())
            .or_else(|| derive_harness_base_url(event_sink_url.as_deref()));
        let harness_health_timeout_ms = parse_u64_env("CTM_RELAYER_HARNESS_TIMEOUT_MS", 1_000)?;
        let harness_health_cache_ms = parse_u64_env("CTM_RELAYER_HARNESS_HEALTH_CACHE_MS", 250)?;
        let harness_health_max_age_ms = parse_u64_env("CTM_RELAYER_HARNESS_MAX_STALE_MS", 30_000)?;
        let harness_reject_market_drift =
            parse_bool_env("CTM_RELAYER_HARNESS_REJECT_MARKET_DRIFT", true);
        let executor_enabled = parse_bool_env("EXECUTION_QUEUE_ENGINE_ENABLED", true)
            && executor_group.is_some()
            && executor_queue.is_some();
        let executor_interval_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_INTERVAL_MS", 25)?;
        let executor_busy_interval_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_BUSY_INTERVAL_MS", 1)?;
        let executor_head_lock_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_HEAD_LOCK_MS", 30)?;
        let executor_head_refresh_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_HEAD_REFRESH_MS", 25)?;
        let executor_pending_timeout_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_PENDING_TIMEOUT_MS", 500)?;
        let executor_status_poll_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_STATUS_POLL_MS", 10)?;
        let executor_target_lane_fanout =
            parse_u64_env("EXECUTION_QUEUE_CRANK_TARGET_LANE_FANOUT", 1)? as usize;
        let executor_head_scan_items =
            parse_u64_env("EXECUTION_QUEUE_CRANK_HEAD_SCAN_ITEMS", 32)? as usize;
        let executor_max_pending_txs =
            parse_u64_env("EXECUTION_QUEUE_CRANK_MAX_PENDING_TXS", 4)? as usize;
        let executor_pipeline_max_per_head =
            parse_u64_env("EXECUTION_QUEUE_CRANK_PIPELINE_MAX_PER_HEAD", 2)? as usize;
        let executor_same_head_send_interval_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_SAME_HEAD_SEND_INTERVAL_MS", 250)?;
        let executor_rpc_max_retries =
            parse_optional_usize_env("EXECUTION_QUEUE_CRANK_RPC_MAX_RETRIES")?;
        let executor_max_items = parse_u64_env("EXECUTION_QUEUE_CRANK_MAX_ITEMS", 32)? as u16;
        let executor_prioritization_fee =
            parse_u64_env("EXECUTION_QUEUE_CRANK_PRIORITIZATION_FEE", 0)?;
        let executor_skip_preflight = parse_bool_env("EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT", true);
        let executor_match_head_only =
            parse_bool_env("EXECUTION_QUEUE_CRANK_MATCH_HEAD_ONLY", true);
        let executor_safe_speculative =
            parse_bool_env("EXECUTION_QUEUE_CRANK_SAFE_SPECULATIVE", true);
        let executor_failure_threshold =
            parse_u64_env("EXECUTION_QUEUE_CRANK_LANE_FAILURE_THRESHOLD", 3)?;
        let executor_failure_backoff_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_LANE_FAILURE_BACKOFF_MS", 10_000)?;
        let executor_include_legacy_fixed_hash =
            parse_bool_env("EXECUTION_QUEUE_CRANK_INCLUDE_LEGACY_FIXED_HASH", true);
        let executor_dynamic_lanes_refresh_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_DYNAMIC_LANES_REFRESH_MS", 500)?;
        let executor_dynamic_lanes_max_events =
            parse_u64_env("EXECUTION_QUEUE_CRANK_DYNAMIC_LANES_MAX_EVENTS", 4096)? as usize;
        let executor_auto_drop_expired_heads =
            parse_bool_env("EXECUTION_QUEUE_CRANK_AUTO_DROP_EXPIRED_HEADS", true);
        let executor_admin_tx_timeout_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_ADMIN_TX_TIMEOUT_MS", 5_000)?;
        let executor_expired_head_drop_batch_max =
            parse_u64_env("EXECUTION_QUEUE_CRANK_EXPIRED_HEAD_DROP_BATCH_MAX", 8)? as usize;
        let executor_expired_head_drop_grace_secs =
            parse_u64_env("EXECUTION_QUEUE_CRANK_EXPIRED_HEAD_DROP_GRACE_SECS", 5)?;
        let executor_gap_recovery_drop_batch_max =
            parse_u64_env("EXECUTION_QUEUE_CRANK_GAP_RECOVERY_DROP_BATCH_MAX", 8)? as usize;
        let executor_max_sequence_failures =
            parse_u64_env("EXECUTION_QUEUE_CRANK_MAX_SEQUENCE_FAILURES", 5)? as u32;
        let executor_no_lane_match_drop_slots =
            parse_u64_env("EXECUTION_QUEUE_CRANK_NO_LANE_MATCH_DROP_SLOTS", 2)?;
        let executor_no_lane_match_drop_batch_max =
            parse_u64_env("EXECUTION_QUEUE_CRANK_NO_LANE_MATCH_DROP_BATCH_MAX", 16)? as usize;
        let executor_optimistic_advance = std::env::var("EXECUTION_QUEUE_CRANK_OPTIMISTIC_ADVANCE")
            .unwrap_or_else(|_| "false".to_string())
            .eq_ignore_ascii_case("true");
        let executor_perp_consume_interval_ms =
            parse_u64_env("EXECUTION_QUEUE_PERP_CONSUME_INTERVAL_MS", 2_000)?;
        let executor_perp_consume_limit =
            parse_u64_env("EXECUTION_QUEUE_PERP_CONSUME_LIMIT", 8)?.min(8) as usize;
        let bridge_health_url = std::env::var("CTM_RELAYER_BRIDGE_HEALTH_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| Some("http://127.0.0.1:9092/healthz".to_string()));
        let bridge_health_probe_timeout_ms =
            parse_u64_env("CTM_RELAYER_BRIDGE_HEALTH_PROBE_TIMEOUT_MS", 1_000)?;
        let health_probe_interval_ms =
            parse_u64_env("CTM_RELAYER_HEALTH_PROBE_INTERVAL_MS", 5_000)?;
        let balance_poll_interval_ms =
            parse_u64_env("CTM_RELAYER_BALANCE_POLL_INTERVAL_MS", 30_000)?;
        let executor_stale_threshold_ms =
            parse_u64_env("CTM_RELAYER_EXECUTOR_STALE_MS", 5_000)?;
        let event_cranker_stale_threshold_ms = parse_u64_env(
            "CTM_RELAYER_EVENT_CRANKER_STALE_MS",
            executor_perp_consume_interval_ms.saturating_mul(5).max(10_000),
        )?;
        // "Processed" tracker — uses the lightweight queue endpoint, which
        // doesn't trigger an upstream RPC fetch on the harness side. This
        // measures `ingress -> harness has reconciled past my sequence on
        // chain`, which is dominated by Solana slot inclusion + harness
        // reconciliation lag. Typically ~1 s.
        let latency_processed_url = std::env::var("CTM_RELAYER_LATENCY_PROCESSED_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| {
                Some("http://127.0.0.1:9091/state/queue/0?view=optimistic".to_string())
            });
        let latency_processed_field = std::env::var("CTM_RELAYER_LATENCY_PROCESSED_FIELD")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "data.last_processed_sequence".to_string());
        // "Optimistic" tracker — uses the markets endpoint's optimistic_seq
        // watermark. The harness updates this from the relay-intent event
        // stream BEFORE on-chain confirmation, so it measures
        // `ingress -> harness internal optimistic state shows my sequence`.
        // Should be ~50-250ms.
        let latency_optimistic_url = std::env::var("CTM_RELAYER_LATENCY_OPTIMISTIC_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| {
                Some("http://127.0.0.1:9091/state/markets/0?view=optimistic".to_string())
            });
        let latency_optimistic_field = std::env::var("CTM_RELAYER_LATENCY_OPTIMISTIC_FIELD")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "data.watermarks.optimistic_seq".to_string());
        // Phase 1.3: tightened from 250ms to 25ms so latency measurements
        // have ≤25ms of polling jitter. The harness queue endpoint is ~200
        // bytes and doesn't trigger an upstream RPC fetch, so 10× the
        // polling rate adds negligible load.
        let latency_probe_interval_ms =
            parse_u64_env("CTM_RELAYER_LATENCY_PROBE_INTERVAL_MS", 25)?;
        let latency_probe_timeout_ms =
            parse_u64_env("CTM_RELAYER_LATENCY_PROBE_TIMEOUT_MS", 2_000)?;
        let latency_pending_max_size =
            parse_u64_env("CTM_RELAYER_LATENCY_PENDING_MAX_SIZE", 100_000)? as usize;
        let latency_pending_max_age_ms =
            parse_u64_env("CTM_RELAYER_LATENCY_PENDING_MAX_AGE_MS", 60_000)?;
        let latency_samples_capacity =
            parse_u64_env("CTM_RELAYER_LATENCY_SAMPLES_CAPACITY", 4_096)? as usize;
        let bg_submit_channel_cap =
            parse_u64_env("CTM_RELAYER_BG_SUBMIT_CHANNEL_CAP", 1_024)? as usize;
        let bg_submit_workers =
            parse_u64_env("CTM_RELAYER_BG_SUBMIT_WORKERS", 16)? as usize;
        let bg_submit_max_retries =
            parse_u64_env("CTM_RELAYER_BG_SUBMIT_MAX_RETRIES", 5)? as u32;
        let bg_submit_retry_base_ms =
            parse_u64_env("CTM_RELAYER_BG_SUBMIT_RETRY_BASE_MS", 50)?;
        let margin_cache_ttl_ms =
            parse_u64_env("CTM_RELAYER_MARGIN_CACHE_TTL_MS", 500)?;
        let margin_cache_max_entries =
            parse_u64_env("CTM_RELAYER_MARGIN_CACHE_MAX_ENTRIES", 4096)? as usize;
        let local_state_enabled = parse_bool_env("CTM_RELAYER_LOCAL_STATE", false);
        let local_state_bootstrap_url = std::env::var("CTM_RELAYER_LOCAL_STATE_BOOTSTRAP_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:9091/state/full?view=confirmed".to_string());
        let local_state_bootstrap_timeout_ms =
            parse_u64_env("CTM_RELAYER_LOCAL_STATE_BOOTSTRAP_TIMEOUT_MS", 30_000)?;

        Ok(Self {
            cluster_url,
            bind_addr,
            http_bind_addr,
            enable_health_check,
            payer,
            executor_admin,
            ctm,
            program_id,
            executor_group,
            executor_queue,
            executor_lane_config_path,
            executor_lane_cache_path,
            executor_relay_event_log_path,
            sequence_state_path,
            min_execute_slot_offset,
            verify_user_signature,
            blockhash_refresh_ms,
            skip_preflight,
            submit_rpc_max_retries,
            queue_wait_timeout_ms,
            max_inflight,
            queue_soft_limit,
            queue_gap_soft_limit,
            sequence_pending_soft_limit,
            sequence_submitted_stale_ms,
            sequence_submit_watch_poll_ms,
            prioritization_fee,
            event_sink_url,
            harness_base_url,
            harness_health_timeout_ms,
            harness_health_cache_ms,
            harness_health_max_age_ms,
            harness_reject_market_drift,
            executor_enabled,
            executor_interval_ms,
            executor_busy_interval_ms,
            executor_head_lock_ms,
            executor_head_refresh_ms,
            executor_pending_timeout_ms,
            executor_status_poll_ms,
            executor_target_lane_fanout,
            executor_head_scan_items,
            executor_max_pending_txs,
            executor_pipeline_max_per_head,
            executor_same_head_send_interval_ms,
            executor_rpc_max_retries,
            executor_max_items,
            executor_prioritization_fee,
            executor_skip_preflight,
            executor_match_head_only,
            executor_safe_speculative,
            executor_failure_threshold,
            executor_failure_backoff_ms,
            executor_include_legacy_fixed_hash,
            executor_dynamic_lanes_refresh_ms,
            executor_dynamic_lanes_max_events,
            executor_auto_drop_expired_heads,
            executor_admin_tx_timeout_ms,
            executor_expired_head_drop_batch_max,
            executor_expired_head_drop_grace_secs,
            executor_gap_recovery_drop_batch_max,
            executor_max_sequence_failures,
            executor_no_lane_match_drop_slots,
            executor_no_lane_match_drop_batch_max,
            executor_optimistic_advance,
            executor_perp_consume_interval_ms,
            executor_perp_consume_limit,
            bridge_health_url,
            bridge_health_probe_timeout_ms,
            health_probe_interval_ms,
            balance_poll_interval_ms,
            executor_stale_threshold_ms,
            event_cranker_stale_threshold_ms,
            latency_processed_url,
            latency_processed_field,
            latency_optimistic_url,
            latency_optimistic_field,
            latency_probe_interval_ms,
            latency_probe_timeout_ms,
            latency_pending_max_size,
            latency_pending_max_age_ms,
            latency_samples_capacity,
            bg_submit_channel_cap,
            bg_submit_workers,
            bg_submit_max_retries,
            bg_submit_retry_base_ms,
            margin_cache_ttl_ms,
            margin_cache_max_entries,
            local_state_enabled,
            local_state_bootstrap_url,
            local_state_bootstrap_timeout_ms,
        })
    }
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    requests_ok: AtomicU64,
    requests_error: AtomicU64,
    inflight: AtomicU64,
    sequence_flushes: AtomicU64,
    submit_total_ms: AtomicU64,
    submit_count: AtomicU64,
    submit_parse_total_ms: AtomicU64,
    submit_prepare_total_ms: AtomicU64,
    submit_send_total_ms: AtomicU64,
    harness_submit_rejects: AtomicU64,
    execute_attempts: AtomicU64,
    execute_sent: AtomicU64,
    execute_errors: AtomicU64,
    execute_head_missing: AtomicU64,
    execute_head_blocked: AtomicU64,
    execute_no_lane_match: AtomicU64,
    execute_confirmed_no_advance: AtomicU64,
    execute_lane_suppressed: AtomicU64,
    execute_targeted: AtomicU64,
    execute_speculative: AtomicU64,
    execute_pipeline_sent: AtomicU64,
    execute_send_suppressed_pending: AtomicU64,
    execute_head_advanced: AtomicU64,
    execute_head_advance_items: AtomicU64,
    perp_events_consumed: AtomicU64,

    // ---- New: lifetime totals incremented by hot path / executor.
    // Producers only ever do `fetch_add(_, Relaxed)`. The sampler task derives
    // tps and 60s windows from these without ever blocking the hot path.
    /// Every gRPC submit_intent invocation increments this exactly once at the
    /// boundary, before any work (queue admit, parse, submit). Captures real
    /// arrival rate including queue-timeout rejects.
    ingress_total: AtomicU64,
    /// submit_intent invocations whose end-to-end result was Ok(_).
    ingress_accepted_total: AtomicU64,
    /// submit_intent invocations whose end-to-end result was Err(_).
    ingress_rejected_total: AtomicU64,
    /// Number of execution-queue items that successfully landed on chain.
    /// Bumped at the executor head-advance site by the advance delta.
    executed_total: AtomicU64,
    /// Number of executor-dispatched txs whose signature status reported an
    /// on-chain failure (the tx landed but the program errored).
    executed_failed_total: AtomicU64,

    // ---- Background submitter pool counters (Phase 1.1).
    /// Lifetime count of bg-submitted txs that succeeded on the first try
    /// or after retries.
    bg_submit_ok_total: AtomicU64,
    /// Lifetime count of bg-submitted txs that hit a hard failure and were
    /// rolled back.
    bg_submit_failed_total: AtomicU64,
    /// Lifetime count of transient failures observed by bg workers (each
    /// retry attempt counts once).
    bg_submit_transient_total: AtomicU64,
    /// Lifetime count of submit_intent calls rejected because the bg
    /// channel was full at try_send time.
    bg_submit_channel_full_total: AtomicU64,
    /// Lifetime count of retries dropped because the retry-spawned task
    /// could not re-enqueue (channel closed).
    bg_submit_retry_dropped_total: AtomicU64,
    /// Current count of in-flight PendingSubmits in the bg channel +
    /// workers. Bumped on enqueue; decremented on terminal outcome
    /// (success or hard failure). Useful as a real-time pool depth gauge.
    bg_submit_inflight: AtomicU64,

    // ---- Phase 3.5 margin-check account cache.
    /// Lifetime count of cache hits in `fetch_margin_check_account_map`.
    margin_cache_hit_total: AtomicU64,
    /// Lifetime count of cache misses (resulted in an RPC fetch).
    margin_cache_miss_total: AtomicU64,
    /// Lifetime count of evictions (entry exceeded the soft size cap or
    /// expired during a sweep).
    margin_cache_evicted_total: AtomicU64,
    /// Current size of the cache.
    margin_cache_size: AtomicU64,

    // ---- Sampler-published rate / windowed values. Only the sampler task
    // writes these; the /metrics renderer only reads them.
    /// Ingress rate sampled over the last 10s, scaled by 1000 (so a value of
    /// 12_345 means 12.345 tps). Avoids needing floats in atomics.
    ingress_tps_10s_milli: AtomicU64,
    ingress_accepted_60s: AtomicU64,
    ingress_rejected_60s: AtomicU64,
    /// Executed (head-advance items) rate sampled over the last 10s, scaled
    /// by 1000.
    executed_tps_10s_milli: AtomicU64,
    executed_60s: AtomicU64,
    executed_failed_60s: AtomicU64,
    /// Snapshot of the executor's current on-chain queue depth (queue_count).
    /// Mirrors ExecutorState::current_queue_count for /metrics consumers.
    execution_queue_depth: AtomicU64,
    /// Approximate count of distinct user_owner pubkeys observed on the
    /// ingress path in the last 60 seconds, computed by the sampler task from
    /// Engine::unique_addresses.
    unique_addresses_60s: AtomicU64,
    /// Wall-clock ms when the metrics sampler last ran. Used by external
    /// monitors to detect a stalled sampler.
    sampler_last_tick_ms: AtomicU64,

    // ---- Health gauges. Background prober writes; /metrics reader reads.
    /// 1 if the metrics endpoint can answer, ie this process is running. The
    /// sampler simply pins this to 1 each tick. External monitors that scrape
    /// /metrics get an implicit liveness signal regardless.
    relayer_healthy: AtomicU64,
    /// 1 when the bridge /healthz probe last succeeded; 0 otherwise.
    bridge_healthy: AtomicU64,
    /// 1 when the executor heartbeat is fresher than the configured stale
    /// threshold; 0 otherwise.
    executor_healthy: AtomicU64,
    /// 1 when the perp event cranker heartbeat is fresher than the configured
    /// stale threshold (or when the cranker is disabled by config).
    event_cranker_healthy: AtomicU64,
    /// Last reported relayer payer balance, in lamports. 0 until the first
    /// poll succeeds.
    relayer_balance_lamports: AtomicU64,
    /// Wall-clock ms when the balance was last refreshed.
    relayer_balance_last_ms: AtomicU64,
    /// Wall-clock ms when the executor loop last completed an iteration.
    executor_last_tick_ms: AtomicU64,
    /// Wall-clock ms when the perp event consumer loop last completed an
    /// iteration.
    event_cranker_last_tick_ms: AtomicU64,

    // ---- Ingress -> "submitted" latency (relayer-only).
    // Hot path pushes elapsed_ms into submit_latency_samples ring; sampler
    // computes percentiles 1Hz and publishes here. Captures only successful
    // submissions, since rejected requests don't represent a meaningful
    // submit pipeline timing.
    /// Median latency from order ingress to the moment the relayer's
    /// submit_intent returned Ok (i.e. the relayer signed and dispatched the
    /// enqueue tx via RPC).
    latency_ingress_to_submitted_p50_ms: AtomicU64,
    latency_ingress_to_submitted_p95_ms: AtomicU64,
    latency_ingress_to_submitted_max_ms: AtomicU64,
    latency_ingress_to_submitted_samples: AtomicU64,
    /// Per-stage published gauges (sampler-written; written from
    /// stage_*_latency histograms once per second).
    latency_stage_parse_p50_ms: AtomicU64,
    latency_stage_parse_p95_ms: AtomicU64,
    latency_stage_margin_check_p50_ms: AtomicU64,
    latency_stage_margin_check_p95_ms: AtomicU64,
    latency_stage_sign_p50_ms: AtomicU64,
    latency_stage_sign_p95_ms: AtomicU64,
    latency_stage_event_dispatch_p50_ms: AtomicU64,
    latency_stage_event_dispatch_p95_ms: AtomicU64,
    latency_stage_bg_rpc_submit_p50_ms: AtomicU64,
    latency_stage_bg_rpc_submit_p95_ms: AtomicU64,

    // ---- Ingress -> "optimistic" latency.
    // Driven by LatencyTracker against the markets endpoint's
    // data.watermarks.optimistic_seq. Captures the time until the harness
    // applies the relayer's relay-intent event into its optimistic state.
    latency_ingress_to_optimistic_p50_ms: AtomicU64,
    latency_ingress_to_optimistic_p95_ms: AtomicU64,
    latency_ingress_to_optimistic_max_ms: AtomicU64,
    latency_ingress_to_optimistic_samples: AtomicU64,
    latency_optimistic_completed_total: AtomicU64,
    latency_optimistic_expired_total: AtomicU64,
    latency_optimistic_pending_inflight: AtomicU64,
    /// Last optimistic watermark observed by the optimistic prober.
    harness_optimistic_watermark_seq: AtomicU64,
    /// Wall-clock ms when the optimistic prober last successfully read the
    /// optimistic watermark.
    latency_optimistic_prober_last_ms: AtomicU64,

    // ---- Ingress -> "processed" latency.
    // Driven by LatencyTracker against the queue endpoint's
    // data.last_processed_sequence. Captures the time until the harness has
    // reconciled past the user's sequence from on-chain state.
    latency_ingress_to_processed_p50_ms: AtomicU64,
    latency_ingress_to_processed_p95_ms: AtomicU64,
    latency_ingress_to_processed_max_ms: AtomicU64,
    latency_ingress_to_processed_samples: AtomicU64,
    latency_processed_completed_total: AtomicU64,
    latency_processed_expired_total: AtomicU64,
    latency_processed_pending_inflight: AtomicU64,
    /// Last processed watermark observed by the processed prober.
    harness_processed_watermark_seq: AtomicU64,
    /// Wall-clock ms when the processed prober last successfully read the
    /// processed watermark.
    latency_processed_prober_last_ms: AtomicU64,

    // ---- Hot-path-owned latency histograms.
    // Each LatencyHistogram is a fixed-size sample ring + a write counter.
    // The hot path acquires the histogram's std mutex once per recorded
    // sample (the same pattern as note_user_owner). The metrics sampler
    // computes percentiles once per second under a brief read lock. No
    // lock is ever held across an `.await`.
    /// End-to-end submit_intent latency for accepted intents.
    submit_latency: LatencyHistogram,
    /// Per-stage histograms for the submit_intent pipeline. Phase 0
    /// instrumentation: tells us where the time goes inside the relayer.
    stage_parse_latency: LatencyHistogram,
    stage_margin_check_latency: LatencyHistogram,
    stage_sign_latency: LatencyHistogram,
    /// Time from `tx` signed to the bg-submitter receiving it from the mpsc
    /// channel — i.e. the dispatch hand-off cost.
    stage_event_dispatch_latency: LatencyHistogram,
    /// Time spent inside `rpc.send_transaction_with_config` from the
    /// background submitter worker. This is no longer on the user-visible
    /// critical path after Phase 1.1; we keep it as a gauge for ops.
    stage_bg_rpc_submit_latency: LatencyHistogram,
}

impl Metrics {
    fn observe_submit(&self, elapsed: Duration, ok: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.submit_total_ms
            .fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
        self.submit_count.fetch_add(1, Ordering::Relaxed);
        if ok {
            self.requests_ok.fetch_add(1, Ordering::Relaxed);
        } else {
            self.requests_error.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Hot-path: bumped at the very top of submit_intent before any other
    /// work. Single relaxed fetch_add per request.
    #[inline(always)]
    fn record_ingress(&self) {
        self.ingress_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Hot-path: bumped exactly once per submit_intent return. Single
    /// branch + fetch_add.
    #[inline(always)]
    fn record_ingress_outcome(&self, ok: bool) {
        if ok {
            self.ingress_accepted_total.fetch_add(1, Ordering::Relaxed);
        } else {
            self.ingress_rejected_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Bumped from the executor when the on-chain queue head advances by
    /// `delta` items.
    #[inline(always)]
    fn record_executed(&self, delta: u64) {
        if delta > 0 {
            self.executed_total.fetch_add(delta, Ordering::Relaxed);
        }
    }

    /// Bumped once per executor-dispatched tx whose signature status reports
    /// an on-chain failure.
    #[inline(always)]
    fn record_executed_failed(&self) {
        self.executed_failed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Heartbeat for the executor loop. Single relaxed store, no I/O.
    #[inline(always)]
    fn tick_executor(&self, now_ms: u64) {
        self.executor_last_tick_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Heartbeat for the perp event consumer loop.
    #[inline(always)]
    fn tick_event_cranker(&self, now_ms: u64) {
        self.event_cranker_last_tick_ms
            .store(now_ms, Ordering::Relaxed);
    }

    /// Hot-path: push a submit latency sample. Delegates to the
    /// LatencyHistogram on `submit_latency`.
    #[inline]
    fn record_submit_latency(&self, ms: u64) {
        self.submit_latency.record(ms);
    }

    /// Sampler-only path (1 Hz): snapshot the submit-latency ring and
    /// compute percentile statistics.
    #[inline]
    fn compute_submit_summary(&self) -> LatencySummary {
        self.submit_latency.compute_summary()
    }

    /// Legacy stage averages — kept around so the existing render output for
    /// `submit_*_avg_ms` continues to compile, even though the call from
    /// submit_intent_inner was removed in Phase 1.1 (we now have per-stage
    /// LatencyHistograms instead). Marked dead-code-allowed; safe to delete
    /// once the avg_ms render lines are retired.
    #[allow(dead_code)]
    fn observe_submit_stages(&self, parse: Duration, prepare: Duration, send: Duration) {
        self.submit_parse_total_ms
            .fetch_add(parse.as_millis() as u64, Ordering::Relaxed);
        self.submit_prepare_total_ms
            .fetch_add(prepare.as_millis() as u64, Ordering::Relaxed);
        self.submit_send_total_ms
            .fetch_add(send.as_millis() as u64, Ordering::Relaxed);
    }

    fn render(&self) -> String {
        let count = self.submit_count.load(Ordering::Relaxed);
        let total_ms = self.submit_total_ms.load(Ordering::Relaxed);
        let avg = if count == 0 {
            0.0
        } else {
            total_ms as f64 / count as f64
        };
        let parse_avg = avg_ms(self.submit_parse_total_ms.load(Ordering::Relaxed), count);
        let prepare_avg = avg_ms(self.submit_prepare_total_ms.load(Ordering::Relaxed), count);
        let send_avg = avg_ms(self.submit_send_total_ms.load(Ordering::Relaxed), count);
        [
            "# TYPE execution_engine_requests_total counter".to_string(),
            format!(
                "execution_engine_requests_total {}",
                self.requests_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_requests_ok_total counter".to_string(),
            format!(
                "execution_engine_requests_ok_total {}",
                self.requests_ok.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_requests_error_total counter".to_string(),
            format!(
                "execution_engine_requests_error_total {}",
                self.requests_error.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_inflight gauge".to_string(),
            format!(
                "execution_engine_inflight {}",
                self.inflight.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_sequence_flushes_total counter".to_string(),
            format!(
                "execution_engine_sequence_flushes_total {}",
                self.sequence_flushes.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_submit_avg_ms gauge".to_string(),
            format!("execution_engine_submit_avg_ms {:.3}", avg),
            "# TYPE execution_engine_submit_parse_avg_ms gauge".to_string(),
            format!("execution_engine_submit_parse_avg_ms {:.3}", parse_avg),
            "# TYPE execution_engine_submit_prepare_avg_ms gauge".to_string(),
            format!("execution_engine_submit_prepare_avg_ms {:.3}", prepare_avg),
            "# TYPE execution_engine_submit_send_avg_ms gauge".to_string(),
            format!("execution_engine_submit_send_avg_ms {:.3}", send_avg),
            "# TYPE execution_engine_harness_submit_rejects_total counter".to_string(),
            format!(
                "execution_engine_harness_submit_rejects_total {}",
                self.harness_submit_rejects.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_attempts_total counter".to_string(),
            format!(
                "execution_engine_execute_attempts_total {}",
                self.execute_attempts.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_sent_total counter".to_string(),
            format!(
                "execution_engine_execute_sent_total {}",
                self.execute_sent.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_errors_total counter".to_string(),
            format!(
                "execution_engine_execute_errors_total {}",
                self.execute_errors.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_head_missing_total counter".to_string(),
            format!(
                "execution_engine_execute_head_missing_total {}",
                self.execute_head_missing.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_head_blocked_total counter".to_string(),
            format!(
                "execution_engine_execute_head_blocked_total {}",
                self.execute_head_blocked.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_no_lane_match_total counter".to_string(),
            format!(
                "execution_engine_execute_no_lane_match_total {}",
                self.execute_no_lane_match.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_confirmed_no_advance_total counter".to_string(),
            format!(
                "execution_engine_execute_confirmed_no_advance_total {}",
                self.execute_confirmed_no_advance.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_lane_suppressed_total counter".to_string(),
            format!(
                "execution_engine_execute_lane_suppressed_total {}",
                self.execute_lane_suppressed.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_targeted_total counter".to_string(),
            format!(
                "execution_engine_execute_targeted_total {}",
                self.execute_targeted.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_speculative_total counter".to_string(),
            format!(
                "execution_engine_execute_speculative_total {}",
                self.execute_speculative.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_pipeline_sent_total counter".to_string(),
            format!(
                "execution_engine_execute_pipeline_sent_total {}",
                self.execute_pipeline_sent.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_send_suppressed_pending_total counter".to_string(),
            format!(
                "execution_engine_execute_send_suppressed_pending_total {}",
                self.execute_send_suppressed_pending.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_head_advanced_total counter".to_string(),
            format!(
                "execution_engine_execute_head_advanced_total {}",
                self.execute_head_advanced.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_execute_head_advance_items_total counter".to_string(),
            format!(
                "execution_engine_execute_head_advance_items_total {}",
                self.execute_head_advance_items.load(Ordering::Relaxed)
            ),

            // ---------- General health gauges ----------
            "# HELP execution_engine_relayer_healthy 1 if this relayer process is serving /metrics".to_string(),
            "# TYPE execution_engine_relayer_healthy gauge".to_string(),
            format!(
                "execution_engine_relayer_healthy {}",
                self.relayer_healthy.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_bridge_healthy 1 if the last HTTP-to-gRPC bridge healthz probe succeeded".to_string(),
            "# TYPE execution_engine_bridge_healthy gauge".to_string(),
            format!(
                "execution_engine_bridge_healthy {}",
                self.bridge_healthy.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_executor_healthy 1 if executor heartbeat is fresh".to_string(),
            "# TYPE execution_engine_executor_healthy gauge".to_string(),
            format!(
                "execution_engine_executor_healthy {}",
                self.executor_healthy.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_event_cranker_healthy 1 if perp event consumer heartbeat is fresh".to_string(),
            "# TYPE execution_engine_event_cranker_healthy gauge".to_string(),
            format!(
                "execution_engine_event_cranker_healthy {}",
                self.event_cranker_healthy.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_relayer_balance_lamports Last polled relayer payer balance in lamports".to_string(),
            "# TYPE execution_engine_relayer_balance_lamports gauge".to_string(),
            format!(
                "execution_engine_relayer_balance_lamports {}",
                self.relayer_balance_lamports.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_relayer_balance_last_ms gauge".to_string(),
            format!(
                "execution_engine_relayer_balance_last_ms {}",
                self.relayer_balance_last_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_executor_last_tick_ms gauge".to_string(),
            format!(
                "execution_engine_executor_last_tick_ms {}",
                self.executor_last_tick_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_event_cranker_last_tick_ms gauge".to_string(),
            format!(
                "execution_engine_event_cranker_last_tick_ms {}",
                self.event_cranker_last_tick_ms.load(Ordering::Relaxed)
            ),

            // ---------- Performance / load ----------
            "# HELP execution_engine_ingress_total Total submit_intent invocations (incl. rejects)".to_string(),
            "# TYPE execution_engine_ingress_total counter".to_string(),
            format!(
                "execution_engine_ingress_total {}",
                self.ingress_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_ingress_accepted_total counter".to_string(),
            format!(
                "execution_engine_ingress_accepted_total {}",
                self.ingress_accepted_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_ingress_rejected_total counter".to_string(),
            format!(
                "execution_engine_ingress_rejected_total {}",
                self.ingress_rejected_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_executed_total counter".to_string(),
            format!(
                "execution_engine_executed_total {}",
                self.executed_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_executed_failed_total counter".to_string(),
            format!(
                "execution_engine_executed_failed_total {}",
                self.executed_failed_total.load(Ordering::Relaxed)
            ),

            // ---------- Background submitter pool (Phase 1.1) ----------
            "# HELP execution_engine_bg_submit_ok_total Successful background tx submits (after any retries)".to_string(),
            "# TYPE execution_engine_bg_submit_ok_total counter".to_string(),
            format!(
                "execution_engine_bg_submit_ok_total {}",
                self.bg_submit_ok_total.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_bg_submit_failed_total Hard failures from the bg submitter (rolled back the local sequence)".to_string(),
            "# TYPE execution_engine_bg_submit_failed_total counter".to_string(),
            format!(
                "execution_engine_bg_submit_failed_total {}",
                self.bg_submit_failed_total.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_bg_submit_transient_total Transient RPC failures observed by the bg submitter (each retry attempt counted)".to_string(),
            "# TYPE execution_engine_bg_submit_transient_total counter".to_string(),
            format!(
                "execution_engine_bg_submit_transient_total {}",
                self.bg_submit_transient_total.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_bg_submit_channel_full_total submit_intent calls rejected because the bg channel was full".to_string(),
            "# TYPE execution_engine_bg_submit_channel_full_total counter".to_string(),
            format!(
                "execution_engine_bg_submit_channel_full_total {}",
                self.bg_submit_channel_full_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_bg_submit_retry_dropped_total counter".to_string(),
            format!(
                "execution_engine_bg_submit_retry_dropped_total {}",
                self.bg_submit_retry_dropped_total.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_bg_submit_inflight Current count of in-flight bg submits (queue + workers)".to_string(),
            "# TYPE execution_engine_bg_submit_inflight gauge".to_string(),
            format!(
                "execution_engine_bg_submit_inflight {}",
                self.bg_submit_inflight.load(Ordering::Relaxed)
            ),

            // ---------- Phase 3.5 margin-check account cache ----------
            "# HELP execution_engine_margin_cache_hit_total Margin precheck account fetch served from local TTL cache".to_string(),
            "# TYPE execution_engine_margin_cache_hit_total counter".to_string(),
            format!(
                "execution_engine_margin_cache_hit_total {}",
                self.margin_cache_hit_total.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_margin_cache_miss_total Margin precheck account fetch fell through to RPC".to_string(),
            "# TYPE execution_engine_margin_cache_miss_total counter".to_string(),
            format!(
                "execution_engine_margin_cache_miss_total {}",
                self.margin_cache_miss_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_margin_cache_evicted_total counter".to_string(),
            format!(
                "execution_engine_margin_cache_evicted_total {}",
                self.margin_cache_evicted_total.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_margin_cache_size Current count of entries in the margin precheck account cache".to_string(),
            "# TYPE execution_engine_margin_cache_size gauge".to_string(),
            format!(
                "execution_engine_margin_cache_size {}",
                self.margin_cache_size.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_ingress_tps_10s Ingress txns per second sampled over last 10s".to_string(),
            "# TYPE execution_engine_ingress_tps_10s gauge".to_string(),
            format!(
                "execution_engine_ingress_tps_10s {:.3}",
                self.ingress_tps_10s_milli.load(Ordering::Relaxed) as f64 / 1000.0
            ),
            "# TYPE execution_engine_ingress_accepted_60s gauge".to_string(),
            format!(
                "execution_engine_ingress_accepted_60s {}",
                self.ingress_accepted_60s.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_ingress_rejected_60s gauge".to_string(),
            format!(
                "execution_engine_ingress_rejected_60s {}",
                self.ingress_rejected_60s.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_executed_tps_10s Executed (head-advance items) per second over last 10s".to_string(),
            "# TYPE execution_engine_executed_tps_10s gauge".to_string(),
            format!(
                "execution_engine_executed_tps_10s {:.3}",
                self.executed_tps_10s_milli.load(Ordering::Relaxed) as f64 / 1000.0
            ),
            "# TYPE execution_engine_executed_60s gauge".to_string(),
            format!(
                "execution_engine_executed_60s {}",
                self.executed_60s.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_executed_failed_60s gauge".to_string(),
            format!(
                "execution_engine_executed_failed_60s {}",
                self.executed_failed_60s.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_execution_queue_depth Current on-chain execution queue depth".to_string(),
            "# TYPE execution_engine_execution_queue_depth gauge".to_string(),
            format!(
                "execution_engine_execution_queue_depth {}",
                self.execution_queue_depth.load(Ordering::Relaxed)
            ),
            "# HELP execution_engine_unique_addresses_60s Approx distinct user_owner pubkeys observed in the last 60s".to_string(),
            "# TYPE execution_engine_unique_addresses_60s gauge".to_string(),
            format!(
                "execution_engine_unique_addresses_60s {}",
                self.unique_addresses_60s.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_sampler_last_tick_ms gauge".to_string(),
            format!(
                "execution_engine_sampler_last_tick_ms {}",
                self.sampler_last_tick_ms.load(Ordering::Relaxed)
            ),

            // ---------- Latency: ingress -> submitted (relayer-only) ----------
            "# HELP execution_engine_latency_ingress_to_submitted_p50_ms Median latency (ms) from order ingress to relayer submit_intent returning Ok".to_string(),
            "# TYPE execution_engine_latency_ingress_to_submitted_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_submitted_p50_ms {}",
                self.latency_ingress_to_submitted_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_submitted_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_submitted_p95_ms {}",
                self.latency_ingress_to_submitted_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_submitted_max_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_submitted_max_ms {}",
                self.latency_ingress_to_submitted_max_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_submitted_samples gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_submitted_samples {}",
                self.latency_ingress_to_submitted_samples.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_submit_total counter".to_string(),
            format!(
                "execution_engine_latency_submit_total {}",
                self.submit_latency.record_total()
            ),

            // ---------- Latency: per-stage breakdown of submit_intent (Phase 0) ----------
            "# HELP execution_engine_latency_stage_parse_p50_ms Median time spent in parse_submit_intent_keys + payload decode".to_string(),
            "# TYPE execution_engine_latency_stage_parse_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_parse_p50_ms {}",
                self.latency_stage_parse_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_parse_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_parse_p95_ms {}",
                self.latency_stage_parse_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_parse_total counter".to_string(),
            format!(
                "execution_engine_latency_stage_parse_total {}",
                self.stage_parse_latency.record_total()
            ),
            "# HELP execution_engine_latency_stage_margin_check_p50_ms Median time spent in ensure_submit_margin_ready (HTTP to harness today)".to_string(),
            "# TYPE execution_engine_latency_stage_margin_check_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_margin_check_p50_ms {}",
                self.latency_stage_margin_check_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_margin_check_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_margin_check_p95_ms {}",
                self.latency_stage_margin_check_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_margin_check_total counter".to_string(),
            format!(
                "execution_engine_latency_stage_margin_check_total {}",
                self.stage_margin_check_latency.record_total()
            ),
            "# HELP execution_engine_latency_stage_sign_p50_ms Median time spent building + signing the enqueue tx".to_string(),
            "# TYPE execution_engine_latency_stage_sign_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_sign_p50_ms {}",
                self.latency_stage_sign_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_sign_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_sign_p95_ms {}",
                self.latency_stage_sign_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_sign_total counter".to_string(),
            format!(
                "execution_engine_latency_stage_sign_total {}",
                self.stage_sign_latency.record_total()
            ),
            "# HELP execution_engine_latency_stage_event_dispatch_p50_ms Median time from signed tx to bg-submit channel hand-off".to_string(),
            "# TYPE execution_engine_latency_stage_event_dispatch_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_event_dispatch_p50_ms {}",
                self.latency_stage_event_dispatch_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_event_dispatch_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_event_dispatch_p95_ms {}",
                self.latency_stage_event_dispatch_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_event_dispatch_total counter".to_string(),
            format!(
                "execution_engine_latency_stage_event_dispatch_total {}",
                self.stage_event_dispatch_latency.record_total()
            ),
            "# HELP execution_engine_latency_stage_bg_rpc_submit_p50_ms Median time spent in rpc.send_transaction inside the bg submitter pool (off the user-visible critical path)".to_string(),
            "# TYPE execution_engine_latency_stage_bg_rpc_submit_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_bg_rpc_submit_p50_ms {}",
                self.latency_stage_bg_rpc_submit_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_bg_rpc_submit_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_stage_bg_rpc_submit_p95_ms {}",
                self.latency_stage_bg_rpc_submit_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_stage_bg_rpc_submit_total counter".to_string(),
            format!(
                "execution_engine_latency_stage_bg_rpc_submit_total {}",
                self.stage_bg_rpc_submit_latency.record_total()
            ),

            // ---------- Latency: ingress -> optimistic (harness internal state) ----------
            "# HELP execution_engine_latency_ingress_to_optimistic_p50_ms Median latency (ms) from order ingress to harness optimistic_seq watermark catching up".to_string(),
            "# TYPE execution_engine_latency_ingress_to_optimistic_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_optimistic_p50_ms {}",
                self.latency_ingress_to_optimistic_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_optimistic_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_optimistic_p95_ms {}",
                self.latency_ingress_to_optimistic_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_optimistic_max_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_optimistic_max_ms {}",
                self.latency_ingress_to_optimistic_max_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_optimistic_samples gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_optimistic_samples {}",
                self.latency_ingress_to_optimistic_samples.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_optimistic_completed_total counter".to_string(),
            format!(
                "execution_engine_latency_optimistic_completed_total {}",
                self.latency_optimistic_completed_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_optimistic_expired_total counter".to_string(),
            format!(
                "execution_engine_latency_optimistic_expired_total {}",
                self.latency_optimistic_expired_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_optimistic_pending_inflight gauge".to_string(),
            format!(
                "execution_engine_latency_optimistic_pending_inflight {}",
                self.latency_optimistic_pending_inflight.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_harness_optimistic_watermark_seq gauge".to_string(),
            format!(
                "execution_engine_harness_optimistic_watermark_seq {}",
                self.harness_optimistic_watermark_seq.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_optimistic_prober_last_ms gauge".to_string(),
            format!(
                "execution_engine_latency_optimistic_prober_last_ms {}",
                self.latency_optimistic_prober_last_ms.load(Ordering::Relaxed)
            ),

            // ---------- Latency: ingress -> processed (harness on-chain reconciliation) ----------
            "# HELP execution_engine_latency_ingress_to_processed_p50_ms Median latency (ms) from order ingress to harness last_processed_sequence catching up".to_string(),
            "# TYPE execution_engine_latency_ingress_to_processed_p50_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_processed_p50_ms {}",
                self.latency_ingress_to_processed_p50_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_processed_p95_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_processed_p95_ms {}",
                self.latency_ingress_to_processed_p95_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_processed_max_ms gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_processed_max_ms {}",
                self.latency_ingress_to_processed_max_ms.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_ingress_to_processed_samples gauge".to_string(),
            format!(
                "execution_engine_latency_ingress_to_processed_samples {}",
                self.latency_ingress_to_processed_samples.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_processed_completed_total counter".to_string(),
            format!(
                "execution_engine_latency_processed_completed_total {}",
                self.latency_processed_completed_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_processed_expired_total counter".to_string(),
            format!(
                "execution_engine_latency_processed_expired_total {}",
                self.latency_processed_expired_total.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_processed_pending_inflight gauge".to_string(),
            format!(
                "execution_engine_latency_processed_pending_inflight {}",
                self.latency_processed_pending_inflight.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_harness_processed_watermark_seq gauge".to_string(),
            format!(
                "execution_engine_harness_processed_watermark_seq {}",
                self.harness_processed_watermark_seq.load(Ordering::Relaxed)
            ),
            "# TYPE execution_engine_latency_processed_prober_last_ms gauge".to_string(),
            format!(
                "execution_engine_latency_processed_prober_last_ms {}",
                self.latency_processed_prober_last_ms.load(Ordering::Relaxed)
            ),
        ]
        .join("\n")
    }
}

/// Snapshot of percentile statistics for the latency-sample window. Returned
/// by LatencyTracker::compute_summary and consumed once per second by the
/// metrics sampler — never touched by the hot path.
#[derive(Default, Clone, Copy)]
struct LatencySummary {
    count: u64,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
}

/// One unit of work for the background submitter pool. Carries the
/// already-signed VersionedTransaction plus the metadata the worker needs
/// to roll back the local sequence on hard failure.
#[derive(Debug, Clone)]
struct PendingSubmit {
    sequence_key: String,
    sequence: u64,
    execution_queue: Pubkey,
    tx: VersionedTransaction,
    /// Pre-computed locally from `tx.signatures[0]` so the user-visible
    /// gRPC response doesn't have to wait for `rpc.send_transaction` to
    /// echo it back.
    tx_signature: Signature,
    /// Number of retry attempts so far. 0 = first attempt.
    attempts: u32,
    /// Wall-clock ms when this was first enqueued. Useful for diagnostics
    /// and to compute total time-in-flight including retries.
    enqueued_at_ms: u64,
    /// Phase 3: token from `apply_relay_intent_local`. On hard ingress
    /// failure the bg submitter calls `state.rollback_local(undo)` to
    /// reverse the in-process optimistic apply. None when local state is
    /// disabled or when the apply itself failed (in which case there's
    /// nothing to undo).
    undo_token: Option<UndoToken>,
}

/// Conservative classification of solana_client errors into "transient"
/// (worth retrying — network blip, server too busy, blockhash race) and
/// "hard" (worth rolling back — signature verify, account not found,
/// program reject). Anything not explicitly transient is treated as hard
/// to avoid masking real bugs.
fn is_transient_rpc_error(err: &solana_client::client_error::ClientError) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("timeout") || msg.contains("timed out") {
        return true;
    }
    if msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("connection closed")
        || msg.contains("connection aborted")
        || msg.contains("broken pipe")
    {
        return true;
    }
    if msg.contains("blockhashnotfound") || msg.contains("blockhash not found") {
        return true;
    }
    if msg.contains("too many requests") || msg.contains("429") {
        return true;
    }
    if msg.contains("server is busy")
        || msg.contains("server too busy")
        || msg.contains("node is unhealthy")
    {
        return true;
    }
    if msg.contains("502 ") || msg.contains("503 ") || msg.contains("504 ") {
        return true;
    }
    false
}

/// Reusable lock-and-push latency histogram. Used both for the existing
/// submit-latency series and for the per-stage timings introduced in
/// Phase 0. Hot-path producers acquire the std mutex briefly to push one
/// u64 sample; the metrics sampler computes percentiles from a snapshot
/// once per second under another brief lock. The mutex is never held
/// across an `.await`.
#[derive(Default)]
struct LatencyHistogram {
    samples: StdMutex<VecDeque<u64>>,
    /// Maximum number of samples retained. Initialized once at startup
    /// from config; never mutated thereafter.
    capacity: AtomicU64,
    /// Lifetime count of samples recorded — exposed as a Prometheus counter
    /// so external monitors can compute deltas independently of the ring.
    record_total: AtomicU64,
}

impl LatencyHistogram {
    fn set_capacity(&self, cap: usize) {
        self.capacity.store(cap as u64, Ordering::Relaxed);
    }

    /// Hot-path: push one sample. Single std mutex acquisition + bounded
    /// VecDeque push. The capacity bound is enforced by popping the oldest
    /// entry when the ring overflows.
    #[inline]
    fn record(&self, ms: u64) {
        let cap = self.capacity.load(Ordering::Relaxed) as usize;
        if cap == 0 {
            return;
        }
        if let Ok(mut guard) = self.samples.lock() {
            guard.push_back(ms);
            while guard.len() > cap {
                guard.pop_front();
            }
        }
        self.record_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Sampler-only path (1 Hz): snapshot the ring, sort, compute stats.
    fn compute_summary(&self) -> LatencySummary {
        let snapshot: Vec<u64> = match self.samples.lock() {
            Ok(guard) => guard.iter().copied().collect(),
            Err(_) => return LatencySummary::default(),
        };
        if snapshot.is_empty() {
            return LatencySummary::default();
        }
        let mut sorted = snapshot;
        sorted.sort_unstable();
        let n = sorted.len();
        let p50 = sorted[n / 2];
        let p95_idx = (((n as f64) * 0.95).floor() as usize).min(n - 1);
        let p95 = sorted[p95_idx];
        let max = *sorted.last().unwrap();
        LatencySummary {
            count: n as u64,
            p50_ms: p50,
            p95_ms: p95,
            max_ms: max,
        }
    }

    fn record_total(&self) -> u64 {
        self.record_total.load(Ordering::Relaxed)
    }
}

/// Tracks "ingress -> orderbook-visible" latency for accepted submit_intent
/// calls. The hot path only ever does a single bounded BTreeMap insert under
/// a std::sync::Mutex (the lock is never held across an `.await`). The
/// background harness prober drains entries whose sequence is now <= the
/// orderbook watermark, computes per-entry latency, and pushes the values
/// into a ring of recent samples. The metrics sampler computes percentiles
/// from that ring once per second and publishes them as gauges.
struct LatencyTracker {
    /// sequence -> ingress wall-clock ms. BTreeMap so the prober can drain a
    /// prefix in O(k) when the watermark advances.
    pending: StdMutex<BTreeMap<u64, u64>>,
    /// Ring of completed latency samples (oldest at the front, newest at the
    /// back). Bounded by samples_capacity.
    samples: StdMutex<VecDeque<u64>>,
    /// Ring capacity, ie maximum number of recent latency samples retained.
    samples_capacity: usize,
    /// Maximum allowed size of the pending map. When exceeded, the smallest
    /// (oldest) sequence is force-evicted.
    pending_max_size: usize,
    /// Pending entries older than this age (ms) are dropped on the next
    /// prober pass without contributing a sample, to bound memory in case
    /// the watermark stalls.
    pending_max_age_ms: u64,
    completed_total: AtomicU64,
    expired_total: AtomicU64,
}

impl LatencyTracker {
    fn new(samples_capacity: usize, pending_max_size: usize, pending_max_age_ms: u64) -> Self {
        Self {
            pending: StdMutex::new(BTreeMap::new()),
            samples: StdMutex::new(VecDeque::with_capacity(samples_capacity)),
            samples_capacity,
            pending_max_size,
            pending_max_age_ms,
            completed_total: AtomicU64::new(0),
            expired_total: AtomicU64::new(0),
        }
    }

    /// Hot-path: insert a pending entry. Single std Mutex acquisition +
    /// BTreeMap insert. Lock never held across an `.await`. Failures are
    /// silently ignored — metrics must never affect intent flow.
    #[inline]
    fn note_ingress(&self, sequence: u64, ingress_ts_ms: u64) {
        if let Ok(mut guard) = self.pending.lock() {
            guard.insert(sequence, ingress_ts_ms);
            // Bound the map. The smallest sequence is the oldest entry under
            // the per-market monotonic-sequence assumption, so popping from
            // the front evicts the oldest in flight.
            while guard.len() > self.pending_max_size {
                let first_seq = match guard.iter().next() {
                    Some((&seq, _)) => seq,
                    None => break,
                };
                guard.remove(&first_seq);
                self.expired_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Background-task path: drain entries whose sequence is <= watermark,
    /// computing latency for each. Also evicts pending entries older than
    /// pending_max_age_ms regardless of watermark, to bound the map when
    /// reconciliation stalls.
    fn complete_up_to(&self, watermark: u64, now_ms: u64) -> usize {
        let drained: Vec<u64> = {
            let Ok(mut guard) = self.pending.lock() else {
                return 0;
            };
            // Drain matched-by-watermark prefix.
            let to_complete: Vec<u64> = guard
                .range(..=watermark)
                .map(|(&seq, _)| seq)
                .collect();
            let mut latencies = Vec::with_capacity(to_complete.len());
            for seq in to_complete {
                if let Some(ingress_ts) = guard.remove(&seq) {
                    latencies.push(now_ms.saturating_sub(ingress_ts));
                }
            }
            // Sweep stale entries that the watermark hasn't caught up to.
            let stale_cutoff = now_ms.saturating_sub(self.pending_max_age_ms);
            let stale: Vec<u64> = guard
                .iter()
                .filter(|(_, &ts)| ts < stale_cutoff)
                .map(|(&seq, _)| seq)
                .collect();
            for seq in stale {
                guard.remove(&seq);
                self.expired_total.fetch_add(1, Ordering::Relaxed);
            }
            latencies
        };

        let drained_count = drained.len();
        if drained_count > 0 {
            self.completed_total
                .fetch_add(drained_count as u64, Ordering::Relaxed);
            if let Ok(mut samples) = self.samples.lock() {
                for lat in drained {
                    samples.push_back(lat);
                    while samples.len() > self.samples_capacity {
                        samples.pop_front();
                    }
                }
            }
        }
        drained_count
    }

    /// Sampler-only path (1 Hz). Snapshots the samples ring under a brief
    /// lock, sorts the snapshot, and computes p50/p95/max.
    fn compute_summary(&self) -> LatencySummary {
        let snapshot: Vec<u64> = match self.samples.lock() {
            Ok(guard) => guard.iter().copied().collect(),
            Err(_) => return LatencySummary::default(),
        };
        if snapshot.is_empty() {
            return LatencySummary::default();
        }
        let mut sorted = snapshot;
        sorted.sort_unstable();
        let n = sorted.len();
        let p50 = sorted[n / 2];
        let p95_idx = (((n as f64) * 0.95).floor() as usize).min(n - 1);
        let p95 = sorted[p95_idx];
        let max = *sorted.last().unwrap();
        LatencySummary {
            count: n as u64,
            p50_ms: p50,
            p95_ms: p95,
            max_ms: max,
        }
    }

    fn pending_len(&self) -> usize {
        self.pending.lock().map(|g| g.len()).unwrap_or(0)
    }
}

fn avg_ms(total: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total as f64 / count as f64
    }
}

fn derive_harness_base_url(event_sink_url: Option<&str>) -> Option<String> {
    const HARNESS_INGEST_SUFFIX: &str = "/ingest/relay-intent";

    event_sink_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.strip_suffix(HARNESS_INGEST_SUFFIX))
        .map(|value| value.trim_end_matches('/').to_string())
}

fn validate_harness_health(
    health: &HarnessHealthResponse,
    now_ms: u64,
    min_freshness_budget_ms: u64,
) -> std::result::Result<(), String> {
    if !health.ok {
        return Err("healthz returned ok=false".to_string());
    }

    let freshness_budget_ms = min_freshness_budget_ms
        .max(
            health
                .reconcile_interval_ms
                .unwrap_or_default()
                .saturating_mul(3),
        )
        .max(1_000);
    let generated_ts_ms = health
        .generated_ts_ms
        .ok_or_else(|| "healthz missing generated_ts_ms".to_string())?;
    let generated_age_ms = now_ms.saturating_sub(generated_ts_ms);
    if generated_age_ms > freshness_budget_ms {
        return Err(format!(
            "generated_ts_ms stale by {}ms (budget={}ms)",
            generated_age_ms, freshness_budget_ms
        ));
    }

    if health.onchain_read_enabled {
        let last_reconcile_ts_ms = health.last_reconcile_ts_ms.ok_or_else(|| {
            "healthz missing last_reconcile_ts_ms while onchain_read_enabled=true".to_string()
        })?;
        let reconcile_age_ms = now_ms.saturating_sub(last_reconcile_ts_ms);
        if reconcile_age_ms > freshness_budget_ms {
            return Err(format!(
                "last_reconcile_ts_ms stale by {}ms (budget={}ms)",
                reconcile_age_ms, freshness_budget_ms
            ));
        }
    }

    Ok(())
}

fn harness_market_has_drift(drift: &HarnessMarketDrift) -> bool {
    drift.replay_open_orders != drift.onchain_open_orders
        || drift.replay_best_bid != drift.onchain_best_bid
        || drift.replay_best_ask != drift.onchain_best_ask
        || drift.bid_base_lots_abs_diff != "0"
        || drift.ask_base_lots_abs_diff != "0"
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaneAccountConfig {
    pubkey: String,
    is_writable: bool,
    #[serde(default)]
    is_signer: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaneConfigFile {
    name: Option<String>,
    remaining_accounts: Vec<LaneAccountConfig>,
}

#[derive(Clone)]
struct Lane {
    name: String,
    remaining_accounts: Vec<AccountMeta>,
    hash: [u8; 32],
}

fn lane_account_width(lane: &Lane) -> usize {
    lane.remaining_accounts.len()
}

fn shared_lane_account_width(lanes: &[Lane]) -> Result<usize> {
    let lane_widths: Vec<usize> = lanes.iter().map(lane_account_width).collect();
    let Some(accounts_per_lane) = lane_widths.first().copied() else {
        return Err(anyhow!("cannot build execute_multi with zero lanes"));
    };
    if lane_widths.iter().any(|width| *width != accounts_per_lane) {
        return Err(anyhow!(
            "incompatible execute_multi lane account widths: {:?}",
            lane_widths
        ));
    }
    Ok(accounts_per_lane)
}

fn select_layout_compatible_lanes(
    candidate_lanes: Vec<Lane>,
    backoff_snapshot: &HashMap<String, u64>,
    now_ms: u64,
    target_lane_fanout: usize,
) -> (Vec<Lane>, usize, usize) {
    let mut eligible_lanes: Vec<Lane> = Vec::new();
    let mut seen_hashes = HashSet::new();
    let mut required_width: Option<usize> = None;
    let mut dropped_for_layout = 0usize;

    for lane in candidate_lanes {
        let lane_key = bytes_to_hex(&lane.hash);
        let blocked_until = backoff_snapshot.get(&lane_key).copied().unwrap_or(0);
        if blocked_until > now_ms {
            continue;
        }
        if !seen_hashes.insert(lane.hash) {
            continue;
        }
        if let Some(width) = required_width {
            if lane_account_width(&lane) != width {
                dropped_for_layout += 1;
                continue;
            }
        } else {
            required_width = Some(lane_account_width(&lane));
        }
        eligible_lanes.push(lane);
        if eligible_lanes.len() >= target_lane_fanout {
            break;
        }
    }

    (eligible_lanes, seen_hashes.len(), dropped_for_layout)
}

#[derive(Clone, Debug, Default)]
struct HarnessReadiness {
    drifted_markets: HashSet<String>,
}

#[derive(Clone, Debug)]
struct CachedHarnessReadiness {
    checked_at_ms: u64,
    result: std::result::Result<HarnessReadiness, String>,
}

#[derive(Clone, Debug, Deserialize)]
struct HarnessHealthResponse {
    ok: bool,
    #[serde(default)]
    onchain_read_enabled: bool,
    generated_ts_ms: Option<u64>,
    last_reconcile_ts_ms: Option<u64>,
    reconcile_interval_ms: Option<u64>,
    #[serde(default)]
    reconcile_markets_with_drift: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessReconciliationResponse {
    data: Option<HarnessReconciliationSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessReconciliationSnapshot {
    #[serde(default)]
    markets: HashMap<String, HarnessMarketDrift>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessMarketDrift {
    replay_open_orders: u64,
    onchain_open_orders: u64,
    replay_best_bid: Option<String>,
    onchain_best_bid: Option<String>,
    replay_best_ask: Option<String>,
    onchain_best_ask: Option<String>,
    bid_base_lots_abs_diff: String,
    ask_base_lots_abs_diff: String,
}

struct ExecutorState {
    group: Pubkey,
    execution_queue: Pubkey,
    current_queue_count: AtomicU64,
    current_queue_next_sequence: AtomicU64,
    current_queue_max_seen_sequence: AtomicU64,
    current_queue_head_available: AtomicU64,
    lanes: Arc<RwLock<HashMap<String, Lane>>>,
    failure_counts: Arc<Mutex<HashMap<String, u64>>>,
    backoff_until_ms: Arc<Mutex<HashMap<String, u64>>>,
    pending_dispatches: Arc<Mutex<Vec<PendingHeadDispatch>>>,
    last_dynamic_refresh_ms: Arc<Mutex<u64>>,
    last_inspect_ms: AtomicU64,
    cached_head: Arc<Mutex<Option<QueueHead>>>,
    last_progress_log_sequence: AtomicU64,
    /// Per-sequence consecutive failure counter. When a sequence exceeds
    /// EXECUTOR_MAX_SEQUENCE_FAILURES consecutive execute-tx failures (including
    /// status-poll failures), the executor proactively fires admin-drop instead
    /// of retrying forever. This prevents expired health-gated orders from
    /// permanently stalling the queue when the normal auto-drop path fails
    /// (e.g., due to RPC rate-limiting preventing signature status checks).
    sequence_failure_counts: Arc<Mutex<HashMap<u64, u32>>>,
    /// Tracks the first slot at which the current head had no lane match.
    /// Tuple of (sequence, first_seen_slot). Reset when the head changes.
    no_lane_match_since: Arc<Mutex<Option<(u64, u64)>>>,
    /// Adaptive max_items: starts at config.executor_max_items, reduced to 1
    /// when ProgramFailedToComplete (heap overflow) is detected, then ramped
    /// back up after consecutive successful head advancements.
    adaptive_max_items: AtomicU16,
    /// Counts consecutive successful head advancements while adaptive_max_items
    /// is below config.executor_max_items. After enough successes, max_items
    /// is restored to its configured value.
    adaptive_success_streak: AtomicU32,
}

impl ExecutorState {
    fn new(group: Pubkey, execution_queue: Pubkey, lanes: HashMap<String, Lane>) -> Self {
        Self {
            group,
            execution_queue,
            current_queue_count: AtomicU64::new(0),
            current_queue_next_sequence: AtomicU64::new(0),
            current_queue_max_seen_sequence: AtomicU64::new(0),
            current_queue_head_available: AtomicU64::new(1),
            lanes: Arc::new(RwLock::new(lanes)),
            failure_counts: Arc::new(Mutex::new(HashMap::new())),
            backoff_until_ms: Arc::new(Mutex::new(HashMap::new())),
            pending_dispatches: Arc::new(Mutex::new(Vec::new())),
            last_dynamic_refresh_ms: Arc::new(Mutex::new(0)),
            last_inspect_ms: AtomicU64::new(0),
            cached_head: Arc::new(Mutex::new(None)),
            last_progress_log_sequence: AtomicU64::new(0),
            sequence_failure_counts: Arc::new(Mutex::new(HashMap::new())),
            no_lane_match_since: Arc::new(Mutex::new(None)),
            adaptive_max_items: AtomicU16::new(0), // 0 = use config default
            adaptive_success_streak: AtomicU32::new(0),
        }
    }

    /// Returns the effective max_items for execute instructions. When the
    /// adaptive value is non-zero and lower than the config default, it takes
    /// precedence. This allows the executor to temporarily reduce batch size
    /// after heap overflow failures, then ramp back up.
    fn effective_max_items(&self, config_max: u16) -> u16 {
        let adaptive = self.adaptive_max_items.load(Ordering::Relaxed);
        if adaptive > 0 && adaptive < config_max {
            adaptive
        } else {
            config_max
        }
    }

    /// Called when ProgramFailedToComplete is detected. Immediately reduces
    /// max_items to 1 to avoid heap overflow on next attempt.
    fn on_heap_overflow(&self, config_max: u16) {
        let prev = self.adaptive_max_items.load(Ordering::Relaxed);
        self.adaptive_max_items.store(1, Ordering::Relaxed);
        self.adaptive_success_streak.store(0, Ordering::Relaxed);
        if prev != 1 {
            warn!(
                "adaptive max_items reduced {} -> 1 due to ProgramFailedToComplete (heap overflow)",
                if prev == 0 { config_max } else { prev }
            );
        }
    }

    /// Called after each successful head advancement. If adaptive_max_items is
    /// reduced, counts successes and ramps back up after 20 consecutive
    /// successful advancements.
    fn on_successful_advance(&self, config_max: u16) {
        let adaptive = self.adaptive_max_items.load(Ordering::Relaxed);
        if adaptive == 0 || adaptive >= config_max {
            return; // not in degraded mode
        }
        let streak = self.adaptive_success_streak.fetch_add(1, Ordering::Relaxed) + 1;
        // Ramp up schedule: after 20 successes at current level, double max_items
        // (capped at config max). This lets us recover TPS gradually.
        const RAMP_UP_THRESHOLD: u32 = 20;
        if streak >= RAMP_UP_THRESHOLD {
            let new_max = (adaptive * 2).min(config_max);
            self.adaptive_max_items.store(new_max, Ordering::Relaxed);
            self.adaptive_success_streak.store(0, Ordering::Relaxed);
            info!(
                "adaptive max_items ramped up {} -> {} (config_max={}, after {} successes)",
                adaptive, new_max, config_max, streak
            );
        }
    }

    async fn register_dynamic_lane(
        &self,
        lane_name: String,
        remaining_accounts: &[AccountMeta],
        include_legacy: bool,
    ) -> bool {
        let mut guard = self.lanes.write().await;
        let mut any_new = false;
        for lane in expand_lane_variants(
            lane_name,
            remaining_accounts,
            self.group,
            self.execution_queue,
            include_legacy,
        ) {
            let lane_hash = bytes_to_hex(&lane.hash);
            let lane_name = lane.name.clone();
            if !guard.contains_key(&lane_hash) {
                any_new = true;
            }
            guard.insert(lane_hash.clone(), lane);
            debug!(
                "executor registered lane name={} hash={} total_lanes={}",
                lane_name,
                lane_hash,
                guard.len(),
            );
        }
        any_new
    }

    async fn lanes_snapshot(&self) -> Vec<Lane> {
        self.lanes.read().await.values().cloned().collect()
    }

    async fn should_refresh_dynamic_lanes(&self, refresh_ms: u64) -> bool {
        let now = unix_timestamp_ms();
        let mut guard = self.last_dynamic_refresh_ms.lock().await;
        if now.saturating_sub(*guard) < refresh_ms {
            return false;
        }
        *guard = now;
        true
    }

    fn update_queue_count(&self, count: u32) {
        self.current_queue_count
            .store(count as u64, Ordering::Relaxed);
    }

    fn update_queue_next_sequence(&self, next_sequence: u64) {
        self.current_queue_next_sequence
            .store(next_sequence, Ordering::Relaxed);
    }

    fn update_queue_max_seen_sequence(&self, max_seen_sequence: u64) {
        self.current_queue_max_seen_sequence
            .store(max_seen_sequence, Ordering::Relaxed);
    }

    fn update_queue_head_available(&self, available: bool) {
        self.current_queue_head_available
            .store(available as u64, Ordering::Relaxed);
    }

    fn queue_count(&self) -> u32 {
        self.current_queue_count.load(Ordering::Relaxed) as u32
    }

    fn queue_next_sequence(&self) -> u64 {
        self.current_queue_next_sequence.load(Ordering::Relaxed)
    }

    fn queue_gap_span(&self) -> u64 {
        self.current_queue_max_seen_sequence
            .load(Ordering::Relaxed)
            .saturating_sub(self.current_queue_next_sequence.load(Ordering::Relaxed))
    }

    fn queue_head_available(&self) -> bool {
        self.current_queue_head_available.load(Ordering::Relaxed) != 0
    }
}

#[derive(Deserialize)]
struct RelayEventAccountMeta {
    pubkey: String,
    is_signer: bool,
    is_writable: bool,
}

#[derive(Deserialize)]
struct RelayIntentAcceptedEvent {
    event_type: String,
    #[serde(default)]
    request_id: Option<String>,
    group: String,
    execution_queue: String,
    market: String,
    sequence: String,
    user_owner: String,
    remaining_accounts: Vec<RelayEventAccountMeta>,
}

#[derive(Clone)]
struct RelayIntentStatusContext {
    request_id: String,
    group: Option<String>,
    execution_queue: Option<String>,
    market: Option<String>,
    sequence: Option<String>,
    kind: Option<u32>,
    user_owner: Option<String>,
    mango_account: Option<String>,
    tx_signature: Option<String>,
}

#[derive(Serialize)]
struct RelayIntentStatusEvent {
    event_type: &'static str,
    ts_ms: u64,
    request_id: String,
    status_code: u8,
    status_label: String,
    reason: Option<String>,
    group: Option<String>,
    execution_queue: Option<String>,
    market: Option<String>,
    sequence: Option<String>,
    kind: Option<u32>,
    user_owner: Option<String>,
    mango_account: Option<String>,
    tx_signature: Option<String>,
    grpc_code: Option<i32>,
    queue_process_status: Option<u32>,
    queue_process_status_name: Option<String>,
}

impl RelayIntentStatusContext {
    fn from_request(request: &SubmitIntentRequest) -> Self {
        let next_id = NEXT_RELAY_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            request_id: format!("relay-{}-{next_id}", unix_timestamp_ms()),
            group: (!request.group.is_empty()).then(|| request.group.clone()),
            execution_queue: (!request.execution_queue.is_empty())
                .then(|| request.execution_queue.clone()),
            market: (!request.market.is_empty()).then(|| request.market.clone()),
            sequence: None,
            kind: None,
            user_owner: (!request.user_owner.is_empty()).then(|| request.user_owner.clone()),
            mango_account: (!request.mango_account.is_empty())
                .then(|| request.mango_account.clone()),
            tx_signature: None,
        }
    }

    fn with_sequence(&mut self, sequence: u64) {
        self.sequence = Some(sequence.to_string());
    }

    fn with_kind(&mut self, kind: u32) {
        self.kind = Some(kind);
    }

    fn with_signature(&mut self, signature: &Signature) {
        self.tx_signature = Some(signature.to_string());
    }

    fn event(
        &self,
        status_code: u8,
        status_label: impl Into<String>,
        reason: Option<String>,
        grpc_code: Option<i32>,
        queue_process_status: Option<u32>,
        queue_process_status_name: Option<String>,
    ) -> RelayIntentStatusEvent {
        RelayIntentStatusEvent {
            event_type: "relay_intent_status",
            ts_ms: unix_timestamp_ms(),
            request_id: self.request_id.clone(),
            status_code,
            status_label: status_label.into(),
            reason,
            group: self.group.clone(),
            execution_queue: self.execution_queue.clone(),
            market: self.market.clone(),
            sequence: self.sequence.clone(),
            kind: self.kind,
            user_owner: self.user_owner.clone(),
            mango_account: self.mango_account.clone(),
            tx_signature: self.tx_signature.clone(),
            grpc_code,
            queue_process_status,
            queue_process_status_name,
        }
    }
}

#[derive(Clone, Copy)]
struct CachedChainState {
    blockhash: solana_sdk::hash::Hash,
    slot: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StartupFlags {
    enable_health_check: bool,
}

#[derive(Clone, Copy, Debug)]
struct ParsedSubmitIntentKeys {
    group: Pubkey,
    execution_queue: Pubkey,
    user_owner: Pubkey,
    mango_account: Pubkey,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessUserStateResponse {
    #[serde(default)]
    data: HarnessUserState,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessUserState {
    #[serde(default)]
    mango_accounts: Vec<String>,
    #[serde(default)]
    open_orders: Vec<HarnessOpenOrder>,
    #[serde(default)]
    per_market: Vec<HarnessUserPerMarket>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessOpenOrder {
    order_id: String,
    mango_account: String,
    market: String,
    side: String,
    base_lots: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessUserPerMarket {
    market: String,
    open_order_base_lots_bid: String,
    open_order_base_lots_ask: String,
    base_position_lots: String,
    quote_position_native: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessMarketStateResponse {
    metadata: Option<HarnessMarketMetadata>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HarnessMarketMetadata {
    market_index: u16,
    perp_market: String,
    oracle: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarginCheckOp {
    Place {
        side: Side,
        max_base_lots: i64,
        reduce_only: bool,
    },
    CancelByOrderId {
        expected_order_id: u128,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HarnessOrderExposure {
    market_index: PerpMarketIndex,
    side: Side,
    base_lots: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct UserPerpMarginOverlay {
    base_position_lots: i64,
    quote_position_native: I80F48,
    bids_base_lots: i64,
    asks_base_lots: i64,
}

#[derive(Clone, Debug, Default)]
struct HarnessMarginSnapshot {
    overlays: HashMap<PerpMarketIndex, UserPerpMarginOverlay>,
    orders_by_id: HashMap<u128, HarnessOrderExposure>,
}

struct BlockhashManager {
    inner: Arc<RwLock<CachedChainState>>,
}

impl BlockhashManager {
    async fn new(client: Arc<RpcClient>, refresh_ms: u64) -> Result<Self> {
        let initial_blockhash = client
            .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
            .await?
            .0;
        let initial_slot = client
            .get_slot_with_commitment(CommitmentConfig::processed())
            .await?;
        let inner = Arc::new(RwLock::new(CachedChainState {
            blockhash: initial_blockhash,
            slot: initial_slot,
        }));
        let inner_clone = inner.clone();
        tokio::spawn(async move {
            loop {
                match (
                    client
                        .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
                        .await,
                    client
                        .get_slot_with_commitment(CommitmentConfig::processed())
                        .await,
                ) {
                    (Ok((blockhash, _)), Ok(slot)) => {
                        let mut state = inner_clone.write().await;
                        state.blockhash = blockhash;
                        state.slot = slot;
                    }
                    (bh, sl) => {
                        warn!(
                            "blockhash refresh failed: blockhash_ok={} slot_ok={}",
                            bh.is_ok(),
                            sl.is_ok()
                        );
                    }
                }
                tokio::time::sleep(Duration::from_millis(refresh_ms)).await;
            }
        });
        Ok(Self { inner })
    }

    async fn snapshot(&self) -> CachedChainState {
        *self.inner.read().await
    }
}

struct SequenceStore {
    state_path: PathBuf,
    sequences: Arc<Mutex<HashMap<String, SequenceCursor>>>,
    flush_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    metrics: Arc<Metrics>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingSequencePhase {
    Reserved,
    Submitted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingSequenceState {
    phase: PendingSequencePhase,
    updated_at_ms: u64,
}

impl PendingSequenceState {
    fn reserved(now_ms: u64) -> Self {
        Self {
            phase: PendingSequencePhase::Reserved,
            updated_at_ms: now_ms,
        }
    }

    fn submitted(now_ms: u64) -> Self {
        Self {
            phase: PendingSequencePhase::Submitted,
            updated_at_ms: now_ms,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SequenceCursor {
    next_sequence: u64,
    pending: BTreeMap<u64, PendingSequenceState>,
    /// Sequences that failed/timed out and can be reused by the next reserve().
    recyclable: BTreeSet<u64>,
}

impl SequenceCursor {
    fn reserve(&mut self, now_ms: u64) -> u64 {
        // Prefer recycling the lowest available abandoned sequence to keep the queue dense.
        while let Some(&recycled) = self.recyclable.iter().next() {
            self.recyclable.remove(&recycled);
            // Only reuse if it's still within the valid enqueue window.
            if recycled >= self.next_sequence && !self.pending.contains_key(&recycled) {
                self.pending
                    .insert(recycled, PendingSequenceState::reserved(now_ms));
                return recycled;
            }
        }
        // Fall back to the next unused sequence.
        let mut candidate = self.next_sequence;
        for pending in self.pending.keys().copied() {
            if pending < candidate {
                continue;
            }
            if pending == candidate {
                candidate = candidate.saturating_add(1);
                continue;
            }
            break;
        }
        self.pending
            .insert(candidate, PendingSequenceState::reserved(now_ms));
        candidate
    }

    fn commit_success(&mut self, sequence: u64, now_ms: u64) {
        self.recyclable.remove(&sequence);
        self.pending
            .insert(sequence, PendingSequenceState::submitted(now_ms));
    }

    fn observe_queue_floor(&mut self, floor: u64) -> bool {
        let mut changed = false;
        if floor > self.next_sequence {
            self.next_sequence = floor;
            changed = true;
        }
        if !self.pending.is_empty() {
            let to_drop: Vec<u64> = self
                .pending
                .range(..floor)
                .map(|(sequence, _)| *sequence)
                .collect();
            if !to_drop.is_empty() {
                changed = true;
                for sequence in to_drop {
                    self.pending.remove(&sequence);
                }
            }
        }
        // Also drop recyclable sequences below the floor.
        self.recyclable.retain(|&seq| seq >= floor);
        changed
    }

    fn mark_submitted(&mut self, sequence: u64, now_ms: u64) -> bool {
        self.recyclable.remove(&sequence);
        let pending = self
            .pending
            .entry(sequence)
            .or_insert_with(|| PendingSequenceState::submitted(now_ms));
        let changed = pending.phase != PendingSequencePhase::Submitted;
        *pending = PendingSequenceState::submitted(now_ms);
        changed
    }

    fn reset_after_failure(&mut self, failed_sequence: u64, fallback_next: u64) -> bool {
        let removed = self.pending.remove(&failed_sequence).is_some();
        if removed && failed_sequence >= fallback_next {
            // Mark this sequence for reuse instead of creating a permanent gap.
            self.recyclable.insert(failed_sequence);
        }
        let observed = self.observe_queue_floor(fallback_next);
        removed || observed
    }

    fn submitted_depth_from(&self, floor: u64) -> usize {
        self.pending
            .range(floor..)
            .filter(|(_, state)| state.phase == PendingSequencePhase::Submitted)
            .count()
    }
}

impl SequenceStore {
    async fn new(state_path: PathBuf, metrics: Arc<Metrics>) -> Result<Self> {
        let sequences = if state_path.exists() {
            let raw = fs::read_to_string(&state_path).await.unwrap_or_default();
            serde_json::from_str::<HashMap<String, u64>>(&raw).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self {
            state_path,
            sequences: Arc::new(Mutex::new(
                sequences
                    .into_iter()
                    .map(|(key, next_sequence)| {
                        (
                            key,
                            SequenceCursor {
                                next_sequence,
                                pending: BTreeMap::new(),
                                recyclable: BTreeSet::new(),
                            },
                        )
                    })
                    .collect(),
            )),
            flush_task: Arc::new(Mutex::new(None)),
            metrics,
        })
    }

    async fn reserve(&self, key: &str) -> u64 {
        let now_ms = unix_timestamp_ms();
        {
            let mut guard = self.sequences.lock().await;
            guard.entry(key.to_string()).or_default().reserve(now_ms)
        }
    }

    async fn commit_success(&self, key: &str, sequence: u64) {
        let now_ms = unix_timestamp_ms();
        {
            let mut guard = self.sequences.lock().await;
            guard
                .entry(key.to_string())
                .or_default()
                .commit_success(sequence, now_ms);
        }
    }

    async fn observe_queue_floor(&self, key: &str, floor: u64) {
        let changed = {
            let mut guard = self.sequences.lock().await;
            guard
                .entry(key.to_string())
                .or_default()
                .observe_queue_floor(floor)
        };
        if changed {
            self.schedule_flush().await;
        }
    }

    async fn mark_submitted(&self, key: &str, sequence: u64) {
        let now_ms = unix_timestamp_ms();
        let changed = {
            let mut guard = self.sequences.lock().await;
            guard
                .entry(key.to_string())
                .or_default()
                .mark_submitted(sequence, now_ms)
        };
        if changed {
            self.schedule_flush().await;
        }
    }

    async fn reset_after_failure(&self, key: &str, failed_sequence: u64, fallback_next: u64) {
        let changed = {
            let mut guard = self.sequences.lock().await;
            guard
                .entry(key.to_string())
                .or_default()
                .reset_after_failure(failed_sequence, fallback_next)
        };
        if changed {
            self.schedule_flush().await;
        }
    }

    async fn submitted_depth_from(&self, key: &str, floor: u64) -> usize {
        let guard = self.sequences.lock().await;
        guard
            .get(key)
            .map(|cursor| cursor.submitted_depth_from(floor))
            .unwrap_or(0)
    }

    async fn schedule_flush(&self) {
        let mut task_guard = self.flush_task.lock().await;
        if task_guard.is_some() {
            return;
        }
        let sequences = self.sequences.clone();
        let path = self.state_path.clone();
        let metrics = self.metrics.clone();
        *task_guard = Some(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let snapshot: HashMap<String, u64> = {
                sequences
                    .lock()
                    .await
                    .iter()
                    .map(|(key, cursor)| (key.clone(), cursor.next_sequence))
                    .collect()
            };
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent).await;
            }
            match serde_json::to_vec_pretty(&snapshot) {
                Ok(serialized) => {
                    if fs::write(&path, serialized).await.is_ok() {
                        metrics.sequence_flushes.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(err) => warn!("failed to serialize sequence store: {err:?}"),
            }
        }));
        let flush_task = self.flush_task.clone();
        tokio::spawn(async move {
            if let Some(handle) = flush_task.lock().await.take() {
                let _ = handle.await;
            }
        });
    }
}

#[derive(Clone)]
struct Engine {
    config: Arc<Config>,
    rpc: Arc<RpcClient>,
    /// Optional secondary RPC for dual-send (fire-and-forget) to increase
    /// tx landing probability across multiple providers.
    secondary_rpc: Option<Arc<RpcClient>>,
    blockhashes: Arc<BlockhashManager>,
    sequences: Arc<SequenceStore>,
    metrics: Arc<Metrics>,
    inflight: Arc<Semaphore>,
    http_client: reqwest::Client,
    harness_readiness: Arc<Mutex<Option<CachedHarnessReadiness>>>,
    executor: Option<Arc<ExecutorState>>,
    execute_nonce: Arc<AtomicU64>,
    /// Mango accounts whose perp order slots are full. Keyed by account
    /// pubkey, value is the wall-clock ms when the block expires.  New
    /// intents targeting a blocked account are rejected immediately with
    /// RESOURCE_EXHAUSTED instead of being enqueued.
    blocked_mango_accounts: Arc<Mutex<HashMap<Pubkey, u64>>>,
    /// user_owner pubkey -> last-seen wall-clock ms. Used by the metrics
    /// sampler to compute unique_addresses_60s.
    ///
    /// Uses std::sync::Mutex (NOT tokio::sync::Mutex) because the only
    /// operation on the hot path is a single bounded HashMap insert that is
    /// dominated by hashing — holding the lock across an `.await` is never
    /// done. The bounded size keeps it cheap: the metrics sampler sweeps
    /// stale entries at a fixed cadence, so the map size is proportional to
    /// the number of distinct senders in the active window, not lifetime
    /// cardinality.
    unique_addresses: Arc<StdMutex<HashMap<[u8; 32], u64>>>,
    /// Tracks ingress->harness-optimistic-state latency by sequence. Hot path
    /// only inserts after a successful submit; the optimistic prober drains
    /// entries as the harness optimistic_seq watermark advances.
    latency_optimistic_tracker: Arc<LatencyTracker>,
    /// Tracks ingress->harness-on-chain-processed latency by sequence. Hot
    /// path only inserts after a successful submit; the processed prober
    /// drains entries as the harness last_processed_sequence advances.
    latency_processed_tracker: Arc<LatencyTracker>,
    /// Sender side of the bounded mpsc channel feeding the background
    /// submitter pool. The hot path only ever calls `try_send` on this so
    /// it never awaits backpressure — full channel becomes a clear
    /// resource_exhausted error to the caller.
    bg_submit_tx: mpsc::Sender<PendingSubmit>,
    /// In-process optimistic state (Phase 2). Linked from rust-harness as
    /// an rlib — no FFI, no JSON, no napi mutex. The hot path acquires the
    /// parking_lot Mutex for sub-microsecond critical sections.
    ///
    /// **Why Mutex (not RwLock):** rust_harness::engine::MarketState holds
    /// the per-market BookSide via `RefCell<BookSide>` which is `!Sync`.
    /// Mutex requires only `Send` from its contents (which RefCell
    /// satisfies); RwLock requires `Sync` (which RefCell does not). Phase
    /// 3's incremental refactor will replace the inner RefCell with a
    /// thread-safe wrapper so we can drop back to RwLock for concurrent
    /// reads.
    ///
    /// `None` when `CTM_RELAYER_LOCAL_STATE=false` — in that mode the
    /// relayer falls back to the legacy harness HTTP path. Default-on once
    /// soak-tested per the deployment plan.
    state: Option<Arc<PlMutex<ContinuumStateEngine>>>,
    /// Per-market metadata cache (Phase 2). Populated lazily on first
    /// `fetch_harness_market_metadata` miss; subsequent submits hit the
    /// cache for free. Static data — perp_market and oracle pubkeys don't
    /// change for the lifetime of a market.
    market_metadata_cache: Arc<StdMutex<HashMap<u16, HarnessMarketMetadata>>>,
    /// Phase 3.5: TTL cache for the margin-check account fetch. The hot
    /// path checks here first; cache hits skip the `get_multiple_accounts`
    /// RPC entirely. The cache stores the same authoritative on-chain
    /// bytes we'd fetch today, just for `margin_cache_ttl_ms` ≤ ~500 ms.
    /// Acceptable staleness because the margin check is a pre-flight
    /// optimization, not authoritative — false accepts are caught by the
    /// on-chain program and rolled back via Phase 3's `rollback_local`.
    margin_account_cache: Arc<StdMutex<HashMap<Pubkey, CachedMarginAccount>>>,
}

/// One entry in the margin-check account cache. Holds a clone of the
/// `KeyedAccountSharedData` returned by `get_multiple_accounts`, plus the
/// wall-clock ms it was cached at so the hot path can check expiry.
#[derive(Clone)]
struct CachedMarginAccount {
    account: KeyedAccountSharedData,
    cached_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingHeadDispatch {
    sequence: u64,
    accounts_hash: Option<[u8; 32]>,
    lane_hash: [u8; 32],
    dispatch_kind: PendingDispatchKind,
    sent_at_ms: u64,
    last_status_check_ms: u64,
    no_advance_recorded: bool,
    targeted: bool,
    signature: Signature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingDispatchKind {
    Execute,
    GapSkip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueueAdminState {
    pause_ingress: bool,
    pause_execute: bool,
    gap_wait_slots: u64,
    liquidity_delay_slots: u64,
    head: QueueHead,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueueExpiredHeadBatch {
    sequences: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecuteLoopOutcome {
    Idle,
    Busy,
    Sent,
}

#[derive(Debug)]
enum ExecuteDispatchError {
    Build(anyhow::Error),
    Send(anyhow::Error),
}

impl ExecuteDispatchError {
    fn err(&self) -> &anyhow::Error {
        match self {
            Self::Build(err) | Self::Send(err) => err,
        }
    }

    fn into_err(self) -> anyhow::Error {
        match self {
            Self::Build(err) | Self::Send(err) => err,
        }
    }

    fn is_build(&self) -> bool {
        matches!(self, Self::Build(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaneFailureClass {
    DeterministicSigner,
    DeterministicLayout,
    Transient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalHeadFailureReason {
    ExpiredOrder,
    WrongProgramOwner,
    PerpOrderSlotsFull,
    HealthCheckFailed,
}

impl LaneFailureClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicSigner => "deterministic_signer",
            Self::DeterministicLayout => "deterministic_layout",
            Self::Transient => "transient",
        }
    }

    fn is_deterministic(self) -> bool {
        matches!(self, Self::DeterministicSigner | Self::DeterministicLayout)
    }
}

fn classify_lane_failure(err: &anyhow::Error) -> LaneFailureClass {
    let msg = format!("{err:#}").to_ascii_lowercase();
    if msg.contains("not enough signers")
        || msg.contains("signature verification failure")
        || msg.contains("transaction signature verification failure")
    {
        LaneFailureClass::DeterministicSigner
    } else if msg.contains("sanitize")
        || msg.contains("account offset")
        || msg.contains("invalid account data")
        || msg.contains("invalid transaction")
    {
        LaneFailureClass::DeterministicLayout
    } else {
        LaneFailureClass::Transient
    }
}

fn is_transaction_too_large_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("transaction too large")
        || msg.contains("versionedtransaction too large")
        || msg.contains("base64 encoded")
        || msg.contains("max: encoded/raw")
        || msg.contains("packet too large")
}

impl Engine {
    /// Hot-path: stamps `user_owner.to_bytes()` with the current wall-clock
    /// time in the unique-address tracker. Single std Mutex acquisition +
    /// HashMap entry update; never held across an await. Failures (poisoned
    /// lock) are silently ignored — metrics must never affect intent flow.
    #[inline]
    fn note_user_owner(&self, user_owner: &Pubkey, now_ms: u64) {
        if let Ok(mut guard) = self.unique_addresses.lock() {
            guard.insert(user_owner.to_bytes(), now_ms);
        }
    }

    fn reject_submit_request(
        &self,
        request: &SubmitIntentRequest,
        code: Code,
        message: impl Into<String>,
    ) -> Status {
        let message = message.into();
        warn!(
            reason = %message,
            group = %request.group,
            execution_queue = %request.execution_queue,
            market = %request.market,
            user_owner = %request.user_owner,
            mango_account = %request.mango_account,
            remaining_accounts_count = request.remaining_accounts.len(),
            payload_len = request.payload.len(),
            "direct submit rejected"
        );
        Status::new(code, message)
    }

    fn parse_submit_intent_keys(
        &self,
        request: &SubmitIntentRequest,
    ) -> Result<ParsedSubmitIntentKeys, Status> {
        Ok(ParsedSubmitIntentKeys {
            group: parse_pubkey(&request.group)?,
            execution_queue: parse_pubkey(&request.execution_queue)?,
            user_owner: parse_pubkey(&request.user_owner)?,
            mango_account: parse_pubkey(&request.mango_account)?,
        })
    }

    fn harness_reject_status(&self, message: impl Into<String>, drift_related: bool) -> Status {
        self.metrics
            .harness_submit_rejects
            .fetch_add(1, Ordering::Relaxed);
        let message = message.into();
        if drift_related {
            Status::failed_precondition(message)
        } else {
            Status::unavailable(message)
        }
    }

    async fn ensure_harness_ready(&self, market: &str) -> Result<(), Status> {
        let Some(harness_base_url) = self.config.harness_base_url.as_deref() else {
            return Ok(());
        };

        let now_ms = unix_timestamp_ms();
        if let Some(cached) = self.harness_readiness.lock().await.clone() {
            if now_ms.saturating_sub(cached.checked_at_ms) <= self.config.harness_health_cache_ms {
                return self.evaluate_harness_readiness(market, cached.result);
            }
        }

        let refreshed = self
            .fetch_harness_readiness(harness_base_url)
            .await
            .map_err(|err| format!("{err:#}"));
        // Only cache successful results — errors should be retried immediately
        // instead of poisoning the cache for the entire cache_ms window.
        if refreshed.is_ok() {
            *self.harness_readiness.lock().await = Some(CachedHarnessReadiness {
                checked_at_ms: now_ms,
                result: refreshed.clone(),
            });
        }
        self.evaluate_harness_readiness(market, refreshed)
    }

    fn evaluate_harness_readiness(
        &self,
        market: &str,
        readiness: std::result::Result<HarnessReadiness, String>,
    ) -> Result<(), Status> {
        match readiness {
            Ok(readiness) => {
                if self.config.harness_reject_market_drift
                    && readiness.drifted_markets.contains(market)
                {
                    return Err(self.harness_reject_status(
                        format!(
                            "harness reconciliation drift active for market={market}; refusing enqueue until harness catches up"
                        ),
                        true,
                    ));
                }
                Ok(())
            }
            Err(err) => Err(self.harness_reject_status(err, false)),
        }
    }

    async fn fetch_harness_readiness(&self, harness_base_url: &str) -> Result<HarnessReadiness> {
        let timeout = Duration::from_millis(self.config.harness_health_timeout_ms);
        let health_url = format!("{}/healthz", harness_base_url.trim_end_matches('/'));
        let health_resp = self
            .http_client
            .get(&health_url)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("harness health request failed: {health_url}"))?;
        let health_status = health_resp.status();
        if !health_status.is_success() {
            return Err(anyhow!(
                "harness health request returned status {} from {}",
                health_status,
                health_url
            ));
        }
        let health: HarnessHealthResponse = health_resp.json().await.with_context(|| {
            format!("failed to decode harness health payload from {health_url}")
        })?;
        validate_harness_health(
            &health,
            unix_timestamp_ms(),
            self.config.harness_health_max_age_ms,
        )
        .map_err(|err| anyhow!("harness health rejected: {err}"))?;

        if !self.config.harness_reject_market_drift || health.reconcile_markets_with_drift == 0 {
            return Ok(HarnessReadiness::default());
        }

        let reconciliation_url = format!(
            "{}/diagnostics/reconciliation",
            harness_base_url.trim_end_matches('/')
        );
        let reconciliation_resp = self
            .http_client
            .get(&reconciliation_url)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| {
                format!("harness reconciliation request failed: {reconciliation_url}")
            })?;
        let reconciliation_status = reconciliation_resp.status();
        if !reconciliation_status.is_success() {
            return Err(anyhow!(
                "harness reconciliation request returned status {} from {}",
                reconciliation_status,
                reconciliation_url
            ));
        }
        let reconciliation: HarnessReconciliationResponse =
            reconciliation_resp.json().await.with_context(|| {
                format!(
                    "failed to decode harness reconciliation payload from {}",
                    reconciliation_url
                )
            })?;
        let drifted_markets: HashSet<String> = reconciliation
            .data
            .map(|snapshot| {
                snapshot
                    .markets
                    .into_iter()
                    .filter_map(|(market, drift)| {
                        harness_market_has_drift(&drift).then_some(market)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if health.reconcile_markets_with_drift > 0 && drifted_markets.is_empty() {
            return Err(anyhow!(
                "harness reported {} drifted markets but reconciliation details were empty",
                health.reconcile_markets_with_drift
            ));
        }

        Ok(HarnessReadiness { drifted_markets })
    }

    async fn ensure_submit_margin_ready(
        &self,
        request: &SubmitIntentRequest,
        keys: ParsedSubmitIntentKeys,
    ) -> Result<(), Status> {
        if !self.config.enable_health_check {
            return Ok(());
        }
        let Some(margin_ops) = decode_margin_check_ops(&request.payload).map_err(|err| {
            self.reject_submit_request(
                request,
                Code::InvalidArgument,
                format!("failed to decode queue payload for margin precheck: {err}"),
            )
        })?
        else {
            return Ok(());
        };
        let remaining_accounts = parse_remaining_accounts(&request.remaining_accounts)?;
        if remaining_accounts.len() < 4 {
            return Err(self.reject_submit_request(
                request,
                Code::InvalidArgument,
                "remaining_accounts missing canonical perp market account",
            ));
        }

        let harness_state = self.fetch_harness_user_state(request, keys).await?;
        let margin_snapshot =
            build_harness_margin_snapshot(harness_state.as_ref(), &request.mango_account).map_err(
                |err| {
                    self.reject_submit_request(
                        request,
                        Code::Unavailable,
                        format!("invalid harness user state for margin precheck: {err}"),
                    )
                },
            )?;
        let extra_market_metadata = self
            .fetch_harness_market_metadata(request, &margin_snapshot)
            .await?;
        let account_map = self
            .fetch_margin_check_account_map(
                request,
                keys.mango_account,
                &remaining_accounts,
                &extra_market_metadata,
            )
            .await?;
        self.evaluate_submit_margin_precheck(
            request,
            &remaining_accounts,
            &margin_ops,
            margin_snapshot,
            &extra_market_metadata,
            account_map,
        )
    }

    async fn fetch_harness_user_state(
        &self,
        request: &SubmitIntentRequest,
        keys: ParsedSubmitIntentKeys,
    ) -> Result<Option<HarnessUserState>, Status> {
        // Phase 2 fast path: in-process state. Single parking_lot Mutex
        // acquisition; no HTTP, no JSON, no FFI. The lock is held for the
        // duration of get_user_state which may trigger an internal lazy
        // rebuild — bounded by the active intent count.
        if let Some(state) = &self.state {
            let owner = keys.user_owner.to_string();
            let user_state = state
                .lock()
                .get_user_state(&owner, QueueView::Optimistic)
                .map_err(|err| {
                    self.reject_submit_request(
                        request,
                        Code::Internal,
                        format!("local state get_user_state failed owner={owner}: {err}"),
                    )
                })?;
            return Ok(Some(harness_user_state_from_rust_harness(user_state)));
        }

        let Some(harness_base_url) = self.config.harness_base_url.as_deref() else {
            return Ok(None);
        };
        let timeout = Duration::from_millis(self.config.harness_health_timeout_ms);
        let owner = keys.user_owner.to_string();
        let url = format!(
            "{}/state/users/{}?view=optimistic&onchain=false",
            harness_base_url.trim_end_matches('/'),
            owner
        );
        let response = self
            .http_client
            .get(&url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!("harness user-state request failed owner={owner}: {err}"),
                )
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.reject_submit_request(
                request,
                Code::Unavailable,
                format!(
                    "harness user-state request returned status {} owner={} url={}",
                    status, owner, url
                ),
            ));
        }
        let payload: HarnessUserStateResponse = response.json().await.map_err(|err| {
            self.reject_submit_request(
                request,
                Code::Unavailable,
                format!("failed to decode harness user-state payload owner={owner}: {err}"),
            )
        })?;
        Ok(Some(payload.data))
    }

    async fn fetch_harness_market_metadata(
        &self,
        request: &SubmitIntentRequest,
        margin_snapshot: &HarnessMarginSnapshot,
    ) -> Result<HashMap<PerpMarketIndex, HarnessMarketMetadata>, Status> {
        // Phase 2 cache: market metadata (perp_market + oracle pubkeys) is
        // static for the lifetime of a market. Check the in-process cache
        // first; only fall through to HTTP for misses. After warmup all
        // submits hit the cache and skip the harness entirely.
        let mut metadata = HashMap::new();
        let mut misses: Vec<PerpMarketIndex> = Vec::new();
        {
            if let Ok(cache) = self.market_metadata_cache.lock() {
                for market_index in margin_snapshot.overlays.keys().copied() {
                    if let Some(entry) = cache.get(&market_index) {
                        metadata.insert(market_index, entry.clone());
                    } else {
                        misses.push(market_index);
                    }
                }
            } else {
                // Poisoned mutex — fall through to HTTP for everything.
                misses.extend(margin_snapshot.overlays.keys().copied());
            }
        }
        if misses.is_empty() {
            return Ok(metadata);
        }

        let Some(harness_base_url) = self.config.harness_base_url.as_deref() else {
            return Ok(metadata);
        };
        let timeout = Duration::from_millis(self.config.harness_health_timeout_ms);
        for market_index in misses {
            let url = format!(
                "{}/state/markets/{}?view=optimistic&metadata_only=true",
                harness_base_url.trim_end_matches('/'),
                market_index
            );
            let response = self
                .http_client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|err| {
                    self.reject_submit_request(
                        request,
                        Code::Unavailable,
                        format!(
                            "harness market-state request failed market_index={market_index}: {err}"
                        ),
                    )
                })?;
            let status = response.status();
            if !status.is_success() {
                return Err(self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!(
                        "harness market-state request returned status {} market_index={} url={}",
                        status, market_index, url
                    ),
                ));
            }
            let payload: HarnessMarketStateResponse = response.json().await.map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!(
                        "failed to decode harness market-state payload market_index={market_index}: {err}"
                    ),
                )
            })?;
            let Some(market_metadata) = payload.metadata else {
                return Err(self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!("harness missing market metadata for market_index={market_index}"),
                ));
            };
            if market_metadata.market_index != market_index {
                return Err(self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!(
                        "harness market metadata mismatch requested_market_index={} returned_market_index={}",
                        market_index, market_metadata.market_index
                    ),
                ));
            }
            // Populate the cache for next time.
            if let Ok(mut cache) = self.market_metadata_cache.lock() {
                cache.insert(market_index, market_metadata.clone());
            }
            metadata.insert(market_index, market_metadata);
        }
        Ok(metadata)
    }

    async fn fetch_margin_check_account_map(
        &self,
        request: &SubmitIntentRequest,
        mango_account: Pubkey,
        remaining_accounts: &[AccountMeta],
        extra_market_metadata: &HashMap<PerpMarketIndex, HarnessMarketMetadata>,
    ) -> Result<HashMap<Pubkey, KeyedAccountSharedData>, Status> {
        // Build the deduped list of pubkeys we need.
        let mut pubkeys = Vec::with_capacity(
            1 + remaining_accounts.len() + extra_market_metadata.len().saturating_mul(2),
        );
        let mut seen = HashSet::new();
        let mut push_pubkey = |pubkey: Pubkey| {
            if seen.insert(pubkey) {
                pubkeys.push(pubkey);
            }
        };
        push_pubkey(mango_account);
        for account in remaining_accounts {
            push_pubkey(account.pubkey);
        }
        for metadata in extra_market_metadata.values() {
            push_pubkey(parse_pubkey(&metadata.perp_market).map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!("invalid harness perp market pubkey for margin precheck: {err}"),
                )
            })?);
            push_pubkey(parse_pubkey(&metadata.oracle).map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!("invalid harness oracle pubkey for margin precheck: {err}"),
                )
            })?);
        }

        // Phase 3.5: serve cache hits and collect misses for a single
        // get_multiple_accounts call. The cache lookup is one std Mutex
        // acquisition; never crosses an `.await`.
        let ttl_ms = self.config.margin_cache_ttl_ms;
        let now_ms = unix_timestamp_ms();
        let mut accounts: HashMap<Pubkey, KeyedAccountSharedData> =
            HashMap::with_capacity(pubkeys.len());
        let mut misses: Vec<Pubkey> = Vec::new();
        if ttl_ms > 0 {
            if let Ok(cache) = self.margin_account_cache.lock() {
                for &pubkey in &pubkeys {
                    match cache.get(&pubkey) {
                        Some(entry)
                            if now_ms.saturating_sub(entry.cached_at_ms) <= ttl_ms =>
                        {
                            accounts.insert(pubkey, entry.account.clone());
                        }
                        _ => misses.push(pubkey),
                    }
                }
            } else {
                misses.extend(pubkeys.iter().copied());
            }
        } else {
            // Cache disabled — every pubkey is a miss.
            misses.extend(pubkeys.iter().copied());
        }

        let hits = (pubkeys.len() - misses.len()) as u64;
        if hits > 0 {
            self.metrics
                .margin_cache_hit_total
                .fetch_add(hits, Ordering::Relaxed);
        }
        if misses.is_empty() {
            return Ok(accounts);
        }
        self.metrics
            .margin_cache_miss_total
            .fetch_add(misses.len() as u64, Ordering::Relaxed);

        // Single batched RPC for the misses.
        let fetched = self
            .rpc
            .get_multiple_accounts(&misses)
            .await
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::Unavailable,
                    format!("margin precheck account fetch failed: {err}"),
                )
            })?;

        // Insert into the cache while we have the data; enforce the soft
        // size cap by evicting the oldest entries when over.
        let max_entries = self.config.margin_cache_max_entries;
        let mut cache_guard = if ttl_ms > 0 {
            self.margin_account_cache.lock().ok()
        } else {
            None
        };
        for (pubkey, maybe_account) in misses.into_iter().zip(fetched.into_iter()) {
            let Some(account) = maybe_account else {
                return Err(self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!("margin precheck account not found: {pubkey}"),
                ));
            };
            let keyed = KeyedAccountSharedData::new(pubkey, account.into());
            if let Some(cache) = cache_guard.as_mut() {
                cache.insert(
                    pubkey,
                    CachedMarginAccount {
                        account: keyed.clone(),
                        cached_at_ms: now_ms,
                    },
                );
                // Enforce the soft cap. The eviction policy is "drop
                // expired entries first; if still over, drop the oldest
                // remaining one." Cheap because we only run it on the
                // miss path, not on every hit.
                if cache.len() > max_entries {
                    let stale_cutoff = now_ms.saturating_sub(ttl_ms);
                    let stale_keys: Vec<Pubkey> = cache
                        .iter()
                        .filter(|(_, entry)| entry.cached_at_ms < stale_cutoff)
                        .map(|(pk, _)| *pk)
                        .collect();
                    let stale_count = stale_keys.len() as u64;
                    for key in stale_keys {
                        cache.remove(&key);
                    }
                    if stale_count > 0 {
                        self.metrics
                            .margin_cache_evicted_total
                            .fetch_add(stale_count, Ordering::Relaxed);
                    }
                    // Still over? Drop the single oldest entry.
                    while cache.len() > max_entries {
                        if let Some(oldest) = cache
                            .iter()
                            .min_by_key(|(_, entry)| entry.cached_at_ms)
                            .map(|(pk, _)| *pk)
                        {
                            cache.remove(&oldest);
                            self.metrics
                                .margin_cache_evicted_total
                                .fetch_add(1, Ordering::Relaxed);
                        } else {
                            break;
                        }
                    }
                }
                self.metrics
                    .margin_cache_size
                    .store(cache.len() as u64, Ordering::Relaxed);
            }
            accounts.insert(pubkey, keyed);
        }
        Ok(accounts)
    }

    fn evaluate_submit_margin_precheck(
        &self,
        request: &SubmitIntentRequest,
        remaining_accounts: &[AccountMeta],
        margin_ops: &[MarginCheckOp],
        margin_snapshot: HarnessMarginSnapshot,
        extra_market_metadata: &HashMap<PerpMarketIndex, HarnessMarketMetadata>,
        account_map: HashMap<Pubkey, KeyedAccountSharedData>,
    ) -> Result<(), Status> {
        let Some(target_market_meta) = remaining_accounts.get(3) else {
            return Err(self.reject_submit_request(
                request,
                Code::InvalidArgument,
                "remaining_accounts missing canonical perp market account",
            ));
        };
        let target_market_account =
            account_map.get(&target_market_meta.pubkey).ok_or_else(|| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!(
                        "target perp market account missing for margin precheck: {}",
                        target_market_meta.pubkey
                    ),
                )
            })?;
        let target_market = target_market_account.load::<PerpMarket>().map_err(|err| {
            self.reject_submit_request(
                request,
                Code::FailedPrecondition,
                format!(
                    "failed to load target perp market {} for margin precheck: {err}",
                    target_market_meta.pubkey
                ),
            )
        })?;
        let target_market_index = target_market.perp_market_index;
        let target_settle_token_index = target_market.settle_token_index;

        let mango_account_key = parse_pubkey(&request.mango_account)?;
        let mango_account = account_map.get(&mango_account_key).ok_or_else(|| {
            self.reject_submit_request(
                request,
                Code::FailedPrecondition,
                format!(
                    "mango account missing for margin precheck: {}",
                    mango_account_key
                ),
            )
        })?;
        if mango_account.data.data().len() < 8 {
            return Err(self.reject_submit_request(
                request,
                Code::FailedPrecondition,
                format!(
                    "mango account data too short for margin precheck mango_account={} data_len={}",
                    mango_account_key,
                    mango_account.data.data().len()
                ),
            ));
        }
        let mut optimistic_account = MangoAccountValue::from_bytes(&mango_account.data.data()[8..])
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!(
                        "failed to deserialize mango account for margin precheck {}: {err}",
                        mango_account_key
                    ),
                )
            })?;

        for (market_index, overlay) in &margin_snapshot.overlays {
            let (market_pubkey, market_account) = if *market_index == target_market_index {
                (target_market_meta.pubkey, target_market_account)
            } else {
                let Some(metadata) = extra_market_metadata.get(market_index) else {
                    return Err(self.reject_submit_request(
                        request,
                        Code::Unavailable,
                        format!(
                            "missing harness metadata for optimistic market {} during margin precheck",
                            market_index
                        ),
                    ));
                };
                let market_pubkey = parse_pubkey(&metadata.perp_market).map_err(|err| {
                    self.reject_submit_request(
                        request,
                        Code::Unavailable,
                        format!(
                            "invalid harness perp market pubkey for market {} during margin precheck: {err}",
                            market_index
                        ),
                    )
                })?;
                let market_account = account_map.get(&market_pubkey).ok_or_else(|| {
                    self.reject_submit_request(
                        request,
                        Code::FailedPrecondition,
                        format!(
                            "optimistic perp market account missing for margin precheck market={} pubkey={}",
                            market_index, market_pubkey
                        ),
                    )
                })?;
                (market_pubkey, market_account)
            };
            let market = market_account.load::<PerpMarket>().map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!(
                        "failed to load optimistic perp market {} for margin precheck market={}: {err}",
                        market_pubkey, market_index
                    ),
                )
            })?;
            let (position, _) = optimistic_account
                .ensure_perp_position(*market_index, market.settle_token_index)
                .map_err(|err| {
                    self.reject_submit_request(
                        request,
                        Code::FailedPrecondition,
                        format!(
                            "failed to ensure optimistic perp position market={} during margin precheck: {err}",
                            market_index
                        ),
                    )
                })?;
            position.base_position_lots = overlay.base_position_lots;
            position.quote_position_native = overlay.quote_position_native;
            position.bids_base_lots = overlay.bids_base_lots;
            position.asks_base_lots = overlay.asks_base_lots;
        }

        optimistic_account
            .ensure_perp_position(target_market_index, target_settle_token_index)
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!(
                        "failed to ensure target perp position for margin precheck market={}: {err}",
                        target_market_index
                    ),
                )
            })?;

        let health_accounts = build_margin_health_accounts(&optimistic_account, &account_map)
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!("failed to assemble health accounts for margin precheck: {err}"),
                )
            })?;
        let active_token_len = optimistic_account.active_token_positions().count();
        let active_perp_len = optimistic_account.active_perp_positions().count();
        let active_serum3_len = optimistic_account.active_serum3_orders().count();
        let begin_fallback_oracles = active_token_len * 2
            + active_perp_len * 2
            + active_serum3_len
            + optimistic_account.active_openbook_v2_orders().count();
        let retriever = FixedOrderAccountRetriever {
            ais: health_accounts,
            n_banks: active_token_len,
            n_perps: active_perp_len,
            begin_perp: active_token_len * 2,
            begin_serum3: active_token_len * 2 + active_perp_len * 2,
            begin_openbook_v2: active_token_len * 2 + active_perp_len * 2 + active_serum3_len,
            staleness_slot: None,
            begin_fallback_oracles,
            usdc_oracle_index: None,
            sol_oracle_index: None,
        };
        let usdc_oracle_index = retriever
            .ais
            .iter()
            .position(|account| account.key == pyth_mainnet_usdc_oracle::ID);
        let sol_oracle_index = retriever
            .ais
            .iter()
            .position(|account| account.key == pyth_mainnet_sol_oracle::ID);
        let retriever = FixedOrderAccountRetriever {
            usdc_oracle_index,
            sol_oracle_index,
            ..retriever
        };
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(internal_status)?
            .as_secs();
        let mut health_cache = new_health_cache(&optimistic_account.borrow(), &retriever, now_ts)
            .map_err(|err| {
            self.reject_submit_request(
                request,
                Code::FailedPrecondition,
                format!("failed to build health cache for margin precheck: {err}"),
            )
        })?;
        let pre_init_health =
            optimistic_account
                .check_health_pre(&health_cache)
                .map_err(|err| {
                    self.reject_submit_request(
                        request,
                        Code::FailedPrecondition,
                        format!("account is not eligible for new intents: {err}"),
                    )
                })?;

        let mut orders_by_id = margin_snapshot.orders_by_id;
        apply_margin_check_ops(
            &mut optimistic_account,
            target_market,
            margin_ops,
            &mut orders_by_id,
        )
        .map_err(|err| {
            self.reject_submit_request(
                request,
                Code::FailedPrecondition,
                format!("failed to apply request delta for margin precheck: {err}"),
            )
        })?;
        let target_position = optimistic_account
            .perp_position(target_market_index)
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!(
                        "target perp position missing after request delta market={}: {err}",
                        target_market_index
                    ),
                )
            })?;
        health_cache
            .recompute_perp_info(target_position, target_market)
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!("failed to recompute perp health info for margin precheck: {err}"),
                )
            })?;
        optimistic_account
            .check_health_post(&health_cache, pre_init_health)
            .map_err(|err| {
                self.reject_submit_request(
                    request,
                    Code::FailedPrecondition,
                    format!("insufficient margin for requested order size: {err}"),
                )
            })?;
        Ok(())
    }

    fn terminal_head_failure_reason(
        &self,
        err: &TransactionError,
        logs: &[String],
    ) -> Option<TerminalHeadFailureReason> {
        if logs
            .iter()
            .any(|line| line.contains("Order is already expired"))
        {
            return Some(TerminalHeadFailureReason::ExpiredOrder);
        }

        if matches!(
            err,
            TransactionError::InstructionError(_, InstructionError::Custom(3007))
        ) || logs.iter().any(|line| {
            line.contains("AccountOwnedByWrongProgram")
                || line.contains("account owned by a different program")
                || line.contains("custom program error: 0xbbf")
        }) {
            return Some(TerminalHeadFailureReason::WrongProgramOwner);
        }

        // "no free perp order index" — the mango account's order slots are
        // full.  This item cannot be executed until slots are freed (which
        // requires event queue consumption that may never happen for this
        // account).  Drop the head to unblock the queue.
        if matches!(
            err,
            TransactionError::InstructionError(_, InstructionError::Custom(6000))
        ) && logs
            .iter()
            .any(|line| line.contains("no free perp order index"))
        {
            return Some(TerminalHeadFailureReason::PerpOrderSlotsFull);
        }

        // "health must be positive" / "health must be positive or not decrease"
        // The account lacks sufficient margin for this order.  The order will
        // never succeed unless the user deposits more — drop it.
        if matches!(
            err,
            TransactionError::InstructionError(_, InstructionError::Custom(6006))
                | TransactionError::InstructionError(_, InstructionError::Custom(6007))
        ) {
            return Some(TerminalHeadFailureReason::HealthCheckFailed);
        }

        None
    }

    fn terminal_head_failure_label(reason: TerminalHeadFailureReason) -> &'static str {
        match reason {
            TerminalHeadFailureReason::ExpiredOrder => "expired_order",
            TerminalHeadFailureReason::WrongProgramOwner => "account_owned_by_wrong_program",
            TerminalHeadFailureReason::PerpOrderSlotsFull => "perp_order_slots_full",
            TerminalHeadFailureReason::HealthCheckFailed => "health_check_failed",
        }
    }

    async fn maybe_auto_drop_terminal_head(
        &self,
        executor: &Arc<ExecutorState>,
        pending: &PendingHeadDispatch,
        err: &TransactionError,
    ) -> bool {
        if !self.config.executor_auto_drop_expired_heads {
            return false;
        }
        let logs = match self.fetch_transaction_logs(pending.signature).await {
            Ok(logs) => logs,
            Err(fetch_err) => {
                debug!(
                    "executor failed tx log fetch skipped sequence={} sig={} err={fetch_err:?}",
                    pending.sequence, pending.signature
                );
                return false;
            }
        };
        let Some(reason) = self.terminal_head_failure_reason(err, &logs) else {
            return false;
        };

        // When perp order slots are full, block the mango account from future
        // enqueues for 60 seconds.  The account pubkey is the second remaining
        // account in the lane (index 1: group, mango_account, owner, ...).
        if matches!(reason, TerminalHeadFailureReason::PerpOrderSlotsFull) {
            let lanes = executor.lanes_snapshot().await;
            if let Some(lane) = lanes.iter().find(|l| l.hash == pending.lane_hash) {
                if lane.remaining_accounts.len() > 1 {
                    let mango_acct_pk = lane.remaining_accounts[1].pubkey;
                    let block_until = unix_timestamp_ms() + 60_000;
                    self.blocked_mango_accounts
                        .lock()
                        .await
                        .insert(mango_acct_pk, block_until);
                    warn!(
                        "blocked mango account {} for 60s due to perp_order_slots_full",
                        mango_acct_pk
                    );
                }
            }
        }

        match self
            .drop_ctm_head_with_admin_tx(
                executor,
                pending.sequence,
                Self::terminal_head_failure_label(reason),
            )
            .await
        {
            Ok(signature) => {
                info!(
                    "executor auto-dropped terminal head sequence={} reason={} recovery_tx={}",
                    pending.sequence,
                    Self::terminal_head_failure_label(reason),
                    signature
                );
                true
            }
            Err(drop_err) => {
                warn!(
                    "executor failed to auto-drop terminal head sequence={} reason={} err={drop_err:?}",
                    pending.sequence,
                    Self::terminal_head_failure_label(reason),
                );
                false
            }
        }
    }

    async fn maybe_auto_drop_sequence_after_failure_threshold(
        &self,
        executor: &Arc<ExecutorState>,
        sequence: u64,
        context: &str,
    ) -> bool {
        let mut should_drop = false;
        {
            let mut seq_failures = executor.sequence_failure_counts.lock().await;
            let count = seq_failures.entry(sequence).or_insert(0);
            *count = count.saturating_add(1);
            if *count >= self.config.executor_max_sequence_failures {
                warn!(
                    "executor sequence {} hit {} consecutive failures ({context}); attempting proactive admin-drop",
                    sequence, count
                );
                *count = 0;
                should_drop = true;
            }
        }
        if !should_drop {
            return false;
        }
        match self
            .drop_ctm_head_with_admin_tx(executor, sequence, "sequence_failure_threshold")
            .await
        {
            Ok(sig) => {
                info!(
                    "executor proactive admin-drop succeeded sequence={} recovery_tx={}",
                    sequence, sig
                );
                true
            }
            Err(err) => {
                warn!(
                    "executor proactive admin-drop failed sequence={} err={err:?}",
                    sequence
                );
                false
            }
        }
    }

    fn current_unix_timestamp_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn inspect_expired_head_batch(
        &self,
        queue_data: &[u8],
        expected_sequence: u64,
    ) -> QueueExpiredHeadBatch {
        let queue_state = inspect_queue_admin_state(queue_data);
        if queue_state.head.reason != "ctm_pending"
            || queue_state.head.next_sequence != expected_sequence
        {
            return QueueExpiredHeadBatch {
                sequences: Vec::new(),
            };
        }
        let now_ts = self.current_unix_timestamp_secs();
        let expiry_cutoff =
            now_ts.saturating_sub(self.config.executor_expired_head_drop_grace_secs);
        let mut sequences = Vec::new();
        let start = queue_state.head.next_sequence;
        let end = queue_state
            .head
            .max_seen_sequence
            .saturating_add(1)
            .min(start.saturating_add(self.config.executor_expired_head_drop_batch_max as u64));
        let mut sequence = start;
        while sequence < end {
            let item_offset = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
                + (sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
            if item_offset + EXECUTION_QUEUE_ITEM_SIZE > queue_data.len() {
                break;
            }
            let slot_sequence = u64::from_le_bytes(
                queue_data[item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                    ..item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let slot_kind = queue_data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET];
            let slot_status = queue_data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
            if slot_status != 1 || slot_kind != 0 || slot_sequence != sequence {
                break;
            }
            let payload_len = u16::from_le_bytes(
                queue_data[item_offset + EXECUTION_QUEUE_ITEM_PAYLOAD_LEN_OFFSET
                    ..item_offset + EXECUTION_QUEUE_ITEM_PAYLOAD_LEN_OFFSET + 2]
                    .try_into()
                    .unwrap_or([0; 2]),
            ) as usize;
            if payload_len < 4
                || payload_len > (EXECUTION_QUEUE_ITEM_SIZE - EXECUTION_QUEUE_ITEM_PAYLOAD_OFFSET)
            {
                break;
            }
            let payload_offset = item_offset + EXECUTION_QUEUE_ITEM_PAYLOAD_OFFSET;
            let payload_end = payload_offset + payload_len;
            if payload_end > queue_data.len() {
                break;
            }
            let payload = &queue_data[payload_offset..payload_end];
            if payload[0] != 1 || payload[1] != 0 || payload[2] != 0 || payload[3] != 0 {
                break;
            }
            let decoded = match PerpPlaceOrderV2Payload::try_from_slice(&payload[4..]) {
                Ok(decoded) => decoded,
                Err(_) => break,
            };
            if decoded.expiry_timestamp == 0 || decoded.expiry_timestamp > expiry_cutoff {
                break;
            }
            sequences.push(sequence);
            sequence = sequence.saturating_add(1);
        }
        QueueExpiredHeadBatch { sequences }
    }

    async fn fetch_transaction_logs(&self, signature: Signature) -> Result<Vec<String>> {
        for attempt in 0..3u8 {
            let response = self
                .http_client
                .post(&self.config.cluster_url)
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": format!("get-transaction-{signature}-{attempt}"),
                    "method": "getTransaction",
                    "params": [
                        signature.to_string(),
                        {
                            "commitment": "confirmed",
                            "encoding": "json",
                            "maxSupportedTransactionVersion": 0
                        }
                    ]
                }))
                .send()
                .await?
                .error_for_status()?;
            let body: serde_json::Value = response.json().await?;
            if let Some(error) = body.get("error") {
                return Err(anyhow!("getTransaction rpc error for {signature}: {error}"));
            }
            if let Some(result) = body.get("result") {
                if result.is_null() {
                    sleep(Duration::from_millis(200)).await;
                    continue;
                }
                if let Some(logs) = result
                    .pointer("/meta/logMessages")
                    .and_then(|value| value.as_array())
                {
                    return Ok(logs
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect());
                }
                return Ok(Vec::new());
            }
        }
        Err(anyhow!(
            "transaction details unavailable for failed executor tx {}",
            signature
        ))
    }

    async fn await_signature_result(&self, signature: Signature, timeout_ms: u64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let response = self.rpc.get_signature_statuses(&[signature]).await?;
            match response.value.into_iter().next().flatten() {
                Some(status) if status.err.is_none() => return Ok(()),
                Some(status) => {
                    return Err(anyhow!(
                        "transaction {} failed while awaiting confirmation: {:?}",
                        signature,
                        status.err
                    ))
                }
                None if Instant::now() >= deadline => {
                    return Err(anyhow!(
                        "timed out awaiting transaction confirmation for {}",
                        signature
                    ))
                }
                None => sleep(Duration::from_millis(200)).await,
            }
        }
    }

    async fn drop_ctm_head_with_admin_tx(
        &self,
        executor: &Arc<ExecutorState>,
        sequence: u64,
        reason: &str,
    ) -> Result<Signature> {
        let queue_account = self.rpc.get_account(&executor.execution_queue).await?;
        let queue_state = inspect_queue_admin_state(&queue_account.data);
        let head_matches = queue_state.head.next_sequence == sequence;
        let head_is_pending = head_matches && queue_state.head.reason == "ctm_pending";
        let head_is_gap = head_matches && queue_state.head.is_ctm_gap_state();
        if !head_is_pending && !head_is_gap {
            return Err(anyhow!(
                "queue head moved before admin drop: expected_sequence={} current_sequence={} reason={}",
                sequence,
                queue_state.head.next_sequence,
                queue_state.head.reason
            ));
        }
        let recovery_mode = if head_is_pending {
            "head_drop"
        } else {
            "gap_span_drop"
        };
        let expired_batch = self.inspect_expired_head_batch(&queue_account.data, sequence);
        let sequences_to_drop = if head_is_pending {
            if matches!(reason, "no_lane_match_stale" | "sequence_failure_threshold") {
                let batch = inspect_no_lane_match_batch(
                    &queue_account.data,
                    &queue_state.head,
                    self.config.executor_no_lane_match_drop_batch_max,
                );
                if batch.is_empty() {
                    vec![sequence]
                } else {
                    batch
                }
            } else if expired_batch.sequences.is_empty() {
                vec![sequence]
            } else {
                expired_batch.sequences
            }
        } else {
            let gap_batch = inspect_gap_recovery_batch(
                &queue_account.data,
                &queue_state.head,
                self.config.executor_gap_recovery_drop_batch_max,
            );
            if gap_batch.is_empty() {
                return Err(anyhow!(
                    "queue gap recovery found no pending CTM span: expected_sequence={} current_sequence={} max_seen_sequence={}",
                    sequence,
                    queue_state.head.next_sequence,
                    queue_state.head.max_seen_sequence
                ));
            }
            gap_batch
        };

        let mut instructions = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        if self.config.executor_prioritization_fee > 0 {
            instructions.push(ComputeBudgetInstruction::set_compute_unit_price(
                self.config.executor_prioritization_fee,
            ));
        }
        instructions.push(build_execution_queue_configure_instruction(
            self.config.program_id,
            executor.group,
            executor.execution_queue,
            self.config.executor_admin.pubkey(),
            &queue_state,
            true,
        ));
        for sequence in &sequences_to_drop {
            instructions.push(build_execution_queue_drop_ctm_instruction(
                self.config.program_id,
                executor.group,
                executor.execution_queue,
                self.config.executor_admin.pubkey(),
                *sequence,
            ));
        }
        instructions.push(build_execution_queue_configure_instruction(
            self.config.program_id,
            executor.group,
            executor.execution_queue,
            self.config.executor_admin.pubkey(),
            &queue_state,
            queue_state.pause_execute,
        ));

        let chain = self.blockhashes.snapshot().await;
        let message = MessageV0::try_compile(
            &self.config.payer.pubkey(),
            &instructions,
            &[],
            chain.blockhash,
        )?;
        let tx = if self.config.executor_admin.pubkey() == self.config.payer.pubkey() {
            VersionedTransaction::try_new(
                solana_sdk::message::VersionedMessage::V0(message),
                &[self.config.payer.as_ref()],
            )?
        } else {
            VersionedTransaction::try_new(
                solana_sdk::message::VersionedMessage::V0(message),
                &[
                    self.config.payer.as_ref(),
                    self.config.executor_admin.as_ref(),
                ],
            )?
        };
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.executor_skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: Some(0),
            ..RpcSendTransactionConfig::default()
        };
        let signature = self.rpc.send_transaction_with_config(&tx, send_cfg).await?;
        self.await_signature_result(signature, self.config.executor_admin_tx_timeout_ms)
            .await?;
        executor.pending_dispatches.lock().await.clear();
        {
            let mut sequence_failures = executor.sequence_failure_counts.lock().await;
            for dropped_sequence in &sequences_to_drop {
                sequence_failures.remove(dropped_sequence);
            }
        }
        executor.last_inspect_ms.store(0, Ordering::Relaxed);
        *executor.cached_head.lock().await = None;
        info!(
            "executor admin recovery completed start_sequence={} first_dropped_sequence={} last_dropped_sequence={} dropped={} mode={} reason={} tx={}",
            sequence,
            sequences_to_drop.first().copied().unwrap_or(sequence),
            sequences_to_drop.last().copied().unwrap_or(sequence),
            sequences_to_drop.len(),
            recovery_mode,
            reason,
            signature
        );
        Ok(signature)
    }

    fn current_queue_state_for(
        &self,
        group: Pubkey,
        execution_queue: Pubkey,
    ) -> Option<(u32, u64, bool)> {
        self.executor.as_ref().and_then(|executor| {
            if executor.group == group && executor.execution_queue == execution_queue {
                Some((
                    executor.queue_count(),
                    executor.queue_gap_span(),
                    executor.queue_head_available(),
                ))
            } else {
                None
            }
        })
    }

    fn current_queue_floor_for(&self, group: Pubkey, execution_queue: Pubkey) -> Option<u64> {
        self.executor.as_ref().and_then(|executor| {
            if executor.group == group && executor.execution_queue == execution_queue {
                Some(executor.queue_next_sequence())
            } else {
                None
            }
        })
    }

    async fn refresh_dynamic_lanes_from_event_log(
        &self,
        executor: &Arc<ExecutorState>,
    ) -> Result<()> {
        let Some(path) = self.config.executor_relay_event_log_path.as_ref() else {
            return Ok(());
        };
        if !executor
            .should_refresh_dynamic_lanes(self.config.executor_dynamic_lanes_refresh_ms)
            .await
        {
            return Ok(());
        }
        let content = tokio::fs::read_to_string(path).await?;
        let lines: Vec<&str> = content
            .lines()
            .rev()
            .filter(|line| line.contains("\"event_type\":\"relay_intent_accepted\""))
            .take(self.config.executor_dynamic_lanes_max_events)
            .collect();
        for line in lines.into_iter().rev() {
            let Ok(event) = serde_json::from_str::<RelayIntentAcceptedEvent>(line) else {
                continue;
            };
            if event.event_type != "relay_intent_accepted"
                || event.group != executor.group.to_string()
                || event.execution_queue != executor.execution_queue.to_string()
                || event.remaining_accounts.is_empty()
            {
                continue;
            }
            let mut remaining_accounts = Vec::with_capacity(event.remaining_accounts.len());
            let mut parse_failed = false;
            for account in &event.remaining_accounts {
                let Ok(pubkey) = Pubkey::from_str(&account.pubkey) else {
                    parse_failed = true;
                    break;
                };
                remaining_accounts.push(AccountMeta {
                    pubkey,
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                });
            }
            if parse_failed {
                continue;
            }
            let is_new = executor
                .register_dynamic_lane(
                    format!(
                        "dynamic-{}-{}-{}",
                        event.market,
                        event.sequence,
                        event.user_owner.chars().take(8).collect::<String>()
                    ),
                    &remaining_accounts,
                    self.config.executor_include_legacy_fixed_hash,
                )
                .await;
            if is_new {
                if let Some(cache_path) = &self.config.executor_lane_cache_path {
                    let _ = self
                        .append_lane_cache(cache_path, &event, &remaining_accounts)
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn append_lane_cache(
        &self,
        cache_path: &PathBuf,
        event: &RelayIntentAcceptedEvent,
        remaining_accounts: &[AccountMeta],
    ) -> Result<()> {
        #[derive(Serialize, Deserialize)]
        struct CachedLane {
            name: String,
            user_owner: String,
            #[serde(rename = "remainingAccounts")]
            remaining_accounts: Vec<CachedLaneAccount>,
        }
        #[derive(Serialize, Deserialize)]
        struct CachedLaneAccount {
            pubkey: String,
            #[serde(rename = "isWritable")]
            is_writable: bool,
            #[serde(rename = "isSigner")]
            is_signer: bool,
        }

        // Read existing cache or start fresh
        let mut cache: Vec<CachedLane> = if cache_path.exists() {
            let content = tokio::fs::read_to_string(cache_path)
                .await
                .unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Vec::new()
        };

        // Dedup by user_owner — keep only the latest lane per owner
        let owner = &event.user_owner;
        let lane_name = format!("cached-{}-{}", event.market, &owner[..owner.len().min(12)]);
        cache.retain(|l| l.user_owner != *owner);
        cache.push(CachedLane {
            name: lane_name.clone(),
            user_owner: owner.clone(),
            remaining_accounts: remaining_accounts
                .iter()
                .map(|a| CachedLaneAccount {
                    pubkey: a.pubkey.to_string(),
                    is_writable: a.is_writable,
                    is_signer: a.is_signer,
                })
                .collect(),
        });

        let json = serde_json::to_string_pretty(&cache)?;
        tokio::fs::write(cache_path, json).await?;
        info!(
            "lane cache updated: owner={} lane={} total_cached={}",
            owner,
            lane_name,
            cache.len()
        );
        Ok(())
    }

    async fn submit_intent(
        &self,
        request: SubmitIntentRequest,
    ) -> Result<SubmitIntentResponse, Status> {
        // Bump ingress at the boundary BEFORE acquire_permit so that
        // queue-timeout / semaphore rejections still show up as ingress.
        // Single relaxed fetch_add — does not block the hot path.
        self.metrics.record_ingress();
        // Capture wall-clock arrival time for ingress->ob latency tracking.
        // unix_timestamp_ms is one syscall; not held across an await beyond
        // what submit_intent already does.
        let request_arrived_ms = unix_timestamp_ms();
        let started = Instant::now();
        let permit = match self.acquire_permit().await {
            Ok(permit) => permit,
            Err(status) => {
                // Permit-acquire rejection still counts toward ingress outcome.
                self.metrics.record_ingress_outcome(false);
                return Err(status);
            }
        };
        self.metrics.inflight.fetch_add(1, Ordering::Relaxed);

        let result = self.submit_intent_inner(request).await;

        self.metrics.inflight.fetch_sub(1, Ordering::Relaxed);
        drop(permit);

        let elapsed = started.elapsed();
        self.metrics.observe_submit(elapsed, result.is_ok());
        self.metrics.record_ingress_outcome(result.is_ok());

        // Stamp the latency trackers only for accepted intents — only those
        // ever land on chain or reach the harness state. All three taps are
        // bounded, lock-and-release operations that never hold a lock across
        // an `.await`.
        if let Ok(ref response) = result {
            // (1) Submit latency: ingress -> relayer's submit_intent Ok.
            self.metrics
                .record_submit_latency(elapsed.as_millis() as u64);
            // (2) Optimistic latency: ingress -> harness applies the
            // relay-intent event into its optimistic state.
            self.latency_optimistic_tracker
                .note_ingress(response.sequence, request_arrived_ms);
            // (3) Processed latency: ingress -> harness has reconciled past
            // the sequence from on-chain state.
            self.latency_processed_tracker
                .note_ingress(response.sequence, request_arrived_ms);
        }

        result
    }

    async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit, Status> {
        timeout(
            Duration::from_millis(self.config.queue_wait_timeout_ms),
            self.inflight.clone().acquire_owned(),
        )
        .await
        .map_err(|_| Status::resource_exhausted("execution engine queue timeout"))?
        .map_err(|_| Status::internal("execution engine semaphore closed"))
    }

    async fn submit_intent_inner(
        &self,
        request: SubmitIntentRequest,
    ) -> Result<SubmitIntentResponse, Status> {
        let mut status_ctx = RelayIntentStatusContext::from_request(&request);
        let result: Result<SubmitIntentResponse, Status> = async {
            let parse_started = Instant::now();
            // Validate market_index is present and parseable as u16 BEFORE
            // any other work. Reject empty/undefined/non-numeric market values
            // with a clear error so callers don't waste compute on the rest of
            // the pipeline.
            if request.market.trim().is_empty() {
                return Err(self.reject_submit_request(
                    &request,
                    Code::InvalidArgument,
                    "market index missing: request.market is empty or undefined",
                ));
            }
            if request.market.trim().parse::<u16>().is_err() {
                return Err(self.reject_submit_request(
                    &request,
                    Code::InvalidArgument,
                    format!(
                        "market index invalid: '{}' is not a valid u16 perp market index",
                        request.market
                    ),
                ));
            }
            let keys = self.parse_submit_intent_keys(&request)?;
            // Phase 0: stamp parse-stage latency. Anything that early-returns
            // before this point is excluded from the histogram, which is what
            // we want — we're measuring the happy path.
            let after_parse_elapsed = parse_started.elapsed();
            self.metrics
                .stage_parse_latency
                .record(after_parse_elapsed.as_millis() as u64);
            let group = keys.group;
            let execution_queue = keys.execution_queue;
            if self.config.queue_soft_limit > 0 {
                if let Some((queue_count, gap_span, head_available)) =
                    self.current_queue_state_for(group, execution_queue)
                {
                    if queue_count >= self.config.queue_soft_limit {
                        return Err(Status::resource_exhausted(format!(
                            "execution queue backpressure count={} soft_limit={}",
                            queue_count, self.config.queue_soft_limit
                        )));
                    }
                    if !head_available && gap_span >= self.config.queue_gap_soft_limit {
                        return Err(Status::resource_exhausted(format!(
                            "execution queue gap backpressure count={} gap_span={} gap_soft_limit={}",
                            queue_count, gap_span, self.config.queue_gap_soft_limit
                        )));
                    }
                    let degraded_head_limit = self.config.queue_soft_limit / 2;
                    if !head_available
                        && degraded_head_limit > 0
                        && queue_count >= degraded_head_limit
                    {
                        return Err(Status::resource_exhausted(format!(
                            "execution queue head-gap backpressure count={} degraded_limit={} gap_span={}",
                            queue_count, degraded_head_limit, gap_span
                        )));
                    }
                }
            }
            self.ensure_harness_ready(&request.market).await?;
            self.ensure_submit_margin_ready(&request, keys).await?;
            // Phase 0: stamp margin-check stage latency, computed against the
            // parse stage end so it's strictly the time spent in
            // ensure_harness_ready + ensure_submit_margin_ready.
            let after_margin_elapsed = parse_started.elapsed();
            self.metrics.stage_margin_check_latency.record(
                after_margin_elapsed
                    .saturating_sub(after_parse_elapsed)
                    .as_millis() as u64,
            );
            let user_owner = keys.user_owner;
            let mango_account = keys.mango_account;

            // Hot-path: stamp the sender's pubkey into the unique-address
            // tracker. Bounded HashMap insert; never holds the lock across an
            // await; failures are silently ignored.
            self.note_user_owner(&user_owner, unix_timestamp_ms());

            // Pre-enqueue gate: reject intents for mango accounts whose perp
            // order slots are known to be full.
            {
                let mut blocked = self.blocked_mango_accounts.lock().await;
                let now_ms = unix_timestamp_ms();
                // Evict expired blocks
                blocked.retain(|_, expires_ms| *expires_ms > now_ms);
                if let Some(expires_ms) = blocked.get(&mango_account) {
                    return Err(Status::resource_exhausted(format!(
                        "mango account {} perp order slots full, blocked until {}ms",
                        mango_account, expires_ms
                    )));
                }
            }
            let remaining_accounts = parse_remaining_accounts(&request.remaining_accounts)?;
            let user_signature = parse_signature_bytes(&request.user_signature)?;
            let chain = self.blockhashes.snapshot().await;
            let parse_elapsed = parse_started.elapsed();
            let min_execute_slot = if request.min_execute_slot == 0 {
                chain.slot + self.config.min_execute_slot_offset
            } else {
                request.min_execute_slot
            };
            let expires_at_slot = request.expires_at_slot;

            let sequence_key = format!("{}:{}", group, request.market);
            if let Some(queue_floor) = self.current_queue_floor_for(group, execution_queue) {
                self.sequences
                    .observe_queue_floor(&sequence_key, queue_floor)
                    .await;
                if self.config.sequence_pending_soft_limit > 0 {
                    let submitted_depth = self
                        .sequences
                        .submitted_depth_from(&sequence_key, queue_floor)
                        .await;
                    if submitted_depth >= self.config.sequence_pending_soft_limit {
                        return Err(Status::resource_exhausted(format!(
                            "execution queue sequence backpressure submitted_depth={} floor={} soft_limit={}",
                            submitted_depth, queue_floor, self.config.sequence_pending_soft_limit
                        )));
                    }
                }
            }
            let sequence = self.sequences.reserve(&sequence_key).await;
            status_ctx.with_sequence(sequence);

            let payload_hash = hashv(&[&request.payload]).to_bytes();
            let accounts_hash = hash_execution_queue_accounts_for_ctm_enqueue(
                group,
                execution_queue,
                &remaining_accounts,
            );
            let envelope = CtmEnvelope {
                sequence,
                min_execute_slot,
                kind: 0,
                payload_hash,
                accounts_hash,
                expires_at_slot,
            };
            status_ctx.with_kind(envelope.kind as u32);

            let user_intent_message =
                canonical_user_intent_message(group, mango_account, user_owner, &envelope);
            let ctm_envelope_message = canonical_envelope_message(group, &envelope);

            let user_message_variant = if self.config.verify_user_signature {
                verify_user_signature(user_owner, &user_signature, &user_intent_message)?
            } else {
                UserSignatureMessage::Raw(user_intent_message)
            };

            let user_preinstruction = build_presigned_ed25519_instruction(
                user_owner.to_bytes(),
                user_message_variant.as_bytes(),
                user_signature,
            );
            let ctm_signature = self.config.ctm.sign_message(&ctm_envelope_message);
            let ctm_preinstruction = build_presigned_ed25519_instruction(
                self.config.ctm.pubkey().to_bytes(),
                &ctm_envelope_message,
                ctm_signature
                    .as_ref()
                    .try_into()
                    .map_err(|_| Status::internal("ctm signature length was not 64 bytes"))?,
            );
            let enqueue_instruction = build_enqueue_instruction(
                self.config.program_id,
                group,
                execution_queue,
                &remaining_accounts,
                envelope.clone(),
                request.payload.clone(),
            );
            let prepare_elapsed = parse_started.elapsed().saturating_sub(parse_elapsed);

            let mut instructions = vec![
                ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
                user_preinstruction,
                ctm_preinstruction,
                enqueue_instruction,
            ];
            if self.config.prioritization_fee > 0 {
                instructions.insert(
                    0,
                    ComputeBudgetInstruction::set_compute_unit_price(
                        self.config.prioritization_fee,
                    ),
                );
            }

            let message = MessageV0::try_compile(
                &self.config.payer.pubkey(),
                &instructions,
                &[],
                chain.blockhash,
            )
            .map_err(internal_status)?;
            let tx = VersionedTransaction::try_new(
                solana_sdk::message::VersionedMessage::V0(message),
                &[self.config.payer.as_ref()],
            )
            .map_err(internal_status)?;
            // Phase 0: stamp sign-stage latency, computed against the margin
            // stage end so it captures sequence reservation, envelope build,
            // signature verification, instruction assembly, MessageV0 compile,
            // and VersionedTransaction signing.
            let after_sign_elapsed = parse_started.elapsed();
            self.metrics.stage_sign_latency.record(
                after_sign_elapsed
                    .saturating_sub(after_margin_elapsed)
                    .as_millis() as u64,
            );
            self.maybe_emit_status_event(status_ctx.event(
                1,
                "accepted",
                None,
                None,
                None,
                None,
            ))
            .await;
            // Phase 1.1: the signature is already locally derivable from the
            // signed VersionedTransaction; we don't need rpc.send_transaction
            // to echo it back. Capture it here and hand the tx off to the
            // background submitter pool. The user-visible gRPC response no
            // longer waits for Helius — the bg worker drains it
            // asynchronously.
            let tx_signature = tx.signatures[0];
            status_ctx.with_signature(&tx_signature);

            // Suppress unused-warning for prepare_elapsed (kept for now for
            // diagnostic legibility; the bg submitter records its own
            // bg_rpc_submit timing).
            let _ = (parse_elapsed, prepare_elapsed);

            // Phase 3: apply the intent to the in-process optimistic state
            // BEFORE bg dispatch so subsequent reads (margin checks,
            // orderbook polls, frontend SSE) reflect it immediately. The
            // UndoToken travels with the PendingSubmit so the bg worker
            // can roll it back on hard ingress failure.
            let undo_token =
                self.apply_relay_intent_to_local_state(&request, &envelope, &tx_signature);

            let dispatch_started = Instant::now();
            let pending = PendingSubmit {
                sequence_key: sequence_key.clone(),
                sequence,
                execution_queue,
                tx,
                tx_signature,
                attempts: 0,
                enqueued_at_ms: unix_timestamp_ms(),
                undo_token,
            };
            match self.bg_submit_tx.try_send(pending) {
                Ok(()) => {
                    self.metrics
                        .stage_event_dispatch_latency
                        .record(dispatch_started.elapsed().as_millis() as u64);
                    self.metrics
                        .bg_submit_inflight
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(mpsc::error::TrySendError::Full(rejected)) => {
                    self.metrics
                        .bg_submit_channel_full_total
                        .fetch_add(1, Ordering::Relaxed);
                    self.recover_sequence_after_submit_error(
                        &sequence_key,
                        sequence,
                        execution_queue,
                    )
                    .await;
                    // Phase 3: roll back the local state apply since the
                    // tx will never be submitted.
                    if let (Some(state), Some(undo)) =
                        (self.state.as_ref(), rejected.undo_token)
                    {
                        let _ = state.lock().rollback_local(undo);
                    }
                    return Err(Status::resource_exhausted(
                        "background submit channel full",
                    ));
                }
                Err(mpsc::error::TrySendError::Closed(rejected)) => {
                    self.recover_sequence_after_submit_error(
                        &sequence_key,
                        sequence,
                        execution_queue,
                    )
                    .await;
                    if let (Some(state), Some(undo)) =
                        (self.state.as_ref(), rejected.undo_token)
                    {
                        let _ = state.lock().rollback_local(undo);
                    }
                    return Err(Status::internal("background submit channel closed"));
                }
            }

            // Hand-off to the bg pool succeeded — commit the local sequence
            // cursor, spawn the chain-confirmation watcher, register the
            // dynamic executor lane, and emit downstream status events.
            self.sequences.commit_success(&sequence_key, sequence).await;
            tokio::spawn(self.clone().watch_submitted_sequence(
                sequence_key.clone(),
                sequence,
                execution_queue,
                tx_signature,
            ));

            if let Some(executor) = &self.executor {
                if executor.group == group && executor.execution_queue == execution_queue {
                    executor
                        .register_dynamic_lane(
                            format!("dynamic-{}-{}", request.market, sequence),
                            &remaining_accounts,
                            self.config.executor_include_legacy_fixed_hash,
                        )
                        .await;
                }
            }

            self.maybe_emit_status_event(status_ctx.event(
                2,
                "submitted",
                None,
                None,
                None,
                None,
            ))
            .await;
            self.maybe_emit_event(&request, &envelope, &tx_signature, &status_ctx.request_id)
                .await;

            Ok(SubmitIntentResponse {
                sequence,
                tx_signature: tx_signature.to_string(),
                user_intent_message: user_intent_message.to_vec(),
                ctm_envelope_message: ctm_envelope_message.to_vec(),
            })
        }
        .await;

        if let Err(status) = &result {
            self.maybe_emit_status_event(status_ctx.event(
                0,
                "rejected",
                Some(status.message().to_string()),
                Some(status.code() as i32),
                None,
                None,
            ))
            .await;
        }

        result
    }

    async fn recover_sequence_after_submit_error(
        &self,
        sequence_key: &str,
        failed_sequence: u64,
        execution_queue: Pubkey,
    ) {
        let account = match self.rpc.get_account(&execution_queue).await {
            Ok(account) => account,
            Err(err) => {
                warn!(
                    "failed to recover sequence state for key={} queue={}: {err:?}",
                    sequence_key, execution_queue
                );
                return;
            }
        };
        let fallback_next = inspect_next_enqueue_sequence(&account.data);
        self.sequences
            .observe_queue_floor(sequence_key, fallback_next)
            .await;
        match inspect_queue_sequence_presence(&account.data, failed_sequence) {
            QueueSequencePresence::PastFloor => {}
            QueueSequencePresence::Pending => {
                self.sequences
                    .mark_submitted(sequence_key, failed_sequence)
                    .await;
            }
            QueueSequencePresence::Absent => {
                self.sequences
                    .reset_after_failure(sequence_key, failed_sequence, fallback_next)
                    .await;
            }
        }
    }

    /// Drain one PendingSubmit through `rpc.send_transaction_with_config`.
    /// Called from background submitter workers; never on the user-visible
    /// hot path. Outcomes:
    /// - Success: bump bg_submit_ok_total, decrement bg_submit_inflight.
    /// - Transient failure (retryable): bump bg_submit_transient_total,
    ///   spawn a delayed retry that re-enqueues the same PendingSubmit.
    ///   bg_submit_inflight stays elevated until the retry resolves.
    /// - Hard failure or retries exhausted: bump bg_submit_failed_total,
    ///   roll back the local sequence cursor, decrement bg_submit_inflight.
    async fn handle_bg_submit(&self, mut pending: PendingSubmit) {
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: self.config.submit_rpc_max_retries,
            ..RpcSendTransactionConfig::default()
        };
        let send_started = Instant::now();
        match self
            .rpc
            .send_transaction_with_config(&pending.tx, send_cfg)
            .await
        {
            Ok(_) => {
                self.metrics
                    .stage_bg_rpc_submit_latency
                    .record(send_started.elapsed().as_millis() as u64);
                self.metrics
                    .bg_submit_ok_total
                    .fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .bg_submit_inflight
                    .fetch_sub(1, Ordering::Relaxed);
            }
            Err(err)
                if is_transient_rpc_error(&err)
                    && pending.attempts < self.config.bg_submit_max_retries =>
            {
                self.metrics
                    .bg_submit_transient_total
                    .fetch_add(1, Ordering::Relaxed);
                pending.attempts = pending.attempts.saturating_add(1);
                let shift = pending.attempts.min(6) as u64;
                let backoff = Duration::from_millis(
                    self.config.bg_submit_retry_base_ms.saturating_mul(1u64 << shift),
                );
                debug!(
                    "bg submitter transient error sequence={} attempt={} backoff_ms={} err={err:?}",
                    pending.sequence,
                    pending.attempts,
                    backoff.as_millis()
                );
                let retry_tx = self.bg_submit_tx.clone();
                let metrics = self.metrics.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(backoff).await;
                    if let Err(send_err) = retry_tx.send(pending).await {
                        warn!("bg submitter retry channel closed: {send_err:?}");
                        metrics
                            .bg_submit_retry_dropped_total
                            .fetch_add(1, Ordering::Relaxed);
                        metrics
                            .bg_submit_failed_total
                            .fetch_add(1, Ordering::Relaxed);
                        metrics
                            .bg_submit_inflight
                            .fetch_sub(1, Ordering::Relaxed);
                    }
                });
            }
            Err(err) => {
                self.metrics
                    .stage_bg_rpc_submit_latency
                    .record(send_started.elapsed().as_millis() as u64);
                self.metrics
                    .bg_submit_failed_total
                    .fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .bg_submit_inflight
                    .fetch_sub(1, Ordering::Relaxed);
                let inflight_ms = unix_timestamp_ms().saturating_sub(pending.enqueued_at_ms);
                warn!(
                    "bg submitter hard failure sequence={} sig={} attempts={} inflight_ms={} err={err:?}",
                    pending.sequence, pending.tx_signature, pending.attempts, inflight_ms
                );
                if !is_execution_queue_duplicate_sequence_error(&err) {
                    self.recover_sequence_after_submit_error(
                        &pending.sequence_key,
                        pending.sequence,
                        pending.execution_queue,
                    )
                    .await;
                }
                // Phase 3: roll back the in-process optimistic state apply
                // since the tx never landed on chain. The reconciler is a
                // second safety net for any state we miss here.
                if let (Some(state), Some(undo)) =
                    (self.state.as_ref(), pending.undo_token.take())
                {
                    if let Err(rb_err) = state.lock().rollback_local(undo) {
                        warn!(
                            "bg submitter rollback_local failed sequence={} err={rb_err:?}",
                            pending.sequence
                        );
                    }
                }
            }
        }
    }

    async fn watch_submitted_sequence(
        self,
        sequence_key: String,
        sequence: u64,
        execution_queue: Pubkey,
        tx_signature: Signature,
    ) {
        let started_at_ms = unix_timestamp_ms();
        loop {
            if unix_timestamp_ms().saturating_sub(started_at_ms)
                >= self.config.sequence_submitted_stale_ms
            {
                debug!(
                    "submit watcher timed out sequence={} sig={} queue={}",
                    sequence, tx_signature, execution_queue
                );
                self.recover_sequence_after_submit_error(&sequence_key, sequence, execution_queue)
                    .await;
                return;
            }

            match self.rpc.get_signature_statuses(&[tx_signature]).await {
                Ok(response) => match response.value.into_iter().next().flatten() {
                    Some(status) if status.err.is_none() => {
                        return;
                    }
                    Some(status) => {
                        warn!(
                            "submit watcher observed failed enqueue sequence={} sig={} err={:?}",
                            sequence, tx_signature, status.err
                        );
                        self.recover_sequence_after_submit_error(
                            &sequence_key,
                            sequence,
                            execution_queue,
                        )
                        .await;
                        return;
                    }
                    None => {}
                },
                Err(err) => {
                    debug!(
                        "submit watcher status poll failed sequence={} sig={} err={err:?}",
                        sequence, tx_signature
                    );
                }
            }

            tokio::time::sleep(Duration::from_millis(
                self.config.sequence_submit_watch_poll_ms,
            ))
            .await;
        }
    }

    /// Phase 3: build a `RelayIntentAcceptedEvent` from the relayer's
    /// SubmitIntentRequest + envelope, and apply it INCREMENTALLY to the
    /// in-process optimistic state. Returns the resulting `UndoToken` so
    /// the caller can plumb it through to the bg submitter for hard-fail
    /// rollback. Returns `None` when local state is disabled or when the
    /// apply itself failed (in which case there's nothing to undo).
    ///
    /// This is the hot-path entry point for Phase 3. Single parking_lot
    /// Mutex acquisition; never held across an `.await`.
    fn apply_relay_intent_to_local_state(
        &self,
        request: &SubmitIntentRequest,
        envelope: &CtmEnvelope,
        tx_signature: &Signature,
    ) -> Option<UndoToken> {
        let state = self.state.as_ref()?;
        use base64::Engine as _;
        let local_event = rust_harness::RelayIntentAcceptedEvent {
            event_type: "relay_intent_accepted".to_string(),
            ts_ms: unix_timestamp_ms(),
            group: request.group.clone(),
            execution_queue: request.execution_queue.clone(),
            market: request.market.clone(),
            sequence: envelope.sequence.to_string(),
            kind: envelope.kind,
            payload_b64: base64::engine::general_purpose::STANDARD.encode(&request.payload),
            remaining_accounts: request
                .remaining_accounts
                .iter()
                .map(|account: &AccountMetaProto| rust_harness::AccountMetaWire {
                    pubkey: account.pubkey.clone(),
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                })
                .collect(),
            min_execute_slot: envelope.min_execute_slot.to_string(),
            expires_at_slot: envelope.expires_at_slot.to_string(),
            user_owner: request.user_owner.clone(),
            mango_account: request.mango_account.clone(),
            enqueue_tx_signature: tx_signature.to_string(),
        };
        match state.lock().apply_relay_intent_local(local_event) {
            Ok((_delta, undo)) => Some(undo),
            Err(err) => {
                warn!(
                    "local state apply_relay_intent_local failed: sequence={} err={err:?}",
                    envelope.sequence
                );
                None
            }
        }
    }

    async fn maybe_emit_event(
        &self,
        request: &SubmitIntentRequest,
        envelope: &CtmEnvelope,
        tx_signature: &Signature,
        request_id: &str,
    ) {
        // Phase 2/3: the in-process state apply moved to
        // `apply_relay_intent_to_local_state`, called earlier in the hot
        // path so its UndoToken can be plumbed through to the bg
        // submitter. This function only handles the legacy HTTP sink for
        // any consumers (e.g. the TS harness shell) that haven't migrated
        // off it yet.
        use base64::Engine as _;
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&request.payload);
        let ts_ms = unix_timestamp_ms();

        let Some(url) = self.config.event_sink_url.clone() else {
            return;
        };
        #[derive(Serialize)]
        struct EventAccountMeta {
            pubkey: String,
            is_signer: bool,
            is_writable: bool,
        }
        #[derive(Serialize)]
        struct EventBody {
            event_type: &'static str,
            ts_ms: u64,
            request_id: String,
            group: String,
            execution_queue: String,
            market: String,
            sequence: String,
            kind: u32,
            payload_b64: String,
            remaining_accounts: Vec<EventAccountMeta>,
            min_execute_slot: String,
            expires_at_slot: String,
            user_owner: String,
            mango_account: String,
            enqueue_tx_signature: String,
        }
        let remaining_accounts = request
            .remaining_accounts
            .iter()
            .map(|account: &AccountMetaProto| EventAccountMeta {
                pubkey: account.pubkey.clone(),
                is_signer: account.is_signer,
                is_writable: account.is_writable,
            })
            .collect();
        let body = EventBody {
            event_type: "relay_intent_accepted",
            ts_ms,
            request_id: request_id.to_string(),
            group: request.group.clone(),
            execution_queue: request.execution_queue.clone(),
            market: request.market.clone(),
            sequence: envelope.sequence.to_string(),
            kind: envelope.kind as u32,
            payload_b64,
            remaining_accounts,
            min_execute_slot: envelope.min_execute_slot.to_string(),
            expires_at_slot: envelope.expires_at_slot.to_string(),
            user_owner: request.user_owner.clone(),
            mango_account: request.mango_account.clone(),
            enqueue_tx_signature: tx_signature.to_string(),
        };
        self.emit_sink_json(body, url).await;
    }

    async fn maybe_emit_status_event(&self, event: RelayIntentStatusEvent) {
        let Some(url) = self.config.event_sink_url.clone() else {
            return;
        };
        self.emit_sink_json(event, url).await;
    }

    async fn emit_sink_json<T>(&self, body: T, url: String)
    where
        T: Serialize + Send + Sync + 'static,
    {
        if let Ok(line) = serde_json::to_string(&body) {
            info!("{line}");
        }
        let client = self.http_client.clone();
        tokio::spawn(async move {
            if let Err(err) = client.post(url).json(&body).send().await {
                warn!("event sink request failed: {err:?}");
            }
        });
    }

    /// Periodic loop that consumes events from the perp event queue. Without
    /// this, fill events accumulate and maker OO slots are never freed,
    /// eventually triggering "no free perp order index" errors that block the
    /// execution queue. The on-chain `perp_consume_events` instruction caps
    /// limit at 8 per call, so we run on a tight interval (default 2s).
    async fn run_perp_event_consumer(self: Arc<Self>, executor: Arc<ExecutorState>) {
        let interval_ms = self.config.executor_perp_consume_interval_ms;
        if interval_ms == 0 {
            info!("perp event consumer disabled (interval_ms=0)");
            return;
        }
        let limit = self.config.executor_perp_consume_limit;
        info!(
            "perp event consumer enabled group={} queue={} interval_ms={} limit={}",
            executor.group, executor.execution_queue, interval_ms, limit
        );

        let mut known_markets: HashSet<Pubkey> = HashSet::new();

        loop {
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            // Heartbeat the cranker on every tick — even an empty queue is a
            // sign the cranker is alive and polling.
            self.metrics.tick_event_cranker(unix_timestamp_ms());

            // Re-scan lanes every tick so newly added markets (dynamic lanes)
            // get picked up. Canonical layout per lane:
            //   [0] group, [1] mango_account, [2] owner,
            //   [3] perp_market, [4] bids, [5] asks, [6] event_queue, [7] oracle, ...
            // Dedupe by perp_market to avoid consuming the same event queue
            // multiple times per tick when multiple lanes exist for the same
            // market (different accounts, same perp market).
            let lanes = executor.lanes_snapshot().await;
            let mut markets: Vec<(Pubkey, Pubkey)> = Vec::new();
            let mut seen: HashSet<Pubkey> = HashSet::new();
            for lane in lanes.iter() {
                if lane.remaining_accounts.len() < 7 {
                    continue;
                }
                let perp_market = lane.remaining_accounts[3].pubkey;
                let event_queue = lane.remaining_accounts[6].pubkey;
                if seen.insert(perp_market) {
                    markets.push((perp_market, event_queue));
                }
            }

            // Log newly discovered markets
            for (pm, _) in &markets {
                if known_markets.insert(*pm) {
                    info!("perp event consumer tracking new market perp_market={pm}");
                }
            }

            if markets.is_empty() {
                continue;
            }

            // Consume events for each unique (perp_market, event_queue) pair.
            // Serial to avoid blockhash race and RPC rate limits; each call is
            // only one tx so this stays cheap even with many markets.
            for (perp_market_pk, event_queue_pk) in markets {
                match self
                    .consume_perp_events_once(
                        executor.group,
                        perp_market_pk,
                        event_queue_pk,
                        limit,
                    )
                    .await
                {
                    Ok(0) => {
                        // queue empty — nothing to do
                    }
                    Ok(consumed) => {
                        debug!(
                            "perp event consumer consumed {} events perp_market={}",
                            consumed, perp_market_pk
                        );
                        self.metrics
                            .perp_events_consumed
                            .fetch_add(consumed as u64, Ordering::Relaxed);
                    }
                    Err(err) => {
                        warn!(
                            "perp event consumer error perp_market={}: {err:#}",
                            perp_market_pk
                        );
                    }
                }
            }
        }
    }

    /// Reads the perp event queue, collects unique mango account keys
    /// referenced by the next `limit` events, and sends a single
    /// perp_consume_events instruction with those accounts as
    /// remaining_accounts. Returns the number of events consumed.
    async fn consume_perp_events_once(
        &self,
        group: Pubkey,
        perp_market: Pubkey,
        event_queue: Pubkey,
        limit: usize,
    ) -> Result<usize> {
        // Fetch event queue account
        let eq_account = self
            .rpc
            .get_account(&event_queue)
            .await
            .with_context(|| format!("fetch perp event queue {event_queue}"))?;
        let keyed = KeyedAccountSharedData::new(event_queue, eq_account.into());
        let eq = keyed
            .load::<EventQueue>()
            .with_context(|| format!("load EventQueue {event_queue}"))?;
        if eq.is_empty() {
            return Ok(0);
        }

        // Walk the next `limit` events to collect unique mango account keys.
        let mut needed_keys: Vec<Pubkey> = Vec::with_capacity(limit * 2);
        let mut events_to_consume = 0usize;
        for ev in eq.iter().take(limit) {
            let ev_type = match EventType::try_from(ev.event_type) {
                Ok(t) => t,
                Err(_) => break,
            };
            match ev_type {
                EventType::Fill => {
                    let fill: &FillEvent = anchor_lang::__private::bytemuck::cast_ref(ev);
                    if !needed_keys.contains(&fill.maker) {
                        needed_keys.push(fill.maker);
                    }
                    if !needed_keys.contains(&fill.taker) {
                        needed_keys.push(fill.taker);
                    }
                }
                EventType::Out => {
                    let out: &OutEvent = anchor_lang::__private::bytemuck::cast_ref(ev);
                    if !needed_keys.contains(&out.owner) {
                        needed_keys.push(out.owner);
                    }
                }
                EventType::Liquidate => {}
            }
            events_to_consume += 1;
        }

        if events_to_consume == 0 {
            return Ok(0);
        }

        // Build the perp_consume_events instruction. Account order:
        //   group, perp_market, event_queue, ..mango_accounts (remaining_accounts)
        let mut accounts = vec![
            AccountMeta::new_readonly(group, false),
            AccountMeta::new(perp_market, false),
            AccountMeta::new(event_queue, false),
        ];
        for key in &needed_keys {
            accounts.push(AccountMeta::new(*key, false));
        }
        let ix = Instruction {
            program_id: self.config.program_id,
            accounts,
            data: mango_v4::instruction::PerpConsumeEvents { limit }.data(),
        };

        // Build and send tx (payer-signed)
        let chain = self.blockhashes.snapshot().await;
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(400_000),
            ix,
        ];
        if self.config.executor_prioritization_fee > 0 {
            instructions.insert(
                0,
                ComputeBudgetInstruction::set_compute_unit_price(
                    self.config.executor_prioritization_fee,
                ),
            );
        }
        let message = MessageV0::try_compile(
            &self.config.payer.pubkey(),
            &instructions,
            &[],
            chain.blockhash,
        )?;
        let tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[self.config.payer.as_ref()],
        )?;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: Some(0),
            ..RpcSendTransactionConfig::default()
        };
        let sig = self
            .rpc
            .send_transaction_with_config(&tx, send_cfg)
            .await
            .with_context(|| "send perp_consume_events tx")?;
        debug!(
            "perp_consume_events sent sig={} attempted_events={}",
            sig, events_to_consume
        );
        Ok(events_to_consume)
    }

    async fn run_executor(self: Arc<Self>, executor: Arc<ExecutorState>) {
        info!(
            "execution engine executor enabled for group={}, queue={}, max_items={}, interval_ms={}, busy_interval_ms={}, head_lock_ms={}, head_refresh_ms={}, pending_timeout_ms={}, status_poll_ms={}, target_lane_fanout={}, head_scan_items={}, max_pending_txs={}, pipeline_max_per_head={}, same_head_send_interval_ms={}, no_lane_match_drop_batch_max={}, match_head_only={}, safe_speculative={}, optimistic_advance={}",
            executor.group,
            executor.execution_queue,
            self.config.executor_max_items,
            self.config.executor_interval_ms,
            self.config.executor_busy_interval_ms,
            self.config.executor_head_lock_ms,
            self.config.executor_head_refresh_ms,
            self.config.executor_pending_timeout_ms,
            self.config.executor_status_poll_ms,
            self.config.executor_target_lane_fanout,
            self.config.executor_head_scan_items,
            self.config.executor_max_pending_txs,
            self.config.executor_pipeline_max_per_head,
            self.config.executor_same_head_send_interval_ms,
            self.config.executor_no_lane_match_drop_batch_max,
            self.config.executor_match_head_only,
            self.config.executor_safe_speculative,
            self.config.executor_optimistic_advance,
        );

        loop {
            let sleep_ms = match self.execute_once(&executor).await {
                Ok(ExecuteLoopOutcome::Idle) => self.config.executor_interval_ms,
                Ok(ExecuteLoopOutcome::Busy | ExecuteLoopOutcome::Sent) => {
                    self.config.executor_busy_interval_ms
                }
                Err(err) => {
                    self.metrics.execute_errors.fetch_add(1, Ordering::Relaxed);
                    warn!("executor loop error: {err:?}");
                    self.config.executor_busy_interval_ms
                }
            };
            // Heartbeat after every iteration (success or err) so the
            // sampler-published executor_healthy gauge tracks loop liveness
            // rather than head movement.
            self.metrics.tick_executor(unix_timestamp_ms());
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }
    }

    async fn apply_executor_lane_failure(
        &self,
        executor: &Arc<ExecutorState>,
        lane_hash: [u8; 32],
        reason: &str,
    ) {
        let lane_key = bytes_to_hex(&lane_hash);
        let now_ms = unix_timestamp_ms();
        let mut should_backoff = false;
        let failure_count = {
            let mut failure_counts = executor.failure_counts.lock().await;
            let count = failure_counts.entry(lane_key.clone()).or_insert(0);
            *count = count.saturating_add(1);
            if *count >= self.config.executor_failure_threshold {
                *count = 0;
                should_backoff = true;
            }
            *count
        };

        if should_backoff {
            executor.backoff_until_ms.lock().await.insert(
                lane_key.clone(),
                now_ms.saturating_add(self.config.executor_failure_backoff_ms),
            );
            self.metrics
                .execute_lane_suppressed
                .fetch_add(1, Ordering::Relaxed);
            warn!(
                "executor lane backoff hash={} reason={} backoff_ms={}",
                lane_key, reason, self.config.executor_failure_backoff_ms
            );
        } else {
            debug!(
                "executor lane failure hash={} reason={} consecutive_failures={}",
                lane_key, reason, failure_count
            );
        }
    }

    async fn reconcile_pending_dispatches(
        &self,
        executor: &Arc<ExecutorState>,
        head: &QueueHead,
        now_ms: u64,
    ) -> Vec<PendingHeadDispatch> {
        let pending_snapshot = {
            let mut pending_dispatches = executor.pending_dispatches.lock().await;
            let mut retained = Vec::with_capacity(pending_dispatches.len());
            for pending in pending_dispatches.drain(..) {
                let pending_age_ms = now_ms.saturating_sub(pending.sent_at_ms);
                if pending.targeted && pending.sequence < head.next_sequence {
                    continue;
                }
                if pending_age_ms >= self.config.executor_pending_timeout_ms {
                    continue;
                }
                retained.push(pending);
            }
            *pending_dispatches = retained.clone();
            retained
        };

        let mut retained = pending_snapshot;
        let mut polls = Vec::new();
        for (index, pending) in retained.iter_mut().enumerate() {
            if now_ms.saturating_sub(pending.last_status_check_ms)
                < self.config.executor_status_poll_ms
            {
                continue;
            }
            pending.last_status_check_ms = now_ms;
            polls.push((index, pending.signature));
        }

        if polls.is_empty() {
            return retained;
        }

        let signatures: Vec<Signature> = polls.iter().map(|(_, signature)| *signature).collect();
        match self.rpc.get_signature_statuses(&signatures).await {
            Ok(response) => {
                let mut failed_indices = HashSet::new();
                for ((index, signature), status) in
                    polls.into_iter().zip(response.value.into_iter())
                {
                    match status {
                        Some(status) if status.err.is_none() => {
                            if !retained[index].no_advance_recorded {
                                retained[index].no_advance_recorded = true;
                                self.metrics
                                    .execute_confirmed_no_advance
                                    .fetch_add(1, Ordering::Relaxed);
                                debug!(
                                    "executor tx confirmed but queue head unchanged sequence={} mode={:?} sig={}",
                                    retained[index].sequence,
                                    retained[index].dispatch_kind,
                                    signature
                                );
                                if retained[index].dispatch_kind == PendingDispatchKind::Execute
                                    && retained[index].targeted
                                {
                                    // Treat confirmed-no-advance targeted executes as a
                                    // persistent failure; if the head doesn't move after N
                                    // confirmed txs, the lane accounts likely have a
                                    // runtime flag mismatch.
                                    self.maybe_auto_drop_sequence_after_failure_threshold(
                                        executor,
                                        retained[index].sequence,
                                        "confirmed_no_advance",
                                    )
                                    .await;
                                }
                            }
                        }
                        Some(status) => {
                            // Detect ProgramFailedToComplete (heap overflow)
                            // and adaptively reduce max_items to prevent
                            // repeated failures that stall the queue.
                            if matches!(
                                status.err.as_ref(),
                                Some(TransactionError::InstructionError(
                                    _,
                                    InstructionError::ProgramFailedToComplete
                                ))
                            ) {
                                executor.on_heap_overflow(self.config.executor_max_items);
                            }
                            let auto_recovered = match status.err.as_ref() {
                                Some(err) => {
                                    self.maybe_auto_drop_terminal_head(
                                        executor,
                                        &retained[index],
                                        err,
                                    )
                                    .await
                                }
                                None => false,
                            };
                            warn!(
                                "executor tx failed sequence={} sig={} err={:?}",
                                retained[index].sequence, signature, status.err
                            );
                            // The tx landed but the program reported an error.
                            // Count it as an executed-but-failed sample.
                            self.metrics.record_executed_failed();
                            if !auto_recovered {
                                self.apply_executor_lane_failure(
                                    executor,
                                    retained[index].lane_hash,
                                    "signature_status_failed",
                                )
                                .await;
                                self.maybe_auto_drop_sequence_after_failure_threshold(
                                    executor,
                                    retained[index].sequence,
                                    "confirmed tx failures",
                                )
                                .await;
                            } else {
                                // Auto-drop succeeded; clear the failure counter
                                executor
                                    .sequence_failure_counts
                                    .lock()
                                    .await
                                    .remove(&retained[index].sequence);
                            }
                            failed_indices.insert(index);
                        }
                        None => {}
                    }
                }
                if !failed_indices.is_empty() {
                    retained = retained
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, pending)| {
                            (!failed_indices.contains(&index)).then_some(pending)
                        })
                        .collect();
                    executor.last_inspect_ms.store(0, Ordering::Relaxed);
                    *executor.cached_head.lock().await = None;
                }
            }
            Err(err) => {
                debug!("executor signature status poll failed: {err:?}");
                // Status poll failure means we cannot confirm whether the tx
                // succeeded or failed. Treat repeated poll failures exactly like
                // repeated confirmed failures for queue-unblock purposes.
                let _ = self
                    .maybe_auto_drop_sequence_after_failure_threshold(
                        executor,
                        head.next_sequence,
                        "status poll errors",
                    )
                    .await;
            }
        }

        *executor.pending_dispatches.lock().await = retained.clone();
        retained
    }

    async fn execute_once(&self, executor: &Arc<ExecutorState>) -> Result<ExecuteLoopOutcome> {
        let now_ms = unix_timestamp_ms();
        let last_inspect = executor.last_inspect_ms.load(Ordering::Relaxed);
        let inspect_interval_ms = self
            .config
            .executor_head_refresh_ms
            .max(self.config.executor_head_lock_ms.max(2));

        let effective_inspect_ms = if self.config.executor_optimistic_advance {
            self.config.executor_head_lock_ms.max(2)
        } else {
            inspect_interval_ms
        };
        let planner_lane_fanout = self.config.executor_target_lane_fanout.max(1).min(20);
        let mut near_head_lane_entries = Vec::new();
        let mut near_head_exact_hashes = Vec::new();
        let head = if now_ms.saturating_sub(last_inspect) >= effective_inspect_ms {
            let accounts = self.rpc.get_account(&executor.execution_queue).await?;
            let head = inspect_queue_head(&accounts.data);
            near_head_lane_entries = inspect_near_head_lane_entries(
                &accounts.data,
                &head,
                self.config.executor_head_scan_items,
                planner_lane_fanout,
            );
            near_head_exact_hashes = near_head_lane_entries
                .iter()
                .map(|(_, hash)| *hash)
                .collect();
            let previous_head = *executor.cached_head.lock().await;
            executor.update_queue_count(head.count);
            executor.update_queue_next_sequence(head.next_sequence);
            executor.update_queue_max_seen_sequence(head.max_seen_sequence);
            executor.update_queue_head_available(head.head_accounts_hash.is_some());
            executor.last_inspect_ms.store(now_ms, Ordering::Relaxed);
            *executor.cached_head.lock().await = Some(head);

            if let Some(prev) = previous_head {
                if head.next_sequence > prev.next_sequence {
                    // Head moved — reset no-lane-match tracker
                    *executor.no_lane_match_since.lock().await = None;
                    let advanced = head.next_sequence.saturating_sub(prev.next_sequence);
                    self.metrics
                        .execute_head_advanced
                        .fetch_add(1, Ordering::Relaxed);
                    self.metrics
                        .execute_head_advance_items
                        .fetch_add(advanced, Ordering::Relaxed);
                    // Each on-chain head advance corresponds to that many
                    // execution-queue items that successfully landed.
                    self.metrics.record_executed(advanced);
                    // Track successful advancement for adaptive max_items ramp-up
                    executor.on_successful_advance(self.config.executor_max_items);
                    let last_logged = executor.last_progress_log_sequence.load(Ordering::Relaxed);
                    if head.next_sequence > last_logged {
                        executor
                            .last_progress_log_sequence
                            .store(head.next_sequence, Ordering::Relaxed);
                        info!(
                            "executor head advanced from={} to={} delta={} queue_count={} prev_queue_count={}",
                            prev.next_sequence,
                            head.next_sequence,
                            advanced,
                            head.count,
                            prev.count,
                        );
                    }
                }
            }

            // Clean up stale per-sequence failure counts: remove entries for
            // sequences that the on-chain queue has already advanced past.
            {
                let mut seq_failures = executor.sequence_failure_counts.lock().await;
                seq_failures.retain(|&seq, _| seq >= head.next_sequence);
            }

            if head.count == 0 {
                executor.pending_dispatches.lock().await.clear();
                return Ok(ExecuteLoopOutcome::Idle);
            }
            head
        } else {
            // Use cached head
            match *executor.cached_head.lock().await {
                Some(head) if head.count > 0 => head,
                _ => return Ok(ExecuteLoopOutcome::Idle),
            }
        };

        let pending_snapshot = self
            .reconcile_pending_dispatches(executor, &head, now_ms)
            .await;
        let same_head_pending_count = pending_snapshot
            .iter()
            .filter(|pending| {
                pending.sequence == head.next_sequence
                    && pending.accounts_hash == head.head_accounts_hash
            })
            .count();
        let latest_same_head_send_ms = pending_snapshot
            .iter()
            .filter(|pending| {
                pending.sequence == head.next_sequence
                    && pending.accounts_hash == head.head_accounts_hash
            })
            .map(|pending| pending.sent_at_ms)
            .max()
            .unwrap_or(0);
        // In optimistic mode we still only allow one in-flight execute per
        // observed head. The execute instruction always starts from the
        // current on-chain head, so broadcasting multiple txs before the head
        // changes just creates same-head duplicates and confirmed-no-advance
        // waste. The "pipeline" behavior we want is to re-inspect
        // immediately and fire on the next observed head, not to flood the
        // current one.
        if self.config.executor_optimistic_advance {
            if pending_snapshot.len() >= self.config.executor_max_pending_txs {
                self.metrics
                    .execute_send_suppressed_pending
                    .fetch_add(1, Ordering::Relaxed);
                return Ok(ExecuteLoopOutcome::Busy);
            }
        } else {
            let _can_pipeline_same_head = same_head_pending_count
                < self.config.executor_pipeline_max_per_head
                && pending_snapshot.len() < self.config.executor_max_pending_txs
                && now_ms.saturating_sub(latest_same_head_send_ms)
                    >= self.config.executor_same_head_send_interval_ms;
            if same_head_pending_count >= self.config.executor_pipeline_max_per_head
                && pending_snapshot.len() >= self.config.executor_max_pending_txs
            {
                self.metrics
                    .execute_send_suppressed_pending
                    .fetch_add(1, Ordering::Relaxed);
                return Ok(ExecuteLoopOutcome::Busy);
            }
            if same_head_pending_count >= self.config.executor_pipeline_max_per_head {
                executor.last_inspect_ms.store(0, Ordering::Relaxed);
                return Ok(ExecuteLoopOutcome::Busy);
            }
        }

        let mut lanes = executor.lanes_snapshot().await;
        if lanes.is_empty() {
            let _ = self.refresh_dynamic_lanes_from_event_log(executor).await;
            lanes = executor.lanes_snapshot().await;
        }
        if lanes.is_empty() {
            debug!(
                "executor skipped: queue_count={} next_sequence={} reason=no_lanes",
                head.count, head.next_sequence,
            );
            return Ok(ExecuteLoopOutcome::Busy);
        }

        let gap_skip_mode = self.config.executor_match_head_only
            && self.config.executor_safe_speculative
            && matches!(
                head.reason,
                "ctm_gap_or_empty_slot" | "ctm_sequence_mismatch"
            );

        let (mut candidate_lanes, mut speculative_mode, mut planned_sequence) = if self
            .config
            .executor_match_head_only
        {
            match head.head_accounts_hash {
                Some(hash) => {
                    let mut ordered_hashes = if near_head_exact_hashes.is_empty() {
                        vec![hash]
                    } else {
                        near_head_exact_hashes.clone()
                    };
                    if ordered_hashes.first().copied() != Some(hash) {
                        ordered_hashes.retain(|lane_hash| *lane_hash != hash);
                        ordered_hashes.insert(0, hash);
                    }
                    ordered_hashes.truncate(planner_lane_fanout);

                    let mut lanes_by_hash: HashMap<[u8; 32], Lane> = lanes
                        .iter()
                        .cloned()
                        .map(|lane| (lane.hash, lane))
                        .collect();
                    let mut matched: Vec<Lane> = ordered_hashes
                        .iter()
                        .filter_map(|lane_hash| lanes_by_hash.remove(lane_hash))
                        .collect();
                    let mut matched_hashes: HashSet<[u8; 32]> =
                        matched.iter().map(|lane| lane.hash).collect();
                    if !matched_hashes.contains(&hash) || matched.len() < ordered_hashes.len() {
                        let _ = self.refresh_dynamic_lanes_from_event_log(executor).await;
                        lanes = executor.lanes_snapshot().await;
                        lanes_by_hash = lanes
                            .iter()
                            .cloned()
                            .map(|lane| (lane.hash, lane))
                            .collect();
                        matched = ordered_hashes
                            .iter()
                            .filter_map(|lane_hash| lanes_by_hash.remove(lane_hash))
                            .collect();
                        matched_hashes = matched.iter().map(|lane| lane.hash).collect();
                    }
                    if !matched_hashes.contains(&hash) {
                        self.metrics
                            .execute_no_lane_match
                            .fetch_add(1, Ordering::Relaxed);

                        // Auto-drop heads stuck with no lane match for too long.
                        // Uses wall-clock ms; 2 slots ≈ 800ms, we use slot_count * 400ms.
                        let drop_slots = self.config.executor_no_lane_match_drop_slots;
                        if drop_slots > 0 {
                            let now_ms = unix_timestamp_ms();
                            let threshold_ms = drop_slots * 400;
                            let mut tracker = executor.no_lane_match_since.lock().await;
                            let should_drop = match *tracker {
                                Some((seq, first_ms)) if seq == head.next_sequence => {
                                    now_ms.saturating_sub(first_ms) >= threshold_ms
                                }
                                _ => {
                                    *tracker = Some((head.next_sequence, now_ms));
                                    false
                                }
                            };
                            drop(tracker);

                            if should_drop {
                                warn!(
                                    "executor auto-dropping no_lane_match head sequence={} head_hash={} after {}ms threshold",
                                    head.next_sequence,
                                    bytes_to_hex(&hash),
                                    threshold_ms,
                                );
                                match self
                                    .drop_ctm_head_with_admin_tx(
                                        executor,
                                        head.next_sequence,
                                        "no_lane_match_stale",
                                    )
                                    .await
                                {
                                    Ok(sig) => {
                                        info!(
                                            "executor no_lane_match admin-drop succeeded sequence={} tx={}",
                                            head.next_sequence, sig
                                        );
                                        // Reset tracker — head will change on next inspect
                                        *executor.no_lane_match_since.lock().await = None;
                                        // Force re-inspect on next iteration
                                        executor.last_inspect_ms.store(0, Ordering::Relaxed);
                                    }
                                    Err(err) => {
                                        warn!(
                                            "executor no_lane_match admin-drop failed sequence={} err={err:?}",
                                            head.next_sequence
                                        );
                                    }
                                }
                                return Ok(ExecuteLoopOutcome::Busy);
                            }
                        }

                        debug!(
                            "executor skipped: queue_count={} next_sequence={} reason=no_lane_match head_hash={}",
                            head.count,
                            head.next_sequence,
                            bytes_to_hex(&hash),
                        );
                        return Ok(ExecuteLoopOutcome::Busy);
                    }

                    if matched.len() > 1 {
                        debug!(
                            "executor near-head lane plan sequence={} scan_items={} candidate_hashes={} matched_lanes={}",
                            head.next_sequence,
                            self.config.executor_head_scan_items,
                            ordered_hashes.len(),
                            matched.len(),
                        );
                    }
                    (matched, false, head.next_sequence)
                }
                None => {
                    self.metrics
                        .execute_head_missing
                        .fetch_add(1, Ordering::Relaxed);
                    let speculative_lanes = if gap_skip_mode {
                        select_gap_speculative_lanes(lanes.clone())
                    } else {
                        Vec::new()
                    };
                    if !speculative_lanes.is_empty() {
                        debug!(
                            "executor head hash unavailable: queue_count={} next_sequence={} max_seen_sequence={} {} fallback=gap_speculative lanes={}",
                            head.count,
                            head.next_sequence,
                            head.max_seen_sequence,
                            head.blocked_reason(),
                            speculative_lanes.len(),
                        );
                        (speculative_lanes, true, head.next_sequence)
                    } else {
                        self.metrics
                            .execute_head_blocked
                            .fetch_add(1, Ordering::Relaxed);
                        debug!(
                            "executor blocked: queue_count={} next_sequence={} max_seen_sequence={} {}",
                            head.count,
                            head.next_sequence,
                            head.max_seen_sequence,
                            head.blocked_reason(),
                        );
                        return Ok(ExecuteLoopOutcome::Busy);
                    }
                }
            }
        } else {
            (lanes.clone(), true, head.next_sequence)
        };

        if self.config.executor_optimistic_advance
            && same_head_pending_count > 0
            && !speculative_mode
            && head.reason == "ctm_pending"
        {
            if let Some(current_hash) = head.head_accounts_hash {
                if let Some((successor_sequence, _)) =
                    near_head_lane_entries
                        .iter()
                        .copied()
                        .find(|(sequence, hash)| {
                            *sequence > head.next_sequence && *hash != current_hash
                        })
                {
                    let speculative_already_pending = pending_snapshot
                        .iter()
                        .any(|pending| !pending.targeted && pending.sequence == successor_sequence);
                    if !speculative_already_pending {
                        let successor_hashes: Vec<[u8; 32]> = near_head_lane_entries
                            .iter()
                            .copied()
                            .filter(|(sequence, _)| *sequence >= successor_sequence)
                            .map(|(_, hash)| hash)
                            .take(planner_lane_fanout)
                            .collect();
                        if !successor_hashes.is_empty() {
                            let mut lanes_by_hash: HashMap<[u8; 32], Lane> = lanes
                                .iter()
                                .cloned()
                                .map(|lane| (lane.hash, lane))
                                .collect();
                            let successor_lanes: Vec<Lane> = successor_hashes
                                .iter()
                                .filter_map(|lane_hash| lanes_by_hash.remove(lane_hash))
                                .collect();
                            if !successor_lanes.is_empty() {
                                debug!(
                                    "executor speculative successor plan base_sequence={} successor_sequence={} hashes={} lanes={}",
                                    head.next_sequence,
                                    successor_sequence,
                                    successor_hashes.len(),
                                    successor_lanes.len(),
                                );
                                candidate_lanes = successor_lanes;
                                speculative_mode = true;
                                planned_sequence = successor_sequence;
                            }
                        }
                    }
                }
            }
        }

        if self.config.executor_optimistic_advance
            && same_head_pending_count > 0
            && !speculative_mode
        {
            executor.last_inspect_ms.store(0, Ordering::Relaxed);
            return Ok(ExecuteLoopOutcome::Busy);
        }

        if pending_snapshot.len() >= self.config.executor_max_pending_txs {
            return Ok(ExecuteLoopOutcome::Busy);
        }

        let backoff_snapshot = { executor.backoff_until_ms.lock().await.clone() };

        if gap_skip_mode {
            let Some(gap_skip_lane) = candidate_lanes.iter().find(|lane| {
                let lane_key = bytes_to_hex(&lane.hash);
                backoff_snapshot.get(&lane_key).copied().unwrap_or(0) <= now_ms
            }) else {
                debug!(
                    "executor gap-skip suppressed: queue_count={} next_sequence={} reason=no_available_speculative_lane",
                    head.count, head.next_sequence,
                );
                return Ok(ExecuteLoopOutcome::Busy);
            };
            self.metrics
                .execute_attempts
                .fetch_add(1, Ordering::Relaxed);
            self.metrics
                .execute_targeted
                .fetch_add(1, Ordering::Relaxed);
            match self
                .build_gap_skip_tx(executor, gap_skip_lane, head.next_sequence)
                .await
            {
                Ok((tx, send_cfg)) => match self
                    .rpc
                    .send_transaction_with_config(&tx, send_cfg)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))
                {
                    Ok(signature) => {
                        self.metrics.execute_sent.fetch_add(1, Ordering::Relaxed);
                        let mut dispatches = executor.pending_dispatches.lock().await;
                        dispatches.push(PendingHeadDispatch {
                            sequence: head.next_sequence,
                            accounts_hash: None,
                            lane_hash: [0u8; 32],
                            dispatch_kind: PendingDispatchKind::GapSkip,
                            sent_at_ms: now_ms,
                            last_status_check_ms: now_ms,
                            no_advance_recorded: false,
                            targeted: true,
                            signature: signature.clone(),
                        });
                        info!(
                            "executor sent gap-skip sequence={} queue_count={} lane={} tx={}",
                            head.next_sequence, head.count, gap_skip_lane.name, signature,
                        );
                        return Ok(ExecuteLoopOutcome::Sent);
                    }
                    Err(err) => {
                        warn!(
                            "executor gap-skip send failed sequence={} err={err:?}",
                            head.next_sequence
                        );
                        return Ok(ExecuteLoopOutcome::Busy);
                    }
                },
                Err(err) => {
                    warn!(
                        "executor gap-skip build failed sequence={} err={err:?}",
                        head.next_sequence
                    );
                    return Ok(ExecuteLoopOutcome::Busy);
                }
            }
        }

        // Collect eligible lanes (de-dup by hash, skip backed-off lanes).
        // execute_multi requires a fixed accounts_per_lane width, so only lanes
        // compatible with the first selected lane can be batched together.
        let target_lane_fanout = planner_lane_fanout;
        let (eligible_lanes, unique_hashes, dropped_for_layout) = select_layout_compatible_lanes(
            candidate_lanes,
            &backoff_snapshot,
            now_ms,
            target_lane_fanout,
        );

        if eligible_lanes.is_empty() {
            return Ok(ExecuteLoopOutcome::Busy);
        }

        if !speculative_mode {
            if let Some(head_hash) = head.head_accounts_hash {
                if eligible_lanes.first().map(|lane| lane.hash) != Some(head_hash) {
                    self.metrics
                        .execute_lane_suppressed
                        .fetch_add(1, Ordering::Relaxed);
                    debug!(
                        "executor targeted lane suppressed sequence={} reason=head_lane_unavailable_after_filtering head_hash={} queue_count={}",
                        planned_sequence,
                        bytes_to_hex(&head_hash),
                        head.count,
                    );
                    return Ok(ExecuteLoopOutcome::Busy);
                }
            }
        }

        if dropped_for_layout > 0 {
            debug!(
                "executor multi-lane layout filtered sequence={} retained_lanes={} dropped_lanes={} accounts_per_lane={} speculative={}",
                planned_sequence,
                eligible_lanes.len(),
                dropped_for_layout,
                lane_account_width(&eligible_lanes[0]),
                speculative_mode,
            );
        }

        if eligible_lanes.len() > 1 {
            info!(
                "executor multi-lane eligible_lanes={} unique_hashes={}",
                eligible_lanes.len(),
                unique_hashes,
            );
        }

        // Check pending budget
        if pending_snapshot.len() >= self.config.executor_max_pending_txs {
            return Ok(ExecuteLoopOutcome::Busy);
        }

        // Fast-path: if the head's mango account is blocked (perp order slots
        // full), skip the execute tx entirely and batch-drop items directly.
        // The mango account is remaining_accounts[1] in the matched lane.
        // The on-chain drop_ctm instruction doesn't check expiry — it drops
        // whatever the admin tells it to.  We batch up to 8 consecutive
        // pending items per tx for high-throughput draining.
        {
            let blocked = self.blocked_mango_accounts.lock().await;
            if !blocked.is_empty() {
                if let Some(lane) = eligible_lanes.first() {
                    if lane.remaining_accounts.len() > 1 {
                        let mango_acct = lane.remaining_accounts[1].pubkey;
                        if blocked.contains_key(&mango_acct) {
                            drop(blocked);
                            // Read queue state and build batch of consecutive pending items
                            let batch_max = self.config.executor_expired_head_drop_batch_max.max(8);
                            match self.rpc.get_account(&executor.execution_queue).await {
                                Ok(queue_account) => {
                                    let qs = inspect_queue_admin_state(&queue_account.data);
                                    if qs.head.reason == "ctm_pending"
                                        && qs.head.next_sequence == head.next_sequence
                                    {
                                        // Collect consecutive pending sequences
                                        let mut seqs = Vec::new();
                                        let start = qs.head.next_sequence;
                                        let end = qs
                                            .head
                                            .max_seen_sequence
                                            .saturating_add(1)
                                            .min(start + batch_max as u64);
                                        for seq in start..end {
                                            let off = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
                                                + (seq as usize % EXECUTION_QUEUE_CTM_CAPACITY)
                                                    * EXECUTION_QUEUE_ITEM_SIZE;
                                            if off + EXECUTION_QUEUE_ITEM_SIZE
                                                > queue_account.data.len()
                                            {
                                                break;
                                            }
                                            let slot_seq = u64::from_le_bytes(
                                                queue_account.data[off
                                                    + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                                                    ..off
                                                        + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                                                        + 8]
                                                    .try_into()
                                                    .unwrap_or([0; 8]),
                                            );
                                            let slot_status = queue_account.data
                                                [off + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
                                            if slot_status != 1 || slot_seq != seq {
                                                break;
                                            }
                                            seqs.push(seq);
                                        }
                                        if !seqs.is_empty() {
                                            let count = seqs.len();
                                            let mut ixs =
                                                vec![build_execution_queue_configure_instruction(
                                                    self.config.program_id,
                                                    executor.group,
                                                    executor.execution_queue,
                                                    self.config.executor_admin.pubkey(),
                                                    &qs,
                                                    true,
                                                )];
                                            for seq in &seqs {
                                                ixs.push(
                                                    build_execution_queue_drop_ctm_instruction(
                                                        self.config.program_id,
                                                        executor.group,
                                                        executor.execution_queue,
                                                        self.config.executor_admin.pubkey(),
                                                        *seq,
                                                    ),
                                                );
                                            }
                                            ixs.push(build_execution_queue_configure_instruction(
                                                self.config.program_id,
                                                executor.group,
                                                executor.execution_queue,
                                                self.config.executor_admin.pubkey(),
                                                &qs,
                                                false,
                                            ));
                                            let chain = self.blockhashes.snapshot().await;
                                            if let Ok(msg) = MessageV0::try_compile(
                                                &self.config.executor_admin.pubkey(),
                                                &ixs,
                                                &[],
                                                chain.blockhash,
                                            ) {
                                                if let Ok(tx) = VersionedTransaction::try_new(
                                                    solana_sdk::message::VersionedMessage::V0(msg),
                                                    &[self.config.executor_admin.as_ref()],
                                                ) {
                                                    match self
                                                        .rpc
                                                        .send_transaction_with_config(
                                                            &tx,
                                                            RpcSendTransactionConfig {
                                                                skip_preflight: true,
                                                                ..Default::default()
                                                            },
                                                        )
                                                        .await
                                                    {
                                                        Ok(sig) => {
                                                            info!(
                                                                "executor fast-drop batch={} blocked_account={} seq={}..{} tx={}",
                                                                count, mango_acct, seqs[0],
                                                                seqs[seqs.len()-1], sig,
                                                            );
                                                        }
                                                        Err(err) => {
                                                            debug!("executor fast-drop send failed: {err:?}");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(err) => {
                                    debug!("executor fast-drop queue read failed: {err:?}");
                                }
                            }
                            executor.last_inspect_ms.store(0, Ordering::Relaxed);
                            return Ok(ExecuteLoopOutcome::Sent);
                        }
                    }
                }
            }
        }

        // Build and send ONE multi-lane execute tx
        self.metrics
            .execute_attempts
            .fetch_add(1, Ordering::Relaxed);
        if speculative_mode {
            self.metrics
                .execute_speculative
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics
                .execute_targeted
                .fetch_add(1, Ordering::Relaxed);
        }

        let requested_lane_count = eligible_lanes.len();

        // Optimistic advance mode: send transaction asynchronously (fire-and-forget)
        // and immediately return Sent so the next loop iteration can build the next
        // tx without waiting for the RPC send round-trip (~200ms).  Confirmation
        // happens via the normal pending_dispatches reconciliation loop.
        if self.config.executor_optimistic_advance {
            let metrics = self.metrics.clone();
            let pending = executor.pending_dispatches.clone();
            let executor_clone = executor.clone();
            let sequence = planned_sequence;
            let accounts_hash = if speculative_mode {
                eligible_lanes.first().map(|lane| lane.hash)
            } else {
                head.head_accounts_hash
            };
            let lane_hash = eligible_lanes[0].hash;
            let is_speculative = speculative_mode;
            let is_pipeline = same_head_pending_count > 0;
            let logged_same_head_pending = if speculative_mode {
                same_head_pending_count
            } else {
                same_head_pending_count + 1
            };
            let self_ref = self.clone();
            let lane_hash_for_failure = eligible_lanes[0].hash;
            let lanes_for_send = eligible_lanes.clone();

            tokio::spawn(async move {
                match self_ref
                    .send_execute_with_lane_reduction(
                        &lanes_for_send,
                        &executor_clone,
                        planned_sequence,
                    )
                    .await
                {
                    Ok((signature, sent_lane_count)) => {
                        metrics.execute_sent.fetch_add(1, Ordering::Relaxed);
                        if is_pipeline {
                            metrics
                                .execute_pipeline_sent
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        let mut dispatches = pending.lock().await;
                        dispatches.push(PendingHeadDispatch {
                            sequence,
                            accounts_hash,
                            lane_hash,
                            dispatch_kind: PendingDispatchKind::Execute,
                            sent_at_ms: unix_timestamp_ms(),
                            last_status_check_ms: unix_timestamp_ms(),
                            no_advance_recorded: false,
                            targeted: !is_speculative,
                            signature: signature.clone(),
                        });
                        if sent_lane_count < lanes_for_send.len() {
                            info!(
                                "executor multi-lane fanout reduced sequence={} requested_lanes={} sent_lanes={} speculative={}",
                                sequence,
                                lanes_for_send.len(),
                                sent_lane_count,
                                is_speculative,
                            );
                        }
                    }
                    Err(err) => {
                        let err = err.into_err();
                        let failure_class = classify_lane_failure(&err);
                        if failure_class.is_deterministic() {
                            self_ref
                                .apply_executor_lane_failure(
                                    &executor_clone,
                                    lane_hash_for_failure,
                                    failure_class.as_str(),
                                )
                                .await;
                        }
                        warn!(
                            "executor async send failed class={} err={err:?}",
                            failure_class.as_str(),
                        );
                    }
                }
            });

            self.metrics
                .execute_attempts
                .fetch_sub(0, Ordering::Relaxed); // noop to keep counters consistent
            info!(
                "executor fire-and-forget multi-lane lanes={} sequence={} queue_count={} pending_same_head={} speculative={}",
                requested_lane_count,
                planned_sequence,
                head.count,
                logged_same_head_pending,
                speculative_mode,
            );
            // Force immediate re-inspect to pick up next head
            executor.last_inspect_ms.store(0, Ordering::Relaxed);
            return Ok(ExecuteLoopOutcome::Sent);
        }

        let send_result = self
            .send_execute_with_lane_reduction(&eligible_lanes, executor, planned_sequence)
            .await;

        match send_result {
            Ok((signature, sent_lane_count)) => {
                self.metrics.execute_sent.fetch_add(1, Ordering::Relaxed);
                if same_head_pending_count > 0 {
                    self.metrics
                        .execute_pipeline_sent
                        .fetch_add(1, Ordering::Relaxed);
                }
                let mut dispatches = executor.pending_dispatches.lock().await;
                dispatches.push(PendingHeadDispatch {
                    sequence: planned_sequence,
                    accounts_hash: if speculative_mode {
                        eligible_lanes.first().map(|lane| lane.hash)
                    } else {
                        head.head_accounts_hash
                    },
                    lane_hash: eligible_lanes[0].hash,
                    dispatch_kind: PendingDispatchKind::Execute,
                    sent_at_ms: now_ms,
                    last_status_check_ms: now_ms,
                    no_advance_recorded: false,
                    targeted: !speculative_mode,
                    signature: signature.clone(),
                });
                info!(
                    "executor sent multi-lane lanes={} sequence={} queue_count={} pending_total={} pending_same_head={} speculative={} tx={}",
                    sent_lane_count,
                    planned_sequence,
                    head.count,
                    pending_snapshot.len() + 1,
                    if speculative_mode {
                        same_head_pending_count
                    } else {
                        same_head_pending_count + 1
                    },
                    speculative_mode,
                    signature,
                );
                if sent_lane_count < requested_lane_count {
                    info!(
                        "executor multi-lane fanout reduced sequence={} requested_lanes={} sent_lanes={} speculative={}",
                        planned_sequence,
                        requested_lane_count,
                        sent_lane_count,
                        speculative_mode,
                    );
                }

                return Ok(ExecuteLoopOutcome::Sent);
            }
            Err(err) => {
                let err = err.into_err();
                let failure_class = classify_lane_failure(&err);
                if failure_class.is_deterministic() {
                    self.apply_executor_lane_failure(
                        executor,
                        eligible_lanes[0].hash,
                        failure_class.as_str(),
                    )
                    .await;
                }
                warn!(
                    "executor multi-lane send failed class={} err={err:?}",
                    failure_class.as_str(),
                );
            }
        }

        Ok(ExecuteLoopOutcome::Busy)
    }

    async fn send_execute(
        &self,
        lane: &Lane,
        executor: &Arc<ExecutorState>,
        head_sequence: u64,
    ) -> Result<Signature> {
        let (tx, send_cfg) = self.build_execute_tx(lane, executor, head_sequence).await?;
        let signature = self.rpc.send_transaction_with_config(&tx, send_cfg).await?;
        Ok(signature)
    }

    async fn send_execute_with_lane_reduction(
        &self,
        lanes: &[Lane],
        executor: &Arc<ExecutorState>,
        head_sequence: u64,
    ) -> std::result::Result<(Signature, usize), ExecuteDispatchError> {
        let mut lane_count = lanes.len().max(1);
        while lane_count > 0 {
            let selected_lanes = &lanes[..lane_count];
            match self
                .send_execute_attempt(selected_lanes, executor, head_sequence)
                .await
            {
                Ok(signature) => return Ok((signature, lane_count)),
                Err(err) if lane_count > 1 && is_transaction_too_large_error(err.err()) => {
                    warn!(
                        "executor multi-lane tx oversized sequence={} lanes={} stage={} err={:?}; retrying with fewer lanes",
                        head_sequence,
                        lane_count,
                        if err.is_build() { "build" } else { "send" },
                        err.err(),
                    );
                    lane_count -= 1;
                }
                Err(err) if lane_count > 1 && err.is_build() => {
                    warn!(
                        "executor multi-lane build failed sequence={} lanes={} err={:?}; falling back to single lane",
                        head_sequence,
                        lane_count,
                        err.err(),
                    );
                    lane_count = 1;
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("lane_count starts at >= 1 and only exits via return");
    }

    async fn send_execute_attempt(
        &self,
        lanes: &[Lane],
        executor: &Arc<ExecutorState>,
        head_sequence: u64,
    ) -> std::result::Result<Signature, ExecuteDispatchError> {
        let (tx, send_cfg) = if lanes.len() > 1 {
            self.build_execute_multi_tx(lanes, executor, head_sequence)
                .await
                .map_err(ExecuteDispatchError::Build)?
        } else {
            self.build_execute_tx(&lanes[0], executor, head_sequence)
                .await
                .map_err(ExecuteDispatchError::Build)?
        };

        if let Some(secondary) = &self.secondary_rpc {
            let tx_clone = tx.clone();
            let cfg_clone = send_cfg;
            let sec = secondary.clone();
            tokio::spawn(async move {
                let _ = sec.send_transaction_with_config(&tx_clone, cfg_clone).await;
            });
        }

        self.rpc
            .send_transaction_with_config(&tx, send_cfg)
            .await
            .map_err(|err| ExecuteDispatchError::Send(anyhow!("{err}")))
    }

    async fn build_execute_tx(
        &self,
        lane: &Lane,
        executor: &Arc<ExecutorState>,
        head_sequence: u64,
    ) -> Result<(VersionedTransaction, RpcSendTransactionConfig)> {
        let chain = self.blockhashes.snapshot().await;
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            build_execute_head_memo_instruction(
                head_sequence,
                self.execute_nonce.fetch_add(1, Ordering::Relaxed),
            ),
            build_execute_instruction(
                self.config.program_id,
                executor.group,
                executor.execution_queue,
                &lane.remaining_accounts,
                executor.effective_max_items(self.config.executor_max_items),
            ),
        ];
        if self.config.executor_prioritization_fee > 0 {
            instructions.insert(
                0,
                ComputeBudgetInstruction::set_compute_unit_price(
                    self.config.executor_prioritization_fee,
                ),
            );
        }
        let message = MessageV0::try_compile(
            &self.config.payer.pubkey(),
            &instructions,
            &[],
            chain.blockhash,
        )?;
        let tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[self.config.payer.as_ref()],
        )?;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.executor_skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: self.config.executor_rpc_max_retries,
            ..RpcSendTransactionConfig::default()
        };
        Ok((tx, send_cfg))
    }

    async fn build_execute_multi_tx(
        &self,
        lanes: &[Lane],
        executor: &Arc<ExecutorState>,
        head_sequence: u64,
    ) -> Result<(VersionedTransaction, RpcSendTransactionConfig)> {
        let accounts_per_lane = shared_lane_account_width(lanes)? as u16;
        let chain = self.blockhashes.snapshot().await;
        let lane_accounts: Vec<Vec<AccountMeta>> =
            lanes.iter().map(|l| l.remaining_accounts.clone()).collect();
        // Pass the pre-computed lane hashes (with original flags, not runtime-OR'd)
        let lane_hashes: Vec<[u8; 32]> = lanes.iter().map(|l| l.hash).collect();
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            build_execute_head_memo_instruction(
                head_sequence,
                self.execute_nonce.fetch_add(1, Ordering::Relaxed),
            ),
            build_execute_multi_instruction(
                self.config.program_id,
                executor.group,
                executor.execution_queue,
                &lane_accounts,
                accounts_per_lane,
                lane_hashes,
                executor.effective_max_items(self.config.executor_max_items),
            ),
        ];
        if self.config.executor_prioritization_fee > 0 {
            instructions.insert(
                0,
                ComputeBudgetInstruction::set_compute_unit_price(
                    self.config.executor_prioritization_fee,
                ),
            );
        }
        let message = MessageV0::try_compile(
            &self.config.payer.pubkey(),
            &instructions,
            &[],
            chain.blockhash,
        )?;
        let tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[self.config.payer.as_ref()],
        )?;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.executor_skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: self.config.executor_rpc_max_retries,
            ..RpcSendTransactionConfig::default()
        };
        Ok((tx, send_cfg))
    }

    async fn build_gap_skip_tx(
        &self,
        executor: &Arc<ExecutorState>,
        lane: &Lane,
        head_sequence: u64,
    ) -> Result<(VersionedTransaction, RpcSendTransactionConfig)> {
        let chain = self.blockhashes.snapshot().await;
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(400_000),
            build_execute_head_memo_instruction(
                head_sequence,
                self.execute_nonce.fetch_add(1, Ordering::Relaxed),
            ),
            build_execute_instruction(
                self.config.program_id,
                executor.group,
                executor.execution_queue,
                &lane.remaining_accounts,
                executor
                    .effective_max_items(self.config.executor_max_items)
                    .max(1),
            ),
        ];
        if self.config.executor_prioritization_fee > 0 {
            instructions.insert(
                0,
                ComputeBudgetInstruction::set_compute_unit_price(
                    self.config.executor_prioritization_fee,
                ),
            );
        }
        let message = MessageV0::try_compile(
            &self.config.payer.pubkey(),
            &instructions,
            &[],
            chain.blockhash,
        )?;
        let tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[self.config.payer.as_ref()],
        )?;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.executor_skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: self.config.executor_rpc_max_retries,
            ..RpcSendTransactionConfig::default()
        };
        Ok((tx, send_cfg))
    }
}

enum UserSignatureMessage {
    Raw([u8; 32]),
    HexUtf8([u8; 64]),
}

impl UserSignatureMessage {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Raw(bytes) => bytes.as_ref(),
            Self::HexUtf8(bytes) => bytes.as_ref(),
        }
    }
}

#[derive(Clone)]
struct GrpcService {
    engine: Arc<Engine>,
}

#[tonic::async_trait]
impl CtmSequencerRelayer for GrpcService {
    async fn submit_intent(
        &self,
        request: Request<SubmitIntentRequest>,
    ) -> Result<Response<SubmitIntentResponse>, Status> {
        let response = self.engine.submit_intent(request.into_inner()).await?;
        Ok(Response::new(response))
    }
}

fn parse_socket_addr(value: String) -> Result<SocketAddr> {
    value
        .parse()
        .with_context(|| format!("invalid socket address: {value}"))
}

fn parse_u64_env(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) => Ok(value.parse::<u64>()?),
        Err(_) => Ok(default),
    }
}

fn parse_optional_usize_env(name: &str) -> Result<Option<usize>> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|value| value.parse::<usize>().map_err(anyhow::Error::from))
        .transpose()
}

fn parse_optional_pubkey_env(name: &str) -> Result<Option<Pubkey>> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|value| Pubkey::from_str(&value).map_err(anyhow::Error::from))
        .transpose()
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

fn read_keypair_env(primary: &str, fallback: &str) -> Result<Keypair> {
    let value = std::env::var(primary)
        .or_else(|_| std::env::var(fallback))
        .with_context(|| format!("{primary} or {fallback} is required"))?;
    read_keypair(&value)
}

fn read_keypair(raw: &str) -> Result<Keypair> {
    let path = Path::new(raw);
    if path.exists() {
        return read_keypair_file(path).map_err(|err| anyhow!(err.to_string()));
    }
    let bytes: Vec<u8> = serde_json::from_str(raw)?;
    Keypair::from_bytes(&bytes).map_err(|err| anyhow!(err.to_string()))
}

fn parse_pubkey(value: &str) -> Result<Pubkey, Status> {
    Pubkey::from_str(value).map_err(|err| Status::invalid_argument(err.to_string()))
}

fn parse_remaining_accounts(accounts: &[AccountMetaProto]) -> Result<Vec<AccountMeta>, Status> {
    accounts
        .iter()
        .map(|account| {
            Ok(AccountMeta {
                pubkey: parse_pubkey(&account.pubkey)?,
                is_signer: account.is_signer,
                is_writable: account.is_writable,
            })
        })
        .collect()
}

fn parse_signature_bytes(bytes: &[u8]) -> Result<[u8; 64], Status> {
    bytes.try_into().map_err(|_| {
        Status::invalid_argument("user_signature must be exactly 64 bytes".to_string())
    })
}

fn decode_margin_check_ops(payload: &[u8]) -> Result<Option<Vec<MarginCheckOp>>> {
    if payload.is_empty() {
        return Ok(None);
    }
    if payload.len() < QUEUE_PAYLOAD_HEADER_LEN {
        return Err(anyhow!(
            "queue payload too short: expected at least {} bytes, got {}",
            QUEUE_PAYLOAD_HEADER_LEN,
            payload.len()
        ));
    }
    let version = payload[0];
    if version != QUEUE_PAYLOAD_VERSION_V1 {
        return Err(anyhow!("unsupported queue payload version: {version}"));
    }
    let variant = queue_payload_variant_from_byte(payload[1])?;
    let flags = u16::from_le_bytes([payload[2], payload[3]]);
    if flags != 0 {
        return Err(anyhow!("queue payload flags must be zero, got {flags}"));
    }
    let body = &payload[QUEUE_PAYLOAD_HEADER_LEN..];
    match variant {
        QueuePayloadVariant::PerpPlaceOrderV2 => {
            let place = decode_perp_place_order_margin_op(body)?;
            Ok(Some(vec![place]).filter(|ops| !ops.is_empty()))
        }
        QueuePayloadVariant::PerpBatchIntent => {
            let ops = decode_perp_batch_margin_ops(body)?;
            if ops
                .iter()
                .any(|op| matches!(op, MarginCheckOp::Place { .. }))
            {
                Ok(Some(ops))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

fn queue_payload_variant_from_byte(value: u8) -> Result<QueuePayloadVariant> {
    match value {
        0 => Ok(QueuePayloadVariant::PerpPlaceOrderV2),
        1 => Ok(QueuePayloadVariant::PerpCancelOrder),
        2 => Ok(QueuePayloadVariant::PerpCancelOrderByClientOrderId),
        3 => Ok(QueuePayloadVariant::PerpCancelAllOrders),
        4 => Ok(QueuePayloadVariant::PerpCancelAllOrdersBySide),
        5 => Ok(QueuePayloadVariant::LiquidityDeposit),
        6 => Ok(QueuePayloadVariant::LiquidityWithdraw),
        7 => Ok(QueuePayloadVariant::PerpCancelOrderBySlot),
        8 => Ok(QueuePayloadVariant::PerpBatchIntent),
        _ => Err(anyhow!("unknown queue payload variant: {value}")),
    }
}

fn decode_perp_place_order_margin_op(body: &[u8]) -> Result<MarginCheckOp> {
    if body.len() != PERP_PLACE_ORDER_V2_PAYLOAD_LEN {
        return Err(anyhow!(
            "unexpected perp place payload length: expected {}, got {}",
            PERP_PLACE_ORDER_V2_PAYLOAD_LEN,
            body.len()
        ));
    }
    let order = PerpPlaceOrderV2Payload::try_from_slice(body)
        .map_err(|err| anyhow!("failed to deserialize perp place payload: {err}"))?;
    Ok(MarginCheckOp::Place {
        side: order.side,
        max_base_lots: order.max_base_lots,
        reduce_only: order.reduce_only,
    })
}

fn decode_perp_batch_margin_ops(body: &[u8]) -> Result<Vec<MarginCheckOp>> {
    if body.is_empty() {
        return Err(anyhow!("empty perp batch payload"));
    }
    let op_count = body[0] as usize;
    if op_count == 0 || op_count > PERP_BATCH_INTENT_MAX_OPS {
        return Err(anyhow!("invalid perp batch op count: {op_count}"));
    }

    let mut offset = 1usize;
    let mut ops = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        if offset >= body.len() {
            return Err(anyhow!("perp batch payload ended before op header"));
        }
        let variant = body[offset];
        offset += 1;
        match variant {
            0 => {
                if offset + PERP_CANCEL_ORDER_BY_SLOT_PAYLOAD_LEN > body.len() {
                    return Err(anyhow!("perp batch cancel-by-slot payload truncated"));
                }
                let cancel = PerpCancelOrderBySlotPayload::try_from_slice(
                    &body[offset..offset + PERP_CANCEL_ORDER_BY_SLOT_PAYLOAD_LEN],
                )
                .map_err(|err| {
                    anyhow!("failed to deserialize batch cancel-by-slot payload: {err}")
                })?;
                ops.push(MarginCheckOp::CancelByOrderId {
                    expected_order_id: cancel.expected_order_id,
                });
                offset += PERP_CANCEL_ORDER_BY_SLOT_PAYLOAD_LEN;
            }
            1 => {
                if offset + PERP_PLACE_ORDER_V2_PAYLOAD_LEN > body.len() {
                    return Err(anyhow!("perp batch place payload truncated"));
                }
                let place = decode_perp_place_order_margin_op(
                    &body[offset..offset + PERP_PLACE_ORDER_V2_PAYLOAD_LEN],
                )?;
                ops.push(place);
                offset += PERP_PLACE_ORDER_V2_PAYLOAD_LEN;
            }
            _ => return Err(anyhow!("unknown perp batch op variant: {variant}")),
        }
    }
    if offset != body.len() {
        return Err(anyhow!(
            "perp batch payload had {} trailing bytes",
            body.len().saturating_sub(offset)
        ));
    }
    Ok(ops)
}

fn build_harness_margin_snapshot(
    user_state: Option<&HarnessUserState>,
    requested_mango_account: &str,
) -> Result<HarnessMarginSnapshot> {
    let Some(user_state) = user_state else {
        return Ok(HarnessMarginSnapshot::default());
    };

    let mut snapshot = HarnessMarginSnapshot::default();
    for order in user_state
        .open_orders
        .iter()
        .filter(|order| order.mango_account == requested_mango_account)
    {
        let order_id = parse_wire_u128("harness order_id", &order.order_id)?;
        let market_index =
            parse_wire_perp_market_index("harness open order market", &order.market)?;
        let base_lots = parse_wire_i64("harness open order base_lots", &order.base_lots)?;
        let side = parse_harness_order_side(&order.side)?;
        snapshot.orders_by_id.insert(
            order_id,
            HarnessOrderExposure {
                market_index,
                side,
                base_lots,
            },
        );
        let overlay = snapshot.overlays.entry(market_index).or_default();
        match side {
            Side::Bid => overlay.bids_base_lots = overlay.bids_base_lots.saturating_add(base_lots),
            Side::Ask => overlay.asks_base_lots = overlay.asks_base_lots.saturating_add(base_lots),
        }
    }

    let is_single_account = user_state.mango_accounts.len() == 1
        && user_state
            .mango_accounts
            .iter()
            .any(|account| account == requested_mango_account);
    if is_single_account {
        for entry in &user_state.per_market {
            let market_index =
                parse_wire_perp_market_index("harness per_market market", &entry.market)?;
            let overlay = snapshot.overlays.entry(market_index).or_default();
            overlay.base_position_lots = parse_wire_i64(
                "harness per_market base_position_lots",
                &entry.base_position_lots,
            )?;
            overlay.quote_position_native = parse_wire_i80f48(
                "harness per_market quote_position_native",
                &entry.quote_position_native,
            )?;
            overlay.bids_base_lots = parse_wire_i64(
                "harness per_market open_order_base_lots_bid",
                &entry.open_order_base_lots_bid,
            )?;
            overlay.asks_base_lots = parse_wire_i64(
                "harness per_market open_order_base_lots_ask",
                &entry.open_order_base_lots_ask,
            )?;
        }
    }

    Ok(snapshot)
}

fn parse_harness_order_side(raw: &str) -> Result<Side> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bid" => Ok(Side::Bid),
        "ask" => Ok(Side::Ask),
        other => Err(anyhow!("invalid harness order side: {other}")),
    }
}

fn parse_wire_i64(label: &str, raw: &str) -> Result<i64> {
    raw.trim()
        .parse::<i64>()
        .with_context(|| format!("failed to parse {label} as i64"))
}

fn parse_wire_u128(label: &str, raw: &str) -> Result<u128> {
    raw.trim()
        .parse::<u128>()
        .with_context(|| format!("failed to parse {label} as u128"))
}

fn parse_wire_perp_market_index(label: &str, raw: &str) -> Result<PerpMarketIndex> {
    raw.trim()
        .parse::<PerpMarketIndex>()
        .with_context(|| format!("failed to parse {label} as perp market index"))
}

fn parse_wire_i80f48(label: &str, raw: &str) -> Result<I80F48> {
    I80F48::from_str(raw.trim()).map_err(|err| anyhow!("failed to parse {label} as I80F48: {err}"))
}

fn effective_requested_base_lots(
    side: Side,
    reduce_only: bool,
    max_base_lots: i64,
    current_base_lots: i64,
) -> i64 {
    if max_base_lots <= 0 {
        return 0;
    }
    if !reduce_only {
        return max_base_lots;
    }
    match side {
        Side::Bid => max_base_lots.min((-current_base_lots).max(0)),
        Side::Ask => max_base_lots.min(current_base_lots.max(0)),
    }
}

fn apply_margin_check_ops(
    account: &mut MangoAccountValue,
    target_market: &PerpMarket,
    margin_ops: &[MarginCheckOp],
    orders_by_id: &mut HashMap<u128, HarnessOrderExposure>,
) -> Result<()> {
    let position = account.perp_position_mut(target_market.perp_market_index)?;
    for op in margin_ops {
        match *op {
            MarginCheckOp::CancelByOrderId { expected_order_id } => {
                let Some(order) = orders_by_id.remove(&expected_order_id) else {
                    continue;
                };
                if order.market_index != target_market.perp_market_index || order.base_lots <= 0 {
                    continue;
                }
                match order.side {
                    Side::Bid => {
                        position.bids_base_lots =
                            position.bids_base_lots.saturating_sub(order.base_lots);
                    }
                    Side::Ask => {
                        position.asks_base_lots =
                            position.asks_base_lots.saturating_sub(order.base_lots);
                    }
                }
            }
            MarginCheckOp::Place {
                side,
                max_base_lots,
                reduce_only,
            } => {
                let effective_lots = effective_requested_base_lots(
                    side,
                    reduce_only,
                    max_base_lots,
                    position.effective_base_position_lots(),
                );
                if effective_lots <= 0 {
                    continue;
                }
                match side {
                    Side::Bid => {
                        position.bids_base_lots =
                            position.bids_base_lots.saturating_add(effective_lots);
                    }
                    Side::Ask => {
                        position.asks_base_lots =
                            position.asks_base_lots.saturating_add(effective_lots);
                    }
                }
            }
        }
    }
    Ok(())
}

fn build_margin_health_accounts(
    account: &MangoAccountValue,
    account_map: &HashMap<Pubkey, KeyedAccountSharedData>,
) -> Result<Vec<KeyedAccountSharedData>> {
    let mut bank_by_token_index = HashMap::new();
    let mut perp_by_market_index = HashMap::new();
    for (pubkey, account_data) in account_map {
        if let Ok(bank) = account_data.load::<Bank>() {
            bank_by_token_index
                .entry(bank.token_index)
                .or_insert(*pubkey);
        }
        if let Ok(perp_market) = account_data.load::<PerpMarket>() {
            perp_by_market_index
                .entry(perp_market.perp_market_index)
                .or_insert(*pubkey);
        }
    }

    let mut ordered_accounts = Vec::new();
    let mut required_pubkeys = HashSet::new();
    for token_position in account.active_token_positions() {
        let token_index = token_position.token_index;
        let bank_pubkey = bank_by_token_index
            .get(&token_index)
            .copied()
            .with_context(|| {
                format!(
                    "missing bank account for token index {} during margin precheck",
                    token_index
                )
            })?;
        append_margin_health_account(account_map, &mut ordered_accounts, bank_pubkey)?;
        required_pubkeys.insert(bank_pubkey);
    }
    for token_position in account.active_token_positions() {
        let token_index = token_position.token_index;
        let bank_pubkey = bank_by_token_index
            .get(&token_index)
            .copied()
            .with_context(|| {
                format!(
                    "missing bank account for token index {} during margin precheck",
                    token_index
                )
            })?;
        let bank = account_map
            .get(&bank_pubkey)
            .context("bank account disappeared during margin precheck")?
            .load::<Bank>()?;
        append_margin_health_account(account_map, &mut ordered_accounts, bank.oracle)?;
        required_pubkeys.insert(bank.oracle);
    }
    for perp_position in account.active_perp_positions() {
        let market_index = perp_position.market_index;
        let market_pubkey = perp_by_market_index
            .get(&market_index)
            .copied()
            .with_context(|| {
                format!(
                    "missing perp market account for market index {} during margin precheck",
                    market_index
                )
            })?;
        append_margin_health_account(account_map, &mut ordered_accounts, market_pubkey)?;
        required_pubkeys.insert(market_pubkey);
    }
    for perp_position in account.active_perp_positions() {
        let market_index = perp_position.market_index;
        let market_pubkey = perp_by_market_index
            .get(&market_index)
            .copied()
            .with_context(|| {
                format!(
                    "missing perp market account for market index {} during margin precheck",
                    market_index
                )
            })?;
        let market = account_map
            .get(&market_pubkey)
            .context("perp market account disappeared during margin precheck")?
            .load::<PerpMarket>()?;
        append_margin_health_account(account_map, &mut ordered_accounts, market.oracle)?;
        required_pubkeys.insert(market.oracle);
    }
    for serum_orders in account.active_serum3_orders() {
        let open_orders = serum_orders.open_orders;
        append_margin_health_account(account_map, &mut ordered_accounts, open_orders)?;
        required_pubkeys.insert(open_orders);
    }
    for openbook_orders in account.active_openbook_v2_orders() {
        let open_orders = openbook_orders.open_orders;
        append_margin_health_account(account_map, &mut ordered_accounts, open_orders)?;
        required_pubkeys.insert(open_orders);
    }
    for (pubkey, account_data) in account_map {
        if required_pubkeys.contains(pubkey) {
            continue;
        }
        ordered_accounts.push(account_data.clone());
    }
    Ok(ordered_accounts)
}

fn append_margin_health_account(
    account_map: &HashMap<Pubkey, KeyedAccountSharedData>,
    ordered_accounts: &mut Vec<KeyedAccountSharedData>,
    pubkey: Pubkey,
) -> Result<()> {
    let account = account_map
        .get(&pubkey)
        .with_context(|| format!("required health account missing: {pubkey}"))?;
    ordered_accounts.push(account.clone());
    Ok(())
}

fn merge_effective_runtime_flags(
    remaining_accounts: &[AccountMeta],
    fixed_accounts: &[AccountMeta],
) -> Vec<AccountMeta> {
    let mut merged = HashMap::<Pubkey, (bool, bool)>::new();
    for account in fixed_accounts.iter().chain(remaining_accounts.iter()) {
        let entry = merged
            .entry(account.pubkey)
            .or_insert((account.is_signer, account.is_writable));
        entry.0 |= account.is_signer;
        entry.1 |= account.is_writable;
    }
    remaining_accounts
        .iter()
        .map(|account| {
            let (is_signer, is_writable) = merged
                .get(&account.pubkey)
                .copied()
                .unwrap_or((account.is_signer, account.is_writable));
            AccountMeta {
                pubkey: account.pubkey,
                is_signer,
                is_writable,
            }
        })
        .collect()
}

fn normalize_lane_runtime_flags(remaining_accounts: &[AccountMeta]) -> Vec<AccountMeta> {
    let mut merged = HashMap::<Pubkey, (bool, bool)>::new();
    for account in remaining_accounts {
        let entry = merged
            .entry(account.pubkey)
            .or_insert((account.is_signer, account.is_writable));
        entry.0 |= account.is_signer;
        entry.1 |= account.is_writable;
    }
    remaining_accounts
        .iter()
        .map(|account| {
            let (is_signer, is_writable) = merged
                .get(&account.pubkey)
                .copied()
                .unwrap_or((account.is_signer, account.is_writable));
            AccountMeta {
                pubkey: account.pubkey,
                is_signer,
                is_writable,
            }
        })
        .collect()
}

fn hash_execution_queue_accounts(accounts: &[AccountMeta]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(accounts.len() * 34);
    for account in accounts {
        bytes.extend_from_slice(account.pubkey.as_ref());
        bytes.push(u8::from(account.is_signer));
        bytes.push(u8::from(account.is_writable));
    }
    hashv(&[&bytes]).to_bytes()
}

fn expand_lane_variants(
    lane_name: String,
    remaining_accounts: &[AccountMeta],
    group: Pubkey,
    execution_queue: Pubkey,
    include_legacy: bool,
) -> Vec<Lane> {
    // Hash the lane accounts the same way as the enqueue path:
    // merge fixed accounts (group, exec_queue, sysvar) before hashing.
    // This produces the accounts_hash stored in queue items.
    let enqueue_hash =
        hash_execution_queue_accounts_for_ctm_enqueue(group, execution_queue, remaining_accounts);
    let raw_accounts = remaining_accounts.to_vec();
    let mut lanes = vec![Lane {
        name: lane_name.clone(),
        hash: enqueue_hash,
        remaining_accounts: raw_accounts,
    }];
    if include_legacy {
        let legacy_accounts = merge_effective_runtime_flags(
            remaining_accounts,
            &[
                AccountMeta::new(group, false),
                AccountMeta::new(execution_queue, false),
                AccountMeta::new_readonly(sysvar::instructions::id(), false),
            ],
        );
        lanes.push(Lane {
            name: format!("{lane_name}-legacy"),
            hash: hash_execution_queue_accounts(&legacy_accounts),
            remaining_accounts: legacy_accounts,
        });
    }
    lanes
}

fn speculative_lane_priority(name: &str) -> u8 {
    if name.contains("dynamic-") && name.ends_with("-legacy") {
        0
    } else if name.contains("dynamic-") {
        1
    } else if name.ends_with("-legacy") {
        2
    } else {
        3
    }
}

fn select_gap_speculative_lanes(mut lanes: Vec<Lane>) -> Vec<Lane> {
    let dynamic_lanes: Vec<Lane> = lanes
        .iter()
        .filter(|lane| lane.name.starts_with("dynamic-"))
        .cloned()
        .collect();
    if !dynamic_lanes.is_empty() {
        lanes = dynamic_lanes;
    }
    lanes.sort_by(|left, right| {
        speculative_lane_priority(&left.name)
            .cmp(&speculative_lane_priority(&right.name))
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    for lane in lanes {
        if !seen.insert(lane.hash) {
            continue;
        }
        selected.push(lane);
        if selected.len() >= 4 {
            break;
        }
    }
    selected
}

fn load_executor_lanes(config: &Config) -> Result<HashMap<String, Lane>> {
    let mut lanes = HashMap::new();
    let (Some(group), Some(execution_queue)) = (config.executor_group, config.executor_queue)
    else {
        return Ok(lanes);
    };
    let Some(path) = config.executor_lane_config_path.as_ref() else {
        return Ok(lanes);
    };
    if !path.exists() {
        return Ok(lanes);
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read lane config {}", path.display()))?;
    let parsed: Vec<LaneConfigFile> = serde_json::from_str(&raw)
        .with_context(|| format!("invalid lane config {}", path.display()))?;
    for lane in parsed {
        let remaining_accounts = lane
            .remaining_accounts
            .iter()
            .map(|account| {
                Ok(AccountMeta {
                    pubkey: Pubkey::from_str(&account.pubkey)?,
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let lane_name = lane.name.unwrap_or_else(|| "lane".to_string());
        for expanded in expand_lane_variants(
            lane_name,
            &remaining_accounts,
            group,
            execution_queue,
            config.executor_include_legacy_fixed_hash,
        ) {
            lanes.insert(bytes_to_hex(&expanded.hash), expanded);
        }
    }
    let static_count = lanes.len();
    info!(
        "loaded static lanes: {} unique hashes from {:?}",
        static_count, path,
    );

    // Also load cached lanes
    if let Some(cache_path) = config.executor_lane_cache_path.as_ref() {
        if cache_path.exists() {
            match std::fs::read_to_string(cache_path) {
                Ok(raw) => {
                    if let Ok(cached) = serde_json::from_str::<Vec<LaneConfigFile>>(&raw) {
                        for lane in cached {
                            let remaining_accounts = lane
                                .remaining_accounts
                                .iter()
                                .map(|account| {
                                    Ok(AccountMeta {
                                        pubkey: Pubkey::from_str(&account.pubkey)?,
                                        is_signer: account.is_signer,
                                        is_writable: account.is_writable,
                                    })
                                })
                                .collect::<Result<Vec<_>>>();
                            if let Ok(accounts) = remaining_accounts {
                                let lane_name = lane.name.unwrap_or_else(|| "cached".to_string());
                                for expanded in expand_lane_variants(
                                    lane_name,
                                    &accounts,
                                    group,
                                    execution_queue,
                                    config.executor_include_legacy_fixed_hash,
                                ) {
                                    lanes.insert(bytes_to_hex(&expanded.hash), expanded);
                                }
                            }
                        }
                        info!(
                            "loaded lane cache: {} total unique hashes (+{} from cache) from {:?}",
                            lanes.len(),
                            lanes.len() - static_count,
                            cache_path,
                        );
                    }
                }
                Err(e) => {
                    warn!("failed to read lane cache {:?}: {e}", cache_path);
                }
            }
        }
    }

    Ok(lanes)
}

fn hash_execution_queue_accounts_for_ctm_enqueue(
    group: Pubkey,
    execution_queue: Pubkey,
    remaining_accounts: &[AccountMeta],
) -> [u8; 32] {
    let effective_remaining = merge_effective_runtime_flags(
        remaining_accounts,
        &[
            AccountMeta::new(group, false),
            AccountMeta::new(execution_queue, false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ],
    );
    let mut bytes = Vec::with_capacity(effective_remaining.len() * 34);
    for account in effective_remaining {
        bytes.extend_from_slice(account.pubkey.as_ref());
        bytes.push(u8::from(account.is_signer));
        bytes.push(u8::from(account.is_writable));
    }
    hashv(&[&bytes]).to_bytes()
}

fn canonical_envelope_message(group: Pubkey, envelope: &CtmEnvelope) -> [u8; 32] {
    hashv(&[
        b"mango-v4-ctm-envelope-v1",
        group.as_ref(),
        &envelope.sequence.to_le_bytes(),
        &envelope.min_execute_slot.to_le_bytes(),
        &[envelope.kind],
        &envelope.payload_hash,
        &envelope.accounts_hash,
        &envelope.expires_at_slot.to_le_bytes(),
    ])
    .to_bytes()
}

fn canonical_user_intent_message(
    group: Pubkey,
    mango_account: Pubkey,
    user_owner: Pubkey,
    envelope: &CtmEnvelope,
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-user-intent-v1",
        group.as_ref(),
        mango_account.as_ref(),
        user_owner.as_ref(),
        &[envelope.kind],
        &envelope.payload_hash,
        &envelope.accounts_hash,
    ])
    .to_bytes()
}

fn canonical_user_intent_message_hex_utf8(message: [u8; 32]) -> [u8; 64] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 64];
    let mut index = 0usize;
    while index < 32 {
        let byte = message[index];
        out[index * 2] = HEX[(byte >> 4) as usize];
        out[index * 2 + 1] = HEX[(byte & 0x0f) as usize];
        index += 1;
    }
    out
}

fn verify_user_signature(
    owner: Pubkey,
    signature_bytes: &[u8; 64],
    message: &[u8; 32],
) -> Result<UserSignatureMessage, Status> {
    let signature = Signature::try_from(signature_bytes.as_slice())
        .map_err(|err| Status::invalid_argument(err.to_string()))?;
    if signature.verify(owner.as_ref(), message) {
        return Ok(UserSignatureMessage::Raw(*message));
    }
    let hex_utf8 = canonical_user_intent_message_hex_utf8(*message);
    if signature.verify(owner.as_ref(), &hex_utf8) {
        return Ok(UserSignatureMessage::HexUtf8(hex_utf8));
    }
    Err(Status::invalid_argument(
        "user_signature verification failed",
    ))
}

fn build_presigned_ed25519_instruction(
    public_key: [u8; 32],
    message: &[u8],
    signature: [u8; 64],
) -> Instruction {
    let public_key_offset = 16u16;
    let signature_offset = public_key_offset + public_key.len() as u16;
    let message_data_offset = signature_offset + signature.len() as u16;
    let mut data = vec![0u8; message_data_offset as usize + message.len()];
    data[0] = 1;
    data[1] = 0;
    data[2..4].copy_from_slice(&signature_offset.to_le_bytes());
    data[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
    data[6..8].copy_from_slice(&public_key_offset.to_le_bytes());
    data[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    data[10..12].copy_from_slice(&message_data_offset.to_le_bytes());
    data[12..14].copy_from_slice(&(message.len() as u16).to_le_bytes());
    data[14..16].copy_from_slice(&u16::MAX.to_le_bytes());
    data[public_key_offset as usize..signature_offset as usize].copy_from_slice(&public_key);
    data[signature_offset as usize..message_data_offset as usize].copy_from_slice(&signature);
    data[message_data_offset as usize..].copy_from_slice(message);
    Instruction {
        program_id: ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

fn build_enqueue_instruction(
    program_id: Pubkey,
    group: Pubkey,
    execution_queue: Pubkey,
    remaining_accounts: &[AccountMeta],
    envelope: CtmEnvelope,
    payload: Vec<u8>,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(execution_queue, false),
        AccountMeta::new_readonly(sysvar::instructions::id(), false),
    ];
    accounts.extend_from_slice(remaining_accounts);
    accounts.push(AccountMeta::new_readonly(program_id, false));
    Instruction {
        program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueEnqueueCtm { envelope, payload }.data(),
    }
}

fn build_execute_instruction(
    program_id: Pubkey,
    group: Pubkey,
    execution_queue: Pubkey,
    remaining_accounts: &[AccountMeta],
    max_items: u16,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(execution_queue, false),
    ];
    accounts.extend_from_slice(remaining_accounts);
    Instruction {
        program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueExecute { max_items }.data(),
    }
}

fn build_execute_multi_instruction(
    program_id: Pubkey,
    group: Pubkey,
    execution_queue: Pubkey,
    lane_accounts: &[Vec<AccountMeta>],
    accounts_per_lane: u16,
    lane_hashes: Vec<[u8; 32]>,
    max_items: u16,
) -> Instruction {
    let lane_count = lane_accounts.len() as u8;
    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new(execution_queue, false),
    ];
    for lane in lane_accounts {
        accounts.extend_from_slice(lane);
    }
    Instruction {
        program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueExecuteMulti {
            max_items,
            lane_count,
            accounts_per_lane,
            lane_hashes,
        }
        .data(),
    }
}

fn build_execute_head_memo_instruction(head_sequence: u64, nonce: u64) -> Instruction {
    Instruction {
        program_id: SPL_MEMO_PROGRAM_ID,
        accounts: Vec::new(),
        data: format!("ctm-head-seq:{head_sequence}:nonce:{nonce}").into_bytes(),
    }
}

fn build_execution_queue_configure_instruction(
    program_id: Pubkey,
    group: Pubkey,
    execution_queue: Pubkey,
    admin: Pubkey,
    queue_state: &QueueAdminState,
    pause_execute: bool,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(group, false),
            AccountMeta::new(execution_queue, false),
            AccountMeta::new_readonly(admin, true),
        ],
        data: mango_v4::instruction::ExecutionQueueConfigure {
            params: ExecutionQueueConfigParams {
                gap_wait_slots: queue_state.gap_wait_slots,
                liquidity_delay_slots: queue_state.liquidity_delay_slots,
                pause_ingress: queue_state.pause_ingress,
                pause_execute,
            },
        }
        .data(),
    }
}

fn build_execution_queue_drop_ctm_instruction(
    program_id: Pubkey,
    group: Pubkey,
    execution_queue: Pubkey,
    admin: Pubkey,
    sequence: u64,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(group, false),
            AccountMeta::new(execution_queue, false),
            AccountMeta::new_readonly(admin, true),
        ],
        data: mango_v4::instruction::ExecutionQueueDropCtm { sequence }.data(),
    }
}

const EXECUTION_QUEUE_GAP_WAIT_SLOTS_OFFSET: usize = 184;
const EXECUTION_QUEUE_PAUSED_INGRESS_OFFSET: usize = 146;
const EXECUTION_QUEUE_PAUSED_EXECUTE_OFFSET: usize = 147;
const EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET: usize = 192;
const EXECUTION_QUEUE_CTM_COUNT_OFFSET: usize = 200;
const EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET: usize = 204;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueHeadSource {
    Ctm,
    Liquidity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueSequencePresence {
    PastFloor,
    Pending,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueueHead {
    count: u32,
    ctm_count: u32,
    liquidity_count: u32,
    next_sequence: u64,
    max_seen_sequence: u64,
    head_accounts_hash: Option<[u8; 32]>,
    source: Option<QueueHeadSource>,
    reason: &'static str,
    ctm_sequence: Option<u64>,
    ctm_kind: Option<u8>,
    ctm_status: Option<u8>,
}

impl QueueHead {
    fn is_ctm_gap_state(&self) -> bool {
        matches!(
            self.reason,
            "ctm_gap_or_empty_slot" | "ctm_sequence_mismatch"
        )
    }

    fn blocked_reason(&self) -> String {
        format!(
            "reason={} ctm_count={} liquidity_count={} ctm_sequence={} ctm_kind={} ctm_status={}",
            self.reason,
            self.ctm_count,
            self.liquidity_count,
            self.ctm_sequence
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.ctm_kind
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.ctm_status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
        )
    }
}

fn inspect_queue_head(queue_data: &[u8]) -> QueueHead {
    let count = if queue_data.len() >= EXECUTION_QUEUE_COUNT_OFFSET + 4 {
        u32::from_le_bytes(
            queue_data[EXECUTION_QUEUE_COUNT_OFFSET..EXECUTION_QUEUE_COUNT_OFFSET + 4]
                .try_into()
                .unwrap_or([0; 4]),
        )
    } else {
        0
    };
    let next_sequence = if queue_data.len() >= EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET + 8 {
        u64::from_le_bytes(
            queue_data
                [EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET..EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        )
    } else {
        0
    };
    let max_seen_sequence = if queue_data.len() >= EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET + 8 {
        u64::from_le_bytes(
            queue_data[EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET
                ..EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        )
    } else {
        0
    };
    let ctm_count = if queue_data.len() >= EXECUTION_QUEUE_CTM_COUNT_OFFSET + 4 {
        u32::from_le_bytes(
            queue_data[EXECUTION_QUEUE_CTM_COUNT_OFFSET..EXECUTION_QUEUE_CTM_COUNT_OFFSET + 4]
                .try_into()
                .unwrap_or([0; 4]),
        )
    } else {
        0
    };
    let liquidity_count = if queue_data.len() >= EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET + 4 {
        u32::from_le_bytes(
            queue_data[EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET
                ..EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET + 4]
                .try_into()
                .unwrap_or([0; 4]),
        )
    } else {
        0
    };
    if count == 0 {
        return QueueHead {
            count,
            ctm_count,
            liquidity_count,
            next_sequence,
            max_seen_sequence,
            head_accounts_hash: None,
            source: None,
            reason: "empty",
            ctm_sequence: None,
            ctm_kind: None,
            ctm_status: None,
        };
    }

    let liquidity_head = if queue_data.len() >= EXECUTION_QUEUE_HEAD_OFFSET + 4 {
        u32::from_le_bytes(
            queue_data[EXECUTION_QUEUE_HEAD_OFFSET..EXECUTION_QUEUE_HEAD_OFFSET + 4]
                .try_into()
                .unwrap_or([0; 4]),
        ) as usize
    } else {
        0
    };
    let mut ctm_sequence = None;
    let mut ctm_kind = None;
    let mut ctm_status = None;
    if ctm_count > 0 {
        let ctm_offset = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
            + (next_sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
        if ctm_offset + EXECUTION_QUEUE_ITEM_SIZE <= queue_data.len() {
            let sequence = u64::from_le_bytes(
                queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                    ..ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let kind = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET];
            let status = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
            ctm_sequence = Some(sequence);
            ctm_kind = Some(kind);
            ctm_status = Some(status);
            if status == 1 && kind == 0 && sequence == next_sequence {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(
                    &queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                        ..ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32],
                );
                return QueueHead {
                    count,
                    ctm_count,
                    liquidity_count,
                    next_sequence,
                    max_seen_sequence,
                    head_accounts_hash: Some(hash),
                    source: Some(QueueHeadSource::Ctm),
                    reason: "ctm_pending",
                    ctm_sequence,
                    ctm_kind,
                    ctm_status,
                };
            }
        } else {
            return QueueHead {
                count,
                ctm_count,
                liquidity_count,
                next_sequence,
                max_seen_sequence,
                head_accounts_hash: None,
                source: None,
                reason: "ctm_slot_oob",
                ctm_sequence,
                ctm_kind,
                ctm_status,
            };
        }
    }

    if liquidity_count > 0 {
        let liq_offset = EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET
            + (liquidity_head % EXECUTION_QUEUE_LIQUIDITY_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
        if liq_offset + EXECUTION_QUEUE_ITEM_SIZE <= queue_data.len() {
            let status = queue_data[liq_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
            if status == 1 {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(
                    &queue_data[liq_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                        ..liq_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32],
                );
                return QueueHead {
                    count,
                    ctm_count,
                    liquidity_count,
                    next_sequence,
                    max_seen_sequence,
                    head_accounts_hash: Some(hash),
                    source: Some(QueueHeadSource::Liquidity),
                    reason: "liquidity_pending",
                    ctm_sequence,
                    ctm_kind,
                    ctm_status,
                };
            }
            return QueueHead {
                count,
                ctm_count,
                liquidity_count,
                next_sequence,
                max_seen_sequence,
                head_accounts_hash: None,
                source: None,
                reason: "liquidity_status_mismatch",
                ctm_sequence,
                ctm_kind,
                ctm_status,
            };
        }
    }

    let reason = if ctm_count == 0 {
        "liquidity_only_no_pending"
    } else if ctm_status == Some(0) {
        "ctm_gap_or_empty_slot"
    } else if ctm_sequence != Some(next_sequence) {
        "ctm_sequence_mismatch"
    } else if ctm_kind != Some(0) {
        "ctm_kind_mismatch"
    } else if ctm_status != Some(1) {
        "ctm_status_mismatch"
    } else {
        "head_unavailable"
    };
    QueueHead {
        count,
        ctm_count,
        liquidity_count,
        next_sequence,
        max_seen_sequence,
        head_accounts_hash: None,
        source: None,
        reason,
        ctm_sequence,
        ctm_kind,
        ctm_status,
    }
}

fn inspect_queue_admin_state(queue_data: &[u8]) -> QueueAdminState {
    let gap_wait_slots = if queue_data.len() >= EXECUTION_QUEUE_GAP_WAIT_SLOTS_OFFSET + 8 {
        u64::from_le_bytes(
            queue_data
                [EXECUTION_QUEUE_GAP_WAIT_SLOTS_OFFSET..EXECUTION_QUEUE_GAP_WAIT_SLOTS_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        )
    } else {
        0
    };
    let liquidity_delay_slots =
        if queue_data.len() >= EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET + 8 {
            u64::from_le_bytes(
                queue_data[EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET
                    ..EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            )
        } else {
            0
        };
    let pause_ingress = queue_data
        .get(EXECUTION_QUEUE_PAUSED_INGRESS_OFFSET)
        .copied()
        .unwrap_or_default()
        != 0;
    let pause_execute = queue_data
        .get(EXECUTION_QUEUE_PAUSED_EXECUTE_OFFSET)
        .copied()
        .unwrap_or_default()
        != 0;
    QueueAdminState {
        pause_ingress,
        pause_execute,
        gap_wait_slots,
        liquidity_delay_slots,
        head: inspect_queue_head(queue_data),
    }
}

fn inspect_next_enqueue_sequence(queue_data: &[u8]) -> u64 {
    let head = inspect_queue_head(queue_data);
    if head.count == 0 {
        head.next_sequence
    } else {
        let start = head.next_sequence;
        let end = head
            .next_sequence
            .saturating_add(EXECUTION_QUEUE_CTM_CAPACITY as u64);
        for sequence in start..end {
            let ctm_offset = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
                + (sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
            if ctm_offset + EXECUTION_QUEUE_ITEM_SIZE > queue_data.len() {
                break;
            }
            let slot_sequence = u64::from_le_bytes(
                queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                    ..ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let slot_status = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
            if slot_status != 1 || slot_sequence != sequence {
                return sequence;
            }
        }
        head.max_seen_sequence.saturating_add(1)
    }
}

fn find_next_pending_ctm_sequence(
    queue_data: &[u8],
    next_sequence: u64,
    max_seen_sequence: u64,
) -> Option<u64> {
    if max_seen_sequence < next_sequence {
        return None;
    }
    let mut sequence = next_sequence;
    while sequence <= max_seen_sequence {
        if inspect_queue_sequence_presence(queue_data, sequence) == QueueSequencePresence::Pending {
            return Some(sequence);
        }
        sequence = sequence.saturating_add(1);
    }
    None
}

fn inspect_gap_recovery_batch(queue_data: &[u8], head: &QueueHead, max_batch: usize) -> Vec<u64> {
    if max_batch == 0 || !head.is_ctm_gap_state() {
        return Vec::new();
    }
    let Some(first_pending_sequence) =
        find_next_pending_ctm_sequence(queue_data, head.next_sequence, head.max_seen_sequence)
    else {
        return Vec::new();
    };

    let mut sequences = Vec::new();
    let end_sequence = first_pending_sequence
        .saturating_add(max_batch.saturating_sub(1) as u64)
        .min(head.max_seen_sequence);
    let mut sequence = first_pending_sequence;
    while sequence <= end_sequence {
        if inspect_queue_sequence_presence(queue_data, sequence) != QueueSequencePresence::Pending {
            break;
        }
        sequences.push(sequence);
        sequence = sequence.saturating_add(1);
    }
    sequences
}

fn inspect_near_head_lane_entries(
    queue_data: &[u8],
    head: &QueueHead,
    max_scan_items: usize,
    max_unique_hashes: usize,
) -> Vec<(u64, [u8; 32])> {
    if max_scan_items == 0 || max_unique_hashes == 0 || head.reason != "ctm_pending" {
        return Vec::new();
    }

    let max_scan_items = max_scan_items.min(64);
    let max_unique_hashes = max_unique_hashes.min(20);
    let end_sequence = head
        .next_sequence
        .saturating_add(max_scan_items.saturating_sub(1) as u64)
        .min(head.max_seen_sequence);

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut sequence = head.next_sequence;
    while sequence <= end_sequence {
        let ctm_offset = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
            + (sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
        if ctm_offset + EXECUTION_QUEUE_ITEM_SIZE > queue_data.len() {
            break;
        }
        let slot_sequence = u64::from_le_bytes(
            queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                ..ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        );
        let slot_kind = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET];
        let slot_status = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
        if slot_status != 1 || slot_kind != 0 || slot_sequence != sequence {
            break;
        }

        let mut slot_hash = [0u8; 32];
        slot_hash.copy_from_slice(
            &queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                ..ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32],
        );
        if seen.insert(slot_hash) {
            entries.push((sequence, slot_hash));
            if entries.len() >= max_unique_hashes {
                break;
            }
        }
        sequence = sequence.saturating_add(1);
    }

    entries
}

fn inspect_near_head_lane_hashes(
    queue_data: &[u8],
    head: &QueueHead,
    max_scan_items: usize,
    max_unique_hashes: usize,
) -> Vec<[u8; 32]> {
    inspect_near_head_lane_entries(queue_data, head, max_scan_items, max_unique_hashes)
        .into_iter()
        .map(|(_, hash)| hash)
        .collect()
}

fn inspect_no_lane_match_batch(queue_data: &[u8], head: &QueueHead, max_batch: usize) -> Vec<u64> {
    if max_batch == 0 || head.reason != "ctm_pending" {
        return Vec::new();
    }
    let Some(target_hash) = head.head_accounts_hash else {
        return Vec::new();
    };

    let mut sequences = Vec::new();
    let end_sequence = head
        .next_sequence
        .saturating_add(max_batch.saturating_sub(1) as u64)
        .min(head.max_seen_sequence);
    let mut sequence = head.next_sequence;
    while sequence <= end_sequence {
        let ctm_offset = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
            + (sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
        if ctm_offset + EXECUTION_QUEUE_ITEM_SIZE > queue_data.len() {
            break;
        }
        let slot_sequence = u64::from_le_bytes(
            queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
                ..ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        );
        let slot_kind = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET];
        let slot_status = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
        if slot_status != 1 || slot_kind != 0 || slot_sequence != sequence {
            break;
        }
        let mut slot_hash = [0u8; 32];
        slot_hash.copy_from_slice(
            &queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                ..ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32],
        );
        if slot_hash != target_hash {
            break;
        }
        sequences.push(sequence);
        sequence = sequence.saturating_add(1);
    }
    sequences
}

fn inspect_queue_sequence_presence(queue_data: &[u8], sequence: u64) -> QueueSequencePresence {
    let head = inspect_queue_head(queue_data);
    if sequence < head.next_sequence {
        return QueueSequencePresence::PastFloor;
    }
    if sequence
        >= head
            .next_sequence
            .saturating_add(EXECUTION_QUEUE_CTM_CAPACITY as u64)
    {
        return QueueSequencePresence::Absent;
    }
    let ctm_offset = EXECUTION_QUEUE_CTM_ITEMS_OFFSET
        + (sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE;
    if ctm_offset + EXECUTION_QUEUE_ITEM_SIZE > queue_data.len() {
        return QueueSequencePresence::Absent;
    }
    let slot_sequence = u64::from_le_bytes(
        queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET
            ..ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
            .try_into()
            .unwrap_or([0; 8]),
    );
    let slot_status = queue_data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
    if slot_status == 1 && slot_sequence == sequence {
        QueueSequencePresence::Pending
    } else {
        QueueSequencePresence::Absent
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn unix_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn internal_status(err: impl std::fmt::Display) -> Status {
    Status::internal(err.to_string())
}

fn is_execution_queue_duplicate_sequence_error(
    err: &solana_client::client_error::ClientError,
) -> bool {
    let duplicate_code = MangoError::ExecutionQueueDuplicateSequence.error_code();
    if let ClientErrorKind::RpcError(RpcError::RpcResponseError { data, .. }) = err.kind() {
        if let RpcResponseErrorData::SendTransactionPreflightFailure(sim) = data {
            if let Some(TransactionError::InstructionError(
                _,
                InstructionError::Custom(custom_code),
            )) = &sim.err
            {
                if *custom_code == duplicate_code {
                    return true;
                }
            }
            if sim.logs.as_ref().is_some_and(|logs| {
                logs.iter()
                    .any(|log| log.contains("custom program error: 0x17c3"))
            }) {
                return true;
            }
        }
    }
    let rendered = err.to_string();
    rendered.contains("custom program error: 0x17c3")
        || rendered.contains("custom program error: 6083")
        || rendered.contains("Custom(6083)")
}

/// Legacy mapper from RPC client error → gRPC Status. The submit_intent
/// hot path no longer calls rpc.send_transaction directly (Phase 1.1
/// moved that to the bg submitter pool), so this is currently unused.
/// Kept around for any future hot-path call site that wants to surface a
/// classified error.
#[allow(dead_code)]
fn rpc_status(err: impl std::fmt::Display) -> Status {
    let message = err.to_string();
    if message.contains("custom program error: 0x17bc")
        || message.contains("custom program error: 0x17BD")
        || message.contains("ExecutionQueueFull")
    {
        return Status::resource_exhausted(message);
    }
    Status::deadline_exceeded(message)
}

fn parse_startup_flags() -> Result<StartupFlags> {
    parse_startup_flags_from_iter(std::env::args().skip(1))
}

fn parse_startup_flags_from_iter<I, S>(args: I) -> Result<StartupFlags>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut flags = StartupFlags::default();
    for arg in args {
        match arg.as_ref() {
            "--enable-health-check" => {
                flags.enable_health_check = true;
            }
            "--help" | "-h" => {
                return Err(anyhow!(
                    "usage: service-mango-execution-engine [--enable-health-check]"
                ));
            }
            other => return Err(anyhow!("unknown startup flag: {other}")),
        }
    }
    Ok(flags)
}

async fn run_http_server(bind_addr: SocketAddr, metrics: Arc<Metrics>) {
    let metrics_filter = warp::any().map(move || metrics.clone());
    let healthz =
        warp::path!("healthz").map(|| warp::reply::json(&serde_json::json!({ "ok": true })));
    let metrics_route = warp::path!("metrics")
        .and(metrics_filter)
        .map(|metrics: Arc<Metrics>| {
            warp::reply::with_header(
                metrics.render(),
                "content-type",
                "text/plain; version=0.0.4",
            )
        });
    warp::serve(healthz.or(metrics_route)).run(bind_addr).await;
}

/// Snapshot of (timestamp_ms, counter_value) used by the sampler to compute
/// rates and windowed deltas without ever touching the hot path's atomics
/// more than once per tick.
#[derive(Clone, Copy)]
struct CounterTick {
    ts_ms: u64,
    ingress_total: u64,
    ingress_accepted_total: u64,
    ingress_rejected_total: u64,
    executed_total: u64,
    executed_failed_total: u64,
}

/// Background task that owns the historical snapshots needed to derive rates
/// and 60s window counts. Hot-path producers only fetch_add lifetime totals;
/// the sampler reads them once per tick, computes deltas against the ring,
/// and publishes the results into Metrics' sampler-published gauges.
///
/// Critically, this task is the *only* writer for those gauges, so the hot
/// path never observes contention from the sampler.
async fn run_metrics_sampler(
    metrics: Arc<Metrics>,
    executor: Option<Arc<ExecutorState>>,
    unique_addresses: Arc<StdMutex<HashMap<[u8; 32], u64>>>,
    latency_optimistic: Arc<LatencyTracker>,
    latency_processed: Arc<LatencyTracker>,
    executor_stale_ms: u64,
    event_cranker_stale_ms: u64,
    event_cranker_enabled: bool,
) {
    // Tick once per second; ring of 70 ticks gives us a 70s window which
    // comfortably covers the 60s rolling counts.
    const TICK_MS: u64 = 1_000;
    const RING_SECONDS: usize = 70;
    let mut ring: VecDeque<CounterTick> = VecDeque::with_capacity(RING_SECONDS + 1);

    loop {
        let now_ms = unix_timestamp_ms();
        let snapshot = CounterTick {
            ts_ms: now_ms,
            ingress_total: metrics.ingress_total.load(Ordering::Relaxed),
            ingress_accepted_total: metrics.ingress_accepted_total.load(Ordering::Relaxed),
            ingress_rejected_total: metrics.ingress_rejected_total.load(Ordering::Relaxed),
            executed_total: metrics.executed_total.load(Ordering::Relaxed),
            executed_failed_total: metrics.executed_failed_total.load(Ordering::Relaxed),
        };
        ring.push_back(snapshot);
        while ring.len() > RING_SECONDS {
            ring.pop_front();
        }

        // Helper: find the oldest snapshot whose ts_ms is <= now_ms - window.
        // Falls back to the oldest snapshot in the ring if the window isn't
        // yet fully populated, which means the rate is computed against the
        // shorter actual window.
        let pick = |window_ms: u64| -> &CounterTick {
            let cutoff = now_ms.saturating_sub(window_ms);
            ring.iter()
                .find(|s| s.ts_ms >= cutoff)
                .unwrap_or(ring.front().expect("ring populated above"))
        };

        let s10 = *pick(10_000);
        let s60 = *pick(60_000);

        // tps = (now_total - then_total) / window_seconds, scaled *1000.
        let span_10s_ms = now_ms.saturating_sub(s10.ts_ms).max(1);
        let span_10s_secs_milli = span_10s_ms; // ms per "1 second" yardstick
        let ingress_delta_10s = snapshot.ingress_total.saturating_sub(s10.ingress_total);
        let executed_delta_10s = snapshot.executed_total.saturating_sub(s10.executed_total);
        let ingress_tps_milli = ingress_delta_10s.saturating_mul(1_000_000) / span_10s_secs_milli;
        let executed_tps_milli =
            executed_delta_10s.saturating_mul(1_000_000) / span_10s_secs_milli;

        let accepted_60s = snapshot
            .ingress_accepted_total
            .saturating_sub(s60.ingress_accepted_total);
        let rejected_60s = snapshot
            .ingress_rejected_total
            .saturating_sub(s60.ingress_rejected_total);
        let executed_60s = snapshot.executed_total.saturating_sub(s60.executed_total);
        let executed_failed_60s = snapshot
            .executed_failed_total
            .saturating_sub(s60.executed_failed_total);

        metrics
            .ingress_tps_10s_milli
            .store(ingress_tps_milli, Ordering::Relaxed);
        metrics
            .executed_tps_10s_milli
            .store(executed_tps_milli, Ordering::Relaxed);
        metrics
            .ingress_accepted_60s
            .store(accepted_60s, Ordering::Relaxed);
        metrics
            .ingress_rejected_60s
            .store(rejected_60s, Ordering::Relaxed);
        metrics.executed_60s.store(executed_60s, Ordering::Relaxed);
        metrics
            .executed_failed_60s
            .store(executed_failed_60s, Ordering::Relaxed);

        // Mirror executor queue depth into a published gauge so /metrics
        // readers don't need to peek into ExecutorState.
        if let Some(exec) = executor.as_ref() {
            metrics
                .execution_queue_depth
                .store(exec.queue_count() as u64, Ordering::Relaxed);
        }

        // Unique addresses: sweep stale entries (>60s old), publish the
        // remaining count. Single bounded HashMap pass per second; not on
        // the hot path.
        {
            if let Ok(mut guard) = unique_addresses.lock() {
                let cutoff = now_ms.saturating_sub(60_000);
                guard.retain(|_, last_seen_ms| *last_seen_ms >= cutoff);
                metrics
                    .unique_addresses_60s
                    .store(guard.len() as u64, Ordering::Relaxed);
            }
        }

        // Heartbeat-derived health gauges. The sampler is the single writer
        // for these so /metrics renderer just reads.
        metrics.relayer_healthy.store(1, Ordering::Relaxed);
        let executor_last = metrics.executor_last_tick_ms.load(Ordering::Relaxed);
        let executor_healthy = if executor.is_none() {
            // Executor disabled — report 1 so the gauge isn't a false alarm.
            1
        } else if executor_last == 0 {
            0
        } else {
            (now_ms.saturating_sub(executor_last) <= executor_stale_ms) as u64
        };
        metrics
            .executor_healthy
            .store(executor_healthy, Ordering::Relaxed);

        let cranker_last = metrics.event_cranker_last_tick_ms.load(Ordering::Relaxed);
        let cranker_healthy = if !event_cranker_enabled {
            1
        } else if cranker_last == 0 {
            0
        } else {
            (now_ms.saturating_sub(cranker_last) <= event_cranker_stale_ms) as u64
        };
        metrics
            .event_cranker_healthy
            .store(cranker_healthy, Ordering::Relaxed);

        // Latency: snapshot each tracker, sort, and publish percentile
        // gauges. Each call holds its own short critical section under that
        // tracker's std mutex; locks are never held across an `.await`.

        // (1) Submit latency (relayer-only).
        let submit_summary = metrics.compute_submit_summary();
        metrics
            .latency_ingress_to_submitted_p50_ms
            .store(submit_summary.p50_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_submitted_p95_ms
            .store(submit_summary.p95_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_submitted_max_ms
            .store(submit_summary.max_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_submitted_samples
            .store(submit_summary.count, Ordering::Relaxed);

        // (2) Optimistic latency.
        let opt_summary = latency_optimistic.compute_summary();
        metrics
            .latency_ingress_to_optimistic_p50_ms
            .store(opt_summary.p50_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_optimistic_p95_ms
            .store(opt_summary.p95_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_optimistic_max_ms
            .store(opt_summary.max_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_optimistic_samples
            .store(opt_summary.count, Ordering::Relaxed);
        metrics.latency_optimistic_completed_total.store(
            latency_optimistic.completed_total.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        metrics.latency_optimistic_expired_total.store(
            latency_optimistic.expired_total.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        metrics
            .latency_optimistic_pending_inflight
            .store(latency_optimistic.pending_len() as u64, Ordering::Relaxed);

        // (3) Processed latency.
        let proc_summary = latency_processed.compute_summary();
        metrics
            .latency_ingress_to_processed_p50_ms
            .store(proc_summary.p50_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_processed_p95_ms
            .store(proc_summary.p95_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_processed_max_ms
            .store(proc_summary.max_ms, Ordering::Relaxed);
        metrics
            .latency_ingress_to_processed_samples
            .store(proc_summary.count, Ordering::Relaxed);
        metrics.latency_processed_completed_total.store(
            latency_processed.completed_total.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        metrics.latency_processed_expired_total.store(
            latency_processed.expired_total.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        metrics
            .latency_processed_pending_inflight
            .store(latency_processed.pending_len() as u64, Ordering::Relaxed);

        // (4) Per-stage histograms (Phase 0). Each compute_summary takes a
        // brief lock on its own histogram; never crosses an `.await`.
        let stage_parse = metrics.stage_parse_latency.compute_summary();
        metrics
            .latency_stage_parse_p50_ms
            .store(stage_parse.p50_ms, Ordering::Relaxed);
        metrics
            .latency_stage_parse_p95_ms
            .store(stage_parse.p95_ms, Ordering::Relaxed);
        let stage_margin = metrics.stage_margin_check_latency.compute_summary();
        metrics
            .latency_stage_margin_check_p50_ms
            .store(stage_margin.p50_ms, Ordering::Relaxed);
        metrics
            .latency_stage_margin_check_p95_ms
            .store(stage_margin.p95_ms, Ordering::Relaxed);
        let stage_sign = metrics.stage_sign_latency.compute_summary();
        metrics
            .latency_stage_sign_p50_ms
            .store(stage_sign.p50_ms, Ordering::Relaxed);
        metrics
            .latency_stage_sign_p95_ms
            .store(stage_sign.p95_ms, Ordering::Relaxed);
        let stage_event = metrics.stage_event_dispatch_latency.compute_summary();
        metrics
            .latency_stage_event_dispatch_p50_ms
            .store(stage_event.p50_ms, Ordering::Relaxed);
        metrics
            .latency_stage_event_dispatch_p95_ms
            .store(stage_event.p95_ms, Ordering::Relaxed);
        let stage_bg = metrics.stage_bg_rpc_submit_latency.compute_summary();
        metrics
            .latency_stage_bg_rpc_submit_p50_ms
            .store(stage_bg.p50_ms, Ordering::Relaxed);
        metrics
            .latency_stage_bg_rpc_submit_p95_ms
            .store(stage_bg.p95_ms, Ordering::Relaxed);

        metrics
            .sampler_last_tick_ms
            .store(now_ms, Ordering::Relaxed);

        tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
    }
}

/// Background task that probes the bridge `/healthz` endpoint and writes the
/// result into the bridge_healthy gauge. Uses its own short-lived reqwest
/// client with a hard timeout so a hung bridge can never wedge this loop.
async fn run_bridge_health_prober(
    metrics: Arc<Metrics>,
    bridge_url: String,
    interval_ms: u64,
    timeout_ms: u64,
) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            warn!("bridge health prober: failed to build client: {err:?}");
            return;
        }
    };
    info!(
        "bridge health prober enabled url={} interval_ms={} timeout_ms={}",
        bridge_url, interval_ms, timeout_ms
    );
    loop {
        let healthy = match client.get(&bridge_url).send().await {
            Ok(resp) if resp.status().is_success() => 1u64,
            Ok(resp) => {
                debug!("bridge health probe non-2xx: {}", resp.status());
                0
            }
            Err(err) => {
                debug!("bridge health probe error: {err:?}");
                0
            }
        };
        metrics.bridge_healthy.store(healthy, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
}

/// Convert a `rust_harness::UserState` (returned by the embedded
/// ContinuumStateEngine) into the relayer's internal `HarnessUserState`
/// DTO. The two types share the same field shape — this is a pure
/// re-pack with no allocation overhead beyond what `into_iter().map(...)`
/// already costs. Used in the Phase 2 hot-path replacement of
/// `fetch_harness_user_state`'s HTTP fallback.
fn harness_user_state_from_rust_harness(user: rust_harness::UserState) -> HarnessUserState {
    HarnessUserState {
        mango_accounts: user.mango_accounts,
        open_orders: user
            .open_orders
            .into_iter()
            .map(|o| HarnessOpenOrder {
                order_id: o.order_id,
                mango_account: o.mango_account,
                market: o.market,
                side: o.side,
                base_lots: o.base_lots,
            })
            .collect(),
        per_market: user
            .per_market
            .into_iter()
            .map(|p| HarnessUserPerMarket {
                market: p.market,
                open_order_base_lots_bid: p.open_order_base_lots_bid,
                open_order_base_lots_ask: p.open_order_base_lots_ask,
                base_position_lots: p.base_position_lots,
                quote_position_native: p.quote_position_native,
            })
            .collect(),
    }
}

/// One-time startup fetch of an EngineSnapshot from the legacy harness so
/// the relayer can populate its in-process ContinuumStateEngine. NOT on the
/// hot path — this runs once during main(), with a long timeout. Phase 5
/// will replace this with a Rust on-chain reader.
async fn bootstrap_local_state(
    url: &str,
    timeout_ms: u64,
) -> anyhow::Result<ContinuumStateEngine> {
    info!("local-state bootstrap: fetching {url} timeout_ms={timeout_ms}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .context("build bootstrap reqwest client")?;
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("local-state bootstrap GET failed: {url}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "local-state bootstrap returned non-2xx status {} from {}",
            status,
            url
        ));
    }
    let body = response
        .text()
        .await
        .context("read local-state bootstrap body")?;
    let snapshot: EngineSnapshot = serde_json::from_str(&body)
        .context("decode local-state bootstrap EngineSnapshot")?;
    let mut engine = ContinuumStateEngine::new();
    engine
        .bootstrap_from_onchain_snapshot(snapshot)
        .map_err(|err| anyhow!("rust-harness bootstrap_from_onchain_snapshot failed: {err}"))?;
    info!("local-state bootstrap: ContinuumStateEngine seeded successfully");
    Ok(engine)
}

/// Parse a harness watermark sequence number out of a JSON response, walking
/// a dot-separated JSON path (e.g. `data.last_processed_sequence` for the
/// queue endpoint or `data.watermarks.optimistic_seq` for the markets
/// endpoint). Accepts both string-encoded and numeric sequence values
/// because the harness stringifies large integers to avoid JS precision
/// loss.
fn parse_harness_watermark(body: &str, json_path: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let mut cursor = &v;
    for segment in json_path.split('.') {
        cursor = cursor.get(segment)?;
    }
    if let Some(s) = cursor.as_str() {
        s.parse::<u64>().ok()
    } else if let Some(n) = cursor.as_u64() {
        Some(n)
    } else {
        None
    }
}

/// Background task that polls a harness watermark URL, parses out a
/// sequence number along the configured JSON path, and advances the
/// associated LatencyTracker. The `watermark_atomic` and `last_ms_atomic`
/// arguments let one prober update the optimistic gauges and another the
/// processed gauges without sharing fields.
///
/// Hits the harness on its own dedicated reqwest client with a hard
/// timeout so a stalled or hung harness can never wedge this loop or
/// back-pressure the hot path.
async fn run_latency_prober(
    label: &'static str,
    metrics: Arc<Metrics>,
    tracker: Arc<LatencyTracker>,
    watermark_atomic: fn(&Metrics) -> &AtomicU64,
    last_ms_atomic: fn(&Metrics) -> &AtomicU64,
    harness_url: String,
    watermark_field: String,
    interval_ms: u64,
    timeout_ms: u64,
) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            warn!("latency prober [{label}]: failed to build client: {err:?}");
            return;
        }
    };
    info!(
        "latency prober [{}] enabled url={} field={} interval_ms={} timeout_ms={}",
        label, harness_url, watermark_field, interval_ms, timeout_ms
    );
    loop {
        match client.get(&harness_url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(body) => {
                    if let Some(wm) = parse_harness_watermark(&body, &watermark_field) {
                        let now_ms = unix_timestamp_ms();
                        tracker.complete_up_to(wm, now_ms);
                        watermark_atomic(&metrics).store(wm, Ordering::Relaxed);
                        last_ms_atomic(&metrics).store(now_ms, Ordering::Relaxed);
                    } else {
                        debug!(
                            "latency prober [{label}]: failed to parse watermark at {}",
                            watermark_field
                        );
                    }
                }
                Err(err) => debug!("latency prober [{label}] body read failed: {err:?}"),
            },
            Ok(resp) => debug!("latency prober [{label}] non-2xx: {}", resp.status()),
            Err(err) => debug!("latency prober [{label}] error: {err:?}"),
        }
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
}

/// Background submitter worker task. Drains PendingSubmits from the shared
/// mpsc channel and dispatches each through `Engine::handle_bg_submit`. One
/// of these is spawned per `bg_submit_workers`. The receiver is shared
/// across workers via a tokio Mutex (mpsc::Receiver is single-consumer by
/// default; the Mutex implements work-stealing).
async fn run_bg_submitter(
    engine: Arc<Engine>,
    rx: Arc<Mutex<mpsc::Receiver<PendingSubmit>>>,
    worker_id: usize,
) {
    info!("bg submitter worker {worker_id} started");
    loop {
        let pending = {
            let mut guard = rx.lock().await;
            guard.recv().await
        };
        match pending {
            Some(pending) => {
                engine.handle_bg_submit(pending).await;
            }
            None => {
                info!("bg submitter worker {worker_id} shutting down (channel closed)");
                return;
            }
        }
    }
}

/// Background task that polls the relayer payer balance via RPC and writes
/// it into the relayer_balance_lamports gauge. Runs at a low cadence
/// (default 30s) so it adds negligible RPC pressure even when the hot path
/// is saturated.
async fn run_balance_poller(
    metrics: Arc<Metrics>,
    rpc: Arc<RpcClient>,
    payer: Pubkey,
    interval_ms: u64,
) {
    if interval_ms == 0 {
        info!("balance poller disabled (interval_ms=0)");
        return;
    }
    info!(
        "balance poller enabled payer={} interval_ms={}",
        payer, interval_ms
    );
    loop {
        match rpc.get_balance(&payer).await {
            Ok(lamports) => {
                metrics
                    .relayer_balance_lamports
                    .store(lamports, Ordering::Relaxed);
                metrics
                    .relayer_balance_last_ms
                    .store(unix_timestamp_ms(), Ordering::Relaxed);
            }
            Err(err) => {
                debug!("balance poll failed: {err:?}");
            }
        }
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
}

fn main() -> Result<()> {
    // Build a custom multi-threaded runtime with a wider worker stack.
    // The default tokio stack is 2 MB which is too small once we embed
    // rust-harness — its `MangoAccountValue`, `BookSide`, `EventQueue`,
    // and friends are large stack-allocated types that overflow the
    // default. The rust-harness test suite uses a 32 MB stack via
    // `run_with_large_stack`; 8 MB is comfortable for production.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .context("build tokio runtime with widened worker stack")?;
    runtime.block_on(async_main())
}

async fn async_main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,service_mango_execution_engine=debug".into()),
        )
        .init();

    let startup_flags = parse_startup_flags()?;
    let config = Arc::new(Config::from_env(startup_flags.enable_health_check)?);
    let rpc = Arc::new(RpcClient::new_with_commitment(
        config.cluster_url.clone(),
        CommitmentConfig::processed(),
    ));
    let metrics = Arc::new(Metrics::default());
    let sequences =
        Arc::new(SequenceStore::new(config.sequence_state_path.clone(), metrics.clone()).await?);
    let blockhashes =
        Arc::new(BlockhashManager::new(rpc.clone(), config.blockhash_refresh_ms).await?);
    let executor = if config.executor_enabled {
        Some(Arc::new(ExecutorState::new(
            config.executor_group.expect("executor group"),
            config.executor_queue.expect("executor queue"),
            load_executor_lanes(config.as_ref())?,
        )))
    } else {
        None
    };
    let secondary_rpc = std::env::var("CTM_RELAYER_SECONDARY_RPC_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|url| {
            info!("secondary RPC enabled: {}", &url[..url.len().min(60)]);
            Arc::new(RpcClient::new_with_commitment(
                url,
                CommitmentConfig::confirmed(),
            ))
        });

    let unique_addresses = Arc::new(StdMutex::new(HashMap::new()));
    let latency_optimistic_tracker = Arc::new(LatencyTracker::new(
        config.latency_samples_capacity,
        config.latency_pending_max_size,
        config.latency_pending_max_age_ms,
    ));
    let latency_processed_tracker = Arc::new(LatencyTracker::new(
        config.latency_samples_capacity,
        config.latency_pending_max_size,
        config.latency_pending_max_age_ms,
    ));
    // Initialize the per-histogram ring capacities. Each histogram lives on
    // the Metrics struct so the hot path can push without coordinating
    // with anything else. We size all of them from the same config knob.
    metrics
        .submit_latency
        .set_capacity(config.latency_samples_capacity);
    metrics
        .stage_parse_latency
        .set_capacity(config.latency_samples_capacity);
    metrics
        .stage_margin_check_latency
        .set_capacity(config.latency_samples_capacity);
    metrics
        .stage_sign_latency
        .set_capacity(config.latency_samples_capacity);
    metrics
        .stage_event_dispatch_latency
        .set_capacity(config.latency_samples_capacity);
    metrics
        .stage_bg_rpc_submit_latency
        .set_capacity(config.latency_samples_capacity);

    // Background submitter pool wiring (Phase 1.1). The mpsc channel feeds
    // a pool of `bg_submit_workers` worker tasks; submit_intent uses the
    // sender's try_send so the hot path never blocks on backpressure.
    let (bg_submit_tx, bg_submit_rx) =
        mpsc::channel::<PendingSubmit>(config.bg_submit_channel_cap);
    let bg_submit_rx = Arc::new(Mutex::new(bg_submit_rx));

    // Phase 2: optional in-process optimistic state. If
    // CTM_RELAYER_LOCAL_STATE=true, fetch a one-time snapshot from the
    // legacy harness and seed an in-process ContinuumStateEngine. The
    // relayer then owns its own copy and never hits the harness HTTP
    // endpoint on the hot path again.
    let local_state = if config.local_state_enabled {
        match bootstrap_local_state(
            &config.local_state_bootstrap_url,
            config.local_state_bootstrap_timeout_ms,
        )
        .await
        {
            Ok(engine) => {
                info!(
                    "CTM_RELAYER_LOCAL_STATE=true — in-process state initialized from {}",
                    config.local_state_bootstrap_url
                );
                Some(Arc::new(PlMutex::new(engine)))
            }
            Err(err) => {
                warn!(
                    "local-state bootstrap failed; falling back to legacy HTTP path: {err:#}"
                );
                None
            }
        }
    } else {
        None
    };

    let engine = Arc::new(Engine {
        config: config.clone(),
        rpc: rpc.clone(),
        secondary_rpc,
        blockhashes,
        sequences,
        metrics: metrics.clone(),
        inflight: Arc::new(Semaphore::new(config.max_inflight)),
        http_client: reqwest::Client::new(),
        harness_readiness: Arc::new(Mutex::new(None)),
        executor: executor.clone(),
        execute_nonce: Arc::new(AtomicU64::new(1)),
        blocked_mango_accounts: Arc::new(Mutex::new(HashMap::new())),
        unique_addresses: unique_addresses.clone(),
        latency_optimistic_tracker: latency_optimistic_tracker.clone(),
        latency_processed_tracker: latency_processed_tracker.clone(),
        bg_submit_tx: bg_submit_tx.clone(),
        state: local_state,
        market_metadata_cache: Arc::new(StdMutex::new(HashMap::new())),
        margin_account_cache: Arc::new(StdMutex::new(HashMap::new())),
    });

    // Spawn the bg submitter worker pool. Each worker shares the receiver
    // via a tokio Mutex (work-stealing). Workers exit when the channel
    // closes (which only happens at shutdown).
    for worker_id in 0..config.bg_submit_workers {
        tokio::spawn(run_bg_submitter(
            engine.clone(),
            bg_submit_rx.clone(),
            worker_id,
        ));
    }
    info!(
        "bg submitter pool spawned: workers={} channel_cap={} max_retries={} retry_base_ms={}",
        config.bg_submit_workers,
        config.bg_submit_channel_cap,
        config.bg_submit_max_retries,
        config.bg_submit_retry_base_ms
    );

    if let Some(http_addr) = config.http_bind_addr {
        tokio::spawn(run_http_server(http_addr, metrics.clone()));
        info!("execution engine HTTP listening on {http_addr}");
    }

    // Background metrics sampler — derives all rate / 60s window / queue
    // depth / unique address / heartbeat-derived health gauges. Single
    // owner of historical state; never contends with the hot path.
    let event_cranker_enabled =
        executor.is_some() && config.executor_perp_consume_interval_ms > 0;
    tokio::spawn(run_metrics_sampler(
        metrics.clone(),
        executor.clone(),
        unique_addresses.clone(),
        latency_optimistic_tracker.clone(),
        latency_processed_tracker.clone(),
        config.executor_stale_threshold_ms,
        config.event_cranker_stale_threshold_ms,
        event_cranker_enabled,
    ));

    // Optional bridge health prober — only runs if a URL is configured.
    if let Some(bridge_url) = config.bridge_health_url.clone() {
        tokio::spawn(run_bridge_health_prober(
            metrics.clone(),
            bridge_url,
            config.health_probe_interval_ms,
            config.bridge_health_probe_timeout_ms,
        ));
    }

    // Optimistic latency prober — drains the optimistic tracker. Each
    // prober is the sole writer for its own pair of (watermark_seq,
    // last_ms) gauges, so they never contend with each other or the hot
    // path.
    if let Some(latency_url) = config.latency_optimistic_url.clone() {
        tokio::spawn(run_latency_prober(
            "optimistic",
            metrics.clone(),
            latency_optimistic_tracker.clone(),
            |m| &m.harness_optimistic_watermark_seq,
            |m| &m.latency_optimistic_prober_last_ms,
            latency_url,
            config.latency_optimistic_field.clone(),
            config.latency_probe_interval_ms,
            config.latency_probe_timeout_ms,
        ));
    }
    // Processed latency prober — drains the processed tracker.
    if let Some(latency_url) = config.latency_processed_url.clone() {
        tokio::spawn(run_latency_prober(
            "processed",
            metrics.clone(),
            latency_processed_tracker.clone(),
            |m| &m.harness_processed_watermark_seq,
            |m| &m.latency_processed_prober_last_ms,
            latency_url,
            config.latency_processed_field.clone(),
            config.latency_probe_interval_ms,
            config.latency_probe_timeout_ms,
        ));
    }

    // Background balance poller — non-blocking, low cadence.
    tokio::spawn(run_balance_poller(
        metrics.clone(),
        rpc.clone(),
        config.payer.pubkey(),
        config.balance_poll_interval_ms,
    ));

    if let Some(executor) = executor {
        tokio::spawn(engine.clone().run_executor(executor.clone()));
        tokio::spawn(engine.clone().run_perp_event_consumer(executor));
    }

    info!(
        "execution engine gRPC listening on {}, program_id={}, ctm={}, payer={}, enable_health_check={}",
        config.bind_addr,
        config.program_id,
        config.ctm.pubkey(),
        config.payer.pubkey(),
        config.enable_health_check,
    );

    Server::builder()
        .add_service(CtmSequencerRelayerServer::new(GrpcService { engine }))
        .serve(config.bind_addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::AnchorSerialize;

    fn make_test_lane(width: usize, hash_byte: u8) -> Lane {
        let remaining_accounts = (0..width)
            .map(|offset| {
                let mut bytes = [hash_byte; 32];
                bytes[31] = offset as u8;
                AccountMeta::new(Pubkey::new_from_array(bytes), false)
            })
            .collect();
        Lane {
            name: format!("lane-{hash_byte}"),
            remaining_accounts,
            hash: [hash_byte; 32],
        }
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn encode_queue_payload(variant: QueuePayloadVariant, body: &[u8]) -> Vec<u8> {
        let mut payload = vec![QUEUE_PAYLOAD_VERSION_V1, variant as u8, 0, 0];
        payload.extend_from_slice(body);
        payload
    }

    #[test]
    fn derive_harness_base_url_uses_relay_ingest_endpoint() {
        assert_eq!(
            derive_harness_base_url(Some("http://127.0.0.1:9091/ingest/relay-intent")),
            Some("http://127.0.0.1:9091".to_string())
        );
        assert_eq!(
            derive_harness_base_url(Some("http://127.0.0.1:9091/other")),
            None
        );
    }

    #[test]
    fn parse_startup_flags_enables_health_check() {
        let flags = parse_startup_flags_from_iter(["--enable-health-check"]).unwrap();
        assert!(flags.enable_health_check);
    }

    #[test]
    fn parse_startup_flags_rejects_unknown_flag() {
        let err = parse_startup_flags_from_iter(["--wat"]).unwrap_err();
        assert!(err.to_string().contains("unknown startup flag"));
    }

    #[test]
    fn detects_transaction_too_large_rpc_error() {
        let err = anyhow!(
            "RpcError(RpcResponseError {{ code: -32602, message: \"base64 encoded solana_transaction::versioned::VersionedTransaction too large: 1676 bytes (max: encoded/raw 1644/1232)\" }})"
        );
        assert!(is_transaction_too_large_error(&err));
    }

    #[test]
    fn ignores_non_size_rpc_errors() {
        let err = anyhow!(
            "RpcError(RpcResponseError {{ code: -32005, message: \"Node is unhealthy\" }})"
        );
        assert!(!is_transaction_too_large_error(&err));
    }

    #[test]
    fn shared_lane_account_width_accepts_uniform_batches() {
        let lanes = vec![make_test_lane(3, 1), make_test_lane(3, 2), make_test_lane(3, 3)];
        assert_eq!(shared_lane_account_width(&lanes).unwrap(), 3);
    }

    #[test]
    fn shared_lane_account_width_rejects_mixed_batches() {
        let lanes = vec![make_test_lane(3, 1), make_test_lane(4, 2)];
        let err = shared_lane_account_width(&lanes).unwrap_err();
        assert!(
            err.to_string()
                .contains("incompatible execute_multi lane account widths")
        );
    }

    #[test]
    fn select_layout_compatible_lanes_skips_incompatible_widths() {
        let lanes = vec![
            make_test_lane(3, 1),
            make_test_lane(3, 2),
            make_test_lane(4, 3),
            make_test_lane(3, 4),
        ];
        let (eligible, unique_hashes, dropped_for_layout) =
            select_layout_compatible_lanes(lanes, &HashMap::new(), 100, 4);

        assert_eq!(eligible.len(), 3);
        assert!(eligible.iter().all(|lane| lane_account_width(lane) == 3));
        assert_eq!(unique_hashes, 4);
        assert_eq!(dropped_for_layout, 1);
    }

    #[test]
    fn select_layout_compatible_lanes_uses_first_unblocked_width() {
        let lanes = vec![
            make_test_lane(5, 1),
            make_test_lane(2, 2),
            make_test_lane(2, 3),
            make_test_lane(4, 4),
        ];
        let mut backoff_snapshot = HashMap::new();
        backoff_snapshot.insert(bytes_to_hex(&[1u8; 32]), 200);

        let (eligible, unique_hashes, dropped_for_layout) =
            select_layout_compatible_lanes(lanes, &backoff_snapshot, 100, 4);

        assert_eq!(eligible.len(), 2);
        assert!(eligible.iter().all(|lane| lane_account_width(lane) == 2));
        assert_eq!(unique_hashes, 3);
        assert_eq!(dropped_for_layout, 1);
    }

    #[test]
    fn decode_margin_check_ops_decodes_perp_place_order_payload() {
        let body = PerpPlaceOrderV2Payload {
            side: Side::Bid,
            price_lots: 100,
            max_base_lots: 7,
            max_quote_lots: 700,
            client_order_id: 1,
            order_type: mango_v4::state::PlaceOrderType::Limit,
            self_trade_behavior: mango_v4::state::SelfTradeBehavior::DecrementTake,
            reduce_only: false,
            expiry_timestamp: 0,
            limit: 10,
        }
        .try_to_vec()
        .unwrap();
        let payload = encode_queue_payload(QueuePayloadVariant::PerpPlaceOrderV2, &body);

        let ops = decode_margin_check_ops(&payload).unwrap().unwrap();
        assert_eq!(
            ops,
            vec![MarginCheckOp::Place {
                side: Side::Bid,
                max_base_lots: 7,
                reduce_only: false,
            }]
        );
    }

    #[test]
    fn decode_margin_check_ops_decodes_batch_cancel_then_place() {
        let cancel = PerpCancelOrderBySlotPayload {
            slot: 3,
            expected_order_id: 55,
        }
        .try_to_vec()
        .unwrap();
        let place = PerpPlaceOrderV2Payload {
            side: Side::Ask,
            price_lots: 101,
            max_base_lots: 9,
            max_quote_lots: 909,
            client_order_id: 9,
            order_type: mango_v4::state::PlaceOrderType::Limit,
            self_trade_behavior: mango_v4::state::SelfTradeBehavior::AbortTransaction,
            reduce_only: true,
            expiry_timestamp: 11,
            limit: 12,
        }
        .try_to_vec()
        .unwrap();
        let mut body = vec![2, 0];
        body.extend_from_slice(&cancel);
        body.push(1);
        body.extend_from_slice(&place);
        let payload = encode_queue_payload(QueuePayloadVariant::PerpBatchIntent, &body);

        let ops = decode_margin_check_ops(&payload).unwrap().unwrap();
        assert_eq!(
            ops,
            vec![
                MarginCheckOp::CancelByOrderId {
                    expected_order_id: 55,
                },
                MarginCheckOp::Place {
                    side: Side::Ask,
                    max_base_lots: 9,
                    reduce_only: true,
                },
            ]
        );
    }

    #[test]
    fn effective_requested_base_lots_clamps_reduce_only_orders() {
        assert_eq!(effective_requested_base_lots(Side::Bid, true, 7, -5), 5);
        assert_eq!(effective_requested_base_lots(Side::Ask, true, 7, 3), 3);
        assert_eq!(effective_requested_base_lots(Side::Bid, true, 7, 2), 0);
        assert_eq!(effective_requested_base_lots(Side::Ask, false, 7, 2), 7);
    }

    #[test]
    fn build_harness_margin_snapshot_filters_orders_per_mango_account() {
        let user_state = HarnessUserState {
            mango_accounts: vec!["acct-a".to_string(), "acct-b".to_string()],
            open_orders: vec![
                HarnessOpenOrder {
                    order_id: "11".to_string(),
                    mango_account: "acct-a".to_string(),
                    market: "7".to_string(),
                    side: "bid".to_string(),
                    base_lots: "4".to_string(),
                },
                HarnessOpenOrder {
                    order_id: "12".to_string(),
                    mango_account: "acct-b".to_string(),
                    market: "7".to_string(),
                    side: "ask".to_string(),
                    base_lots: "6".to_string(),
                },
            ],
            per_market: vec![HarnessUserPerMarket {
                market: "7".to_string(),
                open_order_base_lots_bid: "10".to_string(),
                open_order_base_lots_ask: "10".to_string(),
                base_position_lots: "8".to_string(),
                quote_position_native: "12".to_string(),
            }],
        };

        let snapshot = build_harness_margin_snapshot(Some(&user_state), "acct-a").unwrap();
        let overlay = snapshot.overlays.get(&7).unwrap();
        assert_eq!(snapshot.orders_by_id.len(), 1);
        assert_eq!(overlay.bids_base_lots, 4);
        assert_eq!(overlay.asks_base_lots, 0);
        assert_eq!(overlay.base_position_lots, 0);
        assert_eq!(overlay.quote_position_native, I80F48::ZERO);
    }

    #[test]
    fn validate_harness_health_accepts_fresh_reconciled_payload() {
        let health = HarnessHealthResponse {
            ok: true,
            onchain_read_enabled: true,
            generated_ts_ms: Some(99_500),
            last_reconcile_ts_ms: Some(99_400),
            reconcile_interval_ms: Some(10_000),
            reconcile_markets_with_drift: 0,
        };

        assert!(validate_harness_health(&health, 100_000, 30_000).is_ok());
    }

    #[test]
    fn validate_harness_health_rejects_stale_reconcile_timestamp() {
        let health = HarnessHealthResponse {
            ok: true,
            onchain_read_enabled: true,
            generated_ts_ms: Some(90_000),
            last_reconcile_ts_ms: Some(60_000),
            reconcile_interval_ms: Some(10_000),
            reconcile_markets_with_drift: 0,
        };

        let err = validate_harness_health(&health, 100_000, 30_000).unwrap_err();
        assert!(err.contains("last_reconcile_ts_ms"));
    }

    #[test]
    fn harness_market_has_drift_detects_nonzero_diff_fields() {
        let clean = HarnessMarketDrift {
            replay_open_orders: 2,
            onchain_open_orders: 2,
            replay_best_bid: Some("100".to_string()),
            onchain_best_bid: Some("100".to_string()),
            replay_best_ask: Some("101".to_string()),
            onchain_best_ask: Some("101".to_string()),
            bid_base_lots_abs_diff: "0".to_string(),
            ask_base_lots_abs_diff: "0".to_string(),
        };
        let drifted = HarnessMarketDrift {
            bid_base_lots_abs_diff: "3".to_string(),
            ..clean.clone()
        };

        assert!(!harness_market_has_drift(&clean));
        assert!(harness_market_has_drift(&drifted));
    }

    fn queue_item_offset(sequence: u64) -> usize {
        EXECUTION_QUEUE_CTM_ITEMS_OFFSET
            + (sequence as usize % EXECUTION_QUEUE_CTM_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE
    }

    fn liquidity_item_offset(head: u32) -> usize {
        EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET
            + (head as usize % EXECUTION_QUEUE_LIQUIDITY_CAPACITY) * EXECUTION_QUEUE_ITEM_SIZE
    }

    #[test]
    fn inspect_queue_head_uses_correct_count_offsets() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 2);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 11);
        write_u64(&mut data, EXECUTION_QUEUE_LIQUIDITY_DELAY_SLOTS_OFFSET, 25);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 2);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        let item_offset = queue_item_offset(10);
        write_u64(
            &mut data,
            item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            10,
        );
        data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        data[item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
            ..item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
            .copy_from_slice(&[7u8; 32]);

        let head = inspect_queue_head(&data);
        assert_eq!(head.count, 2);
        assert_eq!(head.ctm_count, 2);
        assert_eq!(head.liquidity_count, 0);
        assert_eq!(head.next_sequence, 10);
        assert_eq!(head.head_accounts_hash, Some([7u8; 32]));
        assert_eq!(head.reason, "ctm_pending");
    }

    #[test]
    fn inspect_next_enqueue_sequence_finds_first_hole() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 2);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 12);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 2);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        for sequence in [10_u64, 12_u64] {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        }

        assert_eq!(inspect_next_enqueue_sequence(&data), 11);
    }

    #[test]
    fn inspect_queue_head_falls_back_to_liquidity_when_ctm_head_missing() {
        let mut data =
            vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET + EXECUTION_QUEUE_ITEM_SIZE];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 2);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 10);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_HEAD_OFFSET, 0);

        let ctm_offset = queue_item_offset(10);
        write_u64(
            &mut data,
            ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            11,
        );
        data[ctm_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;

        let liq_offset = liquidity_item_offset(0);
        write_u64(
            &mut data,
            liq_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            77,
        );
        data[liq_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 1;
        data[liq_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        data[liq_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
            ..liq_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
            .copy_from_slice(&[9u8; 32]);

        let head = inspect_queue_head(&data);
        assert_eq!(head.reason, "liquidity_pending");
        assert_eq!(head.source, Some(QueueHeadSource::Liquidity));
        assert_eq!(head.head_accounts_hash, Some([9u8; 32]));
        assert_eq!(head.ctm_sequence, Some(11));
        assert_eq!(head.ctm_status, Some(1));
    }

    #[test]
    fn inspect_queue_head_reports_ctm_sequence_mismatch() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 10);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);

        let item_offset = queue_item_offset(10);
        write_u64(
            &mut data,
            item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            11,
        );
        data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;

        let head = inspect_queue_head(&data);
        assert_eq!(head.reason, "ctm_sequence_mismatch");
        assert_eq!(head.source, None);
        assert_eq!(head.head_accounts_hash, None);
        assert_eq!(head.ctm_sequence, Some(11));
    }

    #[test]
    fn inspect_queue_head_reports_gap_for_empty_ctm_slot() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 12);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 13);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);

        let head = inspect_queue_head(&data);
        assert_eq!(head.reason, "ctm_gap_or_empty_slot");
        assert_eq!(head.source, None);
        assert_eq!(head.ctm_sequence, Some(0));
        assert_eq!(head.ctm_kind, Some(0));
        assert_eq!(head.ctm_status, Some(0));
    }

    #[test]
    fn inspect_gap_recovery_batch_returns_first_contiguous_pending_span_after_gap() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 3);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 12);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 16);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 3);

        for sequence in 14_u64..=16 {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        }

        let head = inspect_queue_head(&data);
        assert!(head.is_ctm_gap_state());
        assert_eq!(
            inspect_gap_recovery_batch(&data, &head, 8),
            vec![14, 15, 16]
        );
    }

    #[test]
    fn inspect_gap_recovery_batch_respects_batch_cap() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 4);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 20);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 25);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 4);

        for sequence in 22_u64..=25 {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        }

        let head = inspect_queue_head(&data);
        assert_eq!(inspect_gap_recovery_batch(&data, &head, 2), vec![22, 23]);
    }

    #[test]
    fn inspect_near_head_lane_hashes_collects_unique_hashes_in_head_order() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 6);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 15);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 6);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        let hashes = [
            [0xAA; 32], [0xBB; 32], [0xAA; 32], [0xCC; 32], [0xBB; 32], [0xDD; 32],
        ];
        for (index, sequence) in (10_u64..=15).enumerate() {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
            data[item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                ..item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
                .copy_from_slice(&hashes[index]);
        }

        let head = inspect_queue_head(&data);
        assert_eq!(
            inspect_near_head_lane_hashes(&data, &head, 16, 4),
            vec![[0xAA; 32], [0xBB; 32], [0xCC; 32], [0xDD; 32]]
        );
        assert_eq!(
            inspect_near_head_lane_hashes(&data, &head, 16, 3),
            vec![[0xAA; 32], [0xBB; 32], [0xCC; 32]]
        );
    }

    #[test]
    fn inspect_near_head_lane_hashes_stops_at_first_non_contiguous_ctm_slot() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 3);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 20);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 23);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 3);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        for (sequence, hash) in [
            (20_u64, [0x11; 32]),
            (21_u64, [0x22; 32]),
            (23_u64, [0x33; 32]),
        ] {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
            data[item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                ..item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
                .copy_from_slice(&hash);
        }

        let head = inspect_queue_head(&data);
        assert_eq!(
            inspect_near_head_lane_hashes(&data, &head, 16, 4),
            vec![[0x11; 32], [0x22; 32]]
        );
    }

    #[test]
    fn inspect_queue_head_reports_liquidity_status_mismatch() {
        let mut data =
            vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET + EXECUTION_QUEUE_ITEM_SIZE];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 0);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 0);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 0);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_HEAD_OFFSET, 0);

        let liq_offset = liquidity_item_offset(0);
        write_u64(
            &mut data,
            liq_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            77,
        );
        data[liq_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 1;
        data[liq_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 3;

        let head = inspect_queue_head(&data);
        assert_eq!(head.reason, "liquidity_status_mismatch");
        assert_eq!(head.source, None);
        assert_eq!(head.head_accounts_hash, None);
    }

    #[test]
    fn sequence_cursor_reuses_failed_hole() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        let second = cursor.reserve(101);
        cursor.commit_success(first, 102);
        assert_eq!(first, 0);
        assert_eq!(second, 1);

        cursor.reset_after_failure(second, 0);
        assert_eq!(cursor.reserve(104), 1);

        cursor.observe_queue_floor(5);
        assert_eq!(cursor.next_sequence, 5);
        assert!(cursor.pending.keys().all(|sequence| *sequence >= 5));
    }

    #[test]
    fn sequence_cursor_keeps_submitted_head_pending_until_queue_proves_absence() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        let second = cursor.reserve(101);
        cursor.commit_success(first, 102);
        cursor.commit_success(second, 103);

        assert!(!cursor.observe_queue_floor(0));
        assert_eq!(cursor.reserve(2_501), 2);
        assert_eq!(
            cursor.pending.get(&0).map(|state| state.phase),
            Some(PendingSequencePhase::Submitted)
        );
    }

    #[test]
    fn sequence_cursor_counts_only_submitted_and_reuses_recyclable_sequence() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        let second = cursor.reserve(101);
        let third = cursor.reserve(102);
        assert_eq!((first, second, third), (0, 1, 2));

        cursor.commit_success(first, 110);
        cursor.commit_success(third, 111);
        assert_eq!(cursor.submitted_depth_from(0), 2);

        cursor.reset_after_failure(third, 0);
        assert_eq!(cursor.submitted_depth_from(0), 1);
        assert_eq!(cursor.reserve(200), 2);
    }

    #[test]
    fn sequence_cursor_mark_submitted_promotes_reserved_without_double_counting() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        let second = cursor.reserve(101);
        assert_eq!((first, second), (0, 1));
        assert_eq!(cursor.submitted_depth_from(0), 0);

        assert!(cursor.mark_submitted(first, 110));
        assert_eq!(cursor.submitted_depth_from(0), 1);
        assert!(!cursor.mark_submitted(first, 120));
        assert_eq!(cursor.submitted_depth_from(0), 1);
    }

    #[test]
    fn sequence_cursor_observe_queue_floor_drops_old_pending_and_recyclable_sequences() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        let second = cursor.reserve(101);
        let third = cursor.reserve(102);
        cursor.commit_success(first, 110);
        cursor.commit_success(second, 111);
        cursor.reset_after_failure(third, 0);
        assert!(cursor.recyclable.contains(&2));

        assert!(cursor.observe_queue_floor(2));
        assert_eq!(cursor.next_sequence, 2);
        assert!(!cursor.pending.contains_key(&0));
        assert!(!cursor.pending.contains_key(&1));
        assert!(cursor.recyclable.contains(&2));
        assert_eq!(cursor.reserve(200), 2);
    }

    #[test]
    fn inspect_queue_sequence_presence_detects_pending_slot() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 10);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);

        let item_offset = queue_item_offset(10);
        write_u64(
            &mut data,
            item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            10,
        );
        data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;

        assert_eq!(
            inspect_queue_sequence_presence(&data, 10),
            QueueSequencePresence::Pending
        );
        assert_eq!(
            inspect_queue_sequence_presence(&data, 9),
            QueueSequencePresence::PastFloor
        );
        assert_eq!(
            inspect_queue_sequence_presence(&data, 11),
            QueueSequencePresence::Absent
        );
    }

    // ── 4A. Queue Inspection ────────────────────────────────────────────

    #[test]
    fn inspect_queue_head_empty_queue() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 0);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 5);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 5);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 0);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        let head = inspect_queue_head(&data);
        assert_eq!(head.count, 0);
        assert_eq!(head.reason, "empty");
        assert_eq!(head.source, None);
        assert_eq!(head.head_accounts_hash, None);
    }

    #[test]
    fn inspect_queue_head_wraparound_boundary() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        let boundary_seq: u64 = (EXECUTION_QUEUE_CTM_CAPACITY as u64) - 1; // 1023 typically
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(
            &mut data,
            EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET,
            boundary_seq,
        );
        write_u64(
            &mut data,
            EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET,
            boundary_seq,
        );
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        let item_offset = queue_item_offset(boundary_seq);
        write_u64(
            &mut data,
            item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            boundary_seq,
        );
        data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        data[item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
            ..item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
            .copy_from_slice(&[0xAB; 32]);

        let head = inspect_queue_head(&data);
        assert_eq!(head.reason, "ctm_pending");
        assert_eq!(head.source, Some(QueueHeadSource::Ctm));
        assert_eq!(head.head_accounts_hash, Some([0xAB; 32]));
        assert_eq!(head.next_sequence, boundary_seq);
    }

    #[test]
    fn inspect_queue_head_both_ctm_and_liquidity_pending_ctm_wins() {
        let mut data =
            vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET + EXECUTION_QUEUE_ITEM_SIZE];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 2);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 10);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_HEAD_OFFSET, 0);

        // CTM item at sequence 10
        let ctm_offset = queue_item_offset(10);
        write_u64(
            &mut data,
            ctm_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            10,
        );
        data[ctm_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[ctm_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        data[ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
            ..ctm_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
            .copy_from_slice(&[0xCC; 32]);

        // Liquidity item at head 0
        let liq_offset = liquidity_item_offset(0);
        write_u64(
            &mut data,
            liq_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            50,
        );
        data[liq_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 1;
        data[liq_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        data[liq_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
            ..liq_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
            .copy_from_slice(&[0xDD; 32]);

        let head = inspect_queue_head(&data);
        assert_eq!(head.source, Some(QueueHeadSource::Ctm));
        assert_eq!(head.reason, "ctm_pending");
        assert_eq!(head.head_accounts_hash, Some([0xCC; 32]));
    }

    #[test]
    fn inspect_next_enqueue_sequence_no_gaps_returns_max_plus_1() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 3);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 12);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 3);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        // Fill all slots 10, 11, 12 as pending
        for sequence in 10_u64..=12 {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
        }

        assert_eq!(inspect_next_enqueue_sequence(&data), 13);
    }

    #[test]
    fn inspect_next_enqueue_sequence_all_gaps_returns_first_hole() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 5);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 10);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        // No items are actually pending (all slots empty / status=0)
        // So the first hole is at sequence 5 itself
        assert_eq!(inspect_next_enqueue_sequence(&data), 5);
    }

    // ── 4B. Sequence Cursor ─────────────────────────────────────────────

    #[test]
    fn sequence_cursor_recycled_before_increment() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        assert_eq!(first, 0);

        // Fail the first sequence so it becomes recyclable
        cursor.reset_after_failure(first, 0);
        assert!(cursor.recyclable.contains(&0));

        // Next reserve should reuse the recycled sequence, not allocate a new one
        let reused = cursor.reserve(200);
        assert_eq!(reused, 0);
        assert!(cursor.recyclable.is_empty());
    }

    #[test]
    fn sequence_cursor_floor_above_all_pending() {
        let mut cursor = SequenceCursor::default();
        let _a = cursor.reserve(100);
        let _b = cursor.reserve(101);
        let _c = cursor.reserve(102);
        cursor.commit_success(_a, 110);
        cursor.commit_success(_b, 111);
        cursor.commit_success(_c, 112);

        // Floor above all pending sequences drops them all
        assert!(cursor.observe_queue_floor(10));
        assert_eq!(cursor.next_sequence, 10);
        assert!(cursor.pending.is_empty());
    }

    #[test]
    fn sequence_cursor_submitted_depth_excludes_reserved() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);
        let second = cursor.reserve(101);
        let third = cursor.reserve(102);

        // Only submit first and third
        cursor.commit_success(first, 110);
        cursor.commit_success(third, 112);
        // second remains Reserved

        // submitted_depth should count only Submitted entries (first and third)
        assert_eq!(cursor.submitted_depth_from(0), 2);

        // Verify that second is still Reserved and not counted
        assert_eq!(
            cursor.pending.get(&second).map(|s| s.phase),
            Some(PendingSequencePhase::Reserved)
        );
    }

    #[test]
    fn sequence_cursor_multiple_failures_recyclable() {
        let mut cursor = SequenceCursor::default();
        let a = cursor.reserve(100);
        let b = cursor.reserve(101);
        let c = cursor.reserve(102);
        assert_eq!((a, b, c), (0, 1, 2));

        // Fail all three
        cursor.reset_after_failure(a, 0);
        cursor.reset_after_failure(b, 0);
        cursor.reset_after_failure(c, 0);
        assert_eq!(cursor.recyclable.len(), 3);

        // All three should be reused in order (BTreeSet is sorted)
        let r1 = cursor.reserve(200);
        let r2 = cursor.reserve(201);
        let r3 = cursor.reserve(202);
        assert_eq!((r1, r2, r3), (0, 1, 2));
        assert!(cursor.recyclable.is_empty());
    }

    #[test]
    fn sequence_cursor_mark_submitted_idempotent() {
        let mut cursor = SequenceCursor::default();
        let first = cursor.reserve(100);

        // First mark_submitted should succeed
        assert!(cursor.mark_submitted(first, 110));
        assert_eq!(
            cursor.pending.get(&first).map(|s| s.phase),
            Some(PendingSequencePhase::Submitted)
        );

        // Second mark_submitted on same sequence should return false
        assert!(!cursor.mark_submitted(first, 120));
    }

    // ── 4C. Lane Matching ───────────────────────────────────────────────

    #[test]
    fn inspect_queue_sequence_presence_absent_beyond_max_seen() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 1);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 15);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 1);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        // Place item at sequence 10 to satisfy ctm_count > 0
        let item_offset = queue_item_offset(10);
        write_u64(
            &mut data,
            item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
            10,
        );
        data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
        data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;

        // Sequence well beyond max_seen + CTM_CAPACITY should be Absent
        let far_sequence = 10 + EXECUTION_QUEUE_CTM_CAPACITY as u64 + 100;
        assert_eq!(
            inspect_queue_sequence_presence(&data, far_sequence),
            QueueSequencePresence::Absent
        );
    }

    #[test]
    fn inspect_no_lane_match_batch_collects_consecutive_matching_hashes() {
        let mut data = vec![0u8; EXECUTION_QUEUE_LIQUIDITY_ITEMS_OFFSET];
        write_u32(&mut data, EXECUTION_QUEUE_COUNT_OFFSET, 4);
        write_u64(&mut data, EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET, 10);
        write_u64(&mut data, EXECUTION_QUEUE_MAX_SEEN_SEQUENCE_OFFSET, 13);
        write_u32(&mut data, EXECUTION_QUEUE_CTM_COUNT_OFFSET, 4);
        write_u32(&mut data, EXECUTION_QUEUE_LIQUIDITY_COUNT_OFFSET, 0);

        for sequence in 10_u64..=13 {
            let item_offset = queue_item_offset(sequence);
            write_u64(
                &mut data,
                item_offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET,
                sequence,
            );
            data[item_offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET] = 0;
            data[item_offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET] = 1;
            let hash = if sequence <= 12 {
                [0xAA; 32]
            } else {
                [0xBB; 32]
            };
            data[item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                ..item_offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32]
                .copy_from_slice(&hash);
        }

        let head = inspect_queue_head(&data);
        assert_eq!(head.reason, "ctm_pending");
        assert_eq!(
            inspect_no_lane_match_batch(&data, &head, 8),
            vec![10, 11, 12]
        );
        assert_eq!(inspect_no_lane_match_batch(&data, &head, 2), vec![10, 11]);
    }
}
