use std::collections::{BTreeSet, HashMap, HashSet};

use craft_sdk::api::{self, StepKind};

/// UI metadata for an action, provided by the plugin.
#[derive(Debug, Clone)]
pub struct ActionUiMeta {
    pub emoji: &'static str,
    pub description: &'static str,
    pub cpu_cost: &'static str,
    pub reads_block: bool,
    pub hidden: bool,
}

/// UI metadata for a class, provided by the plugin.
#[derive(Debug, Clone)]
pub struct ClassUiMeta {
    pub emoji: &'static str,
    pub description: &'static str,
}

/// Full descriptor for a single action, combining signatures with UI metadata.
#[derive(Debug, Clone)]
pub struct ActionDescriptor {
    pub name: String,
    pub input_classes: Vec<String>,
    pub output_classes: Vec<String>,
    pub ui: ActionUiMeta,
    pub hidden: bool,
}

/// The trait that every plugin must implement.
///
/// A plugin declares the object classes and crafting actions it provides.
/// In Phase 1 the plugin is statically linked; in Phase 2 it will be loaded
/// from a WASM module at runtime.
pub trait PluginSpec {
    /// Human-readable plugin name (e.g. "minecraft-basics").
    fn name(&self) -> &'static str;

    /// The intro-pod and module dependencies for proof generation.
    fn dependencies(&self) -> Vec<api::Dependency>;

    /// The full action definitions (with closures for proof generation).
    ///
    /// This must return a fresh `Vec` each time because the `Detail` closures
    /// inside `Step` are not `Clone`.
    fn actions(&self) -> Vec<api::Action>;

    /// UI metadata for each action, keyed by action name.
    fn action_ui_meta(&self) -> Vec<(&'static str, ActionUiMeta)>;

    /// UI metadata for each class, keyed by class name.
    fn class_ui_meta_entries(&self) -> Vec<(&'static str, ClassUiMeta)>;
}

// ---------------------------------------------------------------------------
// Derived helpers — generic over any PluginSpec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ActionSignature {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

fn action_signatures(actions: &[api::Action]) -> HashMap<String, ActionSignature> {
    let by_name: HashMap<&str, &api::Action> =
        actions.iter().map(|a| (a.name(), a)).collect();
    let mut cache = HashMap::<String, ActionSignature>::new();
    let mut visiting = HashSet::<String>::new();
    for action in actions {
        derive_signature(action.name(), &by_name, &mut cache, &mut visiting);
    }
    cache
}

fn derive_signature(
    action_name: &str,
    actions_by_name: &HashMap<&str, &api::Action>,
    cache: &mut HashMap<String, ActionSignature>,
    visiting: &mut HashSet<String>,
) -> ActionSignature {
    if let Some(sig) = cache.get(action_name) {
        return sig.clone();
    }
    if !visiting.insert(action_name.to_string()) {
        panic!("cyclic action dependency detected at {action_name}");
    }
    let action = actions_by_name
        .get(action_name)
        .unwrap_or_else(|| panic!("missing action definition for {action_name}"));
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for step in action.steps() {
        match step.kind() {
            StepKind::Input => {
                inputs.push(step.class().unwrap().to_string());
            }
            StepKind::Mutate => {
                let class = step.class().unwrap().to_string();
                inputs.push(class.clone());
                outputs.push(class);
            }
            StepKind::Output => {
                outputs.push(step.class().unwrap().to_string());
            }
            StepKind::Depends => {
                let dep = step.action().unwrap();
                let sig = derive_signature(dep, actions_by_name, cache, visiting);
                inputs.extend(sig.inputs);
                outputs.extend(sig.outputs);
            }
        }
    }
    visiting.remove(action_name);
    let sig = ActionSignature { inputs, outputs };
    cache.insert(action_name.to_string(), sig.clone());
    sig
}

/// Compute full action descriptors from a plugin spec.
pub fn action_descriptors(plugin: &dyn PluginSpec) -> Vec<ActionDescriptor> {
    let actions = plugin.actions();
    let signatures = action_signatures(&actions);
    let ui_map: HashMap<&str, ActionUiMeta> =
        plugin.action_ui_meta().into_iter().collect();

    let default_ui = ActionUiMeta {
        emoji: "⚙️",
        description: "SDK action",
        cpu_cost: "unknown",
        reads_block: false,
        hidden: false,
    };

    actions
        .into_iter()
        .map(|action| {
            let name = action.name().to_string();
            let sig = signatures.get(&name).unwrap().clone();
            let ui = ui_map.get(name.as_str()).cloned().unwrap_or(default_ui.clone());
            ActionDescriptor {
                name,
                input_classes: sig.inputs,
                output_classes: sig.outputs,
                hidden: ui.hidden,
                ui,
            }
        })
        .collect()
}

/// Only non-hidden action descriptors.
pub fn visible_action_descriptors(plugin: &dyn PluginSpec) -> Vec<ActionDescriptor> {
    action_descriptors(plugin)
        .into_iter()
        .filter(|d| !d.hidden)
        .collect()
}

/// Action descriptors indexed by name.
pub fn action_descriptors_by_name(
    plugin: &dyn PluginSpec,
) -> HashMap<String, ActionDescriptor> {
    action_descriptors(plugin)
        .into_iter()
        .map(|d| (d.name.clone(), d))
        .collect()
}

/// All known class names (sorted).
pub fn class_names(plugin: &dyn PluginSpec) -> Vec<String> {
    let mut classes = BTreeSet::<String>::new();
    for d in action_descriptors(plugin) {
        classes.extend(d.input_classes);
        classes.extend(d.output_classes);
    }
    for (name, _) in plugin.class_ui_meta_entries() {
        classes.insert(name.to_string());
    }
    classes.into_iter().collect()
}

/// Look up class UI metadata by name.
pub fn class_ui_meta(plugin: &dyn PluginSpec, class_name: &str) -> ClassUiMeta {
    plugin
        .class_ui_meta_entries()
        .into_iter()
        .find(|(name, _)| *name == class_name)
        .map(|(_, meta)| meta)
        .unwrap_or(ClassUiMeta {
            emoji: "📦",
            description: "Unknown class object",
        })
}
