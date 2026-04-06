use std::collections::HashSet;

use anyhow::{Result, anyhow};
use common::{
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use craft_sdk::SpendableObjects;
use pod2::middleware::{Hash, Params};
use txlib::object_nullifier_hash;

use crate::object_record::{ObjectRecord as StoredObjectRecord, ObjectStatus};
use crate::object_store::{ObjectFileEntry, select_object, write_object_file};
use crate::types::{ActionSummary, DriverPaths, ExecuteActionInput, ObjectSelector};

pub(crate) fn reconcile_objects(
    paths: &DriverPaths,
    objects: &mut [ObjectFileEntry],
    on_chain_nullifiers: &HashSet<Hash>,
) {
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
            id: entry.record.id.clone(),
            class_name: entry.record.class_name.clone(),
            status: ObjectStatus::Nullified,
            pod: entry.record.pod.clone(),
            obj: entry.record.obj.clone(),
            tx: entry.record.tx.clone(),
        };
        if let Err(err) = write_object_file(paths, &nullified_record, &entry.file_name) {
            eprintln!(
                "zk-craft: reconcile failed to nullify {}: {err}",
                entry.file_name
            );
            continue;
        }
        entry.record = nullified_record;
    }
}

pub(crate) fn validate_execute_request(
    input: &ExecuteActionInput,
    action: &ActionSummary,
) -> Result<()> {
    if input.input_objects.len() != action.input_classes.len() {
        return Err(anyhow!(
            "{} expects {} inputs, got {}",
            input.action_id,
            action.input_classes.len(),
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
    for (slot, selector) in input.input_objects.iter().enumerate() {
        let expected_class = action.input_classes[slot].as_str();
        let entry = select_object(entries, selector)?;
        if entry.record.is_nullified() {
            return Err(anyhow!("input object is not live: {}", entry.record.id));
        }
        if entry.record.class_name != expected_class {
            return Err(anyhow!(
                "input class mismatch for {}: expected {}, got {}",
                entry.record.id,
                expected_class,
                entry.record.class_name
            ));
        }
        resolved_inputs.push(ResolvedInput {
            file_name: entry.file_name.clone(),
            record: entry.record.clone(),
        });
    }
    Ok(resolved_inputs)
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
    let mut nullified_files = Vec::new();
    for input in resolved_inputs {
        let input_record = &input.record;
        let nullified_record = StoredObjectRecord {
            id: input_record.id.clone(),
            class_name: input_record.class_name.clone(),
            status: ObjectStatus::Nullified,
            pod: input_record.pod.clone(),
            obj: input_record.obj.clone(),
            tx: input_record.tx.clone(),
        };
        write_object_file(paths, &nullified_record, &input.file_name)?;
        nullified_files.push(input.file_name.clone());
    }

    if spendable_outputs.objs.len() != action.output_classes.len() {
        return Err(anyhow!(
            "action {} output mismatch: descriptor expects {}, engine returned {}",
            action_id,
            action.output_classes.len(),
            spendable_outputs.objs.len()
        ));
    }

    let mut output_files = Vec::new();
    for (index, class_name) in action.output_classes.iter().enumerate() {
        let spendable = spendable_outputs.obj(index);
        let object_id = format!("{:#}", spendable.obj.commitment());
        let file_name = format!(
            "{}_{}.dobj",
            class_name.to_ascii_lowercase(),
            object_id.to_ascii_lowercase()
        );
        output_files.push(file_name.clone());

        let live_record = StoredObjectRecord {
            id: object_id,
            class_name: class_name.clone(),
            status: ObjectStatus::Pending,
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

pub(crate) fn rollback_results(
    paths: &DriverPaths,
    resolved_inputs: &[ResolvedInput],
    saved: &SavedFiles,
) {
    for input in resolved_inputs {
        let live_record = StoredObjectRecord {
            id: input.record.id.clone(),
            class_name: input.record.class_name.clone(),
            status: input.record.status,
            pod: input.record.pod.clone(),
            obj: input.record.obj.clone(),
            tx: input.record.tx.clone(),
        };
        if let Err(err) = write_object_file(paths, &live_record, &input.file_name) {
            eprintln!("zk-craft: rollback failed for {}: {err}", input.file_name);
        }
    }
    for file_name in &saved.output_files {
        let path = paths.objects_dir.join(file_name);
        match std::fs::remove_file(&path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("zk-craft: rollback failed to remove {file_name}: {err}");
            }
        }
    }
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
