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
    backends::plonky2::{
        basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        signer::Signer as PodSigner,
    },
    frontend::{MainPod, MultiPodBuilder, Operation, OperationArg, SignedDict},
    lang::{Module, load_module},
    middleware::{
        EMPTY_VALUE, F, Hash, Key, MainPodProver, NativePredicate, OperationAux, OperationType,
        Params, Pod, Predicate, PublicKey, RawValue, SecretKey, Signer as _, Statement, VDSet,
        Value,
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

#[derive(Debug, Clone)]
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
/// operation. Pure Load-time data — enough to render podlang and to
/// derive metadata. Statement-producing operations push their
/// statement onto `ActionContext.sts` eagerly during Rhai; the only
/// state attached to an `Inst` is the pre-mutation dict for `Mutate`,
/// which is stashed here at Rhai time so emit can recover the `old`
/// arg after the rhai body has updated `obj` in place.
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
    },
    Set {
        obj: String,
        kvs: Vec<(String, Ref)>,
    },
    Statement {
        pred: NativePredicate,
        args: Vec<Ref>,
    },
    Intro {
        pred: Intro,
        args: Vec<Ref>,
    },
    /// Reference to another action executed as a sub-action. The
    /// sub-action's events are emitted live during Rhai (inside
    /// `subaction()`); only the structural reference is recorded here.
    SubAction {
        action: String,
        /// Aliases the sub-action's first producing object Ref so
        /// parent scripts can bind it via `var foo = subaction(...)`.
        obj: Ref,
    },
}

/// pod2 value type information.  Used for type checking of vars at Load time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Type {
    Unk,
    Raw,
    Int,
    Dict,
    PublicKey,
    SecretKey,
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
                Type::PublicKey => v.as_public_key().map(|_| ()),
                Type::SecretKey => v.as_secret_key().map(|_| ()),
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
                dict.get(&Key::from(key)).unwrap().expect("key exists")
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
/// structural shape (always populated); `sts` collects the action's
/// non-Object statement clauses eagerly during Rhai. `exe_ctx` is
/// shared by Rc so parent and sub-action handles see the same builder
/// / input queue / tx_builder.
struct ActionContext {
    name: String,
    insts: Vec<Inst>,
    /// Statements composing this action's predicate, in declaration
    /// order. Non-Object clauses (Set/Update/Statement/Intro/SubAction)
    /// are pushed eagerly during Rhai; Object event statements are
    /// appended at the end of `exe_action` to match the
    /// `fmt_podlang::fmt_action` clause ordering.
    sts: Vec<Statement>,
    /// User-defined vars (objects, intro outputs, temporaries). The
    /// txlib chain var is tracked separately in `chain_ts` because its
    /// rendering follows a different (protocol-defined) naming scheme.
    vars: Vec<String>,
    var_state: HashMap<String, VarState>,
    /// Timestamp of the txlib chain var. Bumped once per direct event
    /// (insert/delete/mutate) and once per sub-action call.
    chain_ts: usize,
    exe_ctx: Option<Rc<RefCell<ExeContext>>>,
    unsafe_block: bool,
}

impl ActionContext {
    fn new(name: String, exe_ctx: Option<Rc<RefCell<ExeContext>>>) -> Self {
        Self {
            name,
            insts: Vec::new(),
            sts: Vec::new(),
            vars: Vec::new(),
            var_state: HashMap::new(),
            chain_ts: 0,
            exe_ctx,
            unsafe_block: false,
        }
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
    fn inc_chain(&mut self) {
        self.chain_ts += 1;
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
        ctx.inc_chain();
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
        // The handle's ActionContext must not be borrowed across this
        // call: rhai host functions reborrow it freely.
        let _result = module.engine.call_fn_with_options::<Dynamic>(
            options,
            &mut scope,
            &module.ast,
            &action,
            (self.clone(),),
        )?;

        // Phase 2: walk the recorded insts and emit Object events
        // directly into the open action scope. Append event statements
        // to `sts` so the predicate's clause order matches
        // `fmt_podlang::fmt_action` (non-Object clauses first, then
        // the txlib events).
        let exe_rc = self.0.borrow().exe_ctx.clone().expect("exe phase");

        struct EventData {
            handle: EventHandle,
            class: String,
            object_refs_index: usize,
        }
        let mut events: Vec<EventData> = Vec::new();
        // Indices into `exe_ctx.outputs` for entries this action's
        // direct Objects pushed; we backfill `action_st` on these
        // after building `st_action`.
        let mut direct_outputs: Vec<usize> = Vec::new();
        let mut event_sts: Vec<Statement> = Vec::new();
        let mut obj_refs_index: usize = 0;

        // Borrow exe_ctx while iterating insts. Sub-actions don't run
        // during this loop (their rhai already finished inside the
        // subaction host), so the parent's exe_ctx borrow held here
        // doesn't conflict with anything.
        {
            let mut exe_ctx = exe_rc.borrow_mut();
            let exe_ctx = &mut *exe_ctx;
            // Take a stable view of insts for iteration.
            let ctx = self.0.borrow();
            for inst in &ctx.insts {
                if let Inst::Object {
                    io,
                    obj,
                    class,
                    original,
                } = inst
                {
                    let obj_dict = obj.borrow().to_dict();
                    let (st, handle) = match io {
                        ObjectIO::Output => exe_ctx.tx_builder.insert(&mut exe_ctx.bld, &obj_dict),
                        ObjectIO::Input => exe_ctx.tx_builder.delete(&mut exe_ctx.bld, &obj_dict),
                        ObjectIO::Mutate => {
                            let obj0 = original
                                .as_ref()
                                .expect("Mutate records a pre-mutation dict");
                            exe_ctx.tx_builder.mutate(&mut exe_ctx.bld, &obj_dict, obj0)
                        }
                    };
                    if io.produces() {
                        direct_outputs.push(exe_ctx.outputs.len());
                        exe_ctx.outputs.push(PerOutput {
                            class: class.clone(),
                            obj: obj_dict,
                            action_name: action.clone(),
                            object_refs_index: obj_refs_index,
                            action_st: Statement::None,
                        });
                    }
                    events.push(EventData {
                        handle,
                        class: class.clone(),
                        object_refs_index: obj_refs_index,
                    });
                    obj_refs_index += 1;
                    event_sts.push(st);
                }
            }
        }

        // Compose the action predicate: pre-existing non-Object sts
        // (eagerly accumulated during Rhai) followed by the Object
        // event statements, mirroring fmt_action's clause emission.
        let mut sts = std::mem::take(&mut self.0.borrow_mut().sts);
        sts.extend(event_sts);

        let st_action = {
            let mut exe_ctx = exe_rc.borrow_mut();
            exe_ctx
                .bld
                .apply_custom_pred_simple(false, &action, sts)
                .unwrap()
        };

        // Backfill action_st on each direct PerOutput, then attach an
        // IsX guard to every event in this scope.
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
            } in events
            {
                let st_is_x = module.build_is_x(
                    &mut exe_ctx.bld,
                    &action,
                    &class,
                    object_refs_index,
                    st_action.clone(),
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
        let op = native_pred_to_op(pred);
        let op_type = OperationType::Native(op);
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        let st = ctx.exe_ctx.as_ref().map(|exe_rc| {
            let mut exe_ctx = exe_rc.borrow_mut();
            let op_args = args.iter().map(|v| v.borrow().as_op_arg()).collect();
            exe_ctx
                .bld
                .builder
                .priv_op(Operation(op_type.clone(), op_args, OperationAux::None))
                .unwrap()
        });
        if let Some(st) = st {
            ctx.sts.push(st);
        }
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
    /// its predicate, attach guards, and close the scope — all live,
    /// before this call returns. The sub-action's predicate statement
    /// is then composed into the parent's predicate via `ctx.sts`.
    /// The returned `ArgHandle` aliases the sub's first producing
    /// object so parent scripts can bind it via
    /// `var foo = subaction("X")`.
    fn subaction(self, action: String) -> RuntimeResult<ArgHandle> {
        let exe_rc_opt = self.0.borrow().exe_ctx.clone();
        let arg_placeholder = Rc::new(RefCell::new(VarOrValue::var(Type::Dict)));

        let arg = if let Some(exe_rc) = exe_rc_opt {
            let sub_handle = ActionHandle::new(action.clone(), Some(exe_rc.clone()));
            let st_sub = sub_handle.exe_action()?;

            // Reveal so per-output IsX pods can reference the
            // sub-action's Action statement via the internal pod.
            exe_rc.borrow_mut().bld.builder.reveal(&st_sub).unwrap();

            // Drop the sub handle's shared exe_ctx so it doesn't pin
            // the shared ExeContext past this call.
            sub_handle.0.borrow_mut().exe_ctx = None;

            // Alias the parent's binding to the sub-action's first
            // producing object Ref, or a fresh placeholder if the sub
            // produces nothing.
            let aliased = sub_handle
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

            // Compose sub-action predicate into parent's clause list.
            self.0.borrow_mut().sts.push(st_sub);

            aliased
        } else {
            arg_placeholder
        };

        let mut ctx = self.0.borrow_mut();
        ctx.insts.push(Inst::SubAction {
            action,
            obj: arg.clone(),
        });
        ctx.inc_chain();
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
                    obj.update(&Key::from("key"), &k).unwrap();
                }
            }
            key.borrow_mut().set_value(k);
        }
        Ok(ArgHandle::new(self.clone(), key))
    }
    /// Build a u256 with `n` in the most-significant limb and zeros elsewhere.
    /// Useful as a difficulty target for [`pow_obj_grind`] and [`intro_lt_eq_u256`]
    /// — a u256 `x` satisfies `x <= top_limb_u256(n)` iff the top limb of `x` is
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
        let st = ctx.exe_ctx.as_ref().map(|exe_rc| {
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
            st
        });
        if let Some(st) = st {
            ctx.sts.push(st);
        }
        ctx.insts.push(Inst::Intro {
            pred: Intro::Vdf,
            args: vec![n_iters, input, work.clone()],
        });
        Ok(ArgHandle::new(self.clone(), work))
    }
    /// Build a literal `PublicKey` value from a base58 string. Use to
    /// embed a known issuer pubkey as a `let` constant in a plugin
    /// script:
    ///
    /// ```rhai
    /// let GOVT_PK = action.public_key("base58_pk_here");
    /// action.signed_by(subsidy, GOVT_PK);
    /// ```
    ///
    /// The emitted podlang renders it as a `PublicKey(<base58>)` literal.
    fn public_key(self, b58: String) -> RuntimeResult<ArgHandle> {
        use std::str::FromStr;
        let pk = PublicKey::from_str(&b58)
            .map_err(|e| -> Box<EvalAltResult> { format!("public_key: {e}").into() })?;
        Ok(ArgHandle::literal(self.clone(), Value::from(pk)))
    }
    /// Declare a `SignedBy(msg, pk)` constraint. `msg` is typically a
    /// dict-typed var (an action input/mutate/output object); `pk` is
    /// typically a literal `PublicKey` (e.g. an issuer's known key).
    ///
    /// At Execute time the prover must hold the secret key for `pk` —
    /// register it via [`Executor::add_signer`] before calling the
    /// action. The signature is generated on `msg`'s dict commitment and
    /// attached as `OperationAux::Signature`.
    fn signed_by(self, msg: Dynamic, pk: Dynamic) -> RuntimeResult<()> {
        let [msg, pk] = validate_args([(msg, Type::Dict), (pk, Type::PublicKey)])?;
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        let st = ctx.exe_ctx.as_ref().map(|exe_rc| -> RuntimeResult<Statement> {
            let mut exe_ctx = exe_rc.borrow_mut();
            let msg_value = msg.borrow().as_value();
            let pk_value = pk.borrow().as_value();
            let pk_pod = pk_value
                .as_public_key()
                .ok_or::<Box<EvalAltResult>>("signed_by: pk is not a PublicKey".into())?;
            let sk = exe_ctx
                .signers
                .get(&pk_value.raw())
                .cloned()
                .ok_or_else::<Box<EvalAltResult>, _>(|| {
                    format!(
                        "signed_by: no signer registered for PublicKey({}); call Executor::add_signer first",
                        pk_pod
                    )
                    .into()
                })?;
            let dict = msg_value
                .as_dictionary()
                .ok_or::<Box<EvalAltResult>>("signed_by: msg is not a Dictionary".into())?;
            let msg_raw = RawValue::from(dict.commitment());
            let signer = PodSigner(sk);
            let sig = signer.sign(msg_raw);
            let st = exe_ctx
                .bld
                .builder
                .priv_op(Operation(
                    OperationType::Native(native_pred_to_op(NativePredicate::SignedBy)),
                    vec![
                        OperationArg::Literal(msg_value),
                        OperationArg::Literal(pk_value),
                    ],
                    OperationAux::Signature(sig),
                ))
                .map_err(|e| -> Box<EvalAltResult> { format!("signed_by: {e}").into() })?;
            Ok(st)
        });
        if let Some(st) = st {
            ctx.sts.push(st?);
        }
        ctx.insts.push(Inst::Statement {
            pred: NativePredicate::SignedBy,
            args: vec![msg, pk],
        });
        Ok(())
    }
    /// Introduce an externally-signed dictionary as a private witness.
    /// Pops the next [`SignedDict`] from the executor's queue (see
    /// [`Executor::add_signed_input`]), verifies it was signed by `pk`,
    /// and binds it to a fresh dict-typed wildcard. Use to consume
    /// credentials issued out-of-band — e.g. an income statement
    /// signed by an employer:
    ///
    /// ```rhai
    /// let EMPLOYER_PK = action.public_key("...");
    /// var income = action.input_signed_dict(EMPLOYER_PK);
    /// // income.income, income.recipient_pk, ... are now usable
    /// ```
    ///
    /// Renders as `SignedBy(income, PublicKey(<base58>))` in the
    /// emitted podlang. The dict var stays private (no chain event,
    /// no public arg).
    fn input_signed_dict(self, pk: Dynamic) -> RuntimeResult<ArgHandle> {
        let [pk] = validate_args([(pk, Type::PublicKey)])?;
        let dict_ref = Rc::new(RefCell::new(VarOrValue::var(Type::Dict)));
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        let st = ctx
            .exe_ctx
            .as_ref()
            .map(|exe_rc| -> RuntimeResult<Statement> {
                let mut exe_ctx = exe_rc.borrow_mut();
                let pk_value = pk.borrow().as_value();
                let pk_pod = pk_value
                    .as_public_key()
                    .ok_or::<Box<EvalAltResult>>("input_signed_dict: pk is not a PublicKey".into())?;
                let signed = exe_ctx.signed_inputs.pop().ok_or_else::<Box<EvalAltResult>, _>(
                    || {
                        format!(
                            "input_signed_dict: no SignedDict queued for PublicKey({}); call Executor::add_signed_input first",
                            pk_pod
                        )
                        .into()
                    },
                )?;
                if signed.public_key != pk_pod {
                    return Err(format!(
                        "input_signed_dict: queued SignedDict was signed by PublicKey({}), expected PublicKey({})",
                        signed.public_key, pk_pod
                    )
                    .into());
                }
                signed
                    .verify()
                    .map_err(|e| -> Box<EvalAltResult> {
                        format!("input_signed_dict: signature verification failed: {e}").into()
                    })?;
                dict_ref
                    .borrow_mut()
                    .set_value(Value::from(signed.dict.clone()));
                let st = exe_ctx
                    .bld
                    .builder
                    .priv_op(Operation::dict_signed_by(&signed))
                    .map_err(|e| -> Box<EvalAltResult> {
                        format!("input_signed_dict: {e}").into()
                    })?;
                Ok(st)
            });
        if let Some(st) = st {
            ctx.sts.push(st?);
        }
        ctx.insts.push(Inst::Statement {
            pred: NativePredicate::SignedBy,
            args: vec![dict_ref.clone(), pk],
        });
        Ok(ArgHandle::new(self.clone(), dict_ref))
    }
    /// Declare a `PublicKeyOf(pk, sk)` constraint, asserting that `pk`
    /// is the public key derived from `sk`. Both args are resolved at
    /// Execute time; no extra witness is needed beyond the values
    /// themselves.
    fn public_key_of(self, pk: Dynamic, sk: Dynamic) -> RuntimeResult<()> {
        let [pk, sk] = validate_args([(pk, Type::PublicKey), (sk, Type::SecretKey)])?;
        self.native_st(NativePredicate::PublicKeyOf, vec![pk, sk])
    }
    fn intro_lt_eq_u256(self, lhs: Dynamic, rhs: Dynamic) -> RuntimeResult<()> {
        let [lhs, rhs] = validate_args([(lhs, Type::Raw), (rhs, Type::Raw)])?;
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        let st = ctx.exe_ctx.as_ref().map(|exe_rc| {
            let mut exe_ctx = exe_rc.borrow_mut();
            let l = lhs.borrow().as_value().raw();
            let r = rhs.borrow().as_value().raw();
            let pod = if exe_ctx.mock {
                LtEqU256Pod::new_boxed_mock(&exe_ctx.params, exe_ctx.vd_set.clone(), l, r)
            } else {
                LtEqU256Pod::new_boxed(&exe_ctx.params, exe_ctx.vd_set.clone(), l, r)
            }
            .unwrap();
            add_intro_pod(&mut exe_ctx, pod)
        });
        if let Some(st) = st {
            ctx.sts.push(st);
        }
        ctx.insts.push(Inst::Intro {
            pred: Intro::LtEqU256,
            args: vec![lhs, rhs],
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
            let new_sts: Vec<Statement> = ctx
                .exe_ctx
                .as_ref()
                .map(|exe_rc| {
                    let mut exe_ctx = exe_rc.borrow_mut();
                    let mut obj_set_list = Vec::new();
                    for (key, value) in &kvs {
                        let value = value.borrow().as_value().clone();
                        arg.mut_dict(|obj| {
                            obj.insert(&Key::from(key), &value).expect("TODO");
                        });
                        obj_set_list.push((key, value));
                    }
                    let mut sts = Vec::with_capacity(obj_set_list.len());
                    for (key, value) in obj_set_list {
                        let obj = arg.to_dict();
                        let st = exe_ctx
                            .bld
                            .builder
                            .priv_op(Operation::dict_contains(obj.clone(), key.clone(), value))
                            .unwrap();
                        sts.push(st);
                    }
                    sts
                })
                .unwrap_or_default();
            ctx.sts.extend(new_sts);
            ctx.insts.push(Inst::Set { obj: var_name, kvs });
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
        //     let v = obj.get(&Key::from(key)).expect("TODO").expect("TODO");
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
            let st = ctx.exe_ctx.as_ref().map(|exe_rc| {
                let mut exe_ctx = exe_rc.borrow_mut();
                let v = value.borrow().as_value().clone();
                let (obj0, obj) = arg.mut_dict(|obj| {
                    let obj0 = obj.clone();
                    obj.update(&Key::from(&key), &v).expect("TODO");
                    (obj0, obj.clone())
                });
                exe_ctx
                    .bld
                    .builder
                    .priv_op(Operation::dict_update(obj, obj0, key.clone(), v))
                    .unwrap()
            });
            if let Some(st) = st {
                ctx.sts.push(st);
            }
            ctx.inc_t_var(var_name.as_str())?;
            ctx.insts.push(Inst::Update {
                obj: var_name,
                key,
                value,
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
/// `class` is exposed; `io` is internal — used by the
/// `inputs()`/`outputs()` filters.
#[derive(Debug, Clone)]
pub struct ActionObjectRef {
    io: ObjectIO,
    pub class: String,
}

/// Collected metadata that declares an Action.
///
/// `object_refs` lists the action's direct Object instructions in
/// declaration order, matching the action predicate's public-arg
/// ordering. `aggregated_inputs` / `aggregated_outputs` flatten this
/// action's plus all transitively-called sub-actions' inputs/outputs —
/// `aggregated_inputs` is used by `Executor::action` to zip against
/// caller-supplied inputs, and both are surfaced via the `total_*`
/// helpers for driver/GUI signature reporting.
#[derive(Debug, Default)]
pub struct ActionMeta {
    pub name: String,
    object_refs: Vec<ActionObjectRef>,
    aggregated_inputs: Vec<ActionObjectRef>,
    aggregated_outputs: Vec<ActionObjectRef>,
}

impl ActionMeta {
    /// Direct object refs that this action consumes (Inputs + Mutates).
    pub fn inputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.object_refs.iter().filter(|r| r.io.consumes())
    }

    /// Direct object refs that this action produces (Outputs + Mutates).
    pub fn outputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.object_refs.iter().filter(|r| r.io.produces())
    }

    /// Object refs consumed by this action plus any transitively-called
    /// sub-actions, in declaration order. Used for tx-input zipping and
    /// for action-signature reporting.
    pub fn total_inputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.aggregated_inputs.iter()
    }

    /// Object refs produced by this action plus any transitively-called
    /// sub-actions, in declaration order. Used for action-signature
    /// reporting and output-slot validation by the driver.
    pub fn total_outputs(&self) -> impl Iterator<Item = &ActionObjectRef> {
        self.aggregated_outputs.iter()
    }

    /// Build from a Load-time `ActionContext`, splicing in each
    /// sub-action's already-computed `aggregated_inputs`/`aggregated_outputs`
    /// at the point of its `subaction` call. `prior` must contain entries
    /// for every sub-action this one references.
    fn from_action_ctx(prior: &[ActionMeta], ctx: &ActionContext) -> Result<Self> {
        let mut meta = Self {
            name: ctx.name.clone(),
            ..Self::default()
        };
        for inst in &ctx.insts {
            match inst {
                Inst::Object { io, class, .. } => {
                    let r = ActionObjectRef {
                        io: io.clone(),
                        class: class.clone(),
                    };
                    if io.consumes() {
                        meta.aggregated_inputs.push(r.clone());
                    }
                    if io.produces() {
                        meta.aggregated_outputs.push(r.clone());
                    }
                    meta.object_refs.push(r);
                }
                Inst::SubAction { action, .. } => {
                    let sub = prior
                        .iter()
                        .find(|a| &a.name == action)
                        .ok_or_else(|| anyhow!("subaction {action} not defined"))?;
                    meta.aggregated_inputs
                        .extend(sub.aggregated_inputs.iter().cloned());
                    meta.aggregated_outputs
                        .extend(sub.aggregated_outputs.iter().cloned());
                }
                _ => {}
            }
        }
        Ok(meta)
    }
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
        // declaration order. Each object contributes one public arg at
        // the same position, so the IsX OR branch for this object uses
        // that position as the state slot.
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

    fn action_by_name(&self, name: &str) -> &ActionMeta {
        self.actions_meta.iter().find(|a| a.name == name).unwrap()
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
    // Cached Is{class} predicate hashes, stamped onto new objects at
    // exe time so txlib replay can dispatch the type guard.
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

    /// Build an `Is{class}` statement whose OR branch matches
    /// `(action_name, object_refs_index)` and uses `st_action` as that
    /// branch's proof. All other branches are `Statement::None`. Used
    /// both to attach guard evidence during action emission and to
    /// construct per-output spendable certificates in separate pods.
    fn build_is_x(
        &self,
        bld: &mut BuildContext,
        action_name: &str,
        class: &str,
        object_refs_index: usize,
        st_action: Statement,
    ) -> Statement {
        let class_meta = self.class_by_name(class);
        let mut branch_sts = vec![Statement::None; class_meta.actions.len()];
        let class_st_index =
            self.object_index_class_st_index[&(action_name.to_string(), object_refs_index)];
        branch_sts[class_st_index] = st_action;
        bld.apply_custom_pred_simple(false, &class_predicate_name(class), branch_sts)
            .unwrap()
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
    // Prover-held secret keys, keyed by `RawValue::from(pk)`. Looked up
    // by `signed_by` to sign action-time messages.
    signers: HashMap<RawValue, SecretKey>,
    // Externally-signed dicts queued for `input_signed_dict` to consume
    // in declaration order. Each call pops the next entry.
    signed_inputs: Vec<SignedDict>,
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
    // Prover-held secret keys keyed by `RawValue::from(pk)`. Snapshot
    // of `Executor::signers` at action start so `signed_by` can sign
    // its message without going back through the executor.
    signers: HashMap<RawValue, SecretKey>,
    // Pending externally-signed dicts (snapshot of
    // `Executor::signed_inputs`). `input_signed_dict` pops in
    // declaration order — last entry is at the top of the stack.
    signed_inputs: Vec<SignedDict>,
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
            signers: HashMap::new(),
            signed_inputs: Vec::new(),
        }
    }

    /// Register a `SecretKey` so `signed_by(msg, pk)` calls in the
    /// next action invocation can sign with it, where `pk` is the
    /// derived public key. Repeated calls overwrite. Builder-style:
    /// chain or use as `&mut`.
    pub fn add_signer(&mut self, sk: SecretKey) -> &mut Self {
        let pk = Value::from(sk.public_key()).raw();
        self.signers.insert(pk, sk);
        self
    }

    /// Queue a `SignedDict` to be consumed by the next
    /// `input_signed_dict(pk)` call in declaration order. Use to
    /// supply externally-issued credentials (e.g. an income statement
    /// signed by an employer) as witnesses to the action's predicate.
    pub fn add_signed_input(&mut self, signed: SignedDict) -> &mut Self {
        self.signed_inputs.push(signed);
        self
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
        let action_name = self.module.action_by_name(action).name.clone();
        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.pod_modules.clone(),
        };

        let aggregated = &self.module.action_by_name(&action_name).aggregated_inputs;

        let mut tx_inputs = Vec::new();
        let mut rhai_input_objs: Vec<Dictionary> = Vec::with_capacity(inputs.len());
        for (input, _ref) in zip_eq(inputs, aggregated.iter()) {
            tx_inputs.push(input.tx_input());
            bld.builder
                .add_pod(input.pod)
                .expect("MultiPodBuilder is unlimited");
            rhai_input_objs.push(input.obj);
        }
        // Reverse so rhai pops in declaration order (last-declared on top).
        rhai_input_objs.reverse();

        let tx_builder = self.new_tx_builder(&mut bld, &tx_inputs);
        // Declaration order pops from the end; reverse so the first
        // queued SignedDict is the first one rhai sees.
        let mut signed_inputs = self.signed_inputs.clone();
        signed_inputs.reverse();
        let exe_rc = Rc::new(RefCell::new(ExeContext {
            mock: self.mock,
            params: self.params.clone(),
            vd_set: self.vd_set.clone(),
            inputs: rhai_input_objs,
            bld,
            tx_builder,
            module: self.module.clone(),
            outputs: Vec::new(),
            signers: self.signers.clone(),
            signed_inputs,
        }));
        let action_handle = ActionHandle::new(action_name.clone(), Some(exe_rc.clone()));
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
        .register_fn("public_key", ActionHandle::public_key)
        .register_fn("signed_by", ActionHandle::signed_by)
        .register_fn("public_key_of", ActionHandle::public_key_of)
        .register_fn("input_signed_dict", ActionHandle::input_signed_dict)
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
