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
//! differ (each module has a unique `module_hash`). Cross-plugin class
//! references are not supported: an action must reference classes declared
//! in its own plugin.
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
use sdk::{Sdk, SpendableObject, SpendableObjects, manifest::Manifest};
use txlib::GroundingWitness;

use crate::catalog::{ActionCatalog, CatalogClass, extract_predicate};
use wire_types::{ActionSummary, ClassRef, QualifiedName};

struct Plugin {
    #[allow(dead_code)]
    path: PathBuf,
    manifest: Manifest,
    /// Absent for a recipe pexe, which composes other plugins' actions
    /// and compiles to no module of its own.
    script: Option<String>,
}

/// A catalog action assembled from other plugins' actions rather than
/// compiled from a script. Its steps run as sibling top-level actions of
/// one transaction, and its inputs are the steps' inputs concatenated in
/// step order.
struct RecipeEntry {
    steps: Vec<QualifiedName>,
}

pub struct PexeCatalog {
    plugins: Vec<Plugin>,
    actions: Vec<ActionSummary>,
    actions_by_name: HashMap<QualifiedName, ActionSummary>,
    /// Maps qualified action -> plugin index in `plugins`.
    action_plugin_idx: HashMap<QualifiedName, usize>,
    /// Every action including hidden ones, so recipe steps can resolve
    /// actions the catalog does not surface on its own.
    actions_including_hidden: HashMap<QualifiedName, ActionSummary>,
    /// Recipe actions, keyed by their own qualified name.
    recipes: HashMap<QualifiedName, RecipeEntry>,
    classes: Vec<CatalogClass>,
    classes_by_name: HashMap<QualifiedName, CatalogClass>,
    classes_by_hash: HashMap<Hash, QualifiedName>,
    combined_podlang_src: String,
    mock_proofs: bool,
}

impl PexeCatalog {
    /// Recompile the plugin that provides `action`. The driver does not
    /// cache compiled modules, so this runs per execution.
    fn load_module(&self, sdk: &Sdk, action: &QualifiedName) -> Result<Rc<sdk::SdkModule>> {
        let plugin_idx = *self
            .action_plugin_idx
            .get(action)
            .ok_or_else(|| anyhow!("no plugin provides action {action}"))?;
        let plugin = &self.plugins[plugin_idx];
        let script = plugin.script.as_deref().ok_or_else(|| {
            anyhow!(
                "plugin {} has no script, so it cannot run {action}",
                plugin.manifest.plugin.name
            )
        })?;
        sdk.load_module_from_src_manifest(script, &plugin.manifest)
            .map_err(|err| {
                anyhow!(
                    "failed to reload plugin {} for execution: {err}",
                    plugin.manifest.plugin.name
                )
            })
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

        let mut actions_including_hidden: HashMap<QualifiedName, ActionSummary> = HashMap::new();

        for plugin in plugins {
            let plugin_name = plugin.manifest.plugin.name.clone();
            // Recipes contribute no classes or predicates, so they are
            // resolved after every plugin is loaded and their steps exist.
            if plugin.manifest.is_recipe() {
                enriched_plugins.push(plugin);
                continue;
            }
            let script = plugin.script.as_deref().ok_or_else(|| {
                anyhow!("plugin {plugin_name} has no script and declares no recipes")
            })?;
            let module = sdk
                .load_module_from_src_manifest(script, &plugin.manifest)
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
                let resolve_class = |class_name: &str| -> Result<ClassRef> {
                    let hash = class_hashes.get(class_name).ok_or_else(|| {
                        anyhow!(
                            "plugin {plugin_name}: action {bare} references class {class_name:?} \
                             which is not declared in this plugin (cross-plugin class \
                             references are not supported yet)"
                        )
                    })?;
                    Ok(ClassRef {
                        class: QualifiedName::new(plugin_name.clone(), class_name.to_string()),
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
                actions_including_hidden.insert(summary.action.clone(), summary.clone());

                if meta.is_some_and(|m| m.hidden) {
                    continue;
                }
                all_actions.push(summary);
            }

            enriched_plugins.push(plugin);
        }

        // Recipe pass: every plugin is loaded, so a recipe's steps can be
        // resolved and its inputs derived from them.
        let installed_hashes: HashMap<&str, Option<Hash>> = enriched_plugins
            .iter()
            .map(|plugin| {
                (
                    plugin.manifest.plugin.name.as_str(),
                    plugin.manifest.plugin.module_hash,
                )
            })
            .collect();
        let mut recipes: HashMap<QualifiedName, RecipeEntry> = HashMap::new();
        for (plugin_idx, plugin) in enriched_plugins.iter().enumerate() {
            if !plugin.manifest.is_recipe() {
                continue;
            }
            let plugin_name = plugin.manifest.plugin.name.clone();

            // A required plugin present at a different hash is a different
            // set of classes, so its actions are not the ones this recipe
            // was written against.
            let mut required: HashSet<&str> = HashSet::new();
            for require in &plugin.manifest.requires {
                required.insert(require.plugin.as_str());
                match installed_hashes.get(require.plugin.as_str()) {
                    None => {
                        return Err(anyhow!(
                            "recipe {plugin_name} requires plugin {} which is not installed",
                            require.plugin
                        ));
                    }
                    Some(None) => {
                        return Err(anyhow!(
                            "recipe {plugin_name} requires plugin {} at {:#}, but that plugin declares no module hash",
                            require.plugin,
                            require.module_hash
                        ));
                    }
                    Some(Some(installed)) if *installed != require.module_hash => {
                        return Err(anyhow!(
                            "recipe {plugin_name} requires plugin {} at {:#}, but it is installed at {:#}; rebuild the recipe against the installed version",
                            require.plugin,
                            require.module_hash,
                            installed
                        ));
                    }
                    Some(Some(_)) => {}
                }
            }

            for recipe in &plugin.manifest.recipes {
                let qname = QualifiedName::new(plugin_name.clone(), recipe.name.clone());
                if recipe.steps.is_empty() {
                    return Err(anyhow!("recipe {qname} declares no steps"));
                }
                let mut steps = Vec::with_capacity(recipe.steps.len());
                let mut total_inputs = Vec::new();
                let mut total_outputs = Vec::new();
                for step in &recipe.steps {
                    let step_name = QualifiedName::parse(step)
                        .map_err(|err| anyhow!("recipe {qname}: {err}"))?;
                    if !required.contains(step_name.plugin_name.as_str()) {
                        return Err(anyhow!(
                            "recipe {qname} runs {step_name} but does not require plugin {}",
                            step_name.plugin_name
                        ));
                    }
                    let step_action =
                        actions_including_hidden.get(&step_name).ok_or_else(|| {
                            anyhow!("recipe {qname} runs {step_name}, which no plugin provides")
                        })?;
                    total_inputs.extend(step_action.total_inputs.iter().cloned());
                    total_outputs.extend(step_action.total_outputs.iter().cloned());
                    steps.push(step_name);
                }

                if let Some(prior) = action_plugin_idx.insert(qname.clone(), plugin_idx) {
                    return Err(anyhow!(
                        "duplicate action qualified name {qname} (already mapped to plugin idx {prior})"
                    ));
                }
                let summary = ActionSummary {
                    action: qname.clone(),
                    emoji: recipe.emoji.clone(),
                    hash: String::new(),
                    description: recipe.description.clone(),
                    total_inputs,
                    total_outputs,
                    predicate_source: format!(
                        "// recipe: one transaction running\n//   {}",
                        recipe.steps.join("\n//   ")
                    ),
                };
                actions_including_hidden.insert(qname.clone(), summary.clone());
                all_actions.push(summary);
                recipes.insert(qname, RecipeEntry { steps });
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
            plugins: enriched_plugins,
            actions_including_hidden,
            recipes,
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

        if let Some(recipe) = self.recipes.get(&action) {
            // Inputs arrive in the same order the recipe's `total_inputs`
            // concatenated them, so each step takes the next slice.
            let mut remaining = inputs.into_iter();
            let mut modules: Vec<Rc<sdk::SdkModule>> = Vec::with_capacity(recipe.steps.len());
            let mut invocations = Vec::with_capacity(recipe.steps.len());
            for step in &recipe.steps {
                let module = self.load_module(&sdk, step)?;
                let arity = self
                    .actions_including_hidden
                    .get(step)
                    .ok_or_else(|| anyhow!("recipe {action} runs unknown step {step}"))?
                    .total_inputs
                    .len();
                let step_inputs: Vec<SpendableObject> = remaining.by_ref().take(arity).collect();
                if step_inputs.len() != arity {
                    return Err(anyhow!(
                        "recipe {action} ran out of inputs at step {step}: it needs {arity} more"
                    ));
                }
                modules.push(module.clone());
                invocations.push(sdk::Invocation {
                    module,
                    action: step.name.clone(),
                    inputs: step_inputs,
                });
            }
            if remaining.next().is_some() {
                return Err(anyhow!(
                    "recipe {action} was given more inputs than its steps consume"
                ));
            }
            let executor = sdk::Executor::with_modules(modules, self.mock_proofs, witness)?;
            return Ok(executor.actions(invocations)?);
        }

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
    pexe::pack(manifest, Some(script)).expect("test plugin packs")
}

#[cfg(test)]
/// The bundled recipe example, packed from source like the plugin above.
pub(crate) fn bundled_recipe_bytes() -> Vec<u8> {
    let manifest = include_str!("../../../examples/swap-log-wood/manifest.toml");
    pexe::pack(manifest, None).expect("bundled recipe packs")
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
        pexe::pack(&with_hash, Some(script)).expect("pack synthetic plugin")
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

    fn obj_type_hash_for_test(obj: &pod2::middleware::containers::Dictionary) -> Option<Hash> {
        let value = obj.get(&pod2::middleware::StrKey::from("type")).ok()??;
        Some(Hash(value.raw().0))
    }

    /// The bundled recipe must stay loadable against the bundled
    /// craft-basics. `pexe build` re-pins a plugin's own `module_hash` but
    /// never a recipe's `[[requires]]`, so any change to craft-basics
    /// silently staleness this pin until something checks it.
    #[test]
    fn test_bundled_recipe_matches_bundled_plugin() {
        let catalog = PexeCatalog::from_bytes(
            [
                (PathBuf::from("craft-basics.pexe"), test_plugin_bytes()),
                (PathBuf::from("swap-log-wood.pexe"), bundled_recipe_bytes()),
            ],
            true,
        )
        .expect("bundled recipe loads against bundled craft-basics -- if this fails, re-pin examples/swap-log-wood/manifest.toml to the hash `pexe build examples/craft-basics` prints");

        let recipe = catalog
            .get_action(&QualifiedName::new("swap-log-wood", "SwapLogWood"))
            .expect("bundled recipe is a catalog action");
        let classes: Vec<&str> = recipe
            .total_inputs
            .iter()
            .map(|r| r.class.name.as_str())
            .collect();
        assert_eq!(classes, vec!["Log", "Wood"]);
        assert!(
            recipe
                .total_inputs
                .iter()
                .all(|r| r.class.plugin_name == "craft-basics"),
            "the recipe consumes craft-basics classes, not its own"
        );
    }

    // --- Recipe fixtures -----------------------------------------------------
    //
    // A recipe pexe carries no script. It pins the plugins it composes by
    // module hash and lists qualified actions to run as one transaction.

    const CLAIM_SCRIPT: &str = r#"
fn MakeFoo(action) {
    var foo = action.output("Foo");
    foo.set([["durability", 100]]);
    var key = action.random();
    foo.update("key", key);
}

fn MakeBar(action) {
    var bar = action.output("Bar");
    bar.set([["durability", 100]]);
    var key = action.random();
    bar.update("key", key);
}

fn ClaimFoo(action) {
    var foo = action.mutate("Foo");
    var key = action.random();
    foo.update("key", key);
}

fn ClaimBar(action) {
    var bar = action.mutate("Bar");
    var key = action.random();
    bar.update("key", key);
}
"#;

    /// A plugin exposing claim actions, i.e. the extension surface a base
    /// plugin has to publish before recipes can compose it.
    fn claims_plugin_bytes(plugin_name: &str) -> Vec<u8> {
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
name = "MakeBar"
emoji = "B"
description = "make a Bar"

[[actions]]
name = "ClaimFoo"
emoji = "F"
description = "take possession of a Foo"

[[actions]]
name = "ClaimBar"
emoji = "B"
description = "take possession of a Bar"
"#
        );
        let manifest: sdk::manifest::Manifest =
            toml::from_str(&template).expect("claims manifest parses");
        let real_hash =
            pexe::compile_module_hash(&manifest, CLAIM_SCRIPT).expect("claims script compiles");
        let with_hash =
            pexe::set_manifest_hash(&template, &real_hash).expect("rewrite module_hash");
        pexe::pack(&with_hash, Some(CLAIM_SCRIPT)).expect("pack claims plugin")
    }

    /// The module hash a recipe must pin to compose `claims_plugin_bytes`.
    fn claims_module_hash(plugin_name: &str) -> String {
        let bytes = claims_plugin_bytes(plugin_name);
        let (manifest, _) = pexe::unpack(&bytes).expect("claims pexe unpacks");
        format!(
            "{:#}",
            manifest.plugin.module_hash.expect("plugin has a hash")
        )
        .trim_start_matches("0x")
        .to_string()
    }

    fn recipe_test_witness(
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

    /// The end of the whole chain: a recipe from one pexe consuming and
    /// re-keying objects whose classes were defined by another, in a single
    /// transaction, driven through the ordinary single-action entry point.
    #[test]
    fn test_recipe_runs_its_steps_as_one_transaction() {
        let catalog = claims_and_recipe_catalog();
        let mut state = payload::test_state::TestState::default();

        let mut mint = |action: &str| {
            let out = catalog
                .execute_action(
                    QualifiedName::new("base", action),
                    dummy_grounding_witness(),
                    vec![],
                )
                .unwrap_or_else(|err| panic!("base::{action} runs: {err}"));
            state.apply_tx(
                out.tx.live_commitments().unwrap(),
                out.tx.nullifier_hashes().unwrap(),
            );
            out.obj(0)
        };
        let foo = mint("MakeFoo");
        let bar = mint("MakeBar");

        let witness = recipe_test_witness(&state, &[foo.obj.commitment(), bar.obj.commitment()]);
        let out = catalog
            .execute_action(
                QualifiedName::new("swap", "SwapFooBar"),
                witness,
                vec![foo.clone(), bar.clone()],
            )
            .expect("recipe runs");

        // One transaction spent both inputs, so the pair cannot half-land.
        let nullifiers = out.tx.nullifier_hashes().unwrap();
        assert_eq!(nullifiers.len(), 2, "both inputs spent by the one tx");
        assert!(nullifiers.contains(&txlib::object_nullifier_hash(&foo.obj).unwrap()));
        assert!(nullifiers.contains(&txlib::object_nullifier_hash(&bar.obj).unwrap()));

        let live = out.tx.live_commitments().unwrap();
        assert_eq!(out.objs.len(), 2, "one successor per claimed object");
        for produced in &out.objs {
            assert!(live.contains(&produced.obj.commitment()));
        }

        // Each successor keeps the class its own plugin defined: a recipe
        // cannot mint into a class, only re-key within one.
        let foo_type = obj_type_hash_for_test(&foo.obj).unwrap();
        let bar_type = obj_type_hash_for_test(&bar.obj).unwrap();
        assert_eq!(obj_type_hash_for_test(&out.obj(0).obj).unwrap(), foo_type);
        assert_eq!(obj_type_hash_for_test(&out.obj(1).obj).unwrap(), bar_type);

        // Re-keying moves every commitment.
        assert_ne!(out.obj(0).obj.commitment(), foo.obj.commitment());
        assert_ne!(out.obj(1).obj.commitment(), bar.obj.commitment());
    }

    fn recipe_bytes(recipe_name: &str, requires: &str, module_hash: &str, steps: &str) -> Vec<u8> {
        let manifest = format!(
            r#"[plugin]
name = "{recipe_name}"
version = "0.1.0"

[[requires]]
plugin = "{requires}"
module_hash = "{module_hash}"

[[recipes]]
name = "SwapFooBar"
emoji = "S"
description = "re-key one Foo and one Bar in a single transaction"
steps = [{steps}]
"#
        );
        pexe::pack(&manifest, None).expect("pack recipe")
    }

    fn claims_and_recipe_catalog() -> PexeCatalog {
        let hash = claims_module_hash("base");
        PexeCatalog::from_bytes(
            [
                (PathBuf::from("base.pexe"), claims_plugin_bytes("base")),
                (
                    PathBuf::from("swap.pexe"),
                    recipe_bytes(
                        "swap",
                        "base",
                        &hash,
                        r#""base::ClaimFoo", "base::ClaimBar""#,
                    ),
                ),
            ],
            true,
        )
        .expect("catalog loads plugin plus recipe")
    }

    #[test]
    fn test_recipe_surfaces_as_an_action_with_the_steps_inputs() {
        let catalog = claims_and_recipe_catalog();
        let recipe = catalog
            .get_action(&QualifiedName::new("swap", "SwapFooBar"))
            .expect("recipe is a catalog action");

        // The recipe consumes and produces exactly what its steps do, in
        // step order, which is what lets it run through the ordinary
        // single-action request path.
        let classes = |refs: &[ClassRef]| -> Vec<String> {
            refs.iter().map(|r| r.class.name.clone()).collect()
        };
        assert_eq!(classes(&recipe.total_inputs), vec!["Foo", "Bar"]);
        assert_eq!(classes(&recipe.total_outputs), vec!["Foo", "Bar"]);
        // Its classes stay owned by the plugin that declared them.
        assert_eq!(recipe.total_inputs[0].class.plugin_name, "base");
    }

    #[test]
    fn test_recipe_requiring_a_different_module_hash_is_rejected() {
        let wrong = "1".repeat(64);
        let result = PexeCatalog::from_bytes(
            [
                (PathBuf::from("base.pexe"), claims_plugin_bytes("base")),
                (
                    PathBuf::from("swap.pexe"),
                    recipe_bytes("swap", "base", &wrong, r#""base::ClaimFoo""#),
                ),
            ],
            true,
        );
        let err = result
            .err()
            .map(|err| err.to_string())
            .unwrap_or_else(|| panic!("stale pin must be rejected"));
        assert!(err.contains("installed at"), "unexpected error: {err}");
        assert!(
            err.contains("rebuild the recipe"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_recipe_requiring_a_missing_plugin_is_rejected() {
        let hash = claims_module_hash("base");
        let result = PexeCatalog::from_bytes(
            std::iter::once((
                PathBuf::from("swap.pexe"),
                recipe_bytes("swap", "base", &hash, r#""base::ClaimFoo""#),
            )),
            true,
        );
        let err = result
            .err()
            .map(|err| err.to_string())
            .unwrap_or_else(|| panic!("missing plugin must be rejected"));
        assert!(err.contains("is not installed"), "unexpected error: {err}");
    }

    #[test]
    fn test_recipe_step_outside_its_requires_is_rejected() {
        let hash = claims_module_hash("base");
        let result = PexeCatalog::from_bytes(
            [
                (PathBuf::from("base.pexe"), claims_plugin_bytes("base")),
                (
                    PathBuf::from("swap.pexe"),
                    recipe_bytes("swap", "base", &hash, r#""other::ClaimFoo""#),
                ),
            ],
            true,
        );
        let err = result
            .err()
            .map(|err| err.to_string())
            .unwrap_or_else(|| panic!("step outside requires must be rejected"));
        assert!(
            err.contains("does not require plugin"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_recipe_pexe_with_a_script_is_rejected() {
        let hash = claims_module_hash("base");
        let manifest = format!(
            r#"[plugin]
name = "swap"
version = "0.1.0"

[[requires]]
plugin = "base"
module_hash = "{hash}"

[[recipes]]
name = "SwapFooBar"
emoji = "S"
description = "re-key one Foo and one Bar"
steps = ["base::ClaimFoo"]
"#
        );
        let bytes = pexe::pack(&manifest, Some(CLAIM_SCRIPT)).expect("pack");
        let err = pexe::unpack(&bytes)
            .expect_err("a recipe carrying a script must be rejected")
            .to_string();
        assert!(
            err.contains("has no script of its own"),
            "unexpected error: {err}"
        );
    }
}
