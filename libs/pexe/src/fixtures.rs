//! Synthetic input fixtures and grounded state for driving the SDK
//! against arbitrary actions without real chain state.
//!
//! Used by `pexe inspect plan` (and reusable in unit tests). Mock mode
//! must be set on the `Executor` since the synthetic Merkle proofs are
//! structurally valid but the surrounding chain history is fabricated.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use pod2::middleware::{EMPTY_HASH, EMPTY_VALUE, Hash, StrKey, Value, containers::Array};
use pod2utils::{dict, rand_raw_value};
use sdk::{SdkModule, SpendableObject};
use txlib::{GroundingWitness, StateHeader, with_stable_identifier};

use crate::inspect::derive_class_signature;

/// Mint a synthetic instance of `class_name` whose dict shape matches
/// the class's IsX rule. Fields the signature analyzer recognises
/// (string literals, int initials, witnesses) are populated with a
/// representative value; SDK-pre-populated keys (`type`, `key`, `work`)
/// are added in all cases.
pub fn mint_class(
    module: &SdkModule,
    class_name: &str,
) -> Result<pod2::middleware::containers::Dictionary> {
    let batch = &module.module().batch;
    let signature = derive_class_signature(module, batch, class_name);
    mint_with_signature(module, class_name, &signature)
}

/// Mint one synthetic instance per class name. Class signatures are
/// memoized so repeating a class (e.g. `[Wire, Wire, Steel]`) doesn't
/// re-derive the same signature.
pub fn mint_classes(
    module: &SdkModule,
    class_names: &[String],
) -> Result<Vec<pod2::middleware::containers::Dictionary>> {
    let batch = &module.module().batch;
    let mut cache: HashMap<&str, crate::inspect::ClassSignature> = HashMap::new();
    let mut out = Vec::with_capacity(class_names.len());
    for class in class_names {
        let sig = cache
            .entry(class.as_str())
            .or_insert_with(|| derive_class_signature(module, batch, class));
        out.push(mint_with_signature(module, class, sig)?);
    }
    Ok(out)
}

fn mint_with_signature(
    module: &SdkModule,
    class_name: &str,
    signature: &crate::inspect::ClassSignature,
) -> Result<pod2::middleware::containers::Dictionary> {
    let class_hash = module
        .class_hash(class_name)
        .ok_or_else(|| anyhow!("unknown class: {class_name}"))?;

    let mut d = dict!({
        "type" => Value::from(class_hash),
        "key" => Value::from(rand_raw_value()),
        "work" => Value::from(EMPTY_VALUE),
    });

    for (field_name, info) in &signature.fields {
        // `type`/`key`/`work` are SDK-pre-populated and already stamped.
        if matches!(field_name.as_str(), "type" | "key" | "work") {
            continue;
        }
        let value: Value = if let Some(literal) = info.string_literals.iter().next() {
            Value::from(literal.clone())
        } else if let Some(initial) = info.int_literals.iter().next() {
            Value::from(*initial)
        } else {
            // Witness-derived application field: hand it a random Raw.
            // Mock mode skips the constraints that would otherwise bind
            // these values to real intro outputs.
            Value::from(rand_raw_value())
        };
        d.insert(&StrKey::from(field_name.as_str()), &value)
            .map_err(|err| anyhow!("inserting {field_name}: {err}"))?;
    }
    // A real chain object carries `stable_identifier = commitment(initial)`,
    // stamped by TxInsert when it was first minted. Synthetic inputs stand
    // in for chain objects, so they need the same field or a later mutate
    // (which pins old.stable_identifier == new.stable_identifier) panics on
    // the missing entry.
    Ok(with_stable_identifier(&d))
}

/// Result of fabricating a synthetic chain state that grounds a set of
/// input objects. Pair this with `executor.action(name, spendable)` to
/// drive an action end-to-end without touching the real synchronizer.
pub struct SyntheticState {
    pub grounding_witness: Arc<GroundingWitness>,
    pub spendable: Vec<SpendableObject>,
}

/// Build a state in which each `obj` is Live, by inserting every object
/// into a single global created set (an array, indexed by position) and
/// packaging per-object `(index, membership proof)` into a `GroundingWitness`.
pub fn build_synthetic_state(
    objs: &[pod2::middleware::containers::Dictionary],
) -> Result<SyntheticState> {
    let mut created: Array = Array::new(Vec::new());
    let mut indices: HashMap<Hash, i64> = HashMap::with_capacity(objs.len());
    for obj in objs {
        let commitment = obj.commitment();
        if indices.contains_key(&commitment) {
            continue;
        }
        let index = indices.len() as i64;
        created
            .insert(index as usize, Value::from(obj.clone()))
            .map_err(|err| anyhow!("recording synthetic created object: {err}"))?;
        indices.insert(commitment, index);
    }

    let state_header = StateHeader::new(1, created.commitment(), EMPTY_HASH, EMPTY_HASH);

    let mut created_proofs: HashMap<Hash, _> = HashMap::with_capacity(objs.len());
    for obj in objs {
        let commitment = obj.commitment();
        let index = indices[&commitment];
        let (_value, proof) = created
            .prove(index as usize)
            .map_err(|err| anyhow!("proving synthetic created-set membership: {err}"))?;
        created_proofs.insert(commitment, (index, proof));
    }

    let grounding_witness = Arc::new(GroundingWitness::new(state_header, created_proofs));

    let spendable: Vec<SpendableObject> = objs
        .iter()
        .map(|obj| SpendableObject { obj: obj.clone() })
        .collect();

    Ok(SyntheticState {
        grounding_witness,
        spendable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::Sdk;

    const PLUGIN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/craft-basics");

    fn load_craft_basics() -> std::rc::Rc<SdkModule> {
        let source = crate::PluginSource::read(PLUGIN_DIR).unwrap();
        let manifest = source.parse_manifest().unwrap();
        let action_names: Vec<&str> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
        Sdk::default()
            .load_module_from_src_actions(&source.script, &action_names)
            .unwrap()
    }

    #[test]
    fn mint_log_has_expected_shape() {
        let module = load_craft_basics();
        let log = mint_class(&module, "Log").unwrap();
        let class_hash = module.class_hash("Log").unwrap();
        let typ = log.get(&StrKey::from("type")).unwrap().unwrap();
        assert_eq!(typ.raw(), Value::from(class_hash).raw());
    }

    /// Plan every manifest action against freshly minted inputs, ensuring
    /// that the synthetic objects are valid and preventing drift.
    #[test]
    fn every_action_plans_with_synthetic_inputs() {
        let module = load_craft_basics();
        for action in module.actions() {
            let input_classes: Vec<String> =
                action.total_inputs().map(|r| r.class.clone()).collect();
            let minted = mint_classes(&module, &input_classes).unwrap();
            let state = build_synthetic_state(&minted).unwrap();
            let executor = module.executor(true, state.grounding_witness.clone());
            executor
                .plan_action(&action.name, state.spendable)
                .unwrap_or_else(|err| panic!("planning {} failed: {err}", action.name));
        }
    }

    #[test]
    fn craft_wood_runs_end_to_end_with_synthetic_log() {
        let module = load_craft_basics();
        let log = mint_class(&module, "Log").unwrap();
        let state = build_synthetic_state(&[log]).unwrap();

        let executor = module.executor(true, state.grounding_witness.clone());
        let outputs = executor.action("CraftWood", state.spendable).unwrap();

        // CraftWood consumes one Log, produces one Wood object.
        assert_eq!(outputs.objs.len(), 1);
        let wood = &outputs.objs[0].obj;
        let class_hash = module.class_hash("Wood").unwrap();
        let typ = wood.get(&StrKey::from("type")).unwrap().unwrap();
        assert_eq!(typ.raw(), Value::from(class_hash).raw());
    }
}
