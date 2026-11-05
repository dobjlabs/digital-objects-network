#![allow(clippy::useless_format)]

use std::fs::File;
use std::io::{self, Write as _};

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
enum Resource {
    Wood,
    Stone,
    Iron,
    Coal,
    Stick,
    Bit,
    Geode,
    Quartz,
    Amethyst,
    Sapphire,
    Emerald,
    Ruby,
    Diamond,
    // Exists only as the count `n` of a barrel of oil: there are no Oil
    // objects, and no Oil class.
    Oil,
}

static BUCKET_RESOURCES: &[Resource] = &[
    Resource::Wood,
    Resource::Stone,
    Resource::Iron,
    Resource::Bit,
    Resource::Coal,
    Resource::Quartz,
];

// The class of the bucket holding `resource`: BucketOf<Resource>, except
// that bits take the plural, BucketOfBits, and oil is held in a
// BarrelOfOil. Action names follow the class (CraftBucketOfBits,
// BucketOfBitsAdd5, ...).
fn bucket_class(resource: Resource) -> String {
    match resource {
        Resource::Bit => format!("BucketOfBits"),
        Resource::Oil => format!("BarrelOfOil"),
        _ => format!("BucketOf{resource}"),
    }
}

// Most common first. Each gem's band of mined values is GEM_RARITY times
// narrower than the one before it.
static GEMS: &[Resource] = &[
    Resource::Quartz,
    Resource::Amethyst,
    Resource::Sapphire,
    Resource::Emerald,
    Resource::Ruby,
    Resource::Diamond,
];
const GEM_RARITY: usize = 16;

// The highest level a quarry, mining drill or pumpjack reaches.
const MAX_LEVEL: usize = 10;

// At drill level 1, mining a geode grinds this many times as long as
// gathering wood; at level N, 1/N of that.
const GEM_POW_FACTOR: usize = 1024;

// For each gem rarer than quartz, the highest mined value that still cracks
// into that gem or a rarer one, as `set` entries named after the gem.
fn gem_threshold_initializers(pow_target: usize) -> Vec<String> {
    GEMS.iter()
        .enumerate()
        .skip(1)
        .map(|(tier, gem)| {
            let threshold = pow_target / GEM_RARITY.pow(tier as u32);
            let field = format!("{gem}").to_lowercase();
            format!(r#"["{field}", action.top_limb_u256({threshold})]"#)
        })
        .collect()
}

// Top-limb difficulty target for a proof-of-work weight. A higher weight
// gives a smaller target, so a longer grind. A zero target would read as "no
// proof-of-work", so weights past where the curve reaches zero are refused.
fn pow_target(pow: usize) -> usize {
    let target = 1.93e13 - 4.76e12 * (pow as f64).ln();
    assert!(
        target >= 1.0,
        "proof-of-work weight {pow} gives no positive target"
    );
    target as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
enum Tool {
    Pocketknife,
    WoodAxe,
    StoneAxe,
    IronAxe,
    Quarry,
    MiningDrill,
    Pumpjack,
}

impl Tool {
    // Tools whose actions mine: each carries a `nonce` that the proof-of-work
    // of its next use is tied to.
    fn mines(self) -> bool {
        !matches!(self, Tool::Pumpjack)
    }
}

#[derive(Debug, Default)]
struct Action {
    name: String,
    in_resources: Vec<(usize, Resource)>,
    out_resources: Vec<(usize, Resource)>,
    // Buckets the action mutates, taking this many resources out of each.
    spend_from_buckets: Vec<(usize, Resource)>,
    mut_bucket: Option<Resource>,
    out_bucket: Option<Resource>,
    in_tool: Option<Tool>,
    out_tool: Option<Tool>,
    mut_tool: Option<Tool>,
    // Zero means the action does no proof-of-work.
    pow_target: usize,
    // Reads the caller data `n_steps` (1 if omitted, at least 1), which
    // per-step amounts multiply.
    steps: bool,
    vdf: Vdf,
    tags: Vec<Tag>,
}

// VDF iterations over the action's last output, or its mutated bucket when
// it has no output.
#[derive(Debug, Default)]
enum Vdf {
    #[default]
    None,
    Iterations(usize),
    // This many iterations per step; the action must read `n_steps`.
    PerStep(usize),
}

// What `AddToBucket` adds.
#[derive(Debug)]
enum Amount {
    Fixed(usize),
    // `n_steps` times the level of the tool the action mutates.
    StepsTimesToolLevel,
}

#[derive(Debug, Default)]
enum Tag {
    #[default]
    None,
    SetDurability,
    InheritDurability,
    DecDurability,
    SetLevel,
    IncLevel,
    ReqLevel(usize),
    AddToBucket(Amount),
    TakeFromBucket(usize),
    // Emits a no-op statement. pod2 splits a predicate into a chain only once
    // it exceeds the per-predicate statement cap, and an unsplit predicate
    // must fit the wildcard cap on its own; the extra statement forces the
    // split for an action whose wildcards would otherwise overflow.
    PadForSplit,
    // Records on the output geode, for each gem rarer than quartz, the
    // highest mined value that still cracks into that gem or a rarer one.
    // The bands scale with the action's own target, so they are written at
    // mining time rather than recomputed when the geode is cracked.
    SetGemThresholds,
    // Proves that the input geode's mined value lies in the band of the gem
    // at this index in GEMS.
    CrackGeode(usize),
}

// Gives the mutated object `name` a new key: the argument `key_arg` when the
// caller supplies it, otherwise a random one. Every mutated object is
// rekeyed: its next state is then never one already spent, and whoever holds
// a copy of its previous state cannot predict the next one.
fn rekey(o: &mut File, name: &str, key_arg: Option<&str>) -> Result<(), io::Error> {
    let key = match key_arg {
        Some(key_arg) => format!(r#"arg("{key_arg}") ?? action.random()"#),
        None => "action.random()".to_string(),
    };
    writeln!(
        o,
        r#"  var {name}_key = unsafe {{ {key} }};
  {name}.update("key", {name}_key);"#
    )
}

fn main() -> Result<(), io::Error> {
    let mut rhai_output = File::create("plugin.rhai")?;
    let mut manifest_output = File::create("manifest.toml")?;

    use Resource::*;
    use Tag::*;
    use Tool::*;

    let mut actions = Vec::new();
    actions.push(Action {
        name: format!("CraftPocketknife"),
        out_tool: Some(Pocketknife),
        ..Default::default()
    });

    // Gather
    //
    // Rekeying the pocketknife adds a key wildcard, bringing the action
    // predicate to 9 wildcards over 5 statements. pod2 caps a predicate at 8
    // wildcards, and splits a predicate into a chain of smaller ones (each
    // within the wildcard cap) only when it exceeds 5 statements. At exactly
    // 5 nothing is split and the predicate fails to compile, so PadForSplit
    // adds a no-op sixth statement to trigger the split.
    for (pow, out) in [(10, Wood), (20, Stone), (40, Iron)] {
        actions.push(Action {
            name: format!("Gather{out}"),
            mut_tool: Some(Pocketknife),
            out_resources: vec![(1, out)],
            pow_target: pow_target(pow),
            tags: vec![PadForSplit],
            ..Default::default()
        });
    }

    // Craft Buckets
    for resource in BUCKET_RESOURCES {
        actions.push(Action {
            name: format!("Craft{}", bucket_class(*resource)),
            in_resources: vec![(1, Iron)],
            out_bucket: Some(*resource),
            ..Default::default()
        });
    }

    actions.push(Action {
        name: format!("Craft{}", bucket_class(Oil)),
        in_resources: vec![(5, Iron)],
        out_bucket: Some(Oil),
        ..Default::default()
    });

    // Bucket Add & Take
    for resource in BUCKET_RESOURCES {
        for n in [1, 5, 10, 50] {
            actions.push(Action {
                name: format!("{}Add{n}", bucket_class(*resource)),
                in_resources: vec![(n, *resource)],
                mut_bucket: Some(*resource),
                tags: vec![AddToBucket(Amount::Fixed(n))],
                ..Default::default()
            });
        }

        actions.push(Action {
            name: format!("{}Take1", bucket_class(*resource)),
            out_resources: vec![(1, *resource)],
            mut_bucket: Some(*resource),
            tags: vec![TakeFromBucket(1)],
            ..Default::default()
        });
    }

    actions.push(Action {
        name: format!("CraftStick"),
        in_resources: vec![(1, Wood)],
        out_resources: vec![(2, Stick)],
        ..Default::default()
    });

    // Craft Axe types
    actions.push(Action {
        name: format!("CraftWoodAxe"),
        in_resources: vec![(1, Wood), (2, Stick)],
        out_tool: Some(WoodAxe),
        tags: vec![SetDurability],
        ..Default::default()
    });
    actions.push(Action {
        name: format!("CraftStoneAxe"),
        in_tool: Some(WoodAxe),
        in_resources: vec![(1, Stone)],
        out_tool: Some(StoneAxe),
        tags: vec![InheritDurability],
        ..Default::default()
    });
    actions.push(Action {
        name: format!("CraftIronAxe"),
        in_tool: Some(StoneAxe),
        in_resources: vec![(1, Iron)],
        out_tool: Some(IronAxe),
        tags: vec![InheritDurability],
        ..Default::default()
    });

    // Axe usage
    for (n, axe_type) in [(2, WoodAxe), (3, StoneAxe), (4, IronAxe)] {
        actions.push(Action {
            name: format!("Use{axe_type}"),
            mut_tool: Some(axe_type),
            out_resources: vec![(n, Wood)],
            pow_target: pow_target(10),
            tags: vec![DecDurability],
            ..Default::default()
        });
    }

    // Leveled action
    for lvl in 1..=MAX_LEVEL {
        actions.push(Action {
            name: format!("UseQuarry{lvl}"),
            mut_tool: Some(Quarry),
            out_resources: vec![(lvl + 1, Stone)],
            pow_target: pow_target(20),
            tags: vec![ReqLevel(lvl)],
            ..Default::default()
        });
        actions.push(Action {
            name: format!("MineIron{lvl}"),
            mut_tool: Some(MiningDrill),
            in_resources: vec![(5, Wood)],
            out_resources: vec![(lvl + 1, Iron)],
            pow_target: pow_target(40),
            tags: vec![ReqLevel(lvl)],
            ..Default::default()
        });
        actions.push(Action {
            name: format!("MineCoal{lvl}"),
            mut_tool: Some(MiningDrill),
            in_resources: vec![(5, Wood)],
            out_resources: vec![(lvl, Coal)],
            pow_target: pow_target(40),
            tags: vec![ReqLevel(lvl)],
            ..Default::default()
        });
    }

    // Factory creation
    actions.push(Action {
        name: format!("BuildQuarry"),
        in_resources: vec![(20, Wood), (10, Stone)],
        out_tool: Some(Quarry),
        tags: vec![SetLevel],
        ..Default::default()
    });
    actions.push(Action {
        name: format!("BuildMiningDrill"),
        in_resources: vec![(10, Wood), (50, Stone), (5, Iron)],
        out_tool: Some(MiningDrill),
        tags: vec![SetLevel],
        ..Default::default()
    });
    // Stone and iron come from buckets: taken loose, the build's 150-odd
    // inputs overflow the proof's public statements.
    actions.push(Action {
        name: format!("BuildPumpjack"),
        in_resources: vec![(1, Quartz)],
        spend_from_buckets: vec![(50, Stone), (100, Iron)],
        out_tool: Some(Pumpjack),
        tags: vec![SetLevel],
        ..Default::default()
    });
    // Factory level-up
    for lvl in 1..MAX_LEVEL as u32 {
        actions.push(Action {
            name: format!("LevelUpQuarry{lvl}"),
            spend_from_buckets: vec![(20 * 2usize.pow(lvl), Wood), (10 * 2usize.pow(lvl), Stone)],
            mut_tool: Some(Quarry),
            tags: vec![ReqLevel(lvl as usize), IncLevel],
            ..Default::default()
        });
        actions.push(Action {
            name: format!("LevelUpMiningDrill{lvl}"),
            spend_from_buckets: vec![
                (10 * 2usize.pow(lvl), Wood),
                (50 * 2usize.pow(lvl), Stone),
                (5 * 2usize.pow(lvl), Iron),
            ],
            mut_tool: Some(MiningDrill),
            tags: vec![ReqLevel(lvl as usize), IncLevel],
            ..Default::default()
        });
        actions.push(Action {
            name: format!("LevelUpPumpjack{lvl}"),
            spend_from_buckets: vec![
                (50 * 2usize.pow(lvl), Stone),
                (100 * 2usize.pow(lvl), Iron),
                (2usize.pow(lvl), Quartz),
            ],
            mut_tool: Some(Pumpjack),
            tags: vec![ReqLevel(lvl as usize), IncLevel],
            ..Default::default()
        });
    }

    // Oil pumping
    actions.push(Action {
        name: format!("PumpOil"),
        mut_tool: Some(Pumpjack),
        in_resources: vec![(5, Coal)],
        mut_bucket: Some(Oil),
        steps: true,
        vdf: Vdf::PerStep(50),
        tags: vec![AddToBucket(Amount::StepsTimesToolLevel)],
        ..Default::default()
    });

    // Gem mining
    //
    // An action's output classes are fixed, so a single mining action
    // cannot decide which gem it yields. Mining yields a geode instead, and
    // cracking turns it into the one gem its mined value allows.
    for lvl in 1..=MAX_LEVEL {
        actions.push(Action {
            name: format!("MineGeode{lvl}"),
            mut_tool: Some(MiningDrill),
            in_resources: vec![(5, Wood)],
            out_resources: vec![(1, Geode)],
            pow_target: pow_target(10) * lvl / GEM_POW_FACTOR,
            tags: vec![ReqLevel(lvl), SetGemThresholds],
            ..Default::default()
        });
    }
    for (tier, gem) in GEMS.iter().enumerate() {
        actions.push(Action {
            name: format!("CrackGeode{gem}"),
            in_resources: vec![(1, Geode)],
            out_resources: vec![(1, *gem)],
            tags: vec![CrackGeode(tier)],
            ..Default::default()
        });
    }

    // Bit creation
    actions.push(Action {
        name: format!("BitsFromWood"),
        in_resources: vec![(1, Wood)],
        out_resources: vec![(1, Bit)],
        vdf: Vdf::Iterations(2),
        ..Default::default()
    });
    actions.push(Action {
        name: format!("BitsFromStone"),
        in_resources: vec![(1, Stone)],
        out_resources: vec![(2, Bit)],
        vdf: Vdf::Iterations(2),
        ..Default::default()
    });
    actions.push(Action {
        name: format!("BitsFromIron"),
        in_resources: vec![(1, Iron)],
        out_resources: vec![(5, Bit)],
        vdf: Vdf::Iterations(2),
        ..Default::default()
    });
    let gem_bits = [100, 2_000, 40_000, 800_000, 16_000_000, 320_000_000];
    for (gem, bits) in GEMS.iter().zip(gem_bits) {
        actions.push(Action {
            name: format!("BitsFrom{gem}"),
            in_resources: vec![(1, *gem)],
            mut_bucket: Some(Bit),
            tags: vec![AddToBucket(Amount::Fixed(bits))],
            ..Default::default()
        });
    }

    let mut last_out_resource = String::new();
    let mut last_in_tool = String::new();
    let o = &mut rhai_output;
    for action in &actions {
        writeln!(o, "fn {}(action) {{", action.name)?;

        if let Some(in_tool) = action.in_tool {
            let name = format!("{in_tool}").to_lowercase();
            writeln!(o, "  var {name} = action.input(\"{in_tool}\");")?;
            last_in_tool = name;
        }
        if let Some(out_tool) = action.out_tool {
            let name = format!("{out_tool}").to_lowercase();
            writeln!(o, "  var {name} = action.output(\"{out_tool}\");")?;
            let mut initializers: Vec<String> = action
                .tags
                .iter()
                .filter_map(|tag| match tag {
                    SetDurability => Some(format!(r#"["durability", 100]"#)),
                    SetLevel => Some(format!(r#"["level", 1]"#)),
                    InheritDurability => {
                        Some(format!(r#"["durability", {last_in_tool}.durability]"#))
                    }
                    _ => Option::None,
                })
                .collect();
            // The grounding block's hash is unknown until shortly before the
            // tool exists, so no work toward its first use can start earlier.
            if out_tool.mines() {
                initializers.push(format!(r#"["nonce", state_header.block_hash]"#));
            }
            if !initializers.is_empty() {
                writeln!(o, "  {name}.set([{}]);", initializers.join(", "))?;
            }
        }
        if let Some(mut_tool) = action.mut_tool {
            let name = format!("{mut_tool}").to_lowercase();
            writeln!(o, "  var {name} = action.mutate(\"{mut_tool}\");")?;
            // A nonce searched for before the action is only valid for the
            // tool state it was searched against, new key included.
            let key_arg = (action.pow_target > 0).then(|| format!("{name}_key"));
            rekey(o, &name, key_arg.as_deref())?;

            for tag in &action.tags {
                match tag {
                    DecDurability => {
                        writeln!(
                            o,
                            r#"  action.st_gt({name}.durability, 0);
  var durability = unsafe {{ {name}.durability - 1 }};
  action.st_sum(durability, 1, {name}.durability);
  {name}.update("durability", durability);"#
                        )?;
                    }
                    IncLevel => {
                        writeln!(
                            o,
                            r#"  var level = unsafe {{ {name}.level + 1 }};
  action.st_sum(1, {name}.level, level);
  {name}.update("level", level);"#
                        )?;
                    }
                    ReqLevel(lvl) => {
                        writeln!(o, r#"  action.st_equal({name}.level, {lvl});"#)?;
                    }
                    PadForSplit => {
                        writeln!(o, r#"  action.st_equal(1, 1);"#)?;
                    }
                    _ => {}
                }
            }
        }

        for (n, res) in &action.in_resources {
            if *res == Oil {
                continue;
            }
            for i in 0..*n {
                let name = format!("{res}{:03}", i).to_lowercase();
                writeln!(o, "  var {name} = action.input(\"{res}\");")?;
            }
        }
        for (n, res) in &action.out_resources {
            if *res == Oil {
                continue;
            }
            for i in 0..*n {
                let name = format!("{res}{:03}", i).to_lowercase();
                writeln!(o, "  var {name} = action.output(\"{res}\");")?;
                last_out_resource = name;
            }
        }

        if action.steps {
            writeln!(
                o,
                r#"  var n_steps = unsafe {{ arg("n_steps") ?? 1 }};
  action.st_gt_eq(n_steps, 1);"#
            )?;
        }

        for (n, res) in &action.spend_from_buckets {
            let name = format!("{res}bucket").to_lowercase();
            let class = bucket_class(*res);
            writeln!(o, r#"  var {name} = action.mutate("{class}");"#)?;
            rekey(o, &name, Option::None)?;
            writeln!(
                o,
                r#"  action.st_gt_eq({name}.n, {n});
  var {name}_n = unsafe {{ {name}.n - {n} }};
  action.st_sum({name}_n, {n}, {name}.n);
  {name}.update("n", {name}_n);"#
            )?;
        }

        if let Some(res) = action.out_bucket {
            let name = format!("{res}bucket").to_lowercase();
            let class = bucket_class(res);
            writeln!(
                o,
                r#"  var {name} = action.output("{class}");
  {name}.set([["n", 0]]);"#
            )?;
        }

        if let Some(res) = action.mut_bucket {
            let name = format!("{res}bucket").to_lowercase();
            let class = bucket_class(res);
            writeln!(o, r#"  var {name} = action.mutate("{class}");"#)?;
            rekey(o, &name, Option::None)?;

            for tag in &action.tags {
                match tag {
                    AddToBucket(amount) => {
                        let added = match amount {
                            Amount::Fixed(d) => format!("{d}"),
                            Amount::StepsTimesToolLevel => {
                                let tool = action
                                    .mut_tool
                                    .expect("a per-level amount needs a mutated tool");
                                let tool = format!("{tool}").to_lowercase();
                                writeln!(
                                    o,
                                    r#"  var added = unsafe {{ n_steps * {tool}.level }};
  action.st_product(n_steps, {tool}.level, added);"#
                                )?;
                                format!("added")
                            }
                        };
                        writeln!(
                            o,
                            r#"  var n = unsafe {{ {name}.n + {added} }};
  action.st_sum({name}.n, {added}, n);
  {name}.update("n", n);"#
                        )?;
                    }
                    TakeFromBucket(d) => {
                        writeln!(
                            o,
                            r#"  action.st_gt_eq({name}.n, {d});
  var n = unsafe {{ {name}.n - {d} }};
  action.st_sum(n, {d}, {name}.n);
  {name}.update("n", n);"#
                        )?;
                    }
                    _ => {}
                }
            }
        }

        for tag in &action.tags {
            if let CrackGeode(tier) = tag {
                let geode = format!("{Geode}000").to_lowercase();
                let mined = format!("{geode}.mined");
                if *tier > 0 {
                    let field = format!("{}", GEMS[*tier]).to_lowercase();
                    writeln!(o, "  action.intro_lt_eq_u256({mined}, {geode}.{field});")?;
                }
                if let Some(rarer) = GEMS.get(tier + 1) {
                    let field = format!("{rarer}").to_lowercase();
                    writeln!(o, "  action.intro_lt_eq_u256({geode}.{field}, {mined});")?;
                }
            }
        }

        if action.pow_target > 0 {
            // The grind runs over the tool's nonce, after every other update
            // to the tool, so the work is tied to that tool's state and each
            // use's work can only start once the previous use is mined.
            let tool = action
                .mut_tool
                .filter(|tool| tool.mines())
                .expect("a mining action mutates the tool it mines with");
            let tool = format!("{tool}").to_lowercase();
            writeln!(
                o,
                r#"  let target = action.top_limb_u256({n});
  var grind_nonce = unsafe {{ arg("grind_nonce") ?? action.pow_obj_grind({tool}, "nonce", target) }};
  {tool}.update("nonce", grind_nonce);
  action.intro_lt_eq_u256({tool}, target);"#,
                n = action.pow_target,
            )?;
            if action
                .tags
                .iter()
                .any(|tag| matches!(tag, SetGemThresholds))
            {
                // The geode records the tool's commitment, the value the
                // proof-of-work bounded, which decides the gem it cracks into.
                let mut initializers = vec![format!(r#"["mined", commitment({tool})]"#)];
                initializers.extend(gem_threshold_initializers(action.pow_target));
                writeln!(
                    o,
                    "  {last_out_resource}.set([{}]);",
                    initializers.join(", ")
                )?;
            }
        }

        let vdf_input = || {
            if action.out_resources.is_empty() {
                let res = action
                    .mut_bucket
                    .expect("a VDF needs an output or a mutated bucket");
                format!("{res}bucket").to_lowercase()
            } else {
                last_out_resource.clone()
            }
        };
        match action.vdf {
            Vdf::None => {}
            Vdf::Iterations(n) => {
                let input = vdf_input();
                writeln!(o, r#"  var work = action.intro_vdf({n}, {input});"#)?;
            }
            Vdf::PerStep(n) => {
                assert!(action.steps, "a per-step VDF needs n_steps");
                let input = vdf_input();
                writeln!(
                    o,
                    r#"  var vdf_steps = unsafe {{ n_steps * {n} }};
  action.st_product(n_steps, {n}, vdf_steps);
  var work = action.intro_vdf(vdf_steps, {input});"#
                )?;
            }
        }
        writeln!(o, "}}\n")?;
    }

    let o = &mut manifest_output;
    writeln!(
        o,
        r#"[plugin]
name = "bitcraft"
version = "0.1.0"
# Rewritten by `cargo run -p pexe -- build game`.
module_hash = "8fddf7843a56a85bb8db6b55b3df3737ae67eeff37724f130cc02387bb1048a2"

[[classes]]
name = "Wood"
emoji = "🌲"
description = "-"

[[classes]]
name = "Stone"
emoji = "🌲"
description = "-"

[[classes]]
name = "Iron"
emoji = "🌲"
description = "-"

[[classes]]
name = "Coal"
emoji = "🌲"
description = "-"

[[classes]]
name = "Geode"
emoji = "🌲"
description = "-"

[[classes]]
name = "Quartz"
emoji = "🌲"
description = "-"

[[classes]]
name = "Amethyst"
emoji = "🌲"
description = "-"

[[classes]]
name = "Sapphire"
emoji = "🌲"
description = "-"

[[classes]]
name = "Emerald"
emoji = "🌲"
description = "-"

[[classes]]
name = "Ruby"
emoji = "🌲"
description = "-"

[[classes]]
name = "Diamond"
emoji = "🌲"
description = "-"

[[classes]]
name = "Stick"
emoji = "🌲"
description = "-"

[[classes]]
name = "BucketOfWood"
emoji = "🌲"
description = "-"

[[classes]]
name = "BucketOfStone"
emoji = "🌲"
description = "-"

[[classes]]
name = "BucketOfIron"
emoji = "🌲"
description = "-"

[[classes]]
name = "BucketOfBits"
emoji = "🌲"
description = "-"

[[classes]]
name = "BucketOfCoal"
emoji = "🌲"
description = "-"

[[classes]]
name = "BucketOfQuartz"
emoji = "🌲"
description = "-"

[[classes]]
name = "BarrelOfOil"
emoji = "🌲"
description = "-"

[[classes]]
name = "Pocketknife"
emoji = "🌲"
description = "-"

[[classes]]
name = "WoodAxe"
emoji = "🌲"
description = "-"

[[classes]]
name = "StoneAxe"
emoji = "🌲"
description = "-"

[[classes]]
name = "IronAxe"
emoji = "🌲"
description = "-"

[[classes]]
name = "Quarry"
emoji = "🌲"
description = "-"

[[classes]]
name = "MiningDrill"
emoji = "🌲"
description = "-"

[[classes]]
name = "Pumpjack"
emoji = "🌲"
description = "-"

[[classes]]
name = "Bit"
emoji = "🌲"
description = "-"
"#
    )?;

    for action in &actions {
        writeln!(
            o,
            r#"[[actions]]
name = "{}"
emoji = "⛏️"
description = "-"
"#,
            action.name
        )?;
    }

    Ok(())
}
