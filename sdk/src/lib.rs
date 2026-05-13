use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::slice;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use itertools::zip_eq;
use lt_eq_u256_pod::{LtEqU256Pod, STANDARD_LT_EQ_U256_VD_HASH};
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder, Operation, OperationArg},
    lang::{Module, load_module},
    middleware::{
        EMPTY_VALUE, F, Hash, MainPodProver, NativePredicate, OperationAux, OperationType, Params,
        Pod, Predicate, RawValue, Statement, StrKey, VDSet, Value,
        containers::{Array, Dictionary, Set},
    },
};
use pod2utils::{dict, macros::BuildContext, rand_raw_value};
use rhai::{AST, CallFnOptions, Dynamic, Engine, EvalAltResult, EvalContext, Expression, Scope};
use txlib::{EventHandle, GroundingWitness, Tx, TxBuilder};
use vdfpod::{STANDARD_VDF_VD_HASH, VdfPod};

mod error;
mod fmt_podlang;
pub mod manifest;
mod utils;

#[cfg(test)]
mod tests;

pub use error::SdkError;
use manifest::Manifest;
use utils::native_pred_to_op;

/// Shared reference with interior mutability for anything that could be used as an argument to a
/// statement template
type Ref = Rc<RefCell<VarOrValue>>;
/// Result that carries an error produced during the evaluation of a Rhai script
type RuntimeResult<T> = Result<T, Box<EvalAltResult>>;

#[derive(Debug)]
pub enum Dependency {
    Module { name: String, hash: Hash },
    Intro { pred: String, hash: Hash },
}

#[derive(Debug)]
enum Intro {
    Vdf,      // (n_iters, input, work)
    LtEqU256, // (lhs, rhs)
}

#[derive(Debug, Clone, Copy)]
pub enum ObjectIO {
    Input,
    Mutate,
    Output,
}

impl ObjectIO {
    /// True if the action consumes an object in this position (delete
    /// or mutate).
    pub fn consumes(&self) -> bool {
        matches!(self, Self::Input | Self::Mutate)
    }

    /// True if the action produces an object in this position (insert
    /// or mutate).
    pub fn produces(&self) -> bool {
        matches!(self, Self::Output | Self::Mutate)
    }
}

/// Name of a class's generated type-guard predicate.
fn class_predicate_name(class: &str) -> String {
    format!("Is{class}")
}

/// An instruction records the structural shape of one rhai-level
/// operation. Pure Load-time data, enough to render podlang and to
/// derive metadata. At Execute time, statement-producing operations
/// also capture concrete dict snapshots / cached statements on the
/// Inst itself, so the post-Rhai body walk can re-emit priv_ops
/// without depending on Ref mutation order.
enum Inst {
    Object {
        io: ObjectIO,
        obj: Ref,
        class: String,
        /// Pre-mutation dict for Mutate (Some), None for Input/Output.
        /// Populated at exe time inside `obj_io`; left None at Load.
        original: Option<Dictionary>,
    },
    Update {
        obj: String,
        key: String,
        value: Ref,
        /// Pre-update Object dict snapshot. Some at Execute, None at Load.
        old_dict: Option<Dictionary>,
        /// Post-update Object dict snapshot. Some at Execute, None at Load.
        new_dict: Option<Dictionary>,
    },
    Set {
        obj: String,
        kvs: Vec<(String, Ref)>,
        /// Post-set Object dict snapshot (after all kvs inserted).
        /// Some at Execute, None at Load.
        final_dict: Option<Dictionary>,
    },
    Statement {
        pred: NativePredicate,
        args: Vec<Ref>,
    },
    Intro {
        pred: Intro,
        args: Vec<Ref>,
        /// Pod's first pub statement, cached at Rhai time. Some at
        /// Execute, None at Load.
        statement: Option<Statement>,
    },
    /// Reference to another action executed as a sub-action. The
    /// sub-action's exe_action runs recursively during the parent's
    /// Rhai body and emits its own priv_ops; only the resulting
    /// statement is consumed here in the parent's post-Rhai walk.
    SubAction {
        action: String,
        /// Aliases the sub-action's first producing object Ref so
        /// parent scripts can bind it via `var foo = subaction(...)`.
        obj: Ref,
        /// Sub-action's predicate statement. Some at Execute, None at Load.
        st_sub: Option<Statement>,
    },
}

/// pod2 value type information.  Used for type checking of vars at Load time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Type {
    Unk,
    Raw,
    Int,
    Dict,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Corresponds to a variable or variable.key in a custom predicate.
/// In pod2 a variable is a wildcard.
#[derive(Debug, Clone)]
struct Var {
    name: String,
    typ: Type,
    /// Is None at declaration time, Some at execution time
    value: Option<Value>,
    /// If Some, then this `Var` is treated as an Entry
    key: Option<String>,
}

#[derive(Debug, Clone)]
enum VarOrValue {
    Var(Var),
    Value(Value),
}

impl VarOrValue {
    /// Constructor
    fn value(value: Value) -> Self {
        Self::Value(value)
    }
    /// Constructor
    fn var(typ: Type) -> Self {
        Self::Var(Var {
            name: "?".to_string(),
            typ,
            value: None,
            key: None,
        })
    }
    fn type_check(&self, typ: Type) -> RuntimeResult<()> {
        match self {
            Self::Value(v) => match typ {
                Type::Unk => Some(()),
                Type::Raw => Some(()),
                Type::Int => v.as_int().map(|_| ()),
                Type::Dict => v.as_dictionary().map(|_| ()),
            }
            .ok_or_else(|| format!("type check: expected {}", typ).into()),
            Self::Var(Var {
                typ: var_type,
                key: None,
                ..
            }) => {
                if typ == Type::Unk
                    || typ == Type::Raw
                    || *var_type == Type::Unk
                    || *var_type == typ
                {
                    Ok(())
                } else {
                    Err(format!("type check: expected {}, found {}", typ, var_type).into())
                }
            }
            Self::Var(Var {
                key: Some(_key), ..
            }) => Ok(()),
        }
    }
    fn var_name(&self) -> &str {
        self.as_var().name.as_str()
    }
    fn set_var_name(&mut self, name: String) -> RuntimeResult<()> {
        match self {
            Self::Value(_) => Err("var cannot be a literal value".into()),
            Self::Var(var) => {
                var.name = name;
                Ok(())
            }
        }
    }
    fn as_var(&self) -> &Var {
        match self {
            Self::Var(var) => var,
            Self::Value(_) => panic!("not a var"),
        }
    }
    fn as_mut_var(&mut self) -> &mut Var {
        match self {
            Self::Var(var) => var,
            Self::Value(_) => panic!("not a var"),
        }
    }
    // Only call this at exec time
    fn as_value(&self) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::Var(Var {
                value, key: None, ..
            }) => value.clone().expect("has value at exec time"),
            Self::Var(Var {
                value,
                key: Some(key),
                ..
            }) => {
                let dict = value
                    .as_ref()
                    .expect("has value at exec time")
                    .as_dictionary()
                    .expect("dict");
                dict.get(&StrKey::from(key)).unwrap().expect("key exists")
            }
        }
    }
    // Only call this at exec time
    fn as_mut_value(&mut self) -> &mut Value {
        match self {
            Self::Value(value) => value,
            Self::Var(Var {
                value, key: None, ..
            }) => value.as_mut().expect("has value at exec time"),
            Self::Var(Var {
                value: _,
                key: Some(_),
                ..
            }) => panic!("entry can't be mutated"),
        }
    }
    // Only call this at exec time
    fn as_op_arg(&self) -> OperationArg {
        match self {
            Self::Value(value) => OperationArg::Literal(value.clone()),
            Self::Var(Var { value, key, .. }) => {
                let value = value.as_ref().expect("has value at exec time").clone();
                if let Some(key) = key {
                    let dict = value.as_dictionary().expect("dict");
                    let value = dict.get(&key.into()).unwrap().unwrap();
                    OperationArg::Statement(Statement::Contains(
                        dict.into(),
                        key.clone().into(),
                        value.into(),
                    ))
                } else {
                    OperationArg::Literal(value)
                }
            }
        }
    }
    fn set_value(&mut self, value: Value) {
        match self {
            Self::Value(v) => *v = value,
            Self::Var(Var { value: v, .. }) => *v = Some(value),
        }
    }
    // Only call this at exec time
    fn to_dict(&self) -> Dictionary {
        self.as_value().as_dictionary().expect("is dict")
    }
    // Only call this at exec time
    fn mut_dict<T>(&mut self, mut f: impl FnMut(&mut Dictionary) -> T) -> T {
        let obj = self.as_mut_value();
        let mut dict = obj.as_dictionary().expect("is dict");
        let output = f(&mut dict);
        *obj = Value::from(dict);
        output
    }
}

fn type_check_args<const N: usize>(args_types: [(&ArgHandle, Type); N]) -> RuntimeResult<()> {
    for (arg, typ) in args_types {
        arg.arg.borrow().type_check(typ)?;
    }
    Ok(())
}

fn validate_args<const N: usize>(args_types: [(Dynamic, Type); N]) -> RuntimeResult<[Ref; N]> {
    let rs = args_types
        .into_iter()
        .map(|(arg, typ)| {
            let r = try_ref_from_dynamic(arg)?;
            r.borrow().type_check(typ)?;
            Ok(r)
        })
        .collect::<RuntimeResult<Vec<Ref>>>()?;
    Ok(rs.try_into().expect("len = N"))
}

/// Used to track how many updates from mutations a variable takes.
#[derive(Default, Debug)]
struct VarState {
    ts: usize,
}

/// This handler is accessible in the action script function to define action operations.  It
/// contains a shared ActionContext with interior mutability.
#[derive(Clone)]
struct ActionHandle(Rc<RefCell<ActionContext>>);

/// Holds the state of an action being defined. `insts` is the
/// structural shape (always populated). `exe_ctx` is shared by Rc so
/// parent and sub-action handles see the same builder / input queue /
/// tx_builder.
struct ActionContext {
    name: String,
    insts: Vec<Inst>,
    /// User-defined vars (objects, intro outputs, temporaries) plus
    /// the txlib chain var, registered as `"chain"` so it shares the
    /// same `ts` machinery.
    vars: Vec<String>,
    var_state: HashMap<String, VarState>,
    exe_ctx: Option<Rc<RefCell<ExeContext>>>,
    unsafe_block: bool,
}

impl ActionContext {
    fn new(name: String, exe_ctx: Option<Rc<RefCell<ExeContext>>>) -> Self {
        let mut ctx = Self {
            name,
            insts: Vec::new(),
            vars: Vec::new(),
            var_state: HashMap::new(),
            exe_ctx,
            unsafe_block: false,
        };
        ctx.add_var("chain".to_string())
            .expect("chain not yet defined");
        ctx
    }
    fn add_var(&mut self, var: String) -> RuntimeResult<()> {
        if self.var_state.contains_key(&var) {
            return Err(format!("var {var} already exists").into());
        }
        self.var_state.insert(var.clone(), VarState::default());
        self.vars.push(var);
        Ok(())
    }
    fn inc_t_var(&mut self, var: &str) -> RuntimeResult<()> {
        let state = self.var_state.get_mut(var).expect("var {var} exists");
        state.ts += 1;
        Ok(())
    }
    fn assert_unsafe(&self, unsafe_block: bool) -> RuntimeResult<()> {
        if self.unsafe_block != unsafe_block {
            if self.unsafe_block {
                return Err("unexpected unsafe block".into());
            } else {
                return Err("expected unsafe block".into());
            }
        }
        Ok(())
    }
    /// Borrow the shared `ExeContext` immutably. Returns None at Load
    /// time (no `exe_ctx` set).
    fn exe_ref(&self) -> Option<std::cell::Ref<'_, ExeContext>> {
        self.exe_ctx.as_ref().map(|rc| rc.borrow())
    }
}

impl ActionHandle {
    //
    // Internal methods
    //
    fn new(name: String, exe_ctx: Option<Rc<RefCell<ExeContext>>>) -> Self {
        Self(Rc::new(RefCell::new(ActionContext::new(name, exe_ctx))))
    }
    fn new_obj(exe_ctx: &ExeContext, class: &str) -> Dictionary {
        let type_hash = exe_ctx
            .module
            .class_hashes
            .get(class)
            .copied()
            .unwrap_or_else(|| panic!("no Is{class} predicate hash registered"));
        dict!({
            "type" => type_hash,
            "work" => EMPTY_VALUE,
            "key" => exe_ctx.rand_value()
        })
    }
    fn obj_io(self, io: ObjectIO, class: String) -> RuntimeResult<ArgHandle> {
        let arg = Rc::new(RefCell::new(VarOrValue::var(Type::Dict)));
        let mut ctx = self.0.borrow_mut();
        let mut original: Option<Dictionary> = None;
        if let Some(exe_rc) = ctx.exe_ctx.as_ref() {
            let mut exe_ctx = exe_rc.borrow_mut();
            match io {
                ObjectIO::Output => {
                    arg.borrow_mut()
                        .set_value(Value::from(Self::new_obj(&exe_ctx, &class)));
                }
                ObjectIO::Input => {
                    let obj = exe_ctx.inputs.pop().expect("exists");
                    arg.borrow_mut().set_value(Value::from(obj));
                }
                ObjectIO::Mutate => {
                    let obj = exe_ctx.inputs.pop().expect("exists");
                    arg.borrow_mut().set_value(Value::from(obj.clone()));
                    original = Some(obj);
                }
            }
        }
        ctx.insts.push(Inst::Object {
            io,
            obj: arg.clone(),
            class,
            original,
        });
        ctx.inc_t_var("chain").expect("chain exists");
        Ok(ArgHandle::new(self.clone(), arg))
    }
    fn set_unsafe(&self, unsafe_block: bool) -> RuntimeResult<()> {
        let mut ctx = self.0.borrow_mut();
        if unsafe_block && ctx.unsafe_block {
            Err("unsafe already set".into())
        } else {
            ctx.unsafe_block = unsafe_block;
            Ok(())
        }
    }
    /// Open an action scope, run the action's rhai body, emit its
    /// txlib events, build its predicate, attach guards, and close
    /// the scope. Sub-actions invoked from the rhai body recurse here
    /// and stack their scope on top of this one.
    fn exe_action(&self) -> RuntimeResult<Statement> {
        let (module, scope_id, action) = {
            let ctx = self.0.borrow();
            let exe_rc = ctx.exe_ctx.as_ref().expect("exe phase").clone();
            let mut exe_ctx = exe_rc.borrow_mut();
            let scope_id = exe_ctx.tx_builder.begin_action();
            (exe_ctx.module.clone(), scope_id, ctx.name.clone())
        };
        let mut scope = Scope::new();
        let options = CallFnOptions::new().with_tag(self.clone());
        let _result = module.engine.call_fn_with_options::<Dynamic>(
            options,
            &mut scope,
            &module.ast,
            &action,
            (self.clone(),),
        )?;

        // ---- Build in/out record arrays from the action's direct
        // Object insts. Each Object contributes one entry to its
        // dispatch side's array (Mutate contributes to both).
        let exe_rc = self.0.borrow().exe_ctx.clone().expect("exe phase");
        let mut in_dicts: Vec<Value> = Vec::new();
        let mut out_dicts: Vec<Value> = Vec::new();
        let mut in_entry_idx: HashMap<String, usize> = HashMap::new();
        let mut out_entry_idx: HashMap<String, usize> = HashMap::new();
        {
            let ctx = self.0.borrow();
            for inst in &ctx.insts {
                if let Inst::Object {
                    io, obj, original, ..
                } = inst
                {
                    let varname = obj.borrow().var_name().to_string();
                    let post_dict = obj.borrow().to_dict();
                    match io {
                        ObjectIO::Input => {
                            in_entry_idx.insert(varname, in_dicts.len());
                            in_dicts.push(Value::from(post_dict));
                        }
                        ObjectIO::Output => {
                            out_entry_idx.insert(varname, out_dicts.len());
                            out_dicts.push(Value::from(post_dict));
                        }
                        ObjectIO::Mutate => {
                            let pre_dict = original
                                .as_ref()
                                .expect("Mutate records a pre-mutation dict")
                                .clone();
                            in_entry_idx.insert(varname.clone(), in_dicts.len());
                            in_dicts.push(Value::from(pre_dict));
                            out_entry_idx.insert(varname, out_dicts.len());
                            out_dicts.push(Value::from(post_dict));
                        }
                    }
                }
            }
        }
        let in_array = Array::new(in_dicts);
        let out_array = Array::new(out_dicts);

        // Look up the action's per-side wildcard decisions. Collapsed
        // sides drop their `ArrayContains` priv_op and the matching
        // template sub-statement; body / event priv_ops then take
        // anchored op-args so pod2 matches them to anchored template
        // positions.
        let (needs_in_wildcard, needs_out_wildcard) = {
            let ctx = self.0.borrow();
            let exe_ctx = ctx.exe_ctx.as_ref().expect("exe phase").borrow();
            let meta = exe_ctx.module.action_by_name(&action);
            (
                meta.needs_in_wildcard.clone(),
                meta.needs_out_wildcard.clone(),
            )
        };

        // Per-Object io map and max_ts. Drives op-arg anchoring at
        // pre/post-form ts below.
        let object_io: HashMap<String, ObjectIO> = {
            let ctx = self.0.borrow();
            ctx.insts
                .iter()
                .filter_map(|inst| match inst {
                    Inst::Object { io, obj, .. } => {
                        Some((obj.borrow().var_name().to_string(), *io))
                    }
                    _ => None,
                })
                .collect()
        };
        let max_ts: HashMap<String, usize> = {
            let ctx = self.0.borrow();
            ctx.var_state
                .iter()
                .map(|(k, v)| (k.clone(), v.ts))
                .collect()
        };

        // Returns an anchored op-arg when the Object's side at this ts
        // is collapsed; else a literal op-arg for the dict.
        let anchor_or_literal = |obj_name: &str, dict: &Dictionary, ts: usize| -> OperationArg {
            let Some(io) = object_io.get(obj_name).copied() else {
                return OperationArg::Literal(Value::from(dict.clone()));
            };
            let mts = *max_ts.get(obj_name).unwrap_or(&0);
            let at_in = matches!(io, ObjectIO::Input | ObjectIO::Mutate) && ts == 0;
            let at_out = matches!(io, ObjectIO::Output | ObjectIO::Mutate) && ts == mts;
            if at_in && !needs_in_wildcard.contains(obj_name) {
                return (&in_array, in_entry_idx[obj_name] as i64).into();
            }
            if at_out && !needs_out_wildcard.contains(obj_name) {
                return (&out_array, out_entry_idx[obj_name] as i64).into();
            }
            OperationArg::Literal(Value::from(dict.clone()))
        };

        // ---- Emit ArrayContains clauses for each Object's pre/post-
        // form on sides that need a wildcard.
        let mut array_contains_sts: Vec<Statement> = Vec::new();
        {
            let mut exe_ctx = exe_rc.borrow_mut();
            let exe_ctx = &mut *exe_ctx;
            let ctx = self.0.borrow();
            for inst in &ctx.insts {
                if let Inst::Object {
                    io, obj, original, ..
                } = inst
                {
                    let varname = obj.borrow().var_name().to_string();
                    let post_dict = obj.borrow().to_dict();
                    let emit_in = matches!(io, ObjectIO::Input | ObjectIO::Mutate)
                        && needs_in_wildcard.contains(&varname);
                    let emit_out = matches!(io, ObjectIO::Output | ObjectIO::Mutate)
                        && needs_out_wildcard.contains(&varname);
                    let pre_dict = match io {
                        ObjectIO::Mutate => Some(
                            original
                                .as_ref()
                                .expect("Mutate records a pre-mutation dict")
                                .clone(),
                        ),
                        ObjectIO::Input => Some(post_dict.clone()),
                        ObjectIO::Output => None,
                    };
                    if emit_in {
                        let d = pre_dict.clone().expect("in-side dict");
                        let st = exe_ctx
                            .bld
                            .builder
                            .priv_op(Operation::array_contains(
                                Value::from(in_array.clone()),
                                in_entry_idx[&varname] as i64,
                                Value::from(d),
                            ))
                            .unwrap();
                        array_contains_sts.push(st);
                    }
                    if emit_out {
                        let st = exe_ctx
                            .bld
                            .builder
                            .priv_op(Operation::array_contains(
                                Value::from(out_array.clone()),
                                out_entry_idx[&varname] as i64,
                                Value::from(post_dict),
                            ))
                            .unwrap();
                        array_contains_sts.push(st);
                    }
                }
            }
        }

        // ---- Per-Object type guard + Tx event. Same order as
        // `fmt_action`'s second loop. Tx events come from `tx_builder`
        // with literal dict args; we lift them to anchored form via
        // `ReplaceValueWithEntry` when their side is collapsed.
        struct EventData {
            handle: EventHandle,
            class: String,
            object_refs_index: usize,
            obj_dict: Dictionary,
        }
        let mut events: Vec<EventData> = Vec::new();
        let mut direct_outputs: Vec<usize> = Vec::new();
        let mut event_sts: Vec<Statement> = Vec::new();
        let mut obj_refs_index: usize = 0;
        {
            let mut exe_ctx = exe_rc.borrow_mut();
            let exe_ctx = &mut *exe_ctx;
            let ctx = self.0.borrow();
            for inst in &ctx.insts {
                if let Inst::Object {
                    io,
                    obj,
                    class,
                    original,
                } = inst
                {
                    let varname = obj.borrow().var_name().to_string();
                    let obj_dict = obj.borrow().to_dict();
                    // Type guard: Mutate guards the pre-mutation dict
                    // (ts=0); Input/Output guard the post (final) form.
                    let (guard_dict, guard_ts) = match io {
                        ObjectIO::Mutate => {
                            let d = original
                                .as_ref()
                                .expect("Mutate records a pre-mutation dict")
                                .clone();
                            (d, 0usize)
                        }
                        ObjectIO::Input => (obj_dict.clone(), 0usize),
                        ObjectIO::Output => (obj_dict.clone(), *max_ts.get(&varname).unwrap_or(&0)),
                    };
                    let class_hash = exe_ctx
                        .module
                        .class_hashes
                        .get(class.as_str())
                        .copied()
                        .unwrap_or_else(|| panic!("no Is{class} predicate hash registered"));
                    let guard_arg = anchor_or_literal(&varname, &guard_dict, guard_ts);
                    let st_type = exe_ctx
                        .bld
                        .builder
                        .priv_op(Operation::dict_contains(
                            guard_arg,
                            "type",
                            Value::from(class_hash),
                        ))
                        .unwrap();
                    event_sts.push(st_type);
                    let (st_tx_literal, handle) = match io {
                        ObjectIO::Output => exe_ctx.tx_builder.insert(&mut exe_ctx.bld, &obj_dict),
                        ObjectIO::Input => exe_ctx.tx_builder.delete(&mut exe_ctx.bld, &obj_dict),
                        ObjectIO::Mutate => {
                            let obj0 = original
                                .as_ref()
                                .expect("Mutate records a pre-mutation dict");
                            exe_ctx.tx_builder.mutate(&mut exe_ctx.bld, &obj_dict, obj0)
                        }
                    };
                    // Lift tx event args to anchored form when their
                    // side is collapsed. Arg layout (per txlib):
                    //   TxInsert(chain, prev_chain, new)
                    //   TxDelete(chain, prev_chain, old)
                    //   TxMutate(chain, prev_chain, new, old)
                    let new_anchor = || -> Option<OperationArg> {
                        if matches!(io, ObjectIO::Output | ObjectIO::Mutate)
                            && !needs_out_wildcard.contains(&varname)
                        {
                            Some((&out_array, out_entry_idx[&varname] as i64).into())
                        } else {
                            None
                        }
                    };
                    let old_anchor = || -> Option<OperationArg> {
                        if matches!(io, ObjectIO::Input | ObjectIO::Mutate)
                            && !needs_in_wildcard.contains(&varname)
                        {
                            Some((&in_array, in_entry_idx[&varname] as i64).into())
                        } else {
                            None
                        }
                    };
                    let replacements: Vec<Option<OperationArg>> = match io {
                        ObjectIO::Output => vec![None, None, new_anchor()],
                        ObjectIO::Input => vec![None, None, old_anchor()],
                        ObjectIO::Mutate => {
                            vec![None, None, new_anchor(), old_anchor()]
                        }
                    };
                    let st_tx = if replacements.iter().any(|r| r.is_some()) {
                        exe_ctx
                            .bld
                            .builder
                            .priv_op(Operation::replace_value_with_entry(
                                replacements,
                                st_tx_literal,
                            ))
                            .unwrap()
                    } else {
                        st_tx_literal
                    };
                    if io.produces() {
                        direct_outputs.push(exe_ctx.outputs.len());
                        exe_ctx.outputs.push(PerOutput {
                            class: class.clone(),
                            obj: obj_dict.clone(),
                            action_name: action.clone(),
                            object_refs_index: obj_refs_index,
                            action_st: Statement::None,
                            out_array: out_array.clone(),
                            out_entry_idx: out_entry_idx[&varname],
                        });
                    }
                    events.push(EventData {
                        handle,
                        class: class.clone(),
                        object_refs_index: obj_refs_index,
                        obj_dict,
                    });
                    obj_refs_index += 1;
                    event_sts.push(st_tx);
                }
            }
        }

        // ---- Walk body Insts post-Rhai and emit priv_ops. Dict args
        // at the pre/post-form ts of an Object on a collapsed side are
        // anchored via `anchor_or_literal`; everything else is literal.
        let mut body_sts: Vec<Statement> = Vec::new();
        let mut current_ts: HashMap<String, usize> =
            object_io.keys().map(|n| (n.clone(), 0usize)).collect();
        {
            let mut exe_ctx = exe_rc.borrow_mut();
            let exe_ctx = &mut *exe_ctx;
            let ctx = self.0.borrow();
            for inst in &ctx.insts {
                match inst {
                    Inst::Object { .. } => {}
                    Inst::Statement { pred, args } => {
                        let op = native_pred_to_op(*pred);
                        let op_type = OperationType::Native(op);
                        let op_args = args.iter().map(|v| v.borrow().as_op_arg()).collect();
                        let st = exe_ctx
                            .bld
                            .builder
                            .priv_op(Operation(op_type, op_args, OperationAux::None))
                            .unwrap();
                        body_sts.push(st);
                    }
                    Inst::Intro { statement, .. } => {
                        // pod2's `Statement::Intro` only accepts literal
                        // args, so `compute_wildcard_needs` forces a
                        // wildcard on any side whose Object appears
                        // here at its pre/post-form ts. The cached
                        // literal Statement then matches directly.
                        body_sts.push(statement.clone().expect("Intro statement captured at Rhai"));
                    }
                    Inst::SubAction { st_sub, .. } => {
                        body_sts.push(
                            st_sub
                                .clone()
                                .expect("SubAction statement captured at Rhai"),
                        );
                    }
                    Inst::Set {
                        obj,
                        kvs,
                        final_dict,
                    } => {
                        let dict = final_dict.clone().expect("Set final_dict captured at Rhai");
                        let ts = *current_ts.get(obj).unwrap_or(&0);
                        let dict_arg = anchor_or_literal(obj, &dict, ts);
                        for (key, value) in kvs {
                            let v = value.borrow().as_value().clone();
                            let st = exe_ctx
                                .bld
                                .builder
                                .priv_op(Operation::dict_contains(dict_arg.clone(), key.clone(), v))
                                .unwrap();
                            body_sts.push(st);
                        }
                    }
                    Inst::Update {
                        obj,
                        key,
                        value,
                        old_dict,
                        new_dict,
                    } => {
                        let old = old_dict.clone().expect("Update old_dict captured at Rhai");
                        let new = new_dict.clone().expect("Update new_dict captured at Rhai");
                        let v = value.borrow().as_value().clone();
                        let ts_before = *current_ts.get(obj).unwrap_or(&0);
                        let ts_after = ts_before + 1;
                        let new_arg = anchor_or_literal(obj, &new, ts_after);
                        let old_arg = anchor_or_literal(obj, &old, ts_before);
                        let st = exe_ctx
                            .bld
                            .builder
                            .priv_op(Operation::dict_update(new_arg, old_arg, key.clone(), v))
                            .unwrap();
                        body_sts.push(st);
                        if let Some(t) = current_ts.get_mut(obj) {
                            *t = ts_after;
                        }
                    }
                }
            }
        }

        // ---- Compose the action predicate's sub-statements, matching
        // fmt_action's emission order:
        //   per-Object ArrayContains clauses
        //   body (Inst::Update / Statement / Intro / SubAction / Set)
        //   per-Object {type guard, tx event} pairs
        let mut sts = array_contains_sts;
        sts.extend(body_sts);
        sts.extend(event_sts);

        let st_action = {
            let mut exe_ctx = exe_rc.borrow_mut();
            exe_ctx
                .bld
                .apply_custom_pred_simple(false, &action, sts)
                .unwrap()
        };

        // ---- Backfill action_st on direct outputs + attach IsX
        // guard to each event via the bridge predicate.
        {
            let mut exe_ctx = exe_rc.borrow_mut();
            let exe_ctx = &mut *exe_ctx;
            for idx in &direct_outputs {
                exe_ctx.outputs[*idx].action_st = st_action.clone();
            }
            let module = exe_ctx.module.clone();
            for EventData {
                handle,
                class,
                object_refs_index,
                obj_dict,
            } in events
            {
                // For inserts/deletes, post == pre == obj_dict, so it
                // doesn't matter which form we hand the bridge.
                let action_meta = module.action_by_name(&action);
                let obj_ref = &action_meta.object_refs[object_refs_index];
                let varname = &obj_ref.varname;
                let (bridge_array, entry_idx) = match fmt_podlang::dispatch_side(&obj_ref.io) {
                    fmt_podlang::Side::In => (in_array.clone(), in_entry_idx[varname]),
                    fmt_podlang::Side::Out => (out_array.clone(), out_entry_idx[varname]),
                };
                let st_is_x = module.build_is_x(
                    &mut exe_ctx.bld,
                    &action,
                    &class,
                    object_refs_index,
                    st_action.clone(),
                    &bridge_array,
                    entry_idx,
                    &obj_dict,
                );
                exe_ctx.tx_builder.set_guard(handle, st_is_x);
            }
            exe_ctx.tx_builder.end_action(scope_id);
        }
        Ok(st_action)
    }
    //
    // Exposed methods helpers
    //
    fn native_st(self, pred: NativePredicate, args: Vec<Ref>) -> RuntimeResult<()> {
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        ctx.insts.push(Inst::Statement { pred, args });
        Ok(())
    }
    //
    // Exposed methods
    //
    fn output(self, class: String) -> RuntimeResult<ArgHandle> {
        self.obj_io(ObjectIO::Output, class)
    }
    fn input(self, class: String) -> RuntimeResult<ArgHandle> {
        self.obj_io(ObjectIO::Input, class)
    }
    fn mutate(self, class: String) -> RuntimeResult<ArgHandle> {
        self.obj_io(ObjectIO::Mutate, class)
    }
    /// Reference another action as a sub-action. At Execute time we
    /// open a nested action scope, run the sub-action's rhai body
    /// (sharing the parent's `ExeContext`), emit its events, build
    /// its predicate, attach guards, and close the scope, all live,
    /// before this call returns. The sub-action's predicate statement
    /// is cached on the `Inst::SubAction` and composed into the
    /// parent's predicate during its post-Rhai body walk. The returned
    /// `ArgHandle` aliases the sub's first producing object so parent
    /// scripts can bind it via `var foo = subaction("X")`.
    fn subaction(self, action: String) -> RuntimeResult<ArgHandle> {
        let exe_rc_opt = self.0.borrow().exe_ctx.clone();
        let arg_placeholder = Rc::new(RefCell::new(VarOrValue::var(Type::Dict)));

        let (arg, st_sub) = if let Some(exe_rc) = exe_rc_opt {
            let sub_handle = ActionHandle::new(action.clone(), Some(exe_rc.clone()));
            let st_sub = sub_handle.exe_action()?;

            {
                let mut exe_ctx = exe_rc.borrow_mut();
                // Reveal so per-output IsX pods can reference the
                // sub-action's Action statement via the internal pod.
                exe_ctx.bld.builder.reveal(&st_sub).unwrap();
            }

            // Alias the parent's binding to the sub-action's first
            // producing object Ref, or a fresh placeholder if the sub
            // produces nothing.
            let arg = sub_handle
                .0
                .borrow()
                .insts
                .iter()
                .find_map(|i| match i {
                    Inst::Object {
                        io: ObjectIO::Output | ObjectIO::Mutate,
                        obj,
                        ..
                    } => Some(obj.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| arg_placeholder.clone());
            (arg, Some(st_sub))
        } else {
            (arg_placeholder, None)
        };

        let mut ctx = self.0.borrow_mut();
        ctx.insts.push(Inst::SubAction {
            action,
            obj: arg.clone(),
            st_sub,
        });
        ctx.inc_t_var("chain").expect("chain exists");
        Ok(ArgHandle::new(self.clone(), arg))
    }
    fn random(self) -> RuntimeResult<ArgHandle> {
        let value = Rc::new(RefCell::new(VarOrValue::var(Type::Raw)));
        if let Some(exe_ctx) = self.0.borrow().exe_ref() {
            value.borrow_mut().set_value(exe_ctx.rand_value());
        }
        Ok(ArgHandle::new(self.clone(), value))
    }
    fn pow_obj_grind(self, obj: Dynamic, target: Dynamic) -> RuntimeResult<ArgHandle> {
        // Target is a full u256 (Raw). To build one with a desired top-limb
        // difficulty, scripts use `action.top_limb_u256(n)`.
        let [obj, target] = validate_args([(obj, Type::Dict), (target, Type::Raw)])?;
        // For now we assume that obj is var, and thus return a key that is also var
        let key = Rc::new(RefCell::new(VarOrValue::var(Type::Raw)));
        if let Some(exe_ctx) = self.0.borrow().exe_ref() {
            // This is a copy of the object, we don't modify the obj argument.
            let mut obj = obj.borrow().to_dict();
            let target_raw = target.borrow().as_value().raw();
            let mut k = exe_ctx.rand_value();
            if !exe_ctx.mock {
                while u256_gt(&RawValue::from(obj.commitment()), &target_raw) {
                    k = exe_ctx.rand_value();
                    obj.update(&StrKey::from("key"), &k).unwrap();
                }
            }
            key.borrow_mut().set_value(k);
        }
        Ok(ArgHandle::new(self.clone(), key))
    }
    /// Build a u256 with `n` in the most-significant limb and zeros elsewhere.
    /// Useful as a difficulty target for [`pow_obj_grind`] and [`intro_lt_eq_u256`]:
    /// a u256 `x` satisfies `x <= top_limb_u256(n)` iff the top limb of `x` is
    /// `<= n` (with all lower limbs of `x` implicitly bounded by the zeros).
    fn top_limb_u256(self, n: Dynamic) -> RuntimeResult<ArgHandle> {
        let [n] = validate_args([(n, Type::Int)])?;
        let n_int = match &*n.borrow() {
            VarOrValue::Value(v) => v
                .as_int()
                .ok_or::<Box<EvalAltResult>>("top_limb_u256: expected int".into())?,
            VarOrValue::Var(_) => {
                return Err("top_limb_u256: n must be a literal integer".into());
            }
        };
        let raw = RawValue([F(0), F(0), F(0), F(n_int as u64)]);
        Ok(ArgHandle::literal(self.clone(), Value::from(raw)))
    }
    fn st_gt(self, v0: Dynamic, v1: Dynamic) -> RuntimeResult<()> {
        let [v0, v1] = validate_args([(v0, Type::Int), (v1, Type::Int)])?;
        self.native_st(NativePredicate::Gt, vec![v0, v1])
    }
    fn st_sum_of(self, v0: Dynamic, v1: Dynamic, v2: Dynamic) -> RuntimeResult<()> {
        let [v0, v1, v2] = validate_args([(v0, Type::Int), (v1, Type::Int), (v2, Type::Int)])?;
        self.native_st(NativePredicate::SumOf, vec![v0, v1, v2])
    }
    fn intro_vdf(self, n_iters: Dynamic, input: Dynamic) -> RuntimeResult<ArgHandle> {
        let [n_iters, input] = validate_args([(n_iters, Type::Int), (input, Type::Raw)])?;

        let work = Rc::new(RefCell::new(VarOrValue::var(Type::Raw)));
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        let mut statement: Option<Statement> = None;
        if let Some(exe_rc) = ctx.exe_ctx.as_ref() {
            let mut exe_ctx = exe_rc.borrow_mut();
            let n = n_iters.borrow().as_value().as_int().expect("int") as usize;
            let inp = input.borrow().as_value().raw();
            let pod = if exe_ctx.mock {
                VdfPod::new_boxed_mock(&exe_ctx.params, exe_ctx.vd_set.clone(), n, inp)
            } else {
                VdfPod::new_boxed(&exe_ctx.params, exe_ctx.vd_set.clone(), n, inp)
            }
            .unwrap();
            let st = add_intro_pod(&mut exe_ctx, pod);
            work.borrow_mut().set_value(st.args()[2].literal().unwrap());
            statement = Some(st);
        }
        ctx.insts.push(Inst::Intro {
            pred: Intro::Vdf,
            args: vec![n_iters, input, work.clone()],
            statement,
        });
        Ok(ArgHandle::new(self.clone(), work))
    }
    fn intro_lt_eq_u256(self, lhs: Dynamic, rhs: Dynamic) -> RuntimeResult<()> {
        let [lhs, rhs] = validate_args([(lhs, Type::Raw), (rhs, Type::Raw)])?;
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        let mut statement: Option<Statement> = None;
        if let Some(exe_rc) = ctx.exe_ctx.as_ref() {
            let mut exe_ctx = exe_rc.borrow_mut();
            let l = lhs.borrow().as_value().raw();
            let r = rhs.borrow().as_value().raw();
            let pod = if exe_ctx.mock {
                LtEqU256Pod::new_boxed_mock(&exe_ctx.params, exe_ctx.vd_set.clone(), l, r)
            } else {
                LtEqU256Pod::new_boxed(&exe_ctx.params, exe_ctx.vd_set.clone(), l, r)
            }
            .unwrap();
            let st = add_intro_pod(&mut exe_ctx, pod);
            statement = Some(st);
        }
        ctx.insts.push(Inst::Intro {
            pred: Intro::LtEqU256,
            args: vec![lhs, rhs],
            statement,
        });
        Ok(())
    }
}

/// Attach an intro pod (VDF, LtEqU256, etc.) to the builder and
/// return its first public statement.
fn add_intro_pod(exe_ctx: &mut ExeContext, pod: Box<dyn Pod>) -> Statement {
    let st = pod.pub_statements()[0].clone();
    let main = exe_ctx.main_pod(pod);
    exe_ctx.bld.builder.add_pod(main).unwrap();
    st
}

/// Lexicographic (little-endian limb order) u256 `>` comparison on `RawValue`.
/// `RawValue::0[0]` is the least-significant limb; `0[3]` is the most-significant.
fn u256_gt(a: &RawValue, b: &RawValue) -> bool {
    for i in (0..4).rev() {
        let la = a.0[i].0;
        let lb = b.0[i].0;
        if la != lb {
            return la > lb;
        }
    }
    false
}

/// Helper function to type check and cast an array of pairs of String and VarOrValue, to be used
/// as key values.
fn dynamic_to_kvs(kvs: Dynamic) -> RuntimeResult<Vec<(String, Ref)>> {
    let kvs = kvs.try_cast::<Vec<Dynamic>>().ok_or("kvs not array")?;
    let kvs = kvs
        .into_iter()
        .map(|kv| {
            kv.try_cast::<Vec<Dynamic>>()
                .ok_or_else(|| "kv not array".into())
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    kvs.into_iter()
        .map(|kv| {
            let [k, v] = kv.try_into().map_err(|_| "kv.len != 2")?;
            let k = k.try_cast::<String>().ok_or("k not string")?;
            let v = try_ref_from_dynamic(v)?;
            Ok((k, v))
        })
        .collect()
}

/// This handle is returned by host functions that return a var that can be used to define further
/// constraints.
#[derive(Clone)]
struct ArgHandle {
    ctx: ActionHandle,
    arg: Ref,
}

impl ArgHandle {
    /// Constructor
    fn new(ctx: ActionHandle, arg: Ref) -> Self {
        Self { ctx, arg }
    }
    /// Constructor
    fn literal(ctx: ActionHandle, value: Value) -> Self {
        let arg = Rc::new(RefCell::new(VarOrValue::value(value)));
        Self::new(ctx, arg)
    }
    fn set(self, kvs: Dynamic) -> RuntimeResult<()> {
        type_check_args([(&self, Type::Dict)])?;
        let kvs = dynamic_to_kvs(kvs)?;
        let mut arg = self.arg.borrow_mut();
        if let VarOrValue::Var(var) = &*arg {
            let var_name = var.name.clone();
            let mut ctx = self.ctx.0.borrow_mut();
            ctx.assert_unsafe(false)?;
            let mut final_dict: Option<Dictionary> = None;
            if ctx.exe_ctx.is_some() {
                for (key, value) in &kvs {
                    let value = value.borrow().as_value().clone();
                    arg.mut_dict(|obj| {
                        obj.insert(&StrKey::from(key), &value).expect("TODO");
                    });
                }
                final_dict = Some(arg.to_dict());
            }
            ctx.insts.push(Inst::Set {
                obj: var_name,
                kvs,
                final_dict,
            });
        }
        Ok(())
    }
    fn get(self, _key: String) -> RuntimeResult<ArgHandle> {
        todo!();
        // type_check_args([(&self, Type::Dict)])?;
        // // For now we assume that obj is var, and thus return a value that is also var
        // let value = Rc::new(RefCell::new(VarOrValue::var(Type::Unk)));
        // let ctx = self.ctx.0.borrow();
        // if ctx.exe_ctx.is_some() {
        //     let obj = self.arg.borrow().as_value().as_dictionary().expect("dict");
        //     let v = obj.get(&StrKey::from(key)).expect("TODO").expect("TODO");
        //     value.borrow_mut().set_value(v);
        // }
        // Ok(ArgHandle::new(self.ctx.clone(), value))
    }
    fn update(self, key: String, value: Dynamic) -> RuntimeResult<()> {
        type_check_args([(&self, Type::Dict)])?;
        let mut arg = self.arg.borrow_mut();
        if let VarOrValue::Var(var) = &*arg {
            let var_name = var.name.clone();
            let value = try_ref_from_dynamic(value)?;
            let mut ctx = self.ctx.0.borrow_mut();
            ctx.assert_unsafe(false)?;
            let mut old_dict: Option<Dictionary> = None;
            let mut new_dict: Option<Dictionary> = None;
            if ctx.exe_ctx.is_some() {
                let v = value.borrow().as_value().clone();
                let (obj0, obj) = arg.mut_dict(|obj| {
                    let obj0 = obj.clone();
                    obj.update(&StrKey::from(&key), &v).expect("TODO");
                    (obj0, obj.clone())
                });
                old_dict = Some(obj0);
                new_dict = Some(obj);
            }
            ctx.inc_t_var(var_name.as_str())?;
            ctx.insts.push(Inst::Update {
                obj: var_name,
                key,
                value,
                old_dict,
                new_dict,
            });
        }
        Ok(())
    }
    fn entry(&mut self, index: String) -> RuntimeResult<ArgHandle> {
        let mut arg = self.arg.borrow().clone();
        let var = arg.as_mut_var();
        var.key = Some(index);
        let arg = Rc::new(RefCell::new(arg));
        Ok(ArgHandle::new(self.ctx.clone(), arg))
    }
}

/// operator- for maybe-var types
fn arg_sub(a: ArgHandle, b: ArgHandle) -> RuntimeResult<ArgHandle> {
    type_check_args([(&a, Type::Int), (&b, Type::Int)])?;
    // TODO: Handle the case where a and b are not var
    let value = Rc::new(RefCell::new(VarOrValue::var(Type::Int)));
    let ctx = a.ctx.0.borrow();
    ctx.assert_unsafe(true)?;
    if ctx.exe_ctx.is_some() {
        let a = a.arg.borrow().as_value().as_int().expect("int");
        let b = b.arg.borrow().as_value().as_int().expect("int");
        let result = a.checked_sub(b).expect("no overflow");
        value.borrow_mut().set_value(Value::from(result));
    }
    Ok(ArgHandle::new(a.ctx.clone(), value))
}

/// Try to get the pod2 Value or promote a native type to it.
fn _try_value_from_dynamic(v: Dynamic) -> Result<Value, Dynamic> {
    let v = match v.try_cast_result::<Value>() {
        Ok(v) => return Ok(v),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<String>() {
        Ok(v) => return Ok(Value::from(v)),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<i64>() {
        Ok(v) => return Ok(Value::from(v)),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<RawValue>() {
        Ok(v) => return Ok(Value::from(v)),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<Dictionary>() {
        Ok(v) => return Ok(Value::from(v)),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<Set>() {
        Ok(v) => return Ok(Value::from(v)),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<Array>() {
        Ok(v) => return Ok(Value::from(v)),
        Err(v) => v,
    };
    Err(v)
}

/// Try to get a Ref or promote a native pod2 Value-compatible type to it.
fn try_ref_from_dynamic(v: Dynamic) -> RuntimeResult<Ref> {
    let v = match _try_value_from_dynamic(v) {
        Ok(v) => return Ok(Rc::new(RefCell::new(VarOrValue::value(v)))),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<Ref>() {
        Ok(v) => return Ok(v),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<ArgHandle>() {
        Ok(v) => return Ok(v.arg),
        Err(v) => v,
    };
    _ = v;
    Err(format!("invalid Ref type: {}", v.type_name()).into())
}

/// Get the Value from a type that encapsulates VarOrValue, or promote a native pod2
/// Value-compatible type to it.
/// Only call this at exec time
fn try_value_from_dynamic(v: Dynamic) -> RuntimeResult<Value> {
    let v = match _try_value_from_dynamic(v) {
        Ok(v) => return Ok(v),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<Ref>() {
        Ok(v) => return Ok(v.borrow().as_value().clone()),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<ArgHandle>() {
        Ok(v) => return Ok(v.arg.borrow().as_value().clone()),
        Err(v) => v,
    };
    _ = v;
    Err(format!("invalid value type: {}", v.type_name()).into())
}

/// One object reference in an action, in declaration order. Only
/// `class` is exposed; `io` is internal, used by the
/// `local_inputs()` / `local_outputs()` filters. `varname` is the
/// script-side variable name; it doubles as the entry name in the
/// records-form `<Action>In` / `<Action>Out` schemas.
#[derive(Debug, Clone)]
pub struct ActionObjectRef {
    io: ObjectIO,
    pub class: String,
    pub(crate) varname: String,
}

/// Collected metadata that declares an Action.
///
/// `object_refs` lists the action's direct Object instructions in
/// declaration order, matching the action predicate's public-arg
/// ordering. `total_inputs` / `total_outputs` flatten this action's
/// plus all transitively-called sub-actions' inputs/outputs.
/// `total_inputs` is used by `Executor::action` to zip against
/// caller-supplied inputs, and both are surfaced via the
/// `total_inputs()` / `total_outputs()` helpers for driver/GUI
/// signature reporting.
#[derive(Debug, Default)]
pub struct ActionMeta {
    pub name: String,
    object_refs: Vec<ActionObjectRef>,
    total_inputs: Vec<ActionObjectRef>,
    total_outputs: Vec<ActionObjectRef>,
    /// Object var names whose entry on the `in` record needs a
    /// wildcard (i.e. the body reads a sub-field off the pre-form, or
    /// the pre-form appears as a whole-dict arg to an Intro statement).
    /// Other Objects' `in` entry collapses to an `in.<entry>` anchored
    /// ref. Computed at Load by `compute_wildcard_needs`.
    pub(crate) needs_in_wildcard: HashSet<String>,
    /// Object var names whose entry on the `out` record needs a
    /// wildcard. See `needs_in_wildcard`.
    pub(crate) needs_out_wildcard: HashSet<String>,
}

impl ActionMeta {
    /// Object refs consumed locally by this action (Inputs + Mutates),
    /// excluding any transitively-called sub-actions.
    pub fn local_inputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.object_refs.iter().filter(|r| r.io.consumes())
    }

    /// Object refs produced locally by this action (Outputs + Mutates),
    /// excluding any transitively-called sub-actions.
    pub fn local_outputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.object_refs.iter().filter(|r| r.io.produces())
    }

    /// Object refs consumed by this action plus any transitively-called
    /// sub-actions, in declaration order. Used for tx-input zipping and
    /// for action-signature reporting.
    pub fn total_inputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.total_inputs.iter()
    }

    /// Object refs produced by this action plus any transitively-called
    /// sub-actions, in declaration order. Used for action-signature
    /// reporting and output-slot validation by the driver.
    pub fn total_outputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.total_outputs.iter()
    }

    /// Build from a Load-time `ActionContext`, splicing in each
    /// sub-action's already-computed `total_inputs`/`total_outputs` at
    /// the point of its `subaction` call. `prior` must contain entries
    /// for every sub-action this one references.
    fn from_action_ctx(prior: &[ActionMeta], ctx: &ActionContext) -> Result<Self> {
        let mut meta = Self {
            name: ctx.name.clone(),
            ..Self::default()
        };
        for inst in &ctx.insts {
            match inst {
                Inst::Object { io, obj, class, .. } => {
                    let r = ActionObjectRef {
                        io: *io,
                        class: class.clone(),
                        varname: obj.borrow().var_name().to_string(),
                    };
                    if io.consumes() {
                        meta.total_inputs.push(r.clone());
                    }
                    if io.produces() {
                        meta.total_outputs.push(r.clone());
                    }
                    meta.object_refs.push(r);
                }
                Inst::SubAction { action, .. } => {
                    let sub = prior
                        .iter()
                        .find(|a| &a.name == action)
                        .ok_or_else(|| anyhow!("subaction {action} not defined"))?;
                    meta.total_inputs.extend(sub.total_inputs.iter().cloned());
                    meta.total_outputs.extend(sub.total_outputs.iter().cloned());
                }
                _ => {}
            }
        }
        let (needs_in, needs_out) = compute_wildcard_needs(ctx);
        meta.needs_in_wildcard = needs_in;
        meta.needs_out_wildcard = needs_out;
        Ok(meta)
    }
}

/// Walk an action's Insts and determine, for each direct Object,
/// whether its `in` entry and/or `out` entry needs a wildcard. Returns
/// (needs_in, needs_out) as sets of Object var names.
///
/// An Object's pre-form sits at ts=0 (Input/Mutate); its post-form sits
/// at ts=max (Output/Mutate). Refs at intermediate ts already use
/// their own wildcard and don't influence either decision.
fn compute_wildcard_needs(ctx: &ActionContext) -> (HashSet<String>, HashSet<String>) {
    let mut object_io: HashMap<String, ObjectIO> = HashMap::new();
    for inst in &ctx.insts {
        if let Inst::Object { io, obj, .. } = inst {
            object_io.insert(obj.borrow().var_name().to_string(), *io);
        }
    }
    let mut current_ts: HashMap<String, usize> = object_io.keys().map(|v| (v.clone(), 0)).collect();
    let max_ts: HashMap<String, usize> = ctx
        .var_state
        .iter()
        .map(|(k, v)| (k.clone(), v.ts))
        .collect();
    let mut needs_in: HashSet<String> = HashSet::new();
    let mut needs_out: HashSet<String> = HashSet::new();

    // Force the wildcard on whichever side(s) the Object Ref pins.
    // Sub-field anchored refs (`var.key.is_some()`) always count
    // (double-anchoring isn't supported). Whole-dict refs
    // (`var.key.is_none()`) only count when `whole_dict_pins` is set
    // (currently true for Intro args, since pod2's `Statement::Intro`
    // only accepts literal args and can't be lifted via
    // ReplaceValueWithEntry).
    let check = |arg: &Ref,
                 whole_dict_pins: bool,
                 cur: &HashMap<String, usize>,
                 needs_in: &mut HashSet<String>,
                 needs_out: &mut HashSet<String>| {
        let arg = arg.borrow();
        let VarOrValue::Var(var) = &*arg else {
            return;
        };
        if var.key.is_none() && !whole_dict_pins {
            return;
        }
        let Some(io) = object_io.get(&var.name) else {
            return;
        };
        let ts = *cur.get(&var.name).unwrap_or(&0);
        let mts = *max_ts.get(&var.name).unwrap_or(&0);
        let at_in = matches!(io, ObjectIO::Input | ObjectIO::Mutate) && ts == 0;
        let at_out = matches!(io, ObjectIO::Output | ObjectIO::Mutate) && ts == mts;
        if at_in {
            needs_in.insert(var.name.clone());
        }
        if at_out {
            needs_out.insert(var.name.clone());
        }
    };

    for inst in &ctx.insts {
        match inst {
            Inst::Object { .. } | Inst::SubAction { .. } => {}
            Inst::Update { obj, value, .. } => {
                check(value, false, &current_ts, &mut needs_in, &mut needs_out);
                if let Some(ts) = current_ts.get_mut(obj) {
                    *ts += 1;
                }
            }
            Inst::Set { kvs, .. } => {
                for (_k, v) in kvs {
                    check(v, false, &current_ts, &mut needs_in, &mut needs_out);
                }
            }
            Inst::Statement { args, .. } => {
                for arg in args {
                    check(arg, false, &current_ts, &mut needs_in, &mut needs_out);
                }
            }
            Inst::Intro { args, .. } => {
                for arg in args {
                    check(arg, true, &current_ts, &mut needs_in, &mut needs_out);
                }
            }
        }
    }

    (needs_in, needs_out)
}

/// Collected metadata that declares a Class
#[derive(Debug)]
pub struct ClassMeta {
    pub name: String,
    // Actions that define the class with the index within the Action arguments that correspond to
    // the class.
    pub actions: Vec<(String, usize)>,
}

/// The Loader is used to store declarative module information at Load time.
struct Loader {
    txlib_mod: Arc<Module>,
    dependencies: Vec<Dependency>,
    actions: Vec<ActionHandle>,
    // Metadata extracted from `actions`
    actions_meta: Vec<ActionMeta>,
    classes: Vec<ClassMeta>,
}

impl Loader {
    fn actions_to_classes(actions: &[ActionMeta]) -> Vec<ClassMeta> {
        let mut class_to_actions: HashMap<String, Vec<(String, usize)>> = HashMap::new();
        let mut classes_ordered: Vec<String> = Vec::new();
        // Iterate every Object inst (inputs, outputs, mutates) in
        // declaration order. Each contributes one branch to its
        // class's IsX OR, dispatched via the bridge predicate named
        // by `bridge_predicate_name(class, action, varname, multi)`.
        for action in actions {
            for (obj_index, obj_ref) in action.object_refs.iter().enumerate() {
                if !classes_ordered.contains(&obj_ref.class) {
                    classes_ordered.push(obj_ref.class.clone());
                }
                let entries = class_to_actions.entry(obj_ref.class.clone()).or_default();
                entries.push((action.name.clone(), obj_index));
            }
        }
        let mut classes = Vec::new();
        for class in classes_ordered {
            let actions = class_to_actions[&class].clone();
            classes.push(ClassMeta {
                name: class,
                actions,
            });
        }
        classes
    }

    fn new(actions: Vec<ActionHandle>) -> Result<Self> {
        let txlib_mod = Arc::new(txlib::predicates::module());
        let dependencies = vec![
            Dependency::Module {
                name: "tx".to_string(),
                hash: txlib_mod.id(),
            },
            Dependency::Intro {
                pred: "Vdf(count, input, output)".to_string(),
                hash: *STANDARD_VDF_VD_HASH,
            },
            Dependency::Intro {
                pred: "LtEqU256(lhs, rhs)".to_string(),
                hash: *STANDARD_LT_EQ_U256_VD_HASH,
            },
        ];
        let mut actions_meta = Vec::with_capacity(actions.len());
        for handle in &actions {
            let meta = ActionMeta::from_action_ctx(&actions_meta, &handle.0.borrow())?;
            actions_meta.push(meta);
        }
        let classes = Self::actions_to_classes(&actions_meta);
        Ok(Self {
            txlib_mod,
            dependencies,
            actions,
            actions_meta,
            classes,
        })
    }

    /// Map (action_name, object_index) -> index of that action's branch
    /// in the class's IsX OR, matching the order in which branches are
    /// emitted by `fmt_class`. `object_index` is the 0-based position of
    /// the Object inst within the action (covers inputs, outputs, mutates).
    fn object_index_class_st_index(actions: &[ActionMeta]) -> HashMap<(String, usize), usize> {
        let mut result = HashMap::new();
        let mut class_action_count: HashMap<String, usize> = HashMap::new();
        for action in actions {
            for (obj_index, obj_ref) in action.object_refs.iter().enumerate() {
                let class_st_index = class_action_count.entry(obj_ref.class.clone()).or_insert(0);
                result.insert((action.name.clone(), obj_index), *class_st_index);
                *class_st_index += 1;
            }
        }
        result
    }

    fn module(self, engine: Rc<Engine>, ast: AST) -> SdkModule {
        let mut podlang_src = String::new();
        fmt_podlang::fmt(&self, &mut podlang_src).unwrap();

        let params = Params::default();
        let module = Arc::new(
            load_module(
                podlang_src.as_str(),
                "root",
                &params,
                slice::from_ref(&self.txlib_mod),
            )
            .expect("compiles"),
        );
        let object_index_class_st_index = Self::object_index_class_st_index(&self.actions_meta);
        let class_hashes: HashMap<String, Hash> = self
            .classes
            .iter()
            .filter_map(|c| {
                module
                    .predicate_ref_by_name(&class_predicate_name(&c.name))
                    .map(|p| (c.name.clone(), Predicate::Custom(p).hash()))
            })
            .collect();
        SdkModule {
            txlib_mod: self.txlib_mod,
            podlang_src,
            actions: self.actions_meta,
            classes: self.classes,
            object_index_class_st_index,
            module,
            engine,
            ast,
            class_hashes,
        }
    }
}

/// An SdkModule contains a loaded module and allows executing actions.
pub struct SdkModule {
    txlib_mod: Arc<Module>,
    podlang_src: String,
    actions: Vec<ActionMeta>,
    classes: Vec<ClassMeta>,
    // Maps (action_name, object_inst_index) -> that branch's index in
    // the class's IsX OR, matching fmt_class's emission order.
    object_index_class_st_index: HashMap<(String, usize), usize>,
    module: Arc<Module>,
    engine: Rc<Engine>,
    ast: AST,
    // Cached Is{class} predicate hashes. Stamped onto new objects'
    // "type" key in `new_obj` and used by `exe_action`'s phase 2 to
    // build the `DictContains(obj, "type", ...)` guard priv_op for
    // every Object inst.
    class_hashes: HashMap<String, Hash>,
}

impl SdkModule {
    fn action_by_name(&self, name: &str) -> &ActionMeta {
        self.actions.iter().find(|a| a.name == name).unwrap()
    }
    fn class_by_name(&self, name: &str) -> &ClassMeta {
        self.classes.iter().find(|a| a.name == name).unwrap()
    }
    pub fn podlang_src(&self) -> &str {
        &self.podlang_src
    }
    pub fn actions(&self) -> &[ActionMeta] {
        &self.actions
    }
    pub fn classes(&self) -> &[ClassMeta] {
        &self.classes
    }
    pub fn module(&self) -> &Arc<Module> {
        &self.module
    }
    /// Hash of the action's custom predicate in the loaded module.
    pub fn action_hash(&self, action_name: &str) -> Option<Hash> {
        self.module
            .predicate_ref_by_name(action_name)
            .map(Predicate::Custom)
            .map(|p| p.hash())
    }
    /// Hash of the `Is{class_name}` custom predicate in the loaded module.
    pub fn class_hash(&self, class_name: &str) -> Option<Hash> {
        self.class_hashes.get(class_name).copied()
    }
    pub fn executor(
        self: &Rc<Self>,
        mock: bool,
        grounding_witness: Arc<GroundingWitness>,
    ) -> Executor {
        Executor::new(self.clone(), mock, grounding_witness)
    }

    /// Compute the records-form bridge predicate name for
    /// `(action_name, object_refs_index)`. Delegates to
    /// `fmt_podlang::bridge_predicate_name` so the naming convention has
    /// a single source of truth. Multi-detection counts objects of the
    /// same class within the action regardless of side, since the
    /// IsX OR enumerates one branch per `(action, object-of-class)`.
    pub(crate) fn bridge_predicate_name(
        &self,
        action_name: &str,
        object_refs_index: usize,
    ) -> String {
        let action = self.action_by_name(action_name);
        let obj = &action.object_refs[object_refs_index];
        let count = action
            .object_refs
            .iter()
            .filter(|o| o.class == obj.class)
            .count();
        fmt_podlang::bridge_predicate_name(&obj.class, action_name, &obj.varname, count > 1)
    }

    /// Build an `Is{class}` statement whose OR branch matches
    /// `(action_name, object_refs_index)`. The records-form path:
    ///
    /// 1. Discharge `ArrayContains(<side>, <Schema><Side>::<entry>, state)`
    ///    where `<side>` is the dispatch side (`out` for produces /
    ///    mutate; `in` for input) and `state` is the focused dict.
    /// 2. Discharge the bridge predicate
    ///    `Is<Class>From<Action>[_<entry>](state, chain0, chain)` with
    ///    `[ArrayContains_st, st_action]` as sub-statements.
    /// 3. Discharge the IsX OR with the bridge statement at the matching
    ///    branch and `Statement::None` elsewhere.
    fn build_is_x(
        &self,
        bld: &mut BuildContext,
        action_name: &str,
        class: &str,
        object_refs_index: usize,
        st_action: Statement,
        bridge_array: &Array,
        entry_idx: usize,
        obj_dict: &Dictionary,
    ) -> Statement {
        let bridge_name = self.bridge_predicate_name(action_name, object_refs_index);

        // Step 1: ArrayContains(<side_array>, <entry_idx>, state).
        let st_array_contains = bld
            .builder
            .priv_op(Operation::array_contains(
                Value::from(bridge_array.clone()),
                entry_idx as i64,
                Value::from(obj_dict.clone()),
            ))
            .expect("ArrayContains for IsX bridge");

        // Step 2: discharge the bridge predicate.
        let st_bridge = bld
            .apply_custom_pred_simple(false, &bridge_name, vec![st_array_contains, st_action])
            .expect("apply bridge predicate");

        // Step 3: IsX OR with the bridge at the right branch.
        let class_meta = self.class_by_name(class);
        let mut branch_sts = vec![Statement::None; class_meta.actions.len()];
        let class_st_index =
            self.object_index_class_st_index[&(action_name.to_string(), object_refs_index)];
        branch_sts[class_st_index] = st_bridge;
        bld.apply_custom_pred_simple(false, &class_predicate_name(class), branch_sts)
            .expect("apply IsX OR")
    }
}

#[derive(Debug, Clone)]
pub struct SpendableObject {
    pub pod: MainPod,
    pub obj: Dictionary,
    pub tx: Tx,
}

impl SpendableObject {
    pub fn tx_input(&self) -> (Dictionary, Tx) {
        (self.obj.clone(), self.tx.clone())
    }
}

pub struct SpendableObjects {
    pub tx_pod: MainPod,
    pub obj_pods: Vec<MainPod>,
    pub objs: Vec<Dictionary>,
    pub tx: Tx,
}

impl SpendableObjects {
    pub fn obj(&self, index: usize) -> SpendableObject {
        SpendableObject {
            pod: self.obj_pods[index].clone(),
            obj: self.objs[index].clone(),
            tx: self.tx.clone(),
        }
    }
    pub fn objs<const N: usize>(&self) -> [SpendableObject; N] {
        let objs: Vec<_> = (0..N).map(|i| self.obj(i)).collect();
        objs.try_into().unwrap()
    }
}

/// The Executor is used to hold the state of action execution at Execution time.
pub struct Executor {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    grounding_witness: Arc<GroundingWitness>,
    prover: Box<dyn MainPodProver>,
    pod_modules: Vec<Arc<Module>>,
    module: Rc<SdkModule>,
}

/// This context is available via ActionContext at Execution time.  It keeps the state of the
/// artifacts being generated in the execution of an action.
struct ExeContext {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    tx_builder: TxBuilder,
    bld: BuildContext,
    // Input objects to be consumed by input/mutate (pre-popped stack).
    // Shared across the parent action and any nested sub-actions, which
    // pop in their rhai-call order.
    inputs: Vec<Dictionary>,
    // Shared module handle. `ActionHandle::subaction` uses it to
    // re-enter a sub-action's rhai body inline, and `new_obj` reads
    // cached class hashes off it.
    module: Rc<SdkModule>,
    // Accumulated produced (Output/Mutate) objects across the whole
    // action call tree. Each entry becomes one obj_pod in the returned
    // `SpendableObjects`. Sub-actions append into this same vec so the
    // top-level call sees them in declaration order.
    outputs: Vec<PerOutput>,
}

impl ExeContext {
    fn main_pod(&self, pod: Box<dyn Pod>) -> MainPod {
        let pub_statements = pod.pub_statements();
        MainPod {
            pod,
            public_statements: pub_statements,
            params: self.params.clone(),
        }
    }
    fn rand_value(&self) -> Value {
        // TODO: If mock return a deterministic value that is different after every call, with a
        // nonce that persists
        Value::from(rand_raw_value())
    }
}

fn prove(builder: MultiPodBuilder, prover: &dyn MainPodProver) -> MainPod {
    let solution = builder.solve().unwrap();
    log::info!("solution needs {} pods", solution.solution().pod_count);
    solution.prove(prover).unwrap().pods.pop().unwrap()
}

impl Executor {
    fn new(module: Rc<SdkModule>, mock: bool, grounding_witness: Arc<GroundingWitness>) -> Self {
        let mock_prover = MockProver {};
        let real_prover = Prover {};
        let (vd_set, prover): (_, Box<dyn MainPodProver>) = if mock {
            (VDSet::new(&[]), Box::new(mock_prover))
        } else {
            let vd_set = &*DEFAULT_VD_SET;
            (vd_set.clone(), Box::new(real_prover))
        };
        let params = Params::default();
        let modules = vec![module.txlib_mod.clone(), module.module.clone()];
        Self {
            mock,
            params,
            vd_set,
            grounding_witness,
            prover,
            pod_modules: modules,
            module,
        }
    }
    fn new_builder(&self) -> MultiPodBuilder {
        MultiPodBuilder::new(&self.params, &self.vd_set)
    }
    fn new_tx_builder(&self, ctx: &mut BuildContext, inputs: &[(Dictionary, Tx)]) -> TxBuilder {
        TxBuilder::new(ctx, inputs, self.grounding_witness.clone())
    }
    /// Prove a pod that reveals a single `Is{class}` statement for one
    /// produced output, referencing `source` (the internal pod that
    /// exposes the originating action statement).
    fn prove_is_x_pod(&self, source: &MainPod, out: &PerOutput) -> MainPod {
        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.pod_modules.clone(),
        };
        bld.builder.add_pod(source.clone()).unwrap();
        let st_class = self.module.build_is_x(
            &mut bld,
            &out.action_name,
            &out.class,
            out.object_refs_index,
            out.action_st.clone(),
            &out.out_array,
            out.out_entry_idx,
            &out.obj,
        );
        bld.builder.reveal(&st_class).unwrap();
        let pod = prove(bld.builder, &*self.prover);
        pod.pod.verify().unwrap();
        pod
    }
    /// Execute an action that consumes some input objects and produces some output objects
    pub fn action(
        &self,
        action: &str,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects, SdkError> {
        // TODO: In this function: return errors instead of panic from unwrap.
        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.pod_modules.clone(),
        };

        let total = &self.module.action_by_name(action).total_inputs;

        let mut tx_inputs = Vec::new();
        let mut rhai_input_objs: Vec<Dictionary> = Vec::with_capacity(inputs.len());
        for (input, _ref) in zip_eq(inputs, total.iter()) {
            tx_inputs.push(input.tx_input());
            bld.builder
                .add_pod(input.pod)
                .expect("MultiPodBuilder is unlimited");
            rhai_input_objs.push(input.obj);
        }
        // Reverse so rhai pops in declaration order (last-declared on top).
        rhai_input_objs.reverse();

        let tx_builder = self.new_tx_builder(&mut bld, &tx_inputs);
        let exe_rc = Rc::new(RefCell::new(ExeContext {
            mock: self.mock,
            params: self.params.clone(),
            vd_set: self.vd_set.clone(),
            inputs: rhai_input_objs,
            bld,
            tx_builder,
            module: self.module.clone(),
            outputs: Vec::new(),
        }));
        let action_handle = ActionHandle::new(action.to_string(), Some(exe_rc.clone()));
        let st_action = action_handle.exe_action()?;

        // Release the handle's Rc clone so `exe_rc` has a unique
        // owner for the `try_unwrap` below.
        action_handle.0.borrow_mut().exe_ctx = None;
        let ExeContext {
            tx_builder,
            mut bld,
            outputs,
            ..
        } = Rc::try_unwrap(exe_rc)
            .ok()
            .expect("unique ExeContext reference after rhai")
            .into_inner();

        bld.builder.reveal(&st_action).unwrap();

        let (st_tx_finalize, tx, _stats) = tx_builder.finalize(&mut bld);
        bld.builder.reveal(&st_tx_finalize).unwrap();

        // Internal pod carries both statements so obj_pods can
        // reference st_action. It's not exposed to callers.
        let internal_pod = prove(bld.builder, &*self.prover);
        internal_pod.pod.verify().unwrap();

        // User-facing tx_pod: thin wrapper that exposes only the
        // TxFinalized statement. The relayer / synchronizer's
        // `ProofParser` verifies a single-statement hash, so tx_pod
        // must contain exactly one public statement.
        // `Operation::copy` re-materializes the statement from
        // `internal_pod` into this builder so it can be revealed as
        // our own public output.
        let tx_pod = {
            let builder = self.new_builder();
            let mut bld = BuildContext {
                builder,
                modules: self.pod_modules.clone(),
            };
            bld.builder.add_pod(internal_pod.clone()).unwrap();
            bld.builder
                .pub_op(Operation::copy(st_tx_finalize.clone()))
                .unwrap();
            let pod = prove(bld.builder, &*self.prover);
            pod.pod.verify().unwrap();
            pod
        };

        let obj_pods: Vec<MainPod> = outputs
            .iter()
            .map(|out| self.prove_is_x_pod(&internal_pod, out))
            .collect();

        let objs = outputs.into_iter().map(|out| out.obj).collect();
        Ok(SpendableObjects {
            tx_pod,
            obj_pods,
            objs,
            tx,
        })
    }
}

/// One produced (Output/Mutate) object from a top-level
/// `Executor::action` call. Each becomes one `SpendableObject` in the
/// returned `objs`, with an accompanying `Is{class}` pod built from
/// the originating action's OR branch.
struct PerOutput {
    class: String,
    obj: Dictionary,
    action_name: String,
    object_refs_index: usize,
    action_st: Statement,
    /// The action's `out` record array. Shared (cloned) across all
    /// PerOutputs from the same action.
    out_array: Array,
    /// Index of `obj`'s entry in `out_array`.
    out_entry_idx: usize,
}

/// The Sdk is the main entrypoint of this crate.  It's used to load modules from manifests and
/// scripts.
pub struct Sdk {
    engine: Rc<Engine>,
}

fn new_engine() -> Engine {
    let mut engine = Engine::new();

    // Register the custom syntax: var $ident$ = $expr$
    engine
        .register_custom_syntax(
            ["var", "$ident$", "=", "$expr$"],
            true,
            |ctx: &mut EvalContext, inputs: &[Expression]| -> RuntimeResult<Dynamic> {
                fn f(
                    ctx: &mut EvalContext,
                    var_name: String,
                    expr: &Expression,
                ) -> RuntimeResult<Dynamic> {
                    let value = ctx.eval_expression_tree(expr)?;
                    let arg_ctx = value.try_cast::<ArgHandle>().expect("TODO");

                    arg_ctx.arg.borrow_mut().set_var_name(var_name.clone())?;
                    arg_ctx.ctx.0.borrow_mut().add_var(var_name.clone())?;
                    ctx.scope_mut().push(var_name, arg_ctx.clone());
                    Ok(Dynamic::from(arg_ctx))
                }
                let var_name = inputs[0].get_string_value().expect("ident").to_string();
                let expr = &inputs[1];
                f(ctx, var_name, expr).map_err(|mut e| {
                    e.set_position(expr.position());
                    e
                })
            },
        )
        .unwrap();

    // Register the custom syntax: unsafe $expr$
    engine
        .register_custom_syntax(
            ["unsafe", "$expr$"],
            true,
            |ctx: &mut EvalContext, inputs: &[Expression]| -> RuntimeResult<Dynamic> {
                fn f(ctx: &mut EvalContext, expr: &Expression) -> RuntimeResult<Dynamic> {
                    let action_handle = ctx.tag().clone_cast::<ActionHandle>();
                    action_handle.set_unsafe(true)?;
                    let result = ctx.eval_expression_tree(expr)?;
                    action_handle.set_unsafe(false)?;
                    Ok(result)
                }
                let expr = &inputs[0];
                f(ctx, expr).map_err(|mut e| {
                    e.set_position(expr.position());
                    e
                })
            },
        )
        .unwrap();

    engine
        .register_type_with_name::<ActionHandle>("ActionContext")
        .register_fn("output", ActionHandle::output)
        .register_fn("input", ActionHandle::input)
        .register_fn("mutate", ActionHandle::mutate)
        .register_fn("subaction", ActionHandle::subaction)
        .register_fn("random", ActionHandle::random)
        .register_fn("st_gt", ActionHandle::st_gt)
        .register_fn("st_sum_of", ActionHandle::st_sum_of)
        .register_fn("intro_vdf", ActionHandle::intro_vdf)
        .register_fn("intro_lt_eq_u256", ActionHandle::intro_lt_eq_u256)
        .register_fn("pow_obj_grind", ActionHandle::pow_obj_grind)
        .register_fn("top_limb_u256", ActionHandle::top_limb_u256)
        .register_type_with_name::<ArgHandle>("ArgContext")
        .register_fn("set", ArgHandle::set)
        .register_fn("get", ArgHandle::get)
        .register_fn("update", ArgHandle::update)
        .register_fn(
            "var_assign",
            |lhs: ArgHandle, rhs: Dynamic| -> RuntimeResult<()> {
                let ctx = lhs.ctx.0.borrow();
                if ctx.exe_ctx.is_some() {
                    let rhs = try_value_from_dynamic(rhs)?;
                    *lhs.arg.borrow_mut().as_mut_value() = rhs;
                }
                Ok(())
            },
        )
        .register_fn("-", arg_sub)
        .register_fn("-", |a: ArgHandle, b: i64| -> RuntimeResult<ArgHandle> {
            let ctx = a.ctx.clone();
            arg_sub(a, ArgHandle::literal(ctx, Value::from(b)))
        })
        .register_fn("-", |a: i64, b: ArgHandle| -> RuntimeResult<ArgHandle> {
            let ctx = b.ctx.clone();
            arg_sub(ArgHandle::literal(ctx, Value::from(a)), b)
        })
        .register_indexer_get(ArgHandle::entry);

    engine
}

impl Default for Sdk {
    fn default() -> Self {
        Self {
            engine: Rc::new(new_engine()),
        }
    }
}

impl Sdk {
    /// Load a module defined by the list of `actions` defined in the `src` script.
    pub fn load_module_from_src_actions(
        &self,
        src: &str,
        actions: &[&str],
    ) -> Result<Rc<SdkModule>, SdkError> {
        let scope = Scope::new();
        let ast = self.engine.compile_with_scope(&scope, src).unwrap();

        let mut action_handles = Vec::new();
        for action in actions {
            let action_handle = ActionHandle::new(action.to_string(), None);
            let mut scope = Scope::new();
            let options = CallFnOptions::new().with_tag(action_handle.clone());
            let _result = self.engine.call_fn_with_options::<Dynamic>(
                options,
                &mut scope,
                &ast,
                action,
                (action_handle.clone(),),
            )?;
            action_handles.push(action_handle);
        }

        let loader = Loader::new(action_handles)?;
        Ok(Rc::new(loader.module(self.engine.clone(), ast)))
    }

    pub fn load_module_from_src_manifest(
        &self,
        src: &str,
        manifest: &Manifest,
    ) -> Result<Rc<SdkModule>, SdkError> {
        let manifest_actions: Vec<_> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
        let sdk_module = self.load_module_from_src_actions(src, &manifest_actions)?;

        // Validate against the manifest metadata
        let loaded_classes: HashSet<_> = sdk_module
            .classes()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let manifest_classes: HashSet<_> =
            manifest.classes.iter().map(|c| c.name.as_str()).collect();
        if manifest_classes != loaded_classes {
            return Err(anyhow!(
                "manifest classes = {:?} but module classes = {:?}",
                manifest_classes,
                loaded_classes
            ))?;
        }
        if manifest.plugin.module_hash != sdk_module.module.batch.id() {
            return Err(anyhow!(
                "manifest.plugin.module_hash = {:#} but module.hash = {:#}",
                manifest.plugin.module_hash,
                sdk_module.module.batch.id()
            ))?;
        }

        Ok(sdk_module)
    }
}
