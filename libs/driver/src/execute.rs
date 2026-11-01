use std::collections::HashSet;

use anyhow::{Result, anyhow};
use payload::{
    decode_hash_hex,
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use pod2::middleware::{Hash, Params, StrKey, Value};
use sdk::{ActionArgs, SpendableObjects};

use std::path::Path;

use crate::driver::extract_basename;
use crate::error::DriverError;
use crate::object_record::{FileStatus, ObjectRecord as StoredObjectRecord};
use crate::object_store::ObjectFileEntry;
use crate::types::ExecuteActionInput;
use wire_types::{ActionArgValues, ActionSummary};

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

pub(crate) fn decode_action_args(args: &ActionArgValues) -> Result<ActionArgs> {
    args.iter()
        .map(|(name, json)| {
            let value: Value = serde_json::from_value(json.clone()).map_err(|err| {
                anyhow!(
                    "argument `{name}`: not a pod2 value ({err}); use {{\"Untyped\": \"<64 hex \
                     chars>\"}}, {{\"Int\": \"<decimal>\"}} or a string"
                )
            })?;
            Ok((name.clone(), value))
        })
        .collect()
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
        if entry.record.metadata.status != FileStatus::Live {
            return Err(anyhow!(
                "input object is not live (status: {:?}): {:#}",
                entry.record.metadata.status,
                entry.record.metadata.content_hash
            ));
        }
        // Belt-and-suspenders: compare both the qualified class stored on
        // disk AND the on-chain `obj["type"]` predicate hash. Mismatch on
        // either is fatal — the second check catches files whose class
        // text drifted from the actual pod-level identity.
        if entry.record.metadata.class != required.class {
            return Err(anyhow!(
                "input class mismatch for {:#}: expected {}, got {}",
                entry.record.metadata.content_hash,
                required.class,
                entry.record.metadata.class
            ));
        }
        let actual_class_hash = obj_type_hash(&entry.record.obj).ok_or_else(|| {
            anyhow!(
                "input object {:#} has no readable 'type' field",
                entry.record.metadata.content_hash
            )
        })?;
        if actual_class_hash != expected_class_hash {
            return Err(anyhow!(
                "input class hash mismatch for {:#}: pod 'type' = {:#}, action expects {}",
                entry.record.metadata.content_hash,
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
    let value = obj.get(&StrKey::from("type")).ok()??;
    Some(Hash(value.raw().0))
}

pub(crate) fn build_relayer_payload(
    old_state_root: &Hash,
    action_output: &SpendableObjects,
) -> Result<Vec<u8>> {
    let params = Params::default();
    let shrunk_main_pod = ShrunkMainPodSetup::new(&params)
        .build()
        .map_err(|err| anyhow!("failed to build shrunk proof circuit: {err}"))?;
    let compressed = shrink_compress_pod(&shrunk_main_pod, action_output.tx_pod.clone())
        .map_err(|err| anyhow!("failed to shrink/compress tx proof: {err}"))?;

    let payload = Payload::from_tx(
        PayloadProof::Plonky2(Box::new(compressed)),
        &action_output.tx,
        *old_state_root,
    )?;

    Ok(payload.to_bytes())
}

#[cfg(test)]
mod action_args_tests {
    use super::*;

    #[test]
    fn argument_values_decode_from_json() {
        let json: ActionArgValues = [
            (
                "key".to_string(),
                serde_json::json!({"Untyped": format!("{:0>64}", "beef")}),
            ),
            ("n".to_string(), serde_json::json!({"Int": "42"})),
        ]
        .into();
        let decoded = decode_action_args(&json).unwrap();
        assert_eq!(decoded["n"], Value::from(42));
        assert_eq!(serde_json::to_value(&decoded["key"]).unwrap(), json["key"]);

        let bad: ActionArgValues = [("key".to_string(), serde_json::json!({"Int": 7}))].into();
        let err = decode_action_args(&bad).unwrap_err().to_string();
        assert!(err.contains("argument `key`"), "{err}");
    }
}
