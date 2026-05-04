//! Action catalog backed by `.pexe` plugin archives.
//!
//! At construction time, each installed `.pexe` is unpacked and its script compiled
//! via `Sdk::load_module_from_src_manifest` (which enforces the manifest's
//! `module_hash`). The compiled module is used to derive action/class hashes and
//! the podlang source shown in the GUI.
//!
//! Classes and actions are keyed by qualified id `<plugin_name>:<name>`. Two
//! plugins may declare a class or action with the same bare name; they are kept
//! distinct by qualified id and by their on-chain `Is{class}` predicate hash
//! (which differs between modules because each module has a unique
//! `module_hash`). Cross-plugin class references are not supported: an action
//! must reference classes declared in its own plugin.
//!
//! The compiled [`sdk::SdkModule`] is not kept — it holds a `Rc<Engine>` and is
//! therefore `!Send`. `execute_action` re-loads the script from its stored bytes
//! on demand, matching the per-call pattern used before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use common::decode_hash_hex;
use pod2::middleware::Hash;
use sdk::{Sdk, SpendableObject, SpendableObjects, manifest::Manifest};
use txlib::GroundingWitness;

use crate::catalog::{ActionCatalog, CatalogClass, extract_predicate};
use crate::types::{ActionSummary, ClassRef};

struct Plugin {
    #[allow(dead_code)]
    path: PathBuf,
    manifest: Manifest,
    script: String,
    /// Qualified ids (`<plugin>:<action>`) provided by this plugin. Used to
    /// route `execute_action` back to the originating script bytes.
    action_ids: Vec<String>,
}

pub struct PexeCatalog {
    plugins: Vec<Plugin>,
    actions: Vec<ActionSummary>,
    actions_by_id: HashMap<String, ActionSummary>,
    /// Maps qualified action id -> plugin index in `plugins`.
    action_plugin_idx: HashMap<String, usize>,
    classes: Vec<CatalogClass>,
    classes_by_id: HashMap<String, CatalogClass>,
    classes_by_hash: HashMap<Hash, String>,
    combined_podlang_src: String,
    mock_proofs: bool,
}

pub fn qualified_id(plugin_name: &str, name: &str) -> String {
    format!("{plugin_name}:{name}")
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
        // Validate plugin.name early: it ends up in qualified ids
        // (`<plugin>:<name>`), in `.dobj` filename prefixes, and in GUI
        // labels. The allowlist is filename-safe on every OS we target
        // and rules out the `:` separator (which would break
        // `bare_action_name`'s split) and any path-significant chars
        // (`/`, `\`, `..`) that could otherwise let a malicious or
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

        let mut all_actions: Vec<ActionSummary> = Vec::new();
        let mut classes_in_order: Vec<CatalogClass> = Vec::new();
        let mut combined_podlang = String::new();
        let mut enriched_plugins: Vec<Plugin> = Vec::with_capacity(plugins.len());
        let mut action_plugin_idx: HashMap<String, usize> = HashMap::new();

        for mut plugin in plugins {
            let plugin_name = plugin.manifest.plugin.name.clone();
            let module = sdk
                .load_module_from_src_manifest(&plugin.script, &plugin.manifest)
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
                let qid = qualified_id(&plugin_name, bare);
                let class_hash = class_hashes[bare];
                let meta = class_meta_by_name.get(bare.as_str());
                let predicate_source = extract_predicate(&podlang_src, &format!("Is{bare}"))
                    .unwrap_or_else(|| format!("Is{bare}(state) = OR(...)"));
                classes_in_order.push(CatalogClass {
                    id: qid,
                    display_name: bare.clone(),
                    plugin_name: plugin_name.clone(),
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
            // qualified id routes back to this plugin via execute_action.
            let action_meta_by_name: HashMap<&str, &sdk::manifest::Action> = plugin
                .manifest
                .actions
                .iter()
                .map(|a| (a.name.as_str(), a))
                .collect();
            let plugin_idx = enriched_plugins.len();
            let mut plugin_action_ids: Vec<String> = Vec::new();

            for action in module.actions() {
                let bare = action.name.clone();
                let qid = qualified_id(&plugin_name, &bare);
                plugin_action_ids.push(qid.clone());
                if let Some(prior) = action_plugin_idx.insert(qid.clone(), plugin_idx) {
                    return Err(anyhow!(
                        "internal: duplicate action qualified id {qid} (already mapped to plugin idx {prior})"
                    ));
                }

                let meta = action_meta_by_name.get(bare.as_str());
                let resolve_class = |class_name: &str| -> Result<ClassRef> {
                    let hash = class_hashes.get(class_name).ok_or_else(|| {
                        anyhow!(
                            "plugin {plugin_name}: action {bare} references class {class_name:?} \
                             which is not declared in this plugin (cross-plugin class \
                             references are not supported yet)"
                        )
                    })?;
                    Ok(ClassRef {
                        id: qualified_id(&plugin_name, class_name),
                        display_name: class_name.to_string(),
                        hash: format!("{:#}", hash),
                    })
                };

                let total_inputs = action
                    .total_inputs()
                    .map(|r| resolve_class(&r.class))
                    .collect::<Result<Vec<_>>>()?;
                let total_outputs = action
                    .total_outputs()
                    .map(|r| resolve_class(&r.class))
                    .collect::<Result<Vec<_>>>()?;

                if meta.is_some_and(|m| m.hidden) {
                    continue;
                }

                let action_hash = module
                    .action_hash(&bare)
                    .map(|h| format!("{:#}", h))
                    .unwrap_or_default();
                all_actions.push(ActionSummary {
                    id: qid,
                    display_name: bare.clone(),
                    plugin_name: plugin_name.clone(),
                    emoji: meta.map_or("⚙️", |m| m.emoji.as_str()).to_string(),
                    hash: action_hash,
                    description: meta
                        .map_or("Pexe action", |m| m.description.as_str())
                        .to_string(),
                    total_inputs,
                    total_outputs,
                });
            }

            plugin.action_ids = plugin_action_ids;
            enriched_plugins.push(plugin);
        }

        // Second pass: fill produced_by / consumed_by per class using qualified ids.
        for class in classes_in_order.iter_mut() {
            class.produced_by = all_actions
                .iter()
                .filter(|a| a.total_outputs.iter().any(|r| r.id == class.id))
                .map(|a| a.id.clone())
                .collect();
            class.consumed_by = all_actions
                .iter()
                .filter(|a| a.total_inputs.iter().any(|r| r.id == class.id))
                .map(|a| a.id.clone())
                .collect();
        }

        // Deterministic GUI order: sort by display name, then plugin.
        classes_in_order.sort_by(|a, b| {
            a.display_name
                .cmp(&b.display_name)
                .then_with(|| a.plugin_name.cmp(&b.plugin_name))
        });

        let actions_by_id: HashMap<String, ActionSummary> = all_actions
            .iter()
            .map(|a| (a.id.clone(), a.clone()))
            .collect();
        let classes_by_id: HashMap<String, CatalogClass> = classes_in_order
            .iter()
            .map(|c| (c.id.clone(), c.clone()))
            .collect();
        let classes_by_hash: HashMap<Hash, String> = classes_in_order
            .iter()
            .filter_map(|c| {
                decode_hash_hex(&c.hash)
                    .ok()
                    .map(|hash| (hash, c.id.clone()))
            })
            .collect();

        Ok(Self {
            plugins: enriched_plugins,
            actions: all_actions,
            actions_by_id,
            action_plugin_idx,
            classes: classes_in_order,
            classes_by_id,
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

    fn get_action(&self, action_id: &str) -> Option<ActionSummary> {
        self.actions_by_id.get(action_id).cloned()
    }

    fn list_classes(&self) -> Vec<CatalogClass> {
        self.classes.clone()
    }

    fn get_class(&self, class_id: &str) -> Option<CatalogClass> {
        self.classes_by_id.get(class_id).cloned()
    }

    fn get_class_by_hash(&self, class_hash: &Hash) -> Option<CatalogClass> {
        let id = self.classes_by_hash.get(class_hash)?;
        self.classes_by_id.get(id).cloned()
    }

    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        let plugin_idx = *self
            .action_plugin_idx
            .get(&action_id)
            .ok_or_else(|| anyhow!("no plugin provides action {action_id}"))?;
        let plugin = &self.plugins[plugin_idx];
        let bare_name = bare_action_name(&action_id, &plugin.manifest.plugin.name)
            .ok_or_else(|| anyhow!("invalid qualified action id: {action_id}"))?;
        let sdk = Sdk::default();
        let module = sdk
            .load_module_from_src_manifest(&plugin.script, &plugin.manifest)
            .map_err(|err| {
                anyhow!(
                    "failed to reload plugin {} for execution: {err}",
                    plugin.manifest.plugin.name
                )
            })?;
        let executor = module.executor(self.mock_proofs, Arc::new(grounding_witness));
        Ok(executor.action(bare_name, inputs)?)
    }

    fn generated_podlang(&self) -> Option<String> {
        if self.combined_podlang_src.is_empty() {
            None
        } else {
            Some(self.combined_podlang_src.clone())
        }
    }
}

fn discover_plugins(actions_dir: &Path) -> Result<Vec<Plugin>> {
    if !actions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(actions_dir)
        .with_context(|| format!("failed to read {}", actions_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(pexe::PEXE_EXTENSION))
        .collect();
    entries.sort();

    let mut plugins = Vec::with_capacity(entries.len());
    for path in entries {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("failed to read pexe {}", path.display()))?;
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
        action_ids: Vec::new(),
    })
}

/// Allowlist for `manifest.plugin.name`. Must be non-empty and contain only
/// ASCII alphanumerics, `-`, or `_`. Rules out the qualified-id separator
/// `:`, every path-significant character (`/`, `\`, `.`), whitespace, and
/// any reserved/control characters that would otherwise leak into filenames
/// or split qualified ids unexpectedly.
fn validate_plugin_name(name: &str) -> Result<()> {
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

fn bare_action_name<'a>(qualified_id: &'a str, plugin_name: &str) -> Option<&'a str> {
    qualified_id
        .strip_prefix(plugin_name)
        .and_then(|rest| rest.strip_prefix(':'))
        .filter(|rest| !rest.is_empty())
}

#[cfg(test)]
pub(crate) fn test_plugin_bytes() -> Vec<u8> {
    // Pack the live plugin sources in-memory so tests never touch ~/.dobj/actions.
    let manifest = include_str!("../../plugins/craft-basics/manifest.toml");
    let script = include_str!("../../plugins/craft-basics/plugin.rhai");
    pexe::pack(manifest, script).expect("test plugin packs")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let action_ids: Vec<_> = catalog.list_actions().into_iter().map(|a| a.id).collect();
        assert!(action_ids.contains(&"craft-basics:CraftWood".to_string()));
        assert!(!action_ids.contains(&"craft-basics:UseWoodPick".to_string()));
    }

    #[test]
    fn test_pexe_catalog_lists_classes() {
        let catalog = test_catalog();
        let class_ids: Vec<_> = catalog.list_classes().into_iter().map(|c| c.id).collect();
        assert!(class_ids.contains(&"craft-basics:Log".to_string()));
        assert!(class_ids.contains(&"craft-basics:WoodPick".to_string()));
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
            .get_class("craft-basics:Log")
            .expect("Log class present");
        let by_hash = decode_hash_hex(&log.hash)
            .ok()
            .and_then(|h| catalog.get_class_by_hash(&h))
            .expect("class hash resolves back");
        assert_eq!(by_hash.id, log.id);
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
    fn test_file_prefix_for_class_sanitizes_path_chars() {
        use crate::execute::file_prefix_for_class;
        // Plugin names are validated upstream so the colon here is the
        // only legitimate `:` in a real qualified id, but if a class name
        // ever contained a path-significant character the prefix must
        // still stay confined to a single filename component.
        assert_eq!(
            file_prefix_for_class("craft-basics:Wood"),
            "craft-basics_wood"
        );
        assert_eq!(
            file_prefix_for_class("plugin:weird/class"),
            "plugin_weird_class"
        );
        // `:`, `.`, `.`, `\` all become `_`
        assert_eq!(file_prefix_for_class("plugin:..\\Stone"), "plugin____stone");
        let prefix = file_prefix_for_class("p:c");
        assert!(
            !prefix.contains('/') && !prefix.contains('\\') && !prefix.contains(':'),
            "prefix {prefix:?} must be path-safe"
        );
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
    // bodies differ (`blueprint = "FooA"` vs `"FooB"`), which gives each
    // plugin a different `CustomPredicateBatch` id and therefore different
    // class/action predicate hashes. This is the exact shape the catalog
    // collision bug used to mishandle.

    // Each action introduces a private `key` wildcard so the compiled
    // podlang has a non-empty `private:` clause (an empty one is a syntax
    // error). The literal blueprint string ("FooA" vs "FooB", "BarA" vs
    // "BarB") makes the two modules' predicate batches differ.

    const ALPHA_SCRIPT: &str = r#"
fn MakeFoo(action) {
    var foo = action.output("Foo");
    foo.set([["blueprint", "FooA"]]);
    var key = action.random();
    foo.update("key", key);
}

fn ConsumeFoo(action) {
    var foo = action.input("Foo");
    var bar = action.output("Bar");
    bar.set([["blueprint", "BarA"]]);
    var key = action.random();
    bar.update("key", key);
}
"#;

    const BETA_SCRIPT: &str = r#"
fn MakeFoo(action) {
    var foo = action.output("Foo");
    foo.set([["blueprint", "FooB"]]);
    var key = action.random();
    foo.update("key", key);
}

fn ConsumeFoo(action) {
    var foo = action.input("Foo");
    var bar = action.output("Bar");
    bar.set([["blueprint", "BarB"]]);
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
        let manifest: sdk::manifest::Manifest =
            toml::from_str(&template).expect("synthetic manifest parses");
        let real_hash =
            pexe::compile_module_hash(&manifest, script).expect("synthetic script compiles");
        let with_hash =
            pexe::set_manifest_hash(&template, &real_hash).expect("rewrite module_hash");
        pexe::pack(&with_hash, script).expect("pack synthetic plugin")
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
        let foo_alpha = catalog.get_class("alpha:Foo").expect("alpha:Foo present");
        let foo_beta = catalog.get_class("beta:Foo").expect("beta:Foo present");
        assert_eq!(foo_alpha.display_name, "Foo");
        assert_eq!(foo_beta.display_name, "Foo");
        assert_eq!(foo_alpha.plugin_name, "alpha");
        assert_eq!(foo_beta.plugin_name, "beta");
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
        let alpha_foo = catalog.get_class("alpha:Foo").expect("alpha:Foo present");
        let beta_foo = catalog.get_class("beta:Foo").expect("beta:Foo present");
        let alpha_hash = decode_hash_hex(&alpha_foo.hash).expect("alpha:Foo hash parses");
        let beta_hash = decode_hash_hex(&beta_foo.hash).expect("beta:Foo hash parses");

        let alpha_out = catalog
            .execute_action(
                "alpha:MakeFoo".to_string(),
                dummy_grounding_witness(),
                vec![],
            )
            .expect("alpha:MakeFoo runs");
        let alpha_type =
            obj_type_hash_for_test(&alpha_out.obj(0).obj).expect("alpha output has type");
        assert_eq!(
            alpha_type, alpha_hash,
            "alpha:MakeFoo output type should be alpha's IsFoo hash"
        );

        let beta_out = catalog
            .execute_action(
                "beta:MakeFoo".to_string(),
                dummy_grounding_witness(),
                vec![],
            )
            .expect("beta:MakeFoo runs");
        let beta_type = obj_type_hash_for_test(&beta_out.obj(0).obj).expect("beta output has type");
        assert_eq!(
            beta_type, beta_hash,
            "beta:MakeFoo output type should be beta's IsFoo hash"
        );
    }

    #[test]
    fn test_action_input_class_hash_is_module_scoped() {
        let catalog = alpha_beta_catalog();
        let alpha_foo = catalog.get_class("alpha:Foo").unwrap();
        let beta_foo = catalog.get_class("beta:Foo").unwrap();
        assert_ne!(alpha_foo.hash, beta_foo.hash);

        let alpha_consume = catalog
            .get_action("alpha:ConsumeFoo")
            .expect("alpha:ConsumeFoo present");
        let beta_consume = catalog
            .get_action("beta:ConsumeFoo")
            .expect("beta:ConsumeFoo present");

        let alpha_input = &alpha_consume.total_inputs[0];
        let beta_input = &beta_consume.total_inputs[0];
        assert_eq!(alpha_input.id, "alpha:Foo");
        assert_eq!(beta_input.id, "beta:Foo");
        assert_eq!(
            alpha_input.hash, alpha_foo.hash,
            "alpha:ConsumeFoo's required input hash must be alpha's IsFoo hash"
        );
        assert_eq!(
            beta_input.hash, beta_foo.hash,
            "beta:ConsumeFoo's required input hash must be beta's IsFoo hash"
        );
    }

    #[test]
    fn test_class_cross_references_are_per_plugin() {
        // Each class's `produced_by` / `consumed_by` must list only the
        // actions from its own plugin. If the catalog conflated entries by
        // bare name, alpha:Foo's `produced_by` could end up containing
        // `beta:MakeFoo` (and vice versa), which would mis-route GUI
        // suggestions and feasibility checks.
        let catalog = alpha_beta_catalog();
        let alpha_foo = catalog.get_class("alpha:Foo").unwrap();
        let beta_foo = catalog.get_class("beta:Foo").unwrap();

        assert_eq!(
            alpha_foo.produced_by,
            vec!["alpha:MakeFoo".to_string()],
            "alpha:Foo should only be produced by alpha:MakeFoo"
        );
        assert_eq!(
            alpha_foo.consumed_by,
            vec!["alpha:ConsumeFoo".to_string()],
            "alpha:Foo should only be consumed by alpha:ConsumeFoo"
        );
        assert_eq!(beta_foo.produced_by, vec!["beta:MakeFoo".to_string()]);
        assert_eq!(beta_foo.consumed_by, vec!["beta:ConsumeFoo".to_string()]);

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

    fn dummy_grounding_witness() -> txlib::GroundingWitness {
        txlib::GroundingWitness::new(
            txlib::StateRoot::new(
                1,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
                pod2::middleware::EMPTY_HASH,
            ),
            std::collections::HashMap::new(),
        )
    }

    fn obj_type_hash_for_test(obj: &pod2::middleware::containers::Dictionary) -> Option<Hash> {
        let value = obj.get(&pod2::middleware::Key::from("type")).ok()??;
        Some(Hash(value.raw().0))
    }
}
