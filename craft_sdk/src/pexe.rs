use crate::api;
use pod2::middleware::{
    containers::{Array, Dictionary, Set},
    NativePredicate, RawValue, Value,
};
use pod2utils::dict;
use rhai::{Dynamic, Engine, EvalAltResult, EvalContext, Expression, Scope};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

fn placeholder() -> Value {
    Value::from(0xdeadbeef)
}

#[derive(Clone, Debug)]
struct Context(Rc<RefCell<ContextInner>>);

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.borrow())
    }
}

#[derive(Default, Debug)]
struct ContextInner {
    insts: Vec<Inst>,
    vars: Vec<String>,
    var_state: HashMap<String, VarState>,
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

impl ContextInner {
    fn new() -> Self {
        let mut c = Self::default();
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
    fn fmt_action(&self, w: &mut dyn fmt::Write, name: String) -> fmt::Result {
        write!(w, "{name}(")?;
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
                ObjectIO::Input => writeln!(w, "  tx::Deleted({tx_next}, {tx}, {obj})")?,
                ObjectIO::Output => writeln!(w, "  tx::Inserted({tx_next}, {tx}, {obj})")?,
                ObjectIO::Mutate => writeln!(w, "  tx::Mutated({tx_next}, {tx}, {obj}, {obj}0)")?,
            }
            vars.get_mut("tx").expect("tx exists").inc();
        }
        writeln!(w, ")")?;
        Ok(())
    }
}

impl fmt::Display for ContextInner {
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

impl Context {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(ContextInner::new())))
    }
    fn obj_io(self, io: ObjectIO, class: String) -> RResult<ArgContext> {
        let arg = Rc::new(RefCell::new(Arg::obj()));
        let mut ctx = self.0.borrow_mut();
        ctx.insts.push(Inst::Object {
            io,
            obj: arg.clone(),
            class,
        });
        ctx.inc_t_var("tx").expect("tx exists");
        Ok(ArgContext::new(self.clone(), arg))
    }
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
            var_name: None,
        }
    }
    fn obj() -> Self {
        Self {
            value: None,
            key: None,
            is_object: true,
            var_name: None,
        }
    }
    fn set_var_name(&mut self, name: String) {
        self.var_name = Some(name)
    }
}

#[derive(Clone)]
struct ArgContext {
    ctx: Context,
    arg: RArg,
}

impl ArgContext {
    fn new(ctx: Context, arg: RArg) -> Self {
        Self { ctx, arg }
    }
    fn literal(ctx: Context, value: Value) -> Self {
        let arg = Rc::new(RefCell::new(Arg::literal(value)));
        Self::new(ctx, arg)
    }
    fn set(self, key: String, value: Dynamic) -> RResult<()> {
        let arg = self.arg.borrow();
        if let Some(name) = &arg.var_name {
            self.ctx.0.borrow_mut().insts.push(Inst::Set {
                obj: name.clone(),
                key,
                value: try_rarg_from_dynamic(value)?,
            });
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
            // durability -= 1;
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
        .register_type_with_name::<Context>("Context")
        .register_fn("output", Context::output)
        .register_fn("input", Context::input)
        .register_fn("mutate", Context::mutate)
        .register_fn("random", Context::random)
        .register_fn("st_gt", Context::st_gt)
        .register_fn("st_sum_of", Context::st_sum_of)
        .register_fn("intro_vdf", Context::intro_vdf)
        .register_fn("intro_lt_eq_u256", Context::intro_lt_eq_u256)
        .register_fn("pow_obj_grind", Context::pow_obj_grind)
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

    for action in &[
        // "FindLog",
        // "CraftWood",
        // "CraftSticks",
        // "CraftWoodPick",
        "UseWoodPick",
    ] {
        let ctx = Context::new();
        let _result = engine
            .call_fn::<Dynamic>(&mut scope, &ast, action, (ctx.clone(),))
            .unwrap();
        println!("{action}:\n{ctx}\n");
        let mut podlang_src = String::new();
        ctx.0
            .borrow()
            .fmt_action(&mut podlang_src, action.to_string())
            .unwrap();
        println!("{podlang_src}");
    }
}
// TODO: Fix durability var
