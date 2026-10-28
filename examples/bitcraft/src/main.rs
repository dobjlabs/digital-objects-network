#![allow(clippy::useless_format)]

use std::fs::File;
use std::io::{self, Write as _};

#[derive(Debug, Clone, Copy, strum_macros::Display)]
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
// that bits take the plural, BucketOfBits. Action names follow the class
// (CraftBucketOfBits, BucketOfBitsAdd5, ...).
fn bucket_class(resource: Resource) -> String {
    match resource {
        Resource::Bit => format!("BucketOfBits"),
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

// Top-limb difficulty target for a proof-of-work weight. A higher weight
// gives a smaller target, so a longer grind.
fn pow_target(pow: usize) -> usize {
    (1.93e13 - 4.76e12 * ((pow as f64).ln())) as usize
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
    // Holds a count `n` of oil, which is not an object of its own. Nothing
    // adds to or takes from it yet.
    BarrelOfOil,
}

#[derive(Debug, Default)]
struct Action {
    name: String,
    in_resources: Vec<(usize, Resource)>,
    out_resources: Vec<(usize, Resource)>,
    // Buckets the action mutates, taking this many resources out of each.
    // They are not rekeyed: the count only falls, so a bucket never returns
    // to a state whose nullifier is already spent.
    spend_from_buckets: Vec<(usize, Resource)>,
    mut_bucket: Option<Resource>,
    out_bucket: Option<Resource>,
    in_tool: Option<Tool>,
    out_tool: Option<Tool>,
    mut_tool: Option<Tool>,
    rekey: bool,
    // Zero means the action does no proof-of-work.
    pow_target: usize,
    vdf: usize,
    tags: Vec<Tag>,
}

#[derive(Debug, Default)]
enum Tag {
    #[default]
    None,
    SetDurability,
    InheritDurability,
    DecDurability,
    SetLevel,
    SetEmpty,
    // Adds `n_steps` times the mutated pumpjack's level to a barrel of oil,
    // behind 50 VDF steps per step. `n_steps` is caller data, 1 if omitted,
    // and at least 1, so the barrel's count always rises and it needs no
    // rekey.
    PumpOil,
    IncLevel,
    ReqLevel(usize),
    AddToBucket(usize),
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
    // The pocketknife is mutated rather than consumed, and must be rekeyed:
    // a nullifier is derived from the object and its key, so a pocketknife
    // returned unchanged would carry the nullifier its input just spent.
    //
    // Rekeying adds a `key` wildcard, bringing the action predicate to 9
    // wildcards over 5 statements. pod2 caps a predicate at 8 wildcards, and
    // splits a predicate into a chain of smaller ones (each within the
    // wildcard cap) only when it exceeds 5 statements. At exactly 5 nothing
    // is split and the predicate fails to compile, so PadForSplit adds a
    // no-op sixth statement to trigger the split.
    for (pow, out) in [(10, Wood), (20, Stone), (40, Iron)] {
        actions.push(Action {
            name: format!("Gather{out}"),
            mut_tool: Some(Pocketknife),
            rekey: true,
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
        name: format!("CraftBarrelOfOil"),
        in_resources: vec![(5, Iron)],
        out_tool: Some(BarrelOfOil),
        tags: vec![SetEmpty],
        ..Default::default()
    });

    // Bucket Add & Take
    for resource in BUCKET_RESOURCES {
        for n in [1, 5, 10, 50] {
            actions.push(Action {
                name: format!("{}Add{n}", bucket_class(*resource)),
                in_resources: vec![(n, *resource)],
                mut_bucket: Some(*resource),
                rekey: true,
                tags: vec![AddToBucket(n)],
                ..Default::default()
            });
        }

        actions.push(Action {
            name: format!("{}Take1", bucket_class(*resource)),
            out_resources: vec![(1, *resource)],
            mut_bucket: Some(*resource),
            rekey: true,
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
            rekey: true,
            out_resources: vec![(lvl + 1, Stone)],
            pow_target: pow_target(20),
            tags: vec![ReqLevel(lvl)],
            ..Default::default()
        });
        actions.push(Action {
            name: format!("UseMiningDrill{lvl}"),
            mut_tool: Some(MiningDrill),
            rekey: true,
            in_resources: vec![(5, Wood)],
            out_resources: vec![(lvl + 1, Iron)],
            pow_target: pow_target(40),
            tags: vec![ReqLevel(lvl)],
            ..Default::default()
        });
        actions.push(Action {
            name: format!("MineCoal{lvl}"),
            mut_tool: Some(MiningDrill),
            rekey: true,
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
        rekey: true,
        in_resources: vec![(5, Coal)],
        tags: vec![PumpOil],
        ..Default::default()
    });

    // Gem mining
    //
    // An action's output classes are fixed, so a single mining action
    // cannot decide which gem it yields. Mining yields a geode instead, and
    // cracking turns it into the one gem its mined value allows.
    for lvl in 1..=MAX_LEVEL {
        actions.push(Action {
            name: format!("MineGem{lvl}"),
            mut_tool: Some(MiningDrill),
            rekey: true,
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
        vdf: 2,
        ..Default::default()
    });
    actions.push(Action {
        name: format!("BitsFromStone"),
        in_resources: vec![(1, Stone)],
        out_resources: vec![(2, Bit)],
        vdf: 2,
        ..Default::default()
    });
    actions.push(Action {
        name: format!("BitsFromIron"),
        in_resources: vec![(1, Iron)],
        out_resources: vec![(5, Bit)],
        vdf: 2,
        ..Default::default()
    });
    let gem_bits = [100, 2_000, 40_000, 800_000, 16_000_000, 320_000_000];
    for (gem, bits) in GEMS.iter().zip(gem_bits) {
        actions.push(Action {
            name: format!("BitsFrom{gem}"),
            in_resources: vec![(1, *gem)],
            mut_bucket: Some(Bit),
            rekey: true,
            tags: vec![AddToBucket(bits)],
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
            for tag in &action.tags {
                match tag {
                    SetDurability => {
                        writeln!(o, r#"  {name}.set([["durability", 100]]);"#)?;
                    }
                    SetLevel => {
                        writeln!(o, r#"  {name}.set([["level", 1]]);"#)?;
                    }
                    SetEmpty => {
                        writeln!(o, r#"  {name}.set([["n", 0]]);"#)?;
                    }
                    InheritDurability => {
                        writeln!(
                            o,
                            r#"  {name}.set([["durability", {last_in_tool}.durability]]);"#
                        )?;
                    }
                    _ => {}
                }
            }
        }
        if let Some(mut_tool) = action.mut_tool {
            let name = format!("{mut_tool}").to_lowercase();
            writeln!(o, "  var {name} = action.mutate(\"{mut_tool}\");")?;

            if action.rekey {
                writeln!(
                    o,
                    r#"  var key = unsafe {{ action.random() }};
  {name}.update("key", key);"#
                )?;
            }

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
            for i in 0..*n {
                let name = format!("{res}{:03}", i).to_lowercase();
                writeln!(o, "  var {name} = action.input(\"{res}\");")?;
            }
        }
        for (n, res) in &action.out_resources {
            for i in 0..*n {
                let name = format!("{res}{:03}", i).to_lowercase();
                writeln!(o, "  var {name} = action.output(\"{res}\");")?;
                last_out_resource = name;
            }
        }

        for (n, res) in &action.spend_from_buckets {
            let name = format!("{res}bucket").to_lowercase();
            let class = bucket_class(*res);
            writeln!(
                o,
                r#"  var {name} = action.mutate("{class}");
  action.st_gt_eq({name}.n, {n});
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

            if action.rekey {
                writeln!(
                    o,
                    r#"  var key = unsafe {{ action.random() }};
  {name}.update("key", key);"#
                )?;
            }

            for tag in &action.tags {
                match tag {
                    AddToBucket(d) => {
                        writeln!(
                            o,
                            r#"  var n = unsafe {{ {name}.n + {d} }};
  action.st_sum({name}.n, {d}, n);
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
            match tag {
                PumpOil => {
                    let pumpjack = format!("{Pumpjack}").to_lowercase();
                    let barrel = format!("{BarrelOfOil}").to_lowercase();
                    writeln!(
                        o,
                        r#"  var {barrel} = action.mutate("{BarrelOfOil}");
  var n_steps = unsafe {{ arg("n_steps") ?? 1 }};
  action.st_gt_eq(n_steps, 1);
  var pumped = unsafe {{ n_steps * {pumpjack}.level }};
  action.st_product(n_steps, {pumpjack}.level, pumped);
  var n = unsafe {{ {barrel}.n + pumped }};
  action.st_sum({barrel}.n, pumped, n);
  {barrel}.update("n", n);
  var vdf_steps = unsafe {{ n_steps * 50 }};
  action.st_product(n_steps, 50, vdf_steps);
  var work = action.intro_vdf(vdf_steps, {barrel});"#
                    )?;
                }
                SetGemThresholds => {
                    let initializers: Vec<String> = GEMS
                        .iter()
                        .enumerate()
                        .skip(1)
                        .map(|(tier, gem)| {
                            let threshold = action.pow_target / GEM_RARITY.pow(tier as u32);
                            let field = format!("{gem}").to_lowercase();
                            format!(r#"["{field}", action.top_limb_u256({threshold})]"#)
                        })
                        .collect();
                    writeln!(
                        o,
                        "  {last_out_resource}.set([{}]);",
                        initializers.join(", ")
                    )?;
                }
                CrackGeode(tier) => {
                    // The stable identifier is the commitment of the geode as
                    // mined, which is the value its proof-of-work bounded.
                    let geode = format!("{Geode}000").to_lowercase();
                    let mined = format!("{geode}.stable_identifier");
                    if *tier > 0 {
                        let field = format!("{}", GEMS[*tier]).to_lowercase();
                        writeln!(o, "  action.intro_lt_eq_u256({mined}, {geode}.{field});")?;
                    }
                    if let Some(rarer) = GEMS.get(tier + 1) {
                        let field = format!("{rarer}").to_lowercase();
                        writeln!(o, "  action.intro_lt_eq_u256({geode}.{field}, {mined});")?;
                    }
                }
                _ => {}
            }
        }

        if action.pow_target > 0 {
            writeln!(
                o,
                r#"  let target = action.top_limb_u256({n});
  var grind_key = unsafe {{ arg("grind_key") ?? action.pow_obj_grind({var}, target) }};
  {var}.update("key", grind_key);
  action.intro_lt_eq_u256({var}, target);"#,
                n = action.pow_target,
                var = last_out_resource
            )?;
        }

        if action.vdf > 0 {
            writeln!(
                o,
                r#"  var work = action.intro_vdf({n}, {var});"#,
                n = action.vdf,
                var = last_out_resource
            )?;
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
