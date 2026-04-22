use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::mem;
use std::rc::Rc;
use std::slice;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use hex::FromHex;
use itertools::zip_eq;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder, Operation, OperationArg},
    lang::{load_module, Module},
    middleware::{
        containers::{Array, Dictionary, Set},
        Hash, Key, MainPodProver, NativePredicate, OperationAux, OperationType, Params, Pod,
        Predicate, RawValue, Statement, VDSet, Value, EMPTY_VALUE, F,
    },
};
use pod2utils::{dict, macros::BuildContext, rand_raw_value};
use rhai::{CallFnOptions, Dynamic, Engine, EvalAltResult, EvalContext, Expression, Scope, AST};
use txlib::{GroundingWitness, Tx, TxBuilder};
use vdfpod::VdfPod;

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

#[derive(Debug)]
enum ObjectIO {
    Input,
    Mutate,
    Output,
}

/// An instruction corresponds to some operations that happen in an action.  Each instruction has
/// associated constraints (expressed via statement templates in the action predicate).
#[derive(Debug)]
enum Inst {
    Object {
        io: ObjectIO,
        obj: Ref,
        class: String,
    },
    SubAction {
        action: String,
        obj: Ref,
    },
    /// Update a key of the object
    Update {
        obj: String,
        key: String,
        value: Ref,
    },
    /// Set a list of keys values that the object must have
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

/// Holds the state of an action being defined.
#[derive(Default)]
struct ActionContext {
    name: String,
    insts: Vec<Inst>,
    vars: Vec<String>,
    var_state: HashMap<String, VarState>,
    exe_ctx: Option<ExeContext>,
    unsafe_block: bool,
}

impl ActionContext {
    fn new(name: String, exe_ctx: Option<ExeContext>) -> Self {
        let mut c = Self {
            name,
            exe_ctx,
            ..Default::default()
        };
        c.add_var("tx".to_string()).expect("tx not yet defined");
        c
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
}

impl ActionHandle {
    //
    // Internal methods
    //
    fn new(name: String, exe_ctx: Option<ExeContext>) -> Self {
        Self(Rc::new(RefCell::new(ActionContext::new(name, exe_ctx))))
    }
    fn new_obj(exe_ctx: &ExeContext) -> Dictionary {
        dict!({"work" => EMPTY_VALUE, "key" => exe_ctx.rand_value()})
    }
    fn obj_io(self, io: ObjectIO, class: String) -> RuntimeResult<ArgHandle> {
        let arg = Rc::new(RefCell::new(VarOrValue::var(Type::Dict)));
        let mut ctx = self.0.borrow_mut();
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            match io {
                ObjectIO::Output => {
                    arg.borrow_mut()
                        .set_value(Value::from(Self::new_obj(exe_ctx)));
                }
                ObjectIO::Input => {
                    let (st_class, obj) = exe_ctx.inputs.pop().expect("exists");
                    exe_ctx.sts.push(st_class);
                    arg.borrow_mut().set_value(Value::from(obj));
                }
                ObjectIO::Mutate => {
                    let (st_class, obj) = exe_ctx.inputs.pop().expect("exists");
                    exe_ctx.sts.push(st_class);
                    arg.borrow_mut().set_value(Value::from(obj));
                }
            }
        }
        ctx.insts.push(Inst::Object {
            io,
            obj: arg.clone(),
            class,
        });
        ctx.inc_t_var("tx").expect("tx exists");
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
    // Execute an action (assumes Execution phase), returns the Action statement
    fn exe_action(&self, action: &str) -> RuntimeResult<Statement> {
        let module = {
            let ctx = self.0.borrow();
            ctx.exe_ctx.as_ref().expect("Some").module.clone()
        };
        let options = CallFnOptions::new().with_tag(self.clone());
        // Execute the action rhai code
        let mut scope = Scope::new();
        // We're passing a clone of `self` here which may have its ActionContext borrow-ed or
        // borrow_mut-ed, so we need to make sure the ActionContext is not borrowed at this point.
        let _result = module.engine.call_fn_with_options::<Dynamic>(
            options,
            &mut scope,
            &module.ast,
            &action,
            (self.clone(),),
        )?;
        let mut ctx = self.0.borrow_mut();
        let mut exe_ctx = ctx.exe_ctx.take().expect("Some"); // Take ExeContext from ActionContext

        // Add the transaction predicates
        let mut output_objs = Vec::new();
        let insts = mem::take(&mut ctx.insts); // ctx.insts is cleared for the next action
        for inst in &insts {
            if let Inst::Object { io, obj, class } = inst {
                let obj = obj.borrow().to_dict();
                let st = match io {
                    ObjectIO::Output => {
                        output_objs.push(OutputData {
                            class: class.clone(),
                            obj: obj.clone(),
                        });
                        exe_ctx.tx_builder.insert(&mut exe_ctx.bld, obj)
                    }
                    ObjectIO::Input => {
                        exe_ctx.inputs2.pop().expect("exists");
                        exe_ctx.tx_builder.delete(&mut exe_ctx.bld, obj)
                    }
                    ObjectIO::Mutate => {
                        let obj0 = exe_ctx.inputs2.pop().expect("exists").1;
                        output_objs.push(OutputData {
                            class: class.clone(),
                            obj: obj.clone(),
                        });
                        exe_ctx.tx_builder.mutate(&mut exe_ctx.bld, obj, obj0)
                    }
                };
                exe_ctx.sts.push(st);
            }
        }

        // Action statement
        let sts = mem::take(&mut exe_ctx.sts); // exe_ctx.sts is cleared for the next action
        let st_action = exe_ctx
            .bld
            .apply_custom_pred(false, &action, HashMap::new(), sts)
            .unwrap();
        // Reveal the action in the internal pod that just outputs action statements.
        exe_ctx.bld.builder.reveal(&st_action).unwrap();

        for (index, OutputData { class, obj: _ }) in output_objs.iter().enumerate() {
            let class = module.class_by_name(class);
            let mut sts = vec![Statement::None; class.actions.len()];
            let class_st_index = module.output_index_class_st_index[&(action.to_string(), index)];
            sts[class_st_index] = st_action.clone();
            let pred = format!("Is{}", class.name);
            // We delay the creation of the class statement until we have created all actions
            // because the class statements go to different pods.
            exe_ctx.output_objs_st_class_data.push((pred, sts));
        }
        exe_ctx
            .outputs
            .extend(output_objs.into_iter().map(|o| o.obj));

        ctx.exe_ctx = Some(exe_ctx); // Put ExeContext back into ActionContext
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
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            let args = args.iter().map(|v| v.borrow().as_op_arg()).collect();
            let st = exe_ctx
                .bld
                .builder
                .priv_op(Operation(op_type.clone(), args, OperationAux::None))
                .unwrap();
            exe_ctx.sts.push(st);
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
    fn subaction(self, action: String) -> RuntimeResult<ArgHandle> {
        // For now assume that a subaction returns a single output object.
        // If we want multiple outputs we need to extend the `var` syntax to declare multiple
        // variables.  We could define the var syntax with destructuring of arrays with
        // `engine.register_custom_syntax_with_state_raw`
        let arg = Rc::new(RefCell::new(VarOrValue::var(Type::Dict)));
        // We don't borrow the ActionContext here because exe_action requires exclusive access to
        // it so that it can execute the Rhai code that calls hosts functions which borrow and
        // borrow_mut the ActionContext.
        let is_exe_phase = self.0.borrow().exe_ctx.is_some();
        let st_action = if is_exe_phase {
            self.exe_action(&action)?
        } else {
            Statement::None // placeholder
        };
        let mut ctx = self.0.borrow_mut();
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            exe_ctx.sts.push(st_action);
        }
        ctx.insts.push(Inst::SubAction {
            action,
            obj: arg.clone(),
        });
        ctx.inc_t_var("tx").expect("tx exists");
        Ok(ArgHandle::new(self.clone(), arg))
    }
    fn random(self) -> RuntimeResult<ArgHandle> {
        let value = Rc::new(RefCell::new(VarOrValue::var(Type::Raw)));
        let mut ctx = self.0.borrow_mut();
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
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
        let mut ctx = self.0.borrow_mut();
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
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
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            let n_iters = n_iters.borrow().as_value().as_int().expect("int") as usize;
            let input = input.borrow().as_value().raw();
            let vdf_pod = if exe_ctx.mock {
                VdfPod::new_boxed_mock(&exe_ctx.params, exe_ctx.vd_set.clone(), n_iters, input)
            } else {
                VdfPod::new_boxed(&exe_ctx.params, exe_ctx.vd_set.clone(), n_iters, input)
            }
            .unwrap();
            let st_vdf = vdf_pod.pub_statements()[0].clone();
            let work_value = st_vdf.args()[2].literal().unwrap();
            exe_ctx
                .bld
                .builder
                .add_pod(exe_ctx.main_pod(vdf_pod))
                .unwrap();
            exe_ctx.sts.push(st_vdf);
            work.borrow_mut().set_value(work_value);
        }
        ctx.insts.push(Inst::Intro {
            pred: Intro::Vdf,
            args: vec![n_iters, input, work.clone()],
        });
        Ok(ArgHandle::new(self.clone(), work))
    }
    fn intro_lt_eq_u256(self, lhs: Dynamic, rhs: Dynamic) -> RuntimeResult<()> {
        let [lhs, rhs] = validate_args([(lhs, Type::Raw), (rhs, Type::Raw)])?;
        let mut ctx = self.0.borrow_mut();
        ctx.assert_unsafe(false)?;
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            let lhs = lhs.borrow().as_value().raw();
            let rhs = rhs.borrow().as_value().raw();
            let lt_eq_u256_pod = if exe_ctx.mock {
                LtEqU256Pod::new_boxed_mock(&exe_ctx.params, exe_ctx.vd_set.clone(), lhs, rhs)
            } else {
                LtEqU256Pod::new_boxed(&exe_ctx.params, exe_ctx.vd_set.clone(), lhs, rhs)
            }
            .unwrap();
            let st_lt_eq_u256 = lt_eq_u256_pod.pub_statements()[0].clone();
            exe_ctx
                .bld
                .builder
                .add_pod(exe_ctx.main_pod(lt_eq_u256_pod))
                .unwrap();
            exe_ctx.sts.push(st_lt_eq_u256);
        }
        ctx.insts.push(Inst::Intro {
            pred: Intro::LtEqU256,
            args: vec![lhs, rhs],
        });
        Ok(())
    }
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
            if let Some(exe_ctx) = &mut ctx.exe_ctx {
                let mut obj_set_list = Vec::new();
                for (key, value) in &kvs {
                    let value = value.borrow().as_value().clone();
                    arg.mut_dict(|obj| {
                        obj.insert(&Key::from(key), &value).expect("TODO");
                    });
                    obj_set_list.push((key, value));
                }
                for (key, value) in obj_set_list {
                    let obj = arg.to_dict();
                    let st = exe_ctx
                        .bld
                        .builder
                        .priv_op(Operation::dict_contains(obj.clone(), key.clone(), value))
                        .unwrap();
                    exe_ctx.sts.push(st);
                }
            }
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
            if let Some(exe_ctx) = &mut ctx.exe_ctx {
                let value = value.borrow().as_value().clone();
                let (obj0, obj) = arg.mut_dict(|obj| {
                    let obj0 = obj.clone();
                    obj.update(&Key::from(&key), &value).expect("TODO");
                    (obj0, obj.clone())
                });
                let st = exe_ctx
                    .bld
                    .builder
                    .priv_op(Operation::dict_update(obj, obj0, key.clone(), value))
                    .unwrap();
                exe_ctx.sts.push(st);
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

/// Collected metadata that declares an Action
#[derive(Debug, Default)]
pub struct ActionMeta {
    pub name: String,
    /// List of (object, class) for input/mutate
    pub inputs: Vec<(String, String)>,
    /// List of (object, class) for output/mutate
    pub outputs: Vec<(String, String)>,
}

impl ActionMeta {
    fn from_action_ctx(actions: &[ActionMeta], action_ctx: &ActionContext) -> Result<ActionMeta> {
        let mut meta = Self {
            name: action_ctx.name.clone(),
            ..Self::default()
        };
        for inst in &action_ctx.insts {
            match inst {
                Inst::Object {
                    io: ObjectIO::Input,
                    obj,
                    class,
                } => {
                    let obj_name = obj.borrow().as_var().name.clone();
                    meta.inputs.push((obj_name, class.clone()));
                }
                Inst::Object {
                    io: ObjectIO::Output,
                    obj,
                    class,
                } => {
                    let obj_name = obj.borrow().as_var().name.clone();
                    meta.outputs.push((obj_name, class.clone()));
                }
                Inst::Object {
                    io: ObjectIO::Mutate,
                    obj,
                    class,
                } => {
                    let obj_name = obj.borrow().as_var().name.clone();
                    meta.inputs.push((obj_name.clone(), class.clone()));
                    meta.outputs.push((obj_name, class.clone()));
                }
                Inst::SubAction { action, obj: _ } => {
                    let subaction = actions
                        .iter()
                        .find(|a| &a.name == action)
                        .ok_or_else(|| anyhow!("subaction {action} not defined"))?;
                    for input in &subaction.inputs {
                        meta.inputs.push(input.clone());
                    }
                    // For now subactions only support 1 output
                    // assert_eq!(1, subaction.outputs.len());
                    // let class = subaction.outputs[0].1.clone();
                    // let obj_name = obj.borrow().as_var().name.clone();
                    // meta.outputs.push((obj_name, class));
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
        for action in actions {
            let mut classes = Vec::new();
            for (_obj, class) in &action.outputs {
                classes.push(class.clone());
                if !classes_ordered.contains(class) {
                    classes_ordered.push(class.clone());
                }
            }
            for (i, class) in classes.iter().enumerate() {
                let actions = class_to_actions.entry(class.clone()).or_default();
                actions.push((action.name.clone(), i));
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
                hash: Hash::from_hex(
                    "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
                )
                .unwrap(),
            },
            Dependency::Intro {
                pred: "LtEqU256(lhs, rhs)".to_string(),
                hash: Hash::from_hex(
                    "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529",
                )
                .unwrap(),
            },
        ];
        let mut actions_meta = Vec::with_capacity(actions.len());
        for action in &actions {
            let action_meta = ActionMeta::from_action_ctx(&actions_meta, &*action.0.borrow())?;
            actions_meta.push(action_meta);
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

    fn output_index_class_st_index(actions: &[ActionMeta]) -> HashMap<(String, usize), usize> {
        let mut output_index_class_st_index = HashMap::new();
        let mut class_action_count = HashMap::new();
        for action in actions {
            for (output_index, (_name, class)) in action.outputs.iter().enumerate() {
                let class_st_index = class_action_count.entry(class).or_insert(0);
                output_index_class_st_index
                    .insert((action.name.clone(), output_index), *class_st_index);
                *class_st_index += 1;
            }
        }
        output_index_class_st_index
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
        let output_index_class_st_index = Self::output_index_class_st_index(&self.actions_meta);
        SdkModule {
            txlib_mod: self.txlib_mod,
            podlang_src,
            actions: self.actions_meta,
            classes: self.classes,
            output_index_class_st_index,
            module,
            engine,
            ast,
        }
    }
}

/// An SdkModule contains a loaded module and allows executing actions.
pub struct SdkModule {
    txlib_mod: Arc<Module>,
    podlang_src: String,
    actions: Vec<ActionMeta>,
    classes: Vec<ClassMeta>,
    // Maps from output index in the Action to statement index in the Class predicate
    output_index_class_st_index: HashMap<(String, usize), usize>,
    module: Arc<Module>,
    engine: Rc<Engine>,
    ast: AST,
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
        let pred_name = format!("Is{class_name}");
        self.module
            .predicate_ref_by_name(pred_name.as_str())
            .map(Predicate::Custom)
            .map(|p| p.hash())
    }
    pub fn executor(
        self: &Rc<Self>,
        mock: bool,
        grounding_witness: Arc<GroundingWitness>,
    ) -> Executor {
        Executor::new(self.clone(), mock, grounding_witness)
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
    module: Rc<SdkModule>,
    // -- Consumed during execution --
    // Input (class statement, object) to be consumed by input/mutate
    inputs: Vec<(Statement, Dictionary)>,
    inputs2: Vec<(Statement, Dictionary)>,
    // -- Generated during execution --
    outputs: Vec<Dictionary>,
    // Data necessary to make each output object' class statement: (predicate name, statements)
    output_objs_st_class_data: Vec<(String, Vec<Statement>)>,
    // Statements used to build the Action custom statement
    sts: Vec<Statement>,
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

struct OutputData {
    class: String,
    obj: Dictionary,
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
    /// Execute an action that consumes some input objects and produces some output objects
    pub fn action(
        &self,
        action: &str,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects, SdkError> {
        // TODO: In this function: return errors instead of panic from unwrap.
        let action = self.module.action_by_name(action);
        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.pod_modules.clone(),
        };

        let mut tx_inputs = Vec::new();
        let mut input_class_sts_objs = Vec::with_capacity(inputs.len());
        for (input, (_class, _name)) in zip_eq(inputs, &action.inputs) {
            tx_inputs.push(input.tx_input());
            let input_pod_sts = input.pod.pod.pub_statements();
            let st_class = input_pod_sts[0].clone();
            bld.builder
                .add_pod(input.pod)
                .expect("MultiPodBuilder is unlimited");
            input_class_sts_objs.push((st_class, input.obj));
        }
        // Reverse the input objects so that we can pop them in order
        input_class_sts_objs.reverse();

        let tx_builder = self.new_tx_builder(&mut bld, &tx_inputs);

        let exe_ctx = ExeContext {
            mock: self.mock,
            params: self.params.clone(),
            vd_set: self.vd_set.clone(),
            tx_builder,
            bld,
            module: self.module.clone(),
            inputs: input_class_sts_objs.clone(),
            inputs2: input_class_sts_objs.clone(),
            outputs: Vec::new(),
            output_objs_st_class_data: Vec::new(),
            sts: Vec::new(),
        };
        let action_handle = ActionHandle::new(action.name.clone(), Some(exe_ctx));
        action_handle.exe_action(&action.name)?;

        // let options = CallFnOptions::new().with_tag(action_handle.clone());
        // // Execute the action rhai code
        // let mut scope = Scope::new();
        // let _result = self.module.engine.call_fn_with_options::<Dynamic>(
        //     options,
        //     &mut scope,
        //     &self.module.ast,
        //     &action.name,
        //     (action_handle.clone(),),
        // )?;
        let mut ctx = action_handle.0.borrow_mut();
        let mut exe_ctx = ctx.exe_ctx.take().expect("Some");

        // // Add the transaction predicates
        // let mut output_objs = Vec::new();
        // for inst in &ctx.insts {
        //     if let Inst::Object { io, obj, class } = inst {
        //         let obj = obj.borrow().to_dict();
        //         let st = match io {
        //             ObjectIO::Output => {
        //                 output_objs.push(OutputData {
        //                     class: class.clone(),
        //                     obj: obj.clone(),
        //                 });
        //                 exe_ctx.tx_builder.insert(&mut exe_ctx.bld, obj)
        //             }
        //             ObjectIO::Input => {
        //                 input_class_sts_objs.pop().expect("exists");
        //                 exe_ctx.tx_builder.delete(&mut exe_ctx.bld, obj)
        //             }
        //             ObjectIO::Mutate => {
        //                 let obj0 = input_class_sts_objs.pop().expect("exists").1;
        //                 output_objs.push(OutputData {
        //                     class: class.clone(),
        //                     obj: obj.clone(),
        //                 });
        //                 exe_ctx.tx_builder.mutate(&mut exe_ctx.bld, obj, obj0)
        //             }
        //         };
        //         exe_ctx.sts.push(st);
        //     }
        // }

        // // Action statement
        // let st_action = exe_ctx
        //     .bld
        //     .apply_custom_pred(false, &action.name, HashMap::new(), exe_ctx.sts)
        //     .unwrap();
        // exe_ctx.bld.builder.reveal(&st_action).unwrap();

        // // Data necessary to make each output object' class statement
        // let mut output_objs_st_class_data = Vec::new();

        // // Output (includes Output & Mutate) Class(obj) statements
        // for (index, OutputData { class, obj: _ }) in output_objs.iter().enumerate() {
        //     let class = self.module.class_by_name(class);
        //     let mut sts = vec![Statement::None; class.actions.len()];
        //     let class_st_index =
        //         self.module.output_index_class_st_index[&(action.name.clone(), index)];
        //     sts[class_st_index] = st_action.clone();
        //     let pred = format!("Is{}", class.name);
        //     // We delay the creation of the class statement until we have created all actions
        //     // because the class statements go to different pods.
        //     output_objs_st_class_data.push((pred, sts));
        // }

        // output_objs.extend(output.into_iter().map(|out| out.obj));

        // Prove a pod with the class statements and the last tx statement
        exe_ctx
            .bld
            .builder
            .reveal(exe_ctx.tx_builder.st_tx())
            .unwrap();
        let pod = prove(exe_ctx.bld.builder, &*self.prover);
        pod.pod.verify().unwrap();

        // Finalize tx and prove it in another pod
        let tx = exe_ctx.tx_builder.tx;

        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.pod_modules.clone(),
        };

        bld.builder.add_pod(pod.clone()).unwrap();
        let tx_builder = TxBuilder::new_from_tx(&bld, tx);
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut bld);
        bld.builder.reveal(&st_tx_finalize).unwrap();

        let tx_pod = prove(bld.builder, &*self.prover);
        tx_pod.pod.verify().unwrap();

        // Make one pod for each object with just the corresponding class statement.
        let mut obj_pods = Vec::new();
        for (pred, sts) in exe_ctx.output_objs_st_class_data {
            bld.builder = self.new_builder();
            bld.builder.add_pod(pod.clone()).unwrap();
            let st_class = bld
                .apply_custom_pred(false, &pred, HashMap::new(), sts)
                .unwrap();
            bld.builder.reveal(&st_class).unwrap();

            let obj_pod = prove(bld.builder, &*self.prover);
            obj_pod.pod.verify().unwrap();
            obj_pods.push(obj_pod);
        }

        // let objs = output_objs.into_iter().map(|out| out.obj).collect();
        Ok(SpendableObjects {
            tx_pod,
            obj_pods,
            objs: exe_ctx.outputs,
            tx,
        })
    }
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
