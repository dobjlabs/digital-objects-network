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
//! differ (each module has a unique `module_hash`).
//!
//! A script names its own classes only, so an action's own objects belong to
//! its plugin. It may still act on another plugin's objects by calling that
//! plugin's action -- `subaction("other::Action")` -- which is why an
//! action's declared inputs and outputs can span plugins and why each is
//! resolved against the plugin that owns it. Those calls also set the load
//! order here, since the callee's compiled batch is part of the caller's.
//!
//! The compiled [`sdk::SdkModule`] is not kept — it holds a `Rc<Engine>` and is
//! therefore `!Send`. `execute_action` re-loads the script from its stored bytes
//! on demand, matching the per-call pattern used before.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use payload::decode_hash_hex;
use pod2::middleware::Hash;
use sdk::{
    PluginDeps, Sdk, SpendableObject, SpendableObjects, manifest::Manifest, script_dependencies,
};
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
    /// Recompile the plugin that provides `action`, together with any
    /// plugins its script reaches by qualified sub-action call. The driver
    /// does not cache compiled modules, so this runs per execution.
    fn load_module(&self, sdk: &Sdk, action: &QualifiedName) -> Result<Rc<sdk::SdkModule>> {
        let plugin_idx = *self
            .action_plugin_idx
            .get(action)
            .ok_or_else(|| anyhow!("no plugin provides action {action}"))?;
        self.load_plugin_module(sdk, plugin_idx, &mut HashMap::new())
    }

    /// Compile one plugin, recursing into its dependencies first. `cache`
    /// keeps a plugin compiled once per call even when several dependents
    /// name it.
    fn load_plugin_module(
        &self,
        sdk: &Sdk,
        plugin_idx: usize,
        cache: &mut HashMap<String, Rc<sdk::SdkModule>>,
    ) -> Result<Rc<sdk::SdkModule>> {
        let plugin = &self.plugins[plugin_idx];
        let plugin_name = plugin.manifest.plugin.name.clone();
        if let Some(module) = cache.get(&plugin_name) {
            return Ok(module.clone());
        }
        let mut deps = PluginDeps::new();
        for dep_name in script_dependencies(&plugin.script) {
            let dep_idx = self
                .plugins
                .iter()
                .position(|candidate| candidate.manifest.plugin.name == dep_name)
                .ok_or_else(|| {
                    anyhow!("plugin {plugin_name} calls into {dep_name}, which is not installed")
                })?;
            let dep = self.load_plugin_module(sdk, dep_idx, cache)?;
            // A declared import pin turns version drift into a message naming
            // the dependency; without one the mismatch still fails, but only
            // as the caller's own whole-module hash check. Import paths are a
            // build-machine convenience and mean nothing here.
            if let Some(pinned) = plugin
                .manifest
                .imports
                .iter()
                .find(|import| import.name == dep_name)
                .and_then(|import| import.module_hash)
            {
                let compiled = dep.module().batch.id();
                if pinned != compiled {
                    return Err(anyhow!(
                        "plugin {plugin_name} pins import {dep_name} at {pinned:#}, but the installed {dep_name} compiles to {compiled:#}"
                    ));
                }
            }
            deps.insert(dep_name, dep);
        }
        let module = sdk
            .load_module_from_manifest_deps(&plugin.script, &plugin.manifest, deps)
            .map_err(|err| anyhow!("failed to reload plugin {plugin_name} for execution: {err}"))?;
        cache.insert(plugin_name, module.clone());
        Ok(module)
    }

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

        let mut all_actions: Vec<ActionSummary> = Vec::new();
        let mut classes_in_order: Vec<CatalogClass> = Vec::new();
        let mut combined_podlang = String::new();
        let mut enriched_plugins: Vec<Plugin> = Vec::with_capacity(plugins.len());
        let mut action_plugin_idx: HashMap<QualifiedName, usize> = HashMap::new();

        // Load in dependency order so a plugin whose script makes a
        // qualified `subaction("other::Action")` call has `other`'s compiled
        // module available; the call embeds its batch id in this one's.
        let plugins = order_by_dependencies(plugins)?;
        let mut loaded: PluginDeps = PluginDeps::new();

        for plugin in plugins {
            let plugin_name = plugin.manifest.plugin.name.clone();
            let mut deps = PluginDeps::new();
            for dep_name in script_dependencies(&plugin.script) {
                let dep = loaded.get(&dep_name).cloned().ok_or_else(|| {
                    anyhow!("plugin {plugin_name} calls into {dep_name}, which is not installed")
                })?;
                deps.insert(dep_name, dep);
            }
            let module = sdk
                .load_module_from_manifest_deps(&plugin.script, &plugin.manifest, deps)
                .map_err(|err| anyhow!("failed to load plugin {plugin_name}: {err}"))?;
            loaded.insert(plugin_name.clone(), module.clone());
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
            let plugin_idx = enriched_plugins.len();

            for action in module.actions() {
                let bare = action.name.clone();
                let qname = QualifiedName::new(plugin_name.clone(), bare.clone());
                if let Some(prior) = action_plugin_idx.insert(qname.clone(), plugin_idx) {
                    return Err(anyhow!(
                        "internal: duplicate action qualified name {qname} (already mapped to plugin idx {prior})"
                    ));
                }

                let meta = action_meta_by_name.get(bare.as_str());
                // A class is resolved against the plugin that declares it:
                // this one, or the dependency an imported sub-action came
                // from. Classes never move between plugins.
                let resolve_class = |r: &sdk::ActionObjectRef| -> Result<ClassRef> {
                    let owner = r.owner.as_deref().unwrap_or(plugin_name.as_str());
                    let hash = if owner == plugin_name {
                        class_hashes.get(&r.class).copied()
                    } else {
                        loaded.get(owner).and_then(|dep| dep.class_hash(&r.class))
                    }
                    .ok_or_else(|| {
                        anyhow!(
                            "plugin {plugin_name}: action {bare} references class {:?} of plugin {owner}, which does not declare it",
                            r.class
                        )
                    })?;
                    Ok(ClassRef {
                        class: QualifiedName::new(owner.to_string(), r.class.clone()),
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

                let action_hash = module
                    .action_hash(&bare)
                    .map(|h| format!("{:#}", h))
                    .unwrap_or_default();
                // Action predicates use the bare action name (no `Is`
                // prefix like classes get).
                let predicate_source = extract_predicate(&podlang_src, &bare)
                    .unwrap_or_else(|| format!("{bare}(state) = AND(...)"));
                let summary = ActionSummary {
                    action: qname,
                    emoji: meta.map_or("⚙️", |m| m.emoji.as_str()).to_string(),
                    hash: action_hash,
                    description: meta
                        .map_or("Pexe action", |m| m.description.as_str())
                        .to_string(),
                    total_inputs,
                    total_outputs,
                    predicate_source,
                };
                if meta.is_some_and(|m| m.hidden) {
                    continue;
                }
                all_actions.push(summary);
            }

            enriched_plugins.push(plugin);
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
            plugins: enriched_plugins,
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
        let sdk = Sdk::default();
        let witness = Arc::new(grounding_witness);

        let module = self.load_module(&sdk, &action)?;
        let executor = module.executor(self.mock_proofs, witness);
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
/// The second bundled plugin, for cross-plugin composition tests.
pub(crate) fn test_rocket_bytes() -> Vec<u8> {
    let manifest = include_str!("../../../examples/craft-rocket/manifest.toml");
    let script = include_str!("../../../examples/craft-rocket/plugin.rhai");
    pexe::pack(manifest, script).expect("test rocket packs")
}

#[cfg(test)]
/// The bundled swap example, packed from source like the plugins above. Its
/// script reaches craft-basics AND craft-rocket with qualified sub-action
/// calls, and its manifest pins both through [[imports]].
pub(crate) fn bundled_swap_bytes() -> Vec<u8> {
    let manifest = include_str!("../../../examples/swap-log-copper/manifest.toml");
    let script = include_str!("../../../examples/swap-log-copper/plugin.rhai");
    pexe::pack(manifest, script).expect("bundled swap packs")
}

#[cfg(test)]
/// The permissioned-currency example, packed from source.
pub(crate) fn test_usdc_bytes() -> Vec<u8> {
    let manifest = include_str!("../../../examples/usdc/manifest.toml");
    let script = include_str!("../../../examples/usdc/plugin.rhai");
    pexe::pack(manifest, script).expect("test usdc packs")
}

/// Order plugins so every plugin follows the ones it calls into.
///
/// A cycle is rejected: two plugins cannot each embed the other's batch id
/// in their own, so there is no order in which both could compile.
fn order_by_dependencies(plugins: Vec<Plugin>) -> Result<Vec<Plugin>> {
    let mut remaining = plugins;
    let mut ordered: Vec<Plugin> = Vec::with_capacity(remaining.len());
    let mut placed: HashSet<String> = HashSet::new();

    while !remaining.is_empty() {
        let ready = remaining.iter().position(|plugin| {
            script_dependencies(&plugin.script)
                .iter()
                .all(|dep| placed.contains(dep))
        });
        match ready {
            Some(idx) => {
                let plugin = remaining.remove(idx);
                placed.insert(plugin.manifest.plugin.name.clone());
                ordered.push(plugin);
            }
            None => {
                let stuck: Vec<String> = remaining
                    .iter()
                    .map(|plugin| {
                        let missing: Vec<String> = script_dependencies(&plugin.script)
                            .into_iter()
                            .filter(|dep| !placed.contains(dep))
                            .collect();
                        format!(
                            "{} needs {}",
                            plugin.manifest.plugin.name,
                            missing.join(", ")
                        )
                    })
                    .collect();
                return Err(anyhow!(
                    "cannot resolve plugin load order ({}); either a dependency is not installed or the plugins form a cycle",
                    stuck.join("; ")
                ));
            }
        }
    }
    Ok(ordered)
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

    fn obj_field_for_test(
        obj: &pod2::middleware::containers::Dictionary,
        name: &str,
    ) -> pod2::middleware::Value {
        obj.get(&pod2::middleware::StrKey::from(name))
            .expect("field read")
            .expect("field present")
            .clone()
    }

    fn obj_type_hash_for_test(obj: &pod2::middleware::containers::Dictionary) -> Option<Hash> {
        let value = obj.get(&pod2::middleware::StrKey::from("type")).ok()??;
        Some(Hash(value.raw().0))
    }

    /// The bundled swap example calls into craft-basics and craft-rocket
    /// from its script, and its manifest pins both via [[imports]]. Its own
    /// `module_hash` covers those imports, so any change to either
    /// dependency invalidates it until it is rebuilt -- which is what this
    /// checks, with the pins turning drift into per-import messages.
    #[test]
    fn test_bundled_swap_loads_against_bundled_plugins() {
        let catalog = PexeCatalog::from_bytes(
            [
                (PathBuf::from("swap-log-copper.pexe"), bundled_swap_bytes()),
                (PathBuf::from("craft-basics.pexe"), test_plugin_bytes()),
                (PathBuf::from("craft-rocket.pexe"), test_rocket_bytes()),
            ],
            true,
        )
        .expect("bundled swap loads -- if this fails, rebuild examples/swap-log-copper");

        let swap = catalog
            .get_action(&QualifiedName::new("swap-log-copper", "SwapLogCopper"))
            .expect("swap is a catalog action");
        let ids =
            |refs: &[ClassRef]| -> Vec<String> { refs.iter().map(|r| r.class.id()).collect() };
        // Consumes the dependencies' classes through the qualified calls, and
        // produces those plus its own receipt.
        assert_eq!(
            ids(&swap.total_inputs),
            vec!["craft-basics::Log", "craft-rocket::Copper"]
        );
        assert_eq!(
            ids(&swap.total_outputs),
            vec![
                "craft-basics::Log",
                "craft-rocket::Copper",
                "swap-log-copper::Swapped"
            ]
        );
    }

    /// The whole point, end to end: a plugin's script re-keys two objects
    /// whose classes another plugin defines, and mints one of its own, all in
    /// a single transaction.
    #[test]
    fn test_bundled_swap_executes() {
        let catalog = PexeCatalog::from_bytes(
            [
                (PathBuf::from("craft-basics.pexe"), test_plugin_bytes()),
                (PathBuf::from("craft-rocket.pexe"), test_rocket_bytes()),
                (PathBuf::from("swap-log-copper.pexe"), bundled_swap_bytes()),
            ],
            true,
        )
        .expect("bundled swap loads");
        let mut state = payload::test_state::TestState::default();

        let mut run = |action: QualifiedName, inputs: Vec<SpendableObject>| {
            let commitments: Vec<Hash> = inputs.iter().map(|i| i.obj.commitment()).collect();
            let witness = witness_for(&state, &commitments);
            let out = catalog
                .execute_action(action.clone(), witness, inputs)
                .unwrap_or_else(|err| panic!("{action} runs: {err}"));
            state.apply_tx(
                out.tx.live_commitments().unwrap(),
                out.tx.nullifier_hashes().unwrap(),
            );
            out
        };
        let log = run(QualifiedName::new("craft-basics", "FindLog"), vec![]).obj(0);
        let copper = run(QualifiedName::new("craft-rocket", "MineCopper"), vec![]).obj(0);

        let out = run(
            QualifiedName::new("swap-log-copper", "SwapLogCopper"),
            vec![log.clone(), copper.clone()],
        );

        let nullifiers = out.tx.nullifier_hashes().unwrap();
        assert_eq!(nullifiers.len(), 2, "both rekeys spent their input");
        assert!(nullifiers.contains(&txlib::object_nullifier_hash(&log.obj).unwrap()));
        assert!(nullifiers.contains(&txlib::object_nullifier_hash(&copper.obj).unwrap()));

        assert_eq!(out.objs.len(), 3, "log, copper, and the receipt");
        let live = out.tx.live_commitments().unwrap();
        for produced in &out.objs {
            assert!(live.contains(&produced.obj.commitment()));
        }

        // The rekeyed objects keep their defining plugins' classes; only the
        // receipt carries this plugin's own.
        assert_eq!(
            obj_type_hash_for_test(&out.obj(0).obj).unwrap(),
            obj_type_hash_for_test(&log.obj).unwrap()
        );
        assert_eq!(
            obj_type_hash_for_test(&out.obj(1).obj).unwrap(),
            obj_type_hash_for_test(&copper.obj).unwrap()
        );
        let swapped = catalog
            .get_class(&QualifiedName::new("swap-log-copper", "Swapped"))
            .expect("Swapped class present");
        assert_eq!(
            obj_type_hash_for_test(&out.obj(2).obj).unwrap(),
            decode_hash_hex(&swapped.hash).unwrap()
        );
    }

    /// The permissioned currency, end to end: authority is possession of the
    /// IssuerCap (mint and burn must mutate it, so each use nullifies it),
    /// the supply counter rides the cap, bills carry the cap's stamp through
    /// Halve, and rekeying is the handover step.
    #[test]
    fn test_usdc_lifecycle_executes() {
        let catalog =
            PexeCatalog::from_bytes([(PathBuf::from("usdc.pexe"), test_usdc_bytes())], true)
                .expect("usdc loads -- if this fails, rebuild examples/usdc");
        let mut state = payload::test_state::TestState::default();

        let mut run = |action: QualifiedName, inputs: Vec<SpendableObject>| {
            let commitments: Vec<Hash> = inputs.iter().map(|i| i.obj.commitment()).collect();
            let witness = witness_for(&state, &commitments);
            let out = catalog
                .execute_action(action.clone(), witness, inputs)
                .unwrap_or_else(|err| panic!("{action} runs: {err}"));
            state.apply_tx(
                out.tx.live_commitments().unwrap(),
                out.tx.nullifier_hashes().unwrap(),
            );
            out
        };
        let int_value = |n: i64| pod2::middleware::Value::from(n);

        let cap = run(QualifiedName::new("usdc", "Genesis"), vec![]).obj(0);
        assert_eq!(obj_field_for_test(&cap.obj, "total_issued"), int_value(0));
        let issuer_id = obj_field_for_test(&cap.obj, "issuer_id");

        let minted = run(QualifiedName::new("usdc", "Mint1024"), vec![cap]);
        assert_eq!(minted.objs.len(), 2, "the reproduced cap and the bill");
        let cap = minted.obj(0);
        let bill = minted.obj(1);
        assert_eq!(
            obj_field_for_test(&cap.obj, "total_issued"),
            int_value(1024)
        );
        assert_eq!(obj_field_for_test(&bill.obj, "amount"), int_value(1024));
        assert_eq!(obj_field_for_test(&bill.obj, "issuer"), issuer_id);

        let halved = run(QualifiedName::new("usdc", "Halve"), vec![bill]);
        assert_eq!(halved.objs.len(), 2);
        for half in &halved.objs {
            assert_eq!(obj_field_for_test(&half.obj, "amount"), int_value(512));
            assert_eq!(obj_field_for_test(&half.obj, "issuer"), issuer_id);
        }
        let (spend, keep) = (halved.obj(0), halved.obj(1));

        let rekeyed = run(QualifiedName::new("usdc", "RekeyUSDC"), vec![spend]).obj(0);
        assert_eq!(obj_field_for_test(&rekeyed.obj, "amount"), int_value(512));
        assert_eq!(obj_field_for_test(&rekeyed.obj, "issuer"), issuer_id);

        let burned = run(QualifiedName::new("usdc", "Burn"), vec![cap, rekeyed]);
        assert_eq!(burned.objs.len(), 1, "only the reproduced cap survives");
        let cap = burned.obj(0);
        assert_eq!(obj_field_for_test(&cap.obj, "total_issued"), int_value(512));

        // The gate itself: without the cap as its first input, Mint has
        // nothing of the right class to mutate. In production dobjd refuses
        // a wrong-class input before execution starts; fed directly to the
        // executor, the attempt aborts instead of returning -- either way it
        // must not produce a bill.
        let witness = witness_for(&state, &[keep.obj.commitment()]);
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            catalog.execute_action(QualifiedName::new("usdc", "Mint1024"), witness, vec![keep])
        }));
        assert!(
            !matches!(attempt, Ok(Ok(_))),
            "minting must require possession of the IssuerCap"
        );
    }

    /// A grounding witness over `state` covering the given inputs.
    fn witness_for(
        state: &payload::test_state::TestState,
        input_commitments: &[Hash],
    ) -> txlib::GroundingWitness {
        state.build_grounding_witness(
            input_commitments,
            |meta, created_root, nullifiers_root, prior_state_history_root, created_proofs| {
                txlib::GroundingWitness::new(
                    txlib::StateHeader::new(
                        meta.number as i64,
                        meta.timestamp as i64,
                        meta.hash,
                        created_root,
                        nullifiers_root,
                        prior_state_history_root,
                    ),
                    created_proofs,
                )
            },
        )
    }
}
