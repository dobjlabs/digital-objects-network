use pod2::middleware::Hash;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub plugin: Plugin,
    pub classes: Vec<Class>,
    pub actions: Vec<Action>,
    /// Declared dependencies. Optional: with no entries, builders fall back
    /// to scanning the script for qualified sub-action calls and resolving
    /// each plugin by name from an install directory. With entries, the
    /// declaration is authoritative: it must cover the scanned set exactly,
    /// each dependency loads from its `path`, and a declared `module_hash`
    /// is verified per dependency -- so version drift names the offending
    /// import instead of surfacing as a whole-plugin hash mismatch.
    #[serde(default)]
    pub imports: Vec<Import>,
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

#[derive(Debug, Deserialize)]
pub struct Import {
    /// The dependency's plugin name, as it appears in qualified calls.
    pub name: String,
    /// Where the built `.pexe` lives, relative to this manifest's directory.
    /// A build-time convenience only: installed catalogs still resolve
    /// dependencies by name, so the path never needs to exist off the
    /// machine that built the plugin.
    pub path: String,
    /// The dependency batch id to pin. Optional; when present, resolution
    /// fails with a per-import message if the pexe at `path` (or, in an
    /// installed catalog, the plugin of this name) compiles to a different
    /// batch.
    #[serde(default)]
    pub module_hash: Option<Hash>,
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
        assert_eq!(manifest.classes.len(), 1);
        assert_eq!(manifest.actions.len(), 2);
        assert!(manifest.actions[1].hidden);
    }
}
