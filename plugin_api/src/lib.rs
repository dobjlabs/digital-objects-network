//! Shared types for the `.pexe` plugin system.
//!
//! This crate defines the metadata structures exchanged between WASM plugins
//! and the native host. It has no dependencies on `craft_sdk`, `pod2`, or
//! `extism` so it compiles for both native and `wasm32-unknown-unknown`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Plugin metadata (returned by the WASM plugin's `get_metadata` export)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<DependencyMeta>,
    pub classes: Vec<ClassMeta>,
    pub actions: Vec<ActionMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyMeta {
    pub dep_type: DependencyType,
    pub pred: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DependencyType {
    Intro,
    Module,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassMeta {
    pub name: String,
    pub emoji: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionMeta {
    pub name: String,
    pub emoji: String,
    pub description: String,
    pub cpu_cost: String,
    pub reads_block: bool,
    pub hidden: bool,
    pub steps: Vec<StepMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepMeta {
    pub kind: StepKindMeta,
    pub name: String,
    /// Class name (for Input/Output/Mutate steps).
    #[serde(default)]
    pub class: String,
    /// Sub-action name (for Depends steps).
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub details: Vec<DetailMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepKindMeta {
    Input,
    Output,
    Mutate,
    Depends,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DetailMeta {
    Set {
        key: String,
        value: LiteralValue,
    },
    Update {
        key: String,
        source: String,
    },
    Var {
        name: String,
        recipe: VarRecipe,
    },
    Condition {
        pred: String,
        recipe: ConditionRecipe,
    },
}

/// Recipes for computing variable values at proof-generation time.
///
/// The host has a built-in handler for each recipe variant. Plugins describe
/// WHAT to compute; the host knows HOW (using native pod2/plonky2 code).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum VarRecipe {
    /// Run a VDF for `iters` iterations on the current object state.
    /// Stores `vdf_pod` and `st_vdf` in context storage.
    Vdf { iters: usize },
    /// Brute-force random keys until `commitment < difficulty`.
    PowGrind { difficulty: u64 },
    /// Read a field, decrement by 1, store result under the field name.
    DecrementField { key: String },
    /// Generate a random key value.
    RandomKey,
}

/// Recipes for generating proof conditions (ZK statements).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConditionRecipe {
    /// Add the VDF pod and statement previously stored by a `Vdf` var recipe.
    StoredVdfPod,
    /// Generate an LtEqU256 proof that the object's raw value ≤ difficulty.
    LtEqU256 { difficulty: u64 },
    /// Prove `obj.key > value`.
    Gt { key: String, value: i64 },
    /// Prove `obj.key = stored_var + b` (reads `stored_var` from context storage).
    SumOf {
        key: String,
        stored_var: String,
        b: i64,
    },
}

/// A literal value for Set details.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LiteralValue {
    Int(i64),
    Str(String),
}

// ---------------------------------------------------------------------------
// Host-side query types (used by the app after loading a plugin)
// ---------------------------------------------------------------------------

/// Full descriptor for a single action, combining I/O signatures with UI info.
#[derive(Debug, Clone)]
pub struct ActionDescriptor {
    pub name: String,
    pub input_classes: Vec<String>,
    pub output_classes: Vec<String>,
    pub ui: ActionUiMeta,
    pub hidden: bool,
}

/// UI metadata for an action.
#[derive(Debug, Clone)]
pub struct ActionUiMeta {
    pub emoji: String,
    pub description: String,
    pub cpu_cost: String,
    pub reads_block: bool,
    pub hidden: bool,
}

/// UI metadata for a class.
#[derive(Debug, Clone)]
pub struct ClassUiMeta {
    pub emoji: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Helpers: derive descriptors from metadata
// ---------------------------------------------------------------------------

use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone)]
struct ActionSignature {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

fn action_signatures(actions: &[ActionMeta]) -> HashMap<String, ActionSignature> {
    let by_name: HashMap<&str, &ActionMeta> =
        actions.iter().map(|a| (a.name.as_str(), a)).collect();
    let mut cache = HashMap::new();
    let mut visiting = HashSet::new();
    for action in actions {
        derive_signature(&action.name, &by_name, &mut cache, &mut visiting);
    }
    cache
}

fn derive_signature(
    action_name: &str,
    actions_by_name: &HashMap<&str, &ActionMeta>,
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
    for step in &action.steps {
        match step.kind {
            StepKindMeta::Input => inputs.push(step.class.clone()),
            StepKindMeta::Mutate => {
                inputs.push(step.class.clone());
                outputs.push(step.class.clone());
            }
            StepKindMeta::Output => outputs.push(step.class.clone()),
            StepKindMeta::Depends => {
                let sig = derive_signature(&step.action, actions_by_name, cache, visiting);
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

/// Build action descriptors from plugin metadata.
pub fn action_descriptors(meta: &PluginMetadata) -> Vec<ActionDescriptor> {
    let signatures = action_signatures(&meta.actions);
    meta.actions
        .iter()
        .map(|action| {
            let sig = signatures.get(&action.name).unwrap();
            ActionDescriptor {
                name: action.name.clone(),
                input_classes: sig.inputs.clone(),
                output_classes: sig.outputs.clone(),
                hidden: action.hidden,
                ui: ActionUiMeta {
                    emoji: action.emoji.clone(),
                    description: action.description.clone(),
                    cpu_cost: action.cpu_cost.clone(),
                    reads_block: action.reads_block,
                    hidden: action.hidden,
                },
            }
        })
        .collect()
}

/// Only non-hidden action descriptors.
pub fn visible_action_descriptors(meta: &PluginMetadata) -> Vec<ActionDescriptor> {
    action_descriptors(meta)
        .into_iter()
        .filter(|d| !d.hidden)
        .collect()
}

/// Action descriptors indexed by name.
pub fn action_descriptors_by_name(meta: &PluginMetadata) -> HashMap<String, ActionDescriptor> {
    action_descriptors(meta)
        .into_iter()
        .map(|d| (d.name.clone(), d))
        .collect()
}

/// All known class names (sorted).
pub fn class_names(meta: &PluginMetadata) -> Vec<String> {
    let mut classes = BTreeSet::new();
    for d in action_descriptors(meta) {
        classes.extend(d.input_classes);
        classes.extend(d.output_classes);
    }
    for c in &meta.classes {
        classes.insert(c.name.clone());
    }
    classes.into_iter().collect()
}

/// Look up class UI metadata by name.
pub fn class_ui_meta(meta: &PluginMetadata, class_name: &str) -> ClassUiMeta {
    meta.classes
        .iter()
        .find(|c| c.name == class_name)
        .map(|c| ClassUiMeta {
            emoji: c.emoji.clone(),
            description: c.description.clone(),
        })
        .unwrap_or(ClassUiMeta {
            emoji: "📦".to_string(),
            description: "Unknown class object".to_string(),
        })
}
