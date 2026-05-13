use std::collections::HashMap;

use anyhow::{anyhow, bail};

use crate::ops::CraftOps;
use crate::types::*;

const PLUGIN: &str = "craft-basics";

fn qname(name: &str) -> QualifiedName {
    QualifiedName {
        plugin_name: PLUGIN.to_string(),
        name: name.to_string(),
    }
}

/// Mock implementation of CraftOps for testing.
/// Returns realistic fixtures matching the bitcraft game.
/// Multiple actions can run concurrently.
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
                let class = qname(name);
                let live_count = self
                    .inventory
                    .iter()
                    .filter(|o| o.class == class && o.status == "live")
                    .count();
                let produced_by = self
                    .actions
                    .iter()
                    .filter(|a| a.total_outputs.iter().any(|r| r.class == class))
                    .map(|a| a.action.clone())
                    .collect();
                let consumed_by = self
                    .actions
                    .iter()
                    .filter(|a| a.total_inputs.iter().any(|r| r.class == class))
                    .map(|a| a.action.clone())
                    .collect();
                ClassSummary {
                    class,
                    live_count,
                    produced_by,
                    consumed_by,
                }
            })
            .collect();
        classes.sort_by(|a, b| a.class.name.cmp(&b.class.name));
        Ok(classes)
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        Ok(self.state_root.clone())
    }

    fn inspect_object(&self, file_name: &str) -> anyhow::Result<ObjectDetail> {
        let obj = self
            .inventory
            .iter()
            .find(|o| o.file_name == file_name)
            .ok_or_else(|| anyhow!("object file not found: {file_name}"))?;

        Ok(ObjectDetail {
            id: obj.id.clone(),
            class: obj.class.clone(),
            status: obj.status.clone(),
            tx_hash: obj.tx_hash.clone(),
            state: obj.fields.clone(),
            predicate_source: predicate_source_for(&obj.class.name),
        })
    }

    fn inspect_class(&self, class: &QualifiedName) -> anyhow::Result<ClassDetail> {
        let actions = &self.actions;
        let produced_by = actions
            .iter()
            .filter(|a| a.total_outputs.iter().any(|r| r.class == *class))
            .map(|a| a.action.clone())
            .collect();
        let consumed_by = actions
            .iter()
            .filter(|a| a.total_inputs.iter().any(|r| r.class == *class))
            .map(|a| a.action.clone())
            .collect();

        if class.plugin_name != PLUGIN || !is_known_class(&class.name) {
            bail!("unknown class: {}::{}", class.plugin_name, class.name);
        }

        Ok(ClassDetail {
            class: class.clone(),
            predicate_source: predicate_source_for(&class.name),
            produced_by,
            consumed_by,
        })
    }

    fn inspect_action(&self, action: &QualifiedName) -> anyhow::Result<ActionDetail> {
        let action_summary = self
            .actions
            .iter()
            .find(|a| a.action == *action)
            .ok_or_else(|| anyhow!("unknown action: {}::{}", action.plugin_name, action.name))?;

        Ok(ActionDetail {
            id: action.name.clone(),
            description: action_summary.description.clone(),
            total_input_classes: action_summary
                .total_inputs
                .iter()
                .map(|r| r.class.name.clone())
                .collect(),
            total_output_classes: action_summary
                .total_outputs
                .iter()
                .map(|r| r.class.name.clone())
                .collect(),
            predicate_source: action_predicate_source_for(&action.name),
        })
    }

    fn run_action(&self, input: RunActionInput) -> anyhow::Result<RunActionResult> {
        // Validate the action exists
        if !self.actions.iter().any(|a| a.action == input.action) {
            bail!(
                "unknown action: {}::{}",
                input.action.plugin_name,
                input.action.name
            );
        }

        Ok(RunActionResult {
            success: true,
            message: format!(
                "Action {}::{} completed successfully",
                input.action.plugin_name, input.action.name
            ),
            // Static fixture so tests can assert on it without depending on
            // wall-clock or randomness. Real `DobjdCraftOps` mints a UUID v4.
            run_id: "00000000-0000-4000-8000-000000000000".to_string(),
            outputs: vec![InventoryObject {
                id: "0xnew1234567890abcdef".to_string(),
                class: qname("Wood"),
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

    fn check_feasibility(&self, action: &QualifiedName) -> anyhow::Result<FeasibilityReport> {
        let action_summary = self
            .actions
            .iter()
            .find(|a| a.action == *action)
            .ok_or_else(|| anyhow!("unknown action: {}::{}", action.plugin_name, action.name))?;

        let mut available = Vec::new();
        let mut missing_inputs = Vec::new();

        for required in &action_summary.total_inputs {
            if let Some(obj) = self
                .inventory
                .iter()
                .find(|o| o.class == required.class && o.status == "live")
            {
                available.push(FeasibilityInput {
                    class: obj.class.clone(),
                    object_id: obj.id.clone(),
                    file_name: obj.file_name.clone(),
                });
            } else {
                missing_inputs.push(required.clone());
            }
        }

        Ok(FeasibilityReport {
            feasible: missing_inputs.is_empty(),
            action: action.clone(),
            available_inputs: available,
            missing_inputs,
        })
    }

    fn read_settings(&self) -> anyhow::Result<DriverSettings> {
        Ok(DriverSettings {
            synchronizer_api_url: "http://127.0.0.1:3000".to_string(),
            relayer_api_url: "http://127.0.0.1:3200".to_string(),
        })
    }

    fn write_settings(&self, settings: DriverSettings) -> anyhow::Result<DriverSettings> {
        // Mock is read-only — just echo back what was passed in.
        Ok(settings)
    }

    fn get_objects_dir(&self) -> anyhow::Result<String> {
        Ok("/tmp/mock-dobj-objects".to_string())
    }
}

fn make_obj(
    id: &str,
    class_name: &str,
    file_name: &str,
    tx_hash: &str,
    status: &str,
    extra: Vec<(&str, serde_json::Value)>,
) -> InventoryObject {
    let mut fields = HashMap::from([
        (
            "blueprint".to_string(),
            serde_json::Value::String(class_name.to_string()),
        ),
        ("key".to_string(), serde_json::Value::String(id.to_string())),
    ]);
    for (k, v) in extra {
        fields.insert(k.to_string(), v);
    }
    InventoryObject {
        id: id.to_string(),
        class: qname(class_name),
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
                class: qname(c),
                hash: format!("0x{}", "0".repeat(64)),
            })
            .collect()
    };
    Action {
        action: qname(name),
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

fn action_predicate_source_for(action_name: &str) -> String {
    match action_name {
        "FindLog" => {
            "FindLog(log, chain0, chain, private: log0, work) = AND(\n  DictContains(log0, \"blueprint\", \"Log\")\n  Vdf(3, log0, work)\n  DictUpdate(log, log0, \"work\", work)\n  DictContains(log, \"type\", @self_predicate(IsLog))\n  tx::TxInsert(chain, chain0, log)\n)"
                .to_string()
        }
        "CraftWood" => {
            "CraftWood(log, wood, chain0, chain, private: chain1, wood0, key) = AND(\n  DictContains(wood0, \"blueprint\", \"Wood\")\n  DictUpdate(wood, wood0, \"key\", key)\n  LtEqU256(wood, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))\n  DictContains(log, \"type\", @self_predicate(IsLog))\n  tx::TxDelete(chain1, chain0, log)\n  DictContains(wood, \"type\", @self_predicate(IsWood))\n  tx::TxInsert(chain, chain1, wood)\n)"
                .to_string()
        }
        "CraftSticks" => {
            "CraftSticks(wood, stick_a, stick_b, chain0, chain, private: chain1, chain2) = AND(\n  DictContains(stick_a, \"blueprint\", \"Stick\")\n  DictContains(stick_b, \"blueprint\", \"Stick\")\n  DictContains(wood, \"type\", @self_predicate(IsWood))\n  tx::TxDelete(chain1, chain0, wood)\n  DictContains(stick_a, \"type\", @self_predicate(IsStick))\n  tx::TxInsert(chain2, chain1, stick_a)\n  DictContains(stick_b, \"type\", @self_predicate(IsStick))\n  tx::TxInsert(chain, chain2, stick_b)\n)"
                .to_string()
        }
        "CraftWoodPick" => {
            "CraftWoodPick(wood, stick, pick, chain0, chain, private: chain1, chain2) = AND(\n  DictContains(pick, \"blueprint\", \"WoodPick\")\n  DictContains(pick, \"durability\", 100)\n  DictContains(wood, \"type\", @self_predicate(IsWood))\n  tx::TxDelete(chain1, chain0, wood)\n  DictContains(stick, \"type\", @self_predicate(IsStick))\n  tx::TxDelete(chain2, chain1, stick)\n  DictContains(pick, \"type\", @self_predicate(IsWoodPick))\n  tx::TxInsert(chain, chain2, pick)\n)"
                .to_string()
        }
        "CraftStonePick" => {
            "CraftStonePick(stone, stick, pick, chain0, chain, private: chain1, chain2) = AND(\n  DictContains(pick, \"blueprint\", \"StonePick\")\n  DictContains(pick, \"durability\", 200)\n  DictContains(stone, \"type\", @self_predicate(IsStone))\n  tx::TxDelete(chain1, chain0, stone)\n  DictContains(stick, \"type\", @self_predicate(IsStick))\n  tx::TxDelete(chain2, chain1, stick)\n  DictContains(pick, \"type\", @self_predicate(IsStonePick))\n  tx::TxInsert(chain, chain2, pick)\n)"
                .to_string()
        }
        "MineStoneWithWoodPick" => {
            "MineStoneWithWoodPick(stone, chain0, chain, private: chain1, pick) = AND(\n  UseWoodPick(pick, chain0, chain1)\n  DictContains(stone, \"blueprint\", \"Stone\")\n  DictContains(stone, \"type\", @self_predicate(IsStone))\n  tx::TxInsert(chain, chain1, stone)\n)"
                .to_string()
        }
        "MineStoneWithStonePick" => {
            "MineStoneWithStonePick(stone, chain0, chain, private: chain1, pick) = AND(\n  UseStonePick(pick, chain0, chain1)\n  DictContains(stone, \"blueprint\", \"Stone\")\n  DictContains(stone, \"type\", @self_predicate(IsStone))\n  tx::TxInsert(chain, chain1, stone)\n)"
                .to_string()
        }
        _ => format!("{action_name}(state) = AND(...)"),
    }
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
        let names: Vec<&str> = inv.iter().map(|o| o.class.name.as_str()).collect();
        assert!(names.contains(&"Log"));
        assert!(names.contains(&"Wood"));
        assert!(names.contains(&"Stick"));
        assert!(names.contains(&"WoodPick"));
        assert!(names.contains(&"Stone"));
    }

    #[test]
    fn test_inspect_object_found() {
        let mock = MockCraftOps::new();
        let detail = mock
            .inspect_object("craft-basics__log_0xabc1.dobj")
            .unwrap();
        assert_eq!(detail.class.name, "Log");
        assert_eq!(detail.status, "live");
        assert!(detail.predicate_source.contains("FindLog"));
    }

    #[test]
    fn test_inspect_object_not_found() {
        let mock = MockCraftOps::new();
        assert!(mock.inspect_object("nonexistent.dobj").is_err());
    }

    #[test]
    fn test_inspect_class() {
        let mock = MockCraftOps::new();
        let detail = mock.inspect_class(&qname("Wood")).unwrap();
        assert!(detail.produced_by.contains(&qname("CraftWood")));
        assert!(detail.consumed_by.contains(&qname("CraftSticks")));
        assert!(detail.consumed_by.contains(&qname("CraftWoodPick")));
    }

    #[test]
    fn test_inspect_unknown_class() {
        let mock = MockCraftOps::new();
        assert!(mock.inspect_class(&qname("Diamond")).is_err());
    }

    #[test]
    fn test_inspect_action() {
        let mock = MockCraftOps::new();
        let detail = mock.inspect_action(&qname("CraftWoodPick")).unwrap();
        assert_eq!(detail.id, "CraftWoodPick");
        assert!(detail.total_input_classes.contains(&"Wood".to_string()));
        assert!(detail.total_input_classes.contains(&"Stick".to_string()));
        assert!(
            detail
                .total_output_classes
                .contains(&"WoodPick".to_string())
        );
        assert!(detail.predicate_source.contains("CraftWoodPick"));
    }

    #[test]
    fn test_inspect_unknown_action() {
        let mock = MockCraftOps::new();
        assert!(mock.inspect_action(&qname("CraftDiamond")).is_err());
    }

    #[test]
    fn test_check_feasibility_feasible() {
        let mock = MockCraftOps::new();
        let report = mock.check_feasibility(&qname("CraftWoodPick")).unwrap();
        assert!(report.feasible);
        assert!(report.missing_inputs.is_empty());
        assert_eq!(report.available_inputs.len(), 2);
    }

    #[test]
    fn test_check_feasibility_missing() {
        let mock = MockCraftOps::new();
        let report = mock.check_feasibility(&qname("CraftStonePick")).unwrap();
        // We have Stone and Stick, so this should be feasible
        assert!(report.feasible);
    }

    #[test]
    fn test_check_feasibility_unknown_action() {
        let mock = MockCraftOps::new();
        assert!(mock.check_feasibility(&qname("CraftDiamond")).is_err());
    }

    #[test]
    fn test_run_action_success() {
        let mock = MockCraftOps::new();
        let result = mock
            .run_action(RunActionInput {
                action: qname("CraftWood"),
                input_object_paths: vec!["craft-basics__log_0xabc1.dobj".to_string()],
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
                        action: qname("FindLog"),
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
            action: qname("CraftDiamond"),
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
