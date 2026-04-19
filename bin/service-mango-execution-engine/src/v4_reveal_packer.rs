//! Reveal-batch packing with tx-size awareness.
//!
//! The reveal worker must not overflow Solana's per-tx 1232-byte packet
//! budget. Each reveal contributes:
//!   * variable ix data  — payload bytes + a 2-byte length + 1 byte kind +
//!     1 byte dispatch_accounts_count + 4 bytes borsh-vec prefix (once per
//!     batch header)
//!   * unique dispatch accounts — any pubkeys not already in the shared
//!     prefix cost 32 bytes each
//!   * optional ed25519 pre-instruction entry for variants that require a
//!     user signature (142 bytes per sig header+pubkey+sig+32-byte msg)
//!
//! `pack_reveal_batch` greedily adds reveals in strict sequence order until
//! the next reveal would exceed the remaining budget.

use crate::v4_builders::build_reveal_execute_market_instruction;
use mango_v4::instructions::RevealArgsV4;
use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    signature::Signer,
    transaction::Transaction,
};
use std::collections::BTreeSet;

/// Upper bound on a serialized Solana legacy transaction.
pub const SOLANA_MAX_TX_BYTES: usize = 1232;

/// Target budget we aim to stay under after accounting for compute-budget
/// instructions, priority fee ix, signatures array, and a few bytes of slack.
pub const PACK_HEADROOM_BYTES: usize = 72;

/// One candidate pulled off the reveal worker's in-order queue.
pub struct RevealCandidate {
    pub sequence: u64,
    pub args: RevealArgsV4,
    /// Dispatch accounts for this specific reveal, in the exact order the
    /// commit_hash was computed over.
    pub dispatch_accounts: Vec<AccountMeta>,
    /// When Some, the ed25519 pre-ix must include this (pubkey, sig, msg)
    /// triple. None for variants that do not require a user signature.
    pub user_sig: Option<(Pubkey, [u8; 64], [u8; 32])>,
}

pub struct PackResult {
    pub reveals: Vec<RevealArgsV4>,
    pub remaining_accounts: Vec<AccountMeta>,
    pub user_sigs: Vec<(Pubkey, [u8; 64], Vec<u8>)>,
    /// The estimated size; useful for metrics and for the caller to sanity
    /// check against the real serialized tx.
    pub estimated_tx_bytes: usize,
}

/// Greedy packing of head-ordered reveals into a single tx. `shared_prefix`
/// is the list of fixture accounts (queue_root/page/group/etc.) already in
/// the tx's account list; dispatch accounts that match are free, new ones
/// cost 32 bytes.
pub fn pack_reveal_batch(
    candidates: &[RevealCandidate],
    fixed_tx_overhead: usize,
    shared_prefix: &[Pubkey],
) -> PackResult {
    let mut seen: BTreeSet<Pubkey> = shared_prefix.iter().copied().collect();
    let mut reveals: Vec<RevealArgsV4> = Vec::new();
    let mut remaining_accounts: Vec<AccountMeta> = Vec::new();
    let mut user_sigs: Vec<(Pubkey, [u8; 64], Vec<u8>)> = Vec::new();
    let mut estimated = fixed_tx_overhead + 4 /* borsh vec header for reveals */;

    for cand in candidates {
        let payload_bytes = 4 /* borsh vec header */ + cand.args.payload.len();
        let fixed_fields = 1 /* kind */ + 1 /* dispatch_accounts_count */;
        let ix_delta = payload_bytes + fixed_fields;

        let mut new_account_bytes: usize = 0;
        for meta in &cand.dispatch_accounts {
            if !seen.contains(&meta.pubkey) {
                new_account_bytes += 32 + 1; // pubkey + 1 byte for the meta flag
            }
        }

        let sig_delta = cand.user_sig.as_ref().map(|_| 142).unwrap_or(0);

        let next = estimated + ix_delta + new_account_bytes + sig_delta;
        if !reveals.is_empty() && next > SOLANA_MAX_TX_BYTES - PACK_HEADROOM_BYTES {
            break;
        }
        // Always take at least one reveal; if the single reveal already
        // overflows, we'll fail further up and probably need an ALT-backed
        // path, which this packer intentionally does not handle.
        estimated = next;
        reveals.push(cand.args.clone());
        for meta in &cand.dispatch_accounts {
            if seen.insert(meta.pubkey) {
                remaining_accounts.push(meta.clone());
            }
        }
        if let Some((pk, sig, msg)) = &cand.user_sig {
            user_sigs.push((*pk, *sig, msg.to_vec()));
        }
    }

    PackResult {
        reveals,
        remaining_accounts,
        user_sigs,
        estimated_tx_bytes: estimated,
    }
}

/// Serialize a candidate reveal tx and report its real wire size. Use this
/// in tests / canary paths to verify the packer stays within budget.
#[allow(clippy::too_many_arguments)]
pub fn realize_reveal_tx(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    payer: &dyn Signer,
    recent_blockhash: Hash,
    extra_preixs: Vec<Instruction>,
    pack: PackResult,
) -> Transaction {
    let mut ixs = extra_preixs;
    ixs.push(build_reveal_execute_market_instruction(
        program_id,
        group,
        authority_state,
        queue_root,
        queue_page,
        pack.remaining_accounts,
        pack.reveals,
    ));
    let msg = Message::new(&ixs, Some(&payer.pubkey()));
    Transaction::new(&[payer], msg, recent_blockhash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_candidate(seq: u64, payload_len: usize, user_sig: bool) -> RevealCandidate {
        let args = RevealArgsV4 {
            payload: vec![0u8; payload_len],
            kind: 0,
            dispatch_accounts_count: 3,
        };
        let dispatch = vec![
            AccountMeta::new(Pubkey::new_unique(), false),
            AccountMeta::new_readonly(Pubkey::new_unique(), false),
            AccountMeta::new_readonly(Pubkey::new_unique(), false),
        ];
        RevealCandidate {
            sequence: seq,
            args,
            dispatch_accounts: dispatch,
            user_sig: if user_sig {
                Some((Pubkey::new_unique(), [0u8; 64], [0u8; 32]))
            } else {
                None
            },
        }
    }

    #[test]
    fn packer_stops_before_overflow() {
        let cands: Vec<RevealCandidate> = (0..20).map(|i| mk_candidate(i, 45, true)).collect();
        let res = pack_reveal_batch(&cands, 400, &[]);
        assert!(!res.reveals.is_empty());
        assert!(res.estimated_tx_bytes <= SOLANA_MAX_TX_BYTES - PACK_HEADROOM_BYTES);
    }

    #[test]
    fn packer_fits_more_without_user_sigs() {
        let with_sig: Vec<_> = (0..30).map(|i| mk_candidate(i, 45, true)).collect();
        let no_sig: Vec<_> = (0..30).map(|i| mk_candidate(i, 45, false)).collect();
        let a = pack_reveal_batch(&with_sig, 400, &[]);
        let b = pack_reveal_batch(&no_sig, 400, &[]);
        assert!(b.reveals.len() > a.reveals.len());
    }
}
