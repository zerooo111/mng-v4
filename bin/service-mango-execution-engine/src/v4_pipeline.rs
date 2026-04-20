//! Residual v4 types retained for WAL compatibility. All on-chain v4 types
//! have been removed from the program; this file exists only so that
//! v4_reveal_wal (and any existing WAL files on disk) can still deserialize
//! reveal entries that were written before the v5 migration.

use solana_sdk::{instruction::AccountMeta, pubkey::Pubkey};
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicU64, Arc},
};
use parking_lot::Mutex;

/// Reveal entry written to the WAL before each commit tx. Fields are
/// identical to V5RevealEntry; the type alias in v5_pipeline re-exports
/// this under the V5 name so both routes share the same WAL code.
///
/// `min_execute_slot`, `expires_at_slot`, and `kind` mirror the envelope
/// fields bound into the on-chain commit_hash. The reveal tx recomputes
/// the hash using these values; any divergence from what the relayer
/// committed rejects the reveal on-chain, so they must round-trip through
/// the WAL unchanged.
#[derive(Clone)]
pub struct V4RevealEntry {
    pub sequence: u64,
    pub payload: Vec<u8>,
    pub payload_hash: [u8; 32],
    pub dispatch_accounts: Vec<AccountMeta>,
    pub user_owner: Pubkey,
    pub mango_account: Pubkey,
    pub user_sig: Option<[u8; 64]>,
    pub user_intent_hash: Option<[u8; 32]>,
    pub min_execute_slot: u64,
    pub expires_at_slot: u64,
    pub kind: u8,
}

pub type V4RevealStore = Arc<Mutex<BTreeMap<u64, V4RevealEntry>>>;
pub type V4NextSequence = Arc<AtomicU64>;
