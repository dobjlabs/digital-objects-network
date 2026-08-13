use pod2::middleware::Hash;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub plugin: Plugin,
    #[serde(default)]
    pub classes: Vec<Class>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Plugins whose actions this pexe's recipes compose, each pinned to
    /// the module hash the recipe was authored against.
    #[serde(default)]
    pub requires: Vec<Require>,
    /// Transactions assembled from other plugins' actions. A pexe that
    /// declares recipes carries no script and compiles to no module of
    /// its own, so it can never define or alter a class.
    #[serde(default)]
    pub recipes: Vec<Recipe>,
}

impl Manifest {
    /// Whether this pexe declares any recipes. A pexe may carry recipes
    /// with or without a script of its own: a recipe can name its own
    /// plugin's actions alongside another plugin's, so a composed
    /// transaction can also produce objects of classes it declares here.
    pub fn is_recipe(&self) -> bool {
        !self.recipes.is_empty()
    }
}

#[derive(Debug, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    /// Absent on a recipe pexe. `pexe build` fills it in for a plugin by
    /// compiling the script.
    #[serde(default)]
    pub module_hash: Option<Hash>,
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

/// A pinned dependency on another installed plugin. The hash is checked
/// at catalog load: a required plugin present at a different hash is a
/// different set of classes, so its actions are not the ones the recipe
/// was written against.
#[derive(Debug, Deserialize)]
pub struct Require {
    pub plugin: String,
    pub module_hash: Hash,
}

#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub emoji: String,
    pub description: String,
    /// Qualified action names (`plugin::Action`) run as sibling
    /// top-level actions of one transaction, in this order. The recipe's
    /// inputs are the steps' inputs concatenated in the same order.
    pub steps: Vec<String>,
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
module_hash = "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06"

[[classes]]
name = "WoodPick"
emoji = "⛏️"
description = "A wood pick that can mine stone while durability remains."

[[actions]]
name = "CraftWoodPick"
emoji = "⛏️"
description = "Combine wood and a stick to craft a wood pick."

[[actions]]
name = "UseWoodPick"
emoji = "⛏️"
description = "Internal durability/work update for wood pick usage."
hidden = true
        "#;
        let manifest: Manifest = toml::from_str(toml_str).unwrap();
        assert!(!manifest.is_recipe());
        assert!(manifest.plugin.module_hash.is_some());
        assert_eq!(manifest.classes.len(), 1);
        assert_eq!(manifest.actions.len(), 2);
        assert!(manifest.actions[1].hidden);
    }

    #[test]
    fn test_recipe_manifest() {
        let toml_str = r#"
[plugin]
name = "swap-log-wood"
version = "0.1.0"

[[requires]]
plugin = "craft-basics"
module_hash = "57631b51fb9a921588d391211f94c0bd8f777aff0a16755bc2dfefb52d6ff5b0"

[[recipes]]
name = "SwapLogWood"
emoji = "🤝"
description = "Re-key one Log and one Wood in a single transaction."
steps = ["craft-basics::ClaimLog", "craft-basics::ClaimWood"]
        "#;
        let manifest: Manifest = toml::from_str(toml_str).unwrap();
        assert!(manifest.is_recipe());
        assert!(
            manifest.plugin.module_hash.is_none(),
            "a recipe compiles to no module"
        );
        assert!(manifest.classes.is_empty());
        assert_eq!(manifest.requires.len(), 1);
        assert_eq!(manifest.requires[0].plugin, "craft-basics");
        assert_eq!(
            manifest.recipes[0].steps,
            vec!["craft-basics::ClaimLog", "craft-basics::ClaimWood"]
        );
    }
}
