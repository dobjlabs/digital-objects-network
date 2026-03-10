use std::collections::{HashMap, HashSet};

use hex::FromHex;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::{
    frontend::{MainPod, Operation},
    middleware::{F, Hash, Key, Pod, RawValue, Statement, Value},
};
use txlib::rand_raw_value;
use vdfpod::VdfPod;

use crate::sdk::{Context, api};

pub const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

#[derive(Debug, Clone, Copy)]
pub struct ActionUiMeta {
    pub emoji: &'static str,
    pub description: &'static str,
    pub cpu_cost: &'static str,
    pub reads_block: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ClassUiMeta {
    pub emoji: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone)]
pub struct ActionDescriptor {
    pub name: String,
    pub input_classes: Vec<String>,
    pub output_classes: Vec<String>,
    pub hidden: bool,
    pub ui: ActionUiMeta,
}

pub fn action_ui_meta(name: &str) -> ActionUiMeta {
    match name {
        "FindLog" => ActionUiMeta {
            emoji: "🌲",
            description: "Discover a log object by proving a short VDF.",
            cpu_cost: "20-40s",
            reads_block: true,
            hidden: false,
        },
        "CraftWood" => ActionUiMeta {
            emoji: "🪵",
            description: "Transform one Log into one Wood object.",
            cpu_cost: "20-40s",
            reads_block: false,
            hidden: false,
        },
        "CraftSticks" => ActionUiMeta {
            emoji: "🥢",
            description: "Split one Wood into two Stick objects.",
            cpu_cost: "10-20s",
            reads_block: false,
            hidden: false,
        },
        "CraftWoodPick" => ActionUiMeta {
            emoji: "⛏️",
            description: "Craft a WoodPick from Wood and Stick.",
            cpu_cost: "20-40s",
            reads_block: false,
            hidden: false,
        },
        "CraftStonePick" => ActionUiMeta {
            emoji: "🛠️",
            description: "Craft a StonePick from Stone and Stick.",
            cpu_cost: "20-40s",
            reads_block: false,
            hidden: false,
        },
        "UseWoodPick" => ActionUiMeta {
            emoji: "⛏️",
            description: "Internal: consume WoodPick durability and update work.",
            cpu_cost: "30-60s",
            reads_block: true,
            hidden: true,
        },
        "MineStoneWithWoodPick" => ActionUiMeta {
            emoji: "🪨",
            description: "Use a WoodPick to mutate the pick and output Stone.",
            cpu_cost: "45-90s",
            reads_block: true,
            hidden: false,
        },
        "UseStonePick" => ActionUiMeta {
            emoji: "🛠️",
            description: "Internal: consume StonePick durability and update work.",
            cpu_cost: "20-40s",
            reads_block: true,
            hidden: true,
        },
        "MineStoneWithStonePick" => ActionUiMeta {
            emoji: "🪨",
            description: "Use a StonePick to mutate the pick and output Stone.",
            cpu_cost: "35-70s",
            reads_block: true,
            hidden: false,
        },
        _ => ActionUiMeta {
            emoji: "⚙️",
            description: "SDK action",
            cpu_cost: "pending",
            reads_block: false,
            hidden: false,
        },
    }
}

pub fn class_ui_meta(name: &str) -> ClassUiMeta {
    match name {
        "Log" => ClassUiMeta {
            emoji: "🌲",
            description: "Raw log input discovered from VDF work.",
        },
        "Wood" => ClassUiMeta {
            emoji: "🪵",
            description: "Processed wood used for crafting.",
        },
        "Stick" => ClassUiMeta {
            emoji: "🥢",
            description: "Basic handle material for tools.",
        },
        "WoodPick" => ClassUiMeta {
            emoji: "⛏️",
            description: "Entry-level pickaxe with durability.",
        },
        "Stone" => ClassUiMeta {
            emoji: "🪨",
            description: "Mined stone resource.",
        },
        "StonePick" => ClassUiMeta {
            emoji: "🛠️",
            description: "Improved pickaxe with higher durability.",
        },
        _ => ClassUiMeta {
            emoji: "📦",
            description: "SDK class object.",
        },
    }
}

pub fn dependencies() -> Vec<api::Dependency> {
    vec![
        api::Dependency::Intro {
            pred: "Vdf(count, input, output)",
            hash: Hash::from_hex(
                "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
            )
            .unwrap(),
        },
        api::Dependency::Intro {
            pred: "LtEqU256(lhs, rhs)",
            hash: Hash::from_hex(
                "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529",
            )
            .unwrap(),
        },
    ]
}

fn main_pod(ctx: &Context, pod: Box<dyn Pod>) -> MainPod {
    let pub_statements = pod.pub_statements();
    MainPod {
        pod,
        public_statements: pub_statements,
        params: ctx.params.clone(),
    }
}

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

fn use_pick_details(name: &'static str, vdf_iters: usize) -> Vec<api::Detail> {
    use api::Detail::*;

    vec![
        Condition(
            "Gt({state}.durability, 0)",
            Box::new(|ctx| {
                let obj = ctx.vars.get_dict(name).unwrap();
                ctx.bld
                    .builder
                    .priv_op(Operation::gt((obj, "durability"), 0))
                    .unwrap()
            }),
        ),
        Var(
            "durability",
            Box::new(|ctx| {
                let obj = ctx.vars.get_dict(name).unwrap();
                let mut durability =
                    i64::try_from(obj.get(&Key::from("durability")).unwrap().typed()).unwrap();
                durability -= 1;
                ctx.store("durability", Box::new(durability));
                Value::from(durability)
            }),
        ),
        Condition(
            "SumOf({state}.durability, durability, 1)",
            Box::new(|ctx| {
                let durability: Box<i64> = ctx.take("durability");
                let obj = ctx.vars.get_dict(name).unwrap();
                ctx.bld
                    .builder
                    .priv_op(Operation::sum_of((obj, "durability"), *durability, 1))
                    .unwrap()
            }),
        ),
        Update("durability", api::Arg::Var("durability")),
        Var("key", Box::new(|_ctx| Value::from(rand_raw_value()))),
        Update("key", api::Arg::Var("key")),
        Var(
            "work",
            Box::new(move |ctx| {
                let obj = ctx.vars.get(name);
                let obj_raw = RawValue::from(obj);
                let (vdf_pod, st_vdf, work) = vdf(ctx, vdf_iters, obj_raw);
                ctx.store("vdf_pod", Box::new(vdf_pod));
                ctx.store("st_vdf", Box::new(st_vdf));
                work
            }),
        ),
        Condition(
            format!("Vdf({vdf_iters}, {{state}}, work)").leak(),
            Box::new(|ctx| {
                let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
                let st_vdf: Box<Statement> = ctx.take("st_vdf");
                ctx.bld.builder.add_pod(*vdf_pod).unwrap();
                *st_vdf
            }),
        ),
        Update("work", api::Arg::Var("work")),
    ]
}

pub fn actions() -> Vec<api::Action> {
    use api::Arg;
    use api::Detail::*;
    use api::Step::*;

    let find_log = api::Action {
        name: "FindLog",
        steps: vec![Output {
            name: "log",
            class: "Log",
            details: vec![
                Set("blueprint", Arg::Literal(Value::from("Log"))),
                Var(
                    "work",
                    Box::new(|ctx| {
                        let log = ctx.vars.get("log");
                        let log_raw = RawValue::from(log);
                        let (vdf_pod, st_vdf, work) = vdf(ctx, 3, log_raw);
                        ctx.store("vdf_pod", Box::new(vdf_pod));
                        ctx.store("st_vdf", Box::new(st_vdf));
                        work
                    }),
                ),
                Condition(
                    "Vdf(3, {state}, work)",
                    Box::new(|ctx| {
                        let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
                        let st_vdf: Box<Statement> = ctx.take("st_vdf");
                        ctx.bld.builder.add_pod(*vdf_pod).unwrap();
                        *st_vdf
                    }),
                ),
                Update("work", Arg::Var("work")),
            ],
        }],
    };

    let craft_wood = api::Action {
        name: "CraftWood",
        steps: vec![
            Input {
                name: "log",
                class: "Log",
                details: vec![],
            },
            Output {
                name: "wood",
                class: "Wood",
                details: vec![
                    Set("blueprint", Arg::Literal(Value::from("Wood"))),
                    Var(
                        "key",
                        Box::new(|ctx| {
                            let mut wood = ctx.vars.get_dict("wood").unwrap().clone();
                            let mut key = Value::from(rand_raw_value());
                            if !ctx.mock {
                                while RawValue::from(wood.commitment()).0[3].0 > WOOD_POW_DIFFICULTY
                                {
                                    key = Value::from(rand_raw_value());
                                    wood.update(&Key::from("key"), &key).unwrap();
                                }
                            }
                            key
                        }),
                    ),
                    Update("key", Arg::Var("key")),
                    Condition(
                        "LtEqU256({state}, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))",
                        Box::new(|ctx| {
                            let wood = ctx.vars.get("wood");
                            let wood_raw = RawValue::from(wood);
                            let (lt_eq_u256_pod, st_lt_eq_u256) = lt_eq_u256(
                                ctx,
                                wood_raw,
                                RawValue([F(0), F(0), F(0), F(WOOD_POW_DIFFICULTY)]),
                            );
                            ctx.bld.builder.add_pod(lt_eq_u256_pod).unwrap();
                            st_lt_eq_u256
                        }),
                    ),
                ],
            },
        ],
    };

    let craft_sticks = api::Action {
        name: "CraftSticks",
        steps: vec![
            Input {
                name: "wood",
                class: "Wood",
                details: vec![],
            },
            Output {
                name: "stick_a",
                class: "Stick",
                details: vec![Set("blueprint", Arg::Literal(Value::from("Stick")))],
            },
            Output {
                name: "stick_b",
                class: "Stick",
                details: vec![Set("blueprint", Arg::Literal(Value::from("Stick")))],
            },
        ],
    };

    let craft_wood_pick = api::Action {
        name: "CraftWoodPick",
        steps: vec![
            Input {
                name: "wood",
                class: "Wood",
                details: vec![],
            },
            Input {
                name: "stick",
                class: "Stick",
                details: vec![],
            },
            Output {
                name: "wood_pick",
                class: "WoodPick",
                details: vec![
                    Set("blueprint", Arg::Literal(Value::from("WoodPick"))),
                    Set("durability", Arg::Literal(Value::from(100i64))),
                ],
            },
        ],
    };

    let craft_stone_pick = api::Action {
        name: "CraftStonePick",
        steps: vec![
            Input {
                name: "stone",
                class: "Stone",
                details: vec![],
            },
            Input {
                name: "stick",
                class: "Stick",
                details: vec![],
            },
            Output {
                name: "stone_pick",
                class: "StonePick",
                details: vec![
                    Set("blueprint", Arg::Literal(Value::from("StonePick"))),
                    Set("durability", Arg::Literal(Value::from(200i64))),
                ],
            },
        ],
    };

    let use_wood_pick = api::Action {
        name: "UseWoodPick",
        steps: vec![Mutate {
            name: "wood_pick",
            class: "WoodPick",
            details: use_pick_details("wood_pick", 10),
        }],
    };

    let mine_stone_with_wood_pick = api::Action {
        name: "MineStoneWithWoodPick",
        steps: vec![
            Depend {
                name: "pick",
                action: "UseWoodPick",
            },
            Output {
                name: "stone",
                class: "Stone",
                details: vec![Set("blueprint", Arg::Literal(Value::from("Stone")))],
            },
        ],
    };

    let use_stone_pick = api::Action {
        name: "UseStonePick",
        steps: vec![Mutate {
            name: "stone_pick",
            class: "StonePick",
            details: use_pick_details("stone_pick", 5),
        }],
    };

    let mine_stone_with_stone_pick = api::Action {
        name: "MineStoneWithStonePick",
        steps: vec![
            Depend {
                name: "pick",
                action: "UseStonePick",
            },
            Output {
                name: "stone",
                class: "Stone",
                details: vec![Set("blueprint", Arg::Literal(Value::from("Stone")))],
            },
        ],
    };

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
    ]
}

fn flatten_inputs(
    action_name: &str,
    by_name: &HashMap<&str, &api::Action>,
    stack: &mut Vec<String>,
) -> Vec<String> {
    if stack.iter().any(|entry| entry == action_name) {
        panic!("cyclic dependency detected while flattening inputs for {action_name}");
    }
    stack.push(action_name.to_string());

    let action = by_name.get(action_name).unwrap();
    let mut classes = Vec::new();
    for step in &action.steps {
        match step {
            api::Step::Input { class, .. } | api::Step::Mutate { class, .. } => {
                classes.push((*class).to_string())
            }
            api::Step::Depend { action, .. } => {
                classes.extend(flatten_inputs(action, by_name, stack));
            }
            api::Step::Output { .. } => {}
        }
    }

    stack.pop();
    classes
}

fn flatten_outputs(
    action_name: &str,
    by_name: &HashMap<&str, &api::Action>,
    stack: &mut Vec<String>,
) -> Vec<String> {
    if stack.iter().any(|entry| entry == action_name) {
        panic!("cyclic dependency detected while flattening outputs for {action_name}");
    }
    stack.push(action_name.to_string());

    let action = by_name.get(action_name).unwrap();
    let mut classes = Vec::new();
    for step in &action.steps {
        match step {
            api::Step::Output { class, .. } | api::Step::Mutate { class, .. } => {
                classes.push((*class).to_string())
            }
            api::Step::Depend { action, .. } => {
                classes.extend(flatten_outputs(action, by_name, stack));
            }
            api::Step::Input { .. } => {}
        }
    }

    stack.pop();
    classes
}

pub fn action_descriptors() -> Vec<ActionDescriptor> {
    let api_actions = actions();
    let by_name: HashMap<&str, &api::Action> = api_actions
        .iter()
        .map(|action| (action.name, action))
        .collect();

    let dependency_targets: HashSet<&str> = api_actions
        .iter()
        .flat_map(|action| action.steps.iter())
        .filter_map(|step| match step {
            api::Step::Depend { action, .. } => Some(*action),
            _ => None,
        })
        .collect();

    api_actions
        .iter()
        .map(|action| {
            let ui = action_ui_meta(action.name);
            ActionDescriptor {
                name: action.name.to_string(),
                input_classes: flatten_inputs(action.name, &by_name, &mut Vec::new()),
                output_classes: flatten_outputs(action.name, &by_name, &mut Vec::new()),
                hidden: ui.hidden || dependency_targets.contains(action.name),
                ui,
            }
        })
        .collect()
}
