//! Transaction directories contain payload bytes and a record.
//! The record is written last, by rename. Directories without a record are
//! incomplete writes and are ignored when loading transactions.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use payload::{decode_hash_hex, encode_hash_hex};
use pod2::middleware::Hash;
use wire_types::transactions::TransactionRecord;

use crate::paths::{PAYLOAD_FILE, RECORD_FILE};
use crate::types::DriverPaths;

pub(crate) fn transaction_dir(paths: &DriverPaths, tx_final: &Hash) -> PathBuf {
    paths.transactions_dir.join(encode_hash_hex(tx_final))
}

pub(crate) fn record_tx_final(record: &TransactionRecord) -> Result<Hash> {
    decode_hash_hex(&record.tx_final).map_err(|err| {
        anyhow!(
            "record {} has an unreadable txFinal: {err}",
            record.tx_final
        )
    })
}

pub(crate) fn decode_hashes(hex: &[String]) -> Result<Vec<Hash>> {
    hex.iter()
        .map(|value| {
            decode_hash_hex(value).map_err(|err| anyhow!("unreadable hash {value}: {err}"))
        })
        .collect()
}

pub(crate) fn load_records(paths: &DriverPaths) -> Result<Vec<TransactionRecord>> {
    let mut records = Vec::new();
    if !paths.transactions_dir.exists() {
        return Ok(records);
    }
    for entry in fs::read_dir(&paths.transactions_dir)
        .with_context(|| format!("failed to read {}", paths.transactions_dir.display()))?
    {
        let entry = entry.with_context(|| "failed to read transactions entry")?;
        let record_path = entry.path().join(RECORD_FILE);
        if !record_path.is_file() {
            continue;
        }
        match read_record_file(&record_path) {
            Ok(record) => records.push(record),
            Err(err) => log::warn!("skipping {}: {err:#}", record_path.display()),
        }
    }
    records.sort_by_key(|record| (record.recorded_at_ms, record.tx_final.clone()));
    Ok(records)
}

pub(crate) fn read_record(
    paths: &DriverPaths,
    tx_final: &Hash,
) -> Result<Option<TransactionRecord>> {
    let record_path = transaction_dir(paths, tx_final).join(RECORD_FILE);
    if !record_path.is_file() {
        return Ok(None);
    }
    read_record_file(&record_path).map(Some)
}

fn read_record_file(record_path: &std::path::Path) -> Result<TransactionRecord> {
    let contents = fs::read_to_string(record_path)
        .with_context(|| format!("failed to read {}", record_path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", record_path.display()))
}

pub(crate) fn write_payload(paths: &DriverPaths, tx_final: &Hash, bytes: &[u8]) -> Result<()> {
    let dir = transaction_dir(paths, tx_final);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(PAYLOAD_FILE);
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn read_payload(paths: &DriverPaths, tx_final: &Hash) -> Result<Vec<u8>> {
    let path = transaction_dir(paths, tx_final).join(PAYLOAD_FILE);
    fs::read(&path).with_context(|| format!("failed to read {}", path.display()))
}

/// Replace the record atomically so readers see a complete file.
pub(crate) fn write_record(paths: &DriverPaths, record: &TransactionRecord) -> Result<()> {
    let dir = transaction_dir(paths, &record_tx_final(record)?);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let serialized = serde_json::to_string_pretty(record)
        .with_context(|| format!("failed to serialize record {}", record.tx_final))?;
    let temporary = dir.join(format!("{RECORD_FILE}.tmp"));
    let target = dir.join(RECORD_FILE);
    fs::write(&temporary, serialized)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    fs::rename(&temporary, &target)
        .with_context(|| format!("failed to commit {}", target.display()))
}

pub(crate) fn remove_transaction(paths: &DriverPaths, tx_final: &Hash) -> Result<()> {
    let dir = transaction_dir(paths, tx_final);
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow!("failed to remove {}: {err}", dir.display())),
    }
}

/// Inputs unavailable for spending until their open transactions settle.
pub(crate) fn locked_inputs(records: &[TransactionRecord]) -> HashSet<String> {
    records
        .iter()
        .filter(|record| record.is_open())
        .flat_map(|record| record.input_files.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wire_types::QualifiedName;

    fn record(tx_final: Hash, recorded_at_ms: u64) -> TransactionRecord {
        TransactionRecord::recorded(
            encode_hash_hex(&tx_final),
            QualifiedName::new("plugin", "Action"),
            "test".to_string(),
            vec!["in.dobj".to_string()],
            vec!["out.dobj".to_string()],
            encode_hash_hex(&Hash::default()),
            1,
            vec![],
            vec![],
            recorded_at_ms,
        )
    }

    #[test]
    fn records_round_trip_and_directories_without_a_record_are_skipped() {
        let dir = tempdir().unwrap();
        let paths = DriverPaths::from_dobj_root(dir.path());
        let first = Hash::from(pod2::middleware::RawValue::from(1));
        let second = Hash::from(pod2::middleware::RawValue::from(2));
        write_payload(&paths, &first, b"payload").unwrap();
        write_record(&paths, &record(first, 20)).unwrap();
        write_payload(&paths, &second, b"orphan").unwrap();

        let loaded = load_records(&paths).unwrap();
        assert_eq!(loaded, vec![record(first, 20)]);
        assert_eq!(read_payload(&paths, &first).unwrap(), b"payload");
        assert!(read_record(&paths, &second).unwrap().is_none());
        assert!(
            !transaction_dir(&paths, &first)
                .join("record.json.tmp")
                .exists()
        );

        remove_transaction(&paths, &first).unwrap();
        assert!(load_records(&paths).unwrap().is_empty());
        remove_transaction(&paths, &first).unwrap();
    }
}
