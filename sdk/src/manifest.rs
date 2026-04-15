use pod2::middleware::Hash;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub plugin: Plugin,
    pub classes: Vec<Class>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub module_hash: Hash,
}

#[derive(Debug, Deserialize)]
pub struct Class {
    pub name: String,
    pub emoji: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct Action {
    pub name: String,
    pub emoji: String,
    pub description: String,
    #[serde(default)]
    pub hidden: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest() {
        let toml_str = r#"
[plugin]
name = "craft-wood-pick"
version = "0.1.0"
imports = ["craft-wood", "craft-sticks"]
module_hash = "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06"

[[classes]]
name = "WoodPick"
emoji = "⛏️"
description = "A wood pick that can mine stone while durability remains."

[[actions]]
name = "CraftWoodPick"
fn_name = "CraftWoodPick"
emoji = "⛏️"
description = "Combine wood and a stick to craft a wood pick."

[[actions]]
name = "UseWoodPick"
fn_name = "UseWoodPick"
emoji = "⛏️"
description = "Internal durability/work update for wood pick usage."
hidden = true
        "#;
        let manifest: Manifest = toml::from_str(toml_str).unwrap();
        println!("{:#?}", manifest);
    }
}
