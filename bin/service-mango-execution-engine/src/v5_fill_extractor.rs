//! Parses perp fill events out of reveal transaction logs and emits
//! structured `perp_fill` event lines to the relay event log. The
//! harness's trade-store engine consumes those events to populate
//! `/state/trades/*`.
//!
//! The current mango-v4 fork emits `PerpTakerTradeLog` (one per taker
//! execution) and `FilledPerpOrderLog` (marker per filled maker order).
//! We surface one `perp_fill` event per `PerpTakerTradeLog`, since it
//! carries the taker account, side, base/quote lot totals and fees —
//! the fields `/trades` needs. The older `FillLog{,V2,V3}` variants are
//! kept as fallbacks in case older program builds are observed.

use anchor_lang::{AnchorDeserialize, Discriminator};
use base64::Engine as _;
use mango_v4::logs::{FillLog, FillLogV2, FillLogV3, PerpTakerTradeLog};
use serde_json::json;
use solana_client::{
    nonblocking::rpc_client::RpcClient, rpc_config::RpcTransactionConfig,
};
use solana_sdk::{commitment_config::CommitmentConfig, signature::Signature};
use solana_transaction_status::UiTransactionEncoding;
use std::{path::PathBuf, sync::Arc};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct ExtractedFill {
    pub market_index: u16,
    pub taker_side: u8,
    pub timestamp: u64,
    pub seq_num: u64,
    pub maker: String,
    pub maker_client_order_id: u64,
    pub taker: String,
    pub taker_client_order_id: u64,
    pub price_lots: i64,
    pub base_lots: i64,
    pub group: String,
    pub maker_out: bool,
    pub taker_fees_paid: i128,
}

/// Scan `logMessages` for `Program data: <base64>` lines and decode
/// any perp-fill-class events we know how to emit.
///
/// Priority order:
/// 1. `PerpTakerTradeLog` — the canonical taker execution record on the
///    current program; emits one `perp_fill` per taker trade.
/// 2. `FillLogV3` / `FillLogV2` / `FillLog` — legacy per-fill events
///    retained for compatibility with older program builds.
pub fn parse_fills_from_logs(logs: &[String]) -> Vec<ExtractedFill> {
    let mut out = Vec::new();
    for line in logs {
        let b64 = match line.strip_prefix("Program data: ") {
            Some(s) => s.trim(),
            None => continue,
        };
        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.len() < 8 {
            continue;
        }
        let (disc, body) = bytes.split_at(8);
        if disc == PerpTakerTradeLog::DISCRIMINATOR {
            if let Ok(t) = PerpTakerTradeLog::try_from_slice(body) {
                // Price-per-base-lot derivation: the taker grabbed
                // `total_base_lots_taken` base lots for
                // `total_quote_lots_taken` quote lots (fees excluded).
                // Dividing gives the volume-weighted average price in
                // quote-lots-per-base-lot, matching the `price_lots`
                // semantics used by older FillLog events.
                let base_lots = t.total_base_lots_taken;
                let quote_lots = t.total_quote_lots_taken;
                let price_lots = if base_lots != 0 {
                    quote_lots / base_lots
                } else {
                    0
                };
                out.push(ExtractedFill {
                    market_index: t.perp_market_index,
                    taker_side: t.taker_side,
                    // PerpTakerTradeLog doesn't carry a timestamp/seq_num;
                    // callers stamp ts_ms at emit time.
                    timestamp: 0,
                    seq_num: 0,
                    maker: String::new(),
                    maker_client_order_id: 0,
                    taker: t.mango_account.to_string(),
                    taker_client_order_id: 0,
                    price_lots,
                    base_lots,
                    group: t.mango_group.to_string(),
                    maker_out: false,
                    taker_fees_paid: t.taker_fees_paid,
                });
            }
        } else if disc == FillLogV3::DISCRIMINATOR {
            if let Ok(f) = FillLogV3::try_from_slice(body) {
                out.push(ExtractedFill {
                    market_index: f.market_index,
                    taker_side: f.taker_side,
                    timestamp: f.timestamp,
                    seq_num: f.seq_num,
                    maker: f.maker.to_string(),
                    maker_client_order_id: f.maker_client_order_id,
                    taker: f.taker.to_string(),
                    taker_client_order_id: f.taker_client_order_id,
                    price_lots: f.price,
                    base_lots: f.quantity,
                    group: f.mango_group.to_string(),
                    maker_out: f.maker_out,
                    taker_fees_paid: 0,
                });
            }
        } else if disc == FillLogV2::DISCRIMINATOR {
            if let Ok(f) = FillLogV2::try_from_slice(body) {
                out.push(ExtractedFill {
                    market_index: f.market_index,
                    taker_side: f.taker_side,
                    timestamp: f.timestamp,
                    seq_num: f.seq_num,
                    maker: f.maker.to_string(),
                    maker_client_order_id: f.maker_client_order_id,
                    taker: f.taker.to_string(),
                    taker_client_order_id: f.taker_client_order_id,
                    price_lots: f.price,
                    base_lots: f.quantity,
                    group: f.mango_group.to_string(),
                    maker_out: f.maker_out,
                    taker_fees_paid: 0,
                });
            }
        } else if disc == FillLog::DISCRIMINATOR {
            if let Ok(f) = FillLog::try_from_slice(body) {
                out.push(ExtractedFill {
                    market_index: f.market_index,
                    taker_side: f.taker_side,
                    timestamp: f.timestamp,
                    seq_num: f.seq_num,
                    maker: f.maker.to_string(),
                    maker_client_order_id: 0,
                    taker: f.taker.to_string(),
                    taker_client_order_id: f.taker_client_order_id,
                    price_lots: f.price,
                    base_lots: f.quantity,
                    group: f.mango_group.to_string(),
                    maker_out: f.maker_out,
                    taker_fees_paid: 0,
                });
            }
        }
    }
    out
}

/// Append a single `perp_fill` event line to the relay event log. O_APPEND +
/// write of <4 KiB is atomic under POSIX, so this can run concurrently with
/// the main event-log writer without interleaving.
fn append_perp_fill_line(path: &PathBuf, sig: &Signature, fill: &ExtractedFill) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let quote_lots = (fill.price_lots as i128) * (fill.base_lots as i128);
    let line = json!({
        "event_type": "perp_fill",
        "ts_ms": now_ms,
        "market": fill.market_index.to_string(),
        "market_index": fill.market_index,
        "group": fill.group,
        "taker_side": fill.taker_side,
        "price_lots": fill.price_lots,
        "base_lots": fill.base_lots,
        "quote_lots": quote_lots.to_string(),
        "maker": fill.maker,
        "taker": fill.taker,
        "maker_client_order_id": fill.maker_client_order_id,
        "taker_client_order_id": fill.taker_client_order_id,
        "maker_out": fill.maker_out,
        "taker_fees_paid": fill.taker_fees_paid.to_string(),
        "timestamp": fill.timestamp,
        "seq_num": fill.seq_num,
        "tx_signature": sig.to_string(),
    });
    let mut buf = line.to_string();
    buf.push('\n');
    use std::io::Write as _;
    match std::fs::OpenOptions::new().append(true).create(true).open(path) {
        Ok(mut f) => {
            if let Err(err) = f.write_all(buf.as_bytes()) {
                warn!(target: "v5_fill", error = %err, path = %path.display(), "perp_fill append failed");
            }
        }
        Err(err) => {
            warn!(target: "v5_fill", error = %err, path = %path.display(), "perp_fill log open failed");
        }
    }
}

/// Fetch the reveal tx, parse fills out of its logs, and emit one
/// `perp_fill` event per fill to the event log. Runs as a background task
/// so it never blocks the reveal hot path; at most one tx fetch per
/// landed reveal batch.
pub async fn extract_and_emit_fills(
    rpc: Arc<RpcClient>,
    sig: Signature,
    expected_market_index: u16,
    event_log_path: Option<Arc<PathBuf>>,
) {
    let Some(path) = event_log_path else {
        return;
    };
    let cfg = RpcTransactionConfig {
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
        encoding: Some(UiTransactionEncoding::Json),
    };
    let mut tx = None;
    for attempt in 0..5u32 {
        match rpc.get_transaction_with_config(&sig, cfg).await {
            Ok(t) => {
                tx = Some(t);
                break;
            }
            Err(_) if attempt < 4 => {
                tokio::time::sleep(std::time::Duration::from_millis(300 * (attempt as u64 + 1))).await;
            }
            Err(err) => {
                warn!(
                    target: "v5_fill",
                    %sig,
                    error = %err,
                    "getTransaction failed after retries; fills may be missed"
                );
                return;
            }
        }
    }
    let tx = match tx {
        Some(t) => t,
        None => return,
    };
    let meta = match tx.transaction.meta.as_ref() {
        Some(m) => m,
        None => return,
    };
    let logs: Vec<String> = match &meta.log_messages {
        solana_transaction_status::option_serializer::OptionSerializer::Some(v) => v.clone(),
        _ => return,
    };
    let fills = parse_fills_from_logs(&logs);
    for fill in fills {
        if fill.market_index != expected_market_index {
            continue;
        }
        append_perp_fill_line(&path, &sig, &fill);
    }
}
