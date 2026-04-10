use std::collections::HashMap;

use anyhow::{anyhow, bail};

use crate::ops::CraftOps;
use crate::types::*;

/// Mock implementation of CraftOps for testing.
/// Returns realistic fixtures matching the zk-craft game.
/// Multiple actions can run concurrently
pub struct MockCraftOps {
    inventory: Vec<InventoryObject>,
    actions: Vec<Action>,
    state_root: String,
}

impl MockCraftOps {
    pub fn new() -> Self {
        Self {
            inventory: default_inventory(),
            actions: default_actions(),
            state_root: "0x9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b".to_string(),
        }
    }

    /// Create a mock with a custom inventory.
    pub fn with_inventory(mut self, inventory: Vec<InventoryObject>) -> Self {
        self.inventory = inventory;
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
                let live_count = self
                    .inventory
                    .iter()
                    .filter(|o| o.class_name == name && o.status == "live")
                    .count();
                let produced_by = self
                    .actions
                    .iter()
                    .filter(|a| a.output_classes.contains(&name.to_string()))
                    .map(|a| a.id.clone())
                    .collect();
                let consumed_by = self
                    .actions
                    .iter()
                    .filter(|a| a.input_classes.contains(&name.to_string()))
                    .map(|a| a.id.clone())
                    .collect();
                ClassSummary {
                    name: name.to_string(),
                    live_count,
                    produced_by,
                    consumed_by,
                }
            })
            .collect();
        classes.sort_by(|a, b| a.name.cmp(&b.name));
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
            class_name: obj.class_name.clone(),
            status: obj.status.clone(),
            tx_hash: obj.tx_hash.clone(),
            state: obj.fields.clone(),
            predicate_source: predicate_source_for(&obj.class_name),
        })
    }

    fn inspect_class(&self, class_name: &str) -> anyhow::Result<ClassDetail> {
        let actions = &self.actions;
        let produced_by = actions
            .iter()
            .filter(|a| a.output_classes.contains(&class_name.to_string()))
            .map(|a| a.id.clone())
            .collect();
        let consumed_by = actions
            .iter()
            .filter(|a| a.input_classes.contains(&class_name.to_string()))
            .map(|a| a.id.clone())
            .collect();

        if !is_known_class(class_name) {
            bail!("unknown class: {class_name}");
        }

        Ok(ClassDetail {
            class_name: class_name.to_string(),
            predicate_source: predicate_source_for(class_name),
            produced_by,
            consumed_by,
        })
    }

    fn run_action(&self, input: RunActionInput) -> anyhow::Result<RunActionResult> {
        // Validate the action exists
        if !self.actions.iter().any(|a| a.id == input.action_id) {
            bail!("unknown action: {}", input.action_id);
        }

        Ok(RunActionResult {
            success: true,
            message: format!("Action {} completed successfully", input.action_id),
            outputs: vec![InventoryObject {
                id: "0xnew1234567890abcdef".to_string(),
                class_name: "Wood".to_string(),
                file_name: "Wood.dobj".to_string(),
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
        let mut missing = Vec::new();

        for required_class in &action.input_classes {
            if let Some(obj) = self
                .inventory
                .iter()
                .find(|o| &o.class_name == required_class && o.status == "live")
            {
                available.push(FeasibilityInput {
                    class_name: obj.class_name.clone(),
                    object_id: obj.id.clone(),
                    file_name: obj.file_name.clone(),
                });
            } else {
                missing.push(required_class.clone());
            }
        }

        Ok(FeasibilityReport {
            feasible: missing.is_empty(),
            action_id: action_id.to_string(),
            available_inputs: available,
            missing_inputs: missing,
        })
    }
}

fn default_inventory() -> Vec<InventoryObject> {
    vec![
        InventoryObject {
            id: "0xabc1111111111111".to_string(),
            class_name: "Log".to_string(),
            file_name: "Log.dobj".to_string(),
            status: "live".to_string(),
            tx_hash: Some("0xmocktx1111111111".to_string()),
            fields: HashMap::from([
                (
                    "blueprint".to_string(),
                    serde_json::Value::String("Log".to_string()),
                ),
                (
                    "key".to_string(),
                    serde_json::Value::String("0xabc1111111111111".to_string()),
                ),
            ]),
        },
        InventoryObject {
            id: "0xabc2222222222222".to_string(),
            class_name: "Wood".to_string(),
            file_name: "Wood.dobj".to_string(),
            status: "live".to_string(),
            tx_hash: Some("0xmocktx2222222222".to_string()),
            fields: HashMap::from([
                (
                    "blueprint".to_string(),
                    serde_json::Value::String("Wood".to_string()),
                ),
                (
                    "key".to_string(),
                    serde_json::Value::String("0xabc2222222222222".to_string()),
                ),
            ]),
        },
        InventoryObject {
            id: "0xabc3333333333333".to_string(),
            class_name: "Stick".to_string(),
            file_name: "Stick.dobj".to_string(),
            status: "live".to_string(),
            tx_hash: Some("0xmocktx3333333333".to_string()),
            fields: HashMap::from([
                (
                    "blueprint".to_string(),
                    serde_json::Value::String("Stick".to_string()),
                ),
                (
                    "key".to_string(),
                    serde_json::Value::String("0xabc3333333333333".to_string()),
                ),
            ]),
        },
        InventoryObject {
            id: "0xabc4444444444444".to_string(),
            class_name: "WoodPick".to_string(),
            file_name: "WoodPick.dobj".to_string(),
            status: "live".to_string(),
            tx_hash: Some("0xmocktx4444444444".to_string()),
            fields: HashMap::from([
                (
                    "blueprint".to_string(),
                    serde_json::Value::String("WoodPick".to_string()),
                ),
                (
                    "durability".to_string(),
                    serde_json::Value::Number(3.into()),
                ),
                (
                    "key".to_string(),
                    serde_json::Value::String("0xabc4444444444444".to_string()),
                ),
            ]),
        },
        InventoryObject {
            id: "0xabc5555555555555".to_string(),
            class_name: "Stone".to_string(),
            file_name: "Stone.dobj".to_string(),
            status: "live".to_string(),
            tx_hash: Some("0xmocktx5555555555".to_string()),
            fields: HashMap::from([
                (
                    "blueprint".to_string(),
                    serde_json::Value::String("Stone".to_string()),
                ),
                (
                    "key".to_string(),
                    serde_json::Value::String("0xabc5555555555555".to_string()),
                ),
            ]),
        },
        // A nullified object to test liveness filtering
        InventoryObject {
            id: "0xdead000000000000".to_string(),
            class_name: "Log".to_string(),
            file_name: "Log_old.dobj".to_string(),
            status: "nullified".to_string(),
            tx_hash: Some("0xmocktxdead000000".to_string()),
            fields: HashMap::from([(
                "blueprint".to_string(),
                serde_json::Value::String("Log".to_string()),
            )]),
        },
    ]
}

fn default_actions() -> Vec<Action> {
    vec![
        Action {
            id: "FindLog".to_string(),
            description: "Discover a log by proving a short VDF".to_string(),
            input_classes: vec![],
            output_classes: vec!["Log".to_string()],
            cpu_cost: "~20-40s".to_string(),
        },
        Action {
            id: "CraftWood".to_string(),
            description: "Refine one log into wood".to_string(),
            input_classes: vec!["Log".to_string()],
            output_classes: vec!["Wood".to_string()],
            cpu_cost: "~15-30s".to_string(),
        },
        Action {
            id: "CraftSticks".to_string(),
            description: "Split one wood into two sticks".to_string(),
            input_classes: vec!["Wood".to_string()],
            output_classes: vec!["Stick".to_string(), "Stick".to_string()],
            cpu_cost: "~5-10s".to_string(),
        },
        Action {
            id: "CraftWoodPick".to_string(),
            description: "Combine wood and stick to craft a wood pickaxe".to_string(),
            input_classes: vec!["Wood".to_string(), "Stick".to_string()],
            output_classes: vec!["WoodPick".to_string()],
            cpu_cost: "~10-20s".to_string(),
        },
        Action {
            id: "CraftStonePick".to_string(),
            description: "Combine stone and stick to craft a stone pickaxe".to_string(),
            input_classes: vec!["Stone".to_string(), "Stick".to_string()],
            output_classes: vec!["StonePick".to_string()],
            cpu_cost: "~10-20s".to_string(),
        },
        Action {
            id: "MineStoneWithWoodPick".to_string(),
            description: "Mine stone using a wood pick (consumes durability)".to_string(),
            input_classes: vec!["WoodPick".to_string()],
            output_classes: vec!["Stone".to_string(), "WoodPick".to_string()],
            cpu_cost: "~25-45s".to_string(),
        },
        Action {
            id: "MineStoneWithStonePick".to_string(),
            description: "Mine stone using a stone pick (consumes durability)".to_string(),
            input_classes: vec!["StonePick".to_string()],
            output_classes: vec!["Stone".to_string(), "StonePick".to_string()],
            cpu_cost: "~15-35s".to_string(),
        },
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
        let classes: Vec<&str> = inv.iter().map(|o| o.class_name.as_str()).collect();
        assert!(classes.contains(&"Log"));
        assert!(classes.contains(&"Wood"));
        assert!(classes.contains(&"Stick"));
        assert!(classes.contains(&"WoodPick"));
        assert!(classes.contains(&"Stone"));
    }

    #[test]
    fn test_inspect_object_found() {
        let mock = MockCraftOps::new();
        let detail = mock.inspect_object("0xabc1111111111111").unwrap();
        assert_eq!(detail.class_name, "Log");
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
        let detail = mock.inspect_class("Wood").unwrap();
        assert!(detail.produced_by.contains(&"CraftWood".to_string()));
        assert!(detail.consumed_by.contains(&"CraftSticks".to_string()));
        assert!(detail.consumed_by.contains(&"CraftWoodPick".to_string()));
    }

    #[test]
    fn test_inspect_unknown_class() {
        let mock = MockCraftOps::new();
        assert!(mock.inspect_class("Diamond").is_err());
    }

    #[test]
    fn test_check_feasibility_feasible() {
        let mock = MockCraftOps::new();
        let report = mock.check_feasibility("CraftWoodPick").unwrap();
        assert!(report.feasible);
        assert!(report.missing_inputs.is_empty());
        assert_eq!(report.available_inputs.len(), 2);
    }

    #[test]
    fn test_check_feasibility_missing() {
        let mock = MockCraftOps::new();
        let report = mock.check_feasibility("CraftStonePick").unwrap();
        // We have Stone and Stick, so this should be feasible
        assert!(report.feasible);
    }

    #[test]
    fn test_check_feasibility_unknown_action() {
        let mock = MockCraftOps::new();
        assert!(mock.check_feasibility("CraftDiamond").is_err());
    }

    #[test]
    fn test_run_action_success() {
        let mock = MockCraftOps::new();
        let result = mock
            .run_action(RunActionInput {
                action_id: "CraftWood".to_string(),
                input_object_paths: vec!["Log.dobj".to_string()],
            })
            .unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_run_action_concurrent() {
        use std::sync::Arc;
        let mock = Arc::new(MockCraftOps::new());
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let mock = mock.clone();
                std::thread::spawn(move || {
                    mock.run_action(RunActionInput {
                        action_id: "FindLog".to_string(),
                        input_object_paths: vec![],
                    })
                })
            })
            .collect();
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_ok());
            assert!(result.unwrap().success);
        }
    }

    #[test]
    fn test_run_action_unknown() {
        let mock = MockCraftOps::new();
        let result = mock.run_action(RunActionInput {
            action_id: "CraftDiamond".to_string(),
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
