//! Recipe interpreter: converts plugin metadata into `craft_sdk::api::Action`
//! values with real proof-generation closures.
//!
//! Each `VarRecipe` / `ConditionRecipe` variant maps to a built-in handler that
//! knows how to invoke the corresponding native proof machinery (VdfPod,
//! LtEqU256Pod, builder operations, etc.).

use craft_sdk::{
    api::{self, Arg, Step},
    Context,
};
use lt_eq_u256_pod::LtEqU256Pod;
use plugin_api::*;
use pod2::{
    frontend::{MainPod, Operation},
    middleware::{Key, Pod, RawValue, Statement, Value, F},
};
use pod2utils::rand_raw_value;
use vdfpod::VdfPod;

/// Convert all actions in plugin metadata into `craft_sdk::api::Action` values.
pub(crate) fn metadata_to_actions(meta: &PluginMetadata) -> Vec<api::Action> {
    meta.actions
        .iter()
        .map(|action_meta| api::Action {
            name: action_meta.name.clone().leak(),
            steps: action_meta
                .steps
                .iter()
                .map(|step_meta| build_step(step_meta))
                .collect(),
        })
        .collect()
}

fn build_step(step_meta: &StepMeta) -> Step {
    let mut step = match step_meta.kind {
        StepKindMeta::Input => Step::input(&step_meta.name, &step_meta.class),
        StepKindMeta::Output => Step::output(&step_meta.name, &step_meta.class),
        StepKindMeta::Mutate => Step::mutate(&step_meta.name, &step_meta.class),
        StepKindMeta::Depends => Step::depends(&step_meta.name, &step_meta.action),
    };

    for detail in &step_meta.details {
        step = apply_detail(step, detail, &step_meta.name);
    }

    step
}

fn apply_detail(step: Step, detail: &DetailMeta, step_name: &str) -> Step {
    match detail {
        DetailMeta::Set { key, value } => step.set(key, Arg::literal(literal_to_value(value))),
        DetailMeta::Update { key, source } => step.update(key, Arg::var(source)),
        DetailMeta::Var { name, recipe } => {
            let closure = var_closure(recipe, step_name);
            step.var(name, closure)
        }
        DetailMeta::Condition { pred, recipe } => {
            let closure = condition_closure(recipe, step_name);
            step.condition(pred.clone().leak(), closure)
        }
    }
}

fn literal_to_value(lit: &LiteralValue) -> Value {
    match lit {
        LiteralValue::Int(n) => Value::from(*n),
        LiteralValue::Str(s) => Value::from(s.as_str()),
    }
}

// ---------------------------------------------------------------------------
// Var recipe handlers
// ---------------------------------------------------------------------------

fn var_closure(
    recipe: &VarRecipe,
    step_name: &str,
) -> Box<dyn Fn(&mut Context<'_>) -> Value> {
    match recipe {
        VarRecipe::Vdf { iters } => {
            let iters = *iters;
            let obj_name = step_name.to_string();
            Box::new(move |ctx| {
                let obj = ctx.vars.get(&obj_name);
                let obj_raw = obj.as_raw();
                let (vdf_pod, st_vdf, work) = run_vdf(ctx, iters, obj_raw);
                ctx.store("vdf_pod", Box::new(vdf_pod));
                ctx.store("st_vdf", Box::new(st_vdf));
                work
            })
        }
        VarRecipe::PowGrind { difficulty } => {
            let difficulty = *difficulty;
            let obj_name = step_name.to_string();
            Box::new(move |ctx| {
                let mut obj = ctx.vars.get(&obj_name).as_dictionary().unwrap();
                let mut key = Value::from(rand_raw_value());
                if !ctx.mock {
                    while RawValue::from(obj.commitment()).0[3].0 > difficulty {
                        key = Value::from(rand_raw_value());
                        obj.update(&Key::from("key"), &key).unwrap();
                    }
                }
                key
            })
        }
        VarRecipe::DecrementField { key } => {
            let key_static: &'static str = key.clone().leak();
            let obj_name = step_name.to_string();
            Box::new(move |ctx| {
                let obj = ctx.vars.get(&obj_name).as_dictionary().unwrap();
                let mut val = obj
                    .get(&Key::from(key_static))
                    .unwrap()
                    .unwrap()
                    .as_int()
                    .unwrap();
                val -= 1;
                ctx.store(key_static, Box::new(val));
                Value::from(val)
            })
        }
        VarRecipe::RandomKey => Box::new(|_ctx| Value::from(rand_raw_value())),
    }
}

// ---------------------------------------------------------------------------
// Condition recipe handlers
// ---------------------------------------------------------------------------

fn condition_closure(
    recipe: &ConditionRecipe,
    step_name: &str,
) -> Box<dyn Fn(&mut Context<'_>) -> Statement> {
    match recipe {
        ConditionRecipe::StoredVdfPod => Box::new(|ctx| {
            let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
            let st_vdf: Box<Statement> = ctx.take("st_vdf");
            ctx.bld.builder.add_pod(*vdf_pod).unwrap();
            *st_vdf
        }),
        ConditionRecipe::LtEqU256 { difficulty } => {
            let difficulty = *difficulty;
            let obj_name = step_name.to_string();
            Box::new(move |ctx| {
                let obj = ctx.vars.get(&obj_name);
                let obj_raw = obj.as_raw();
                let (lt_pod, st) = run_lt_eq_u256(
                    ctx,
                    obj_raw,
                    RawValue([F(0), F(0), F(0), F(difficulty)]),
                );
                ctx.bld.builder.add_pod(lt_pod).unwrap();
                st
            })
        }
        ConditionRecipe::Gt { key, value } => {
            let key_name = key.clone();
            let val = *value;
            let obj_name = step_name.to_string();
            Box::new(move |ctx| {
                let obj = ctx.vars.get(&obj_name).as_dictionary().unwrap();
                ctx.bld
                    .builder
                    .priv_op(Operation::gt((&obj, key_name.as_str()), val))
                    .unwrap()
            })
        }
        ConditionRecipe::SumOf {
            key,
            stored_var,
            b,
        } => {
            let key_name = key.clone();
            let var_static: &'static str = stored_var.clone().leak();
            let b_val = *b;
            let obj_name = step_name.to_string();
            Box::new(move |ctx| {
                let stored: Box<i64> = ctx.take(var_static);
                let obj = ctx.vars.get(&obj_name).as_dictionary().unwrap();
                ctx.bld
                    .builder
                    .priv_op(Operation::sum_of(
                        (&obj, key_name.as_str()),
                        *stored,
                        b_val,
                    ))
                    .unwrap()
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Native proof helpers (same logic as the original spec.rs)
// ---------------------------------------------------------------------------

fn main_pod(ctx: &Context<'_>, pod: Box<dyn Pod>) -> MainPod {
    let pub_statements = pod.pub_statements();
    MainPod {
        pod,
        public_statements: pub_statements,
        params: ctx.params.clone(),
    }
}

fn run_vdf(
    ctx: &mut Context<'_>,
    n_iters: usize,
    input: RawValue,
) -> (MainPod, Statement, Value) {
    let vdf_pod = if ctx.mock {
        VdfPod::new_boxed_mock(&ctx.params, ctx.vd_set.clone(), n_iters, input)
    } else {
        VdfPod::new_boxed(&ctx.params, ctx.vd_set.clone(), n_iters, input)
    }
    .unwrap();
    let st_vdf = vdf_pod.pub_statements()[0].clone();
    let work = st_vdf.args()[2].literal().unwrap();
    (main_pod(ctx, vdf_pod), st_vdf, work)
}

fn run_lt_eq_u256(
    ctx: &mut Context<'_>,
    lhs: RawValue,
    rhs: RawValue,
) -> (MainPod, Statement) {
    let lt_pod = if ctx.mock {
        LtEqU256Pod::new_boxed_mock(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
    } else {
        LtEqU256Pod::new_boxed(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
    }
    .unwrap();
    let st = lt_pod.pub_statements()[0].clone();
    (main_pod(ctx, lt_pod), st)
}
