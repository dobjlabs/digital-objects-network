use crate::api;
use hex::FromHex;
use pod2::middleware::{
    containers::{Array, Dictionary, Set},
    Hash, Key, MainPodProver, NativePredicate, Params, RawValue, Statement, VDSet, Value,
    EMPTY_VALUE,
};

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
    gen_ctx: Option<GenContext>,
}

#[derive(Default, Debug)]
struct VarState {
    ts: usize,
}

struct Var<'a> {
    name: &'a str,
    ts: usize,
    max_ts: usize,
}

impl<'a> Var<'a> {
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

impl<'a> fmt::Display for Var<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if self.ts != self.max_ts {
            write!(f, "{}", self.ts)?;
        }
        Ok(())
    }
}

struct ArgFmt<'a>(&'a HashMap<&'a str, Var<'a>>, &'a RArg);

impl<'a> fmt::Display for ArgFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = self.1.borrow();
        if let Some(name) = arg.var_name.as_ref() {
            if let Some(key) = arg.key.as_ref() {
                write!(f, "{}.{key}", self.0[name.as_str()])
            } else {
                write!(f, "{}", self.0[name.as_str()])
            }
        } else {
            write!(f, "{}", arg.value.as_ref().expect("value defined"))
        }
    }
}

impl ActionContextInner {
    fn new(name: String, gen_ctx: Option<GenContext>) -> Self {
        let mut c = Self {
            name,
            gen_ctx,
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
                    vars.push(obj.borrow().var_name.clone().unwrap());
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
        let mut vars: HashMap<&str, Var> = self
            .vars
            .iter()
            .map(|v| {
                (
                    v.as_str(),
                    Var {
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
                    objs.push((io, obj.var_name.clone().expect("obj var name")));
                }
                Inst::Set { obj, key, value } => {
                    let obj = &vars[obj.as_str()];
                    let value = ArgFmt(&vars, value);
                    writeln!(w, r#"  DictContains({obj}, "{key}", {value})"#,)?;
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
    /// Set a key of the object (doesn't modify the object)
    Set {
        obj: String,
        key: String,
        value: RArg,
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
            Self::Set { obj, key, value } => write!(f, "set {obj}.{key} {}", value.borrow()),
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
    fn new(name: String, gen_ctx: Option<GenContext>) -> Self {
        Self(Rc::new(RefCell::new(ActionContextInner::new(
            name, gen_ctx,
        ))))
    }
    fn new_obj() -> Dictionary {
        dict!({"work" => EMPTY_VALUE, "key" => Value::from(rand_raw_value())})
    }
    fn obj_io(self, io: ObjectIO, class: String) -> RResult<ArgContext> {
        let arg = Rc::new(RefCell::new(Arg::obj()));
        let mut ctx = self.0.borrow_mut();
        if let Some(gen_ctx) = &mut ctx.gen_ctx {
            match io {
                ObjectIO::Output => {
                    arg.borrow_mut().value = Some(Value::from(Self::new_obj()));
                }
                _ => todo!(),
            }
        } else {
            ctx.insts.push(Inst::Object {
                io,
                obj: arg.clone(),
                class,
            });
        }
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
        let value = Rc::new(RefCell::new(Arg::literal(placeholder())));
        Ok(ArgContext::new(self.clone(), value))
    }
    fn pow_obj_grind(self, obj: Dynamic, target: i64) -> RResult<ArgContext> {
        let key = Rc::new(RefCell::new(Arg::literal(placeholder())));
        Ok(ArgContext::new(self.clone(), key))
    }
    fn st_gt(self, v0: Dynamic, v1: Dynamic) -> RResult<()> {
        self.native_st(NativePredicate::Gt, vec![v0, v1])
    }
    fn st_sum_of(self, v0: Dynamic, v1: Dynamic, v2: Dynamic) -> RResult<()> {
        self.native_st(NativePredicate::SumOf, vec![v0, v1, v2])
    }
    fn intro_vdf(self, n_iters: Dynamic, input: Dynamic) -> RResult<ArgContext> {
        let work = Rc::new(RefCell::new(Arg::literal(placeholder())));
        self.0.borrow_mut().insts.push(Inst::Intro {
            pred: Intro::Vdf,
            args: vec![
                try_rarg_from_dynamic(n_iters)?,
                try_rarg_from_dynamic(input)?,
                work.clone(),
            ],
        });
        Ok(ArgContext::new(self.clone(), work))
    }
    fn intro_lt_eq_u256(self, lhs: Dynamic, rhs: Dynamic) -> RResult<()> {
        self.0.borrow_mut().insts.push(Inst::Intro {
            pred: Intro::LtEqU256,
            args: vec![try_rarg_from_dynamic(lhs)?, try_rarg_from_dynamic(rhs)?],
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

/// Argument to a statement template
#[derive(Clone, Debug)]
pub struct Arg {
    value: Option<Value>,
    key: Option<String>,
    is_object: bool,
    obj_set_list: Vec<(String, Value)>,
    var_name: Option<String>,
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.var_name, &self.key) {
            (Some(name), None) => write!(f, "{name}"),
            (Some(name), Some(key)) => write!(f, "{name}.{key}"),
            (None, _) => write!(f, "{}", self.value.as_ref().expect("has value")),
        }
    }
}

impl Arg {
    fn literal(value: Value) -> Self {
        Self {
            value: Some(value),
            key: None,
            is_object: false,
            obj_set_list: Vec::new(),
            var_name: None,
        }
    }
    fn obj() -> Self {
        Self {
            value: None,
            key: None,
            is_object: true,
            obj_set_list: Vec::new(),
            var_name: None,
        }
    }
    fn set_var_name(&mut self, name: String) {
        self.var_name = Some(name)
    }
    fn mut_dict<T>(&mut self, mut f: impl FnMut(&mut Dictionary) -> T) -> T {
        let obj = self.value.as_mut().expect("has value");
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

impl ArgContext {
    fn new(ctx: ActionContext, arg: RArg) -> Self {
        Self { ctx, arg }
    }
    fn literal(ctx: ActionContext, value: Value) -> Self {
        let arg = Rc::new(RefCell::new(Arg::literal(value)));
        Self::new(ctx, arg)
    }
    fn set(self, key: String, value: Dynamic) -> RResult<()> {
        let mut arg = self.arg.borrow_mut();
        if let Some(name) = &arg.var_name {
            let mut ctx = self.ctx.0.borrow_mut();
            let value = try_rarg_from_dynamic(value)?;
            if let Some(gen_ctx) = &mut ctx.gen_ctx {
                let value = value.borrow().value.clone().expect("has value");
                arg.mut_dict(|obj| {
                    obj.insert(&Key::from(&key), &value).expect("TODO");
                });
                arg.obj_set_list.push((key, value));
                // let st = gen_ctx
                //     .bld
                //     .builder
                //     .priv_op(Operation::dict_contains(obj.clone(), key, value))
                //     .unwrap();
                // gen_ctx.sts.push(st);
            } else {
                ctx.insts.push(Inst::Set {
                    obj: name.clone(),
                    key,
                    value,
                });
            }
        }
        Ok(())
    }
    fn get(self, key: String) -> RResult<ArgContext> {
        let value = Rc::new(RefCell::new(Arg::literal(placeholder())));
        Ok(ArgContext::new(self.ctx.clone(), value))
    }
    fn update(self, key: String, value: Dynamic) -> RResult<()> {
        let arg = self.arg.borrow();
        if let Some(name) = &arg.var_name {
            let mut ctx = self.ctx.0.borrow_mut();
            ctx.insts.push(Inst::Update {
                obj: name.clone(),
                key,
                value: try_rarg_from_dynamic(value)?,
            });
            ctx.inc_t_var(name.as_str())?;
        }
        Ok(())
    }
    fn entry(&mut self, index: String) -> RResult<ArgContext> {
        let mut arg = self.arg.borrow().clone();
        assert!(arg.key.is_none());
        assert!(arg.var_name.is_some());
        arg.key = Some(index);
        let arg = Rc::new(RefCell::new(arg));
        Ok(ArgContext::new(self.ctx.clone(), arg))
    }
}

fn arg_sub(a: ArgContext, b: ArgContext) -> RResult<ArgContext> {
    // TODO
    let value = Rc::new(RefCell::new(Arg::literal(placeholder())));
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
        Ok(v) => return Ok(Rc::new(RefCell::new(Arg::literal(v)))),
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
        Ok(v) => return Ok(v.borrow().value.clone().expect("TODO")),
        Err(v) => v,
    };
    let v = match v.try_cast_result::<ArgContext>() {
        Ok(v) => return Ok(v.arg.borrow().value.clone().expect("TODO")),
        Err(v) => v,
    };
    _ = v;
    Err(format!("invalid value type: {}", v.type_name()).into())
}

type RArg = Rc<RefCell<Arg>>;

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
                    let obj_name = obj.borrow().var_name.clone().expect("obj has name");
                    meta.inputs.push((obj_name, class.clone()));
                }
                Inst::Object {
                    io: ObjectIO::Output,
                    obj,
                    class,
                } => {
                    let obj_name = obj.borrow().var_name.clone().expect("obj has name");
                    meta.outputs.push((obj_name, class.clone()));
                }
                Inst::Object {
                    io: ObjectIO::Mutate,
                    obj,
                    class,
                } => {
                    let obj_name = obj.borrow().var_name.clone().expect("obj has name");
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
struct Class {
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
    classes: Vec<Class>,
}

impl Data0 {
    fn actions_to_classes(actions: &[ActionMeta]) -> Vec<Class> {
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
            classes.push(Class {
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

    fn fmt_class(&self, w: &mut dyn fmt::Write, class: &Class) -> fmt::Result {
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
        Data1 {
            txlib_mod: self.txlib_mod,
            podlang_src,
            actions: self.actions_meta,
            classes: self.classes,
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
    classes: Vec<Class>,
    module: Arc<Module>,
    engine: Engine,
    ast: AST,
}

impl Data1 {
    fn action_by_name(&self, name: &str) -> &ActionMeta {
        self.actions.iter().find(|a| a.name == name).unwrap()
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

struct GenContext {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    grounding_witness: Arc<GroundingWitness>,
    prover: Box<dyn MainPodProver>,
    tx_builder: TxBuilder,
    bld: BuildContext,
    // Statements used to build the Action custom statement
    sts: Vec<Statement>,
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
    fn action(self, action: &str, inputs: Vec<SpendableObject>) -> SpendableObjects {
        let action = self.data.action_by_name(action);
        let builder = self.new_builder();
        let mut bld = BuildContext {
            builder,
            modules: self.modules.clone(),
        };

        let mut tx_inputs = Vec::new();
        for (input_index, (_class, _name)) in action.inputs.iter().enumerate() {
            let input = &inputs[input_index];
            tx_inputs.push(input.tx_input());
        }

        let tx_builder = self.new_tx_builder(&mut bld, &tx_inputs);

        let gen_ctx = GenContext {
            mock: self.mock,
            params: self.params,
            vd_set: self.vd_set,
            grounding_witness: self.grounding_witness,
            prover: self.prover,
            bld,
            tx_builder,
            sts: Vec::new(),
        };
        let ctx = ActionContext::new(action.name.clone(), Some(gen_ctx));
        let mut scope = Scope::new();
        let _result = self
            .data
            .engine
            .call_fn::<Dynamic>(&mut scope, &self.data.ast, &action.name, (ctx.clone(),))
            .unwrap();
        for st in &ctx.0.borrow().gen_ctx.as_ref().unwrap().sts {
            println!("DBG st: {st}");
        }
        todo!()
    }
}

use common::test_state::TestState;

fn tx_hash(tx: &Tx) -> Hash {
    tx.dict().commitment()
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
            log.set("blueprint", "Log");
            var work = ctx.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(ctx) {
            var log = ctx.input("Log");
            var wood = ctx.output("Wood");
            wood.set("blueprint", "Wood");
            var key = ctx.pow_obj_grind(wood, 9007199254740992);
            wood.update("key", key);
            ctx.intro_lt_eq_u256(wood, 9007199254740992);
        }

        fn CraftSticks(ctx) {
            var wood = ctx.input("Wood");
            var stick_a = ctx.output("Stick");
            var stick_b = ctx.output("Stick");
            stick_a.set("blueprint", "Stick");
            stick_b.set("blueprint", "Stick");
        }

        fn CraftWoodPick(ctx) {
            var wood = ctx.input("Wood");
            var stick = ctx.input("Stick");
            var pick = ctx.output("WoodPick");
            pick.set("blueprint", "WoodPick");
            pick.set("durability", 100);
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
                let var_name = inputs[0].get_string_value().expect("ident").to_string();
                let expr = &inputs[1];
                let value = ctx.eval_expression_tree(expr)?;
                let arg_ctx = value.try_cast::<ArgContext>().expect("TODO");

                // Push a new variable into the scope if it doesn't already exist and upgrade it to
                // store the var_name.
                // Otherwise just set its value.
                if !ctx.scope().is_constant(&var_name).unwrap_or(false) {
                    arg_ctx.arg.borrow_mut().set_var_name(var_name.clone());
                    arg_ctx.ctx.0.borrow_mut().add_var(var_name.clone())?;
                    ctx.scope_mut().set_value(var_name, arg_ctx.clone());
                    Ok(Dynamic::from(arg_ctx))
                } else {
                    Err(format!("variable {} is constant", var_name).into())
                }
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
                lhs.arg.borrow_mut().value = Some(value);
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
        // "CraftWood",
        // "CraftSticks",
        // "CraftWoodPick",
        // "UseWoodPick",
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
    phase2.action("FindLog", vec![]);
}
