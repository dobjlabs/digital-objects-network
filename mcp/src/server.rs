use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    ListResourcesResult, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
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
    /// The qualified class id to inspect, e.g. "craft-basics:WoodPick"
    pub class_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckFeasibilityParams {
    /// The qualified action id to check, e.g. "craft-basics:CraftWoodPick"
    pub action_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadDocParams {
    /// Document name. Use "list" to see available documents. Available: "podlang-reference", "object-lifecycle", "txlib.podlang", "time.podlang"
    pub name: String,
}

// -- Tool implementations --

#[tool_router]
impl<T: CraftOps> CraftMcpService<T> {
    #[tool(
        description = "List all objects in the inventory with their types, fields, and liveness status"
    )]
    fn list_inventory(&self) -> Result<Json<InventoryList>, String> {
        self.ops
            .list_inventory()
            .map(|objects| Json(InventoryList { objects }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "List all available crafting actions with input/output class requirements and CPU cost"
    )]
    fn list_actions(&self) -> Result<Json<ActionList>, String> {
        self.ops
            .list_actions()
            .map(|actions| Json(ActionList { actions }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "List all known object classes with live inventory counts and which actions produce/consume each class"
    )]
    fn list_classes(&self) -> Result<Json<ClassList>, String> {
        self.ops
            .list_classes()
            .map(|classes| Json(ClassList { classes }))
            .map_err(|e| e.to_string())
    }

    #[tool(description = "Get the current global state root hash from the synchronizer")]
    fn get_state_root(&self) -> Result<Json<StateRootResponse>, String> {
        self.ops
            .get_state_root()
            .map(|root| Json(StateRootResponse { state_root: root }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Inspect an object by ID: full detail including fields, class, liveness, and predicate source"
    )]
    fn inspect_object(
        &self,
        Parameters(params): Parameters<InspectObjectParams>,
    ) -> Result<Json<ObjectDetail>, String> {
        self.ops
            .inspect_object(&params.object_id)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Inspect a class by name: predicate definition, and which actions produce/consume it"
    )]
    fn inspect_class(
        &self,
        Parameters(params): Parameters<InspectClassParams>,
    ) -> Result<Json<ClassDetail>, String> {
        self.ops
            .inspect_class(&params.class_id)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Execute a crafting action. Blocks until proof generation completes. Returns error if another action is already in progress."
    )]
    async fn run_action(
        &self,
        Parameters(params): Parameters<RunActionInput>,
    ) -> Result<Json<RunActionResult>, String> {
        // Run on the blocking thread pool so we don't starve the async runtime
        // while proof generation, relayer submission, and sync polling happen.
        let ops = self.ops.clone();
        tokio::task::spawn_blocking(move || ops.run_action(params))
            .await
            .map_err(|e| format!("action task failed: {e}"))?
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Check whether an action can be executed with the current inventory. Reports available and missing inputs."
    )]
    fn check_feasibility(
        &self,
        Parameters(params): Parameters<CheckFeasibilityParams>,
    ) -> Result<Json<FeasibilityReport>, String> {
        self.ops
            .check_feasibility(&params.action_id)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read reference documentation. Available docs: \"podlang-reference\" (full podlang language reference), \"object-lifecycle\" (how Digital Objects are created, mutated, consumed), \"txlib.podlang\" (core transaction predicates source), \"time.podlang\" (time/locking predicates source), \"generated.podlang\" (generated podlang for all actions and classes in this game). Pass \"list\" to see all available documents."
    )]
    fn read_doc(&self, Parameters(params): Parameters<ReadDocParams>) -> String {
        match params.name.as_str() {
            "list" => {
                let docs = crate::resources::list();
                let mut lines: Vec<String> = docs
                    .iter()
                    .map(|r| {
                        format!(
                            "- {} ({})\n  {}",
                            r.name,
                            r.uri,
                            r.description.as_deref().unwrap_or("")
                        )
                    })
                    .collect();
                lines.push(
                    "- generated.podlang\n  Generated podlang source for all actions and IsClassName predicates in this game instance."
                        .to_string(),
                );
                lines.join("\n")
            }
            _ => {
                let uri = match params.name.as_str() {
                    "podlang-reference" => "zk-craft://docs/podlang-reference",
                    "object-lifecycle" => "zk-craft://docs/object-lifecycle",
                    "txlib.podlang" => "zk-craft://source/txlib.podlang",
                    "time.podlang" => "zk-craft://source/time.podlang",
                    "generated.podlang" => {
                        return self
                            .ops
                            .generated_podlang()
                            .unwrap_or_else(|| "(not available in mock mode)".to_string());
                    }
                    other => {
                        return format!(
                            "Unknown document: \"{other}\". Use read_doc(\"list\") to see available documents."
                        );
                    }
                };
                crate::resources::read(uri)
                    .map(|r| {
                        r.contents
                            .into_iter()
                            .next()
                            .map(|c| match c {
                                rmcp::model::ResourceContents::TextResourceContents {
                                    text,
                                    ..
                                } => text,
                                rmcp::model::ResourceContents::BlobResourceContents {
                                    blob,
                                    ..
                                } => blob,
                            })
                            .unwrap_or_default()
                    })
                    .unwrap_or_else(|| "Resource not found".to_string())
            }
        }
    }
}

// -- Instructions --

const INSTRUCTIONS: &str = include_str!("../docs/instructions.md");

// -- ServerHandler --

#[tool_handler]
impl<T: CraftOps> ServerHandler for CraftMcpService<T> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(INSTRUCTIONS)
    }

    fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListResourcesResult {
            meta: None,
            resources: crate::resources::list(),
            next_cursor: None,
        }))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        std::future::ready(crate::resources::read(&request.uri).ok_or_else(|| {
            McpError::resource_not_found(format!("unknown resource: {}", request.uri), None)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockCraftOps;
    use rmcp::handler::server::wrapper::Json;

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
        assert!(tools.contains(&"list_classes".to_string()));
        assert!(tools.contains(&"read_doc".to_string()));
        assert_eq!(tools.len(), 9);
    }

    #[test]
    fn test_get_info_has_tools_capability() {
        let service = make_service();
        let info = service.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
        assert!(info.instructions.unwrap().contains("ZK-Craft MCP Server"));
    }

    #[test]
    fn test_list_inventory_returns_structured() {
        let service = make_service();
        let Json(list) = service.list_inventory().unwrap();
        assert!(!list.objects.is_empty());
        assert!(list.objects.iter().any(|o| o.class_display_name == "Log"));
    }

    #[test]
    fn test_list_actions_returns_structured() {
        let service = make_service();
        let Json(list) = service.list_actions().unwrap();
        assert!(!list.actions.is_empty());
        assert!(
            list.actions
                .iter()
                .any(|a| a.id == "craft-basics:CraftWoodPick")
        );
    }

    #[test]
    fn test_get_state_root_returns_structured() {
        let service = make_service();
        let Json(resp) = service.get_state_root().unwrap();
        assert!(resp.state_root.starts_with("0x"));
    }

    #[test]
    fn test_inspect_object_via_handler() {
        let service = make_service();
        let Json(detail) = service
            .inspect_object(Parameters(InspectObjectParams {
                object_id: "0xabc4444444444444".to_string(),
            }))
            .unwrap();
        assert_eq!(detail.class_display_name, "WoodPick");
        assert!(detail.predicate_source.contains("CraftWoodPick"));
    }

    #[test]
    fn test_inspect_object_not_found_returns_error() {
        let service = make_service();
        let result = service.inspect_object(Parameters(InspectObjectParams {
            object_id: "0xnonexistent".to_string(),
        }));
        let err = result.err().expect("should be an error");
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_inspect_class_via_handler() {
        let service = make_service();
        let Json(detail) = service
            .inspect_class(Parameters(InspectClassParams {
                class_id: "craft-basics:Stick".to_string(),
            }))
            .unwrap();
        assert_eq!(detail.class_display_name, "Stick");
        assert!(
            detail
                .produced_by
                .contains(&"craft-basics:CraftSticks".to_string())
        );
    }

    #[tokio::test]
    async fn test_run_action_via_handler() {
        let service = make_service();
        let Json(result) = service
            .run_action(Parameters(RunActionInput {
                action_id: "craft-basics:FindLog".to_string(),
                input_object_paths: vec![],
            }))
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_action_in_progress_returns_error() {
        let service = CraftMcpService::new(Arc::new(MockCraftOps::new().with_action_in_progress()));
        let result = service
            .run_action(Parameters(RunActionInput {
                action_id: "craft-basics:FindLog".to_string(),
                input_object_paths: vec![],
            }))
            .await;
        let err = result.err().expect("should be an error");
        assert!(err.contains("already in progress"));
    }

    #[test]
    fn test_check_feasibility_via_handler() {
        let service = make_service();
        let Json(report) = service
            .check_feasibility(Parameters(CheckFeasibilityParams {
                action_id: "craft-basics:CraftWoodPick".to_string(),
            }))
            .unwrap();
        assert!(report.feasible);
        assert_eq!(report.available_inputs.len(), 2);
    }

    #[test]
    fn test_check_feasibility_infeasible() {
        let mock = MockCraftOps::new().with_inventory(vec![]);
        let service = CraftMcpService::new(Arc::new(mock));
        let Json(report) = service
            .check_feasibility(Parameters(CheckFeasibilityParams {
                action_id: "craft-basics:CraftWoodPick".to_string(),
            }))
            .unwrap();
        assert!(!report.feasible);
        assert!(!report.missing_input_class_ids.is_empty());
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
