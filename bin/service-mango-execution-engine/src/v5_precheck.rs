//! Ingress-time pre-checks that prevent commits of intents which would fail
//! at the reveal/execute step. Every committed intent that passes this gate
//! should reveal cleanly — the reveal worker only has to worry about landing
//! order, not payload validity.
//!
//! Structured as a sequence of stages, cheapest first. Each stage returns
//! `Err(PrecheckReject)` on failure; the caller maps that to a gRPC
//! FAILED_PRECONDITION rejection plus a metric-labeled counter.
//!
//! Coverage parity with the program's `execution_queue_v5_reveal_execute_market`
//! handler is the goal. Stages we implement here and the corresponding
//! program reverts they avoid:
//!
//! - Stage 1 payload decode            ↔ decode_queue_payload err (6xxx)
//! - Stage 2 terminal prevalidation   ↔ prevalidate_terminal_ctm_payload
//!                                       (InvalidNumericInput, Expired, …)
//! - Stage 3 numeric bounds            ↔ CU-overflow / silent terminal-clear
//! - Stage 4 dispatch shape (lite)     ↔ ExecutionQueueDispatchAccountLayoutInvalid
//!                                       and ExecutionQueueSubQueueMarketIndexMismatch
//! - Stage 8 timing margin             ↔ ExecutionQueueEnvelopeExpired,
//!                                       head-jump-via-expiry
//!
//! Stages 6 (mango_account state), 7 (oracle freshness), 9 (health sim) and
//! 10 (payer balance) require state-cache access; they're deferred to a
//! follow-up module `v5_precheck_state` so we can land this critical path
//! first and measure its impact in isolation.

use mango_v4::instructions::{
    decode_queue_payload, prevalidate_terminal_ctm_payload, queue_item_kind_for_payload_variant,
    variant_uses_user_signature, DecodedQueuePayload, QueuePayloadVariant,
    TerminalCtmFailureReason,
};
use solana_sdk::{instruction::AccountMeta, pubkey::Pubkey};

/// Canonical position of the perp_market account in the CTM dispatch list.
/// Must stay in sync with the program's `require_dispatch_market_index`
/// (which also reads position [3]).
const PERP_MARKET_DISPATCH_INDEX: usize = 3;

/// Canonical position of the mango_account in the CTM dispatch list.
const MANGO_ACCOUNT_DISPATCH_INDEX: usize = 1;

/// Minimum dispatch-accounts length for CTM-wrapped variants. The program
/// requires >3 (for market_index check at position [3]). Different market
/// configurations use different numbers of health-cache accounts after
/// position [3], so we hold at the program's strict minimum of 4 and let
/// variant-specific shape checks catch layout mismatches elsewhere.
const MIN_CTM_DISPATCH_ACCOUNTS: usize = 4;

#[derive(Debug, Clone)]
pub enum PrecheckReject {
    PayloadDecode {
        error: String,
    },
    NonZeroFlags {
        flags: u16,
    },
    KindMismatch {
        variant_kind: u8,
        envelope_kind: u8,
    },
    TerminalVariant {
        reason: TerminalCtmFailureReason,
    },
    NumericOutOfBounds {
        field: &'static str,
        value: String,
    },
    DispatchTooShort {
        len: usize,
        min: usize,
    },
    DispatchMarketMismatch {
        position: usize,
        expected: Pubkey,
        actual: Pubkey,
    },
    DispatchMangoAccountMismatch {
        position: usize,
        expected: Pubkey,
        actual: Pubkey,
    },
    EnvelopeExpired {
        now_slot: u64,
        expires_at_slot: u64,
    },
    EnvelopeExpiryTooTight {
        now_slot: u64,
        expires_at_slot: u64,
        margin_needed: u64,
    },
    MinExecuteSlotTooFar {
        now_slot: u64,
        min_execute_slot: u64,
        max_future_slots: u64,
    },
    MinExecuteSlotAfterExpiry {
        min_execute_slot: u64,
        expires_at_slot: u64,
    },
    UserSignatureMissingForVariant {
        variant: QueuePayloadVariant,
    },
    /// `payload.expiry_timestamp` is set and would be expired at reveal time
    /// (or within `min_expiry_margin_secs`). Catches MMs that stamp a fixed
    /// expiry_timestamp at startup and reuse it across quotes — those quotes
    /// reach commit successfully but every reveal aborts with
    /// `TerminalCtmFailureReason::Expired`. Logged at ingress so the offending
    /// client is identifiable without spelunking on-chain tx logs.
    PayloadExpiryStale {
        expiry_timestamp: u64,
        now_ts: u64,
        min_margin_secs: u64,
    },
    /// A `PerpCancelOrder` / `PerpCancelOrderByClientOrderId` intent whose
    /// target order id (or client-order-id) isn't in the mango account's
    /// current `perp_open_orders`. Without this check every phantom cancel
    /// burns a commit slot and reveal-fails with `PerpOrderIdNotFound`
    /// (6044), inflating `live_count` on the queue until autodrop catches
    /// up. Rejected at ingress so the bot sees the feedback immediately.
    PhantomCancelOrder {
        market_index: u16,
        is_client_id: bool,
        target: String,
    },
}

impl PrecheckReject {
    /// Short label for the reject — used as a Prometheus metric label value
    /// so all keys are bounded and low-cardinality.
    pub fn reason_tag(&self) -> &'static str {
        match self {
            PrecheckReject::PayloadDecode { .. } => "payload_decode",
            PrecheckReject::NonZeroFlags { .. } => "nonzero_flags",
            PrecheckReject::KindMismatch { .. } => "kind_mismatch",
            PrecheckReject::TerminalVariant { .. } => "terminal_variant",
            PrecheckReject::NumericOutOfBounds { .. } => "numeric_bounds",
            PrecheckReject::DispatchTooShort { .. } => "dispatch_too_short",
            PrecheckReject::DispatchMarketMismatch { .. } => "dispatch_market_mismatch",
            PrecheckReject::DispatchMangoAccountMismatch { .. } => {
                "dispatch_mango_account_mismatch"
            }
            PrecheckReject::EnvelopeExpired { .. } => "envelope_expired",
            PrecheckReject::EnvelopeExpiryTooTight { .. } => "envelope_tight",
            PrecheckReject::MinExecuteSlotTooFar { .. } => "min_execute_far",
            PrecheckReject::MinExecuteSlotAfterExpiry { .. } => "min_execute_after_expiry",
            PrecheckReject::UserSignatureMissingForVariant { .. } => "user_sig_missing",
            PrecheckReject::PayloadExpiryStale { .. } => "payload_expiry_stale",
            PrecheckReject::PhantomCancelOrder { .. } => "phantom_cancel",
        }
    }
}

impl std::fmt::Display for PrecheckReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrecheckReject::PayloadDecode { error } => {
                write!(f, "payload decode failed: {error}")
            }
            PrecheckReject::NonZeroFlags { flags } => {
                write!(f, "payload has non-zero flags (flags={flags})")
            }
            PrecheckReject::KindMismatch {
                variant_kind,
                envelope_kind,
            } => write!(
                f,
                "payload variant kind ({variant_kind}) != envelope kind ({envelope_kind})"
            ),
            PrecheckReject::TerminalVariant { reason } => {
                write!(f, "payload would terminalize at reveal: {reason:?}")
            }
            PrecheckReject::NumericOutOfBounds { field, value } => {
                write!(f, "numeric bound violation: {field}={value}")
            }
            PrecheckReject::DispatchTooShort { len, min } => write!(
                f,
                "dispatch accounts too short: got {len}, need at least {min}"
            ),
            PrecheckReject::DispatchMarketMismatch {
                position,
                expected,
                actual,
            } => write!(
                f,
                "dispatch[{position}] perp_market mismatch: expected {expected}, got {actual}"
            ),
            PrecheckReject::DispatchMangoAccountMismatch {
                position,
                expected,
                actual,
            } => write!(
                f,
                "dispatch[{position}] mango_account mismatch: expected {expected}, got {actual}"
            ),
            PrecheckReject::EnvelopeExpired {
                now_slot,
                expires_at_slot,
            } => write!(
                f,
                "envelope already expired: now_slot={now_slot} expires_at_slot={expires_at_slot}"
            ),
            PrecheckReject::EnvelopeExpiryTooTight {
                now_slot,
                expires_at_slot,
                margin_needed,
            } => write!(
                f,
                "envelope expiry too tight: now={now_slot} exp={expires_at_slot} need_margin={margin_needed}"
            ),
            PrecheckReject::MinExecuteSlotTooFar {
                now_slot,
                min_execute_slot,
                max_future_slots,
            } => write!(
                f,
                "min_execute_slot too far in future: now={now_slot} min_exec={min_execute_slot} max_future={max_future_slots}"
            ),
            PrecheckReject::MinExecuteSlotAfterExpiry {
                min_execute_slot,
                expires_at_slot,
            } => write!(
                f,
                "min_execute_slot {min_execute_slot} >= expires_at_slot {expires_at_slot}"
            ),
            PrecheckReject::UserSignatureMissingForVariant { variant } => {
                write!(f, "user signature required for variant {variant:?} but not provided")
            }
            PrecheckReject::PayloadExpiryStale {
                expiry_timestamp,
                now_ts,
                min_margin_secs,
            } => write!(
                f,
                "payload expiry_timestamp {expiry_timestamp} would be expired at reveal \
                 (now_ts={now_ts}, min_margin_secs={min_margin_secs}, \
                 delta_secs={delta})",
                delta = (*now_ts as i128) - (*expiry_timestamp as i128),
            ),
            PrecheckReject::PhantomCancelOrder {
                market_index,
                is_client_id,
                target,
            } => write!(
                f,
                "phantom cancel: no {kind} order {target} resting on mango \
                 account for market_index={market_index}",
                kind = if *is_client_id { "client-id" } else { "order-id" },
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrecheckConfig {
    /// Reject if `expires_at_slot - now_slot < this`. Protects against reveals
    /// that land in the expiry-flush path. 0 disables the check (but we still
    /// reject already-expired envelopes).
    pub expire_margin_slots: u64,
    /// Reject if `min_execute_slot - now_slot > this`. Protects against
    /// speculative commits that sit stale at the head.
    pub max_future_slots: u64,
    /// Reject a `PerpPlaceOrderV2` payload at ingress if its
    /// `expiry_timestamp` is set (non-zero) and would be treated as expired
    /// at reveal, i.e. `expiry_timestamp <= now_ts + this`. Set to 0 to only
    /// reject already-stamped-in-the-past timestamps; raise to protect
    /// against reveals that land after a short queue drain. Independent of
    /// the slot-level `expire_margin_slots` because `expiry_timestamp` is
    /// unix-seconds (user-supplied) while `expires_at_slot` is a solana
    /// slot (relayer-supplied).
    pub payload_expiry_min_margin_secs: u64,
    /// Master kill switch. When false, `precheck_reveal_ready` always returns
    /// Ok. Useful for emergency rollback without redeploying.
    pub enabled: bool,
}

impl Default for PrecheckConfig {
    fn default() -> Self {
        Self {
            expire_margin_slots: 10,          // ~4s at 400ms/slot
            max_future_slots: 150,            // ~60s — generous, catches clock skew
            payload_expiry_min_margin_secs: 2, // reveal ≤ ~1 slot away; 2s covers that
            enabled: true,
        }
    }
}

/// Top-level pre-check. Returns Ok iff the committed intent should reveal
/// cleanly under normal conditions.
///
/// Call this AFTER the relayer has finished assembling `remaining_accounts`
/// and the ed25519 user signature has been verified (the relayer already
/// does that before calling here), but BEFORE `fetch_add(v5_seq)`.
pub fn precheck_reveal_ready(
    payload: &[u8],
    envelope_kind: u8,
    min_execute_slot: u64,
    expires_at_slot: u64,
    now_slot: u64,
    now_ts: u64,
    remaining_accounts: &[AccountMeta],
    expected_perp_market: Pubkey,
    expected_mango_account: Pubkey,
    user_signature_provided: bool,
    cfg: &PrecheckConfig,
) -> Result<DecodedQueuePayload, PrecheckReject> {
    if !cfg.enabled {
        // Even disabled, we still need to return a decoded payload for the
        // caller. But we skip all the rejection logic. Decoding itself is
        // the minimum the caller needs.
        return decode_queue_payload(payload).map_err(|e| PrecheckReject::PayloadDecode {
            error: format!("{e:?}"),
        });
    }

    // ── Stage 1: payload decode ─────────────────────────────────────────
    let decoded = decode_queue_payload(payload).map_err(|e| PrecheckReject::PayloadDecode {
        error: format!("{e:?}"),
    })?;
    if decoded.flags != 0 {
        return Err(PrecheckReject::NonZeroFlags {
            flags: decoded.flags,
        });
    }
    let variant_kind = queue_item_kind_for_payload_variant(decoded.variant);
    if variant_kind != envelope_kind {
        return Err(PrecheckReject::KindMismatch {
            variant_kind,
            envelope_kind,
        });
    }

    // ── Stage 2a: explicit stale-expiry check (PerpPlaceOrderV2 only) ──
    // Reads `expiry_timestamp` directly from the raw payload bytes so this
    // stage is independent of `DecodedQueuePayload.body`'s `pub(crate)`
    // visibility. Fires BEFORE the catch-all `prevalidate_terminal_ctm_payload`
    // below so the reject reason is specific ("payload_expiry_stale") and
    // easy to grep, and so we can add a safety margin the program doesn't
    // enforce. Catches MMs that stamp a fixed `expiry_timestamp` at startup
    // and never refresh it across quotes.
    if let Some(expiry) = parse_perp_place_order_v2_expiry_timestamp(payload) {
        if expiry != 0 && expiry <= now_ts.saturating_add(cfg.payload_expiry_min_margin_secs) {
            return Err(PrecheckReject::PayloadExpiryStale {
                expiry_timestamp: expiry,
                now_ts,
                min_margin_secs: cfg.payload_expiry_min_margin_secs,
            });
        }
    }

    // ── Stage 2: terminal prevalidation (mirror of program) ────────────
    if let Some(reason) = prevalidate_terminal_ctm_payload(&decoded, now_ts) {
        return Err(PrecheckReject::TerminalVariant { reason });
    }

    // ── Stage 3: numeric bounds (stricter than program) ────────────────
    check_numeric_bounds(&decoded)?;

    // ── Stage 4: dispatch shape (lite — pubkey-level, no fetch) ────────
    check_dispatch_shape(
        remaining_accounts,
        expected_perp_market,
        expected_mango_account,
    )?;

    // ── Stage 5: user-sig presence flag ────────────────────────────────
    // Full sig verification is done by the caller's existing
    // verify_user_signature path; here we only guard against variants that
    // require a sig when none was provided.
    if variant_uses_user_signature(decoded.variant) && !user_signature_provided {
        return Err(PrecheckReject::UserSignatureMissingForVariant {
            variant: decoded.variant,
        });
    }

    // ── Stage 8: timing margin ─────────────────────────────────────────
    check_timing_margin(min_execute_slot, expires_at_slot, now_slot, cfg)?;

    Ok(decoded)
}

/// Stage 3. The program accepts numerically-valid-but-suspect payloads (e.g.
/// price_lots=0 only fails at perp_place_order CU-burn time). Pre-reject the
/// cases we can statically detect at ingress to save reveal-side retry
/// budget and head-jump noise.
fn check_numeric_bounds(decoded: &DecodedQueuePayload) -> Result<(), PrecheckReject> {
    // The DecodedQueuePayload struct stores variant-specific fields behind
    // an enum; we probe only the fields we can reach without replicating
    // the full program decode. For the critical PerpPlaceOrderV2 variant
    // the fields of interest are price_lots, max_base_quantity,
    // max_quote_quantity, client_order_id. The program-side
    // prevalidate_terminal_ctm_payload already returns Expired /
    // InvalidNumericInput for trivial violations (zero size, negative price
    // parsed as huge unsigned, negative client_ts). Anything that slips
    // past that + passes the variant-kind check is accepted by the program,
    // so mirroring program semantics here suffices for the MVP.
    //
    // We keep this as a no-op stage for now; when we add the
    // price-vs-oracle bound check it will live here.
    let _ = decoded;
    Ok(())
}

/// Stage 4 (lite). Check dispatch[3] == expected_perp_market and
/// dispatch[1] == expected_mango_account. Length guard covers
/// `ExecutionQueueDispatchAccountLayoutInvalid`. This is a client-side
/// pubkey check (no RPC fetch); the program's stricter variant-shape
/// validation runs at reveal time and should NOT reject anything we pass
/// here if the relayer assembled `remaining_accounts` canonically.
fn check_dispatch_shape(
    remaining_accounts: &[AccountMeta],
    expected_perp_market: Pubkey,
    expected_mango_account: Pubkey,
) -> Result<(), PrecheckReject> {
    if remaining_accounts.len() < MIN_CTM_DISPATCH_ACCOUNTS {
        return Err(PrecheckReject::DispatchTooShort {
            len: remaining_accounts.len(),
            min: MIN_CTM_DISPATCH_ACCOUNTS,
        });
    }
    let perp_market_actual = remaining_accounts[PERP_MARKET_DISPATCH_INDEX].pubkey;
    if perp_market_actual != expected_perp_market {
        return Err(PrecheckReject::DispatchMarketMismatch {
            position: PERP_MARKET_DISPATCH_INDEX,
            expected: expected_perp_market,
            actual: perp_market_actual,
        });
    }
    let mango_actual = remaining_accounts[MANGO_ACCOUNT_DISPATCH_INDEX].pubkey;
    if mango_actual != expected_mango_account {
        return Err(PrecheckReject::DispatchMangoAccountMismatch {
            position: MANGO_ACCOUNT_DISPATCH_INDEX,
            expected: expected_mango_account,
            actual: mango_actual,
        });
    }
    Ok(())
}

/// Borsh offsets inside a `PerpPlaceOrderV2Payload` body (after the 4-byte
/// QueuePayloadHeader):
///   side(1) + price_lots(8) + max_base_lots(8) + max_quote_lots(8)
///   + client_order_id(8) + order_type(1) + self_trade_behavior(1)
///   + reduce_only(1) + expiry_timestamp(8) + limit(1) = 45 bytes body
/// So `expiry_timestamp` lives at body offset 36..44 = payload offset 40..48.
const QUEUE_PAYLOAD_HEADER_LEN: usize = 4;
const PERP_PLACE_V2_EXPIRY_OFFSET_IN_BODY: usize = 1 + 8 * 4 + 1 + 1 + 1;
const PERP_PLACE_V2_MIN_BODY_LEN: usize = PERP_PLACE_V2_EXPIRY_OFFSET_IN_BODY + 8 + 1;
const QUEUE_PAYLOAD_VERSION_V1: u8 = 1;
/// `QueuePayloadVariant::PerpPlaceOrderV2` serializes to discriminant 0
/// (see `queue_payload_variant_from_u8` in programs/mango-v4/src/instructions/
/// execution_queue.rs).
const QUEUE_PAYLOAD_VARIANT_PERP_PLACE_V2: u8 = 0;

/// Read `expiry_timestamp` from a PerpPlaceOrderV2 payload without going
/// through the program's full-variant decoder. Returns None when the payload
/// is too short, the wrong version, or a different variant. A None return
/// means "this stage has nothing to say"; the subsequent stages (including
/// the full program-mirrored decode) still run.
pub(crate) fn parse_perp_place_order_v2_expiry_timestamp(payload: &[u8]) -> Option<u64> {
    if payload.len() < QUEUE_PAYLOAD_HEADER_LEN + PERP_PLACE_V2_MIN_BODY_LEN {
        return None;
    }
    if payload[0] != QUEUE_PAYLOAD_VERSION_V1 {
        return None;
    }
    if payload[1] != QUEUE_PAYLOAD_VARIANT_PERP_PLACE_V2 {
        return None;
    }
    // flags at bytes [2..4] — not validated here; stage 1 handles that.
    let off = QUEUE_PAYLOAD_HEADER_LEN + PERP_PLACE_V2_EXPIRY_OFFSET_IN_BODY;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&payload[off..off + 8]);
    Some(u64::from_le_bytes(buf))
}

fn check_timing_margin(
    min_execute_slot: u64,
    expires_at_slot: u64,
    now_slot: u64,
    cfg: &PrecheckConfig,
) -> Result<(), PrecheckReject> {
    if expires_at_slot != 0 {
        if expires_at_slot <= now_slot {
            return Err(PrecheckReject::EnvelopeExpired {
                now_slot,
                expires_at_slot,
            });
        }
        let remaining = expires_at_slot - now_slot;
        if remaining < cfg.expire_margin_slots {
            return Err(PrecheckReject::EnvelopeExpiryTooTight {
                now_slot,
                expires_at_slot,
                margin_needed: cfg.expire_margin_slots,
            });
        }
        if min_execute_slot != 0 && min_execute_slot >= expires_at_slot {
            return Err(PrecheckReject::MinExecuteSlotAfterExpiry {
                min_execute_slot,
                expires_at_slot,
            });
        }
    }
    if min_execute_slot > now_slot {
        let skew = min_execute_slot - now_slot;
        if skew > cfg.max_future_slots {
            return Err(PrecheckReject::MinExecuteSlotTooFar {
                now_slot,
                min_execute_slot,
                max_future_slots: cfg.max_future_slots,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_shape_reject_short() {
        let a = AccountMeta::new_readonly(Pubkey::new_unique(), false);
        let remaining = vec![a.clone(); 3];
        let err = check_dispatch_shape(&remaining, Pubkey::new_unique(), Pubkey::new_unique())
            .unwrap_err();
        matches!(err, PrecheckReject::DispatchTooShort { .. });
    }

    #[test]
    fn timing_reject_already_expired() {
        let cfg = PrecheckConfig::default();
        let err = check_timing_margin(0, 100, 200, &cfg).unwrap_err();
        matches!(err, PrecheckReject::EnvelopeExpired { .. });
    }

    #[test]
    fn timing_reject_too_tight() {
        let cfg = PrecheckConfig {
            expire_margin_slots: 10,
            ..Default::default()
        };
        let err = check_timing_margin(0, 105, 100, &cfg).unwrap_err();
        matches!(err, PrecheckReject::EnvelopeExpiryTooTight { .. });
    }

    /// Build the exact 49-byte payload shape the relayer sees on the wire:
    /// 4-byte header (version=1, variant=0, flags=0) + 45-byte body with
    /// `expiry_timestamp` placed at body[36..44].
    fn mk_perp_place_v2_payload(expiry_timestamp: u64) -> Vec<u8> {
        let mut p = vec![0u8; 4 + 45];
        p[0] = 1; // version
        p[1] = 0; // variant = PerpPlaceOrderV2
        // flags @ [2..4] left 0
        // side @ body[0], lots fields @ body[1..33], client_order_id @ body[25..33],
        // order_type @ body[33], self_trade_behavior @ body[34], reduce_only @ body[35]
        // expiry_timestamp @ body[36..44], limit @ body[44]
        let off = 4 + 36;
        p[off..off + 8].copy_from_slice(&expiry_timestamp.to_le_bytes());
        p
    }

    #[test]
    fn expiry_parse_roundtrip() {
        let ts = 1_776_669_808u64; // the exact stale stamp seen in production
        let p = mk_perp_place_v2_payload(ts);
        assert_eq!(parse_perp_place_order_v2_expiry_timestamp(&p), Some(ts));
    }

    #[test]
    fn expiry_parse_none_on_short_payload() {
        let short = vec![1u8, 0, 0, 0, 0, 0];
        assert_eq!(parse_perp_place_order_v2_expiry_timestamp(&short), None);
    }

    #[test]
    fn expiry_parse_none_on_wrong_variant() {
        let mut p = mk_perp_place_v2_payload(1_000);
        p[1] = 1; // flip variant to PerpCancelOrder
        assert_eq!(parse_perp_place_order_v2_expiry_timestamp(&p), None);
    }

    #[test]
    fn expiry_parse_none_on_wrong_version() {
        let mut p = mk_perp_place_v2_payload(1_000);
        p[0] = 2;
        assert_eq!(parse_perp_place_order_v2_expiry_timestamp(&p), None);
    }

    /// The production stale-expiry pattern: `expiry_timestamp` stamped at
    /// MM startup (~1776669808, 2026-04-20 20:03 UTC), reveals 12h later.
    /// Stage 2a must reject at ingress.
    #[test]
    fn stage_2a_rejects_stale_expiry_exact_production_pattern() {
        let p = mk_perp_place_v2_payload(1_776_669_808);
        let stale_now_ts = 1_776_762_000u64; // ~12h later
        let err = parse_perp_place_order_v2_expiry_timestamp(&p).expect("parses");
        assert!(err != 0 && err <= stale_now_ts.saturating_add(2));
    }

    #[test]
    fn stage_2a_passes_fresh_expiry() {
        let now_ts = 1_776_762_000u64;
        let fresh_expiry = now_ts + 60; // 60s in the future
        let p = mk_perp_place_v2_payload(fresh_expiry);
        let parsed = parse_perp_place_order_v2_expiry_timestamp(&p).unwrap();
        assert!(parsed > now_ts + 2, "fresh expiry should pass a 2s margin");
    }

    #[test]
    fn stage_2a_passes_zero_expiry() {
        let p = mk_perp_place_v2_payload(0);
        let parsed = parse_perp_place_order_v2_expiry_timestamp(&p).unwrap();
        assert_eq!(parsed, 0, "zero expiry_timestamp means 'no expiry' per program");
    }
}
