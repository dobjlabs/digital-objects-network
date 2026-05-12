use std::path::{Path, PathBuf};
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
        description = "List all installed bitcraft commands (skills in `~/.claude/skills/bitcraft-*`) with their descriptions. The list is computed fresh on every call, so newly-created commands appear without restarting the agent. Used to validate command names the user types. For rendering `bitcraft help`, call `get_help_block` instead — it returns the exact pre-formatted text to echo."
    )]
    fn list_commands(&self) -> Json<CommandList> {
        let commands = default_skills_dir()
            .map(|d| enumerate_commands_in(&d))
            .unwrap_or_default()
            .into_iter()
            .map(|(name, description)| Command { name, description })
            .collect();
        Json(CommandList { commands })
    }

    #[tool(
        description = "Return the pre-rendered help block as a single plain-text string. This is the ENTIRE output the agent should print on a `bitcraft help` request — echo the returned string verbatim, with NO additional formatting, NO markdown, NO table, NO prefix or suffix lines. The string is computed fresh on every call from the installed commands in `~/.claude/skills/bitcraft-*`."
    )]
    fn get_help_block(&self) -> String {
        let commands = default_skills_dir()
            .map(|d| enumerate_commands_in(&d))
            .unwrap_or_default();
        if commands.is_empty() {
            return "Commands:\n  (no bitcraft commands installed — type create-command to define one)".to_string();
        }
        let width = commands.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        let mut out = String::from("Commands:\n");
        for (name, desc) in &commands {
            out.push_str(&format!("  {:<width$}  {}\n", name, desc, width = width));
        }
        out.trim_end().to_string()
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

// -- Instructions + command enumeration --

const INSTRUCTIONS: &str = include_str!("../docs/instructions.md");
const SKILL_PREFIX: &str = "bitcraft-";

/// Default location to scan for installed bitcraft commands.
fn default_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("skills"))
}

/// Scan a skills directory for `bitcraft-*` SKILL.md files and parse
/// `(display_name, description)` pairs from each frontmatter. The
/// `bitcraft-` prefix is stripped from the display name. Sorted
/// alphabetically. Returns an empty Vec if the directory is missing.
fn enumerate_commands_in(skills_dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(SKILL_PREFIX)
        })
        .filter_map(|e| parse_skill_meta(&e.path().join("SKILL.md")))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Parse `name` and `description` from a SKILL.md frontmatter. The
/// frontmatter is a YAML block delimited by `---` lines at the top of
/// the file. Returns None if missing/malformed. Strips the `bitcraft-`
/// prefix from `name` to produce the display name.
fn parse_skill_meta(skill_md: &Path) -> Option<(String, String)> {
    let contents = std::fs::read_to_string(skill_md).ok()?;
    let mut lines = contents.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = Some(rest.trim().to_string());
        }
    }
    let display = name?.strip_prefix(SKILL_PREFIX)?.to_string();
    Some((display, description?))
}

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
        .with_instructions(INSTRUCTIONS.to_string())
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
        assert!(tools.contains(&"list_commands".to_string()));
        assert!(tools.contains(&"get_help_block".to_string()));
        assert_eq!(tools.len(), 15);
    }

    #[test]
    fn test_get_info_has_tools_capability() {
        let service = make_service();
        let info = service.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
        let instructions = info.instructions.unwrap();
        assert!(instructions.contains("> bitcraft"));
        assert!(instructions.contains("Three input cases"));
        assert!(instructions.contains("no such bitcraft command"));
        assert!(instructions.contains("list_commands"));
    }

    #[test]
    fn test_enumerate_commands_filters_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path();
        write_skill(skills, "bitcraft-foo", "Bitcraft foo command.");
        write_skill(skills, "bitcraft-bar", "Bitcraft bar command.");
        write_skill(skills, "other-thing", "Not a bitcraft skill.");

        let commands = enumerate_commands_in(skills);

        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].0, "bar");
        assert_eq!(commands[0].1, "Bitcraft bar command.");
        assert_eq!(commands[1].0, "foo");
        assert_eq!(commands[1].1, "Bitcraft foo command.");
    }

    #[test]
    fn test_enumerate_commands_missing_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(enumerate_commands_in(&missing).is_empty());
    }

    #[test]
    fn test_parse_skill_meta_extracts_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bitcraft-test");
        std::fs::create_dir(&dir).unwrap();
        let skill_md = dir.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: bitcraft-test\ndescription: A test command.\n---\n\n# test\n\nbody\n",
        )
        .unwrap();

        let (display, desc) = parse_skill_meta(&skill_md).unwrap();
        assert_eq!(display, "test");
        assert_eq!(desc, "A test command.");
    }

    #[test]
    fn test_parse_skill_meta_rejects_missing_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        std::fs::write(&skill_md, "no frontmatter here\n").unwrap();
        assert!(parse_skill_meta(&skill_md).is_none());
    }

    fn write_skill(skills_dir: &Path, name: &str, description: &str) {
        let dir = skills_dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n"),
        )
        .unwrap();
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
