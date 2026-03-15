use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::ops::CraftOps;
use crate::types::*;

/// MCP server service that exposes zk-craft operations as tools.
#[derive(Clone)]
pub struct CraftMcpService<T: CraftOps> {
    ops: Arc<T>,
    tool_router: ToolRouter<Self>,
}

impl<T: CraftOps> CraftMcpService<T> {
    pub fn new(ops: Arc<T>) -> Self {
        Self {
            ops,
            tool_router: Self::tool_router(),
        }
    }
}

// -- Tool parameter types --

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectObjectParams {
    /// The object ID (hex hash) to inspect
    pub object_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectClassParams {
    /// The class name to inspect, e.g. "WoodPick"
    pub class_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckFeasibilityParams {
    /// The action ID to check, e.g. "CraftWoodPick"
    pub action_id: String,
}

// -- Tool implementations --

#[tool_router]
impl<T: CraftOps> CraftMcpService<T> {
    #[tool(
        description = "List all objects in the inventory with their types, fields, and liveness status"
    )]
    fn list_inventory(&self) -> String {
        match self.ops.list_inventory() {
            Ok(items) => serde_json::to_string_pretty(&items)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        description = "List all available crafting actions with input/output class requirements and CPU cost"
    )]
    fn list_actions(&self) -> String {
        match self.ops.list_actions() {
            Ok(actions) => serde_json::to_string_pretty(&actions)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(description = "Get the current global state root hash from the synchronizer")]
    fn get_state_root(&self) -> String {
        match self.ops.get_state_root() {
            Ok(root) => serde_json::to_string_pretty(&StateRootResponse { state_root: root })
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        description = "Inspect an object by ID: full detail including fields, class, liveness, and predicate source"
    )]
    fn inspect_object(&self, Parameters(params): Parameters<InspectObjectParams>) -> String {
        match self.ops.inspect_object(&params.object_id) {
            Ok(detail) => serde_json::to_string_pretty(&detail)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        description = "Inspect a class by name: predicate definition, and which actions produce/consume it"
    )]
    fn inspect_class(&self, Parameters(params): Parameters<InspectClassParams>) -> String {
        match self.ops.inspect_class(&params.class_name) {
            Ok(detail) => serde_json::to_string_pretty(&detail)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        description = "Execute a crafting action. Blocks until proof generation completes. Returns error if another action is already in progress."
    )]
    fn run_action(&self, Parameters(params): Parameters<RunActionInput>) -> String {
        match self.ops.run_action(params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        description = "Check whether an action can be executed with the current inventory. Reports available and missing inputs."
    )]
    fn check_feasibility(&self, Parameters(params): Parameters<CheckFeasibilityParams>) -> String {
        match self.ops.check_feasibility(&params.action_id) {
            Ok(report) => serde_json::to_string_pretty(&report)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }
}

// -- ServerHandler --

#[tool_handler]
impl<T: CraftOps> ServerHandler for CraftMcpService<T> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "ZK-Craft MCP server. Provides tools to inspect inventory, \
             explore crafting actions, and execute ZK proof-based crafting operations.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockCraftOps;

    fn make_service() -> CraftMcpService<MockCraftOps> {
        CraftMcpService::new(Arc::new(MockCraftOps::new()))
    }

    #[test]
    fn test_tool_router_lists_all_seven_tools() {
        let service = make_service();
        let tools: Vec<String> = service
            .tool_router
            .map
            .keys()
            .map(|k| k.to_string())
            .collect();
        assert!(tools.contains(&"list_inventory".to_string()));
        assert!(tools.contains(&"list_actions".to_string()));
        assert!(tools.contains(&"get_state_root".to_string()));
        assert!(tools.contains(&"inspect_object".to_string()));
        assert!(tools.contains(&"inspect_class".to_string()));
        assert!(tools.contains(&"run_action".to_string()));
        assert!(tools.contains(&"check_feasibility".to_string()));
        assert_eq!(tools.len(), 7);
    }

    #[test]
    fn test_get_info_has_tools_capability() {
        let service = make_service();
        let info = service.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
        assert!(info.instructions.unwrap().contains("ZK-Craft MCP server"));
    }

    #[test]
    fn test_list_inventory_returns_valid_json() {
        let service = make_service();
        let result = service.list_inventory();
        let parsed: Vec<InventoryObject> = serde_json::from_str(&result).unwrap();
        assert!(!parsed.is_empty());
        assert!(parsed.iter().any(|o| o.class_name == "Log"));
    }

    #[test]
    fn test_list_actions_returns_valid_json() {
        let service = make_service();
        let result = service.list_actions();
        let parsed: Vec<Action> = serde_json::from_str(&result).unwrap();
        assert!(!parsed.is_empty());
        assert!(parsed.iter().any(|a| a.id == "CraftWoodPick"));
    }

    #[test]
    fn test_get_state_root_returns_valid_json() {
        let service = make_service();
        let result = service.get_state_root();
        let parsed: StateRootResponse = serde_json::from_str(&result).unwrap();
        assert!(parsed.state_root.starts_with("0x"));
    }

    #[test]
    fn test_inspect_object_via_handler() {
        let service = make_service();
        let result = service.inspect_object(Parameters(InspectObjectParams {
            object_id: "0xabc4444444444444".to_string(),
        }));
        let parsed: ObjectDetail = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.class_name, "WoodPick");
        assert!(parsed.predicate_source.contains("CraftWoodPick"));
    }

    #[test]
    fn test_inspect_object_not_found_returns_error() {
        let service = make_service();
        let result = service.inspect_object(Parameters(InspectObjectParams {
            object_id: "0xnonexistent".to_string(),
        }));
        assert!(result.starts_with("error:"));
    }

    #[test]
    fn test_inspect_class_via_handler() {
        let service = make_service();
        let result = service.inspect_class(Parameters(InspectClassParams {
            class_name: "Stick".to_string(),
        }));
        let parsed: ClassDetail = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.class_name, "Stick");
        assert!(parsed.produced_by.contains(&"CraftSticks".to_string()));
    }

    #[test]
    fn test_run_action_via_handler() {
        let service = make_service();
        let result = service.run_action(Parameters(RunActionInput {
            action_id: "FindLog".to_string(),
            input_object_paths: vec![],
        }));
        let parsed: RunActionResult = serde_json::from_str(&result).unwrap();
        assert!(parsed.success);
    }

    #[test]
    fn test_run_action_in_progress_returns_error() {
        let service = CraftMcpService::new(Arc::new(MockCraftOps::new().with_action_in_progress()));
        let result = service.run_action(Parameters(RunActionInput {
            action_id: "FindLog".to_string(),
            input_object_paths: vec![],
        }));
        assert!(result.starts_with("error:"));
        assert!(result.contains("already in progress"));
    }

    #[test]
    fn test_check_feasibility_via_handler() {
        let service = make_service();
        let result = service.check_feasibility(Parameters(CheckFeasibilityParams {
            action_id: "CraftWoodPick".to_string(),
        }));
        let parsed: FeasibilityReport = serde_json::from_str(&result).unwrap();
        assert!(parsed.feasible);
        assert_eq!(parsed.available_inputs.len(), 2);
    }

    #[test]
    fn test_check_feasibility_infeasible() {
        // Use an empty inventory so nothing is available
        let mock = MockCraftOps::new().with_inventory(vec![]);
        let service = CraftMcpService::new(Arc::new(mock));
        let result = service.check_feasibility(Parameters(CheckFeasibilityParams {
            action_id: "CraftWoodPick".to_string(),
        }));
        let parsed: FeasibilityReport = serde_json::from_str(&result).unwrap();
        assert!(!parsed.feasible);
        assert!(!parsed.missing_inputs.is_empty());
    }

    #[tokio::test]
    async fn test_server_starts_and_binds() {
        use crate::McpConfig;
        use crate::McpServer;
        use tokio_util::sync::CancellationToken;

        let ct = CancellationToken::new();
        let config = McpConfig {
            cancellation_token: ct.clone(),
        };
        let server = McpServer::new(MockCraftOps::new(), config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            server.serve(listener).await.unwrap();
        });

        // Verify the server is listening by connecting
        let stream = tokio::net::TcpStream::connect(addr).await;
        assert!(stream.is_ok());

        ct.cancel();
        // Give the server time to shut down
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    }
}
