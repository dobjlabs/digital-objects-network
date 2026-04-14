use crate::api;
use hex::FromHex;
use itertools::zip_eq;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::middleware::{
    containers::{Array, Dictionary, Set},
    Hash, Key, MainPodProver, NativePredicate, Params, Pod, RawValue, Statement, VDSet, Value,
    EMPTY_VALUE,
};
use vdfpod::VdfPod;

use pod2::lang::{load_module, Module};
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder, Operation},
};
use pod2utils::{dict, macros::BuildContext, rand_raw_value};
use rhai::{Dynamic, Engine, EvalAltResult, EvalContext, Expression, Scope, AST};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::slice;
use std::sync::Arc;
use txlib::{GroundingWitness, StateRoot, Tx, TxBuilder};

#[derive(Debug)]
pub enum Dependency {
    Module { name: String, hash: Hash },
    Intro { pred: String, hash: Hash },
}

impl Dependency {
    fn fmt(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        match self {
            Dependency::Module { name, hash } => {
                writeln!(w, "use module {:#} as {name}", hash)?;
            }
            Dependency::Intro { pred, hash } => {
                writeln!(w, "use intro {pred} from {:#}", hash)?;
            }
        }
        Ok(())
    }
}

fn placeholder() -> Value {
    Value::from(0xdeadbeef)
}

#[derive(Clone)]
struct ActionContext(Rc<RefCell<ActionContextInner>>);

impl fmt::Display for ActionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.borrow())
    }
}

#[derive(Default)]
struct ActionContextInner {
    name: String,
    insts: Vec<Inst>,
    vars: Vec<String>,
    var_state: HashMap<String, VarState>,
    exe_ctx: Option<ExeContext>,
}

#[derive(Default, Debug)]
struct VarState {
    ts: usize,
}

struct VarNameFmt<'a> {
    name: &'a str,
    ts: usize,
    max_ts: usize,
}

impl<'a> VarNameFmt<'a> {
    fn inc(&mut self) {
        self.ts += 1;
    }
    fn next(&'a self) -> Self {
        Self {
            name: self.name,
            ts: self.ts + 1,
            max_ts: self.max_ts,
        }
    }
}

impl<'a> fmt::Display for VarNameFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if self.ts != self.max_ts {
            write!(f, "{}", self.ts)?;
        }
        Ok(())
    }
}

struct ArgFmt<'a>(&'a HashMap<&'a str, VarNameFmt<'a>>, &'a RArg);

impl<'a> fmt::Display for ArgFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = self.1.borrow();
        match &*arg {
            VarOrValue::Var(Var {
                name, key: None, ..
            }) => write!(f, "{}", self.0[name.as_str()]),
            VarOrValue::Var(Var {
                name,
                key: Some(key),
                ..
            }) => write!(f, "{}.{key}", self.0[name.as_str()]),
            VarOrValue::Value(value) => write!(f, "{value}"),
        }
    }
}

impl ActionContextInner {
    fn new(name: String, exe_ctx: Option<ExeContext>) -> Self {
        let mut c = Self {
            name,
            exe_ctx: exe_ctx,
            ..Default::default()
        };
        c.add_var("tx".to_string()).expect("tx not yet defined");
        c
    }
    fn add_var(&mut self, var: String) -> RResult<()> {
        if self.var_state.contains_key(&var) {
            return Err(format!("var {var} already exists").into());
        }
        self.var_state.insert(var.clone(), VarState::default());
        self.vars.push(var);
        Ok(())
    }
    fn inc_t_var(&mut self, var: &str) -> RResult<()> {
        let state = self.var_state.get_mut(var).expect("var {var} exists");
        state.ts += 1;
        Ok(())
    }
    fn mut_out_len(&self) -> usize {
        self.insts
            .iter()
            .filter(|inst| {
                matches!(
                    inst,
                    Inst::Object {
                        io: ObjectIO::Mutate | ObjectIO::Output,
                        obj: _,
                        class: _
                    }
                )
            })
            .count()
    }
    fn fmt_vars(&self) -> Vec<String> {
        let mut vars = Vec::new();
        for var in &self.vars {
            let state = &self.var_state[var];
            for i in 0..state.ts + 1 {
                if i == state.ts {
                    vars.push(var.clone());
                } else {
                    vars.push(format!("{var}{i}"));
                }
            }
        }
        vars
    }
    fn fmt_pub_vars(&self) -> Vec<String> {
        let mut vars = Vec::new();
        for inst in &self.insts {
            match inst {
                Inst::Object {
                    io: ObjectIO::Mutate | ObjectIO::Output,
                    obj,
                    class: _,
                } => {
                    vars.push(obj.borrow().var_name().to_string());
                }
                _ => {}
            }
        }
        vars.extend_from_slice(&["tx".to_string(), "tx0".to_string()]);
        vars
    }
    fn fmt_action(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        write!(w, "{}(", self.name)?;
        let pub_var_names = self.fmt_pub_vars();
        for var in &pub_var_names {
            write!(w, "{var}, ")?;
        }
        write!(w, "private: ")?;
        let var_names = self.fmt_vars();
        for (i, var) in var_names
            .iter()
            .filter(|v| !pub_var_names.contains(v))
            .enumerate()
        {
            if i != 0 {
                write!(w, ", ")?;
            }
            write!(w, "{var}")?;
        }
        let mut vars: HashMap<&str, VarNameFmt> = self
            .vars
            .iter()
            .map(|v| {
                (
                    v.as_str(),
                    VarNameFmt {
                        name: v,
                        ts: 0,
                        max_ts: self.var_state[v].ts,
                    },
                )
            })
            .collect();
        writeln!(w, ") = AND(")?;
        let mut objs = Vec::new();
        for inst in &self.insts {
            match inst {
                Inst::Object { io, obj, class } => {
                    match io {
                        ObjectIO::Input => {
                            writeln!(w, "  Is{}({})", class, ArgFmt(&vars, obj))?;
                        }
                        ObjectIO::Output => {}
                        ObjectIO::Mutate => {
                            writeln!(w, "  Is{}({})", class, ArgFmt(&vars, obj))?;
                        }
                    }
                    let obj = obj.borrow();
                    objs.push((io, obj.var_name().to_string()));
                }
                Inst::Set { obj, kvs } => {
                    let obj = &vars[obj.as_str()];
                    for (key, value) in kvs {
                        let value = ArgFmt(&vars, value);
                        writeln!(w, r#"  DictContains({obj}, "{key}", {value})"#,)?;
                    }
                }
                Inst::Update { obj, key, value } => {
                    let obj_name = obj.as_str();
                    let obj = &vars[obj_name];
                    let obj_next = obj.next();
                    let value = ArgFmt(&vars, value);
                    writeln!(w, r#"  DictUpdate({obj_next}, {obj}, "{key}", {value})"#,)?;
                    vars.get_mut(obj_name).expect("obj exists").inc();
                }
                Inst::Statement { pred, args } => {
                    write!(w, "  {pred}(")?;
                    for (i, arg) in args.iter().enumerate() {
                        if i != 0 {
                            write!(w, ", ")?;
                        }
                        write!(w, "{}", ArgFmt(&vars, arg))?;
                    }
                    writeln!(w, ")")?;
                }
                Inst::Intro { pred, args } => {
                    write!(w, "  {pred}(")?;
                    for (i, arg) in args.iter().enumerate() {
                        if i != 0 {
                            write!(w, ", ")?;
                        }
                        write!(w, "{}", ArgFmt(&vars, arg))?;
                    }
                    writeln!(w, ")")?;
                }
            }
        }
        for (io, obj) in &objs {
            let tx = &vars["tx"];
            let tx_next = tx.next();
            match io {
                ObjectIO::Input => writeln!(w, "  tx::TxDeleted({tx_next}, {tx}, {obj})")?,
                ObjectIO::Output => writeln!(w, "  tx::TxInserted({tx_next}, {tx}, {obj})")?,
                ObjectIO::Mutate => writeln!(w, "  tx::TxMutated({tx_next}, {tx}, {obj}, {obj}0)")?,
            }
            vars.get_mut("tx").expect("tx exists").inc();
        }
        writeln!(w, ")")?;
        Ok(())
    }
}

impl fmt::Display for ActionContextInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vars: ")?;
        for (i, var) in self.vars.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{var}")?;
        }
        writeln!(f, "")?;
        writeln!(f, "Instructions:")?;
        for inst in &self.insts {
            writeln!(f, "  - {}", inst)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
enum Inst {
    Object {
        io: ObjectIO,
        obj: RArg,
        class: String,
    },
    /// Update a key of the object
    Update {
        obj: String,
        key: String,
        value: RArg,
    },
    /// Set a list of keys values that the object must have
    Set {
        obj: String,
        kvs: Vec<(String, RArg)>,
    },
    Statement {
        pred: NativePredicate,
        args: Vec<RArg>,
    },
    Intro {
        pred: Intro,
        args: Vec<RArg>,
    },
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn fmt_args(args: &[RArg], f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for (i, arg) in args.iter().enumerate() {
                if i != 0 {
                    write!(f, " ")?;
                }
                write!(f, "{}", arg.borrow())?;
            }
            Ok(())
        }
        match self {
            Self::Object { io, obj, class } => write!(f, "object {io} {}: {class}", obj.borrow()),
            Self::Update { obj, key, value } => write!(f, "update {obj}.{key} {}", value.borrow()),
            Self::Set { obj, kvs } => {
                write!(f, "set [")?;
                for (i, (k, v)) in kvs.iter().enumerate() {
                    if i != 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{obj}.{k} {}", v.borrow())?;
                }
                write!(f, "]")
            }
            Self::Statement { pred, args } => {
                write!(f, "statement {pred}(")?;
                fmt_args(args, f)?;
                write!(f, ")")
            }
            Self::Intro { pred, args } => {
                write!(f, "intro {pred}(")?;
                fmt_args(args, f)?;
                write!(f, ")")
            }
        }
    }
}

#[derive(Debug)]
enum Intro {
    Vdf,      // (n_iters, input, work)
    LtEqU256, // (lhs, rhs)
}

impl fmt::Display for Intro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vdf => write!(f, "Vdf"),
            Self::LtEqU256 => write!(f, "LtEqU256"),
        }
    }
}

type RResult<T> = Result<T, Box<EvalAltResult>>;

impl ActionContext {
    //
    // Internal methods
    //
    fn new(name: String, exe_ctx: Option<ExeContext>) -> Self {
        Self(Rc::new(RefCell::new(ActionContextInner::new(
            name, exe_ctx,
        ))))
    }
    fn new_obj(exe_ctx: &ExeContext) -> Dictionary {
        dict!({"work" => EMPTY_VALUE, "key" => exe_ctx.rand_value()})
    }
    fn obj_io(self, io: ObjectIO, class: String) -> RResult<ArgContext> {
        let arg = Rc::new(RefCell::new(VarOrValue::var()));
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
        Ok(ArgContext::new(self.clone(), arg))
    }
    //
    // Exposed methods helpers
    //
    fn native_st(self, pred: NativePredicate, args: Vec<Dynamic>) -> RResult<()> {
        let args = args
            .into_iter()
            .map(|v| try_rarg_from_dynamic(v))
            .collect::<RResult<Vec<_>>>()?;
        self.0
            .borrow_mut()
            .insts
            .push(Inst::Statement { pred, args });
        Ok(())
    }
    //
    // Exposed methods
    //
    fn output(self, class: String) -> RResult<ArgContext> {
        self.obj_io(ObjectIO::Output, class)
    }
    fn input(self, class: String) -> RResult<ArgContext> {
        self.obj_io(ObjectIO::Input, class)
    }
    fn mutate(self, class: String) -> RResult<ArgContext> {
        self.obj_io(ObjectIO::Mutate, class)
    }
    fn random(self) -> RResult<ArgContext> {
        let value = Rc::new(RefCell::new(VarOrValue::var()));
        let mut ctx = self.0.borrow_mut();
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            value.borrow_mut().set_value(exe_ctx.rand_value());
        }
        Ok(ArgContext::new(self.clone(), value))
    }
    fn pow_obj_grind(self, obj: Dynamic, target: Dynamic) -> RResult<ArgContext> {
        // For now we assume that obj is var, and thus return a key that is also var
        let key = Rc::new(RefCell::new(VarOrValue::var()));
        let obj = try_rarg_from_dynamic(obj)?;
        let target = try_rarg_from_dynamic(target)?;
        let mut ctx = self.0.borrow_mut();
        if let Some(exe_ctx) = &mut ctx.exe_ctx {
            // This is a copy of the object, we don't modify the obj argument.
            let mut obj = obj.borrow().as_value().as_dictionary().expect("dict");
            let target = target.borrow().as_value().as_int().expect("int") as u64;
            let mut k = exe_ctx.rand_value();
            if !exe_ctx.mock {
                while RawValue::from(obj.commitment()).0[3].0 > target {
                    k = exe_ctx.rand_value();
                    obj.update(&Key::from("key"), &k).unwrap();
                }
            }
            key.borrow_mut().set_value(k);
        }
        Ok(ArgContext::new(self.clone(), key))
    }
    fn st_gt(self, v0: Dynamic, v1: Dynamic) -> RResult<()> {
        self.native_st(NativePredicate::Gt, vec![v0, v1])
    }
    fn st_sum_of(self, v0: Dynamic, v1: Dynamic, v2: Dynamic) -> RResult<()> {
        self.native_st(NativePredicate::SumOf, vec![v0, v1, v2])
    }
    fn intro_vdf(self, n_iters: Dynamic, input: Dynamic) -> RResult<ArgContext> {
        let n_iters = try_rarg_from_dynamic(n_iters)?;
        let input = try_rarg_from_dynamic(input)?;
        let work = Rc::new(RefCell::new(VarOrValue::var()));
        let mut ctx = self.0.borrow_mut();
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
        Ok(ArgContext::new(self.clone(), work))
    }
    fn intro_lt_eq_u256(self, lhs: Dynamic, rhs: Dynamic) -> RResult<()> {
        let lhs = try_rarg_from_dynamic(lhs)?;
        let rhs = try_rarg_from_dynamic(rhs)?;
        let mut ctx = self.0.borrow_mut();
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

#[derive(Debug)]
enum ObjectIO {
    Input,
    Mutate,
    Output,
}

impl fmt::Display for ObjectIO {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => write!(f, "in"),
            Self::Mutate => write!(f, "mut"),
            Self::Output => write!(f, "out"),
        }
    }
}

/// Corresponds to a variable/wildcard in a custom predicate.
#[derive(Debug, Clone)]
struct Var {
    name: String,
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

impl fmt::Display for VarOrValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            Self::Var(Var {
                name, key: None, ..
            }) => write!(f, "{name}"),
            Self::Var(Var {
                name,
                key: Some(key),
                ..
            }) => write!(f, "{name}.{key}"),
            Self::Value(value) => write!(f, "{value}"),
        }
    }
}

impl VarOrValue {
    fn value(value: Value) -> Self {
        Self::Value(value)
    }
    fn var() -> Self {
        Self::Var(Var {
            name: "?".to_string(),
            value: None,
            key: None,
        })
    }
    fn var_name(&self) -> &str {
        self.as_var().name.as_str()
    }
    fn set_var_name(&mut self, name: String) -> RResult<()> {
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
    fn as_value(&self) -> &Value {
        match self {
            Self::Value(value) => value,
            Self::Var(Var { value, .. }) => value.as_ref().expect("has value"),
        }
    }
    fn as_mut_value(&mut self) -> &mut Value {
        match self {
            Self::Value(value) => value,
            Self::Var(Var { value, .. }) => value.as_mut().expect("has value"),
        }
    }
    fn set_value(&mut self, value: Value) {
        match self {
            Self::Value(v) => *v = value,
            Self::Var(Var { value: v, .. }) => *v = Some(value),
        }
    }
    fn into_obj(&self) -> Dictionary {
        self.as_value().as_dictionary().expect("is dict")
    }
    fn mut_dict<T>(&mut self, mut f: impl FnMut(&mut Dictionary) -> T) -> T {
        let obj = self.as_mut_value();
        let mut dict = obj.as_dictionary().expect("is dict");
        let output = f(&mut dict);
        *obj = Value::from(dict);
        output
    }
}

#[derive(Clone)]
struct ArgContext {
    ctx: ActionContext,
    arg: RArg,
}

fn dynamic_to_kvs(kvs: Dynamic) -> RResult<Vec<(String, RArg)>> {
    let kvs = kvs
        .try_cast::<Vec<Dynamic>>()
        .ok_or_else(|| "kvs not array")?;
    let kvs = kvs
        .into_iter()
        .map(|kv| {
            kv.try_cast::<Vec<Dynamic>>()
                .ok_or_else(|| "kv not array".into())
        })
        .collect::<RResult<Vec<_>>>()?;
    kvs.into_iter()
        .map(|kv| {
            let [k, v] = kv.try_into().map_err(|_| "kv.len != 2")?;
            let k = k.try_cast::<String>().ok_or_else(|| "k not string")?;
            let v = try_rarg_from_dynamic(v)?;
            Ok((k, v))
        })
        .collect()
}

impl ArgContext {
    fn new(ctx: ActionContext, arg: RArg) -> Self {
        Self { ctx, arg }
    }
    fn literal(ctx: ActionContext, value: Value) -> Self {
        let arg = Rc::new(RefCell::new(VarOrValue::value(value)));
        Self::new(ctx, arg)
    }
    fn set(self, kvs: Dynamic) -> RResult<()> {
        let kvs = dynamic_to_kvs(kvs)?;
        let mut arg = self.arg.borrow_mut();
        if let VarOrValue::Var(var) = &*arg {
            let var_name = var.name.clone();
            let mut ctx = self.ctx.0.borrow_mut();
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
                    let obj = arg.into_obj();
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
    fn get(self, key: String) -> RResult<ArgContext> {
        // For now we assume that obj is var, and thus return a value that is also var
        let value = Rc::new(RefCell::new(VarOrValue::var()));
        let mut ctx = self.ctx.0.borrow_mut();
        if let Some(_) = &mut ctx.exe_ctx {
            let obj = self.arg.borrow().as_value().as_dictionary().expect("dict");
            let v = obj.get(&Key::from(key)).expect("TODO").expect("TODO");
            value.borrow_mut().set_value(v);
        }
        Ok(ArgContext::new(self.ctx.clone(), value))
    }
    fn update(self, key: String, value: Dynamic) -> RResult<()> {
        let mut arg = self.arg.borrow_mut();
        if let VarOrValue::Var(var) = &*arg {
            let var_name = var.name.clone();
            let value = try_rarg_from_dynamic(value)?;
            let mut ctx = self.ctx.0.borrow_mut();
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
    fn entry(&mut self, index: String) -> RResult<ArgContext> {
        let mut arg = self.arg.borrow().clone();
        let var = arg.as_mut_var();
        var.key = Some(index);
        let arg = Rc::new(RefCell::new(arg));
        Ok(ArgContext::new(self.ctx.clone(), arg))
    }
}

fn arg_sub(a: ArgContext, b: ArgContext) -> RResult<ArgContext> {
    // TODO
    let value = Rc::new(RefCell::new(VarOrValue::value(placeholder())));
    Ok(ArgContext::new(a.ctx.clone(), value))
}

fn _try_value_from_dynamic(v: Dynamic) -> Result<Value, Dynamic> {
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

fn try_rarg_from_dynamic(v: Dynamic) -> RResult<RArg> {
    let v = match _try_value_from_dynamic(v) {
        Ok(v) => return Ok(Rc::new(RefCell::new(VarOrValue::value(v)))),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<RArg>() {
        Ok(v) => return Ok(v),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<ArgContext>() {
        Ok(v) => return Ok(v.arg),
        Err(v) => v,
    };
    _ = v;
    Err(format!("invalid RArg type: {}", v.type_name()).into())
}

fn try_value_from_dynamic(v: Dynamic) -> RResult<Value> {
    let v = match _try_value_from_dynamic(v) {
        Ok(v) => return Ok(v),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<RArg>() {
        Ok(v) => return Ok(v.borrow().as_value().clone()),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<ArgContext>() {
        Ok(v) => return Ok(v.arg.borrow().as_value().clone()),
        Err(v) => v,
    };
    _ = v;
    Err(format!("invalid value type: {}", v.type_name()).into())
}

type RArg = Rc<RefCell<VarOrValue>>;

#[derive(Debug, Default)]
struct ActionMeta {
    name: String,
    /// List of (object, class) for input/mutate
    inputs: Vec<(String, String)>,
    /// List of (object, class) for output/mutate
    outputs: Vec<(String, String)>,
}

impl From<&ActionContextInner> for ActionMeta {
    fn from(action: &ActionContextInner) -> Self {
        let mut meta = Self {
            name: action.name.clone(),
            ..Self::default()
        };
        for inst in &action.insts {
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
                _ => {}
            }
        }
        meta
    }
}

#[derive(Debug)]
struct ClassMeta {
    name: String,
    // Actions that define the class with the index within the Action arguments that correspond to
    // the class.
    actions: Vec<(String, usize)>,
}

struct Data0 {
    txlib_mod: Arc<Module>,
    dependencies: Vec<Dependency>,
    actions: Vec<ActionContext>,
    // Metadata extracted from `actions`
    actions_meta: Vec<ActionMeta>,
    classes: Vec<ClassMeta>,
}

impl Data0 {
    fn actions_to_classes(actions: &[ActionMeta]) -> Vec<ClassMeta> {
        let mut class_to_actions: HashMap<String, Vec<(String, usize)>> = HashMap::new();
        let mut classes_ordered: Vec<String> = Vec::new();
        for action in actions {
            let mut classes = Vec::new();
            for (_obj, class) in &action.outputs {
                classes.push(class.clone());
                if !classes_ordered.contains(&class) {
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

    fn new(actions: Vec<ActionContext>) -> Self {
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
        let actions_meta: Vec<_> = actions
            .iter()
            .map(|a| ActionMeta::from(&*a.0.borrow()))
            .collect();
        let classes = Self::actions_to_classes(&actions_meta);
        Self {
            txlib_mod,
            dependencies,
            actions,
            actions_meta,
            classes,
        }
    }

    fn action_by_name(&self, name: &str) -> &ActionMeta {
        self.actions_meta.iter().find(|a| a.name == name).unwrap()
    }

    fn fmt_class(&self, w: &mut dyn fmt::Write, class: &ClassMeta) -> fmt::Result {
        let name = &class.name;
        write!(w, "Is{name}(state, private: tx, tx0")?;

        let other_len = class
            .actions
            .iter()
            .map(|(action_name, _)| self.action_by_name(action_name).outputs.len())
            .max()
            .unwrap()
            - 1;
        if other_len != 0 {
            write!(w, ", ")?;
        }
        for i in 0..other_len {
            if i != 0 {
                write!(w, ", ")?;
            }
            write!(w, "_other_{i}")?;
        }
        writeln!(w, ") = OR(")?;
        for (action_name, index) in &class.actions {
            write!(w, "  {action_name}(")?;
            let action = self.action_by_name(action_name);
            let mut count = 0;
            for i in 0..action.outputs.len() {
                if i != 0 {
                    write!(w, ", ")?;
                }
                if i == *index {
                    write!(w, "state")?;
                } else {
                    write!(w, "_other_{count}")?;
                    count += 1;
                }
            }
            writeln!(w, ", tx, tx0)")?;
        }
        writeln!(w, ")")?;
        Ok(())
    }

    fn fmt(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        for dep in &self.dependencies {
            dep.fmt(w).unwrap();
        }
        writeln!(w, "\n// Actions\n")?;
        for action in &self.actions {
            action.0.borrow().fmt_action(w)?;
            writeln!(w, "")?;
        }
        writeln!(w, "// Classes\n")?;
        for class in &self.classes {
            self.fmt_class(w, &class)?;
            writeln!(w, "")?;
        }
        Ok(())
    }

    fn output_index_class_st_index(actions: &[ActionMeta]) -> HashMap<(String, usize), usize> {
        let mut output_index_class_st_index = HashMap::new();
        let mut class_action_count = HashMap::new();
        for action in actions {
            for (output_index, (class, _name)) in action.outputs.iter().enumerate() {
                let class_st_index = class_action_count.entry(class).or_insert(0);
                output_index_class_st_index
                    .insert((action.name.clone(), output_index), *class_st_index);
                *class_st_index += 1;
            }
        }
        output_index_class_st_index
    }

    fn data1(self, engine: Engine, ast: AST) -> Data1 {
        let mut podlang_src = String::new();
        self.fmt(&mut podlang_src).unwrap();

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
        Data1 {
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

struct Data1 {
    txlib_mod: Arc<Module>,
    podlang_src: String,
    actions: Vec<ActionMeta>,
    classes: Vec<ClassMeta>,
    // Maps from output index in the Action to statement index in the Class predicate
    output_index_class_st_index: HashMap<(String, usize), usize>,
    module: Arc<Module>,
    engine: Engine,
    ast: AST,
}

impl Data1 {
    fn action_by_name(&self, name: &str) -> &ActionMeta {
        self.actions.iter().find(|a| a.name == name).unwrap()
    }
    fn class_by_name(&self, name: &str) -> &ClassMeta {
        self.classes.iter().find(|a| a.name == name).unwrap()
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

struct Phase2 {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    grounding_witness: Arc<GroundingWitness>,
    prover: Box<dyn MainPodProver>,
    modules: Vec<Arc<Module>>,
    data: Data1,
}

struct ExeContext {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    // grounding_witness: Arc<GroundingWitness>,
    // prover: Box<dyn MainPodProver>,
    tx_builder: TxBuilder,
    bld: BuildContext,
    // Input (class statement, object) to be consumed by input/mutate
    inputs: Vec<(Statement, Dictionary)>,
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
    log::debug!("solution needs {} pods", solution.solution().pod_count);
    solution.prove(prover).unwrap().pods.pop().unwrap()
}

impl Phase2 {
    fn new(mock: bool, data: Data1, grounding_witness: Arc<GroundingWitness>) -> Self {
        let mock_prover = MockProver {};
        let real_prover = Prover {};
        let (vd_set, prover): (_, Box<dyn MainPodProver>) = if mock {
            (VDSet::new(&[]), Box::new(mock_prover))
        } else {
            let vd_set = &*DEFAULT_VD_SET;
            (vd_set.clone(), Box::new(real_prover))
        };
        let params = Params::default();
        let modules = vec![data.txlib_mod.clone(), data.module.clone()];
        Self {
            mock,
            params,
            vd_set,
            grounding_witness,
            prover,
            modules,
            data,
        }
    }
    fn new_builder(&self) -> MultiPodBuilder {
        MultiPodBuilder::new(&self.params, &self.vd_set)
    }
    fn new_tx_builder(&self, ctx: &mut BuildContext, inputs: &[(Dictionary, Tx)]) -> TxBuilder {
        TxBuilder::new(ctx, inputs, self.grounding_witness.clone())
    }
    fn action(&self, action: &str, inputs: Vec<SpendableObject>) -> SpendableObjects {
        let action = self.data.action_by_name(action);
        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.modules.clone(),
        };

        let mut tx_inputs = Vec::new();
        let mut input_class_sts_objs = Vec::with_capacity(inputs.len());
        for (input, (_class, _name)) in zip_eq(inputs, &action.inputs) {
            tx_inputs.push(input.tx_input());
            let input_pod_sts = input.pod.pod.pub_statements();
            let st_class = input_pod_sts[0].clone();
            bld.builder.add_pod(input.pod).unwrap();
            input_class_sts_objs.push((st_class, input.obj));
        }
        // Reverse the input objects so that we can pop them in order
        input_class_sts_objs.reverse();

        let tx_builder = self.new_tx_builder(&mut bld, &tx_inputs);

        let exe_ctx = ExeContext {
            mock: self.mock,
            params: self.params.clone(),
            vd_set: self.vd_set.clone(),
            // grounding_witness: self.grounding_witness,
            // prover: self.prover,
            inputs: input_class_sts_objs.clone(),
            bld,
            tx_builder,
            sts: Vec::new(),
        };
        let rctx = ActionContext::new(action.name.clone(), Some(exe_ctx));
        let mut scope = Scope::new();
        // Execute the action rhai code
        let _result = self
            .data
            .engine
            .call_fn::<Dynamic>(&mut scope, &self.data.ast, &action.name, (rctx.clone(),))
            .unwrap();
        let mut ctx = rctx.0.borrow_mut();
        let mut exe_ctx = ctx.exe_ctx.take().unwrap();

        // Add the transaction predicates
        let mut output_objs = Vec::new();
        for inst in &ctx.insts {
            match inst {
                Inst::Object { io, obj, class } => {
                    let obj = obj.borrow().into_obj();
                    let st = match io {
                        ObjectIO::Output => {
                            output_objs.push(OutputData {
                                class: class.clone(),
                                obj: obj.clone(),
                            });
                            exe_ctx.tx_builder.insert(&mut exe_ctx.bld, obj)
                        }
                        ObjectIO::Input => {
                            input_class_sts_objs.pop().expect("exists");
                            exe_ctx.tx_builder.delete(&mut exe_ctx.bld, obj)
                        }
                        ObjectIO::Mutate => {
                            let obj0 = input_class_sts_objs.pop().expect("exists").1;
                            output_objs.push(OutputData {
                                class: class.clone(),
                                obj: obj.clone(),
                            });
                            exe_ctx.tx_builder.mutate(&mut exe_ctx.bld, obj, obj0)
                        }
                    };
                    exe_ctx.sts.push(st);
                }
                _ => {}
            };
        }
        for st in &exe_ctx.sts {
            println!("DBG st: {st}");
        }

        // Action statement
        let st_action = exe_ctx
            .bld
            .apply_custom_pred(false, &action.name, HashMap::new(), exe_ctx.sts)
            .unwrap();
        exe_ctx.bld.builder.reveal(&st_action).unwrap();

        // Data necessary to make each output object' class statement
        let mut output_objs_st_class_data = Vec::new();

        // Output (includes Output & Mutate) Class(obj) statements
        for (index, OutputData { class, obj }) in output_objs.iter().enumerate() {
            let class = self.data.class_by_name(class);
            let mut sts = vec![Statement::None; class.actions.len()];
            let class_st_index =
                self.data.output_index_class_st_index[&(action.name.clone(), index)];
            sts[class_st_index] = st_action.clone();
            let pred = format!("Is{}", class.name);
            // We delay the creation of the class statement until we have created all actions
            // because the class statements go to different pods.
            output_objs_st_class_data.push((pred, sts));
        }

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

        let mut builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.modules.clone(),
        };

        bld.builder.add_pod(pod.clone()).unwrap();
        let tx_builder = TxBuilder::new_from_tx(&bld, tx);
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut bld);
        bld.builder.reveal(&st_tx_finalize).unwrap();

        let tx_pod = prove(bld.builder, &*self.prover);
        tx_pod.pod.verify().unwrap();

        // Make one pod for each object with just the corresponding class statement.
        let mut obj_pods = Vec::new();
        for (pred, sts) in output_objs_st_class_data {
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

        let objs = output_objs.into_iter().map(|out| out.obj).collect();
        SpendableObjects {
            tx_pod,
            obj_pods,
            objs,
            tx,
        }
    }
}

use common::test_state::TestState;

fn tx_hash(tx: &Tx) -> Hash {
    tx.dict().commitment()
}

fn tx_nullifiers(tx: &Tx) -> Vec<Hash> {
    tx.nullifiers
        .iter()
        .map(|nullifier| {
            let nullifier = nullifier.expect("tx nullifier should decode");
            Hash(nullifier.raw().0)
        })
        .collect()
}

fn apply_tx(state: &mut TestState, tx: &Tx) {
    state.apply_tx(tx_hash(tx), tx_nullifiers(tx));
}

fn grounding_witness(state: &TestState, inputs: &[Tx]) -> Arc<GroundingWitness> {
    state.build_grounding_witness(
        inputs,
        tx_hash,
        |block_number, transactions_root, nullifiers_root, gsrs_root, source_tx_proofs| {
            Arc::new(GroundingWitness::new(
                StateRoot::new(block_number, transactions_root, nullifiers_root, gsrs_root),
                source_tx_proofs,
            ))
        },
    )
}

#[test]
fn test_pexe() {
    let find_log_src = r#"
        fn FindLog(ctx) {
            var log = ctx.output("Log");
            log.set([["blueprint", "Log"]]);
            var work = ctx.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(ctx) {
            var log = ctx.input("Log");
            var wood = ctx.output("Wood");
            wood.set([["blueprint", "Wood"]]);
            var key = ctx.pow_obj_grind(wood, 9007199254740992);
            wood.update("key", key);
            ctx.intro_lt_eq_u256(wood, 9007199254740992);
        }

        fn CraftSticks(ctx) {
            var wood = ctx.input("Wood");
            var stick_a = ctx.output("Stick");
            var stick_b = ctx.output("Stick");
            stick_a.set([["blueprint", "Stick"]]);
            stick_b.set([["blueprint", "Stick"]]);
        }

        fn CraftWoodPick(ctx) {
            var wood = ctx.input("Wood");
            var stick = ctx.input("Stick");
            var pick = ctx.output("WoodPick");
            pick.set([
                ["blueprint", "WoodPick"],
                ["durability", 100]
            ]);
        }

        fn use_pick(ctx, pick, vdf_iters) {
            ctx.st_gt(pick.durability, 0);
            var durability = pick.get("durability");
            // durability -= 1; // Requires AST rewrite
            var_assign(durability, durability - 1);
            ctx.st_sum_of(pick.durability, durability, 1);
            pick.update("durability", durability);
            var key = ctx.random();
            pick.update("key", key);
            var work = ctx.intro_vdf(vdf_iters, pick);
            pick.update("work", work);
        }

        fn UseWoodPick(ctx) {
            var wood_pick = ctx.mutate("WoodPick");
            use_pick(ctx, wood_pick, 10);
        }
"#;

    let mut engine = Engine::new();

    // Register the custom syntax: var x = ???
    engine
        .register_custom_syntax(
            ["var", "$ident$", "=", "$expr$"],
            true,
            |ctx: &mut EvalContext, inputs: &[Expression]| -> RResult<Dynamic> {
                fn f(
                    ctx: &mut EvalContext,
                    var_name: String,
                    expr: &Expression,
                ) -> RResult<Dynamic> {
                    let value = ctx.eval_expression_tree(expr)?;
                    let arg_ctx = value.try_cast::<ArgContext>().expect("TODO");

                    // Push a new variable into the scope if it doesn't already exist and upgrade it to
                    // store the var_name.
                    // Otherwise just set its value.
                    if !ctx.scope().is_constant(&var_name).unwrap_or(false) {
                        arg_ctx.arg.borrow_mut().set_var_name(var_name.clone())?;
                        arg_ctx.ctx.0.borrow_mut().add_var(var_name.clone())?;
                        ctx.scope_mut().set_value(var_name, arg_ctx.clone());
                        Ok(Dynamic::from(arg_ctx))
                    } else {
                        Err(format!("variable {} is constant", var_name).into())
                    }
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

    engine
        .register_type_with_name::<ActionContext>("ActionContext")
        .register_fn("output", ActionContext::output)
        .register_fn("input", ActionContext::input)
        .register_fn("mutate", ActionContext::mutate)
        .register_fn("random", ActionContext::random)
        .register_fn("st_gt", ActionContext::st_gt)
        .register_fn("st_sum_of", ActionContext::st_sum_of)
        .register_fn("intro_vdf", ActionContext::intro_vdf)
        .register_fn("intro_lt_eq_u256", ActionContext::intro_lt_eq_u256)
        .register_fn("pow_obj_grind", ActionContext::pow_obj_grind)
        .register_type_with_name::<ArgContext>("ArgContext")
        .register_fn("set", ArgContext::set)
        .register_fn("get", ArgContext::get)
        .register_fn("update", ArgContext::update)
        .register_fn(
            "var_assign",
            |lhs: ArgContext, rhs: Dynamic| -> RResult<()> {
                let value = try_value_from_dynamic(rhs)?;
                let mut ctx = lhs.ctx.0.borrow_mut();
                if let Some(_) = &mut ctx.exe_ctx {
                    *lhs.arg.borrow_mut().as_mut_value() = value;
                }
                Ok(())
            },
        )
        .register_fn("-", arg_sub)
        .register_fn("-", |a: ArgContext, b: i64| -> RResult<ArgContext> {
            let ctx = a.ctx.clone();
            arg_sub(a, ArgContext::literal(ctx, Value::from(b)))
        })
        .register_fn("-", |a: i64, b: ArgContext| -> RResult<ArgContext> {
            let ctx = b.ctx.clone();
            arg_sub(ArgContext::literal(ctx, Value::from(a)), b)
        })
        .register_indexer_get(ArgContext::entry);

    let src = find_log_src;
    let mut scope = Scope::new();
    let ast = engine.compile_with_scope(&mut scope, src).unwrap();
    // TODO: Rewrite the AST to replace assignment to `ArgContext` types by `var_assign` function
    // calls.  Otherwise we have to manually write `var_assign(foo, expr)` to assign a new value to
    // an existing `var`.

    let mut actions = Vec::new();

    for action in &[
        "FindLog",
        "CraftWood",
        "CraftSticks",
        "CraftWoodPick",
        "UseWoodPick",
    ] {
        let ctx = ActionContext::new(action.to_string(), None);
        let _result = engine
            .call_fn::<Dynamic>(&mut scope, &ast, action, (ctx.clone(),))
            .unwrap();
        actions.push(ctx);
    }

    let data = Data0::new(actions);
    let mut podlang_src = String::new();
    data.fmt(&mut podlang_src).unwrap();

    // println!("{:#?}", classes);

    println!("{podlang_src}");

    let mut state = TestState::default();
    let data = data.data1(engine, ast);

    let phase2 = Phase2::new(true, data, grounding_witness(&state, &[]));
    let [log_a] = phase2.action("FindLog", vec![]).objs();
    println!();
    apply_tx(&mut state, &log_a.tx);

    let data = phase2.data;
    let phase2 = Phase2::new(true, data, grounding_witness(&state, &[log_a.tx.clone()]));
    let [wood_a] = phase2.action("CraftWood", vec![log_a]).objs();
    println!();
    apply_tx(&mut state, &wood_a.tx);

    let data = phase2.data;
    let phase2 = Phase2::new(true, data, grounding_witness(&state, &[wood_a.tx.clone()]));
    let [stick_a, stick_b] = phase2.action("CraftSticks", vec![wood_a]).objs();
    println!();
    apply_tx(&mut state, &stick_a.tx);

    let data = phase2.data;
    let phase2 = Phase2::new(true, data, grounding_witness(&state, &[]));
    let [log_b] = phase2.action("FindLog", vec![]).objs();
    println!();
    apply_tx(&mut state, &log_b.tx);

    let data = phase2.data;
    let phase2 = Phase2::new(true, data, grounding_witness(&state, &[log_b.tx.clone()]));
    let [wood_b] = phase2.action("CraftWood", vec![log_b]).objs();
    println!();
    apply_tx(&mut state, &wood_b.tx);

    let data = phase2.data;
    let phase2 = Phase2::new(
        true,
        data,
        grounding_witness(&state, &[wood_b.tx.clone(), stick_a.tx.clone()]),
    );
    let [wood_pick] = phase2.action("CraftWoodPick", vec![wood_b, stick_a]).objs();
    println!();
    apply_tx(&mut state, &wood_pick.tx);

    let data = phase2.data;
    let phase2 = Phase2::new(
        true,
        data,
        grounding_witness(&state, &[wood_pick.tx.clone()]),
    );
    let [wood_pick] = phase2.action("UseWoodPick", vec![wood_pick]).objs();
    println!();
    apply_tx(&mut state, &wood_pick.tx);
}
