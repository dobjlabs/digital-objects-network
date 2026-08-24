//! Synthetic input fixtures and grounded state for driving the SDK
//! against arbitrary actions without real chain state.
//!
//! Used by `pexe inspect plan` (and reusable in unit tests). Mock mode
//! must be set on the `Executor` since the synthetic Merkle proofs are
//! structurally valid but the surrounding chain history is fabricated.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use pod2::middleware::{EMPTY_HASH, EMPTY_VALUE, Hash, StrKey, Value, containers::Array};
use pod2utils::{dict, rand_raw_value};
use sdk::{ActionMeta, ActionObjectRef, FieldFacts, SdkModule, SpendableObject};
use txlib::{GroundingWitness, STABLE_IDENTIFIER_FIELD, StateHeader, with_stable_identifier};

/// Mint one synthetic instance for each object `action` consumes,
/// positionally aligned with its `total_inputs`.
///
/// Values come from the action's own requirements rather than from the
/// class's producers, which is what makes repeated slots of one class
/// work: two `Resource` inputs the action pins to different
/// `resource_type` values get different fixtures. Fields the action
/// forces to be equal share one freshly chosen value.
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

/// What an action demands of one field, once its facts are collapsed to
/// a single answer.
enum Chosen<'a> {
    Exact(i64),
    Shared { group: &'a str, integer: bool },
    AnyInt,
    Any,
}

/// Collapse one field's facts. `Err` carries the values that cannot be
/// reconciled.
fn choose(facts: &FieldFacts) -> Result<Chosen<'_>, BTreeSet<i64>> {
    let mut pinned = facts.pinned.iter().copied();
    match (pinned.next(), pinned.next()) {
        (Some(pin), None) => match facts.min {
            // A pin below a floor the same action demands.
            Some(min) if pin < min => Err([pin, min].into_iter().collect()),
            _ => Ok(Chosen::Exact(pin)),
        },
        // A bound is satisfied at its floor, and an equality group with a
        // floor is satisfied by every member taking it.
        (None, _) => Ok(match (facts.min, facts.group.as_deref()) {
            (Some(min), _) => Chosen::Exact(min),
            (None, Some(group)) => Chosen::Shared {
                group,
                integer: facts.integer,
            },
            (None, None) if facts.integer => Chosen::AnyInt,
            (None, None) => Chosen::Any,
        }),
        _ => Err(facts.pinned.clone()),
    }
}

/// Name every field no input can satisfy in one error, rather than
/// letting the action fail later on a statement whose connection to the
/// fixture is not obvious.
fn report_unsatisfiable(action_name: &str, inputs: &[&ActionObjectRef]) -> Result<()> {
    let mut bad: Vec<String> = Vec::new();
    for (slot, obj) in inputs.iter().enumerate() {
        for (field, facts) in obj.field_facts() {
            if let Err(values) = choose(facts) {
                let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
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
        // Whatever the SDK already stamped above stays as stamped, and
        // `stable_identifier` is stamped below; inserting either twice
        // fails. Checking presence rather than naming the fields keeps
        // this from drifting as the stamped set changes.
        if d.get(&StrKey::from(field_name))
            .map_err(|err| anyhow!("reading {field_name}: {err}"))?
            .is_some()
            || field_name == STABLE_IDENTIFIER_FIELD
        {
            continue;
        }
        let value = match choose(facts) {
            Ok(Chosen::Exact(v)) => Value::from(v),
            // Mock mode drops the constraints that would otherwise bind
            // an unconstrained field to a real intro output, so any value
            // of the right kind does.
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

    /// Plan every manifest action against freshly minted inputs, ensuring
    /// that the synthetic objects are valid and preventing drift.
    /// craft-rocket is here for its sub-action calls, whose consumed
    /// objects are spliced into the caller's input list: their
    /// requirements have to arrive at the same positions or fixtures land
    /// in the wrong slots.
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

    /// Two inputs of one class that the action pins to different values.
    /// A per-class fixture cannot satisfy both, which is what made
    /// repeated slots fail.
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

    /// A body long enough that lowering splits it across several helper
    /// predicates, reading fields at both ends. Requirements are read off
    /// the instruction list, so the split is irrelevant; reading the
    /// lowered predicates instead would see only the first few.
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

        // The guard only means something if lowering really did split.
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

    /// Fields the action forces to be equal across two slots have to get
    /// one shared value, not two independently chosen ones.
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

    /// An action whose own statements demand two different values for one
    /// field cannot be satisfied by any input, so minting says so instead
    /// of handing back a fixture that fails deeper in.
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
