use std::collections::HashSet;

use anyhow::{Result, anyhow};
use common::{
    decode_hash_hex,
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use pod2::middleware::{Hash, Key, Params};
use sdk::SpendableObjects;
use txlib::object_nullifier_hash;

use std::path::Path;

use crate::driver::extract_basename;
use crate::error::DriverError;
use crate::object_record::ObjectRecord as StoredObjectRecord;
use crate::object_store::{ObjectFileEntry, write_object_file};
use crate::paths::DOBJ_EXTENSION;
use crate::types::{DriverPaths, ExecuteActionInput};
use wire_types::{ActionSummary, ObjectStatus};

pub(crate) fn reconcile_objects(
    paths: &DriverPaths,
    objects: &mut [ObjectFileEntry],
    grounded_txs: &HashSet<Hash>,
    on_chain_nullifiers: &HashSet<Hash>,
) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    // First pass: nullify objects whose nullifiers appear on-chain.
    for entry in objects.iter_mut() {
        if entry.record.is_nullified() {
            continue;
        }
        let nullifier_hash = match object_nullifier_hash(&entry.record.obj) {
            Ok(hash) => hash,
            Err(_) => continue,
        };
        if !on_chain_nullifiers.contains(&nullifier_hash) {
            continue;
        }
        let nullified_record = StoredObjectRecord {
            status: ObjectStatus::Nullified,
            ..entry.record.clone()
        };
        if let Err(err) = write_object_file(paths, &nullified_record, &entry.file_name) {
            errors.push(format!("failed to nullify {}: {err}", entry.file_name));
            continue;
        }
        entry.record = nullified_record;
    }

    // Second pass: restore locally-nullified objects whose nullifiers are
    // NOT in the on-chain set. This handles the case where a consuming
    // action's proof failed and the nullifier never landed. The object
    // is set to Unknown — it cannot be used as an action input until a
    // subsequent sync promotes it back to Live.
    for entry in objects.iter_mut() {
        if !entry.record.is_nullified() {
            continue;
        }
        let nullifier_hash = match object_nullifier_hash(&entry.record.obj) {
            Ok(hash) => hash,
            Err(_) => continue,
        };
        if on_chain_nullifiers.contains(&nullifier_hash) {
            continue; // Confirmed nullified on-chain, keep as is.
        }
        let restored_record = StoredObjectRecord {
            status: ObjectStatus::Unknown,
            ..entry.record.clone()
        };
        if let Err(err) = write_object_file(paths, &restored_record, &entry.file_name) {
            errors.push(format!("failed to restore {}: {err}", entry.file_name));
            continue;
        }
        entry.record = restored_record;
    }

    // Third pass: mark non-nullified objects as Live when their source tx
    // is in the canonical grounded set.
    for entry in objects.iter_mut() {
        if entry.record.is_nullified() || entry.record.status == ObjectStatus::Live {
            continue;
        }
        let source_tx_hash = entry.record.tx.dict().commitment();
        if !grounded_txs.contains(&source_tx_hash) {
            continue;
        }
        let live_record = StoredObjectRecord {
            status: ObjectStatus::Live,
            ..entry.record.clone()
        };
        if let Err(err) = write_object_file(paths, &live_record, &entry.file_name) {
            errors.push(format!("failed to mark {} as live: {err}", entry.file_name));
            continue;
        }
        entry.record = live_record;
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "reconcile encountered {} error(s): {}",
            errors.len(),
            errors.join("; ")
        ))
    }
}

pub(crate) fn validate_execute_request(
    input: &ExecuteActionInput,
    action: &ActionSummary,
) -> Result<()> {
    if input.input_objects.len() != action.total_inputs.len() {
        return Err(anyhow!(
            "{} expects {} inputs, got {}",
            input.action,
            action.total_inputs.len(),
            input.input_objects.len()
        ));
    }

    // Dedupe by basename so `Wood.dobj` and `/abs/path/Wood.dobj` are
    // treated as the same input — they resolve to the same managed file.
    let mut seen = HashSet::new();
    for raw in &input.input_objects {
        let file_name = extract_basename(Path::new(raw))?;
        if !seen.insert(file_name.clone()) {
            return Err(anyhow!(
                "duplicate input object is not allowed: {file_name}"
            ));
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedInput {
    pub(crate) file_name: String,
    pub(crate) record: StoredObjectRecord,
}

pub(crate) fn resolve_inputs(
    entries: &[ObjectFileEntry],
    input: &ExecuteActionInput,
    action: &ActionSummary,
) -> Result<Vec<ResolvedInput>> {
    let mut resolved_inputs = Vec::new();
    for (raw, required) in input.input_objects.iter().zip(action.total_inputs.iter()) {
        let expected_class_hash = decode_hash_hex(required.hash.as_str())?;

        let file_name = extract_basename(Path::new(raw))?;
        let entry = entries
            .iter()
            .find(|entry| entry.file_name == file_name)
            .ok_or_else(|| DriverError::ObjectFileNotFound(file_name.clone()))?;
        if entry.record.status != ObjectStatus::Live {
            return Err(anyhow!(
                "input object is not live (status: {:?}): {}",
                entry.record.status,
                entry.record.id
            ));
        }
        // Belt-and-suspenders: compare both the qualified class stored on
        // disk AND the on-chain `obj["type"]` predicate hash. Mismatch on
        // either is fatal — the second check catches files whose class
        // text drifted from the actual pod-level identity.
        if entry.record.class != required.class {
            return Err(anyhow!(
                "input class mismatch for {}: expected {}, got {}",
                entry.record.id,
                required.class,
                entry.record.class
            ));
        }
        let actual_class_hash = obj_type_hash(&entry.record.obj).ok_or_else(|| {
            anyhow!(
                "input object {} has no readable 'type' field",
                entry.record.id
            )
        })?;
        if actual_class_hash != expected_class_hash {
            return Err(anyhow!(
                "input class hash mismatch for {}: pod 'type' = {:#}, action expects {}",
                entry.record.id,
                actual_class_hash,
                required.hash,
            ));
        }
        resolved_inputs.push(ResolvedInput {
            file_name: entry.file_name.clone(),
            record: entry.record.clone(),
        });
    }
    Ok(resolved_inputs)
}

pub(crate) fn obj_type_hash(obj: &pod2::middleware::containers::Dictionary) -> Option<Hash> {
    let value = obj.get(&Key::from("type")).ok()??;
    Some(Hash(value.raw().0))
}

#[derive(Debug)]
pub(crate) struct SavedFiles {
    pub(crate) output_files: Vec<String>,
    pub(crate) nullified_files: Vec<String>,
}

pub(crate) fn save_results(
    paths: &DriverPaths,
    action: &ActionSummary,
    resolved_inputs: &[ResolvedInput],
    spendable_outputs: &SpendableObjects,
) -> Result<SavedFiles> {
    let nullified_files = resolved_inputs
        .iter()
        .map(|input| input.file_name.clone())
        .collect();

    if spendable_outputs.objs.len() != action.total_outputs.len() {
        return Err(anyhow!(
            "action {} output mismatch: descriptor expects {}, engine returned {}",
            action.action,
            action.total_outputs.len(),
            spendable_outputs.objs.len()
        ));
    }

    let mut output_files = Vec::new();
    for (index, output) in action.total_outputs.iter().enumerate() {
        let spendable = spendable_outputs.obj(index);
        let object_id = format!("{:#}", spendable.obj.commitment());
        let file_name = format!(
            "{}_{}.{DOBJ_EXTENSION}",
            output.class.file_prefix(),
            object_id.to_ascii_lowercase()
        );
        output_files.push(file_name.clone());

        let live_record = StoredObjectRecord {
            id: object_id,
            class: output.class.clone(),
            status: ObjectStatus::Unknown,
            tx_hash: None,
            pod: spendable.pod,
            obj: spendable.obj,
            tx: spendable.tx,
        };
        write_object_file(paths, &live_record, &file_name)?;
    }

    Ok(SavedFiles {
        output_files,
        nullified_files,
    })
}

/// Update the status and tx_hash of previously saved output files on disk.
pub(crate) fn update_output_files(
    paths: &DriverPaths,
    output_files: &[String],
    status: ObjectStatus,
    tx_hash: Option<&str>,
) -> Result<()> {
    for file_name in output_files {
        let file_path = paths.objects_dir.join(file_name);
        let contents = std::fs::read_to_string(&file_path)
            .map_err(|err| anyhow!("failed to read {file_name} for status update: {err}"))?;
        let mut record: StoredObjectRecord = serde_json::from_str(&contents)
            .map_err(|err| anyhow!("failed to parse {file_name} for status update: {err}"))?;
        record.status = status;
        record.tx_hash = tx_hash.map(|s| s.to_string());
        write_object_file(paths, &record, file_name)?;
    }
    Ok(())
}

pub(crate) fn build_relayer_payload(
    old_state_root_hash: &Hash,
    action_output: &SpendableObjects,
) -> Result<Vec<u8>> {
    let params = Params::default();
    let shrunk_main_pod = ShrunkMainPodSetup::new(&params)
        .build()
        .map_err(|err| anyhow!("failed to build shrunk proof circuit: {err}"))?;
    let compressed = shrink_compress_pod(&shrunk_main_pod, action_output.tx_pod.clone())
        .map_err(|err| anyhow!("failed to shrink/compress tx proof: {err}"))?;

    let tx_final = action_output.tx.dict().commitment();
    let nullifiers = action_output
        .tx
        .nullifiers
        .iter()
        .map(|entry| Ok(Hash(entry?.raw().0)))
        .collect::<Result<Vec<_>>>()?;
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final,
        state_root_hash: *old_state_root_hash,
        nullifiers,
    };

    Ok(payload.to_bytes())
}
