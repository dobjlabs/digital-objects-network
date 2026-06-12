use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, ListPromptsResult, ListResourcesResult,
    ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{CommandList, CommandStamp, UserCommand};
use crate::ops::DobjOps;
use crate::types::*;

/// Memoized [`CommandStore::list`](crate::commands::CommandStore::list) output,
/// reused while the store's fingerprint is unchanged. `stamps` is the
/// change-key it was built from.
#[derive(Default)]
struct CommandListCache {
    stamps: Vec<CommandStamp>,
    commands: Vec<UserCommand>,
}

/// MCP server service that exposes Digital Objects Network operations as tools.
#[derive(Clone)]
pub struct DobjMcpService<T: DobjOps> {
    ops: Arc<T>,
    /// Used by the `#[tool_handler]` macro at request-dispatch time and by
    /// the test below. Plain dead-code analysis can't see the macro
    /// expansion, so silence the warning.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    /// Overrides the user-command store directory; `None` derives a `commands`
    /// sibling of the driver's objects dir (under `~/.dobj`). Set in tests.
    command_dir: Option<PathBuf>,
    /// Caches the saved-command listing for the hot read paths (`list_prompts`,
    /// the `start`/`help` menus, `list_commands`), refreshed only when the store
    /// changes on disk. `Arc<Mutex<_>>` so all clones share one cache.
    command_cache: Arc<Mutex<CommandListCache>>,
}

impl<T: DobjOps> DobjMcpService<T> {
    pub fn new(ops: Arc<T>) -> Self {
        Self {
            ops,
            tool_router: Self::tool_router(),
            command_dir: None,
            command_cache: Arc::new(Mutex::new(CommandListCache::default())),
        }
    }

    /// Point the user-command store at an explicit directory (tests).
    pub fn with_command_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.command_dir = Some(dir.into());
        self
    }

    /// The user-command store. By default it lives in a `commands` directory
    /// beside the driver's objects dir, so definitions land under
    /// `~/.dobj/commands/` without the driver needing to know about them.
    fn command_store(&self) -> anyhow::Result<crate::commands::CommandStore> {
        if let Some(dir) = &self.command_dir {
            return Ok(crate::commands::CommandStore::new(dir.clone()));
        }
        let dir = crate::dobj_root(&self.ops.get_objects_dir()?).join("commands");
        Ok(crate::commands::CommandStore::new(dir))
    }

    /// Resolve a command name to its definition -- a built-in or a saved one.
    /// Shared by the `get_command` tool and the prompt dispatcher. `help` is
    /// special: its menu is rendered live from the current saved commands, so a
    /// command saved earlier this session shows up without a restart.
    fn resolve_command(&self, name: &str) -> Option<UserCommand> {
        if name == crate::prompts::HELP {
            return Some(crate::prompts::help_command(&self.list_commands_cached()));
        }
        crate::prompts::builtin_command(name)
            .or_else(|| self.command_store().ok().and_then(|store| store.get(name)))
    }

    /// The saved commands, served from an in-memory cache that refreshes only
    /// when the on-disk store changes. Recomputing the change-key is a directory
    /// scan plus a stat per command -- far cheaper than re-reading and parsing
    /// every README, which the hot paths would otherwise do on each call.
    fn list_commands_cached(&self) -> Vec<UserCommand> {
        let Ok(store) = self.command_store() else {
            return Vec::new();
        };
        let stamps = store.fingerprint();
        let mut cache = self
            .command_cache
            .lock()
            .expect("command cache mutex poisoned");
        if stamps != cache.stamps {
            cache.commands = store.list();
            cache.stamps = stamps;
        }
        cache.commands.clone()
    }
}

// -- Tool parameter types --

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectObjectParams {
    /// The `.dobj` file name (e.g. `craft-basics__wood_0xabc….dobj`) to
    /// inspect. Must be a basename in `~/.dobj/objects/` — use
    /// `list_objects` to see what's available.
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
pub struct ImportObjectFileParams {
    /// Local filesystem path (on the machine running dobjd) to an external
    /// `.dobj` file — one not produced by this driver (e.g. a download, or a
    /// file from outside `~/.dobj/`). dobjd reads the file, validates it
    /// (class identity + on-chain grounding), and files it under a canonical
    /// name derived from its commitment. If you only have the object's JSON
    /// inline, write it to a temp file first and pass that path.
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRunParams {
    /// The `runId` returned by `run_action`.
    pub run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadDocParams {
    /// Document name. Use "list" to see available documents. Available: "podlang-reference", "object-lifecycle", "how-it-works", "command-examples", "txlib.podlang"
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DefineCommandParams {
    /// Command name; slugified to lowercase-dashed (e.g. "Build Rocket" ->
    /// "build-rocket"). The names "start", "help", "create-command",
    /// "consult-docs", and "dashboard" are reserved.
    pub name: String,
    /// One-line description shown in the command menu.
    pub description: String,
    /// The steps to run when the command is invoked: plain instructions over
    /// the loaded plugin's actions (and other saved commands) that the model
    /// follows in order.
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteCommandParams {
    /// Name of the command to remove.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCommandParams {
    /// The command name to load -- a built-in or a saved command.
    pub name: String,
}

// -- Tool implementations --

#[tool_router]
impl<T: DobjOps> DobjMcpService<T> {
    #[tool(
        description = "List all objects held by this driver with their types, fields, and liveness status"
    )]
    fn list_objects(&self) -> Result<Json<ObjectList>, String> {
        self.ops
            .list_objects()
            .map(|objects| Json(ObjectList { objects }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "List all available actions with input/output class requirements and CPU cost"
    )]
    fn list_actions(&self) -> Result<Json<ActionList>, String> {
        self.ops
            .list_actions()
            .map(|actions| Json(ActionList { actions }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "List all known object classes with live object counts and which actions produce/consume each class"
    )]
    fn list_classes(&self) -> Result<Json<ClassList>, String> {
        self.ops
            .list_classes()
            .map(|classes| Json(ClassList { classes }))
            .map_err(|e| e.to_string())
    }

    #[tool(description = "Get the current state root hash from the synchronizer")]
    fn get_state_root(&self) -> Result<Json<StateRootResponse>, String> {
        self.ops
            .get_state_root()
            .map(|root| Json(StateRootResponse { state_root: root }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Inspect an object by file name: full detail including fields, class, liveness, and predicate source. The file_name is a `.dobj` basename from list_objects (e.g. `craft-basics__wood_0xabc….dobj`)."
    )]
    fn inspect_object(
        &self,
        Parameters(params): Parameters<InspectObjectParams>,
    ) -> Result<Json<ObjectSummary>, String> {
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
    ) -> Result<Json<ClassSummary>, String> {
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
    ) -> Result<Json<ActionSummary>, String> {
        self.ops
            .inspect_action(&params.action)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Start an action. Returns immediately with a runId and status=queued; proof generation and commit run in the background. Poll get_run(runId) until status is succeeded or failed (then read result / error). Multiple actions run concurrently."
    )]
    fn run_action(
        &self,
        Parameters(params): Parameters<RunActionInput>,
    ) -> Result<Json<RunAccepted>, String> {
        // Returns as soon as the run is registered + spawned; the heavy work
        // happens on a background task, so this tool call never holds the
        // connection open for the proof.
        self.ops
            .run_action(params)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Get the current state of a run started by run_action. Returns its status (queued, generateProof, committing, succeeded, failed), the result (old/new root + output and nullified files) once succeeded, an error message if failed, and the ordered progress log. Poll this after run_action."
    )]
    fn get_run(
        &self,
        Parameters(params): Parameters<GetRunParams>,
    ) -> Result<Json<RunState>, String> {
        self.ops
            .get_run(&params.run_id)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Check whether an action can be executed with the current objects. Reports available and missing inputs."
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

    #[tool(
        description = "Import an external .dobj object — one not produced by this driver — into the local object store. Pass `path`, a local filesystem path (on the machine running dobjd) to the .dobj file; dobjd reads it. If you only have the object's JSON inline, write it to a temp file first and pass that path. Validates the object's class identity and on-chain grounding, files it under a canonical name, and returns its summary (status is `live` if grounded, otherwise `unknown`). Errors if the object is already present or already spent on-chain."
    )]
    fn import_object_file(
        &self,
        Parameters(params): Parameters<ImportObjectFileParams>,
    ) -> Result<Json<ObjectSummary>, String> {
        self.ops
            .import_object_file(&params.path)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read the daemon's current configuration: synchronizer + relayer URLs and the MCP server toggle."
    )]
    fn read_settings(&self) -> Result<Json<DriverSettings>, String> {
        self.ops
            .read_settings()
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Update the daemon's configuration. Send only the fields you want to change; any field you omit keeps its current value. Setting mcpEnabled to false shuts down the MCP server you are connected through."
    )]
    fn write_settings(
        &self,
        Parameters(params): Parameters<DriverSettingsPatch>,
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
        description = "Read reference documentation. Available docs: \"podlang-reference\" (full podlang language reference), \"object-lifecycle\" (how Digital Objects are created, mutated, consumed), \"how-it-works\" (generic framing for working with Digital Objects), \"command-examples\" (worked templates for create-command bodies), \"txlib.podlang\" (core transaction predicates source), \"generated.podlang\" (generated podlang for all actions and classes). Pass \"list\" to see all available documents."
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
                    "- generated.podlang\n  Generated podlang source for all actions and IsClassName predicates."
                        .to_string(),
                );
                lines.join("\n")
            }
            _ => {
                let uri = match params.name.as_str() {
                    "podlang-reference" => "dobj://docs/podlang-reference",
                    "object-lifecycle" => "dobj://docs/object-lifecycle",
                    "how-it-works" => "dobj://docs/how-it-works",
                    "command-examples" => "dobj://docs/command-examples",
                    "txlib.podlang" => "dobj://source/txlib.podlang",
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

    #[tool(
        description = "Define a reusable command: a named macro of steps over the loaded plugin's actions. Writes ~/.dobj/commands/<name>/README.md (frontmatter + body); you may add sibling scripts in that directory and reference them by absolute path in the body. It then appears in the command menu and as its own prompt; running its name follows the steps. Slugifies the name and rejects the reserved framework command names. Re-saving rewrites the README and keeps any sibling scripts."
    )]
    fn define_command(
        &self,
        Parameters(params): Parameters<DefineCommandParams>,
    ) -> Result<Json<UserCommand>, String> {
        let store = self.command_store().map_err(|e| e.to_string())?;
        store
            .save(&params.name, &params.description, &params.body)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "List the saved commands (defined via define_command), with their descriptions and bodies."
    )]
    fn list_commands(&self) -> Result<Json<CommandList>, String> {
        Ok(Json(CommandList {
            commands: self.list_commands_cached(),
        }))
    }

    #[tool(description = "Delete a saved command by name.")]
    fn delete_command(&self, Parameters(params): Parameters<DeleteCommandParams>) -> String {
        match self.command_store() {
            Ok(store) => match store.delete(&params.name) {
                Ok(true) => format!("deleted: {}", params.name),
                Ok(false) => format!("no such command: {}", params.name),
                Err(err) => err.to_string(),
            },
            Err(err) => err.to_string(),
        }
    }

    #[tool(
        description = "Load a command's full body so you can follow it: a built-in (help, create-command, consult-docs, dashboard) or a saved command. Call this when the user types a command's name, then follow the returned body. Returns name, description, and body."
    )]
    fn get_command(
        &self,
        Parameters(params): Parameters<GetCommandParams>,
    ) -> Result<Json<UserCommand>, String> {
        self.resolve_command(&params.name)
            .map(Json)
            .ok_or_else(|| format!("no such command: {}", params.name))
    }
}

// -- Instructions --

const INSTRUCTIONS: &str = include_str!("../docs/instructions.md");

// -- ServerHandler --

#[tool_handler]
impl<T: DobjOps> ServerHandler for DobjMcpService<T> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
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

    fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        let mut prompts = crate::prompts::list();
        prompts.extend(
            self.list_commands_cached()
                .iter()
                .map(crate::prompts::command_prompt),
        );
        std::future::ready(Ok(ListPromptsResult::with_all_items(prompts)))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        let arguments = request.arguments.as_ref();
        if request.name == crate::prompts::START {
            let stored = self.list_commands_cached();
            return std::future::ready(Ok(crate::prompts::start_result(&stored, arguments)));
        }
        let result = self
            .resolve_command(&request.name)
            .map(|command| crate::prompts::command_result(&command, arguments))
            .ok_or_else(|| {
                McpError::invalid_params(format!("unknown prompt: {}", request.name), None)
            });
        std::future::ready(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockDobjOps;
    use rmcp::handler::server::wrapper::Json;

    fn make_service() -> DobjMcpService<MockDobjOps> {
        DobjMcpService::new(Arc::new(MockDobjOps::new()))
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
        assert!(tools.contains(&"list_objects".to_string()));
        assert!(tools.contains(&"list_actions".to_string()));
        assert!(tools.contains(&"get_state_root".to_string()));
        assert!(tools.contains(&"inspect_object".to_string()));
        assert!(tools.contains(&"inspect_class".to_string()));
        assert!(tools.contains(&"inspect_action".to_string()));
        assert!(tools.contains(&"run_action".to_string()));
        assert!(tools.contains(&"get_run".to_string()));
        assert!(tools.contains(&"check_feasibility".to_string()));
        assert!(tools.contains(&"list_classes".to_string()));
        assert!(tools.contains(&"read_doc".to_string()));
        assert!(tools.contains(&"read_settings".to_string()));
        assert!(tools.contains(&"write_settings".to_string()));
        assert!(tools.contains(&"get_objects_dir".to_string()));
        assert!(tools.contains(&"import_object_file".to_string()));
        assert!(tools.contains(&"define_command".to_string()));
        assert!(tools.contains(&"list_commands".to_string()));
        assert!(tools.contains(&"delete_command".to_string()));
        assert!(tools.contains(&"get_command".to_string()));
        assert_eq!(tools.len(), 19);
    }

    #[test]
    fn test_get_info_has_tools_capability() {
        let service = make_service();
        let info = service.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
        assert!(
            info.instructions
                .unwrap()
                .contains("Digital Objects Network MCP Server")
        );
    }

    #[test]
    fn test_get_info_has_prompts_capability() {
        let service = make_service();
        let info = service.get_info();
        assert!(info.capabilities.prompts.is_some());
    }

    #[test]
    fn test_define_and_list_commands() {
        let dir = tempfile::tempdir().unwrap();
        let service = make_service().with_command_dir(dir.path());
        let Json(saved) = service
            .define_command(Parameters(DefineCommandParams {
                name: "Stock Up".to_string(),
                description: "gather raw materials".to_string(),
                body: "run the mining actions a few times".to_string(),
            }))
            .unwrap();
        assert_eq!(saved.name, "stock-up");
        let Json(list) = service.list_commands().unwrap();
        assert_eq!(list.commands.len(), 1);
        assert_eq!(list.commands[0].name, "stock-up");
    }

    #[test]
    fn test_delete_command() {
        let dir = tempfile::tempdir().unwrap();
        let service = make_service().with_command_dir(dir.path());
        service
            .define_command(Parameters(DefineCommandParams {
                name: "temp".to_string(),
                description: "t".to_string(),
                body: "do a thing".to_string(),
            }))
            .unwrap();
        assert!(
            service
                .delete_command(Parameters(DeleteCommandParams {
                    name: "temp".to_string(),
                }))
                .contains("deleted")
        );
        let Json(list) = service.list_commands().unwrap();
        assert!(list.commands.is_empty());
    }

    #[test]
    fn test_define_command_rejects_reserved_name() {
        let dir = tempfile::tempdir().unwrap();
        let service = make_service().with_command_dir(dir.path());
        let result = service.define_command(Parameters(DefineCommandParams {
            name: "start".to_string(),
            description: "x".to_string(),
            body: "y".to_string(),
        }));
        assert!(result.is_err());
    }

    #[test]
    fn test_list_commands_cache_tracks_disk_changes() {
        let dir = tempfile::tempdir().unwrap();
        let service = make_service().with_command_dir(dir.path());
        let define = |name: &str, desc: &str| {
            service
                .define_command(Parameters(DefineCommandParams {
                    name: name.to_string(),
                    description: desc.to_string(),
                    body: "do the steps".to_string(),
                }))
                .unwrap();
        };

        define("alpha", "first");
        let Json(list) = service.list_commands().unwrap();
        assert_eq!(list.commands.len(), 1);
        assert_eq!(list.commands[0].description, "first");

        // Redefining rewrites the same README in place; the changed size (and
        // mtime) must invalidate the cache rather than serving the stale entry.
        define("alpha", "second revision");
        let Json(list) = service.list_commands().unwrap();
        assert_eq!(list.commands.len(), 1);
        assert_eq!(list.commands[0].description, "second revision");

        // Adding a command changes the directory set.
        define("beta", "b");
        let Json(list) = service.list_commands().unwrap();
        assert_eq!(list.commands.len(), 2);

        // Deleting is reflected too.
        assert!(
            service
                .delete_command(Parameters(DeleteCommandParams {
                    name: "alpha".to_string(),
                }))
                .contains("deleted")
        );
        let Json(list) = service.list_commands().unwrap();
        assert_eq!(list.commands.len(), 1);
        assert_eq!(list.commands[0].name, "beta");
    }

    #[test]
    fn test_list_objects_returns_structured() {
        let service = make_service();
        let Json(list) = service.list_objects().unwrap();
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

    #[test]
    fn test_run_action_via_handler() {
        let service = make_service();
        let Json(accepted) = service
            .run_action(Parameters(RunActionInput {
                action: craft_basics("FindLog"),
                input_object_paths: vec![],
            }))
            .unwrap();
        assert!(!accepted.run_id.is_empty());
    }

    #[test]
    fn test_get_run_via_handler() {
        let service = make_service();
        let Json(state) = service
            .get_run(Parameters(GetRunParams {
                run_id: "run-1".to_string(),
            }))
            .unwrap();
        assert_eq!(state.run_id, "run-1");
        assert_eq!(state.status, RunStatus::Succeeded);
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
        let mock = MockDobjOps::new().with_objects(vec![]);
        let service = DobjMcpService::new(Arc::new(mock));
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
            dobj_port: crate::DEFAULT_DOBJD_PORT,
        };
        let server = McpServer::new(MockDobjOps::new(), config);
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
