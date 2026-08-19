//! Action catalog backed by `.pexe` plugin archives.
//!
//! At construction time, each installed `.pexe` is unpacked and its script compiled
//! via `Sdk::load_module_from_src_manifest` (which enforces the manifest's
//! `module_hash`). The compiled module is used to derive action/class hashes and
//! the podlang source shown in the GUI.
//!
//! Classes and actions are keyed by [`QualifiedName`] (`<plugin>::<name>`
//! when printed). Two plugins may declare a class or action with the same
//! bare name; they stay distinct because every internal map keys on the full
//! `QualifiedName` and because their on-chain `Is{class}` predicate hashes
//! differ (each module has a unique `module_hash`). A plugin interacts with
//! another plugin's classes only by declaring the plugin in its manifest's
//! `[[imports]]` and calling its actions via `subaction("<plugin>::<Action>")`;
//! plugins load in dependency order and an unresolvable or hash-mismatched
//! import fails the whole catalog load.
//!
//! The compiled [`sdk::SdkModule`] is not kept — it holds a `Rc<Engine>` and is
//! therefore `!Send`. `execute_action` re-loads the script from its stored bytes
//! on demand, matching the per-call pattern used before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use payload::decode_hash_hex;
use pod2::middleware::Hash;
use sdk::{Sdk, SpendableObject, SpendableObjects, manifest::Manifest};
use txlib::GroundingWitness;

use crate::catalog::{ActionCatalog, CatalogClass, extract_predicate};
use wire_types::{ActionSummary, ClassRef, QualifiedName};

struct Plugin {
    #[allow(dead_code)]
    path: PathBuf,
    manifest: Manifest,
    script: String,
}

pub struct PexeCatalog {
    plugins: Vec<Plugin>,
    actions: Vec<ActionSummary>,
    actions_by_name: HashMap<QualifiedName, ActionSummary>,
    /// Maps qualified action -> plugin index in `plugins`.
    action_plugin_idx: HashMap<QualifiedName, usize>,
    classes: Vec<CatalogClass>,
    classes_by_name: HashMap<QualifiedName, CatalogClass>,
    classes_by_hash: HashMap<Hash, QualifiedName>,
    combined_podlang_src: String,
    mock_proofs: bool,
}

impl PexeCatalog {
    /// Scan `actions_dir` for `.pexe` files, unpack them, and assemble the catalog.
    pub fn load(actions_dir: &Path) -> Result<Self> {
        let plugins = discover_plugins(actions_dir)?;
        Self::from_plugins(plugins, false)
    }

    /// Assemble a catalog from already-loaded plugin sources. Used by tests that
    /// pack plugin bytes in-memory.
    pub fn from_bytes<I>(pexe_entries: I, mock_proofs: bool) -> Result<Self>
    where
        I: IntoIterator<Item = (PathBuf, Vec<u8>)>,
    {
        let mut plugins = Vec::new();
        for (path, bytes) in pexe_entries {
            plugins.push(load_plugin_from_bytes(path, &bytes)?);
        }
        Self::from_plugins(plugins, mock_proofs)
    }

    fn from_plugins(plugins: Vec<Plugin>, mock_proofs: bool) -> Result<Self> {
        // Validate plugin.name early: it ends up as the `plugin_name`
        // component of every `QualifiedName`, in `.dobj` filename prefixes,
        // and in GUI labels. The allowlist is filename-safe on every OS we
        // target and rules out `:` (which would let a name straddle the
        // `::` separator when callers stringify), and any path-significant
        // chars (`/`, `\`, `..`) that could otherwise let a malicious or
        // misconfigured plugin escape the objects directory.
        let mut seen_plugin_names: HashMap<String, usize> = HashMap::new();
        for (idx, plugin) in plugins.iter().enumerate() {
            let name = &plugin.manifest.plugin.name;
            validate_plugin_name(name).map_err(|err| {
                anyhow!(
                    "invalid plugin name {name:?} in {}: {err}",
                    plugin.path.display()
                )
            })?;
            if let Some(prior) = seen_plugin_names.insert(name.clone(), idx) {
                return Err(anyhow!(
                    "duplicate plugin name {name:?}: already registered by {} (other entry at index {prior})",
                    plugins[prior].path.display(),
                ));
            }
        }

        let sdk = Sdk::default();
        let mut resolver = plugin_resolver(&sdk, &plugins);

        let mut all_actions: Vec<ActionSummary> = Vec::new();
        let mut classes_in_order: Vec<CatalogClass> = Vec::new();
        let mut combined_podlang = String::new();
        let mut action_plugin_idx: HashMap<QualifiedName, usize> = HashMap::new();

        for (plugin_idx, plugin) in plugins.iter().enumerate() {
            let plugin_name = plugin.manifest.plugin.name.clone();
            let module = resolver
                .load(&plugin_name)
                .map_err(|err| anyhow!("failed to load plugin {plugin_name}: {err}"))?;
            let podlang_src = module.podlang_src().to_string();
            if !combined_podlang.is_empty() {
                combined_podlang.push_str("\n// ---\n");
            }
            combined_podlang.push_str(&format!("// plugin: {plugin_name}\n{podlang_src}"));

            // Per-plugin class hash map. Module-scoped: a `Wood` class in
            // another plugin has a different IsWood predicate hash and lives
            // in a different `class_hashes` map below.
            let mut class_hashes: HashMap<String, Hash> = HashMap::new();
            for class in module.classes() {
                let hash = module.class_hash(&class.name).ok_or_else(|| {
                    anyhow!(
                        "plugin {plugin_name}: class {} has no compiled hash",
                        class.name
                    )
                })?;
                class_hashes.insert(class.name.clone(), hash);
            }

            // Build CatalogClass entries from this plugin's classes.
            let class_meta_by_name: HashMap<&str, &sdk::manifest::Class> = plugin
                .manifest
                .classes
                .iter()
                .map(|c| (c.name.as_str(), c))
                .collect();

            for class in module.classes() {
                let bare = &class.name;
                let qname = QualifiedName::new(plugin_name.clone(), bare.clone());
                let class_hash = class_hashes[bare];
                let meta = class_meta_by_name.get(bare.as_str());
                let predicate_source = extract_predicate(&podlang_src, &format!("Is{bare}"))
                    .unwrap_or_else(|| format!("Is{bare}(state) = OR(...)"));
                classes_in_order.push(CatalogClass {
                    class: qname,
                    emoji: meta.map_or("📦", |m| m.emoji.as_str()).to_string(),
                    hash: format!("{:#}", class_hash),
                    description: meta
                        .map_or("Unknown class object", |m| m.description.as_str())
                        .to_string(),
                    produced_by: Vec::new(), // filled in second pass
                    consumed_by: Vec::new(), // filled in second pass
                    predicate_source,
                });
            }

            // Build ActionSummary rows. Each input/output class is resolved
            // against this plugin's own class set; cross-plugin references
            // are rejected. Hidden actions are still recorded so their
            // qualified name routes back to this plugin via execute_action.
            let action_meta_by_name: HashMap<&str, &sdk::manifest::Action> = plugin
                .manifest
                .actions
                .iter()
                .map(|a| (a.name.as_str(), a))
                .collect();

            for action in module.actions() {
                let bare = action.name.clone();
                let qname = QualifiedName::new(plugin_name.clone(), bare.clone());
                if let Some(prior) = action_plugin_idx.insert(qname.clone(), plugin_idx) {
                    return Err(anyhow!(
                        "internal: duplicate action qualified name {qname} (already mapped to plugin idx {prior})"
                    ));
                }

                let meta = action_meta_by_name.get(bare.as_str());
                // A class spliced in from an imported sub-action resolves
                // against its defining plugin's module, reached through
                // this module's import graph.
                let resolve_class = |r: &sdk::ActionObjectRef| -> Result<ClassRef> {
                    let owner = r.plugin.clone().unwrap_or_else(|| plugin_name.clone());
                    let hash = module
                        .defining_module(r.plugin.as_deref())
                        .and_then(|defining| defining.class_hash(&r.class))
                        .ok_or_else(|| {
                            anyhow!(
                                "plugin {plugin_name}: action {bare} references class {r}, \
                                 which has no compiled hash in plugin {owner}"
                            )
                        })?;
                    Ok(ClassRef {
                        class: QualifiedName::new(owner, r.class.clone()),
                        hash: format!("{:#}", hash),
                    })
                };

                let total_inputs = action
                    .total_inputs()
                    .map(resolve_class)
                    .collect::<Result<Vec<_>>>()?;
                let total_outputs = action
                    .total_outputs()
                    .map(resolve_class)
                    .collect::<Result<Vec<_>>>()?;

                if meta.is_some_and(|m| m.hidden) {
                    continue;
                }

                let action_hash = module
                    .action_hash(&bare)
                    .map(|h| format!("{:#}", h))
                    .unwrap_or_default();
                // Action predicates use the bare action name (no `Is`
                // prefix like classes get).
                let predicate_source = extract_predicate(&podlang_src, &bare)
                    .unwrap_or_else(|| format!("{bare}(state) = AND(...)"));
                all_actions.push(ActionSummary {
                    action: qname,
                    emoji: meta.map_or("⚙️", |m| m.emoji.as_str()).to_string(),
                    hash: action_hash,
                    description: meta
                        .map_or("Pexe action", |m| m.description.as_str())
                        .to_string(),
                    total_inputs,
                    total_outputs,
                    predicate_source,
                });
            }
        }

        // Second pass: fill produced_by / consumed_by per class.
        for class in classes_in_order.iter_mut() {
            class.produced_by = all_actions
                .iter()
                .filter(|a| a.total_outputs.iter().any(|r| r.class == class.class))
                .map(|a| a.action.clone())
                .collect();
            class.consumed_by = all_actions
                .iter()
                .filter(|a| a.total_inputs.iter().any(|r| r.class == class.class))
                .map(|a| a.action.clone())
                .collect();
        }

        // Deterministic GUI order: sort by display name, then plugin.
        classes_in_order.sort_by(|a, b| {
            a.class
                .name
                .cmp(&b.class.name)
                .then_with(|| a.class.plugin_name.cmp(&b.class.plugin_name))
        });

        let actions_by_name: HashMap<QualifiedName, ActionSummary> = all_actions
            .iter()
            .map(|a| (a.action.clone(), a.clone()))
            .collect();
        let classes_by_name: HashMap<QualifiedName, CatalogClass> = classes_in_order
            .iter()
            .map(|c| (c.class.clone(), c.clone()))
            .collect();
        let classes_by_hash: HashMap<Hash, QualifiedName> = classes_in_order
            .iter()
            .filter_map(|c| {
                decode_hash_hex(&c.hash)
                    .ok()
                    .map(|hash| (hash, c.class.clone()))
            })
            .collect();

        Ok(Self {
            plugins,
            actions: all_actions,
            actions_by_name,
            action_plugin_idx,
            classes: classes_in_order,
            classes_by_name,
            classes_by_hash,
            combined_podlang_src: combined_podlang,
            mock_proofs,
        })
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }
}

impl ActionCatalog for PexeCatalog {
    fn list_actions(&self) -> Vec<ActionSummary> {
        self.actions.clone()
    }

    fn get_action(&self, action: &QualifiedName) -> Option<ActionSummary> {
        self.actions_by_name.get(action).cloned()
    }

    fn list_classes(&self) -> Vec<CatalogClass> {
        self.classes.clone()
    }

    fn get_class(&self, class: &QualifiedName) -> Option<CatalogClass> {
        self.classes_by_name.get(class).cloned()
    }

    fn get_class_by_hash(&self, class_hash: &Hash) -> Option<CatalogClass> {
        let qname = self.classes_by_hash.get(class_hash)?;
        self.classes_by_name.get(qname).cloned()
    }

    fn execute_action(
        &self,
        action: QualifiedName,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        let plugin_idx = *self
            .action_plugin_idx
            .get(&action)
            .ok_or_else(|| anyhow!("no plugin provides action {action}"))?;
        let plugin = &self.plugins[plugin_idx];
        let sdk = Sdk::default();
        let mut resolver = plugin_resolver(&sdk, &self.plugins);
        let module = resolver.load(&plugin.manifest.plugin.name).map_err(|err| {
            anyhow!(
                "failed to reload plugin {} for execution: {err}",
                plugin.manifest.plugin.name
            )
        })?;
        let executor = module.executor(self.mock_proofs, Arc::new(grounding_witness));
        Ok(executor.action(&action.name, inputs)?)
    }

    fn generated_podlang(&self) -> Option<String> {
        if self.combined_podlang_src.is_empty() {
            None
        } else {
            Some(self.combined_podlang_src.clone())
        }
    }
}

/// Resolver over the installed plugins: each loads once, dependencies
/// before importers, with cycles and hash-pin mismatches rejected.
fn plugin_resolver<'a>(sdk: &'a Sdk, plugins: &'a [Plugin]) -> sdk::ImportResolver<'a> {
    sdk::ImportResolver::new(
        sdk,
        plugins
            .iter()
            .map(|plugin| (&plugin.manifest, plugin.script.as_str())),
    )
}

fn discover_plugins(actions_dir: &Path) -> Result<Vec<Plugin>> {
    if !actions_dir.exists() {
        return Ok(Vec::new());
    }
    let entries = pexe::pexe_paths_in(actions_dir);
    let mut plugins = Vec::with_capacity(entries.len());
    for path in entries {
        let bytes = pexe::read_pexe_file(&path)?;
        plugins.push(load_plugin_from_bytes(path, &bytes)?);
    }
    Ok(plugins)
}

fn load_plugin_from_bytes(path: PathBuf, bytes: &[u8]) -> Result<Plugin> {
    let (manifest, script) = pexe::unpack(bytes)
        .map_err(|err| anyhow!("failed to unpack pexe {}: {err}", path.display()))?;
    Ok(Plugin {
        path,
        manifest,
        script,
    })
}

/// Allowlist for `manifest.plugin.name`. Must be non-empty and contain only
/// ASCII alphanumerics, `-`, or `_`. Rules out `:` (which would straddle
/// the `::` qualified-id separator), every path-significant character
/// (`/`, `\`, `.`), whitespace, and any reserved/control characters that
/// would otherwise leak into filenames or split qualified ids unexpectedly.
pub(crate) fn validate_plugin_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow!("plugin name must be non-empty"));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
    {
        return Err(anyhow!(
            "plugin name may only contain ASCII letters, digits, '-', and '_'; \
             rejected character {bad:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_plugin_bytes() -> Vec<u8> {
    // Pack the live plugin sources in-memory so tests never touch ~/.dobj/actions.
    let manifest = include_str!("../../../examples/craft-basics/manifest.toml");
    let script = include_str!("../../../examples/craft-basics/plugin.rhai");
    pexe::pack(manifest, script).expect("test plugin packs")
}

#[cfg(test)]
pub(crate) fn test_totem_plugin_bytes() -> Vec<u8> {
    let manifest = include_str!("../../../examples/craft-totem/manifest.toml");
    let script = include_str!("../../../examples/craft-totem/plugin.rhai");
    pexe::pack(manifest, script).expect("totem plugin packs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn craft_basics(name: &str) -> QualifiedName {
        QualifiedName::new("craft-basics", name)
    }

    fn test_catalog() -> PexeCatalog {
        PexeCatalog::from_bytes(
            std::iter::once((PathBuf::from("craft-basics.pexe"), test_plugin_bytes())),
            true,
        )
        .unwrap()
    }

    #[test]
    fn test_pexe_catalog_hides_internal_actions() {
        let catalog = test_catalog();
        let names: Vec<_> = catalog
            .list_actions()
            .into_iter()
            .map(|a| a.action)
            .collect();
        assert!(names.contains(&craft_basics("CraftWood")));
        assert!(!names.contains(&craft_basics("UseWoodPick")));
    }

    #[test]
    fn test_pexe_catalog_lists_classes() {
        let catalog = test_catalog();
        let classes: Vec<_> = catalog
            .list_classes()
            .into_iter()
            .map(|c| c.class)
            .collect();
        assert!(classes.contains(&craft_basics("Log")));
        assert!(classes.contains(&craft_basics("WoodPick")));
    }

    #[test]
    fn test_pexe_catalog_empty_dir_has_no_plugins() {
        let catalog = PexeCatalog::from_bytes(std::iter::empty(), true).unwrap();
        assert_eq!(catalog.plugin_count(), 0);
        assert!(catalog.list_actions().is_empty());
        assert!(catalog.generated_podlang().is_none());
    }

    #[test]
    fn test_get_class_by_hash_round_trip() {
        let catalog = test_catalog();
        let log = catalog
            .get_class(&craft_basics("Log"))
            .expect("Log class present");
        let by_hash = decode_hash_hex(&log.hash)
            .ok()
            .and_then(|h| catalog.get_class_by_hash(&h))
            .expect("class hash resolves back");
        assert_eq!(by_hash.class, log.class);
    }

    #[test]
    fn test_invalid_plugin_name_rejected() {
        // Each of these would either break qualified-id parsing or escape
        // the objects directory when used as a filename prefix.
        let cases = [
            ("weird:plugin", "':' in plugin name"),
            ("foo/bar", "'/' in plugin name"),
            ("foo\\bar", "'\\' in plugin name"),
            ("..", "'..' as plugin name"),
            ("with space", "whitespace in plugin name"),
            ("", "empty plugin name"),
        ];
        for (name, label) in cases {
            let bytes = synthetic_plugin_bytes(name, ALPHA_SCRIPT);
            let result = PexeCatalog::from_bytes(
                std::iter::once((PathBuf::from(format!("{name}.pexe")), bytes)),
                true,
            );
            match result {
                Ok(_) => panic!("expected catalog to reject {label}, but load succeeded"),
                Err(err) => {
                    let msg = err.to_string();
                    assert!(
                        msg.contains("invalid plugin name") || msg.contains("plugin name"),
                        "unexpected error for {label}: {msg}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_duplicate_plugin_name_rejected() {
        let result = PexeCatalog::from_bytes(
            [
                (PathBuf::from("a.pexe"), test_plugin_bytes()),
                (PathBuf::from("b.pexe"), test_plugin_bytes()),
            ],
            true,
        );
        match result {
            Ok(_) => panic!("expected duplicate-plugin-name error, but load succeeded"),
            Err(err) => assert!(
                err.to_string().contains("duplicate plugin name"),
                "expected duplicate-plugin-name error, got: {err}"
            ),
        }
    }

    // --- Synthetic two-plugin fixtures ---------------------------------------
    //
    // `alpha` and `beta` both declare classes named `Foo` and `Bar` and actions
    // named `MakeFoo` and `ConsumeFoo`. The class names collide; the script
    // bodies differ in the `durability` constant they bake into each output
    // (alpha bakes 100, beta bakes 200), which gives each plugin a different
    // `CustomPredicateBatch` id and therefore different class/action predicate
    // hashes. This is the same mechanism that gives the real craft-basics
    // plugin distinct hashes for `WoodPick` (durability 100) and `StonePick`
    // (durability 200) — it's the exact shape the catalog collision bug used
    // to mishandle.
    //
    // Each action introduces a private `key` wildcard so the compiled podlang
    // has a non-empty `private:` clause (an empty one is a syntax error).

    const ALPHA_SCRIPT: &str = r#"
fn MakeFoo(action) {
    var foo = action.output("Foo");
    foo.set([["durability", 100]]);
    var key = action.random();
    foo.update("key", key);
}

fn ConsumeFoo(action) {
    var foo = action.input("Foo");
    var bar = action.output("Bar");
    bar.set([["durability", 100]]);
    var key = action.random();
    bar.update("key", key);
}
"#;

    const BETA_SCRIPT: &str = r#"
fn MakeFoo(action) {
    var foo = action.output("Foo");
    foo.set([["durability", 200]]);
    var key = action.random();
    foo.update("key", key);
}

fn ConsumeFoo(action) {
    var foo = action.input("Foo");
    var bar = action.output("Bar");
    bar.set([["durability", 200]]);
    var key = action.random();
    bar.update("key", key);
}
"#;

    fn synthetic_plugin_bytes(plugin_name: &str, script: &str) -> Vec<u8> {
        // Manifest with a placeholder hash; we rewrite it to the real
        // compiled hash below before packing so the catalog's
        // `load_module_from_src_manifest` validation passes.
        let template = format!(
            r#"[plugin]
name = "{plugin_name}"
version = "0.1.0"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

[[classes]]
name = "Foo"
emoji = "F"
description = "test class Foo"

[[classes]]
name = "Bar"
emoji = "B"
description = "test class Bar"

[[actions]]
name = "MakeFoo"
emoji = "F"
description = "make a Foo"

[[actions]]
name = "ConsumeFoo"
emoji = "B"
description = "consume a Foo to make a Bar"
"#
        );
        pack_stamped(&template, script, &[])
    }

    /// Compile a synthetic plugin, stamp its manifest's `module_hash`
    /// (and any `[[imports]]` pins) the way `pexe build` would, and pack
    /// it, so the catalog's manifest validation passes.
    fn pack_stamped(template: &str, script: &str, imports: &[sdk::ModuleImport]) -> Vec<u8> {
        let manifest: sdk::manifest::Manifest =
            toml::from_str(template).expect("synthetic manifest parses");
        let module = pexe::compile_module(&Sdk::default(), &manifest, script, imports)
            .expect("synthetic script compiles");
        let mut toml_src =
            pexe::set_manifest_hash(template, &format!("{:#}", module.module().batch.id()))
                .expect("rewrite module_hash");
        for import in imports {
            toml_src = pexe::set_manifest_import_hash(
                &toml_src,
                &import.name,
                &format!("{:#}", import.module.module().batch.id()),
            )
            .expect("rewrite import module_hash");
        }
        pexe::pack(&toml_src, script).expect("pack synthetic plugin")
    }

    fn alpha_beta_catalog() -> PexeCatalog {
        let alpha = synthetic_plugin_bytes("alpha", ALPHA_SCRIPT);
        let beta = synthetic_plugin_bytes("beta", BETA_SCRIPT);
        PexeCatalog::from_bytes(
            [
                (PathBuf::from("alpha.pexe"), alpha),
                (PathBuf::from("beta.pexe"), beta),
            ],
            true,
        )
        .expect("alpha + beta catalog loads")
    }

    #[test]
    fn test_two_plugins_same_class_name_keeps_distinct_hashes() {
        let catalog = alpha_beta_catalog();
        let alpha_foo = QualifiedName::new("alpha", "Foo");
        let beta_foo = QualifiedName::new("beta", "Foo");
        let foo_alpha = catalog.get_class(&alpha_foo).expect("alpha::Foo present");
        let foo_beta = catalog.get_class(&beta_foo).expect("beta::Foo present");
        assert_eq!(foo_alpha.class.name, "Foo");
        assert_eq!(foo_beta.class.name, "Foo");
        assert_eq!(foo_alpha.class.plugin_name, "alpha");
        assert_eq!(foo_beta.class.plugin_name, "beta");
        assert_ne!(
            foo_alpha.hash, foo_beta.hash,
            "Foo from two different modules must have different IsFoo predicate hashes"
        );
    }

    #[test]
    fn test_two_plugins_same_action_name_routes_to_correct_module() {
        let catalog = alpha_beta_catalog();

        // Each plugin's MakeFoo produces an output whose obj["type"] is *that
        // plugin's* IsFoo predicate hash. If the catalog routed the wrong
        // script, the type field would be the other plugin's hash.
        let alpha_foo = catalog
            .get_class(&QualifiedName::new("alpha", "Foo"))
            .expect("alpha::Foo present");
        let beta_foo = catalog
            .get_class(&QualifiedName::new("beta", "Foo"))
            .expect("beta::Foo present");
        let alpha_hash = decode_hash_hex(&alpha_foo.hash).expect("alpha::Foo hash parses");
        let beta_hash = decode_hash_hex(&beta_foo.hash).expect("beta::Foo hash parses");

        let alpha_out = catalog
            .execute_action(
                QualifiedName::new("alpha", "MakeFoo"),
                dummy_grounding_witness(),
                vec![],
            )
            .expect("alpha::MakeFoo runs");
        let alpha_type =
            obj_type_hash_for_test(&alpha_out.obj(0).obj).expect("alpha output has type");
        assert_eq!(
            alpha_type, alpha_hash,
            "alpha::MakeFoo output type should be alpha's IsFoo hash"
        );

        let beta_out = catalog
            .execute_action(
                QualifiedName::new("beta", "MakeFoo"),
                dummy_grounding_witness(),
                vec![],
            )
            .expect("beta::MakeFoo runs");
        let beta_type = obj_type_hash_for_test(&beta_out.obj(0).obj).expect("beta output has type");
        assert_eq!(
            beta_type, beta_hash,
            "beta::MakeFoo output type should be beta's IsFoo hash"
        );
    }

    #[test]
    fn test_action_input_class_hash_is_module_scoped() {
        let catalog = alpha_beta_catalog();
        let alpha_foo = catalog
            .get_class(&QualifiedName::new("alpha", "Foo"))
            .unwrap();
        let beta_foo = catalog
            .get_class(&QualifiedName::new("beta", "Foo"))
            .unwrap();
        assert_ne!(alpha_foo.hash, beta_foo.hash);

        let alpha_consume = catalog
            .get_action(&QualifiedName::new("alpha", "ConsumeFoo"))
            .expect("alpha::ConsumeFoo present");
        let beta_consume = catalog
            .get_action(&QualifiedName::new("beta", "ConsumeFoo"))
            .expect("beta::ConsumeFoo present");

        let alpha_input = &alpha_consume.total_inputs[0];
        let beta_input = &beta_consume.total_inputs[0];
        assert_eq!(alpha_input.class, QualifiedName::new("alpha", "Foo"));
        assert_eq!(beta_input.class, QualifiedName::new("beta", "Foo"));
        assert_eq!(
            alpha_input.hash, alpha_foo.hash,
            "alpha::ConsumeFoo's required input hash must be alpha's IsFoo hash"
        );
        assert_eq!(
            beta_input.hash, beta_foo.hash,
            "beta::ConsumeFoo's required input hash must be beta's IsFoo hash"
        );
    }

    #[test]
    fn test_class_cross_references_are_per_plugin() {
        // Each class's `produced_by` / `consumed_by` must list only the
        // actions from its own plugin. If the catalog conflated entries by
        // bare name, alpha::Foo's `produced_by` could end up containing
        // beta::MakeFoo (and vice versa), which would mis-route GUI
        // suggestions and feasibility checks.
        let catalog = alpha_beta_catalog();
        let alpha_foo = catalog
            .get_class(&QualifiedName::new("alpha", "Foo"))
            .unwrap();
        let beta_foo = catalog
            .get_class(&QualifiedName::new("beta", "Foo"))
            .unwrap();

        assert_eq!(
            alpha_foo.produced_by,
            vec![QualifiedName::new("alpha", "MakeFoo")]
        );
        assert_eq!(
            alpha_foo.consumed_by,
            vec![QualifiedName::new("alpha", "ConsumeFoo")]
        );
        assert_eq!(
            beta_foo.produced_by,
            vec![QualifiedName::new("beta", "MakeFoo")]
        );
        assert_eq!(
            beta_foo.consumed_by,
            vec![QualifiedName::new("beta", "ConsumeFoo")]
        );

        // The predicate source string is also non-empty and looks like an
        // IsFoo predicate. (The IsFoo body itself is the same shape in both
        // plugins, so we don't compare it across plugins — the cryptographic
        // identity is captured by `hash`, not the printed source.)
        assert!(
            alpha_foo.predicate_source.contains("IsFoo"),
            "alpha IsFoo source should mention IsFoo; got {}",
            alpha_foo.predicate_source
        );
        assert!(
            beta_foo.predicate_source.contains("IsFoo"),
            "beta IsFoo source should mention IsFoo; got {}",
            beta_foo.predicate_source
        );
    }

    // --- Cross-plugin import fixtures -----------------------------------------
    //
    // `gamma` imports `alpha` and runs alpha::MakeFoo as a sub-action,
    // reading the produced Foo's key into its own Box output.

    const GAMMA_SCRIPT: &str = r#"
fn WrapFoo(action) {
    var foo = action.subaction("alpha::MakeFoo");
    var wrapped = action.output("Box");
    wrapped.set([["foo_key", foo.key]]);
}
"#;

    /// Load a plugin from packed bytes, resolving its imports among the
    /// other packed plugins given.
    fn load_packed(sdk: &Sdk, name: &str, packed: &[&[u8]]) -> std::rc::Rc<sdk::SdkModule> {
        let sources: Vec<(sdk::manifest::Manifest, String)> = packed
            .iter()
            .map(|bytes| pexe::unpack(bytes).expect("plugin unpacks"))
            .collect();
        sdk::ImportResolver::new(
            sdk,
            sources
                .iter()
                .map(|(manifest, script)| (manifest, script.as_str())),
        )
        .load(name)
        .unwrap_or_else(|err| panic!("{name} loads: {err}"))
    }

    /// Pack a gamma plugin importing the given built alpha archive, with
    /// both the import pin and the module hash stamped the way `pexe
    /// build` would. The placeholder pin exercises
    /// `set_manifest_import_hash`.
    fn gamma_plugin_bytes(alpha_bytes: &[u8]) -> Vec<u8> {
        let sdk = Sdk::default();
        let alpha_module = load_packed(&sdk, "alpha", &[alpha_bytes]);
        let template = r#"[plugin]
name = "gamma"
version = "0.1.0"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

[[imports]]
name = "alpha"
module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

[[classes]]
name = "Box"
emoji = "X"
description = "wraps an alpha Foo"

[[actions]]
name = "WrapFoo"
emoji = "X"
description = "run alpha::MakeFoo and box the result"
"#;
        pack_stamped(
            template,
            GAMMA_SCRIPT,
            &[sdk::ModuleImport {
                name: "alpha".to_string(),
                module: alpha_module,
            }],
        )
    }

    #[test]
    fn test_cross_plugin_import_in_catalog() {
        let alpha = synthetic_plugin_bytes("alpha", ALPHA_SCRIPT);
        let gamma = gamma_plugin_bytes(&alpha);
        let catalog = PexeCatalog::from_bytes(
            [
                // Importer listed first: load order comes from the
                // resolver, not the listing order.
                (PathBuf::from("gamma.pexe"), gamma),
                (PathBuf::from("alpha.pexe"), alpha),
            ],
            true,
        )
        .expect("catalog with import loads");

        // WrapFoo's spliced totals point at alpha's Foo with alpha's hash.
        let wrap = catalog
            .get_action(&QualifiedName::new("gamma", "WrapFoo"))
            .expect("gamma::WrapFoo present");
        let alpha_foo = catalog
            .get_class(&QualifiedName::new("alpha", "Foo"))
            .expect("alpha::Foo present");
        assert!(wrap.total_inputs.is_empty());
        let out_classes: Vec<&QualifiedName> =
            wrap.total_outputs.iter().map(|r| &r.class).collect();
        assert_eq!(
            out_classes,
            vec![
                &QualifiedName::new("alpha", "Foo"),
                &QualifiedName::new("gamma", "Box"),
            ]
        );
        assert_eq!(wrap.total_outputs[0].hash, alpha_foo.hash);

        // alpha::Foo's produced_by includes the importing action.
        assert!(
            alpha_foo
                .produced_by
                .contains(&QualifiedName::new("gamma", "WrapFoo")),
            "alpha::Foo produced_by should include gamma::WrapFoo; got {:?}",
            alpha_foo.produced_by
        );

        // Executing the importer emits a Foo typed with alpha's guard
        // hash and a Box typed with gamma's.
        let out = catalog
            .execute_action(
                QualifiedName::new("gamma", "WrapFoo"),
                dummy_grounding_witness(),
                vec![],
            )
            .expect("gamma::WrapFoo runs");
        assert_eq!(out.objs.len(), 2);
        let foo_type = obj_type_hash_for_test(&out.obj(0).obj).expect("foo output has type");
        assert_eq!(foo_type, decode_hash_hex(&alpha_foo.hash).unwrap());
        let gamma_box = catalog
            .get_class(&QualifiedName::new("gamma", "Box"))
            .expect("gamma::Box present");
        let box_type = obj_type_hash_for_test(&out.obj(1).obj).expect("box output has type");
        assert_eq!(box_type, decode_hash_hex(&gamma_box.hash).unwrap());
    }

    /// The shipped craft-totem example end to end through the catalog:
    /// its stamped manifest pins resolve against the shipped
    /// craft-basics, and CraftTotem consumes a grounded Log through the
    /// imported CraftWood sub-action.
    #[test]
    fn test_craft_totem_example_runs() {
        let basics_bytes = test_plugin_bytes();
        let totem_bytes = test_totem_plugin_bytes();
        let catalog = PexeCatalog::from_bytes(
            [
                (PathBuf::from("craft-basics.pexe"), basics_bytes.clone()),
                (PathBuf::from("craft-totem.pexe"), totem_bytes),
            ],
            true,
        )
        .expect("example plugins load");

        // Mint a synthetic grounded Log against craft-basics's module.
        let sdk = Sdk::default();
        let basics = load_packed(&sdk, "craft-basics", &[&basics_bytes]);
        let log = pexe::fixtures::mint_class(&basics, "Log").expect("mint Log");
        let state = pexe::fixtures::build_synthetic_state(&[log]).expect("synthetic state");

        let out = catalog
            .execute_action(
                QualifiedName::new("craft-totem", "CraftTotem"),
                (*state.grounding_witness).clone(),
                state.spendable,
            )
            .expect("craft-totem::CraftTotem runs");
        assert_eq!(out.objs.len(), 2, "one Wood plus one Totem");
        let wood_type = obj_type_hash_for_test(&out.obj(0).obj).expect("wood has type");
        let basics_wood = catalog
            .get_class(&QualifiedName::new("craft-basics", "Wood"))
            .expect("craft-basics::Wood present");
        assert_eq!(wood_type, decode_hash_hex(&basics_wood.hash).unwrap());
        let totem_type = obj_type_hash_for_test(&out.obj(1).obj).expect("totem has type");
        let totem_class = catalog
            .get_class(&QualifiedName::new("craft-totem", "Totem"))
            .expect("craft-totem::Totem present");
        assert_eq!(totem_type, decode_hash_hex(&totem_class.hash).unwrap());
    }

    #[test]
    fn test_cross_plugin_missing_dep_fails_catalog() {
        let alpha = synthetic_plugin_bytes("alpha", ALPHA_SCRIPT);
        let gamma = gamma_plugin_bytes(&alpha);
        let err = PexeCatalog::from_bytes([(PathBuf::from("gamma.pexe"), gamma)], true)
            .err()
            .expect("catalog without the dependency must fail");
        assert!(
            err.to_string().contains("not among the available plugins"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_cross_plugin_pin_mismatch_fails_catalog() {
        let alpha = synthetic_plugin_bytes("alpha", ALPHA_SCRIPT);
        let gamma = gamma_plugin_bytes(&alpha);
        // Install a different plugin under the same name: beta's script
        // compiles to a different batch id, so gamma's pin no longer
        // matches the module the name resolves to.
        let impostor_alpha = synthetic_plugin_bytes("alpha", BETA_SCRIPT);
        let err = PexeCatalog::from_bytes(
            [
                (PathBuf::from("alpha.pexe"), impostor_alpha),
                (PathBuf::from("gamma.pexe"), gamma),
            ],
            true,
        )
        .err()
        .expect("catalog with a mismatched import pin must fail");
        assert!(
            err.to_string().contains("manifest pins module_hash"),
            "unexpected error: {err}"
        );
    }

    fn dummy_grounding_witness() -> txlib::GroundingWitness {
        txlib::GroundingWitness::new(
            txlib::StateHeader::new(
                1,
                1,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
            ),
            std::collections::HashMap::new(),
        )
    }

    fn obj_type_hash_for_test(obj: &pod2::middleware::containers::Dictionary) -> Option<Hash> {
        let value = obj.get(&pod2::middleware::StrKey::from("type")).ok()??;
        Some(Hash(value.raw().0))
    }
}
