use crate::api;
use pod2::middleware::{
    containers::{Array, Dictionary, Set},
    RawValue, Value,
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

#[derive(Default, Clone, Debug)]
struct Context(Rc<RefCell<ContextInner>>);

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.borrow())
    }
}

#[derive(Default, Debug)]
struct ContextInner {
    // objs: Vec<Object>,
    insts: Vec<Inst>,
}

impl fmt::Display for ContextInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // writeln!(f, "Objects:")?;
        // for obj in &self.objs {
        //     writeln!(f, "  - {}", obj)?;
        // }
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
    }
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
    Intro(Intro),
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Update { obj, key, value } => write!(f, "update {obj}.{key} {}", value.borrow()),
            Self::Set { obj, key, value } => write!(f, "set {obj}.{key} {}", value.borrow()),
            Self::Intro(intro) => write!(f, "intro {intro}"),
        }
    }
}

#[derive(Debug)]
enum Intro {
    Vdf {
        n_iters: RArg,
        input: RArg,
        work: RArg,
    },
    LtEqU256 {
        lhs: RArg,
        rhs: RArg,
    },
}

impl fmt::Display for Intro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vdf {
                n_iters,
                input,
                work,
            } => write!(
                f,
                "vdf({}, {}, {})",
                n_iters.borrow(),
                input.borrow(),
                work.borrow()
            ),
            Self::LtEqU256 { lhs, rhs } => {
                write!(f, "lt_eq_u256({}, {})", lhs.borrow(), rhs.borrow(),)
            }
        }
    }
}

type RResult<T> = Result<T, Box<EvalAltResult>>;

impl Context {
    fn output(self, class: String) -> RResult<ArgContext> {
        let arg = Rc::new(RefCell::new(Arg::obj()));
        let obj = Object::new(class, ObjectIO::Output, arg.clone());
        self.0.borrow_mut().objs.push(obj);
        Ok(ArgContext::new(self.clone(), arg))
    }
    fn input(self, class: String) -> RResult<ArgContext> {
        let arg = Rc::new(RefCell::new(Arg::obj()));
        let obj = Object::new(class, ObjectIO::Input, arg.clone());
        self.0.borrow_mut().objs.push(obj);
        Ok(ArgContext::new(self.clone(), arg))
    }
    fn intro_vdf(self, n_iters: Dynamic, input: Dynamic) -> RResult<ArgContext> {
        let work = Rc::new(RefCell::new(Arg::literal(placeholder())));
        self.0.borrow_mut().insts.push(Inst::Intro(Intro::Vdf {
            n_iters: try_rarg_from_dynamic(n_iters)?,
            input: try_rarg_from_dynamic(input)?,
            work: work.clone(),
        }));
        Ok(ArgContext::new(self.clone(), work))
    }
    fn pow_obj_grind(self, obj: Dynamic, target: i64) -> RResult<ArgContext> {
        let key = Rc::new(RefCell::new(Arg::literal(placeholder())));
        Ok(ArgContext::new(self.clone(), key))
    }
    fn intro_lt_eq_u256(self, lhs: Dynamic, rhs: Dynamic) -> RResult<()> {
        self.0.borrow_mut().insts.push(Inst::Intro(Intro::LtEqU256 {
            lhs: try_rarg_from_dynamic(lhs)?,
            rhs: try_rarg_from_dynamic(rhs)?,
        }));
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

// #[derive(Debug)]
// struct Object {
//     io: ObjectIO,
//     class: String,
//     arg: RArg,
// }
// 
// impl fmt::Display for Object {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         write!(f, "{} {}:{}", self.io, self.arg.borrow(), self.class)
//     }
// }
// 
// impl Object {
//     fn new(class: String, io: ObjectIO, arg: RArg) -> Self {
//         Object { io, class, arg }
//     }
// }

#[derive(Clone, Debug)]
/// Argument to an Update/Set detail
pub struct Arg {
    value: Option<Value>,
    is_object: bool,
    var_name: Option<String>,
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.var_name {
            Some(name) => write!(f, "{name}"),
            None => write!(f, "{}", self.value.as_ref().expect("has value")),
        }
    }
}

impl Arg {
    fn literal(value: Value) -> Self {
        Self {
            value: Some(value),
            is_object: false,
            var_name: None,
        }
    }
    fn obj() -> Self {
        Self {
            value: None,
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
    fn update(self, key: String, value: Dynamic) -> RResult<()> {
        let arg = self.arg.borrow();
        if let Some(name) = &arg.var_name {
            self.ctx.0.borrow_mut().insts.push(Inst::Update {
                obj: name.clone(),
                key,
                value: try_rarg_from_dynamic(value)?,
            });
        }
        Ok(())
    }
}

fn try_value_from_dynamic(v: Dynamic) -> Result<Value, Dynamic> {
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
    let v = match try_value_from_dynamic(v) {
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

                // Push a new variable into the scope if it doesn't already exist.
                // Otherwise just set its value.
                if !ctx.scope().is_constant(&var_name).unwrap_or(false) {
                    arg_ctx.arg.borrow_mut().set_var_name(var_name.clone());
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
        .register_fn("intro_vdf", Context::intro_vdf)
        .register_fn("intro_lt_eq_u256", Context::intro_lt_eq_u256)
        .register_fn("pow_obj_grind", Context::pow_obj_grind)
        .register_type_with_name::<ArgContext>("ArgContext")
        .register_fn("set", ArgContext::set)
        .register_fn("update", ArgContext::update);

    let src = find_log_src;
    let mut scope = Scope::new();
    let ast = engine.compile_with_scope(&mut scope, src).unwrap();

    let ctx = Context::default();
    let _result = engine
        .call_fn::<Dynamic>(&mut scope, &ast, "FindLog", (ctx.clone(),))
        .unwrap();
    println!("FindLog:\n{}\n", ctx);

    let ctx = Context::default();
    let _result = engine
        .call_fn::<Dynamic>(&mut scope, &ast, "CraftWood", (ctx.clone(),))
        .unwrap();
    println!("FindLog:\n{}\n", ctx);
}
