use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anchor_lang::{AnchorDeserialize, InstructionData};
use anyhow::{anyhow, Context, Result};
use mango_v4::{
    error::MangoError,
    instructions::{CtmEnvelope, ExecutionQueueConfigParams, PerpPlaceOrderV2Payload},
    state::{
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
    sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{debug, info, warn};
use warp::Filter;

pub mod proto {
    tonic::include_proto!("ctmsequencer");
}

use proto::{
    ctm_sequencer_relayer_server::{CtmSequencerRelayer, CtmSequencerRelayerServer},
    AccountMeta as AccountMetaProto, SubmitIntentRequest, SubmitIntentResponse,
};

const SPL_MEMO_PROGRAM_ID: Pubkey = pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

#[derive(Clone)]
struct Config {
    cluster_url: String,
    bind_addr: SocketAddr,
    http_bind_addr: Option<SocketAddr>,
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
    executor_pending_timeout_ms: u64,
    executor_status_poll_ms: u64,
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
    /// Maximum consecutive failures for the same sequence before the executor
    /// proactively fires an admin-drop tx. This handles the case where the
    /// normal auto-drop path (via signature status + log inspection) fails
    /// due to RPC rate-limiting or other transient errors.
    executor_max_sequence_failures: u32,
    /// When a head item has had no matching lane for longer than this many
    /// Solana slots (~400ms each), the executor admin-drops it as a health
    /// failure rather than spinning forever.  Set 0 to disable.
    executor_no_lane_match_drop_slots: u64,
}

impl Config {
    fn from_env() -> Result<Self> {
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
        let executor_pending_timeout_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_PENDING_TIMEOUT_MS", 500)?;
        let executor_status_poll_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_STATUS_POLL_MS", 10)?;
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
        let executor_max_sequence_failures =
            parse_u64_env("EXECUTION_QUEUE_CRANK_MAX_SEQUENCE_FAILURES", 5)? as u32;
        let executor_no_lane_match_drop_slots =
            parse_u64_env("EXECUTION_QUEUE_CRANK_NO_LANE_MATCH_DROP_SLOTS", 2)?;

        Ok(Self {
            cluster_url,
            bind_addr,
            http_bind_addr,
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
            executor_pending_timeout_ms,
            executor_status_poll_ms,
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
            executor_max_sequence_failures,
            executor_no_lane_match_drop_slots,
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
        ]
        .join("\n")
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
    group: String,
    execution_queue: String,
    market: String,
    sequence: String,
    user_owner: String,
    remaining_accounts: Vec<RelayEventAccountMeta>,
}

#[derive(Clone, Copy)]
struct CachedChainState {
    blockhash: solana_sdk::hash::Hash,
    slot: u64,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingHeadDispatch {
    sequence: u64,
    accounts_hash: Option<[u8; 32]>,
    lane_hash: [u8; 32],
    sent_at_ms: u64,
    last_status_check_ms: u64,
    no_advance_recorded: bool,
    targeted: bool,
    signature: Signature,
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

impl Engine {
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

        None
    }

    fn terminal_head_failure_label(reason: TerminalHeadFailureReason) -> &'static str {
        match reason {
            TerminalHeadFailureReason::ExpiredOrder => "expired_order",
            TerminalHeadFailureReason::WrongProgramOwner => "account_owned_by_wrong_program",
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
        if queue_state.head.reason != "ctm_pending" || queue_state.head.next_sequence != sequence {
            return Err(anyhow!(
                "queue head moved before admin drop: expected_sequence={} current_sequence={} reason={}",
                sequence,
                queue_state.head.next_sequence,
                queue_state.head.reason
            ));
        }
        let expired_batch = self.inspect_expired_head_batch(&queue_account.data, sequence);
        let sequences_to_drop = if expired_batch.sequences.is_empty() {
            vec![sequence]
        } else {
            expired_batch.sequences
        };

        let mut instructions = vec![build_execution_queue_configure_instruction(
            self.config.program_id,
            executor.group,
            executor.execution_queue,
            self.config.executor_admin.pubkey(),
            &queue_state,
            true,
        )];
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
            skip_preflight: false,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: Some(0),
            ..RpcSendTransactionConfig::default()
        };
        let signature = self.rpc.send_transaction_with_config(&tx, send_cfg).await?;
        self.await_signature_result(signature, self.config.executor_admin_tx_timeout_ms)
            .await?;
        executor.pending_dispatches.lock().await.clear();
        executor.last_inspect_ms.store(0, Ordering::Relaxed);
        *executor.cached_head.lock().await = None;
        info!(
            "executor admin recovery completed start_sequence={} dropped={} reason={} tx={}",
            sequence,
            sequences_to_drop.len(),
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
        let started = Instant::now();
        let permit = self.acquire_permit().await?;
        self.metrics.inflight.fetch_add(1, Ordering::Relaxed);

        let result = self.submit_intent_inner(request).await;

        self.metrics.inflight.fetch_sub(1, Ordering::Relaxed);
        drop(permit);

        self.metrics
            .observe_submit(started.elapsed(), result.is_ok());
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
        let parse_started = Instant::now();
        let group = parse_pubkey(&request.group)?;
        let execution_queue = parse_pubkey(&request.execution_queue)?;
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
                if !head_available && degraded_head_limit > 0 && queue_count >= degraded_head_limit
                {
                    return Err(Status::resource_exhausted(format!(
                        "execution queue head-gap backpressure count={} degraded_limit={} gap_span={}",
                        queue_count, degraded_head_limit, gap_span
                    )));
                }
            }
        }
        self.ensure_harness_ready(&request.market).await?;
        let user_owner = parse_pubkey(&request.user_owner)?;
        let mango_account = parse_pubkey(&request.mango_account)?;
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
                ComputeBudgetInstruction::set_compute_unit_price(self.config.prioritization_fee),
            );
        }

        let message = MessageV0::try_compile(
            &self.config.payer.pubkey(),
            &instructions,
            &[],
            chain.blockhash,
        )
        .map_err(internal_status)?;
        let mut tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[self.config.payer.as_ref()],
        )
        .map_err(internal_status)?;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            max_retries: self.config.submit_rpc_max_retries,
            ..RpcSendTransactionConfig::default()
        };
        let send_started = Instant::now();
        let tx_signature = match self.rpc.send_transaction_with_config(&tx, send_cfg).await {
            Ok(signature) => signature,
            Err(err) => {
                self.metrics.observe_submit_stages(
                    parse_elapsed,
                    prepare_elapsed,
                    send_started.elapsed(),
                );
                self.recover_sequence_after_submit_error(&sequence_key, sequence, execution_queue)
                    .await;
                if is_execution_queue_duplicate_sequence_error(&err) {
                    return Err(Status::aborted(
                        "execution queue duplicate sequence; relayer cursor reconciled",
                    ));
                }
                return Err(rpc_status(err));
            }
        };
        self.metrics
            .observe_submit_stages(parse_elapsed, prepare_elapsed, send_started.elapsed());
        tx.signatures[0] = tx_signature;
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

        self.maybe_emit_event(&request, &envelope, &tx_signature)
            .await;

        Ok(SubmitIntentResponse {
            sequence,
            tx_signature: tx_signature.to_string(),
            user_intent_message: user_intent_message.to_vec(),
            ctm_envelope_message: ctm_envelope_message.to_vec(),
        })
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

    async fn maybe_emit_event(
        &self,
        request: &SubmitIntentRequest,
        envelope: &CtmEnvelope,
        tx_signature: &Signature,
    ) {
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
            ts_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            group: request.group.clone(),
            execution_queue: request.execution_queue.clone(),
            market: request.market.clone(),
            sequence: envelope.sequence.to_string(),
            kind: envelope.kind as u32,
            payload_b64: {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&request.payload)
            },
            remaining_accounts,
            min_execute_slot: envelope.min_execute_slot.to_string(),
            expires_at_slot: envelope.expires_at_slot.to_string(),
            user_owner: request.user_owner.clone(),
            mango_account: request.mango_account.clone(),
            enqueue_tx_signature: tx_signature.to_string(),
        };
        let client = self.http_client.clone();
        tokio::spawn(async move {
            if let Err(err) = client.post(url).json(&body).send().await {
                warn!("event sink request failed: {err:?}");
            }
        });
    }

    async fn run_executor(self: Arc<Self>, executor: Arc<ExecutorState>) {
        info!(
            "execution engine executor enabled for group={}, queue={}, max_items={}, interval_ms={}, busy_interval_ms={}, head_lock_ms={}, pending_timeout_ms={}, status_poll_ms={}, max_pending_txs={}, pipeline_max_per_head={}, same_head_send_interval_ms={}, match_head_only={}, safe_speculative={}",
            executor.group,
            executor.execution_queue,
            self.config.executor_max_items,
            self.config.executor_interval_ms,
            self.config.executor_busy_interval_ms,
            self.config.executor_head_lock_ms,
            self.config.executor_pending_timeout_ms,
            self.config.executor_status_poll_ms,
            self.config.executor_max_pending_txs,
            self.config.executor_pipeline_max_per_head,
            self.config.executor_same_head_send_interval_ms,
            self.config.executor_match_head_only,
            self.config.executor_safe_speculative,
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
                                    "executor tx confirmed but queue head unchanged sequence={} sig={}",
                                    retained[index].sequence, signature
                                );
                                // Treat confirmed-no-advance as a persistent failure;
                                // if the head doesn't move after N confirmed txs, the
                                // lane accounts likely have a runtime flag mismatch.
                                self.maybe_auto_drop_sequence_after_failure_threshold(
                                    executor,
                                    retained[index].sequence,
                                    "confirmed_no_advance",
                                )
                                .await;
                            }
                        }
                        Some(status) => {
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
        let inspect_interval_ms = 500u64; // Only fetch queue account every 500ms

        let head = if now_ms.saturating_sub(last_inspect) >= inspect_interval_ms {
            let accounts = self.rpc.get_account(&executor.execution_queue).await?;
            let head = inspect_queue_head(&accounts.data);
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
        let can_pipeline_same_head = same_head_pending_count
            < self.config.executor_pipeline_max_per_head
            && pending_snapshot.len() < self.config.executor_max_pending_txs
            && now_ms.saturating_sub(latest_same_head_send_ms)
                >= self.config.executor_same_head_send_interval_ms;
        // Only suppress if we've hit the per-head pipeline cap AND the total
        // pending count is high.  Previously this blocked ALL sends when the
        // current head had pending txs, idling the cranker even when it could
        // be targeting a new head.  Now we allow sending as long as the global
        // pending budget has room — enabling back-to-back txs for consecutive
        // heads without waiting for confirmations.
        if same_head_pending_count >= self.config.executor_pipeline_max_per_head
            && pending_snapshot.len() >= self.config.executor_max_pending_txs
        {
            self.metrics
                .execute_send_suppressed_pending
                .fetch_add(1, Ordering::Relaxed);
            return Ok(ExecuteLoopOutcome::Busy);
        }
        if same_head_pending_count >= self.config.executor_pipeline_max_per_head {
            // Hit per-head cap but global budget has room — the head may have
            // already advanced by the time the next inspect fires.  Re-inspect
            // immediately instead of sleeping.
            executor.last_inspect_ms.store(0, Ordering::Relaxed);
            return Ok(ExecuteLoopOutcome::Busy);
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

        let (candidate_lanes, speculative_mode) = if self.config.executor_match_head_only {
            match head.head_accounts_hash {
                Some(hash) => {
                    let mut matched: Vec<Lane> = lanes
                        .iter()
                        .cloned()
                        .filter(|lane| lane.hash == hash)
                        .collect();
                    if matched.is_empty() {
                        let _ = self.refresh_dynamic_lanes_from_event_log(executor).await;
                        matched = executor
                            .lanes_snapshot()
                            .await
                            .into_iter()
                            .filter(|lane| lane.hash == hash)
                            .collect();
                    }
                    if matched.is_empty() {
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
                    // Include additional lanes so execute_multi can batch
                    // consecutive items with different hashes in one tx.
                    // With HLT (precomputed lane hashes), per-item hash matching
                    // is O(L) byte comparisons, so more lanes are affordable.
                    // Cap at 5 to stay within tx size limit (1232 bytes).
                    let head_account_count = matched
                        .first()
                        .map(|l| l.remaining_accounts.len())
                        .unwrap_or(0);
                    let mut seen: std::collections::HashSet<[u8; 32]> =
                        matched.iter().map(|l| l.hash).collect();
                    for lane in &lanes {
                        if seen.len() >= 5 {
                            break;
                        }
                        if seen.contains(&lane.hash) {
                            continue;
                        }
                        if lane.remaining_accounts.len() != head_account_count {
                            continue;
                        }
                        seen.insert(lane.hash);
                        matched.push(lane.clone());
                    }
                    (matched, false)
                }
                None => {
                    self.metrics
                        .execute_head_missing
                        .fetch_add(1, Ordering::Relaxed);
                    let speculative_lanes = if gap_skip_mode {
                        select_gap_speculative_lanes(lanes)
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
                        (speculative_lanes, true)
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
            (lanes, true)
        };

        if pending_snapshot.len() >= self.config.executor_max_pending_txs {
            return Ok(ExecuteLoopOutcome::Busy);
        }

        if gap_skip_mode {
            self.metrics
                .execute_attempts
                .fetch_add(1, Ordering::Relaxed);
            self.metrics
                .execute_targeted
                .fetch_add(1, Ordering::Relaxed);
            match self.build_gap_skip_tx(executor, head.next_sequence).await {
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
                            sent_at_ms: now_ms,
                            last_status_check_ms: now_ms,
                            no_advance_recorded: false,
                            targeted: true,
                            signature: signature.clone(),
                        });
                        info!(
                            "executor sent gap-skip sequence={} queue_count={} tx={}",
                            head.next_sequence, head.count, signature,
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

        // Collect eligible lanes (de-dup by hash, skip backed-off lanes)
        let backoff_snapshot = { executor.backoff_until_ms.lock().await.clone() };
        let mut eligible_lanes: Vec<Lane> = Vec::new();
        let mut seen_hashes = std::collections::HashSet::new();
        for lane in candidate_lanes {
            let lane_key = bytes_to_hex(&lane.hash);
            let blocked_until = backoff_snapshot.get(&lane_key).copied().unwrap_or(0);
            if blocked_until > unix_timestamp_ms() {
                continue;
            }
            if !seen_hashes.insert(lane.hash) {
                continue;
            }
            eligible_lanes.push(lane);
            if eligible_lanes.len() >= 5 {
                break; // Cap at 5 lanes to fit within tx size limit (1232 bytes)
            }
        }

        if eligible_lanes.is_empty() {
            return Ok(ExecuteLoopOutcome::Busy);
        }

        if eligible_lanes.len() > 1 {
            info!(
                "executor multi-lane eligible_lanes={} unique_hashes={}",
                eligible_lanes.len(),
                seen_hashes.len(),
            );
        }

        // Check pending budget
        if pending_snapshot.len() >= self.config.executor_max_pending_txs {
            return Ok(ExecuteLoopOutcome::Busy);
        }

        // Build and send ONE multi-lane execute tx
        self.metrics
            .execute_attempts
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .execute_targeted
            .fetch_add(1, Ordering::Relaxed);

        let (tx, send_cfg) = if eligible_lanes.len() > 1 {
            match self
                .build_execute_multi_tx(&eligible_lanes, executor, head.next_sequence)
                .await
            {
                Ok(built) => built,
                Err(err) => {
                    warn!("executor multi-lane build failed, falling back to single: {err:?}");
                    self.build_execute_tx(&eligible_lanes[0], executor, head.next_sequence)
                        .await?
                }
            }
        } else {
            self.build_execute_tx(&eligible_lanes[0], executor, head.next_sequence)
                .await?
        };

        // Dual-send: fire-and-forget to secondary RPC for higher landing probability
        if let Some(secondary) = &self.secondary_rpc {
            let tx_clone = tx.clone();
            let cfg_clone = send_cfg;
            let sec = secondary.clone();
            tokio::spawn(async move {
                let _ = sec.send_transaction_with_config(&tx_clone, cfg_clone).await;
            });
        }

        let send_result = self
            .rpc
            .send_transaction_with_config(&tx, send_cfg)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"));

        match send_result {
            Ok(signature) => {
                self.metrics.execute_sent.fetch_add(1, Ordering::Relaxed);
                if same_head_pending_count > 0 {
                    self.metrics
                        .execute_pipeline_sent
                        .fetch_add(1, Ordering::Relaxed);
                }
                let mut dispatches = executor.pending_dispatches.lock().await;
                dispatches.push(PendingHeadDispatch {
                    sequence: head.next_sequence,
                    accounts_hash: head.head_accounts_hash,
                    lane_hash: eligible_lanes[0].hash,
                    sent_at_ms: now_ms,
                    last_status_check_ms: now_ms,
                    no_advance_recorded: false,
                    targeted: !speculative_mode,
                    signature: signature.clone(),
                });
                info!(
                    "executor sent multi-lane lanes={} sequence={} queue_count={} pending_total={} pending_same_head={} tx={}",
                    eligible_lanes.len(),
                    head.next_sequence,
                    head.count,
                    pending_snapshot.len() + 1,
                    same_head_pending_count + 1,
                    signature,
                );
                return Ok(ExecuteLoopOutcome::Sent);
            }
            Err(err) => {
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
                self.config.executor_max_items,
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
                lane_hashes,
                self.config.executor_max_items,
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
                &[],
                self.config.executor_max_items.max(1),
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
    lane_hashes: Vec<[u8; 32]>,
    max_items: u16,
) -> Instruction {
    let lane_count = lane_accounts.len() as u8;
    let accounts_per_lane = lane_accounts.first().map(|l| l.len()).unwrap_or(0) as u16;
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,service_mango_execution_engine=debug".into()),
        )
        .init();

    let config = Arc::new(Config::from_env()?);
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
            Arc::new(RpcClient::new_with_commitment(url, CommitmentConfig::confirmed()))
        });

    let engine = Arc::new(Engine {
        config: config.clone(),
        rpc,
        secondary_rpc,
        blockhashes,
        sequences,
        metrics: metrics.clone(),
        inflight: Arc::new(Semaphore::new(config.max_inflight)),
        http_client: reqwest::Client::new(),
        harness_readiness: Arc::new(Mutex::new(None)),
        executor: executor.clone(),
        execute_nonce: Arc::new(AtomicU64::new(1)),
    });

    if let Some(http_addr) = config.http_bind_addr {
        tokio::spawn(run_http_server(http_addr, metrics.clone()));
        info!("execution engine HTTP listening on {http_addr}");
    }

    if let Some(executor) = executor {
        tokio::spawn(engine.clone().run_executor(executor));
    }

    info!(
        "execution engine gRPC listening on {}, program_id={}, ctm={}, payer={}",
        config.bind_addr,
        config.program_id,
        config.ctm.pubkey(),
        config.payer.pubkey(),
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

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
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
}
