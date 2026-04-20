//! Durable write-ahead log for V4 reveal material.
//!
//! A commit tx lands on-chain with just the `commit_hash`. The reveal tx
//! that follows needs the original payload, dispatch accounts, and user
//! signature to prove the committed hash. Before this WAL existed, that
//! material lived only in an in-memory `BTreeMap` — on relayer restart the
//! head became unrevealable and had to be cleared via admin autodrop.
//!
//! The WAL is a single append-only file of JSON-lines records. Each
//! `Insert` row carries the full reveal material for one sequence. Each
//! `Remove` row is a tombstone written when the corresponding commit tx
//! fails to land, or when the on-chain head advances past the sequence.
//!
//! On startup, `replay_live` scans the file once and returns the set of
//! inserts that have not been tombstoned. Callers filter by the current
//! on-chain `next_sequence_to_execute` so pre-head entries (already
//! executed on a prior run) are dropped.
//!
//! Compaction rewrites the file with only live inserts. The caller can
//! invoke it on a schedule or after a size threshold.

use crate::v4_pipeline::V4RevealEntry;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::{instruction::AccountMeta, pubkey::Pubkey};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug, Serialize, Deserialize)]
struct WalAccountMeta {
    pubkey: [u8; 32],
    is_signer: bool,
    is_writable: bool,
}

impl From<&AccountMeta> for WalAccountMeta {
    fn from(m: &AccountMeta) -> Self {
        Self {
            pubkey: m.pubkey.to_bytes(),
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        }
    }
}

impl From<&WalAccountMeta> for AccountMeta {
    fn from(m: &WalAccountMeta) -> Self {
        AccountMeta {
            pubkey: Pubkey::new_from_array(m.pubkey),
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WalEntry {
    sequence: u64,
    payload: Vec<u8>,
    payload_hash: [u8; 32],
    dispatch_accounts: Vec<WalAccountMeta>,
    user_owner: [u8; 32],
    mango_account: [u8; 32],
    /// Stored as a Vec (always 64 bytes when Some). The serde version in
    /// this crate doesn't derive Serialize for fixed arrays longer than 32.
    user_sig: Option<Vec<u8>>,
    user_intent_hash: Option<[u8; 32]>,
    /// Envelope fields added in v5. `#[serde(default)]` on each so older
    /// WAL lines (pre-v5) still parse; they default to 0, which matches
    /// the legacy "immediate execute, no expiry" semantics that v4 used
    /// for every reveal.
    #[serde(default)]
    min_execute_slot: u64,
    #[serde(default)]
    expires_at_slot: u64,
    #[serde(default)]
    kind: u8,
    /// P0.5 single-hash migration: user-supplied 8-byte randomizer bound
    /// into canonical_user_intent_v3. Pre-migration entries deserialize
    /// as 0; the reveal will then fail commit-hash match and get handled
    /// via the autodrop path.
    #[serde(default)]
    client_order_id: u64,
    /// Exact bytes the user ed25519 sig covers (raw 32 or hex-utf8 64). When
    /// None or absent, reveal falls back to `user_intent_hash`. Added after
    /// the P0.5 migration to fix a devnet mismatch where bots signing
    /// hex-utf8 passed commit-time verify but failed reveal-side pre-ix.
    #[serde(default)]
    user_sig_message: Option<Vec<u8>>,
}

impl From<&V4RevealEntry> for WalEntry {
    fn from(e: &V4RevealEntry) -> Self {
        Self {
            sequence: e.sequence,
            payload: e.payload.clone(),
            payload_hash: e.payload_hash,
            dispatch_accounts: e.dispatch_accounts.iter().map(Into::into).collect(),
            user_owner: e.user_owner.to_bytes(),
            mango_account: e.mango_account.to_bytes(),
            user_sig: e.user_sig.map(|s| s.to_vec()),
            user_intent_hash: e.user_intent_hash,
            min_execute_slot: e.min_execute_slot,
            expires_at_slot: e.expires_at_slot,
            kind: e.kind,
            client_order_id: e.client_order_id,
            user_sig_message: e.user_sig_message.clone(),
        }
    }
}

impl From<&WalEntry> for V4RevealEntry {
    fn from(w: &WalEntry) -> Self {
        V4RevealEntry {
            sequence: w.sequence,
            payload: w.payload.clone(),
            payload_hash: w.payload_hash,
            dispatch_accounts: w.dispatch_accounts.iter().map(Into::into).collect(),
            user_owner: Pubkey::new_from_array(w.user_owner),
            mango_account: Pubkey::new_from_array(w.mango_account),
            user_sig: w.user_sig.as_ref().and_then(|v| <[u8; 64]>::try_from(&v[..]).ok()),
            user_intent_hash: w.user_intent_hash,
            min_execute_slot: w.min_execute_slot,
            expires_at_slot: w.expires_at_slot,
            kind: w.kind,
            client_order_id: w.client_order_id,
            user_sig_message: w.user_sig_message.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op")]
enum WalRecord {
    #[serde(rename = "insert")]
    Insert(WalEntry),
    #[serde(rename = "remove")]
    Remove { sequence: u64 },
}

/// Append-only JSON-lines WAL for V4 reveal material.
pub struct V4RevealWal {
    path: PathBuf,
    file: Mutex<File>,
}

impl V4RevealWal {
    /// Open (or create) the WAL file at `path`. Creates parent directories.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create WAL parent dir {parent:?}"))?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open WAL at {path:?}"))?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Append an `Insert` record for `entry` and fsync. Must be called
    /// before the corresponding commit tx is submitted.
    pub fn append_insert(&self, entry: &V4RevealEntry) -> Result<()> {
        let rec = WalRecord::Insert(WalEntry::from(entry));
        self.append_record(&rec)
    }

    /// Append a `Remove` tombstone. Used on commit-send failure so the entry
    /// is dropped from replay even though it was written speculatively.
    pub fn append_remove(&self, sequence: u64) -> Result<()> {
        let rec = WalRecord::Remove { sequence };
        self.append_record(&rec)
    }

    fn append_record(&self, rec: &WalRecord) -> Result<()> {
        let line = serde_json::to_string(rec).context("serialize WAL record")?;
        let mut file = self.file.lock().expect("WAL mutex poisoned");
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }

    /// Read the WAL from the beginning and return the set of live inserts
    /// with `sequence >= min_sequence`. Tombstones and entries below
    /// `min_sequence` are dropped.
    pub fn replay_live(&self, min_sequence: u64) -> Result<BTreeMap<u64, V4RevealEntry>> {
        let mut file = self.file.lock().expect("WAL mutex poisoned");
        file.seek(SeekFrom::Start(0))?;
        let mut buf = String::new();
        {
            let mut reader = BufReader::new(&mut *file);
            reader.read_to_string(&mut buf)?;
        }
        // Restore append position so subsequent writes go to EOF.
        file.seek(SeekFrom::End(0))?;

        let mut live: BTreeMap<u64, V4RevealEntry> = BTreeMap::new();
        for (lineno, line) in buf.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<WalRecord>(line) {
                Ok(WalRecord::Insert(entry)) => {
                    if entry.sequence >= min_sequence {
                        live.insert(entry.sequence, V4RevealEntry::from(&entry));
                    }
                }
                Ok(WalRecord::Remove { sequence }) => {
                    live.remove(&sequence);
                }
                Err(e) => {
                    // A torn trailing record can happen if the process crashed
                    // mid-append. Skip it — fsync on each record means at
                    // worst the last partial line is lost.
                    tracing::warn!(
                        target: "v4_reveal_wal",
                        lineno,
                        error = %e,
                        "skipping malformed WAL record"
                    );
                }
            }
        }
        Ok(live)
    }

    /// Rewrite the WAL file retaining only inserts with
    /// `sequence >= min_sequence`. Called periodically from the reveal GC
    /// loop to bound file size.
    pub fn compact(&self, min_sequence: u64) -> Result<()> {
        let live = self.replay_live(min_sequence)?;
        let tmp_path = self.path.with_extension("wal.compact.tmp");
        {
            let mut tmp = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
                .with_context(|| format!("open WAL compact tmp {tmp_path:?}"))?;
            for entry in live.values() {
                let rec = WalRecord::Insert(WalEntry::from(entry));
                let line = serde_json::to_string(&rec).context("serialize WAL record")?;
                tmp.write_all(line.as_bytes())?;
                tmp.write_all(b"\n")?;
            }
            tmp.sync_data()?;
        }
        let mut file = self.file.lock().expect("WAL mutex poisoned");
        std::fs::rename(&tmp_path, &self.path)
            .with_context(|| format!("rename {tmp_path:?} -> {:?}", self.path))?;
        // Re-open the file handle so future appends go to the new inode.
        *file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("reopen WAL after compact at {:?}", self.path))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn mk_entry(seq: u64) -> V4RevealEntry {
        V4RevealEntry {
            sequence: seq,
            payload: vec![1, 2, 3, seq as u8],
            payload_hash: [seq as u8; 32],
            dispatch_accounts: vec![AccountMeta::new(Pubkey::new_unique(), false)],
            user_owner: Pubkey::new_unique(),
            mango_account: Pubkey::new_unique(),
            user_sig: Some([9u8; 64]),
            user_intent_hash: Some([seq as u8; 32]),
            user_sig_message: None,
            min_execute_slot: 0,
            expires_at_slot: 0,
            kind: 0,
            client_order_id: seq.wrapping_mul(0xDEAD_BEEF),
        }
    }

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "v4_reveal_wal_test_{}_{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn replay_filters_below_head_and_respects_tombstones() {
        let path = tmp_path("replay");
        let wal = V4RevealWal::open(&path).unwrap();
        wal.append_insert(&mk_entry(1)).unwrap();
        wal.append_insert(&mk_entry(2)).unwrap();
        wal.append_insert(&mk_entry(3)).unwrap();
        wal.append_remove(2).unwrap();

        // Simulate restart.
        drop(wal);
        let wal2 = V4RevealWal::open(&path).unwrap();

        // Head at 2 drops seq 1 and seq 2 (tombstoned); only seq 3 replays.
        let live = wal2.replay_live(2).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), vec![3]);
        assert_eq!(live[&3].payload, vec![1, 2, 3, 3]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn compact_retains_only_live_inserts() {
        let path = tmp_path("compact");
        let wal = V4RevealWal::open(&path).unwrap();
        wal.append_insert(&mk_entry(1)).unwrap();
        wal.append_insert(&mk_entry(2)).unwrap();
        wal.append_insert(&mk_entry(3)).unwrap();
        wal.append_remove(1).unwrap();

        let sz_before = std::fs::metadata(&path).unwrap().len();
        wal.compact(2).unwrap();
        let sz_after = std::fs::metadata(&path).unwrap().len();
        assert!(sz_after < sz_before, "expected compaction to shrink file");

        // Subsequent appends still land and replay correctly.
        wal.append_insert(&mk_entry(5)).unwrap();
        let live = wal.replay_live(0).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), vec![2, 3, 5]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_trailing_line_is_skipped() {
        let path = tmp_path("torn");
        let wal = V4RevealWal::open(&path).unwrap();
        wal.append_insert(&mk_entry(7)).unwrap();
        drop(wal);

        // Simulate a torn trailing record by appending garbage directly.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"this_is_not_json\n").unwrap();
        drop(f);

        let wal2 = V4RevealWal::open(&path).unwrap();
        let live = wal2.replay_live(0).unwrap();
        assert_eq!(live.keys().copied().collect::<Vec<_>>(), vec![7]);
        let _ = std::fs::remove_file(&path);
    }
}
