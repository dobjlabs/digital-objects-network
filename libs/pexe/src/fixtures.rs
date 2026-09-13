//! Synthetic input fixtures and grounded state for executing SDK actions
//! without live chain state.
//!
//! Used by `pexe inspect plan` and unit tests. Mock mode must be enabled
//! on the `Executor` because synthetic Merkle proofs are structurally valid
//! but do not correspond to real chain history.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use pod2::middleware::{EMPTY_HASH, EMPTY_VALUE, Hash, StrKey, Value, containers::Array};
use pod2utils::{dict, rand_raw_value};
use sdk::{ActionMeta, ActionObjectRef, FieldFacts, Pin, SdkModule, SpendableObject};
use txlib::{GroundingWitness, STABLE_IDENTIFIER_FIELD, StateHeader, with_stable_identifier};

/// Mints a synthetic dictionary instance for each input consumed by `action`,
/// matching the order of `action.total_inputs()`.
///
/// Field values are derived directly from the action's constraints rather
/// than class declarations. This allows repeated inputs of the same class
/// with differing requirements to receive distinct fixtures, while coupled
/// fields share a single value.
pub fn mint_action_inputs(
    module: &SdkModule,
    action: &ActionMeta,
) -> Result<Vec<pod2::middleware::containers::Dictionary>> {
    let inputs: Vec<&ActionObjectRef> = action.total_inputs().collect();
    report_unsatisfiable(&action.name, &inputs)?;

    // One value per equality group, shared by every field in it.
    let mut groups: HashMap<&str, Value> = HashMap::new();
    inputs
        .iter()
        .map(|obj| mint_object(module, obj, &mut groups))
        .collect()
}

/// Resolved value requirement for a field after constraint evaluation.
enum Chosen<'a> {
    Int(i64),
    Text(&'a str),
    Shared { group: &'a str, integer: bool },
    AnyInt,
    Any,
}

/// Resolves constraints for a single field. Returns conflicting values on error.
fn choose(facts: &FieldFacts) -> Result<Chosen<'_>, BTreeSet<Pin>> {
    let mut pinned = facts.pinned.iter();
    match (pinned.next(), pinned.next(), facts.min) {
        // Pinned value violates the lower bound constraint.
        (Some(Pin::Int(v)), None, Some(min)) if *v < min => {
            Err([Pin::Int(*v), Pin::Int(min)].into_iter().collect())
        }
        (Some(Pin::Text(t)), None, Some(min)) => {
            Err([Pin::Text(t.clone()), Pin::Int(min)].into_iter().collect())
        }
        (Some(Pin::Int(v)), None, _) => Ok(Chosen::Int(*v)),
        (Some(Pin::Text(t)), None, _) => Ok(Chosen::Text(t)),
        // Satisfy lower-bound constraints using the minimum value.
        (None, _, Some(min)) => Ok(Chosen::Int(min)),
        (None, _, None) => Ok(match facts.group.as_deref() {
            Some(group) => Chosen::Shared {
                group,
                integer: facts.integer,
            },
            None if facts.integer => Chosen::AnyInt,
            None => Chosen::Any,
        }),
        _ => Err(facts.pinned.clone()),
    }
}

/// Validates that all input field constraints are satisfiable, returning a
/// consolidated error for any conflicting requirements.
fn report_unsatisfiable(action_name: &str, inputs: &[&ActionObjectRef]) -> Result<()> {
    let mut bad: Vec<String> = Vec::new();
    for (slot, obj) in inputs.iter().enumerate() {
        for (field, facts) in obj.field_facts() {
            if let Err(values) = choose(facts) {
                let values: Vec<String> = values.iter().map(Pin::to_string).collect();
                bad.push(format!(
                    "  input {slot} `{}` ({}): field `{field}` is required to be {}",
                    obj.varname(),
                    obj.class,
                    values.join(" and ")
                ));
            }
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "unsupported fixture: {action_name} constrains an input field to two different \
         values, so no synthetic input can satisfy it:\n{}",
        bad.join("\n")
    ))
}

fn mint_object<'a>(
    module: &SdkModule,
    obj: &'a ActionObjectRef,
    groups: &mut HashMap<&'a str, Value>,
) -> Result<pod2::middleware::containers::Dictionary> {
    let class_hash = module
        .class_hash(&obj.class)
        .ok_or_else(|| anyhow!("unknown class: {}", obj.class))?;

    let mut d = dict!({
        "type" => Value::from(class_hash),
        "key" => Value::from(rand_raw_value()),
        "work" => Value::from(EMPTY_VALUE),
    });

    for (field_name, facts) in obj.field_facts() {
        // Skip already-populated fields and reserved fields (`stable_identifier`).
        if d.get(&StrKey::from(field_name))
            .map_err(|err| anyhow!("reading {field_name}: {err}"))?
            .is_some()
            || field_name == STABLE_IDENTIFIER_FIELD
        {
            continue;
        }
        let value = match choose(facts) {
            Ok(Chosen::Int(v)) => Value::from(v),
            Ok(Chosen::Text(t)) => Value::from(t),
            // In mock mode, unconstrained fields default to arbitrary values of the required type.
            Ok(Chosen::AnyInt) => Value::from(0i64),
            Ok(Chosen::Any) => Value::from(rand_raw_value()),
            Ok(Chosen::Shared { group, integer }) => groups
                .entry(group)
                .or_insert_with(|| {
                    if integer {
                        Value::from(0i64)
                    } else {
                        Value::from(rand_raw_value())
                    }
                })
                .clone(),
            Err(_) => unreachable!("refused by report_unsatisfiable"),
        };
        d.insert(&StrKey::from(field_name), &value)
            .map_err(|err| anyhow!("inserting {field_name}: {err}"))?;
    }
    Ok(with_stable_identifier(&d))
}

/// Synthetic chain state and spendable objects for testing action execution
/// without an active synchronizer.
pub struct SyntheticState {
    pub grounding_witness: Arc<GroundingWitness>,
    pub spendable: Vec<SpendableObject>,
}

/// Builds a synthetic state where all provided objects are live in the created set
/// with corresponding membership proofs in a `GroundingWitness`.
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

    let state_header = StateHeader::new(
        1,
        1,
        EMPTY_HASH,
        created.commitment(),
        EMPTY_HASH,
        EMPTY_HASH,
    );

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
    const ROCKET_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/craft-rocket");

    fn load_plugin(dir: &str) -> std::rc::Rc<SdkModule> {
        let source = crate::PluginSource::read(dir).unwrap();
        let manifest = source.parse_manifest().unwrap();
        let action_names: Vec<&str> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
        Sdk::default()
            .load_module_from_src_actions(&source.script, &action_names)
            .unwrap()
    }

    fn load_craft_basics() -> std::rc::Rc<SdkModule> {
        load_plugin(PLUGIN_DIR)
    }

    fn action<'a>(module: &'a SdkModule, name: &str) -> &'a ActionMeta {
        module
            .actions()
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("no action {name}"))
    }

    #[test]
    fn mint_log_has_expected_shape() {
        let module = load_craft_basics();
        let [log] = &mint_action_inputs(&module, action(&module, "CraftWood")).unwrap()[..] else {
            panic!("CraftWood consumes exactly one object");
        };
        let class_hash = module.class_hash("Log").unwrap();
        let typ = log.get(&StrKey::from("type")).unwrap().unwrap();
        assert_eq!(typ.raw(), Value::from(class_hash).raw());
    }

    /// Verifies that synthetic inputs satisfy planning for all actions across
    /// example plugins, including actions with sub-action calls.
    #[test]
    fn every_action_plans_with_synthetic_inputs() {
        for dir in [PLUGIN_DIR, ROCKET_DIR] {
            let module = load_plugin(dir);
            for action in module.actions() {
                let minted = mint_action_inputs(&module, action).unwrap();
                assert_eq!(
                    minted.len(),
                    action.total_inputs().count(),
                    "{}: one fixture per input",
                    action.name
                );
                let state = build_synthetic_state(&minted).unwrap();
                let executor = module.executor(true, state.grounding_witness.clone());
                executor
                    .plan_action(&action.name, state.spendable)
                    .unwrap_or_else(|err| panic!("planning {} failed: {err}", action.name));
            }
        }
    }

    #[test]
    fn craft_wood_runs_end_to_end_with_synthetic_log() {
        let module = load_craft_basics();
        let minted = mint_action_inputs(&module, action(&module, "CraftWood")).unwrap();
        let state = build_synthetic_state(&minted).unwrap();

        let executor = module.executor(true, state.grounding_witness.clone());
        let outputs = executor.action("CraftWood", state.spendable).unwrap();

        // CraftWood consumes one Log, produces one Wood object.
        assert_eq!(outputs.objs.len(), 1);
        let wood = &outputs.objs[0].obj;
        let class_hash = module.class_hash("Wood").unwrap();
        let typ = wood.get(&StrKey::from("type")).unwrap().unwrap();
        assert_eq!(typ.raw(), Value::from(class_hash).raw());
    }

    /// Verifies that multiple inputs of the same class receive distinct fixtures
    /// when the action imposes different field constraints.
    #[test]
    fn repeated_class_slots_get_their_own_values() {
        let src = r#"
            fn MakeResource(action) {
                var r = action.output("Resource");
                r.set([["kind", 1], ["amount", 10]]);
            }

            fn CombineTwo(action) {
                var a = action.mutate("Resource");
                var b = action.mutate("Resource");
                action.st_sum(a.kind, 0, 1);
                action.st_sum(b.kind, 0, 2);
                action.st_sum(a.amount, 0, 10);
                action.st_sum(b.amount, 0, 10);
            }
        "#;
        let module = Sdk::default()
            .load_module_from_src_actions(src, &["MakeResource", "CombineTwo"])
            .unwrap();

        let minted = mint_action_inputs(&module, action(&module, "CombineTwo")).unwrap();
        let kind = |d: &pod2::middleware::containers::Dictionary| {
            d.get(&StrKey::from("kind")).unwrap().unwrap().as_int()
        };
        assert_eq!(kind(&minted[0]), Some(1));
        assert_eq!(kind(&minted[1]), Some(2));

        let state = build_synthetic_state(&minted).unwrap();
        let executor = module.executor(true, state.grounding_witness.clone());
        executor.plan_action("CombineTwo", state.spendable).unwrap();
    }

    /// Verifies that field requirements are preserved even when action lowering
    /// splits the body across multiple helper predicates.
    #[test]
    fn deeply_split_body_keeps_every_field() {
        let src = r#"
            fn MakeWidget(action) {
                var w = action.output("Widget");
                w.set([
                    ["f01", 1], ["f02", 2], ["f03", 3], ["f04", 4], ["f05", 5],
                    ["f06", 6], ["f07", 7], ["f08", 8], ["f09", 9], ["f10", 10],
                ]);
            }

            fn ReadEveryField(action) {
                var w = action.mutate("Widget");
                action.st_sum(w.f01, 0, 1);
                action.st_sum(w.f02, 0, 2);
                action.st_sum(w.f03, 0, 3);
                action.st_sum(w.f04, 0, 4);
                action.st_sum(w.f05, 0, 5);
                action.st_sum(w.f06, 0, 6);
                action.st_sum(w.f07, 0, 7);
                action.st_sum(w.f08, 0, 8);
                action.st_sum(w.f09, 0, 9);
                action.st_sum(w.f10, 0, 10);
            }
        "#;
        let module = Sdk::default()
            .load_module_from_src_actions(src, &["MakeWidget", "ReadEveryField"])
            .unwrap();

        // Ensure lowering generated helper predicates.
        let levels = module
            .module()
            .batch
            .predicates()
            .iter()
            .filter(|p| p.name.starts_with("ReadEveryField_"))
            .count();
        assert!(levels >= 2, "expected a split body, got {levels} helpers");

        let minted = mint_action_inputs(&module, action(&module, "ReadEveryField")).unwrap();
        let [widget] = &minted[..] else {
            panic!("one input");
        };
        for i in 1..=10 {
            let field = format!("f{i:02}");
            let got = widget.get(&StrKey::from(field.as_str())).unwrap();
            assert_eq!(
                got.and_then(|v| v.as_int()),
                Some(i),
                "field {field} missing or wrong"
            );
        }

        let state = build_synthetic_state(&minted).unwrap();
        let executor = module.executor(true, state.grounding_witness.clone());
        executor
            .plan_action("ReadEveryField", state.spendable)
            .unwrap();
    }

    /// Verifies that coupled fields across distinct input slots share the same value.
    #[test]
    fn coupled_fields_across_slots_share_a_value() {
        let src = r#"
            fn MakeParts(action) {
                var a = action.output("Left");
                a.set([["here", 3]]);
                var b = action.output("Right");
                b.set([["there", 3]]);
            }

            fn CoupleThem(action) {
                var a = action.mutate("Left");
                var b = action.mutate("Right");
                action.st_sum(a.here, 0, b.there);
            }
        "#;
        let module = Sdk::default()
            .load_module_from_src_actions(src, &["MakeParts", "CoupleThem"])
            .unwrap();

        let minted = mint_action_inputs(&module, action(&module, "CoupleThem")).unwrap();
        let here = minted[0].get(&StrKey::from("here")).unwrap().unwrap();
        let there = minted[1].get(&StrKey::from("there")).unwrap().unwrap();
        assert_eq!(here.raw(), there.raw());

        let state = build_synthetic_state(&minted).unwrap();
        let executor = module.executor(true, state.grounding_witness.clone());
        executor.plan_action("CoupleThem", state.spendable).unwrap();
    }

    /// Verifies that conflicting field constraints produce an error during fixture generation.
    #[test]
    fn contradictory_pins_are_reported_not_minted() {
        let src = r#"
            fn MakeThing(action) {
                var t = action.output("Thing");
                t.set([["n", 1]]);
            }

            fn WantsBoth(action) {
                var t = action.mutate("Thing");
                action.st_sum(t.n, 0, 1);
                action.st_sum(t.n, 0, 2);
            }
        "#;
        let module = Sdk::default()
            .load_module_from_src_actions(src, &["MakeThing", "WantsBoth"])
            .unwrap();

        let err = match mint_action_inputs(&module, action(&module, "WantsBoth")) {
            Ok(_) => panic!("expected the contradiction to be reported"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("unsupported fixture"), "{err}");
        assert!(err.contains("`n`"), "{err}");
        assert!(err.contains("1 and 2"), "{err}");
    }
}
