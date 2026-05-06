use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, bail};

use crate::ops::CraftOps;
use crate::types::*;

const PLUGIN: &str = "craft-basics";

fn qid(name: &str) -> String {
    format!("{PLUGIN}::{name}")
}

/// Mock implementation of CraftOps for testing.
/// Returns realistic fixtures matching the zk-craft game.
pub struct MockCraftOps {
    inventory: Vec<InventoryObject>,
    actions: Vec<Action>,
    state_root: String,
    /// Simulates mutual exclusion: only one action at a time.
    action_in_progress: Mutex<bool>,
}

impl MockCraftOps {
    pub fn new() -> Self {
        Self {
            inventory: default_inventory(),
            actions: default_actions(),
            state_root: "0x9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b".to_string(),
            action_in_progress: Mutex::new(false),
        }
    }

    /// Create a mock with a custom inventory.
    pub fn with_inventory(mut self, inventory: Vec<InventoryObject>) -> Self {
        self.inventory = inventory;
        self
    }

    /// Create a mock that simulates an action already in progress.
    pub fn with_action_in_progress(self) -> Self {
        *self.action_in_progress.lock().unwrap() = true;
        self
    }
}

impl Default for MockCraftOps {
    fn default() -> Self {
        Self::new()
    }
}

impl CraftOps for MockCraftOps {
    fn list_inventory(&self) -> anyhow::Result<Vec<InventoryObject>> {
        Ok(self.inventory.clone())
    }

    fn list_actions(&self) -> anyhow::Result<Vec<Action>> {
        Ok(self.actions.clone())
    }

    fn list_classes(&self) -> anyhow::Result<Vec<ClassSummary>> {
        let mut classes: Vec<ClassSummary> = KNOWN_CLASSES
            .iter()
            .map(|&name| {
                let class_id = qid(name);
                let live_count = self
                    .inventory
                    .iter()
                    .filter(|o| o.class_id == class_id && o.status == "live")
                    .count();
                let produced_by = self
                    .actions
                    .iter()
                    .filter(|a| a.total_outputs.iter().any(|r| r.id == class_id))
                    .map(|a| a.id.clone())
                    .collect();
                let consumed_by = self
                    .actions
                    .iter()
                    .filter(|a| a.total_inputs.iter().any(|r| r.id == class_id))
                    .map(|a| a.id.clone())
                    .collect();
                ClassSummary {
                    id: class_id,
                    display_name: name.to_string(),
                    plugin_name: PLUGIN.to_string(),
                    live_count,
                    produced_by,
                    consumed_by,
                }
            })
            .collect();
        classes.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        Ok(classes)
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        Ok(self.state_root.clone())
    }

    fn inspect_object(&self, object_id: &str) -> anyhow::Result<ObjectDetail> {
        let obj = self
            .inventory
            .iter()
            .find(|o| o.id == object_id)
            .ok_or_else(|| anyhow!("object not found: {object_id}"))?;

        Ok(ObjectDetail {
            id: obj.id.clone(),
            class_id: obj.class_id.clone(),
            class_display_name: obj.class_display_name.clone(),
            plugin_name: obj.plugin_name.clone(),
            status: obj.status.clone(),
            tx_hash: obj.tx_hash.clone(),
            state: obj.fields.clone(),
            predicate_source: predicate_source_for(&obj.class_display_name),
        })
    }

    fn inspect_class(&self, class_id: &str) -> anyhow::Result<ClassDetail> {
        let actions = &self.actions;
        let produced_by = actions
            .iter()
            .filter(|a| a.total_outputs.iter().any(|r| r.id == class_id))
            .map(|a| a.id.clone())
            .collect();
        let consumed_by = actions
            .iter()
            .filter(|a| a.total_inputs.iter().any(|r| r.id == class_id))
            .map(|a| a.id.clone())
            .collect();

        let display_name = class_id
            .strip_prefix(&format!("{PLUGIN}::"))
            .unwrap_or(class_id);
        if !is_known_class(display_name) {
            bail!("unknown class: {class_id}");
        }

        Ok(ClassDetail {
            class_id: class_id.to_string(),
            class_display_name: display_name.to_string(),
            plugin_name: PLUGIN.to_string(),
            predicate_source: predicate_source_for(display_name),
            produced_by,
            consumed_by,
        })
    }

    fn run_action(&self, input: RunActionInput) -> anyhow::Result<RunActionResult> {
        let mut in_progress = self.action_in_progress.lock().unwrap();
        if *in_progress {
            bail!("an action is already in progress");
        }

        // Validate the action exists
        if !self.actions.iter().any(|a| a.id == input.action_id) {
            bail!("unknown action: {}", input.action_id);
        }

        *in_progress = true;
        // Simulate completion (in real impl this would block for proof generation)
        *in_progress = false;

        Ok(RunActionResult {
            success: true,
            message: format!("Action {} completed successfully", input.action_id),
            outputs: vec![InventoryObject {
                id: "0xnew1234567890abcdef".to_string(),
                class_id: qid("Wood"),
                class_display_name: "Wood".to_string(),
                plugin_name: PLUGIN.to_string(),
                file_name: "craft-basics__wood_0xnew.dobj".to_string(),
                status: "live".to_string(),
                tx_hash: Some("0xmocktxnew12345678".to_string()),
                fields: HashMap::from([
                    (
                        "blueprint".to_string(),
                        serde_json::Value::String("Wood".to_string()),
                    ),
                    (
                        "key".to_string(),
                        serde_json::Value::String("0xnew1234567890abcdef".to_string()),
                    ),
                ]),
            }],
            consumed: input.input_object_paths,
        })
    }

    fn check_feasibility(&self, action_id: &str) -> anyhow::Result<FeasibilityReport> {
        let action = self
            .actions
            .iter()
            .find(|a| a.id == action_id)
            .ok_or_else(|| anyhow!("unknown action: {action_id}"))?;

        let mut available = Vec::new();
        let mut missing_inputs = Vec::new();

        for required in &action.total_inputs {
            if let Some(obj) = self
                .inventory
                .iter()
                .find(|o| o.class_id == required.id && o.status == "live")
            {
                available.push(FeasibilityInput {
                    class_id: obj.class_id.clone(),
                    class_display_name: obj.class_display_name.clone(),
                    plugin_name: obj.plugin_name.clone(),
                    object_id: obj.id.clone(),
                    file_name: obj.file_name.clone(),
                });
            } else {
                missing_inputs.push(required.clone());
            }
        }

        Ok(FeasibilityReport {
            feasible: missing_inputs.is_empty(),
            action_id: action_id.to_string(),
            available_inputs: available,
            missing_inputs,
        })
    }
}

fn make_obj(
    id: &str,
    class: &str,
    file_name: &str,
    tx_hash: &str,
    status: &str,
    extra: Vec<(&str, serde_json::Value)>,
) -> InventoryObject {
    let mut fields = HashMap::from([
        (
            "blueprint".to_string(),
            serde_json::Value::String(class.to_string()),
        ),
        ("key".to_string(), serde_json::Value::String(id.to_string())),
    ]);
    for (k, v) in extra {
        fields.insert(k.to_string(), v);
    }
    InventoryObject {
        id: id.to_string(),
        class_id: qid(class),
        class_display_name: class.to_string(),
        plugin_name: PLUGIN.to_string(),
        file_name: file_name.to_string(),
        status: status.to_string(),
        tx_hash: Some(tx_hash.to_string()),
        fields,
    }
}

fn default_inventory() -> Vec<InventoryObject> {
    vec![
        make_obj(
            "0xabc1111111111111",
            "Log",
            "craft-basics__log_0xabc1.dobj",
            "0xmocktx1111111111",
            "live",
            vec![],
        ),
        make_obj(
            "0xabc2222222222222",
            "Wood",
            "craft-basics__wood_0xabc2.dobj",
            "0xmocktx2222222222",
            "live",
            vec![],
        ),
        make_obj(
            "0xabc3333333333333",
            "Stick",
            "craft-basics__stick_0xabc3.dobj",
            "0xmocktx3333333333",
            "live",
            vec![],
        ),
        make_obj(
            "0xabc4444444444444",
            "WoodPick",
            "craft-basics__woodpick_0xabc4.dobj",
            "0xmocktx4444444444",
            "live",
            vec![("durability", serde_json::Value::Number(3.into()))],
        ),
        make_obj(
            "0xabc5555555555555",
            "Stone",
            "craft-basics__stone_0xabc5.dobj",
            "0xmocktx5555555555",
            "live",
            vec![],
        ),
        // A nullified object to test liveness filtering
        make_obj(
            "0xdead000000000000",
            "Log",
            "craft-basics__log_0xdead.dobj",
            "0xmocktxdead000000",
            "nullified",
            vec![],
        ),
    ]
}

fn make_action(name: &str, description: &str, inputs: &[&str], outputs: &[&str]) -> Action {
    let to_refs = |classes: &[&str]| -> Vec<ClassRef> {
        classes
            .iter()
            .map(|c| ClassRef {
                id: qid(c),
                display_name: c.to_string(),
                hash: format!("0x{}", "0".repeat(64)),
            })
            .collect()
    };
    Action {
        id: qid(name),
        display_name: name.to_string(),
        plugin_name: PLUGIN.to_string(),
        description: description.to_string(),
        total_inputs: to_refs(inputs),
        total_outputs: to_refs(outputs),
    }
}

fn default_actions() -> Vec<Action> {
    vec![
        make_action(
            "FindLog",
            "Discover a log by proving a short VDF",
            &[],
            &["Log"],
        ),
        make_action("CraftWood", "Refine one log into wood", &["Log"], &["Wood"]),
        make_action(
            "CraftSticks",
            "Split one wood into two sticks",
            &["Wood"],
            &["Stick", "Stick"],
        ),
        make_action(
            "CraftWoodPick",
            "Combine wood and stick to craft a wood pickaxe",
            &["Wood", "Stick"],
            &["WoodPick"],
        ),
        make_action(
            "CraftStonePick",
            "Combine stone and stick to craft a stone pickaxe",
            &["Stone", "Stick"],
            &["StonePick"],
        ),
        make_action(
            "MineStoneWithWoodPick",
            "Mine stone using a wood pick (consumes durability)",
            &["WoodPick"],
            &["Stone", "WoodPick"],
        ),
        make_action(
            "MineStoneWithStonePick",
            "Mine stone using a stone pick (consumes durability)",
            &["StonePick"],
            &["Stone", "StonePick"],
        ),
    ]
}

const KNOWN_CLASSES: &[&str] = &["Log", "Wood", "Stick", "WoodPick", "Stone", "StonePick"];

fn is_known_class(name: &str) -> bool {
    KNOWN_CLASSES.contains(&name)
}

fn predicate_source_for(class_name: &str) -> String {
    match class_name {
        "Log" => "IsLog(state) = AND(\n  FindLog(state)\n)".to_string(),
        "Wood" => "IsWood(state) = AND(\n  CraftWood(state, log)\n)".to_string(),
        "Stick" => "IsStick(state) = AND(\n  CraftSticks(state, wood)\n)".to_string(),
        "WoodPick" => {
            "IsWoodPick(state) = OR(\n  CraftWoodPick(state, wood, stick)\n  UseWoodPick(state, prev)\n)"
                .to_string()
        }
        "Stone" => {
            "IsStone(state) = OR(\n  MineStoneWithWoodPick(state, pick)\n  MineStoneWithStonePick(state, pick)\n)"
                .to_string()
        }
        "StonePick" => {
            "IsStonePick(state) = OR(\n  CraftStonePick(state, stone, stick)\n  UseStonePick(state, prev)\n)"
                .to_string()
        }
        _ => format!("Is{class_name}(state) = UNKNOWN"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_inventory_has_all_classes() {
        let mock = MockCraftOps::new();
        let inv = mock.list_inventory().unwrap();
        let names: Vec<&str> = inv.iter().map(|o| o.class_display_name.as_str()).collect();
        assert!(names.contains(&"Log"));
        assert!(names.contains(&"Wood"));
        assert!(names.contains(&"Stick"));
        assert!(names.contains(&"WoodPick"));
        assert!(names.contains(&"Stone"));
    }

    #[test]
    fn test_inspect_object_found() {
        let mock = MockCraftOps::new();
        let detail = mock.inspect_object("0xabc1111111111111").unwrap();
        assert_eq!(detail.class_display_name, "Log");
        assert_eq!(detail.status, "live");
        assert!(detail.predicate_source.contains("FindLog"));
    }

    #[test]
    fn test_inspect_object_not_found() {
        let mock = MockCraftOps::new();
        assert!(mock.inspect_object("0xnonexistent").is_err());
    }

    #[test]
    fn test_inspect_class() {
        let mock = MockCraftOps::new();
        let detail = mock.inspect_class("craft-basics::Wood").unwrap();
        assert!(detail.produced_by.contains(&qid("CraftWood")));
        assert!(detail.consumed_by.contains(&qid("CraftSticks")));
        assert!(detail.consumed_by.contains(&qid("CraftWoodPick")));
    }

    #[test]
    fn test_inspect_unknown_class() {
        let mock = MockCraftOps::new();
        assert!(mock.inspect_class("craft-basics::Diamond").is_err());
    }

    #[test]
    fn test_check_feasibility_feasible() {
        let mock = MockCraftOps::new();
        let report = mock.check_feasibility(&qid("CraftWoodPick")).unwrap();
        assert!(report.feasible);
        assert!(report.missing_inputs.is_empty());
        assert_eq!(report.available_inputs.len(), 2);
    }

    #[test]
    fn test_check_feasibility_missing() {
        let mock = MockCraftOps::new();
        let report = mock.check_feasibility(&qid("CraftStonePick")).unwrap();
        // We have Stone and Stick, so this should be feasible
        assert!(report.feasible);
    }

    #[test]
    fn test_check_feasibility_unknown_action() {
        let mock = MockCraftOps::new();
        assert!(mock.check_feasibility(&qid("CraftDiamond")).is_err());
    }

    #[test]
    fn test_run_action_success() {
        let mock = MockCraftOps::new();
        let result = mock
            .run_action(RunActionInput {
                action_id: qid("CraftWood"),
                input_object_paths: vec!["craft-basics__log_0xabc1.dobj".to_string()],
            })
            .unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_run_action_already_in_progress() {
        let mock = MockCraftOps::new().with_action_in_progress();
        let result = mock.run_action(RunActionInput {
            action_id: qid("CraftWood"),
            input_object_paths: vec!["craft-basics__log_0xabc1.dobj".to_string()],
        });
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already in progress")
        );
    }

    #[test]
    fn test_run_action_unknown() {
        let mock = MockCraftOps::new();
        let result = mock.run_action(RunActionInput {
            action_id: qid("CraftDiamond"),
            input_object_paths: vec![],
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_get_state_root() {
        let mock = MockCraftOps::new();
        let root = mock.get_state_root().unwrap();
        assert!(root.starts_with("0x"));
    }
}
