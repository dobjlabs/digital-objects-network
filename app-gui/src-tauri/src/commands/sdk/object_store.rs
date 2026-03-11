use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use craft_sdk::SpendableObject;
use serde::{Deserialize, Serialize};
use txlib::StateRoot;

use crate::state::{ObjectsRuntimeState, RuntimeObjectRecord, RuntimeValidity};

use super::naming::object_state_hash_from_spendable;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedStateRoot {
    block_number: i64,
    transactions: serde_json::Value,
    nullifiers: serde_json::Value,
    gsrs: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedObjectRecord {
    id: String,
    class_name: String,
    source_action: Option<String>,
    validity: String,
    state_hash: String,
    nullifier: Option<String>,
    pod: Option<serde_json::Value>,
    obj: Option<serde_json::Value>,
    tx_live: Option<serde_json::Value>,
    tx_nullifiers: Option<serde_json::Value>,
    tx_state_root: Option<PersistedStateRoot>,
}

const NULLIFIED_DIR_NAME: &str = ".nullified";

pub(super) fn nullified_objects_dir(objects_dir: &Path) -> PathBuf {
    objects_dir.join(NULLIFIED_DIR_NAME)
}

pub(super) fn ensure_objects_dirs(objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    fs::create_dir_all(nullified_objects_dir(objects_dir))
        .map_err(|err| format!("failed to create nullified directory: {err}"))?;
    Ok(())
}

fn persist_state_root(state_root: &StateRoot) -> Result<PersistedStateRoot, String> {
    Ok(PersistedStateRoot {
        block_number: state_root.block_number,
        transactions: serde_json::to_value(&state_root.transactions)
            .map_err(|err| format!("failed to serialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::to_value(&state_root.nullifiers)
            .map_err(|err| format!("failed to serialize state_root.nullifiers: {err}"))?,
        gsrs: serde_json::to_value(&state_root.gsrs)
            .map_err(|err| format!("failed to serialize state_root.gsrs: {err}"))?,
    })
}

fn restore_state_root(data: PersistedStateRoot) -> Result<StateRoot, String> {
    Ok(StateRoot {
        block_number: data.block_number,
        transactions: serde_json::from_value(data.transactions)
            .map_err(|err| format!("failed to deserialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::from_value(data.nullifiers)
            .map_err(|err| format!("failed to deserialize state_root.nullifiers: {err}"))?,
        gsrs: serde_json::from_value(data.gsrs)
            .map_err(|err| format!("failed to deserialize state_root.gsrs: {err}"))?,
    })
}

fn persist_spendable(
    spendable: &SpendableObject,
) -> Result<
    (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        PersistedStateRoot,
    ),
    String,
> {
    Ok((
        serde_json::to_value(&spendable.pod)
            .map_err(|err| format!("failed to serialize spendable.pod: {err}"))?,
        serde_json::to_value(&spendable.obj)
            .map_err(|err| format!("failed to serialize spendable.obj: {err}"))?,
        serde_json::to_value(&spendable.tx.live)
            .map_err(|err| format!("failed to serialize spendable.tx.live: {err}"))?,
        serde_json::to_value(&spendable.tx.nullifiers)
            .map_err(|err| format!("failed to serialize spendable.tx.nullifiers: {err}"))?,
        persist_state_root(spendable.tx.state_root.as_ref())?,
    ))
}

fn restore_spendable(
    pod: Option<serde_json::Value>,
    obj: Option<serde_json::Value>,
    tx_live: Option<serde_json::Value>,
    tx_nullifiers: Option<serde_json::Value>,
    tx_state_root: Option<PersistedStateRoot>,
) -> Result<Option<SpendableObject>, String> {
    match (pod, obj, tx_live, tx_nullifiers, tx_state_root) {
        (None, None, None, None, None) => Ok(None),
        (Some(pod), Some(obj), Some(tx_live), Some(tx_nullifiers), Some(tx_state_root)) => {
            let state_root = restore_state_root(tx_state_root)?;
            let tx = txlib::Tx {
                live: serde_json::from_value(tx_live)
                    .map_err(|err| format!("failed to deserialize spendable.tx.live: {err}"))?,
                nullifiers: serde_json::from_value(tx_nullifiers).map_err(|err| {
                    format!("failed to deserialize spendable.tx.nullifiers: {err}")
                })?,
                state_root: Arc::new(state_root),
            };
            Ok(Some(SpendableObject {
                pod: serde_json::from_value(pod)
                    .map_err(|err| format!("failed to deserialize spendable.pod: {err}"))?,
                obj: serde_json::from_value(obj)
                    .map_err(|err| format!("failed to deserialize spendable.obj: {err}"))?,
                tx,
            }))
        }
        _ => Err(
            "invalid object file: spendable fields must all be present or all absent".to_string(),
        ),
    }
}

fn validity_from_str(raw: &str, context: &str) -> Result<RuntimeValidity, String> {
    match raw {
        "live" => Ok(RuntimeValidity::Live),
        "nullified" => Ok(RuntimeValidity::Nullified),
        other => Err(format!("invalid object validity in {context}: {other}")),
    }
}

fn restore_object_record(
    record: PersistedObjectRecord,
    file_name: &str,
) -> Result<RuntimeObjectRecord, String> {
    let spendable = restore_spendable(
        record.pod,
        record.obj,
        record.tx_live,
        record.tx_nullifiers,
        record.tx_state_root,
    )?;
    let state_hash = spendable
        .as_ref()
        .map(object_state_hash_from_spendable)
        .unwrap_or(record.state_hash);
    Ok(RuntimeObjectRecord {
        id: record.id,
        file_name: file_name.to_string(),
        class_name: record.class_name,
        source_action: record.source_action,
        validity: validity_from_str(&record.validity, "object file")?,
        state_hash,
        nullifier: record.nullifier,
        spendable,
    })
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<RuntimeObjectRecord, String> {
    let record = serde_json::from_str::<PersistedObjectRecord>(contents)
        .map_err(|err| format!("failed to parse {file_name} as object file: {err}"))?;
    restore_object_record(record, file_name)
}

fn persist_object_record(record: &RuntimeObjectRecord) -> Result<PersistedObjectRecord, String> {
    let (pod, obj, tx_live, tx_nullifiers, tx_state_root) =
        if let Some(spendable) = record.spendable.as_ref() {
            let (pod, obj, tx_live, tx_nullifiers, tx_state_root) = persist_spendable(spendable)?;
            (
                Some(pod),
                Some(obj),
                Some(tx_live),
                Some(tx_nullifiers),
                Some(tx_state_root),
            )
        } else {
            (None, None, None, None, None)
        };
    let state_hash = record
        .spendable
        .as_ref()
        .map(object_state_hash_from_spendable)
        .unwrap_or_else(|| record.state_hash.clone());
    Ok(PersistedObjectRecord {
        id: record.id.clone(),
        class_name: record.class_name.clone(),
        source_action: record.source_action.clone(),
        validity: match record.validity {
            RuntimeValidity::Live => "live".to_string(),
            RuntimeValidity::Nullified => "nullified".to_string(),
        },
        state_hash,
        nullifier: record.nullifier.clone(),
        pod,
        obj,
        tx_live,
        tx_nullifiers,
        tx_state_root,
    })
}

pub(super) fn sync_object_files(
    inner: &ObjectsRuntimeState,
    objects_dir: &Path,
) -> Result<(), String> {
    ensure_objects_dirs(objects_dir)?;
    let nullified_dir = nullified_objects_dir(objects_dir);

    for record in &inner.objects {
        let persisted = persist_object_record(record)?;
        let serialized = serde_json::to_string_pretty(&persisted).map_err(|err| {
            format!(
                "failed to serialize object file {}: {err}",
                record.file_name
            )
        })?;
        let target_path = match record.validity {
            RuntimeValidity::Live => objects_dir.join(&record.file_name),
            RuntimeValidity::Nullified => nullified_dir.join(&record.file_name),
        };
        fs::write(&target_path, serialized)
            .map_err(|err| format!("failed to write object file {}: {err}", record.file_name))?;

        let stale_path = match record.validity {
            RuntimeValidity::Live => nullified_dir.join(&record.file_name),
            RuntimeValidity::Nullified => objects_dir.join(&record.file_name),
        };
        if stale_path != target_path {
            match fs::remove_file(&stale_path) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    eprintln!(
                        "zk-craft: failed to remove stale object file {}: {err}",
                        stale_path.display()
                    );
                }
            }
        }
    }

    Ok(())
}

fn load_object_files_from_dir(
    objects: &mut HashMap<String, (RuntimeObjectRecord, u8)>,
    source_dir: &Path,
    in_nullified_dir: bool,
) -> Result<(), String> {
    if !source_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source_dir)
        .map_err(|err| format!("failed to read objects directory: {err}"))?
    {
        let entry = entry.map_err(|err| format!("failed to read objects entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_dobj = path.extension().and_then(|ext| ext.to_str()) == Some("dobj");
        if !is_dobj {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => {
                eprintln!("zk-craft: failed to read {file_name}, skipping: {err}");
                continue;
            }
        };

        match parse_object_file(&contents, file_name) {
            Ok(record) => {
                let score = if (in_nullified_dir
                    && matches!(record.validity, RuntimeValidity::Nullified))
                    || (!in_nullified_dir && matches!(record.validity, RuntimeValidity::Live))
                {
                    2
                } else {
                    1
                };
                let replace = objects
                    .get(&record.file_name)
                    .map(|(_, existing_score)| score > *existing_score)
                    .unwrap_or(true);
                if replace {
                    objects.insert(record.file_name.clone(), (record, score));
                }
            }
            Err(err) => eprintln!("zk-craft: failed to parse {file_name}, skipping: {err}"),
        }
    }

    Ok(())
}

pub(super) fn load_object_files(objects_dir: &Path) -> Result<Vec<RuntimeObjectRecord>, String> {
    let mut records_by_file = HashMap::<String, (RuntimeObjectRecord, u8)>::new();
    load_object_files_from_dir(&mut records_by_file, objects_dir, false)?;
    load_object_files_from_dir(
        &mut records_by_file,
        &nullified_objects_dir(objects_dir),
        true,
    )?;

    let mut objects = records_by_file
        .into_values()
        .map(|(record, _)| record)
        .collect::<Vec<_>>();
    objects.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(objects)
}

pub(super) fn parse_object_file_from_path(path: &Path) -> Result<RuntimeObjectRecord, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid input path (missing file name): {}", path.display()))?;
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read input file {}: {err}", path.display()))?;
    parse_object_file(&contents, file_name)
}

#[cfg(test)]
mod tests {
    use super::{parse_object_file, persist_object_record};
    use crate::state::{RuntimeObjectRecord, RuntimeValidity};

    #[test]
    fn metadata_only_record_round_trips() {
        let record = RuntimeObjectRecord {
            id: "0xabc".to_string(),
            file_name: "stone_0xabc.dobj".to_string(),
            class_name: "Stone".to_string(),
            source_action: Some("MineStoneWithWoodPick".to_string()),
            validity: RuntimeValidity::Live,
            state_hash: "0xstate".to_string(),
            nullifier: None,
            spendable: None,
        };

        let persisted = persist_object_record(&record).expect("persist succeeds");
        let json = serde_json::to_string(&persisted).expect("serialize succeeds");
        let restored = parse_object_file(&json, &record.file_name).expect("parse succeeds");

        assert_eq!(restored.id, record.id);
        assert_eq!(restored.file_name, record.file_name);
        assert_eq!(restored.class_name, record.class_name);
        assert_eq!(restored.source_action, record.source_action);
        assert_eq!(restored.validity, record.validity);
        assert_eq!(restored.state_hash, record.state_hash);
        assert_eq!(restored.nullifier, record.nullifier);
        assert!(restored.spendable.is_none());
    }

    #[test]
    fn rejects_partial_spendable_payloads() {
        let json = r#"{
            "id":"0xabc",
            "className":"Stone",
            "sourceAction":null,
            "validity":"live",
            "stateHash":"0xstate",
            "nullifier":null,
            "pod":{},
            "obj":null,
            "txLive":null,
            "txNullifiers":null,
            "txStateRoot":null
        }"#;

        let err = parse_object_file(json, "stone_0xabc.dobj").expect_err("must fail");
        assert!(err.contains("spendable fields must all be present or all absent"));
    }
}
