use extism_pdk::*;
use plugin_api::*;

#[plugin_fn]
pub fn get_metadata() -> FnResult<Json<PluginMetadata>> {
    Ok(Json(PluginMetadata {
        name: "minecraft-basics".into(),
        version: "0.1.0".into(),
        dependencies: dependencies(),
        classes: classes(),
        actions: actions(),
    }))
}

fn dependencies() -> Vec<DependencyMeta> {
    vec![
        DependencyMeta {
            dep_type: DependencyType::Intro,
            pred: "Vdf(count, input, output)".into(),
            hash: "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06".into(),
        },
        DependencyMeta {
            dep_type: DependencyType::Intro,
            pred: "LtEqU256(lhs, rhs)".into(),
            hash: "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529".into(),
        },
    ]
}

fn classes() -> Vec<ClassMeta> {
    vec![
        ClassMeta {
            name: "Log".into(),
            emoji: "🌲".into(),
            description: "A discovered log that can be refined into wood.".into(),
        },
        ClassMeta {
            name: "Wood".into(),
            emoji: "🪵".into(),
            description: "Refined wood used for sticks and basic tools.".into(),
        },
        ClassMeta {
            name: "Stick".into(),
            emoji: "🥢".into(),
            description: "A stick used as a handle in tool crafting.".into(),
        },
        ClassMeta {
            name: "WoodPick".into(),
            emoji: "⛏️".into(),
            description: "A wood pick that can mine stone while durability remains.".into(),
        },
        ClassMeta {
            name: "Stone".into(),
            emoji: "🪨".into(),
            description: "Mined stone used to craft stronger tools.".into(),
        },
        ClassMeta {
            name: "StonePick".into(),
            emoji: "⛏️".into(),
            description: "A sturdier pick with higher starting durability.".into(),
        },
    ]
}

const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

fn actions() -> Vec<ActionMeta> {
    vec![
        ActionMeta {
            name: "FindLog".into(),
            emoji: "🌲".into(),
            description: "Discover a log object by proving a short VDF.".into(),
            cpu_cost: "20-40s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![StepMeta {
                kind: StepKindMeta::Output,
                name: "log".into(),
                class: "Log".into(),
                action: String::new(),
                details: vec![
                    DetailMeta::Set {
                        key: "blueprint".into(),
                        value: LiteralValue::Str("Log".into()),
                    },
                    DetailMeta::Var {
                        name: "work".into(),
                        recipe: VarRecipe::Vdf { iters: 3 },
                    },
                    DetailMeta::Condition {
                        pred: "Vdf(3, {state}, work)".into(),
                        recipe: ConditionRecipe::StoredVdfPod,
                    },
                    DetailMeta::Update {
                        key: "work".into(),
                        source: "work".into(),
                    },
                ],
            }],
        },
        ActionMeta {
            name: "CraftWood".into(),
            emoji: "🪵".into(),
            description: "Refine one log into a wood object with PoW quality checks.".into(),
            cpu_cost: "15-30s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![
                StepMeta {
                    kind: StepKindMeta::Input,
                    name: "log".into(),
                    class: "Log".into(),
                    action: String::new(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "wood".into(),
                    class: "Wood".into(),
                    action: String::new(),
                    details: vec![
                        DetailMeta::Set {
                            key: "blueprint".into(),
                            value: LiteralValue::Str("Wood".into()),
                        },
                        DetailMeta::Var {
                            name: "key".into(),
                            recipe: VarRecipe::PowGrind {
                                difficulty: WOOD_POW_DIFFICULTY,
                            },
                        },
                        DetailMeta::Update {
                            key: "key".into(),
                            source: "key".into(),
                        },
                        DetailMeta::Condition {
                            pred: "LtEqU256({state}, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))".into(),
                            recipe: ConditionRecipe::LtEqU256 {
                                difficulty: WOOD_POW_DIFFICULTY,
                            },
                        },
                    ],
                },
            ],
        },
        ActionMeta {
            name: "CraftSticks".into(),
            emoji: "🥢".into(),
            description: "Split one wood object into two stick objects.".into(),
            cpu_cost: "5-10s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![
                StepMeta {
                    kind: StepKindMeta::Input,
                    name: "wood".into(),
                    class: "Wood".into(),
                    action: String::new(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "stick_a".into(),
                    class: "Stick".into(),
                    action: String::new(),
                    details: vec![DetailMeta::Set {
                        key: "blueprint".into(),
                        value: LiteralValue::Str("Stick".into()),
                    }],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "stick_b".into(),
                    class: "Stick".into(),
                    action: String::new(),
                    details: vec![DetailMeta::Set {
                        key: "blueprint".into(),
                        value: LiteralValue::Str("Stick".into()),
                    }],
                },
            ],
        },
        ActionMeta {
            name: "CraftWoodPick".into(),
            emoji: "⛏️".into(),
            description: "Combine wood and a stick to craft a wood pick.".into(),
            cpu_cost: "10-20s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![
                StepMeta {
                    kind: StepKindMeta::Input,
                    name: "wood".into(),
                    class: "Wood".into(),
                    action: String::new(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Input,
                    name: "stick".into(),
                    class: "Stick".into(),
                    action: String::new(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "wood_pick".into(),
                    class: "WoodPick".into(),
                    action: String::new(),
                    details: vec![
                        DetailMeta::Set {
                            key: "blueprint".into(),
                            value: LiteralValue::Str("WoodPick".into()),
                        },
                        DetailMeta::Set {
                            key: "durability".into(),
                            value: LiteralValue::Int(100),
                        },
                    ],
                },
            ],
        },
        ActionMeta {
            name: "CraftStonePick".into(),
            emoji: "⛏️".into(),
            description: "Combine stone and a stick to craft a stronger stone pick.".into(),
            cpu_cost: "10-20s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![
                StepMeta {
                    kind: StepKindMeta::Input,
                    name: "stone".into(),
                    class: "Stone".into(),
                    action: String::new(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Input,
                    name: "stick".into(),
                    class: "Stick".into(),
                    action: String::new(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "stone_pick".into(),
                    class: "StonePick".into(),
                    action: String::new(),
                    details: vec![
                        DetailMeta::Set {
                            key: "blueprint".into(),
                            value: LiteralValue::Str("StonePick".into()),
                        },
                        DetailMeta::Set {
                            key: "durability".into(),
                            value: LiteralValue::Int(200),
                        },
                    ],
                },
            ],
        },
        ActionMeta {
            name: "UseWoodPick".into(),
            emoji: "⛏️".into(),
            description: "Internal durability/work update for wood pick usage.".into(),
            cpu_cost: "10-30s".into(),
            reads_block: false,
            hidden: true,
            steps: vec![StepMeta {
                kind: StepKindMeta::Mutate,
                name: "wood_pick".into(),
                class: "WoodPick".into(),
                action: String::new(),
                details: use_pick_details(10),
            }],
        },
        ActionMeta {
            name: "MineStoneWithWoodPick".into(),
            emoji: "🪨".into(),
            description: "Mine stone using a wood pick (consumes durability).".into(),
            cpu_cost: "25-45s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![
                StepMeta {
                    kind: StepKindMeta::Depends,
                    name: "pick".into(),
                    class: String::new(),
                    action: "UseWoodPick".into(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "stone".into(),
                    class: "Stone".into(),
                    action: String::new(),
                    details: vec![DetailMeta::Set {
                        key: "blueprint".into(),
                        value: LiteralValue::Str("Stone".into()),
                    }],
                },
            ],
        },
        ActionMeta {
            name: "UseStonePick".into(),
            emoji: "⛏️".into(),
            description: "Internal durability/work update for stone pick usage.".into(),
            cpu_cost: "5-20s".into(),
            reads_block: false,
            hidden: true,
            steps: vec![StepMeta {
                kind: StepKindMeta::Mutate,
                name: "stone_pick".into(),
                class: "StonePick".into(),
                action: String::new(),
                details: use_pick_details(5),
            }],
        },
        ActionMeta {
            name: "MineStoneWithStonePick".into(),
            emoji: "🪨".into(),
            description: "Mine stone using a stone pick (consumes durability).".into(),
            cpu_cost: "15-35s".into(),
            reads_block: false,
            hidden: false,
            steps: vec![
                StepMeta {
                    kind: StepKindMeta::Depends,
                    name: "pick".into(),
                    class: String::new(),
                    action: "UseStonePick".into(),
                    details: vec![],
                },
                StepMeta {
                    kind: StepKindMeta::Output,
                    name: "stone".into(),
                    class: "Stone".into(),
                    action: String::new(),
                    details: vec![DetailMeta::Set {
                        key: "blueprint".into(),
                        value: LiteralValue::Str("Stone".into()),
                    }],
                },
            ],
        },
    ]
}

/// Shared detail pattern for pick usage (durability + VDF + key rotation).
fn use_pick_details(vdf_iters: usize) -> Vec<DetailMeta> {
    vec![
        DetailMeta::Condition {
            pred: "Gt({state}.durability, 0)".into(),
            recipe: ConditionRecipe::Gt {
                key: "durability".into(),
                value: 0,
            },
        },
        DetailMeta::Var {
            name: "durability".into(),
            recipe: VarRecipe::DecrementField {
                key: "durability".into(),
            },
        },
        DetailMeta::Condition {
            pred: "SumOf({state}.durability, durability, 1)".into(),
            recipe: ConditionRecipe::SumOf {
                key: "durability".into(),
                stored_var: "durability".into(),
                b: 1,
            },
        },
        DetailMeta::Update {
            key: "durability".into(),
            source: "durability".into(),
        },
        DetailMeta::Var {
            name: "key".into(),
            recipe: VarRecipe::RandomKey,
        },
        DetailMeta::Update {
            key: "key".into(),
            source: "key".into(),
        },
        DetailMeta::Var {
            name: "work".into(),
            recipe: VarRecipe::Vdf { iters: vdf_iters },
        },
        DetailMeta::Condition {
            pred: format!("Vdf({vdf_iters}, {{state}}, work)"),
            recipe: ConditionRecipe::StoredVdfPod,
        },
        DetailMeta::Update {
            key: "work".into(),
            source: "work".into(),
        },
    ]
}
