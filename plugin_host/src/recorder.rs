//! Rhai "recording mode" interpreter.
//!
//! Runs each action function with stub host functions that only record calls,
//! building up `Vec<StepMeta>` for podlang generation and proof-time recipe
//! dispatch.
//!
//! The manifest provides all static metadata (name, version, imports, deps,
//! classes, action UI). The recorder only extracts the _step structure_ from
//! the Rhai script so that `recipes.rs` can build proof-generation closures.
//!
//! ## Host function conventions in recording mode
//!
//! - Object handles are step indices (`i64`)
//! - `vdf`, `pow_grind`, `random_key` return the var name (`ImmutableString`)
//!   so `update(obj, key, var_name)` can record `source = var_name`
//! - `get_int` returns a dummy integer (100) so Rhai arithmetic works;
//!   it records `VarRecipe::DecrementField` which reads + decrements at proof time
//! - `sum_of(obj, key, int_val, b)` ignores the int and records
//!   `SumOf { stored_var: key }` (convention: `get_int` stores under `key`)
//! - `update_int(obj, key, int_val)` records `Update { source: key }`

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;
use plugin_api::*;
use rhai::{Dynamic, Engine, ImmutableString, Scope, AST};

// ---------------------------------------------------------------------------
// Recorder state — shared between all host function closures via Rc<RefCell>
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RecorderState {
    steps: Vec<StepMeta>,
}

impl RecorderState {
    fn push_step(
        &mut self,
        kind: StepKindMeta,
        name: String,
        class: String,
        action: String,
    ) -> i64 {
        let idx = self.steps.len() as i64;
        self.steps.push(StepMeta {
            kind,
            name,
            class,
            action,
            details: vec![],
        });
        idx
    }

    fn add_detail(&mut self, handle: i64, detail: DetailMeta) {
        self.steps[handle as usize].details.push(detail);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run a single action function in recording mode, returning the captured steps.
///
/// `fn_name` is the Rhai function to call (from `ActionMeta.fn_name`).
pub(crate) fn record_action_steps(
    ast: &AST,
    fn_name: &str,
) -> Result<Vec<StepMeta>> {
    let state = Rc::new(RefCell::new(RecorderState::default()));
    let engine = create_recording_engine(Rc::clone(&state));
    let mut scope = Scope::new();

    let _ = engine
        .call_fn::<Dynamic>(&mut scope, ast, fn_name, ())
        .map_err(|e| anyhow::anyhow!("failed to record action function '{fn_name}': {e}"))?;

    let recorded = state.borrow();
    Ok(recorded.steps.clone())
}

// ---------------------------------------------------------------------------
// Recording engine — register stub host functions
// ---------------------------------------------------------------------------

/// Placeholder for TinyTemplate's `{state}` variable in pred strings.
/// We can't use `format!("{state}")` because Rust would try to format a
/// variable named `state`.
const S: &str = "{state}";

fn create_recording_engine(state: Rc<RefCell<RecorderState>>) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(10_000);
    engine.set_max_array_size(1_000);
    engine.set_max_map_size(100);

    // ── Object lifecycle ──────────────────────────────────────────────

    let s = Rc::clone(&state);
    engine.register_fn(
        "output",
        move |name: ImmutableString, class: ImmutableString| -> i64 {
            s.borrow_mut()
                .push_step(StepKindMeta::Output, name.into(), class.into(), String::new())
        },
    );

    let s = Rc::clone(&state);
    engine.register_fn(
        "input",
        move |name: ImmutableString, class: ImmutableString| -> i64 {
            s.borrow_mut()
                .push_step(StepKindMeta::Input, name.into(), class.into(), String::new())
        },
    );

    let s = Rc::clone(&state);
    engine.register_fn(
        "mutate",
        move |name: ImmutableString, class: ImmutableString| -> i64 {
            s.borrow_mut()
                .push_step(StepKindMeta::Mutate, name.into(), class.into(), String::new())
        },
    );

    let s = Rc::clone(&state);
    engine.register_fn(
        "depends",
        move |name: ImmutableString, action: ImmutableString| {
            s.borrow_mut().push_step(
                StepKindMeta::Depends,
                name.into(),
                String::new(),
                action.into(),
            );
        },
    );

    // ── Field operations: set ─────────────────────────────────────────

    // set(handle, key, string_value)
    let s = Rc::clone(&state);
    engine.register_fn(
        "set",
        move |handle: i64, key: ImmutableString, value: ImmutableString| {
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Set {
                    key: key.into(),
                    value: LiteralValue::Str(value.into()),
                },
            );
        },
    );

    // set_int(handle, key, int_value)
    let s = Rc::clone(&state);
    engine.register_fn(
        "set_int",
        move |handle: i64, key: ImmutableString, value: i64| {
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Set {
                    key: key.into(),
                    value: LiteralValue::Int(value),
                },
            );
        },
    );

    // ── Field operations: update ──────────────────────────────────────

    // update(handle, key, var_name_string)
    // Used after vdf/pow_grind/random_key which return the var name as a string.
    let s = Rc::clone(&state);
    engine.register_fn(
        "update",
        move |handle: i64, key: ImmutableString, source: ImmutableString| {
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Update {
                    key: key.into(),
                    source: source.into(),
                },
            );
        },
    );

    // update_int(handle, key, int_value)
    // Used after get_int + arithmetic. Convention: source var name = key name
    // (get_int stores the computed value under the field key).
    let s = Rc::clone(&state);
    engine.register_fn(
        "update_int",
        move |handle: i64, key: ImmutableString, _value: i64| {
            let key_str: String = key.into();
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Update {
                    key: key_str.clone(),
                    source: key_str,
                },
            );
        },
    );

    // ── Field read ────────────────────────────────────────────────────

    // get_int(handle, key) → dummy int for Rhai arithmetic
    // Records VarRecipe::DecrementField which, at proof time, reads the field
    // value, decrements by 1, and stores the result for sum_of.
    let s = Rc::clone(&state);
    engine.register_fn(
        "get_int",
        move |handle: i64, key: ImmutableString| -> i64 {
            let key_str: String = key.into();
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Var {
                    name: key_str.clone(),
                    recipe: VarRecipe::DecrementField { key: key_str },
                },
            );
            100 // dummy value — script arithmetic runs but result is ignored
        },
    );

    // ── Intro pods ────────────────────────────────────────────────────

    // obj_raw(handle) → passthrough
    engine.register_fn("obj_raw", |handle: i64| -> i64 { handle });

    // vdf(iters, obj_raw_handle) → var name "work"
    let s = Rc::clone(&state);
    engine.register_fn(
        "vdf",
        move |iters: i64, handle: i64| -> ImmutableString {
            let mut st = s.borrow_mut();
            st.add_detail(
                handle,
                DetailMeta::Var {
                    name: "work".into(),
                    recipe: VarRecipe::Vdf {
                        iters: iters as usize,
                    },
                },
            );
            st.add_detail(
                handle,
                DetailMeta::Condition {
                    pred: format!("Vdf({iters}, {S}, work)"),
                    recipe: ConditionRecipe::StoredVdfPod,
                },
            );
            "work".into()
        },
    );

    // pow_grind(handle, difficulty) → var name "key"
    let s = Rc::clone(&state);
    engine.register_fn(
        "pow_grind",
        move |handle: i64, difficulty: i64| -> ImmutableString {
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Var {
                    name: "key".into(),
                    recipe: VarRecipe::PowGrind {
                        difficulty: difficulty as u64,
                    },
                },
            );
            "key".into()
        },
    );

    // lt_eq_u256(handle, difficulty)
    let s = Rc::clone(&state);
    engine.register_fn(
        "lt_eq_u256",
        move |handle: i64, difficulty: i64| {
            let diff_u64 = difficulty as u64;
            let pred = format!(
                "LtEqU256({S}, Raw(0x{:016x}{}))",
                diff_u64,
                "0".repeat(48),
            );
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Condition {
                    pred,
                    recipe: ConditionRecipe::LtEqU256 {
                        difficulty: diff_u64,
                    },
                },
            );
        },
    );

    // ── Proof conditions ──────────────────────────────────────────────

    // gt(handle, key, value)
    let s = Rc::clone(&state);
    engine.register_fn(
        "gt",
        move |handle: i64, key: ImmutableString, value: i64| {
            let key_str: String = key.into();
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Condition {
                    pred: format!("Gt({S}.{key_str}, {value})"),
                    recipe: ConditionRecipe::Gt {
                        key: key_str,
                        value,
                    },
                },
            );
        },
    );

    // sum_of(handle, key, int_val, b) — int version (after get_int + arithmetic)
    // Convention: stored_var = key (get_int stores under the field key name).
    let s = Rc::clone(&state);
    engine.register_fn(
        "sum_of",
        move |handle: i64, key: ImmutableString, _value: i64, b: i64| {
            let key_str: String = key.into();
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Condition {
                    pred: format!("SumOf({S}.{key_str}, {key_str}, {b})"),
                    recipe: ConditionRecipe::SumOf {
                        key: key_str.clone(),
                        stored_var: key_str,
                        b,
                    },
                },
            );
        },
    );

    // ── Utilities ─────────────────────────────────────────────────────

    // random_key(handle) → var name "key"
    let s = Rc::clone(&state);
    engine.register_fn(
        "random_key",
        move |handle: i64| -> ImmutableString {
            s.borrow_mut().add_detail(
                handle,
                DetailMeta::Var {
                    name: "key".into(),
                    recipe: VarRecipe::RandomKey,
                },
            );
            "key".into()
        },
    );

    engine
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_and_record(script: &str, fn_name: &str) -> Vec<StepMeta> {
        let engine = Engine::new();
        let ast = engine.compile(script).expect("compile failed");
        record_action_steps(&ast, fn_name).expect("record failed")
    }

    #[test]
    fn test_record_find_log() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/find_log/plugin.rhai"),
            "FindLog",
        );
        assert_eq!(steps.len(), 1);
        assert!(matches!(steps[0].kind, StepKindMeta::Output));
        assert_eq!(steps[0].name, "log");
        assert_eq!(steps[0].class, "Log");
        // Set "blueprint" + Var "work" + Condition StoredVdfPod + Update "work"
        assert_eq!(steps[0].details.len(), 4);
    }

    #[test]
    fn test_record_craft_wood() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/craft_wood/plugin.rhai"),
            "CraftWood",
        );
        assert_eq!(steps.len(), 2); // input + output
        assert!(matches!(steps[0].kind, StepKindMeta::Input));
        assert!(matches!(steps[1].kind, StepKindMeta::Output));
        // output details: Set + Var(PowGrind) + Update + Condition(LtEqU256)
        assert_eq!(steps[1].details.len(), 4);
    }

    #[test]
    fn test_record_craft_sticks() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/craft_sticks/plugin.rhai"),
            "CraftSticks",
        );
        assert_eq!(steps.len(), 3); // 1 input + 2 outputs
        assert!(matches!(steps[0].kind, StepKindMeta::Input));
        assert!(matches!(steps[1].kind, StepKindMeta::Output));
        assert!(matches!(steps[2].kind, StepKindMeta::Output));
    }

    #[test]
    fn test_record_craft_wood_pick() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/craft_wood_pick/plugin.rhai"),
            "CraftWoodPick",
        );
        assert_eq!(steps.len(), 3); // 2 inputs + 1 output
        assert!(matches!(steps[0].kind, StepKindMeta::Input));
        assert!(matches!(steps[1].kind, StepKindMeta::Input));
        assert!(matches!(steps[2].kind, StepKindMeta::Output));
        // output: Set "blueprint" + Set "durability"
        assert_eq!(steps[2].details.len(), 2);
    }

    #[test]
    fn test_record_use_wood_pick() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/use_wood_pick/plugin.rhai"),
            "UseWoodPick",
        );
        assert_eq!(steps.len(), 1); // 1 mutate
        assert!(matches!(steps[0].kind, StepKindMeta::Mutate));
        assert_eq!(steps[0].name, "wood_pick");
        assert_eq!(steps[0].class, "WoodPick");
        // Var(durability) + Condition(Gt) + Condition(SumOf) + Update(durability)
        // + Var(key) + Update(key) + Var(work) + Condition(StoredVdfPod) + Update(work)
        assert_eq!(steps[0].details.len(), 9);
    }

    #[test]
    fn test_record_stone_tools_mine() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/stone_tools/plugin.rhai"),
            "MineStoneWithWoodPick",
        );
        assert_eq!(steps.len(), 2); // depends + output
        assert!(matches!(steps[0].kind, StepKindMeta::Depends));
        assert_eq!(steps[0].action, "UseWoodPick");
        assert!(matches!(steps[1].kind, StepKindMeta::Output));
        assert_eq!(steps[1].class, "Stone");
    }

    #[test]
    fn test_record_craft_stone_pick() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/stone_tools/plugin.rhai"),
            "CraftStonePick",
        );
        assert_eq!(steps.len(), 3); // 2 inputs + 1 output
        assert!(matches!(steps[0].kind, StepKindMeta::Input));
        assert!(matches!(steps[1].kind, StepKindMeta::Input));
        assert!(matches!(steps[2].kind, StepKindMeta::Output));
    }

    #[test]
    fn test_record_use_stone_pick() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/stone_tools/plugin.rhai"),
            "UseStonePick",
        );
        assert_eq!(steps.len(), 1); // 1 mutate
        assert!(matches!(steps[0].kind, StepKindMeta::Mutate));
        assert_eq!(steps[0].name, "stone_pick");
        assert_eq!(steps[0].class, "StonePick");
        // Same 9 details as UseWoodPick (use_pick helper)
        assert_eq!(steps[0].details.len(), 9);
    }

    #[test]
    fn test_record_mine_stone_with_stone_pick() {
        let steps = compile_and_record(
            include_str!("../../data/plugins/actions/stone_tools/plugin.rhai"),
            "MineStoneWithStonePick",
        );
        assert_eq!(steps.len(), 2); // depends + output
        assert!(matches!(steps[0].kind, StepKindMeta::Depends));
        assert_eq!(steps[0].action, "UseStonePick");
    }
}
