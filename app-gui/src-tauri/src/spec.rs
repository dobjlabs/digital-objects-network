#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap, HashSet};

use craft_sdk::{
    api::{self, Arg, Step, StepKind},
    Context,
};
use hex::FromHex;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::{
    frontend::{MainPod, Operation},
    middleware::{Hash, Key, Pod, RawValue, Statement, Value, F},
};
use pod2utils::rand_raw_value;
use vdfpod::VdfPod;

const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

#[derive(Debug, Clone)]
pub(crate) struct ActionUiMeta {
    pub(crate) emoji: &'static str,
    pub(crate) description: &'static str,
    pub(crate) cpu_cost: &'static str,
    pub(crate) reads_block: bool,
    pub(crate) hidden: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ClassUiMeta {
    pub(crate) emoji: &'static str,
    pub(crate) description: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct ActionDescriptor {
    pub(crate) name: String,
    pub(crate) input_classes: Vec<String>,
    pub(crate) output_classes: Vec<String>,
    pub(crate) ui: ActionUiMeta,
    pub(crate) hidden: bool,
}

struct ActionMetaSpec {
    name: &'static str,
    ui: ActionUiMeta,
}

struct ClassMetaSpec {
    name: &'static str,
    ui: ClassUiMeta,
}

#[derive(Debug, Clone)]
struct ActionSignature {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

const ACTION_META: &[ActionMetaSpec] = &[
    ActionMetaSpec {
        name: "FindLog",
        ui: ActionUiMeta {
            emoji: "🌲",
            description: "Discover a log object by proving a short VDF.",
            cpu_cost: "20-40s",
            reads_block: false,
            hidden: false,
        },
    },
    ActionMetaSpec {
        name: "CraftWood",
        ui: ActionUiMeta {
            emoji: "🪵",
            description: "Refine one log into a wood object with PoW quality checks.",
            cpu_cost: "15-30s",
            reads_block: false,
            hidden: false,
        },
    },
    ActionMetaSpec {
        name: "CraftSticks",
        ui: ActionUiMeta {
            emoji: "🥢",
            description: "Split one wood object into two stick objects.",
            cpu_cost: "5-10s",
            reads_block: false,
            hidden: false,
        },
    },
    ActionMetaSpec {
        name: "CraftWoodPick",
        ui: ActionUiMeta {
            emoji: "⛏️",
            description: "Combine wood and a stick to craft a wood pick.",
            cpu_cost: "10-20s",
            reads_block: false,
            hidden: false,
        },
    },
    ActionMetaSpec {
        name: "CraftStonePick",
        ui: ActionUiMeta {
            emoji: "⛏️",
            description: "Combine stone and a stick to craft a stronger stone pick.",
            cpu_cost: "10-20s",
            reads_block: false,
            hidden: false,
        },
    },
    ActionMetaSpec {
        name: "UseWoodPick",
        ui: ActionUiMeta {
            emoji: "⛏️",
            description: "Internal durability/work update for wood pick usage.",
            cpu_cost: "10-30s",
            reads_block: false,
            hidden: true,
        },
    },
    ActionMetaSpec {
        name: "MineStoneWithWoodPick",
        ui: ActionUiMeta {
            emoji: "🪨",
            description: "Mine stone using a wood pick (consumes durability).",
            cpu_cost: "25-45s",
            reads_block: false,
            hidden: false,
        },
    },
    ActionMetaSpec {
        name: "UseStonePick",
        ui: ActionUiMeta {
            emoji: "⛏️",
            description: "Internal durability/work update for stone pick usage.",
            cpu_cost: "5-20s",
            reads_block: false,
            hidden: true,
        },
    },
    ActionMetaSpec {
        name: "MineStoneWithStonePick",
        ui: ActionUiMeta {
            emoji: "🪨",
            description: "Mine stone using a stone pick (consumes durability).",
            cpu_cost: "15-35s",
            reads_block: false,
            hidden: false,
        },
    },
];

const CLASS_META: &[ClassMetaSpec] = &[
    ClassMetaSpec {
        name: "Log",
        ui: ClassUiMeta {
            emoji: "🌲",
            description: "A discovered log that can be refined into wood.",
        },
    },
    ClassMetaSpec {
        name: "Wood",
        ui: ClassUiMeta {
            emoji: "🪵",
            description: "Refined wood used for sticks and basic tools.",
        },
    },
    ClassMetaSpec {
        name: "Stick",
        ui: ClassUiMeta {
            emoji: "🥢",
            description: "A stick used as a handle in tool crafting.",
        },
    },
    ClassMetaSpec {
        name: "WoodPick",
        ui: ClassUiMeta {
            emoji: "⛏️",
            description: "A wood pick that can mine stone while durability remains.",
        },
    },
    ClassMetaSpec {
        name: "Stone",
        ui: ClassUiMeta {
            emoji: "🪨",
            description: "Mined stone used to craft stronger tools.",
        },
    },
    ClassMetaSpec {
        name: "StonePick",
        ui: ClassUiMeta {
            emoji: "⛏️",
            description: "A sturdier pick with higher starting durability.",
        },
    },
];

fn main_pod(ctx: &Context<'_>, pod: Box<dyn Pod>) -> MainPod {
    let pub_statements = pod.pub_statements();
    MainPod {
        pod,
        public_statements: pub_statements,
        params: ctx.params.clone(),
    }
}

// Returns VdfPod, Vdf statement, work.
fn vdf(ctx: &mut Context<'_>, n_iters: usize, input: RawValue) -> (MainPod, Statement, Value) {
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
fn lt_eq_u256(ctx: &mut Context<'_>, lhs: RawValue, rhs: RawValue) -> (MainPod, Statement) {
    let lt_eq_u256_pod = if ctx.mock {
        LtEqU256Pod::new_boxed_mock(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
    } else {
        LtEqU256Pod::new_boxed(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
    }
    .unwrap();
    let st_lt_eq_u256 = lt_eq_u256_pod.pub_statements()[0].clone();
    (main_pod(ctx, lt_eq_u256_pod), st_lt_eq_u256)
}

fn use_pick_details(step: Step, name: &'static str, vdf_iters: usize) -> Step {
    step.condition(
        "Gt({state}.durability, 0)",
        Box::new(|ctx| {
            let obj = ctx.vars.get_dict(name).unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::gt((obj, "durability"), 0))
                .unwrap()
        }),
    )
    .var(
        "durability",
        Box::new(|ctx| {
            let obj = ctx.vars.get_dict(name).unwrap();
            let mut durability =
                i64::try_from(obj.get(&Key::from("durability")).unwrap().typed()).unwrap();
            durability -= 1;
            ctx.store("durability", Box::new(durability));
            Value::from(durability)
        }),
    )
    .condition(
        "SumOf({state}.durability, durability, 1)",
        Box::new(|ctx| {
            let durability: Box<i64> = ctx.take("durability");
            let obj = ctx.vars.get_dict(name).unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::sum_of((obj, "durability"), *durability, 1))
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
            let obj_raw = RawValue::from(obj);
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

pub(crate) fn dependencies() -> Vec<api::Dependency> {
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

pub(crate) fn actions() -> Vec<api::Action> {
    vec![
        api::Action {
            name: "FindLog",
            steps: vec![
                Step::output("log", "Log")
                    .set("blueprint", Arg::literal("Log"))
                    .var(
                        "work",
                        Box::new(|ctx| {
                            let log = ctx.vars.get("log");
                            let log_raw = RawValue::from(log);
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
                    .update("work", Arg::var("work")),
            ],
        },
        api::Action {
            name: "CraftWood",
            steps: vec![
                Step::input("log", "Log"),
                Step::output("wood", "Wood")
                    .set("blueprint", Arg::literal("Wood"))
                    .var(
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
                    )
                    .update("key", Arg::var("key"))
                    .condition(
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
        api::Action {
            name: "CraftSticks",
            steps: vec![
                Step::input("wood", "Wood"),
                Step::output("stick_a", "Stick").set("blueprint", Arg::literal("Stick")),
                Step::output("stick_b", "Stick").set("blueprint", Arg::literal("Stick")),
            ],
        },
        api::Action {
            name: "CraftWoodPick",
            steps: vec![
                Step::input("wood", "Wood"),
                Step::input("stick", "Stick"),
                Step::output("wood_pick", "WoodPick")
                    .set("blueprint", Arg::literal("WoodPick"))
                    .set("durability", Arg::literal(100i64)),
            ],
        },
        api::Action {
            name: "CraftStonePick",
            steps: vec![
                Step::input("stone", "Stone"),
                Step::input("stick", "Stick"),
                Step::output("stone_pick", "StonePick")
                    .set("blueprint", Arg::literal("StonePick"))
                    .set("durability", Arg::literal(200i64)),
            ],
        },
        api::Action {
            name: "UseWoodPick",
            steps: vec![
                Step::mutate("wood_pick", "WoodPick")
                    .snippet(|step| use_pick_details(step, "wood_pick", 10)),
            ],
        },
        api::Action {
            name: "MineStoneWithWoodPick",
            steps: vec![
                Step::depends("pick", "UseWoodPick"),
                Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")),
            ],
        },
        api::Action {
            name: "UseStonePick",
            steps: vec![
                Step::mutate("stone_pick", "StonePick")
                    .snippet(|step| use_pick_details(step, "stone_pick", 5)),
            ],
        },
        api::Action {
            name: "MineStoneWithStonePick",
            steps: vec![
                Step::depends("pick", "UseStonePick"),
                Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")),
            ],
        },
    ]
}

fn default_action_ui() -> ActionUiMeta {
    ActionUiMeta {
        emoji: "⚙️",
        description: "SDK action",
        cpu_cost: "unknown",
        reads_block: false,
        hidden: false,
    }
}

fn action_meta_by_name(name: &str) -> ActionUiMeta {
    ACTION_META
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.ui.clone())
        .unwrap_or_else(default_action_ui)
}

fn action_signatures(actions: &[api::Action]) -> HashMap<String, ActionSignature> {
    let by_name = actions
        .iter()
        .map(|action| (action.name(), action))
        .collect::<HashMap<_, _>>();
    let mut signatures = HashMap::<String, ActionSignature>::new();
    let mut visiting = HashSet::<String>::new();
    for action in actions {
        derive_signature(action.name(), &by_name, &mut signatures, &mut visiting);
    }
    signatures
}

fn derive_signature<'a>(
    action_name: &str,
    actions_by_name: &HashMap<&'a str, &'a api::Action>,
    cache: &mut HashMap<String, ActionSignature>,
    visiting: &mut HashSet<String>,
) -> ActionSignature {
    if let Some(signature) = cache.get(action_name) {
        return signature.clone();
    }
    if !visiting.insert(action_name.to_string()) {
        panic!("cyclic action dependency detected at {action_name}");
    }
    let action = actions_by_name
        .get(action_name)
        .unwrap_or_else(|| panic!("missing action definition for {action_name}"));
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for step in action.steps() {
        match step.kind() {
            StepKind::Input => {
                let class_name = step
                    .class()
                    .unwrap_or_else(|| panic!("input step missing class in {action_name}"));
                inputs.push(class_name.to_string());
            }
            StepKind::Mutate => {
                let class_name = step
                    .class()
                    .unwrap_or_else(|| panic!("mutate step missing class in {action_name}"))
                    .to_string();
                inputs.push(class_name.clone());
                outputs.push(class_name);
            }
            StepKind::Output => {
                let class_name = step
                    .class()
                    .unwrap_or_else(|| panic!("output step missing class in {action_name}"));
                outputs.push(class_name.to_string());
            }
            StepKind::Depends => {
                let dependency = step
                    .action()
                    .unwrap_or_else(|| panic!("depends step missing action in {action_name}"));
                let signature = derive_signature(dependency, actions_by_name, cache, visiting);
                inputs.extend(signature.inputs);
                outputs.extend(signature.outputs);
            }
        }
    }

    visiting.remove(action_name);
    let signature = ActionSignature { inputs, outputs };
    cache.insert(action_name.to_string(), signature.clone());
    signature
}

pub(crate) fn action_descriptors() -> Vec<ActionDescriptor> {
    let defs = actions();
    let signatures = action_signatures(&defs);
    defs.into_iter()
        .map(|action| {
            let name = action.name().to_string();
            let signature = signatures
                .get(&name)
                .unwrap_or_else(|| panic!("missing signature for action {name}"))
                .clone();
            let ui = action_meta_by_name(&name);
            ActionDescriptor {
                name,
                input_classes: signature.inputs,
                output_classes: signature.outputs,
                hidden: ui.hidden,
                ui,
            }
        })
        .collect()
}

pub(crate) fn visible_action_descriptors() -> Vec<ActionDescriptor> {
    action_descriptors()
        .into_iter()
        .filter(|descriptor| !descriptor.hidden)
        .collect()
}

pub(crate) fn action_descriptors_by_name() -> HashMap<String, ActionDescriptor> {
    action_descriptors()
        .into_iter()
        .map(|descriptor| (descriptor.name.clone(), descriptor))
        .collect()
}

pub(crate) fn action_ids() -> Vec<String> {
    action_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .collect()
}

pub(crate) fn class_names() -> Vec<String> {
    let mut classes = BTreeSet::<String>::new();
    for descriptor in action_descriptors() {
        classes.extend(descriptor.input_classes);
        classes.extend(descriptor.output_classes);
    }
    for class_meta in CLASS_META {
        classes.insert(class_meta.name.to_string());
    }
    classes.into_iter().collect()
}

pub(crate) fn class_ui_meta(class_name: &str) -> ClassUiMeta {
    CLASS_META
        .iter()
        .find(|entry| entry.name == class_name)
        .map(|entry| entry.ui.clone())
        .unwrap_or(ClassUiMeta {
            emoji: "📦",
            description: "Unknown class object",
        })
}
