#![allow(dead_code)]

use craft_sdk::{
    api::{self, Arg, Step},
    Context,
};
use hex::FromHex;
use lt_eq_u256_pod::LtEqU256Pod;
use plugin_api::{ActionUiMeta, ClassUiMeta, PluginSpec};
use pod2::{
    frontend::{MainPod, Operation},
    middleware::{Hash, Key, Pod, RawValue, Statement, Value, F},
};
use pod2utils::rand_raw_value;
use vdfpod::VdfPod;

const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

/// The built-in "Minecraft basics" plugin that ships with the app.
pub struct MinecraftPlugin;

impl PluginSpec for MinecraftPlugin {
    fn name(&self) -> &'static str {
        "minecraft-basics"
    }

    fn dependencies(&self) -> Vec<api::Dependency> {
        dependencies()
    }

    fn actions(&self) -> Vec<api::Action> {
        actions()
    }

    fn action_ui_meta(&self) -> Vec<(&'static str, ActionUiMeta)> {
        vec![
            (
                "FindLog",
                ActionUiMeta {
                    emoji: "🌲",
                    description: "Discover a log object by proving a short VDF.",
                    cpu_cost: "20-40s",
                    reads_block: false,
                    hidden: false,
                },
            ),
            (
                "CraftWood",
                ActionUiMeta {
                    emoji: "🪵",
                    description: "Refine one log into a wood object with PoW quality checks.",
                    cpu_cost: "15-30s",
                    reads_block: false,
                    hidden: false,
                },
            ),
            (
                "CraftSticks",
                ActionUiMeta {
                    emoji: "🥢",
                    description: "Split one wood object into two stick objects.",
                    cpu_cost: "5-10s",
                    reads_block: false,
                    hidden: false,
                },
            ),
            (
                "CraftWoodPick",
                ActionUiMeta {
                    emoji: "⛏️",
                    description: "Combine wood and a stick to craft a wood pick.",
                    cpu_cost: "10-20s",
                    reads_block: false,
                    hidden: false,
                },
            ),
            (
                "CraftStonePick",
                ActionUiMeta {
                    emoji: "⛏️",
                    description: "Combine stone and a stick to craft a stronger stone pick.",
                    cpu_cost: "10-20s",
                    reads_block: false,
                    hidden: false,
                },
            ),
            (
                "UseWoodPick",
                ActionUiMeta {
                    emoji: "⛏️",
                    description: "Internal durability/work update for wood pick usage.",
                    cpu_cost: "10-30s",
                    reads_block: false,
                    hidden: true,
                },
            ),
            (
                "MineStoneWithWoodPick",
                ActionUiMeta {
                    emoji: "🪨",
                    description: "Mine stone using a wood pick (consumes durability).",
                    cpu_cost: "25-45s",
                    reads_block: false,
                    hidden: false,
                },
            ),
            (
                "UseStonePick",
                ActionUiMeta {
                    emoji: "⛏️",
                    description: "Internal durability/work update for stone pick usage.",
                    cpu_cost: "5-20s",
                    reads_block: false,
                    hidden: true,
                },
            ),
            (
                "MineStoneWithStonePick",
                ActionUiMeta {
                    emoji: "🪨",
                    description: "Mine stone using a stone pick (consumes durability).",
                    cpu_cost: "15-35s",
                    reads_block: false,
                    hidden: false,
                },
            ),
        ]
    }

    fn class_ui_meta_entries(&self) -> Vec<(&'static str, ClassUiMeta)> {
        vec![
            (
                "Log",
                ClassUiMeta {
                    emoji: "🌲",
                    description: "A discovered log that can be refined into wood.",
                },
            ),
            (
                "Wood",
                ClassUiMeta {
                    emoji: "🪵",
                    description: "Refined wood used for sticks and basic tools.",
                },
            ),
            (
                "Stick",
                ClassUiMeta {
                    emoji: "🥢",
                    description: "A stick used as a handle in tool crafting.",
                },
            ),
            (
                "WoodPick",
                ClassUiMeta {
                    emoji: "⛏️",
                    description: "A wood pick that can mine stone while durability remains.",
                },
            ),
            (
                "Stone",
                ClassUiMeta {
                    emoji: "🪨",
                    description: "Mined stone used to craft stronger tools.",
                },
            ),
            (
                "StonePick",
                ClassUiMeta {
                    emoji: "⛏️",
                    description: "A sturdier pick with higher starting durability.",
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// Helper functions for proof generation (moved from spec.rs)
// ---------------------------------------------------------------------------

fn main_pod(ctx: &Context<'_>, pod: Box<dyn Pod>) -> MainPod {
    let pub_statements = pod.pub_statements();
    MainPod {
        pod,
        public_statements: pub_statements,
        params: ctx.params.clone(),
    }
}

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

fn dependencies() -> Vec<api::Dependency> {
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

fn actions() -> Vec<api::Action> {
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
                            let mut wood = ctx.vars.get("wood").as_dictionary().unwrap();
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
                            let wood_raw = wood.as_raw();
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
            steps: vec![Step::mutate("wood_pick", "WoodPick")
                .snippet(|step| use_pick_details(step, "wood_pick", 10))],
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
            steps: vec![Step::mutate("stone_pick", "StonePick")
                .snippet(|step| use_pick_details(step, "stone_pick", 5))],
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
