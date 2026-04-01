use std::sync::Arc;

use common::test_state::TestState;
use hex::FromHex;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::{
    frontend::{MainPod, Operation},
    middleware::{Hash, Key, Pod, RawValue, Statement, Value, F},
};
use pod2utils::rand_raw_value;
use txlib::{GroundingWitness, StateRoot, Tx};
use vdfpod::VdfPod;

use super::{api::*, Context, Helper};

const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

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

fn main_pod(ctx: &Context, pod: Box<dyn Pod>) -> MainPod {
    let pub_statements = pod.pub_statements();
    MainPod {
        pod,
        public_statements: pub_statements,
        params: ctx.params.clone(),
    }
}

// Returns VdfPod, Vdf statement, work
fn vdf(ctx: &mut Context, n_iters: usize, input: RawValue) -> (MainPod, Statement, Value) {
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

// Returns LtEqU256Pod and LtEqU256 statement used to verify PoW.
fn lt_eq_u256(ctx: &mut Context, lhs: RawValue, rhs: RawValue) -> (MainPod, Statement) {
    let lt_eq_u256_pod = if ctx.mock {
        LtEqU256Pod::new_boxed_mock(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
    } else {
        LtEqU256Pod::new_boxed(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
    }
    .unwrap();
    let st_lt_eq_u256 = lt_eq_u256_pod.pub_statements()[0].clone();
    (main_pod(ctx, lt_eq_u256_pod), st_lt_eq_u256)
}

#[test]
fn test_sdk() {
    let _ = env_logger::builder().try_init();

    let find_log = Action {
        name: "FindLog",
        steps: vec![Step::output("log", "Log")
            .set("blueprint", Arg::literal("Log"))
            .var(
                "work",
                Box::new(|ctx| {
                    let log = ctx.vars.get("log");
                    let log_raw = log.as_raw();
                    let (vdf_pod, st_vdf, work) = vdf(ctx, 3, log_raw);
                    ctx.store("vdf_pod", Box::new(vdf_pod));
                    ctx.store("st_vdf", Box::new(st_vdf));
                    work
                }),
            )
            .condition(
                "Vdf(3, {state}, work)",
                Box::new(|ctx| {
                    let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
                    let st_vdf: Box<Statement> = ctx.take("st_vdf");
                    ctx.bld.builder.add_pod(*vdf_pod).unwrap();
                    *st_vdf
                }),
            )
            .update("work", Arg::var("work"))],
    };

    let craft_wood = Action {
            name: "CraftWood",
            steps: vec![
                Step::input("log", "Log"),
                Step::output("wood", "Wood")
                    .set("blueprint", Arg::literal("Wood"))
                    .var(
                        "key",
                        Box::new(|ctx| {
                            let mut wood = ctx.vars.get("wood").as_dictionary().unwrap();
                            let mut key = Value::from(rand_raw_value());
                            if !ctx.mock {
                                while RawValue::from(wood.commitment()).0[3].0
                                    > WOOD_POW_DIFFICULTY
                                {
                                    key = Value::from(rand_raw_value());

                                    wood.update(&Key::from("key"), &key).unwrap();
                                }
                            }
                            key
                        }),
                    )
                    .update("key", Arg::var("key"))
                    .condition(
                        "LtEqU256({state}, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))",
                        Box::new(|ctx| {
                            let wood = ctx.vars.get("wood");
                            let wood_raw = wood.as_raw();
                            let (lt_eq_u256_pod, st_lt_eq_u256) = lt_eq_u256(
                                ctx,
                                wood_raw,
                                RawValue([F(0), F(0), F(0), F(WOOD_POW_DIFFICULTY)]),
                            );
                            ctx.bld.builder.add_pod(lt_eq_u256_pod).unwrap();
                            st_lt_eq_u256
                        }),
                    )
                ]
        };

    let craft_sticks = Action {
        name: "CraftSticks",
        steps: vec![
            Step::input("wood", "Wood"),
            Step::output("stick_a", "Stick").set("blueprint", Arg::literal("Stick")),
            Step::output("stick_b", "Stick").set("blueprint", Arg::literal("Stick")),
        ],
    };

    let craft_wood_pick = Action {
        name: "CraftWoodPick",
        steps: vec![
            Step::input("wood", "Wood"),
            Step::input("stick", "Stick"),
            Step::output("wood_pick", "WoodPick")
                .set("blueprint", Arg::literal("WoodPick"))
                .set("durability", Arg::literal(100i64)),
        ],
    };

    let craft_stone_pick = Action {
        name: "CraftStonePick",
        steps: vec![
            Step::input("stone", "Stone"),
            Step::input("stick", "Stick"),
            Step::output("stone_pick", "StonePick")
                .set("blueprint", Arg::literal("StonePick"))
                .set("durability", Arg::literal(200i64)),
        ],
    };

    fn use_pick_details(step: Step, name: &'static str, vdf_iters: usize) -> Step {
        step.condition(
            "Gt({state}.durability, 0)",
            Box::new(|ctx| {
                let obj = ctx.vars.get(name).as_dictionary().unwrap();
                ctx.bld
                    .builder
                    .priv_op(Operation::gt((&obj, "durability"), 0))
                    .unwrap()
            }),
        )
        .var(
            "durability",
            Box::new(|ctx| {
                let obj = ctx.vars.get(name).as_dictionary().unwrap();
                let mut durability = obj
                    .get(&Key::from("durability"))
                    .unwrap()
                    .unwrap()
                    .as_int()
                    .unwrap();
                durability -= 1;
                ctx.store("durability", Box::new(durability));
                Value::from(durability)
            }),
        )
        .condition(
            "SumOf({state}.durability, durability, 1)",
            Box::new(|ctx| {
                let durability: Box<i64> = ctx.take("durability");
                let obj = ctx.vars.get(name).as_dictionary().unwrap();
                ctx.bld
                    .builder
                    .priv_op(Operation::sum_of((&obj, "durability"), *durability, 1))
                    .unwrap()
            }),
        )
        .update("durability", Arg::var("durability"))
        .var("key", Box::new(|_ctx| Value::from(rand_raw_value())))
        .update("key", Arg::var("key"))
        .var(
            "work",
            Box::new(move |ctx| {
                let obj = ctx.vars.get(name);
                let obj_raw = obj.as_raw();
                let (vdf_pod, st_vdf, work) = vdf(ctx, vdf_iters, obj_raw);
                ctx.store("vdf_pod", Box::new(vdf_pod));
                ctx.store("st_vdf", Box::new(st_vdf));
                work
            }),
        )
        .condition(
            format!("Vdf({vdf_iters}, {{state}}, work)").leak(),
            Box::new(|ctx| {
                let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
                let st_vdf: Box<Statement> = ctx.take("st_vdf");
                ctx.bld.builder.add_pod(*vdf_pod).unwrap();
                *st_vdf
            }),
        )
        .update("work", Arg::var("work"))
    }

    let use_wood_pick = Action {
        name: "UseWoodPick",
        steps: vec![Step::mutate("wood_pick", "WoodPick")
            .snippet(|step| use_pick_details(step, "wood_pick", 10))],
    };

    let mine_stone_with_wood_pick = Action {
        name: "MineStoneWithWoodPick",
        steps: vec![
            Step::depends("pick", "UseWoodPick"),
            Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")),
        ],
    };

    let use_stone_pick = Action {
        name: "UseStonePick",
        steps: vec![Step::mutate("stone_pick", "StonePick")
            .snippet(|step| use_pick_details(step, "stone_pick", 5))],
    };

    let mine_stone_with_stone_pick = Action {
        name: "MineStoneWithStonePick",
        steps: vec![
            Step::depends("pick", "UseStonePick"),
            Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")),
        ],
    };

    let dependencies = vec![
        Dependency::Intro {
            pred: "Vdf(count, input, output)",
            hash: Hash::from_hex(
                "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
            )
            .unwrap(),
        },
        Dependency::Intro {
            pred: "LtEqU256(lhs, rhs)",
            hash: Hash::from_hex(
                "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529",
            )
            .unwrap(),
        },
    ];

    let helper = Helper::new(
        dependencies,
        vec![
            find_log,
            craft_wood,
            craft_sticks,
            craft_wood_pick,
            craft_stone_pick,
            use_wood_pick,
            mine_stone_with_wood_pick,
            use_stone_pick,
            mine_stone_with_stone_pick,
        ],
    );
    println!("{}", helper.podlang_src);

    let mut state = TestState::default();

    let mock = true;

    let builder = helper.builder(mock, grounding_witness(&state, &[]));
    let [log_a] = builder.action("FindLog", vec![]).objs();
    apply_tx(&mut state, &log_a.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[log_a.tx.clone()]));
    let [wood_a] = builder.action("CraftWood", vec![log_a]).objs();
    apply_tx(&mut state, &wood_a.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[wood_a.tx.clone()]));
    let [stick_a, stick_b] = builder.action("CraftSticks", vec![wood_a]).objs();
    apply_tx(&mut state, &stick_a.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[]));
    let [log_b] = builder.action("FindLog", vec![]).objs();
    apply_tx(&mut state, &log_b.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[log_b.tx.clone()]));
    let [wood_b] = builder.action("CraftWood", vec![log_b]).objs();
    apply_tx(&mut state, &wood_b.tx);

    let builder = helper.builder(
        mock,
        grounding_witness(&state, &[wood_b.tx.clone(), stick_a.tx.clone()]),
    );
    let [wood_pick] = builder
        .action("CraftWoodPick", vec![wood_b, stick_a])
        .objs();
    apply_tx(&mut state, &wood_pick.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[wood_pick.tx.clone()]));
    let [wood_pick, stone_a] = builder
        .action("MineStoneWithWoodPick", vec![wood_pick])
        .objs();
    apply_tx(&mut state, &wood_pick.tx);

    let builder = helper.builder(
        mock,
        grounding_witness(&state, &[stone_a.tx.clone(), stick_b.tx.clone()]),
    );
    let [stone_pick] = builder
        .action("CraftStonePick", vec![stone_a, stick_b])
        .objs();
    apply_tx(&mut state, &stone_pick.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[stone_pick.tx.clone()]));
    let [stone_pick, _stone_b] = builder
        .action("MineStoneWithStonePick", vec![stone_pick])
        .objs();
    apply_tx(&mut state, &stone_pick.tx);

    let builder = helper.builder(mock, grounding_witness(&state, &[stone_pick.tx.clone()]));
    let [stone_pick, _stone_c] = builder
        .action("MineStoneWithStonePick", vec![stone_pick])
        .objs();
    apply_tx(&mut state, &stone_pick.tx);
}

use crate::api;
use pod2::middleware::containers::Dictionary;
use pod2utils::dict;
use rhai::{Engine, EvalAltResult, Scope};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default, Clone)]
pub struct Vars {
    vars: HashMap<String, Value>,
}

impl Vars {
    pub(crate) fn insert(&mut self, name: String, value: Value) {
        self.vars.insert(name, value);
    }
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        self.vars.get_mut(name).map(|v| *v = value).is_some()
    }
    pub fn get(&self, name: &str) -> Option<Value> {
        self.vars.get(name).map(|v| v.clone())
    }
    pub(crate) fn mut_dict<T>(&mut self, name: &str, mut f: impl FnMut(&mut Dictionary) -> T) -> T {
        let obj = self.vars.get_mut(name).unwrap();
        let mut dict = obj.as_dictionary().unwrap();
        let output = f(&mut dict);
        *obj = Value::from(dict);
        output
    }
    // Resolve a Arg::Var or return its Literal value
    pub fn value(&self, arg: &api::Arg) -> Value {
        match arg {
            api::Arg::Literal(v) => v.clone(),
            api::Arg::Var(name) => self.vars[name.as_str()].clone(),
        }
    }
}

type EvalResult<T> = Result<T, Box<EvalAltResult>>;

#[test]
fn test_rhai() {
    // Box::new(|ctx| {
    //             let obj = ctx.vars.get(name).as_dictionary().unwrap();
    //             let mut durability = obj
    //                 .get(&Key::from("durability"))
    //                 .unwrap()
    //                 .unwrap()
    //                 .as_int()
    //                 .unwrap();
    //             durability -= 1;
    //             ctx.store("durability", Box::new(durability));
    //             Value::from(durability)
    //         })
    let mut engine = Engine::new();

    let vars = Rc::new(RefCell::new(Vars::default()));

    engine
        .register_fn("vars_get", {
            let vars = vars.clone();
            move |name: &str| -> EvalResult<Value> {
                vars.borrow().get(name).ok_or_else(|| "not found".into())
            }
        })
        .register_fn("vars_set", {
            let vars = vars.clone();
            move |name: &str, v: Value| -> EvalResult<()> {
                vars.borrow_mut()
                    .set(name, v)
                    .then_some(())
                    .ok_or_else(|| "not found".into())
            }
        })
        .register_type_with_name::<Value>("Value")
        .register_fn("value_from", |v: i64| Value::from(v))
        .register_fn("value_from", |v: String| Value::from(v))
        .register_fn("as_int", |v: Value| -> EvalResult<i64> {
            v.as_int().ok_or_else(|| "not an int".into())
        })
        .register_fn("as_dict", |v: Value| -> EvalResult<Dictionary> {
            v.as_dictionary().ok_or_else(|| "not a dict".into())
        })
        .register_type_with_name::<Dictionary>("Dictionary")
        .register_fn("get", |d: Dictionary, key: &str| -> EvalResult<Value> {
            d.get(&Key::from(key))
                .map_err(|e| EvalAltResult::from(format!("{e}")))?
                .ok_or_else(|| "not found".into())
        });

    vars.borrow_mut()
        .insert("pick".into(), Value::from(dict!({"durability" => 100})));

    let mut scope = Scope::new();
    let src = r#"
        let obj = vars_get("pick").as_dict();
        let durability = obj.get("durability").as_int();
        durability -= 1;
        value_from(durability)
        "#;
    let ast = engine.compile_with_scope(&mut scope, src).unwrap();
    let result = engine
        .eval_ast_with_scope::<Value>(&mut scope, &ast)
        .unwrap();
    println!("Result: {result}");

    // Scripting API
    // ```
    // value_from(i64) -> Value
    // value_from(PublicKey) -> Value
    // value_from(SecretKey) -> Value
    // value_from(Predicate) -> Value
    // value_from(Set) -> Value
    // value_from(Dictionary) -> Value
    // value_from(Array) -> Value
    // value_from(String) -> Value
    //
    // Value::as_int(self) -> Result<i64>
    // Value::as_public_key(self) -> Result<PublicKey>
    // Value::as_secret_key(self) -> Result<SecretKey>
    // Value::as_predicate(self) -> Result<Predicate>
    // Value::as_set(self) -> Result<Set>
    // Value::as_dict(self) -> Result<Dictionary>
    // Value::as_array(self) -> Result<Array>
    // Value::as_string(self) -> Result<String>
    // Value::as_bool(self) -> Result<bool>
    // Value::as_raw(self) -> RawValue
    //
    // // TODO: RawValue methods
    //
    // new_dict() -> Dictionary
    // Dictionary::get(&str) -> Result<Value>
    // Dictionary::insert(String, Value) -> Result<()>
    // Dictionary::update(&str, Value) -> Result<()>
    // Dictionary::delete(&str) -> Result<()>
    //
    // new_set() -> Set
    // Set::contains(&str) -> Result<bool>
    // Set::insert(String, Value) -> Result<()>
    // Set::delete(&str) -> Result<()>
    //
    // new_array() -> Array
    // Array::get(usize) -> Result<Value>
    // Array::insert(usize, Value) -> Result<()>
    // Array::update(usize, Value) -> Result<()>
    // Array::delete(usize) -> Result<()>
    //
    // vars_get(&str) -> Result<Value>
    // vars_set(&str, Value) -> Result<()>
    //
    // intro_vdf(usize, Value) -> (Statement, Value)
    // intro_lt_eq_u256(Value, Value) -> Statement
    //
    // builder_priv_op(String, [args])
    // ```
}
