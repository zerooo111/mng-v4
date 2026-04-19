//! Instruction + hash builders for the v4 commit-reveal queue.
//!
//! Keep everything here side-effect free so it can be unit-tested without
//! spinning up the full relayer. The batch worker in `v4_batcher` and the
//! reveal worker in `v4_reveal_packer` call into these to produce the exact
//! bytes that land on chain.

use anchor_lang::InstructionData;
use mango_v4::instruction::{
    ExecutionQueueV4CommitMarket, ExecutionQueueV4RevealExecuteMarket,
};
use mango_v4::instructions::{CommitEntryV4, RevealArgsV4};
use solana_program::hash::hashv;
use solana_sdk::{
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    sysvar,
};

// ---------------------------------------------------------------------------
// PDA lookups
// ---------------------------------------------------------------------------

pub fn find_commit_queue_root_pda(
    program_id: Pubkey,
    group: Pubkey,
    market_index: u16,
    shard_id: u8,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-root".as_ref(),
            group.as_ref(),
            &market_index.to_le_bytes(),
            &[shard_id],
        ],
        &program_id,
    )
    .0
}

pub fn find_commit_queue_page_pda(
    program_id: Pubkey,
    queue_root: Pubkey,
    page_slot: u16,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"commit-queue-page".as_ref(),
            queue_root.as_ref(),
            &page_slot.to_le_bytes(),
        ],
        &program_id,
    )
    .0
}

// ---------------------------------------------------------------------------
// Hash helpers — must stay byte-for-byte identical to the on-chain
// definitions in programs/mango-v4/src/instructions/execution_queue_v4.rs.
// ---------------------------------------------------------------------------

pub fn hash_accounts(accounts: &[AccountMeta]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(accounts.len() * 34);
    for a in accounts {
        buf.extend_from_slice(a.pubkey.as_ref());
        buf.push(u8::from(a.is_signer));
        buf.push(u8::from(a.is_writable));
    }
    hashv(&[&buf]).to_bytes()
}

/// Per-entry commit_hash bound into the on-chain CommitItemV4.
#[allow(clippy::too_many_arguments)]
pub fn canonical_commit_hash(
    group: Pubkey,
    market_index: u16,
    sequence: u64,
    kind: u8,
    payload_hash: &[u8; 32],
    accounts_hash: &[u8; 32],
    min_execute_slot: u64,
    expires_at_slot: u64,
) -> [u8; 32] {
    hashv(&[
        b"mango-v4-commit-v1",
        group.as_ref(),
        &market_index.to_le_bytes(),
        &sequence.to_le_bytes(),
        &[kind],
        payload_hash,
        accounts_hash,
        &min_execute_slot.to_le_bytes(),
        &expires_at_slot.to_le_bytes(),
    ])
    .to_bytes()
}

/// The single message the relayer signs that authorizes a whole batch.
pub fn canonical_commit_batch_message(
    group: Pubkey,
    market_index: u16,
    shard_id: u8,
    first_sequence: u64,
    entries: &[CommitEntryV4],
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(entries.len() * 48 + 64);
    buf.extend_from_slice(b"mango-v4-commit-batch-v1");
    buf.extend_from_slice(group.as_ref());
    buf.extend_from_slice(&market_index.to_le_bytes());
    buf.push(shard_id);
    buf.extend_from_slice(&first_sequence.to_le_bytes());
    buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in entries {
        buf.extend_from_slice(&e.commit_hash);
        buf.extend_from_slice(&e.min_execute_slot.to_le_bytes());
        buf.extend_from_slice(&e.expires_at_slot.to_le_bytes());
    }
    hashv(&[&buf]).to_bytes()
}

// ---------------------------------------------------------------------------
// Ed25519 pre-instruction builder (single-sig; reused for batch sig).
// Copied from main.rs so v4 code is self-contained.
// ---------------------------------------------------------------------------

pub fn build_presigned_ed25519_instruction(
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

/// Multi-signature ed25519 pre-instruction — N sigs verified in a single
/// syscall. This is the main reveal-path throughput trick: per-sig cost drops
/// because the fixed verify overhead is amortized across the batch.
pub fn build_multi_ed25519_instruction(sigs: &[(Pubkey, [u8; 64], Vec<u8>)]) -> Instruction {
    let n = sigs.len();
    assert!(n <= u8::MAX as usize, "ed25519 multi-sig cap is 255");

    // Layout: [count:u8, pad:u8, N × offsets:14b, then blobs (pubkey, sig, msg)*].
    let header = 2 + n * 14;
    let mut body: Vec<u8> = Vec::new();
    let mut offsets: Vec<(u16, u16, u16, u16)> = Vec::with_capacity(n); // (pk, sig, msg, msg_len)

    for (pk, sig, msg) in sigs {
        let pk_off = (header + body.len()) as u16;
        body.extend_from_slice(pk.as_ref());
        let sig_off = (header + body.len()) as u16;
        body.extend_from_slice(sig);
        let msg_off = (header + body.len()) as u16;
        body.extend_from_slice(msg);
        let msg_len = msg.len() as u16;
        offsets.push((pk_off, sig_off, msg_off, msg_len));
    }

    let mut data = vec![0u8; header + body.len()];
    data[0] = n as u8;
    data[1] = 0;
    for (i, (pk_off, sig_off, msg_off, msg_len)) in offsets.iter().enumerate() {
        let base = 2 + i * 14;
        data[base..base + 2].copy_from_slice(&sig_off.to_le_bytes());
        data[base + 2..base + 4].copy_from_slice(&u16::MAX.to_le_bytes());
        data[base + 4..base + 6].copy_from_slice(&pk_off.to_le_bytes());
        data[base + 6..base + 8].copy_from_slice(&u16::MAX.to_le_bytes());
        data[base + 8..base + 10].copy_from_slice(&msg_off.to_le_bytes());
        data[base + 10..base + 12].copy_from_slice(&msg_len.to_le_bytes());
        data[base + 12..base + 14].copy_from_slice(&u16::MAX.to_le_bytes());
    }
    data[header..].copy_from_slice(&body);

    Instruction {
        program_id: ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

// ---------------------------------------------------------------------------
// Instruction builders for commit + reveal
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn build_commit_market_instruction(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    market_index: u16,
    first_sequence: u64,
    entries: Vec<CommitEntryV4>,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new_readonly(group, false),
        AccountMeta::new(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new(queue_page, false),
        AccountMeta::new_readonly(sysvar::instructions::id(), false),
    ];
    Instruction {
        program_id,
        accounts,
        data: ExecutionQueueV4CommitMarket {
            market_index,
            first_sequence,
            entries,
        }
        .data(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_reveal_execute_market_instruction(
    program_id: Pubkey,
    group: Pubkey,
    authority_state: Pubkey,
    queue_root: Pubkey,
    queue_page: Pubkey,
    remaining_accounts: Vec<AccountMeta>,
    reveals: Vec<RevealArgsV4>,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(group, false),
        AccountMeta::new_readonly(authority_state, false),
        AccountMeta::new(queue_root, false),
        AccountMeta::new(queue_page, false),
        AccountMeta::new_readonly(sysvar::instructions::id(), false),
    ];
    accounts.extend(remaining_accounts);
    // dispatch_queue_payload needs the program itself in remaining accounts
    // to invoke inner CPIs; mirrors the v3 execute layout.
    accounts.push(AccountMeta::new_readonly(program_id, false));
    Instruction {
        program_id,
        accounts,
        data: ExecutionQueueV4RevealExecuteMarket { reveals }.data(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_hash_is_deterministic() {
        let group = Pubkey::new_unique();
        let h = canonical_commit_hash(group, 7, 42, 0, &[1; 32], &[2; 32], 100, 200);
        let h2 = canonical_commit_hash(group, 7, 42, 0, &[1; 32], &[2; 32], 100, 200);
        assert_eq!(h, h2);
        let h3 = canonical_commit_hash(group, 7, 43, 0, &[1; 32], &[2; 32], 100, 200);
        assert_ne!(h, h3);
    }

    #[test]
    fn batch_msg_depends_on_each_entry() {
        let group = Pubkey::new_unique();
        let entries = vec![
            CommitEntryV4 {
                commit_hash: [1; 32],
                min_execute_slot: 5,
                expires_at_slot: 100,
            },
            CommitEntryV4 {
                commit_hash: [2; 32],
                min_execute_slot: 5,
                expires_at_slot: 100,
            },
        ];
        let a = canonical_commit_batch_message(group, 0, 0, 10, &entries);
        let mut e2 = entries.clone();
        e2[1].commit_hash = [3; 32];
        let b = canonical_commit_batch_message(group, 0, 0, 10, &e2);
        assert_ne!(a, b);
    }
}
