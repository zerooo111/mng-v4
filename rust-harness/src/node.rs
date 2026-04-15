use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Mutex,
    time::Instant,
};

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::{ContinuumStateEngine, QueueView};

#[napi]
pub struct NativeContinuumStateEngine {
    inner: Mutex<ContinuumStateEngine>,
}

#[napi]
impl NativeContinuumStateEngine {
    #[napi(constructor)]
    pub fn new() -> Self {
        crate::logging::init_native_logging();
        Self {
            inner: Mutex::new(ContinuumStateEngine::new()),
        }
    }

    #[napi(js_name = "bootstrapFromOnchainSnapshotJson")]
    pub fn bootstrap_from_onchain_snapshot_json(&self, snapshot_json: String) -> Result<()> {
        self.with_engine("bootstrap_from_onchain_snapshot_json", |engine| {
            engine.bootstrap_from_onchain_snapshot_json(&snapshot_json)
        })
    }

    #[napi(js_name = "ingestRelayIntentJson")]
    pub fn ingest_relay_intent_json(&self, event_json: String) -> Result<()> {
        self.with_engine("ingest_relay_intent_json", |engine| {
            engine.ingest_relay_intent_json(&event_json)
        })
    }

    #[napi(js_name = "ingestQueueEnqueuedJson")]
    pub fn ingest_queue_enqueued_json(&self, event_json: String) -> Result<()> {
        self.with_engine("ingest_queue_enqueued_json", |engine| {
            engine.ingest_queue_enqueued_json(&event_json)
        })
    }

    #[napi(js_name = "ingestQueueProcessedJson")]
    pub fn ingest_queue_processed_json(&self, event_json: String) -> Result<()> {
        self.with_engine("ingest_queue_processed_json", |engine| {
            engine.ingest_queue_processed_json(&event_json)
        })
    }

    #[napi(js_name = "listDivergencesJson")]
    pub fn list_divergences_json(&self, limit: u32) -> Result<String> {
        self.with_engine("list_divergences_json", |engine| {
            engine.list_divergences_json(limit as usize)
        })
    }

    #[napi(js_name = "listIntentsJson")]
    pub fn list_intents_json(&self) -> Result<String> {
        self.with_engine("list_intents_json", |engine| engine.list_intents_json())
    }

    #[napi(js_name = "findIntentJson")]
    pub fn find_intent_json(
        &self,
        group: String,
        sequence: String,
        kind: u32,
        // Optional v2 sub-queue market index. Pass undefined/null to do a
        // best-effort scan across all markets (legacy single-stream lookup).
        market_index: Option<u32>,
    ) -> Result<Option<String>> {
        let market_index_u16 = market_index.map(|m| m as u16);
        self.with_engine("find_intent_json", |engine| {
            engine.find_intent_json(&group, market_index_u16, &sequence, kind as u8)
        })
    }

    #[napi(js_name = "getValidatedLocalPayloadJson")]
    pub fn get_validated_local_payload_json(
        &self,
        group: String,
        sequence: String,
        kind: u32,
        include_owner_state: bool,
        include_market_state: bool,
        include_market_open_orders: bool,
        // Optional v2 sub-queue market index.
        market_index: Option<u32>,
    ) -> Result<Option<String>> {
        let market_index_u16 = market_index.map(|m| m as u16);
        self.with_engine("get_validated_local_payload_json", |engine| {
            engine.get_validated_local_payload_json(
                &group,
                market_index_u16,
                &sequence,
                kind as u8,
                include_owner_state,
                include_market_state,
                include_market_open_orders,
            )
        })
    }

    #[napi(js_name = "getSnapshotJson")]
    pub fn get_snapshot_json(&self, view: String) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_snapshot_json", |engine| engine.get_snapshot_json(view))
    }

    #[napi(js_name = "getMarketStateJson")]
    pub fn get_market_state_json(&self, market: String, view: String) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_market_state_json", |engine| {
            engine.get_market_state_json(&market, view)
        })
    }

    #[napi(js_name = "getUserStateJson")]
    pub fn get_user_state_json(&self, owner: String, view: String) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_user_state_json", |engine| {
            engine.get_user_state_json(&owner, view)
        })
    }

    #[napi(js_name = "getQueueStateJson")]
    pub fn get_queue_state_json(&self, market: String) -> Result<String> {
        self.with_engine("get_queue_state_json", |engine| {
            engine.get_queue_state_json(&market)
        })
    }

    #[napi(js_name = "getBalancesJson")]
    pub fn get_balances_json(&self, owner: String, view: String) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_balances_json", |engine| {
            engine.get_balances_json(&owner, view)
        })
    }

    #[napi(js_name = "getOrdersJson")]
    pub fn get_orders_json(
        &self,
        market: String,
        owner: Option<String>,
        view: String,
    ) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_orders_json", |engine| {
            engine.get_orders_json(&market, owner.as_deref(), view)
        })
    }

    #[napi(js_name = "getTradesJson")]
    pub fn get_trades_json(&self, market: String, view: String, limit: u32) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_trades_json", |engine| {
            engine.get_trades_json(&market, view, limit as usize)
        })
    }

    #[napi(js_name = "getAllTradesJson")]
    pub fn get_all_trades_json(&self, view: String, limit: u32) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_all_trades_json", |engine| {
            engine.get_all_trades_json(view, limit as usize)
        })
    }

    #[napi(js_name = "getTradesFilteredJson")]
    pub fn get_trades_filtered_json(
        &self,
        market: Option<String>,
        owner: Option<String>,
        view: String,
        limit: u32,
    ) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_trades_filtered_json", |engine| {
            engine.get_trades_filtered_json(
                market.as_deref(),
                owner.as_deref(),
                view,
                limit as usize,
            )
        })
    }

    #[napi(js_name = "getCandlesJson")]
    pub fn get_candles_json(
        &self,
        market: String,
        view: String,
        resolution_sec: u32,
        limit: u32,
    ) -> Result<String> {
        let view = parse_view(&view)?;
        self.with_engine("get_candles_json", |engine| {
            engine.get_candles_json(&market, view, resolution_sec as u64, limit as usize)
        })
    }

    #[napi(js_name = "reportExternalDivergenceJson")]
    pub fn report_external_divergence_json(
        &self,
        reason: String,
        key: String,
        details_json: String,
    ) -> Result<()> {
        let details = serde_json::from_str::<HashMap<String, String>>(&details_json)
            .map_err(|err| Error::from_reason(err.to_string()))?;
        self.with_engine("report_external_divergence_json", |engine| {
            engine.report_external_divergence(&reason, &key, details);
            Ok(())
        })
    }
}

impl NativeContinuumStateEngine {
    fn with_engine<T>(
        &self,
        op_name: &'static str,
        f: impl FnOnce(&mut ContinuumStateEngine) -> crate::Result<T>,
    ) -> Result<T> {
        let started = Instant::now();
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut recovered = poisoned.into_inner();
                *recovered = ContinuumStateEngine::new();
                crate::logging::log_error(
                    "node.with_engine",
                    "rust-harness mutex was poisoned; engine state reset",
                    BTreeMap::from([("operation".to_string(), op_name.to_string())]),
                );
                return Err(Error::from_reason(
                    "rust-harness mutex was poisoned; engine state reset".to_string(),
                ));
            }
        };
        let result = match catch_unwind(AssertUnwindSafe(|| f(&mut guard))) {
            Ok(Ok(value)) => {
                crate::logging::log_call_outcome(
                    op_name,
                    started.elapsed(),
                    "ok",
                    None,
                    BTreeMap::new(),
                );
                Ok(value)
            }
            Ok(Err(err)) => {
                let message = err.to_string();
                crate::logging::log_call_outcome(
                    op_name,
                    started.elapsed(),
                    "error",
                    Some(&message),
                    BTreeMap::new(),
                );
                Err(Error::from_reason(message))
            }
            Err(payload) => {
                *guard = ContinuumStateEngine::new();
                let message = format!(
                    "rust-harness panic recovered: {}; engine state reset",
                    panic_payload_message(payload)
                );
                crate::logging::log_call_outcome(
                    op_name,
                    started.elapsed(),
                    "panic",
                    Some(&message),
                    BTreeMap::new(),
                );
                Err(Error::from_reason(message))
            }
        };
        result
    }
}

fn panic_payload_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn parse_view(view: &str) -> Result<QueueView> {
    match view.trim().to_ascii_lowercase().as_str() {
        "optimistic" => Ok(QueueView::Optimistic),
        "confirmed" => Ok(QueueView::Confirmed),
        _ => Err(Error::from_reason(format!(
            "unsupported queue view: {view}"
        ))),
    }
}
