use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, ListPromptsResult, ListResourcesResult,
    PaginatedRequestParams, Prompt, PromptArgument, PromptMessage, PromptMessageContent,
    PromptMessageRole, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::ops::CraftOps;
use crate::types::*;

/// MCP server service that exposes bitcraft operations as tools.
#[derive(Clone)]
pub struct CraftMcpService<T: CraftOps> {
    ops: Arc<T>,
    /// Used by the `#[tool_handler]` macro at request-dispatch time and by
    /// the test below. Plain dead-code analysis can't see the macro
    /// expansion, so silence the warning.
    #[allow(dead_code)]
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
    /// The `.dobj` file name (e.g. `craft-basics__wood_0xabc….dobj`) to
    /// inspect. Must be a basename in `~/.dobj/objects/` — use
    /// `list_inventory` to see what's available.
    pub file_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectClassParams {
    /// The plugin-scoped class to inspect, e.g.
    /// `{ "pluginName": "craft-basics", "name": "WoodPick" }`.
    pub class: QualifiedName,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectActionParams {
    /// The plugin-scoped action to inspect, e.g.
    /// `{ "pluginName": "craft-basics", "name": "CraftWoodPick" }`.
    pub action: QualifiedName,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckFeasibilityParams {
    /// The plugin-scoped action to check, e.g.
    /// `{ "pluginName": "craft-basics", "name": "CraftWoodPick" }`.
    pub action: QualifiedName,
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
        description = "Inspect an object by file name: full detail including fields, class, liveness, and predicate source. The file_name is a `.dobj` basename from list_inventory (e.g. `craft-basics__wood_0xabc….dobj`)."
    )]
    fn inspect_object(
        &self,
        Parameters(params): Parameters<InspectObjectParams>,
    ) -> Result<Json<ObjectDetail>, String> {
        self.ops
            .inspect_object(&params.file_name)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Inspect a class by plugin-scoped name: predicate definition, and which actions produce/consume it"
    )]
    fn inspect_class(
        &self,
        Parameters(params): Parameters<InspectClassParams>,
    ) -> Result<Json<ClassDetail>, String> {
        self.ops
            .inspect_class(&params.class)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Inspect an action by plugin-scoped name: predicate definition, description, and input/output class requirements"
    )]
    fn inspect_action(
        &self,
        Parameters(params): Parameters<InspectActionParams>,
    ) -> Result<Json<ActionDetail>, String> {
        self.ops
            .inspect_action(&params.action)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Execute a crafting action. Blocks until proof generation completes. Multiple actions can run concurrently."
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
            .check_feasibility(&params.action)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(description = "Read the driver's current configuration: synchronizer + relayer URLs.")]
    fn read_settings(&self) -> Result<Json<DriverSettings>, String> {
        self.ops
            .read_settings()
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Update the driver's configuration. Both synchronizer and relayer URLs are required — pass the current value for whichever you don't want to change. Most callers will read_settings first, mutate, then write_settings."
    )]
    fn write_settings(
        &self,
        Parameters(params): Parameters<DriverSettings>,
    ) -> Result<Json<DriverSettings>, String> {
        self.ops
            .write_settings(params)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Filesystem path to the local objects directory (~/.dobj/objects/). Returned as a string the user can paste into a file manager."
    )]
    fn get_objects_dir(&self) -> Result<Json<ObjectsDirInfo>, String> {
        self.ops
            .get_objects_dir()
            .map(|path| Json(ObjectsDirInfo { path }))
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
                    "podlang-reference" => "bitcraft://docs/podlang-reference",
                    "object-lifecycle" => "bitcraft://docs/object-lifecycle",
                    "txlib.podlang" => "bitcraft://source/txlib.podlang",
                    "time.podlang" => "bitcraft://source/time.podlang",
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

// -- Sample prompts --
//
// Demonstrates the MCP `prompts` primitive. Clients (Claude Code, Cursor, …)
// surface these as slash commands, e.g. `/bitcraft:welcome`. `get_prompt`
// returns the message(s) that get injected into the chat when the user picks
// one — argument values are substituted server-side.

fn sample_prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(
            "welcome",
            Some("Onboard a new bitcraft player — survey inventory and suggest a next move."),
            None,
        ),
        Prompt::new(
            "craft-plan",
            Some("Plan a sequence of bitcraft commands to obtain a target item."),
            Some(vec![
                PromptArgument::new("target")
                    .with_description("Class name to craft, e.g. WoodPick, StonePick, Wood")
                    .with_required(true),
            ]),
        ),
    ]
}

fn render_prompt(request: &GetPromptRequestParams) -> Result<GetPromptResult, McpError> {
    let arg = |name: &str| -> Option<String> {
        request
            .arguments
            .as_ref()
            .and_then(|args| args.get(name))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    match request.name.as_str() {
        "welcome" => Ok(GetPromptResult::new(vec![PromptMessage::new(
            PromptMessageRole::User,
            PromptMessageContent::text(
                "I'm starting a new bitcraft session. Call `list_inventory` and `list_actions`, \
                 then suggest one concrete next step in a single line.",
            ),
        )])),
        "craft-plan" => {
            let target = arg("target").ok_or_else(|| {
                McpError::invalid_params("missing required argument: target", None)
            })?;
            Ok(GetPromptResult::new(vec![PromptMessage::new(
                PromptMessageRole::User,
                PromptMessageContent::text(format!(
                    "Plan how to obtain a {target}. List the ordered bitcraft commands to invoke \
                     (chop-log, craft-wood, craft-sticks, craft-wood-pick, mine-stone, \
                     craft-stone-pick). Output one command per line, nothing else."
                )),
            )]))
        }
        other => Err(McpError::invalid_params(
            format!("unknown prompt: {other}"),
            None,
        )),
    }
}

// -- ServerHandler --

#[tool_handler]
impl<T: CraftOps> ServerHandler for CraftMcpService<T> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_instructions(INSTRUCTIONS.to_string())
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
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

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListPromptsResult {
            meta: None,
            prompts: sample_prompts(),
            next_cursor: None,
        }))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        std::future::ready(render_prompt(&request))
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

    fn craft_basics(name: &str) -> QualifiedName {
        QualifiedName {
            plugin_name: "craft-basics".to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn test_tool_router_lists_all_tools() {
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
        assert!(tools.contains(&"inspect_action".to_string()));
        assert!(tools.contains(&"run_action".to_string()));
        assert!(tools.contains(&"check_feasibility".to_string()));
        assert!(tools.contains(&"list_classes".to_string()));
        assert!(tools.contains(&"read_doc".to_string()));
        assert!(tools.contains(&"read_settings".to_string()));
        assert!(tools.contains(&"write_settings".to_string()));
        assert!(tools.contains(&"get_objects_dir".to_string()));
        assert_eq!(tools.len(), 13);
    }

    #[test]
    fn test_get_info_has_tools_capability() {
        let service = make_service();
        let info = service.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.prompts.is_some());
        assert!(info.instructions.is_some());
        let instructions = info.instructions.unwrap();
        assert!(instructions.contains("> bitcraft"));
        assert!(instructions.contains("Two input cases"));
        assert!(instructions.contains("no such bitcraft command"));
    }

    #[test]
    fn test_sample_prompts_listed() {
        let prompts = sample_prompts();
        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"welcome"));
        assert!(names.contains(&"craft-plan"));
        let craft = prompts.iter().find(|p| p.name == "craft-plan").unwrap();
        let args = craft.arguments.as_ref().expect("craft-plan has arguments");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "target");
        assert_eq!(args[0].required, Some(true));
    }

    #[test]
    fn test_render_prompt_craft_plan_substitutes_target() {
        let mut args = serde_json::Map::new();
        args.insert("target".to_string(), serde_json::json!("StonePick"));
        let req = GetPromptRequestParams::new("craft-plan").with_arguments(args);
        let result = render_prompt(&req).unwrap();
        assert_eq!(result.messages.len(), 1);
        let PromptMessageContent::Text { text } = &result.messages[0].content else {
            panic!("expected text content");
        };
        assert!(text.contains("StonePick"));
    }

    #[test]
    fn test_render_prompt_craft_plan_missing_arg_errors() {
        let req = GetPromptRequestParams::new("craft-plan");
        assert!(render_prompt(&req).is_err());
    }

    #[test]
    fn test_render_prompt_unknown_errors() {
        let req = GetPromptRequestParams::new("no-such-prompt");
        assert!(render_prompt(&req).is_err());
    }

    #[test]
    fn test_list_inventory_returns_structured() {
        let service = make_service();
        let Json(list) = service.list_inventory().unwrap();
        assert!(!list.objects.is_empty());
        assert!(list.objects.iter().any(|o| o.class.name == "Log"));
    }

    #[test]
    fn test_list_actions_returns_structured() {
        let service = make_service();
        let Json(list) = service.list_actions().unwrap();
        assert!(!list.actions.is_empty());
        assert!(
            list.actions
                .iter()
                .any(|a| a.action == craft_basics("CraftWoodPick"))
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
                file_name: "craft-basics__woodpick_0xabc4.dobj".to_string(),
            }))
            .unwrap();
        assert_eq!(detail.class.name, "WoodPick");
    }

    #[test]
    fn test_inspect_object_not_found_returns_error() {
        let service = make_service();
        let result = service.inspect_object(Parameters(InspectObjectParams {
            file_name: "nonexistent.dobj".to_string(),
        }));
        let err = result.err().expect("should be an error");
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_inspect_class_via_handler() {
        let service = make_service();
        let Json(detail) = service
            .inspect_class(Parameters(InspectClassParams {
                class: craft_basics("Stick"),
            }))
            .unwrap();
        assert_eq!(detail.class.name, "Stick");
        assert!(detail.produced_by.contains(&craft_basics("CraftSticks")));
    }

    #[test]
    fn test_inspect_action_via_handler() {
        let service = make_service();
        let Json(detail) = service
            .inspect_action(Parameters(InspectActionParams {
                action: craft_basics("CraftWoodPick"),
            }))
            .unwrap();
        assert_eq!(detail.action.name, "CraftWoodPick");
        assert!(detail.total_inputs.iter().any(|r| r.class.name == "Wood"));
        assert!(detail.total_inputs.iter().any(|r| r.class.name == "Stick"));
        assert!(detail.predicate_source.contains("CraftWoodPick"));
    }

    #[test]
    fn test_inspect_action_unknown_returns_error() {
        let service = make_service();
        let result = service.inspect_action(Parameters(InspectActionParams {
            action: craft_basics("CraftDiamond"),
        }));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_action_via_handler() {
        let service = make_service();
        let Json(result) = service
            .run_action(Parameters(RunActionInput {
                action: craft_basics("FindLog"),
                input_object_paths: vec![],
                run_id: None,
            }))
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_action_concurrent() {
        let service = Arc::new(make_service());
        let mut handles = Vec::new();
        for _ in 0..3 {
            let svc = service.clone();
            handles.push(tokio::spawn(async move {
                svc.run_action(Parameters(RunActionInput {
                    action: craft_basics("FindLog"),
                    input_object_paths: vec![],
                    run_id: None,
                }))
                .await
            }));
        }
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
            assert!(result.unwrap().0.success);
        }
    }

    #[test]
    fn test_check_feasibility_via_handler() {
        let service = make_service();
        let Json(report) = service
            .check_feasibility(Parameters(CheckFeasibilityParams {
                action: craft_basics("CraftWoodPick"),
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
                action: craft_basics("CraftWoodPick"),
            }))
            .unwrap();
        assert!(!report.feasible);
        assert!(!report.missing_inputs.is_empty());
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
