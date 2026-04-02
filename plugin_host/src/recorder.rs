//! Rhai "recording mode" interpreter.
//!
//! Runs each action function with stub host functions that only record calls,
//! building up `PluginMetadata` identical to what the old WASM plugin returned.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use plugin_api::*;
use rhai::{AST, Dynamic, Engine, ImmutableString, Scope};

// ---------------------------------------------------------------------------
// Recorder state — shared between all host function closures
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RecorderState {
    steps: Vec<StepMeta>,
}

impl RecorderState {
    fn push_step(&mut self, kind: StepKindMeta, name: String, class: String, action: String) -> i64 {
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

/// Create a Rhai engine with sandbox limits (no recording functions yet).
pub(crate) fn create_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(10_000);
    engine.set_max_array_size(1_000);
    engine.set_max_map_size(100);
    engine
}

/// Load a Rhai script and extract full `PluginMetadata`.
pub(crate) fn record_plugin(engine: &Engine, ast: &AST) -> Result<PluginMetadata> {
    // Step 1: evaluate top-level code to populate globals
    let mut scope = Scope::new();
    engine
        .eval_ast_with_scope::<()>(&mut scope, ast)
        .map_err(|e| anyhow::anyhow!("failed to evaluate plugin script globals: {e}"))?;

    // Step 2: read metadata globals from scope
    let plugin_map = get_map(&scope, "plugin")?;
    let name = map_str(&plugin_map, "name")?;
    let version = map_str(&plugin_map, "version")?;

    let dependencies = read_dependencies(&scope)?;
    let classes = read_classes(&scope)?;
    let action_defs = read_action_defs(&scope)?;

    // Step 3: run each action function in recording mode
    let mut actions = Vec::new();
    for def in &action_defs {
        let fn_name = map_str(def, "fn_name")?;
        let state = Rc::new(RefCell::new(RecorderState::default()));
        let rec_engine = create_recording_engine(Rc::clone(&state));

        // Provide an empty scope — action functions don't need globals
        let mut fn_scope = Scope::new();
        let _ = rec_engine
            .call_fn::<Dynamic>(&mut fn_scope, ast, &fn_name, ())
            .map_err(|e| anyhow::anyhow!("failed to record action function '{fn_name}': {e}"))?;

        let recorded = state.borrow();
        actions.push(ActionMeta {
            name: map_str(def, "name")?,
            emoji: map_str(def, "emoji")?,
            description: map_str(def, "description")?,
            cpu_cost: map_str(def, "cpu_cost")?,
            reads_block: map_bool(def, "reads_block")?,
            hidden: map_bool(def, "hidden")?,
            steps: recorded.steps.clone(),
        });
    }

    // Step 4: read optional imports
    let imports = scope
        .get_value::<rhai::Array>("imports")
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.into_string().unwrap_or_default().into())
        .collect();

    Ok(PluginMetadata {
        name,
        version,
        dependencies,
        classes,
        actions,
        imports,
    })
}

// ---------------------------------------------------------------------------
// Recording engine — register stub host functions
// ---------------------------------------------------------------------------

fn create_recording_engine(state: Rc<RefCell<RecorderState>>) -> Engine {
    let mut engine = create_engine();

    // --- Object lifecycle ---

    let s = Rc::clone(&state);
    engine.register_fn("output", move |name: ImmutableString, class: ImmutableString| -> i64 {
        s.borrow_mut().push_step(StepKindMeta::Output, name.into(), class.into(), String::new())
    });

    let s = Rc::clone(&state);
    engine.register_fn("input", move |name: ImmutableString, class: ImmutableString| -> i64 {
        s.borrow_mut().push_step(StepKindMeta::Input, name.into(), class.into(), String::new())
    });

    let s = Rc::clone(&state);
    engine.register_fn("mutate", move |name: ImmutableString, class: ImmutableString| -> i64 {
        s.borrow_mut().push_step(StepKindMeta::Mutate, name.into(), class.into(), String::new())
    });

    let s = Rc::clone(&state);
    engine.register_fn("depends", move |name: ImmutableString, action: ImmutableString| {
        s.borrow_mut().push_step(StepKindMeta::Depends, name.into(), String::new(), action.into());
    });

    // --- Field operations ---

    // set(handle, key, string_value)
    let s = Rc::clone(&state);
    engine.register_fn("set", move |handle: i64, key: ImmutableString, value: ImmutableString| {
        s.borrow_mut().add_detail(handle, DetailMeta::Set {
            key: key.into(),
            value: LiteralValue::Str(value.into()),
        });
    });

    // set_int(handle, key, int_value)
    let s = Rc::clone(&state);
    engine.register_fn("set_int", move |handle: i64, key: ImmutableString, value: i64| {
        s.borrow_mut().add_detail(handle, DetailMeta::Set {
            key: key.into(),
            value: LiteralValue::Int(value),
        });
    });

    // update(handle, key, var_name)
    let s = Rc::clone(&state);
    engine.register_fn("update", move |handle: i64, key: ImmutableString, source: ImmutableString| {
        s.borrow_mut().add_detail(handle, DetailMeta::Update {
            key: key.into(),
            source: source.into(),
        });
    });

    // obj_raw(handle) → returns the handle (passthrough in recording mode)
    engine.register_fn("obj_raw", |handle: i64| -> i64 { handle });

    // --- Intro pods ---

    // vdf(iters, handle_from_obj_raw) → returns var name "work"
    let s = Rc::clone(&state);
    engine.register_fn("vdf", move |iters: i64, handle: i64| -> ImmutableString {
        let mut st = s.borrow_mut();
        st.add_detail(handle, DetailMeta::Var {
            name: "work".into(),
            recipe: VarRecipe::Vdf { iters: iters as usize },
        });
        st.add_detail(handle, DetailMeta::Condition {
            pred: format!("Vdf({iters}, {{state}}, work)"),
            recipe: ConditionRecipe::StoredVdfPod,
        });
        "work".into()
    });

    // pow_grind(handle, difficulty) → returns var name "key"
    let s = Rc::clone(&state);
    engine.register_fn("pow_grind", move |handle: i64, difficulty: i64| -> ImmutableString {
        let mut st = s.borrow_mut();
        st.add_detail(handle, DetailMeta::Var {
            name: "key".into(),
            recipe: VarRecipe::PowGrind { difficulty: difficulty as u64 },
        });
        st.add_detail(handle, DetailMeta::Update {
            key: "key".into(),
            source: "key".into(),
        });
        "key".into()
    });

    // lt_eq_u256(handle, difficulty)
    let s = Rc::clone(&state);
    engine.register_fn("lt_eq_u256", move |handle: i64, difficulty: i64| {
        let diff_u64 = difficulty as u64;
        // Format as 256-bit big-endian: u64 occupies the most-significant 8 bytes,
        // followed by 48 zero hex chars for the remaining 24 bytes.
        let pred = format!(
            "LtEqU256({{state}}, Raw(0x{:016x}{}))",
            diff_u64,
            "0".repeat(48),
        );
        s.borrow_mut().add_detail(handle, DetailMeta::Condition {
            pred,
            recipe: ConditionRecipe::LtEqU256 { difficulty: diff_u64 },
        });
    });

    // --- Proof conditions ---

    // gt(handle, key, value)
    let s = Rc::clone(&state);
    engine.register_fn("gt", move |handle: i64, key: ImmutableString, value: i64| {
        let key_str: String = key.into();
        s.borrow_mut().add_detail(handle, DetailMeta::Condition {
            pred: format!("Gt({{state}}.{key_str}, {value})"),
            recipe: ConditionRecipe::Gt { key: key_str, value },
        });
    });

    // sum_of(handle, key, stored_var, b)
    let s = Rc::clone(&state);
    engine.register_fn("sum_of", move |handle: i64, key: ImmutableString, stored_var: ImmutableString, b: i64| {
        let key_str: String = key.into();
        let var_str: String = stored_var.into();
        s.borrow_mut().add_detail(handle, DetailMeta::Condition {
            pred: format!("SumOf({{state}}.{key_str}, {var_str}, {b})"),
            recipe: ConditionRecipe::SumOf { key: key_str, stored_var: var_str, b },
        });
    });

    // --- Utilities ---

    // decrement(handle, key) → returns var name (same as key)
    let s = Rc::clone(&state);
    engine.register_fn("decrement", move |handle: i64, key: ImmutableString| -> ImmutableString {
        let key_str: String = key.clone().into();
        s.borrow_mut().add_detail(handle, DetailMeta::Var {
            name: key_str.clone(),
            recipe: VarRecipe::DecrementField { key: key_str },
        });
        key
    });

    // random_key(handle) → returns var name "key"
    let s = Rc::clone(&state);
    engine.register_fn("random_key", move |handle: i64| -> ImmutableString {
        s.borrow_mut().add_detail(handle, DetailMeta::Var {
            name: "key".into(),
            recipe: VarRecipe::RandomKey,
        });
        "key".into()
    });

    engine
}

// ---------------------------------------------------------------------------
// Helpers — read typed values from Rhai scope / maps
// ---------------------------------------------------------------------------

fn get_map(scope: &Scope, name: &str) -> Result<rhai::Map> {
    scope
        .get_value::<rhai::Map>(name)
        .with_context(|| format!("missing global variable '{name}'"))
}

fn get_array(scope: &Scope, name: &str) -> Result<rhai::Array> {
    scope
        .get_value::<rhai::Array>(name)
        .with_context(|| format!("missing global variable '{name}'"))
}

fn map_str(map: &rhai::Map, key: &str) -> Result<String> {
    map.get(key)
        .and_then(|v| v.clone().into_string().ok())
        .with_context(|| format!("missing or non-string key '{key}'"))
}

fn map_bool(map: &rhai::Map, key: &str) -> Result<bool> {
    map.get(key)
        .and_then(|v| v.as_bool().ok())
        .with_context(|| format!("missing or non-bool key '{key}'"))
}

fn read_dependencies(scope: &Scope) -> Result<Vec<DependencyMeta>> {
    let arr = get_array(scope, "dependencies")?;
    arr.into_iter()
        .map(|item| {
            let map = item.cast::<rhai::Map>();
            let dep_type_str = map_str(&map, "dep_type")?;
            let dep_type = match dep_type_str.as_str() {
                "Intro" => DependencyType::Intro,
                "Module" => DependencyType::Module,
                other => bail!("unknown dependency type: {other}"),
            };
            Ok(DependencyMeta {
                dep_type,
                pred: map_str(&map, "pred")?,
                hash: map_str(&map, "hash")?,
            })
        })
        .collect()
}

fn read_classes(scope: &Scope) -> Result<Vec<ClassMeta>> {
    let arr = get_array(scope, "classes")?;
    arr.into_iter()
        .map(|item| {
            let map = item.cast::<rhai::Map>();
            Ok(ClassMeta {
                name: map_str(&map, "name")?,
                emoji: map_str(&map, "emoji")?,
                description: map_str(&map, "description")?,
            })
        })
        .collect()
}

fn read_action_defs(scope: &Scope) -> Result<Vec<rhai::Map>> {
    let arr = get_array(scope, "action_defs")?;
    arr.into_iter()
        .map(|item| Ok(item.cast::<rhai::Map>()))
        .collect()
}
