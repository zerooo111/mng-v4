use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use anchor_lang::InstructionData;
use mango_v4::{
    instructions::CtmEnvelope,
    state::{
        EXECUTION_QUEUE_COUNT_OFFSET, EXECUTION_QUEUE_HEAD_OFFSET,
        EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET, EXECUTION_QUEUE_ITEM_KIND_OFFSET,
        EXECUTION_QUEUE_ITEM_SIZE, EXECUTION_QUEUE_ITEM_STATUS_OFFSET,
        EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET, EXECUTION_QUEUE_ITEMS_OFFSET,
        EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET,
    },
};
use serde::{Deserialize, Serialize};
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::hash::hashv;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    message::v0::Message as MessageV0,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    signer::keypair::read_keypair_file,
    sysvar,
    transaction::VersionedTransaction,
};
use tokio::{
    fs,
    sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore},
    task::JoinHandle,
    time::timeout,
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

#[derive(Clone)]
struct Config {
    cluster_url: String,
    bind_addr: SocketAddr,
    http_bind_addr: Option<SocketAddr>,
    payer: Arc<Keypair>,
    ctm: Arc<Keypair>,
    program_id: Pubkey,
    executor_group: Option<Pubkey>,
    executor_queue: Option<Pubkey>,
    executor_lane_config_path: Option<PathBuf>,
    sequence_state_path: PathBuf,
    min_execute_slot_offset: u64,
    verify_user_signature: bool,
    blockhash_refresh_ms: u64,
    skip_preflight: bool,
    queue_wait_timeout_ms: u64,
    max_inflight: usize,
    prioritization_fee: u64,
    event_sink_url: Option<String>,
    executor_enabled: bool,
    executor_interval_ms: u64,
    executor_max_items: u16,
    executor_prioritization_fee: u64,
    executor_skip_preflight: bool,
    executor_match_head_only: bool,
    executor_failure_threshold: u64,
    executor_failure_backoff_ms: u64,
    executor_include_legacy_fixed_hash: bool,
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
        let payer = Arc::new(read_keypair_env("CTM_RELAYER_PAYER_KEYPAIR", "MB_PAYER_KEYPAIR")?);
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
        let sequence_state_path = PathBuf::from(
            std::env::var("CTM_RELAYER_SEQUENCE_STATE_PATH")
                .unwrap_or_else(|_| "/tmp/ctm-sequences.json".into()),
        );
        let min_execute_slot_offset = parse_u64_env("CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET", 1)?;
        let verify_user_signature =
            parse_bool_env("CTM_RELAYER_VERIFY_USER_SIGNATURE", true);
        let blockhash_refresh_ms = parse_u64_env("CTM_RELAYER_BLOCKHASH_CACHE_MS", 250)?;
        let skip_preflight = matches!(
            std::env::var("CTM_RELAYER_SUBMIT_MODE").ok().as_deref(),
            Some("fast")
        );
        let queue_wait_timeout_ms = parse_u64_env("CTM_RELAYER_QUEUE_WAIT_TIMEOUT_MS", 500)?;
        let max_inflight = parse_u64_env("CTM_RELAYER_MAX_INFLIGHT", 128)? as usize;
        let prioritization_fee = parse_u64_env("CTM_RELAYER_PRIORITIZATION_FEE", 0)?;
        let event_sink_url = std::env::var("CTM_RELAYER_EVENT_SINK_URL")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let executor_enabled = parse_bool_env("EXECUTION_QUEUE_ENGINE_ENABLED", true)
            && executor_group.is_some()
            && executor_queue.is_some();
        let executor_interval_ms = parse_u64_env("EXECUTION_QUEUE_CRANK_INTERVAL_MS", 250)?;
        let executor_max_items = parse_u64_env("EXECUTION_QUEUE_CRANK_MAX_ITEMS", 8)? as u16;
        let executor_prioritization_fee =
            parse_u64_env("EXECUTION_QUEUE_CRANK_PRIORITIZATION_FEE", 0)?;
        let executor_skip_preflight =
            parse_bool_env("EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT", true);
        let executor_match_head_only =
            parse_bool_env("EXECUTION_QUEUE_CRANK_MATCH_HEAD_ONLY", true);
        let executor_failure_threshold =
            parse_u64_env("EXECUTION_QUEUE_CRANK_LANE_FAILURE_THRESHOLD", 3)?;
        let executor_failure_backoff_ms =
            parse_u64_env("EXECUTION_QUEUE_CRANK_LANE_FAILURE_BACKOFF_MS", 10_000)?;
        let executor_include_legacy_fixed_hash =
            parse_bool_env("EXECUTION_QUEUE_CRANK_INCLUDE_LEGACY_FIXED_HASH", true);

        Ok(Self {
            cluster_url,
            bind_addr,
            http_bind_addr,
            payer,
            ctm,
            program_id,
            executor_group,
            executor_queue,
            executor_lane_config_path,
            sequence_state_path,
            min_execute_slot_offset,
            verify_user_signature,
            blockhash_refresh_ms,
            skip_preflight,
            queue_wait_timeout_ms,
            max_inflight,
            prioritization_fee,
            event_sink_url,
            executor_enabled,
            executor_interval_ms,
            executor_max_items,
            executor_prioritization_fee,
            executor_skip_preflight,
            executor_match_head_only,
            executor_failure_threshold,
            executor_failure_backoff_ms,
            executor_include_legacy_fixed_hash,
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
    execute_attempts: AtomicU64,
    execute_sent: AtomicU64,
    execute_errors: AtomicU64,
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

    fn render(&self) -> String {
        let count = self.submit_count.load(Ordering::Relaxed);
        let total_ms = self.submit_total_ms.load(Ordering::Relaxed);
        let avg = if count == 0 {
            0.0
        } else {
            total_ms as f64 / count as f64
        };
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
        ]
        .join("\n")
    }
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

struct ExecutorState {
    group: Pubkey,
    execution_queue: Pubkey,
    lanes: Arc<RwLock<HashMap<String, Lane>>>,
    failure_counts: Arc<Mutex<HashMap<String, u64>>>,
    backoff_until_ms: Arc<Mutex<HashMap<String, u64>>>,
}

impl ExecutorState {
    fn new(group: Pubkey, execution_queue: Pubkey, lanes: HashMap<String, Lane>) -> Self {
        Self {
            group,
            execution_queue,
            lanes: Arc::new(RwLock::new(lanes)),
            failure_counts: Arc::new(Mutex::new(HashMap::new())),
            backoff_until_ms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn register_dynamic_lane(
        &self,
        lane_name: String,
        remaining_accounts: &[AccountMeta],
        include_legacy: bool,
    ) {
        let mut guard = self.lanes.write().await;
        for lane in expand_lane_variants(
            lane_name,
            remaining_accounts,
            self.group,
            self.execution_queue,
            include_legacy,
        ) {
            let lane_hash = bytes_to_hex(&lane.hash);
            let lane_name = lane.name.clone();
            guard.insert(lane_hash.clone(), lane);
            debug!(
                "executor registered lane name={} hash={} total_lanes={}",
                lane_name,
                lane_hash,
                guard.len(),
            );
        }
    }

    async fn lanes_snapshot(&self) -> Vec<Lane> {
        self.lanes.read().await.values().cloned().collect()
    }
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
    sequences: Arc<Mutex<HashMap<String, u64>>>,
    flush_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    metrics: Arc<Metrics>,
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
            sequences: Arc::new(Mutex::new(sequences)),
            flush_task: Arc::new(Mutex::new(None)),
            metrics,
        })
    }

    async fn reserve(&self, key: &str) -> u64 {
        let next = {
            let mut guard = self.sequences.lock().await;
            let current = guard.get(key).copied().unwrap_or(0);
            guard.insert(key.to_string(), current + 1);
            current
        };
        self.schedule_flush().await;
        next
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
            let snapshot = { sequences.lock().await.clone() };
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
    blockhashes: Arc<BlockhashManager>,
    sequences: Arc<SequenceStore>,
    metrics: Arc<Metrics>,
    inflight: Arc<Semaphore>,
    http_client: reqwest::Client,
    executor: Option<Arc<ExecutorState>>,
}

impl Engine {
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

        self.metrics.observe_submit(started.elapsed(), result.is_ok());
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
        let group = parse_pubkey(&request.group)?;
        let execution_queue = parse_pubkey(&request.execution_queue)?;
        let user_owner = parse_pubkey(&request.user_owner)?;
        let mango_account = parse_pubkey(&request.mango_account)?;
        let remaining_accounts = parse_remaining_accounts(&request.remaining_accounts)?;
        let user_signature = parse_signature_bytes(&request.user_signature)?;
        let chain = self.blockhashes.snapshot().await;
        let min_execute_slot = if request.min_execute_slot == 0 {
            chain.slot + self.config.min_execute_slot_offset
        } else {
            request.min_execute_slot
        };
        let expires_at_slot = request.expires_at_slot;

        let sequence_key = format!("{}:{}", group, request.market);
        let sequence = self.sequences.reserve(&sequence_key).await;

        let payload_hash = hashv(&[&request.payload]).to_bytes();
        let accounts_hash =
            hash_execution_queue_accounts_for_ctm_enqueue(group, execution_queue, &remaining_accounts);
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
            ctm_signature.as_ref().try_into().map_err(|_| {
                Status::internal("ctm signature length was not 64 bytes")
            })?,
        );
        let enqueue_instruction = build_enqueue_instruction(
            self.config.program_id,
            group,
            execution_queue,
            &remaining_accounts,
            envelope.clone(),
            request.payload.clone(),
        );

        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(900_000),
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
        let mut tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[self.config.payer.as_ref()],
        )
        .map_err(internal_status)?;
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: self.config.skip_preflight,
            preflight_commitment: Some(CommitmentConfig::processed().commitment),
            ..RpcSendTransactionConfig::default()
        };
        let tx_signature = self
            .rpc
            .send_transaction_with_config(&tx, send_cfg)
            .await
            .map_err(rpc_status)?;
        tx.signatures[0] = tx_signature;

        if let Some(executor) = &self.executor {
            if executor.group == group && executor.execution_queue == execution_queue {
                executor
                    .register_dynamic_lane(
                        format!(
                            "dynamic-{}-{}",
                            request.market,
                            sequence
                        ),
                        &remaining_accounts,
                        self.config.executor_include_legacy_fixed_hash,
                    )
                    .await;
            }
        }

        self.maybe_emit_event(&request, &envelope, &tx_signature).await;

        Ok(SubmitIntentResponse {
            sequence,
            tx_signature: tx_signature.to_string(),
            user_intent_message: user_intent_message.to_vec(),
            ctm_envelope_message: ctm_envelope_message.to_vec(),
        })
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
            "execution engine executor enabled for group={}, queue={}, max_items={}, interval_ms={}",
            executor.group,
            executor.execution_queue,
            self.config.executor_max_items,
            self.config.executor_interval_ms,
        );

        loop {
            if let Err(err) = self.execute_once(&executor).await {
                self.metrics.execute_errors.fetch_add(1, Ordering::Relaxed);
                warn!("executor loop error: {err:?}");
            }
            tokio::time::sleep(Duration::from_millis(self.config.executor_interval_ms)).await;
        }
    }

    async fn execute_once(&self, executor: &Arc<ExecutorState>) -> Result<()> {
        let accounts = self
            .rpc
            .get_account(&executor.execution_queue)
            .await?;
        let queue_account = accounts;

        let head = inspect_queue_head(&queue_account.data);
        if head.count == 0 {
            return Ok(());
        }

        let lanes = executor.lanes_snapshot().await;
        if lanes.is_empty() {
            debug!(
                "executor skipped: queue_count={} next_sequence={} reason=no_lanes",
                head.count,
                head.next_sequence,
            );
            return Ok(());
        }

        let candidate_lanes = if self.config.executor_match_head_only {
            match head.head_accounts_hash {
                Some(hash) => {
                    let matched: Vec<Lane> =
                        lanes.into_iter().filter(|lane| lane.hash == hash).collect();
                    if matched.is_empty() {
                        debug!(
                            "executor skipped: queue_count={} next_sequence={} reason=no_lane_match head_hash={}",
                            head.count,
                            head.next_sequence,
                            bytes_to_hex(&hash),
                        );
                        return Ok(());
                    }
                    matched
                }
                None => {
                    debug!(
                        "executor skipped: queue_count={} next_sequence={} reason=no_head_hash",
                        head.count,
                        head.next_sequence,
                    );
                    return Ok(());
                }
            }
        } else {
            lanes
        };

        for lane in candidate_lanes {
            let lane_key = bytes_to_hex(&lane.hash);
            let blocked_until = {
                let backoff = executor.backoff_until_ms.lock().await;
                backoff.get(&lane_key).copied().unwrap_or(0)
            };
            if blocked_until > unix_timestamp_ms() {
                continue;
            }

            self.metrics.execute_attempts.fetch_add(1, Ordering::Relaxed);
            match self.send_execute(&lane, executor).await {
                Ok(signature) => {
                    self.metrics.execute_sent.fetch_add(1, Ordering::Relaxed);
                    executor.failure_counts.lock().await.insert(lane_key.clone(), 0);
                    executor.backoff_until_ms.lock().await.remove(&lane_key);
                    info!(
                        "executor sent lane={} sequence={} queue_count={} tx={}",
                        lane.name,
                        head.next_sequence,
                        head.count,
                        signature,
                    );
                    return Ok(());
                }
                Err(err) => {
                    let next_fails = {
                        let mut failures = executor.failure_counts.lock().await;
                        let next = failures.get(&lane_key).copied().unwrap_or(0) + 1;
                        failures.insert(lane_key.clone(), next);
                        next
                    };
                    if next_fails >= self.config.executor_failure_threshold {
                        executor
                            .backoff_until_ms
                            .lock()
                            .await
                            .insert(
                                lane_key.clone(),
                                unix_timestamp_ms() + self.config.executor_failure_backoff_ms,
                            );
                    }
                    warn!(
                        "executor lane={} failed attempt={} err={err:?}",
                        lane.name,
                        next_fails,
                    );
                }
            }
        }

        Ok(())
    }

    async fn send_execute(
        &self,
        lane: &Lane,
        executor: &Arc<ExecutorState>,
    ) -> Result<Signature> {
        let chain = self.blockhashes.snapshot().await;
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(900_000),
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
            ..RpcSendTransactionConfig::default()
        };
        let signature = self
            .rpc
            .send_transaction_with_config(&tx, send_cfg)
            .await?;
        Ok(signature)
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

fn parse_remaining_accounts(
    accounts: &[AccountMetaProto],
) -> Result<Vec<AccountMeta>, Status> {
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
    let current_accounts = normalize_lane_runtime_flags(remaining_accounts);
    let mut lanes = vec![Lane {
        name: lane_name.clone(),
        hash: hash_execution_queue_accounts(&current_accounts),
        remaining_accounts: current_accounts,
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

fn load_executor_lanes(config: &Config) -> Result<HashMap<String, Lane>> {
    let mut lanes = HashMap::new();
    let (Some(group), Some(execution_queue)) = (config.executor_group, config.executor_queue) else {
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
    let parsed: Vec<LaneConfigFile> =
        serde_json::from_str(&raw).with_context(|| format!("invalid lane config {}", path.display()))?;
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
    accounts.push(AccountMeta::new_readonly(program_id, false));
    Instruction {
        program_id,
        accounts,
        data: mango_v4::instruction::ExecutionQueueExecute { max_items }.data(),
    }
}

struct QueueHead {
    count: u32,
    next_sequence: u64,
    head_accounts_hash: Option<[u8; 32]>,
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
            queue_data[EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET..EXECUTION_QUEUE_NEXT_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        )
    } else {
        0
    };
    if count == 0 {
        return QueueHead {
            count,
            next_sequence,
            head_accounts_hash: None,
        };
    }

    let head = if queue_data.len() >= EXECUTION_QUEUE_HEAD_OFFSET + 4 {
        u32::from_le_bytes(
            queue_data[EXECUTION_QUEUE_HEAD_OFFSET..EXECUTION_QUEUE_HEAD_OFFSET + 4]
                .try_into()
                .unwrap_or([0; 4]),
        ) as usize
    } else {
        0
    };
    let capacity = queue_data
        .len()
        .saturating_sub(EXECUTION_QUEUE_ITEMS_OFFSET)
        / EXECUTION_QUEUE_ITEM_SIZE;
    if capacity == 0 {
        return QueueHead {
            count,
            next_sequence,
            head_accounts_hash: None,
        };
    }
    let mut logical_index = 0usize;
    while logical_index < count as usize {
        let physical_index = (head + logical_index) % capacity;
        let offset = EXECUTION_QUEUE_ITEMS_OFFSET + physical_index * EXECUTION_QUEUE_ITEM_SIZE;
        if offset + EXECUTION_QUEUE_ITEM_SIZE > queue_data.len() {
            break;
        }
        let sequence = u64::from_le_bytes(
            queue_data
                [offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET..offset + EXECUTION_QUEUE_ITEM_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap_or([0; 8]),
        );
        let kind = queue_data[offset + EXECUTION_QUEUE_ITEM_KIND_OFFSET];
        let status = queue_data[offset + EXECUTION_QUEUE_ITEM_STATUS_OFFSET];
        if status == 1 && kind == 0 && sequence == next_sequence {
            let mut hash = [0u8; 32];
            hash.copy_from_slice(
                &queue_data[offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET
                    ..offset + EXECUTION_QUEUE_ITEM_ACCOUNTS_HASH_OFFSET + 32],
            );
            return QueueHead {
                count,
                next_sequence,
                head_accounts_hash: Some(hash),
            };
        }
        logical_index += 1;
    }

    QueueHead {
        count,
        next_sequence,
        head_accounts_hash: None,
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

fn rpc_status(err: impl std::fmt::Display) -> Status {
    Status::deadline_exceeded(err.to_string())
}

async fn run_http_server(bind_addr: SocketAddr, metrics: Arc<Metrics>) {
    let metrics_filter = warp::any().map(move || metrics.clone());
    let healthz = warp::path!("healthz").map(|| warp::reply::json(&serde_json::json!({ "ok": true })));
    let metrics_route = warp::path!("metrics")
        .and(metrics_filter)
        .map(|metrics: Arc<Metrics>| warp::reply::with_header(metrics.render(), "content-type", "text/plain; version=0.0.4"));
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
    let sequences = Arc::new(SequenceStore::new(
        config.sequence_state_path.clone(),
        metrics.clone(),
    ).await?);
    let blockhashes = Arc::new(
        BlockhashManager::new(rpc.clone(), config.blockhash_refresh_ms).await?,
    );
    let executor = if config.executor_enabled {
        Some(Arc::new(ExecutorState::new(
            config.executor_group.expect("executor group"),
            config.executor_queue.expect("executor queue"),
            load_executor_lanes(config.as_ref())?,
        )))
    } else {
        None
    };
    let engine = Arc::new(Engine {
        config: config.clone(),
        rpc,
        blockhashes,
        sequences,
        metrics: metrics.clone(),
        inflight: Arc::new(Semaphore::new(config.max_inflight)),
        http_client: reqwest::Client::new(),
        executor: executor.clone(),
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
