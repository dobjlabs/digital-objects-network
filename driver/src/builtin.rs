use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use craft_sdk::{
    Context, Helper, SpendableObject, SpendableObjects,
    api::{self, Arg, Step, StepKind},
};
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::{
    frontend::{MainPod, Operation},
    middleware::{F, Hash, Key, Pod, RawValue, Statement, Value, hash_values, EMPTY_HASH},
};
use txlib::GroundingWitness;
use vdfpod::VdfPod;

use crate::catalog::{ActionCatalog, CatalogClass, extract_predicate};
use crate::types::ActionSummary;

const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

struct ActionMeta {
    name: &'static str,
    emoji: &'static str,
    description: &'static str,
    cpu_cost: &'static str,
    reads_block: bool,
    hidden: bool,
}

struct ClassMeta {
    name: &'static str,
    emoji: &'static str,
    description: &'static str,
}

#[derive(Debug, Clone)]
struct ActionSignature {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

const ACTION_META: &[ActionMeta] = &[
    ActionMeta {
        name: "FindLog",
        emoji: "🌲",
        description: "Discover a log object by proving a short VDF.",
        cpu_cost: "20-40s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "CraftWood",
        emoji: "🪵",
        description: "Refine one log into a wood object with PoW quality checks.",
        cpu_cost: "15-30s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "CraftSticks",
        emoji: "🥢",
        description: "Split one wood object into two stick objects.",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "CraftWoodPick",
        emoji: "⛏️",
        description: "Combine wood and a stick to craft a wood pick.",
        cpu_cost: "10-20s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "CraftStonePick",
        emoji: "⛏️",
        description: "Combine stone and a stick to craft a stronger stone pick.",
        cpu_cost: "10-20s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "UseWoodPick",
        emoji: "⛏️",
        description: "Internal durability/work update for wood pick usage.",
        cpu_cost: "10-30s",
        reads_block: false,
        hidden: true,
    },
    ActionMeta {
        name: "MineStoneWithWoodPick",
        emoji: "🪨",
        description: "Mine stone using a wood pick (consumes durability).",
        cpu_cost: "25-45s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "UseStonePick",
        emoji: "⛏️",
        description: "Internal durability/work update for stone pick usage.",
        cpu_cost: "5-20s",
        reads_block: false,
        hidden: true,
    },
    ActionMeta {
        name: "MineStoneWithStonePick",
        emoji: "🪨",
        description: "Mine stone using a stone pick (consumes durability).",
        cpu_cost: "15-35s",
        reads_block: false,
        hidden: false,
    },
    // ---------------------------------------------------------------
    // Message passing
    // ---------------------------------------------------------------
    ActionMeta {
        name: "CreateCounterInbox",
        emoji: "📬",
        description: "Create a counter with a public inbox for message passing.",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "SendCounterMessage",
        emoji: "✉️",
        description: "Send an increment/decrement message to a counter inbox.",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "ProcessCounterMessages",
        emoji: "⚙️",
        description: "Process pending messages and update the counter.",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "SendNegativeCounterMessage",
        emoji: "📉",
        description: "Send a -5 decrement message to a counter inbox (for testing rejection).",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
    ActionMeta {
        name: "RejectCounterMessage",
        emoji: "🚫",
        description: "Reject a message that would make the counter negative.",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
];

const CLASS_META: &[ClassMeta] = &[
    ClassMeta {
        name: "Log",
        emoji: "🌲",
        description: "A discovered log that can be refined into wood.",
    },
    ClassMeta {
        name: "Wood",
        emoji: "🪵",
        description: "Refined wood used for sticks and basic tools.",
    },
    ClassMeta {
        name: "Stick",
        emoji: "🥢",
        description: "A stick used as a handle in tool crafting.",
    },
    ClassMeta {
        name: "WoodPick",
        emoji: "⛏️",
        description: "A wood pick that can mine stone while durability remains.",
    },
    ClassMeta {
        name: "Stone",
        emoji: "🪨",
        description: "Mined stone used to craft stronger tools.",
    },
    ClassMeta {
        name: "StonePick",
        emoji: "⛏️",
        description: "A sturdier pick with higher starting durability.",
    },
    // ---------------------------------------------------------------
    // Message passing
    // ---------------------------------------------------------------
    ClassMeta {
        name: "Inbox",
        emoji: "📬",
        description: "A public inbox for message passing to a private object.",
    },
    ClassMeta {
        name: "InboxMessage",
        emoji: "✉️",
        description: "A public message sent to an inbox.",
    },
    ClassMeta {
        name: "Counter",
        emoji: "🔢",
        description: "A private counter whose state is driven by inbox messages.",
    },
];

pub struct BuiltinActionCatalog {
    actions: Vec<ActionSummary>,
    actions_by_name: HashMap<String, ActionSummary>,
    classes: Vec<CatalogClass>,
    classes_by_name: HashMap<String, CatalogClass>,
    podlang_src: String,
}

impl BuiltinActionCatalog {
    pub fn new() -> Self {
        let helper = Helper::new(dependencies(), actions());
        let action_hashes = helper.action_hashes();
        let class_hashes = helper.class_hashes();
        let podlang_src = helper.podlang_src.clone();

        let action_defs = actions();
        let signatures = action_signatures(&action_defs);
        let meta_by_name: HashMap<&str, &ActionMeta> =
            ACTION_META.iter().map(|m| (m.name, m)).collect();

        let actions: Vec<ActionSummary> = action_defs
            .iter()
            .filter_map(|action| {
                let name = action.name();
                let meta = meta_by_name.get(name);
                if meta.is_some_and(|m| m.hidden) {
                    return None;
                }
                let signature = signatures.get(name)?;
                Some(ActionSummary {
                    id: name.to_string(),
                    emoji: meta.map_or("⚙️", |m| m.emoji).to_string(),
                    hash: action_hashes
                        .get(name)
                        .map(|hash| format!("{:#}", hash))
                        .unwrap_or_default(),
                    input_class_hashes: signature
                        .inputs
                        .iter()
                        .map(|class_name| {
                            class_hashes
                                .get(class_name.as_str())
                                .map(|hash| format!("{:#}", hash))
                                .unwrap_or_default()
                        })
                        .collect(),
                    description: meta.map_or("SDK action", |m| m.description).to_string(),
                    cpu_cost: meta.map_or("unknown", |m| m.cpu_cost).to_string(),
                    reads_block: meta.is_some_and(|m| m.reads_block),
                    input_classes: signature.inputs.clone(),
                    output_classes: signature.outputs.clone(),
                })
            })
            .collect();
        let actions_by_name: HashMap<String, ActionSummary> = actions
            .iter()
            .map(|action| (action.id.clone(), action.clone()))
            .collect();

        let class_meta_by_name: HashMap<&str, &ClassMeta> =
            CLASS_META.iter().map(|m| (m.name, m)).collect();
        let mut class_name_set = BTreeSet::<String>::new();
        for sig in signatures.values() {
            class_name_set.extend(sig.inputs.iter().cloned());
            class_name_set.extend(sig.outputs.iter().cloned());
        }
        for cm in CLASS_META {
            class_name_set.insert(cm.name.to_string());
        }

        let classes: Vec<CatalogClass> = class_name_set
            .into_iter()
            .map(|class_name| {
                let cm = class_meta_by_name.get(class_name.as_str());
                let produced_by = actions
                    .iter()
                    .filter(|a| a.output_classes.contains(&class_name))
                    .map(|a| a.id.clone())
                    .collect();
                let consumed_by = actions
                    .iter()
                    .filter(|a| a.input_classes.contains(&class_name))
                    .map(|a| a.id.clone())
                    .collect();
                let predicate_source = extract_predicate(&podlang_src, &format!("Is{class_name}"))
                    .unwrap_or_else(|| format!("Is{class_name}(state) = OR(...)"));
                CatalogClass {
                    name: class_name.clone(),
                    emoji: cm.map_or("📦", |m| m.emoji).to_string(),
                    hash: class_hashes
                        .get(&class_name)
                        .map(|hash| format!("{:#}", hash))
                        .unwrap_or_default(),
                    description: cm
                        .map_or("Unknown class object", |m| m.description)
                        .to_string(),
                    produced_by,
                    consumed_by,
                    predicate_source,
                }
            })
            .collect();
        let classes_by_name: HashMap<String, CatalogClass> = classes
            .iter()
            .map(|c| (c.name.clone(), c.clone()))
            .collect();

        Self {
            actions,
            actions_by_name,
            classes,
            classes_by_name,
            podlang_src,
        }
    }
}

impl Default for BuiltinActionCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionCatalog for BuiltinActionCatalog {
    fn list_actions(&self) -> Vec<ActionSummary> {
        self.actions.clone()
    }

    fn get_action(&self, action_id: &str) -> Option<ActionSummary> {
        self.actions_by_name.get(action_id).cloned()
    }

    fn list_classes(&self) -> Vec<CatalogClass> {
        self.classes.clone()
    }

    fn get_class(&self, class_name: &str) -> Option<CatalogClass> {
        self.classes_by_name.get(class_name).cloned()
    }

    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        let helper = Helper::new(dependencies(), actions());
        let builder = helper.builder(false, Arc::new(grounding_witness));
        Ok(builder.action(&action_id, inputs))
    }

    fn generated_podlang(&self) -> Option<String> {
        Some(self.podlang_src.clone())
    }
}

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
    .var(
        "key",
        Box::new(|_ctx| Value::from(pod2utils::rand_raw_value())),
    )
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

pub(crate) fn dependencies() -> Vec<api::Dependency> {
    vec![
        api::Dependency::Intro {
            pred: "Vdf(count, input, output)",
            hash: *vdfpod::STANDARD_VDF_VD_HASH,
        },
        api::Dependency::Intro {
            pred: "LtEqU256(lhs, rhs)",
            hash: *lt_eq_u256_pod::STANDARD_LT_EQ_U256_VD_HASH,
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
                    .set("public", Arg::literal(false))
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
                    .set("public", Arg::literal(false))
                    .var(
                        "key",
                        Box::new(|ctx| {
                            let mut wood = ctx.vars.get("wood").as_dictionary().unwrap();
                            let mut key = Value::from(pod2utils::rand_raw_value());
                            if !ctx.mock {
                                while RawValue::from(wood.commitment()).0[3].0 > WOOD_POW_DIFFICULTY
                                {
                                    key = Value::from(pod2utils::rand_raw_value());
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
                Step::output("stick_a", "Stick").set("blueprint", Arg::literal("Stick")).set("public", Arg::literal(false)),
                Step::output("stick_b", "Stick").set("blueprint", Arg::literal("Stick")).set("public", Arg::literal(false)),
            ],
        },
        api::Action {
            name: "CraftWoodPick",
            steps: vec![
                Step::input("wood", "Wood"),
                Step::input("stick", "Stick"),
                Step::output("wood_pick", "WoodPick")
                    .set("blueprint", Arg::literal("WoodPick"))
                    .set("public", Arg::literal(false))
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
                    .set("public", Arg::literal(false))
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
                Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")).set("public", Arg::literal(false)),
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
                Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")).set("public", Arg::literal(false)),
            ],
        },
        // ---------------------------------------------------------------
        // Public object example: Bounty
        // ---------------------------------------------------------------
        api::Action {
            name: "CreateBounty",
            steps: vec![
                Step::input("reward", "WoodPick"),
                Step::output("bounty", "Bounty")
                    .set("blueprint", Arg::literal("Bounty"))
                    .set("public", Arg::literal(true))
                    .set("status", Arg::literal("open")),
            ],
        },
        api::Action {
            name: "FillBounty",
            steps: vec![
                Step::input("bounty", "Bounty").is_public(true),
                Step::input("log", "Log"),
                Step::output("filled_bounty", "Bounty")
                    .set("blueprint", Arg::literal("Bounty"))
                    .set("public", Arg::literal(true))
                    .set("status", Arg::literal("filled")),
            ],
        },
        // ---------------------------------------------------------------
        // Message passing example: Counter with Inbox
        // ---------------------------------------------------------------
        //
        // CreateCounterInbox: create a public Inbox + private Counter.
        // CreateCounterInbox: create Inbox (public) + Counter (private).
        // Both share an inbox_id. The inbox tracks message_count,
        // messages_root (hash chain), processed_count,
        // processed_messages_root, and state_commitment.
        api::Action {
            name: "CreateCounterInbox",
            steps: vec![
                Step::output("counter", "Counter")
                    .set("blueprint", Arg::literal("Counter"))
                    .set("public", Arg::literal(false))
                    .set("count", Arg::literal(0i64))
                    .var(
                        "inbox_id",
                        Box::new(|ctx| {
                            // Derive inbox_id from the inbox output's key
                            // (inbox isn't staged yet, so we use the counter's
                            // key as seed — both objects share the same tx)
                            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
                            let key_val = counter.get(&Key::from("key")).unwrap().unwrap();
                            let id = hash_values(&[key_val, Value::from("inbox-id")]);
                            ctx.store("inbox_id_hash", Box::new(id));
                            Value::from(RawValue::from(id))
                        }),
                    )
                    .set("inbox_id", Arg::var("inbox_id")),
                Step::output("inbox", "Inbox")
                    .set("blueprint", Arg::literal("Inbox"))
                    .set("public", Arg::literal(true))
                    .set("message_count", Arg::literal(0i64))
                    .set("processed_count", Arg::literal(0i64))
                    .var(
                        "init_inbox_id",
                        Box::new(|ctx| {
                            let id: Box<Hash> = ctx.take("inbox_id_hash");
                            Value::from(RawValue::from(*id))
                        }),
                    )
                    .var(
                        "state_commitment",
                        Box::new(|ctx| {
                            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
                            Value::from(RawValue::from(counter.commitment()))
                        }),
                    )
                    .var(
                        "empty_root",
                        Box::new(|_ctx| Value::from(EMPTY_HASH)),
                    )
                    .set("inbox_id", Arg::var("init_inbox_id"))
                    .set("state_commitment", Arg::var("state_commitment"))
                    .set("messages_root", Arg::var("empty_root"))
                    .set("processed_messages_root", Arg::var("empty_root")),
            ],
        },
        // SendCounterMessage: anyone consumes the old inbox, produces a
        // new inbox (message_count++, messages_root extended) and a
        // separate InboxMessage public object carrying the amount.
        api::Action {
            name: "SendCounterMessage",
            steps: vec![
                Step::input("inbox", "Inbox").is_public(true),
                Step::output("message", "InboxMessage")
                    .set("blueprint", Arg::literal("InboxMessage"))
                    .set("public", Arg::literal(true))
                    .set("amount", Arg::literal(1i64))
                    .var(
                        "msg_inbox_id",
                        Box::new(|ctx| {
                            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
                            inbox.get(&Key::from("inbox_id")).unwrap().unwrap()
                        }),
                    )
                    .set("inbox_id", Arg::var("msg_inbox_id")),
                Step::output("new_inbox", "Inbox")
                    .set("blueprint", Arg::literal("Inbox"))
                    .set("public", Arg::literal(true))
                    .snippet(|step| send_counter_message_inbox_details(step)),
            ],
        },
        // ProcessCounterMessages: the holder consumes the message, inbox,
        // and counter. Produces a new counter (count updated) and a new
        // inbox (processed_count advanced, messages_root chain verified,
        // state_commitment updated).
        api::Action {
            name: "ProcessCounterMessages",
            steps: vec![
                Step::input("message", "InboxMessage").is_public(true),
                Step::input("inbox", "Inbox").is_public(true),
                Step::input("counter", "Counter"),
                Step::output("new_counter", "Counter")
                    .set("blueprint", Arg::literal("Counter"))
                    .set("public", Arg::literal(false))
                    .set("count", Arg::literal(0i64))
                    .snippet(|step| process_counter_update_details(step)),
                Step::output("new_inbox", "Inbox")
                    .set("blueprint", Arg::literal("Inbox"))
                    .set("public", Arg::literal(true))
                    .snippet(|step| process_counter_inbox_details(step)),
            ],
        },
        // SendNegativeCounterMessage: same as SendCounterMessage but
        // with amount=-5. Used to test rejection.
        api::Action {
            name: "SendNegativeCounterMessage",
            steps: vec![
                Step::input("inbox", "Inbox").is_public(true),
                Step::output("message", "InboxMessage")
                    .set("blueprint", Arg::literal("InboxMessage"))
                    .set("public", Arg::literal(true))
                    .set("amount", Arg::literal(-5i64))
                    .var(
                        "msg_inbox_id",
                        Box::new(|ctx| {
                            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
                            inbox.get(&Key::from("inbox_id")).unwrap().unwrap()
                        }),
                    )
                    .set("inbox_id", Arg::var("msg_inbox_id")),
                Step::output("new_inbox", "Inbox")
                    .set("blueprint", Arg::literal("Inbox"))
                    .set("public", Arg::literal(true))
                    .snippet(|step| send_counter_message_inbox_details(step)),
            ],
        },
        // RejectCounterMessage: the holder proves a message would make
        // the counter negative. The counter is consumed and re-emitted
        // unchanged (same count, same inbox_id) so the proof can read
        // count and verify count + amount < 0. The inbox advances
        // processed_count and processed_messages_root but
        // state_commitment stays the same.
        api::Action {
            name: "RejectCounterMessage",
            steps: vec![
                Step::input("message", "InboxMessage").is_public(true),
                Step::input("inbox", "Inbox").is_public(true),
                Step::input("counter", "Counter"),
                Step::output("new_counter", "Counter")
                    .set("blueprint", Arg::literal("Counter"))
                    .set("public", Arg::literal(false))
                    .set("count", Arg::literal(0i64))
                    .snippet(|step| reject_counter_reemit_details(step)),
                Step::output("new_inbox", "Inbox")
                    .set("blueprint", Arg::literal("Inbox"))
                    .set("public", Arg::literal(true))
                    .snippet(|step| reject_counter_inbox_details(step)),
            ],
        },
    ]
}

/// RejectCounterMessage counter output: re-emit the counter with
/// the same count and inbox_id. The counter is consumed and re-created
/// so the proof can read its fields and prove count + amount < 0.
fn reject_counter_reemit_details(step: Step) -> Step {
    step
    .var(
        "kept_count",
        Box::new(|ctx| {
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            let count = counter.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap();
            ctx.store("rejection_count", Box::new(count));
            Value::from(count)
        }),
    )
    .var(
        "kept_counter_inbox_id",
        Box::new(|ctx| {
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            counter.get(&Key::from("inbox_id")).unwrap().unwrap()
        }),
    )
    .set("inbox_id", Arg::var("kept_counter_inbox_id"))
    .update("count", Arg::var("kept_count"))
}

/// RejectCounterMessage inbox output: advance processed_count by 1,
/// advance processed_messages_root by one hash chain link, preserve
/// state_commitment and all queue fields. Prove the rejection:
/// count + amount < 0 (would make counter negative).
fn reject_counter_inbox_details(step: Step) -> Step {
    step
    .var(
        "kept_inbox_id",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("inbox_id")).unwrap().unwrap()
        }),
    )
    .var(
        "kept_message_count",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("message_count")).unwrap().unwrap()
        }),
    )
    .var(
        "kept_messages_root",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("messages_root")).unwrap().unwrap()
        }),
    )
    .var(
        "kept_state_commitment",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("state_commitment")).unwrap().unwrap()
        }),
    )
    .var(
        "new_processed_count",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let old_pc = inbox
                .get(&Key::from("processed_count"))
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap();
            ctx.store("old_processed_count", Box::new(old_pc));
            Value::from(old_pc + 1)
        }),
    )
    .var(
        "new_processed_messages_root",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let old_pmr = inbox
                .get(&Key::from("processed_messages_root"))
                .unwrap()
                .unwrap();
            let old_root = Hash(old_pmr.raw().0);
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            let new_root = hash_values(&[
                Value::from(old_root),
                Value::from(message.commitment()),
            ]);
            ctx.store("new_pmr_hash", Box::new(new_root));
            Value::from(RawValue::from(new_root))
        }),
    )
    // Compute would_be_result = count + amount (for rejection proof)
    .var(
        "would_be_result",
        Box::new(|ctx| {
            let count: Box<i64> = ctx.take("rejection_count");
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            let amount = message
                .get(&Key::from("amount"))
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap();
            ctx.store("would_be_result_val", Box::new(*count + amount));
            Value::from(*count + amount)
        }),
    )
    // All Sets
    .set("inbox_id", Arg::var("kept_inbox_id"))
    .set("message_count", Arg::var("kept_message_count"))
    .set("messages_root", Arg::var("kept_messages_root"))
    .set("state_commitment", Arg::var("kept_state_commitment"))
    .set("processed_count", Arg::var("new_processed_count"))
    .set("processed_messages_root", Arg::var("new_processed_messages_root"))
    // Conditions
    // 1. processed_count incremented by 1
    .condition(
        "SumOf(new_processed_count, inbox.processed_count, 1)",
        Box::new(|ctx| {
            let old_pc: Box<i64> = ctx.take("old_processed_count");
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::sum_of(
                    *old_pc + 1,
                    (&inbox, "processed_count"),
                    1,
                ))
                .unwrap()
        }),
    )
    // 2. Prove would_be_result = counter.count + message.amount
    .condition(
        "SumOf(would_be_result, counter.count, message.amount)",
        Box::new(|ctx| {
            let would_be: Box<i64> = ctx.take("would_be_result_val");
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::sum_of(
                    *would_be,
                    (&counter, "count"),
                    (&message, "amount"),
                ))
                .unwrap()
        }),
    )
    // 3. Prove 0 > would_be_result (counter would go negative)
    .condition(
        "Gt(0, would_be_result)",
        Box::new(|ctx| {
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            let count = counter.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap();
            let amount = message.get(&Key::from("amount")).unwrap().unwrap().as_int().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::gt(0, count + amount))
                .unwrap()
        }),
    )
}

/// SendCounterMessage inbox output: copy inbox_id, state_commitment,
/// processed_count, processed_messages_root. Increment message_count.
/// Extend messages_root hash chain: new_root = H(old_root, commitment(message)).
fn send_counter_message_inbox_details(step: Step) -> Step {
    step
    // Pre-compute all values before Sets
    .var(
        "old_inbox_id",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("inbox_id")).unwrap().unwrap()
        }),
    )
    .var(
        "old_state_commitment",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("state_commitment")).unwrap().unwrap()
        }),
    )
    .var(
        "old_processed_count",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("processed_count")).unwrap().unwrap()
        }),
    )
    .var(
        "old_processed_messages_root",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("processed_messages_root")).unwrap().unwrap()
        }),
    )
    .var(
        "new_message_count",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let old_count = inbox
                .get(&Key::from("message_count"))
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap();
            ctx.store("old_message_count", Box::new(old_count));
            Value::from(old_count + 1)
        }),
    )
    .var(
        "new_messages_root",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let old_root_val = inbox
                .get(&Key::from("messages_root"))
                .unwrap()
                .unwrap();
            let old_root = Hash(old_root_val.raw().0);
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            let msg_commitment = message.commitment();
            let new_root = hash_values(&[
                Value::from(old_root),
                Value::from(msg_commitment),
            ]);
            ctx.store("new_messages_root_hash", Box::new(new_root));
            Value::from(RawValue::from(new_root))
        }),
    )
    // All Sets (order matters: Sets before Conditions/Updates)
    .set("inbox_id", Arg::var("old_inbox_id"))
    .set("state_commitment", Arg::var("old_state_commitment"))
    .set("processed_count", Arg::var("old_processed_count"))
    .set("processed_messages_root", Arg::var("old_processed_messages_root"))
    .set("message_count", Arg::var("new_message_count"))
    .set("messages_root", Arg::var("new_messages_root"))
    // Conditions: prove message_count increment and hash chain extension
    .condition(
        "SumOf(new_message_count, inbox.message_count, 1)",
        Box::new(|ctx| {
            let old_count: Box<i64> = ctx.take("old_message_count");
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::sum_of(
                    *old_count + 1,
                    (&inbox, "message_count"),
                    1,
                ))
                .unwrap()
        }),
    )
    .condition(
        "HashOf(new_messages_root, inbox.messages_root, message)",
        Box::new(|ctx| {
            let new_root: Box<Hash> = ctx.take("new_messages_root_hash");
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::hash_of(
                    RawValue::from(*new_root),
                    (&inbox, "messages_root"),
                    message.commitment(),
                ))
                .unwrap()
        }),
    )
}

/// ProcessCounterMessages counter output: read amount from the message,
/// prove new_count = old_count + amount. Copy inbox_id from old counter.
fn process_counter_update_details(step: Step) -> Step {
    step.var(
        "amount",
        Box::new(|ctx| {
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            let amount = message
                .get(&Key::from("amount"))
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap();
            ctx.store("amount_i64", Box::new(amount));
            Value::from(amount)
        }),
    )
    .var(
        "counter_inbox_id",
        Box::new(|ctx| {
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            counter.get(&Key::from("inbox_id")).unwrap().unwrap()
        }),
    )
    .var(
        "new_count",
        Box::new(|ctx| {
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            let old_count = counter
                .get(&Key::from("count"))
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap();
            let amount: Box<i64> = ctx.take("amount_i64");
            ctx.store("old_count", Box::new(old_count));
            ctx.store("amount_val", Box::new(*amount));
            Value::from(old_count + *amount)
        }),
    )
    .set("inbox_id", Arg::var("counter_inbox_id"))
    .condition(
        "SumOf(new_count, counter.count, amount)",
        Box::new(|ctx| {
            let old_count: Box<i64> = ctx.take("old_count");
            let amount: Box<i64> = ctx.take("amount_val");
            let counter = ctx.vars.get("counter").as_dictionary().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::sum_of(
                    *old_count + *amount,
                    (&counter, "count"),
                    *amount,
                ))
                .unwrap()
        }),
    )
    .update("count", Arg::var("new_count"))
}

/// ProcessCounterMessages inbox output: advance processed_count by 1,
/// advance processed_messages_root by one hash chain link, preserve
/// queue fields, update state_commitment.
fn process_counter_inbox_details(step: Step) -> Step {
    step
    .var(
        "kept_inbox_id",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("inbox_id")).unwrap().unwrap()
        }),
    )
    .var(
        "kept_message_count",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("message_count")).unwrap().unwrap()
        }),
    )
    .var(
        "kept_messages_root",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            inbox.get(&Key::from("messages_root")).unwrap().unwrap()
        }),
    )
    .var(
        "new_state_commitment",
        Box::new(|ctx| {
            let new_counter = ctx.vars.get("new_counter").as_dictionary().unwrap();
            Value::from(RawValue::from(new_counter.commitment()))
        }),
    )
    // Advance processed_count by 1 (process one message at a time)
    .var(
        "new_processed_count",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let old_pc = inbox
                .get(&Key::from("processed_count"))
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap();
            ctx.store("old_processed_count_val", Box::new(old_pc));
            Value::from(old_pc + 1)
        }),
    )
    // Advance processed_messages_root by one chain link:
    // new_pmr = H(old_pmr, commitment(message))
    .var(
        "new_processed_messages_root",
        Box::new(|ctx| {
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            let old_pmr = inbox
                .get(&Key::from("processed_messages_root"))
                .unwrap()
                .unwrap();
            let old_root = Hash(old_pmr.raw().0);
            let message = ctx.vars.get("message").as_dictionary().unwrap();
            let new_root = hash_values(&[
                Value::from(old_root),
                Value::from(message.commitment()),
            ]);
            ctx.store("process_new_pmr_hash", Box::new(new_root));
            Value::from(RawValue::from(new_root))
        }),
    )
    .set("inbox_id", Arg::var("kept_inbox_id"))
    .set("message_count", Arg::var("kept_message_count"))
    .set("messages_root", Arg::var("kept_messages_root"))
    .set("processed_count", Arg::var("new_processed_count"))
    .set("processed_messages_root", Arg::var("new_processed_messages_root"))
    .set("state_commitment", Arg::var("new_state_commitment"))
    // Prove processed_count incremented by 1
    .condition(
        "SumOf(new_processed_count, inbox.processed_count, 1)",
        Box::new(|ctx| {
            let old_pc: Box<i64> = ctx.take("old_processed_count_val");
            let inbox = ctx.vars.get("inbox").as_dictionary().unwrap();
            ctx.bld
                .builder
                .priv_op(Operation::sum_of(
                    *old_pc + 1,
                    (&inbox, "processed_count"),
                    1,
                ))
                .unwrap()
        }),
    )
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

#[cfg(test)]
mod tests {
    use super::BuiltinActionCatalog;
    use crate::catalog::ActionCatalog;

    #[test]
    fn test_builtin_catalog_hides_internal_actions() {
        let catalog = BuiltinActionCatalog::new();
        let action_ids = catalog
            .list_actions()
            .into_iter()
            .map(|action| action.id)
            .collect::<Vec<_>>();
        assert!(action_ids.contains(&"CraftWood".to_string()));
        assert!(!action_ids.contains(&"UseWoodPick".to_string()));
    }

    #[test]
    fn test_builtin_catalog_lists_classes() {
        let catalog = BuiltinActionCatalog::new();
        let class_names = catalog
            .list_classes()
            .into_iter()
            .map(|class_info| class_info.name)
            .collect::<Vec<_>>();
        assert!(class_names.contains(&"Log".to_string()));
        assert!(class_names.contains(&"StonePick".to_string()));
    }
}
