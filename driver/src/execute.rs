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

use crate::object_record::{ObjectRecord as StoredObjectRecord, ObjectStatus};
use crate::object_store::{ObjectFileEntry, select_object, write_object_file};
use crate::paths::DOBJ_EXTENSION;
use crate::types::{ActionSummary, DriverPaths, ExecuteActionInput, ObjectSelector};

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
            input.action_id,
            action.total_inputs.len(),
            input.input_objects.len()
        ));
    }

    let mut seen = HashSet::new();
    for selector in &input.input_objects {
        let key = match selector {
            ObjectSelector::FileName(file_name) => format!("file:{file_name}"),
            ObjectSelector::ObjectId(object_id) => format!("id:{object_id}"),
        };
        if !seen.insert(key.clone()) {
            return Err(anyhow!(
                "duplicate input object selector is not allowed: {key}"
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
    for (selector, required) in input.input_objects.iter().zip(action.total_inputs.iter()) {
        let expected_class_hash = decode_hash_hex(required.hash.as_str())?;

        let entry = select_object(entries, selector)?;
        if entry.record.status != ObjectStatus::Live {
            return Err(anyhow!(
                "input object is not live (status: {:?}): {}",
                entry.record.status,
                entry.record.id
            ));
        }
        // Belt-and-suspenders: compare both the qualified class id stored on
        // disk AND the on-chain `obj["type"]` predicate hash. Mismatch on
        // either is fatal — the second check catches files whose class_id
        // text drifted from the actual pod-level identity.
        if entry.record.class_id != required.id {
            return Err(anyhow!(
                "input class mismatch for {}: expected {}, got {}",
                entry.record.id,
                required.id,
                entry.record.class_id
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

/// Build the lowercase filename prefix for a `.dobj` of the given qualified
/// class id (`<plugin>:<class>`). Plugin names are already restricted to
/// `[A-Za-z0-9_-]` at catalog load, but class names come from arbitrary
/// rhai string literals (e.g. `action.output("…")`) so they could in
/// principle contain path-significant characters. To keep written files
/// inside `~/.dobj/objects/`, every char outside the allowlist
/// `[a-z0-9_-]` (after lowercasing) is replaced with `_`.
pub(crate) fn file_prefix_for_class(class_id: &str) -> String {
    class_id
        .chars()
        .map(|c| {
            let lower = c.to_ascii_lowercase();
            if lower.is_ascii_alphanumeric() || lower == '-' || lower == '_' {
                lower
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct SavedFiles {
    pub(crate) output_files: Vec<String>,
    pub(crate) nullified_files: Vec<String>,
}

pub(crate) fn save_results(
    paths: &DriverPaths,
    action: &ActionSummary,
    action_id: &str,
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
            action_id,
            action.total_outputs.len(),
            spendable_outputs.objs.len()
        ));
    }

    let mut output_files = Vec::new();
    for (index, output) in action.total_outputs.iter().enumerate() {
        let class_id = &output.id;
        let spendable = spendable_outputs.obj(index);
        let object_id = format!("{:#}", spendable.obj.commitment());
        let file_name = format!(
            "{}_{}.{DOBJ_EXTENSION}",
            file_prefix_for_class(class_id),
            object_id.to_ascii_lowercase()
        );
        output_files.push(file_name.clone());

        let live_record = StoredObjectRecord {
            id: object_id,
            class_id: class_id.clone(),
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
