#![allow(clippy::useless_format)]

use std::fs::File;
use std::io::{self, Write as _};

#[derive(Debug, Clone, Copy, strum_macros::Display)]
enum Resource {
    Wood,
    Stone,
    Iron,
    Stick,
    Bit,
}

static BUCKET_RESOURCES: &[Resource] = &[Resource::Wood, Resource::Stone, Resource::Iron];

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
enum Tool {
    Pocketknife,
    WoodAxe,
    StoneAxe,
    IronAxe,
    Quarry,
    MiningDrill,
}

#[derive(Debug, Default)]
struct Action {
    name: String,
    in_resources: Vec<(usize, Resource)>,
    out_resources: Vec<(usize, Resource)>,
    in_buckets: Vec<(usize, Resource)>,
    mut_bucket: Option<Resource>,
    out_bucket: Option<Resource>,
    in_tool: Option<Tool>,
    out_tool: Option<Tool>,
    mut_tool: Option<Tool>,
    rekey: bool,
    pow: usize,
    vdf: usize,
    tag: Tag,
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
    AddToBucket(usize),
    TakeFromBucket(usize),
    // Emits a no-op statement. pod2 splits a predicate into a chain only once
    // it exceeds the per-predicate statement cap, and an unsplit predicate
    // must fit the wildcard cap on its own; the extra statement forces the
    // split for an action whose wildcards would otherwise overflow.
    PadForSplit,
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
            pow,
            tag: PadForSplit,
            ..Default::default()
        });
    }

    // Craft Buckets
    for resource in BUCKET_RESOURCES {
        actions.push(Action {
            name: format!("CraftBucketOf{resource}"),
            in_resources: vec![(1, Iron)],
            out_bucket: Some(*resource),
            ..Default::default()
        });
    }

    // Bucket Add & Take
    for resource in BUCKET_RESOURCES {
        for n in [1, 5, 10, 50] {
            actions.push(Action {
                name: format!("BucketOf{resource}Add{n}"),
                in_resources: vec![(n, *resource)],
                mut_bucket: Some(*resource),
                rekey: true,
                tag: AddToBucket(n),
                ..Default::default()
            });
        }

        actions.push(Action {
            name: format!("BucketOf{resource}Take1"),
            out_resources: vec![(1, *resource)],
            mut_bucket: Some(*resource),
            rekey: true,
            tag: TakeFromBucket(1),
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
        tag: SetDurability,
        ..Default::default()
    });
    actions.push(Action {
        name: format!("CraftStoneAxe"),
        in_tool: Some(WoodAxe),
        in_resources: vec![(1, Stone)],
        out_tool: Some(StoneAxe),
        tag: InheritDurability,
        ..Default::default()
    });
    actions.push(Action {
        name: format!("CraftIronAxe"),
        in_tool: Some(StoneAxe),
        in_resources: vec![(1, Iron)],
        out_tool: Some(IronAxe),
        tag: InheritDurability,
        ..Default::default()
    });

    // Axe usage
    for (n, axe_type) in [(2, WoodAxe), (3, StoneAxe), (4, IronAxe)] {
        actions.push(Action {
            name: format!("Use{axe_type}"),
            mut_tool: Some(axe_type),
            out_resources: vec![(n, Wood)],
            pow: 10,
            tag: DecDurability,
            ..Default::default()
        });
    }

    // Leveled action
    for lvl in 1..=6 {
        actions.push(Action {
            name: format!("UseQuarry{lvl}"),
            mut_tool: Some(Quarry),
            rekey: true,
            out_resources: vec![(lvl + 1, Stone)],
            pow: 20,
            tag: ReqLevel(lvl),
            ..Default::default()
        });
        actions.push(Action {
            name: format!("UseMiningDrill{lvl}"),
            mut_tool: Some(MiningDrill),
            rekey: true,
            in_resources: vec![(5, Wood)],
            out_resources: vec![(lvl + 1, Iron)],
            pow: 40,
            tag: ReqLevel(lvl),
            ..Default::default()
        });
    }

    // Factory creation
    actions.push(Action {
        name: format!("BuildQuarry"),
        in_resources: vec![(20, Wood), (10, Stone)],
        out_tool: Some(Quarry),
        tag: SetLevel,
        ..Default::default()
    });
    actions.push(Action {
        name: format!("BuildMiningDrill"),
        in_resources: vec![(10, Wood), (50, Stone), (5, Iron)],
        out_tool: Some(MiningDrill),
        tag: SetLevel,
        ..Default::default()
    });
    // Factory level-up
    for lvl in 1..=5 {
        actions.push(Action {
            name: format!("LevelUpQuarry{lvl}"),
            in_buckets: vec![(20 * 2usize.pow(lvl), Wood), (10 * 2usize.pow(lvl), Stone)],
            mut_tool: Some(Quarry),
            tag: IncLevel,
            ..Default::default()
        });
        actions.push(Action {
            name: format!("LevelUpMiningDrill{lvl}"),
            in_buckets: vec![
                (10 * 2usize.pow(lvl), Wood),
                (50 * 2usize.pow(lvl), Stone),
                (5 * 2usize.pow(lvl), Iron),
            ],
            mut_tool: Some(MiningDrill),
            tag: IncLevel,
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
            match action.tag {
                SetDurability => {
                    writeln!(o, r#"  {name}.set([["durability", 100]]);"#)?;
                }
                SetLevel => {
                    writeln!(o, r#"  {name}.set([["level", 1]]);"#)?;
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

            match action.tag {
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

        for (n, res) in &action.in_buckets {
            let name = format!("{res}bucket").to_lowercase();
            writeln!(
                o,
                r#"  var {name} = action.input("BucketOf{res}");
  action.st_equal({name}.n, {n});"#
            )?;
        }

        if let Some(res) = action.out_bucket {
            let name = format!("{res}bucket").to_lowercase();
            writeln!(
                o,
                r#"  var {name} = action.output("BucketOf{res}");
  {name}.set([["n", 0]]);"#
            )?;
        }

        if let Some(res) = action.mut_bucket {
            let name = format!("{res}bucket").to_lowercase();
            writeln!(o, r#"  var {name} = action.mutate("BucketOf{res}");"#)?;

            if action.rekey {
                writeln!(
                    o,
                    r#"  var key = unsafe {{ action.random() }};
  {name}.update("key", key);"#
                )?;
            }

            match action.tag {
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

        if action.pow > 0 {
            let equation = |x: usize| (1.93e13 - 4.76e12 * ((x as f64).ln())) as usize;
            writeln!(
                o,
                r#"  let target = action.top_limb_u256({n});
  var grind_key = unsafe {{ arg("grind_key") ?? action.pow_obj_grind({var}, target) }};
  {var}.update("key", grind_key);
  action.intro_lt_eq_u256({var}, target);"#,
                n = equation(action.pow),
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
