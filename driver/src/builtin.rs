use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use anyhow::Result;
use sdk::{Sdk, SpendableObject, SpendableObjects};
use txlib::GroundingWitness;

use crate::catalog::{ActionCatalog, CatalogClass, extract_predicate};
use crate::types::ActionSummary;

struct ActionMetaInfo {
    name: &'static str,
    emoji: &'static str,
    description: &'static str,
    cpu_cost: &'static str,
    reads_block: bool,
    hidden: bool,
}

struct ClassMetaInfo {
    name: &'static str,
    emoji: &'static str,
    description: &'static str,
}

const ACTION_META: &[ActionMetaInfo] = &[
    ActionMetaInfo {
        name: "FindLog",
        emoji: "🌲",
        description: "Discover a log object by proving a short VDF.",
        cpu_cost: "20-40s",
        reads_block: false,
        hidden: false,
    },
    ActionMetaInfo {
        name: "CraftWood",
        emoji: "🪵",
        description: "Refine one log into a wood object with PoW quality checks.",
        cpu_cost: "15-30s",
        reads_block: false,
        hidden: false,
    },
    ActionMetaInfo {
        name: "CraftSticks",
        emoji: "🥢",
        description: "Split one wood object into two stick objects.",
        cpu_cost: "5-10s",
        reads_block: false,
        hidden: false,
    },
    ActionMetaInfo {
        name: "CraftWoodPick",
        emoji: "⛏️",
        description: "Combine wood and a stick to craft a wood pick.",
        cpu_cost: "10-20s",
        reads_block: false,
        hidden: false,
    },
    ActionMetaInfo {
        name: "UseWoodPick",
        emoji: "⛏️",
        description: "Internal durability/work update for wood pick usage.",
        cpu_cost: "10-30s",
        reads_block: false,
        hidden: true,
    },
];

const CLASS_META: &[ClassMetaInfo] = &[
    ClassMetaInfo {
        name: "Log",
        emoji: "🌲",
        description: "A discovered log that can be refined into wood.",
    },
    ClassMetaInfo {
        name: "Wood",
        emoji: "🪵",
        description: "Refined wood used for sticks and basic tools.",
    },
    ClassMetaInfo {
        name: "Stick",
        emoji: "🥢",
        description: "A stick used as a handle in tool crafting.",
    },
    ClassMetaInfo {
        name: "WoodPick",
        emoji: "⛏️",
        description: "A wood pick that can mine stone while durability remains.",
    },
];

pub(crate) const ACTION_NAMES: &[&str] = &[
    "FindLog",
    "CraftWood",
    "CraftSticks",
    "CraftWoodPick",
    "UseWoodPick",
];

pub(crate) const CRAFT_SCRIPT: &str = r#"
fn FindLog(action) {
    var log = action.output("Log");
    log.set([["blueprint", "Log"]]);
    var work = action.intro_vdf(3, log);
    log.update("work", work);
}

fn CraftWood(action) {
    var log = action.input("Log");
    var wood = action.output("Wood");
    wood.set([["blueprint", "Wood"]]);
    var key = action.pow_obj_grind(wood, 9007199254740992);
    wood.update("key", key);
    action.intro_lt_eq_u256(wood, 9007199254740992);
}

fn CraftSticks(action) {
    var wood = action.input("Wood");
    var stick_a = action.output("Stick");
    var stick_b = action.output("Stick");
    stick_a.set([["blueprint", "Stick"]]);
    stick_b.set([["blueprint", "Stick"]]);
}

fn CraftWoodPick(action) {
    var wood = action.input("Wood");
    var stick = action.input("Stick");
    var pick = action.output("WoodPick");
    pick.set([
        ["blueprint", "WoodPick"],
        ["durability", 100]
    ]);
}

fn use_pick(action, pick, vdf_iters) {
    action.st_gt(pick.durability, 0);
    var durability = unsafe { pick.durability - 1 };
    action.st_sum_of(pick.durability, durability, 1);
    pick.update("durability", durability);
    var key = action.random();
    pick.update("key", key);
    var work = action.intro_vdf(vdf_iters, pick);
    pick.update("work", work);
}

fn UseWoodPick(action) {
    var wood_pick = action.mutate("WoodPick");
    use_pick(action, wood_pick, 10);
}
"#;

pub struct BuiltinActionCatalog {
    actions: Vec<ActionSummary>,
    actions_by_name: HashMap<String, ActionSummary>,
    classes: Vec<CatalogClass>,
    classes_by_name: HashMap<String, CatalogClass>,
    podlang_src: String,
}

impl BuiltinActionCatalog {
    pub fn new() -> Self {
        let sdk = Sdk::default();
        let module = sdk
            .load_module_from_src_actions(CRAFT_SCRIPT, ACTION_NAMES)
            .expect("builtin craft script compiles");

        let podlang_src = module.podlang_src().to_string();

        let action_meta_by_name: HashMap<&str, &ActionMetaInfo> =
            ACTION_META.iter().map(|m| (m.name, m)).collect();

        let actions: Vec<ActionSummary> = module
            .actions()
            .iter()
            .filter_map(|action| {
                let name = action.name.as_str();
                let meta = action_meta_by_name.get(name);
                if meta.is_some_and(|m| m.hidden) {
                    return None;
                }
                let input_classes: Vec<String> =
                    action.inputs.iter().map(|(_o, c)| c.clone()).collect();
                let output_classes: Vec<String> =
                    action.outputs.iter().map(|(_o, c)| c.clone()).collect();
                Some(ActionSummary {
                    id: name.to_string(),
                    emoji: meta.map_or("⚙️", |m| m.emoji).to_string(),
                    hash: module
                        .action_hash(name)
                        .map(|h| format!("{:#}", h))
                        .unwrap_or_default(),
                    input_class_hashes: input_classes
                        .iter()
                        .map(|c| {
                            module
                                .class_hash(c)
                                .map(|h| format!("{:#}", h))
                                .unwrap_or_default()
                        })
                        .collect(),
                    description: meta.map_or("SDK action", |m| m.description).to_string(),
                    cpu_cost: meta.map_or("unknown", |m| m.cpu_cost).to_string(),
                    reads_block: meta.is_some_and(|m| m.reads_block),
                    input_classes,
                    output_classes,
                })
            })
            .collect();
        let actions_by_name: HashMap<String, ActionSummary> = actions
            .iter()
            .map(|action| (action.id.clone(), action.clone()))
            .collect();

        let class_meta_by_name: HashMap<&str, &ClassMetaInfo> =
            CLASS_META.iter().map(|m| (m.name, m)).collect();
        let mut class_name_set = BTreeSet::<String>::new();
        for action in module.actions() {
            for (_, class) in &action.inputs {
                class_name_set.insert(class.clone());
            }
            for (_, class) in &action.outputs {
                class_name_set.insert(class.clone());
            }
        }
        for cm in CLASS_META {
            class_name_set.insert(cm.name.to_string());
        }

        let classes: Vec<CatalogClass> = class_name_set
            .into_iter()
            .map(|class_name| {
                let cm = class_meta_by_name.get(class_name.as_str());
                let produced_by = actions
                    .iter()
                    .filter(|a| a.output_classes.contains(&class_name))
                    .map(|a| a.id.clone())
                    .collect();
                let consumed_by = actions
                    .iter()
                    .filter(|a| a.input_classes.contains(&class_name))
                    .map(|a| a.id.clone())
                    .collect();
                let predicate_source = extract_predicate(&podlang_src, &format!("Is{class_name}"))
                    .unwrap_or_else(|| format!("Is{class_name}(state) = OR(...)"));
                CatalogClass {
                    name: class_name.clone(),
                    emoji: cm.map_or("📦", |m| m.emoji).to_string(),
                    hash: module
                        .class_hash(&class_name)
                        .map(|h| format!("{:#}", h))
                        .unwrap_or_default(),
                    description: cm
                        .map_or("Unknown class object", |m| m.description)
                        .to_string(),
                    produced_by,
                    consumed_by,
                    predicate_source,
                }
            })
            .collect();
        let classes_by_name: HashMap<String, CatalogClass> = classes
            .iter()
            .map(|c| (c.name.clone(), c.clone()))
            .collect();

        Self {
            actions,
            actions_by_name,
            classes,
            classes_by_name,
            podlang_src,
        }
    }
}

impl Default for BuiltinActionCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionCatalog for BuiltinActionCatalog {
    fn list_actions(&self) -> Vec<ActionSummary> {
        self.actions.clone()
    }

    fn get_action(&self, action_id: &str) -> Option<ActionSummary> {
        self.actions_by_name.get(action_id).cloned()
    }

    fn list_classes(&self) -> Vec<CatalogClass> {
        self.classes.clone()
    }

    fn get_class(&self, class_name: &str) -> Option<CatalogClass> {
        self.classes_by_name.get(class_name).cloned()
    }

    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        execute_with_script(&action_id, grounding_witness, inputs, false)
    }

    fn generated_podlang(&self) -> Option<String> {
        Some(self.podlang_src.clone())
    }
}

/// Run an action against the built-in craft script. Used by the catalog and by tests.
pub(crate) fn execute_with_script(
    action_id: &str,
    grounding_witness: GroundingWitness,
    inputs: Vec<SpendableObject>,
    mock: bool,
) -> Result<SpendableObjects> {
    let sdk = Sdk::default();
    let module = sdk.load_module_from_src_actions(CRAFT_SCRIPT, ACTION_NAMES)?;
    let executor = module.executor(mock, Arc::new(grounding_witness));
    Ok(executor.action(action_id, inputs)?)
}

#[cfg(test)]
mod tests {
    use super::BuiltinActionCatalog;
    use crate::catalog::ActionCatalog;

    #[test]
    fn test_builtin_catalog_hides_internal_actions() {
        let catalog = BuiltinActionCatalog::new();
        let action_ids = catalog
            .list_actions()
            .into_iter()
            .map(|action| action.id)
            .collect::<Vec<_>>();
        assert!(action_ids.contains(&"CraftWood".to_string()));
        assert!(!action_ids.contains(&"UseWoodPick".to_string()));
    }

    #[test]
    fn test_builtin_catalog_lists_classes() {
        let catalog = BuiltinActionCatalog::new();
        let class_names = catalog
            .list_classes()
            .into_iter()
            .map(|class_info| class_info.name)
            .collect::<Vec<_>>();
        assert!(class_names.contains(&"Log".to_string()));
        assert!(class_names.contains(&"WoodPick".to_string()));
    }
}
