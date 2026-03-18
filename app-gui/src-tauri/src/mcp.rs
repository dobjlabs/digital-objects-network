use std::collections::HashMap;
use std::path::PathBuf;

use tauri::Emitter;
use zk_craft_mcp::ops::CraftOps;
use zk_craft_mcp::types as mcp;

use crate::objects::ObjectRecord;
use crate::sdk::{object_store, synchronizer_client};
use crate::settings::AppSettings;
use crate::spec;

/// Real implementation of CraftOps backed by the app's live state.
///
/// Read-only tools are implemented directly (disk reads + HTTP to synchronizer).
/// `run_action` delegates to the Tauri command so it gets the full pipeline:
/// run gate, progress events, proof generation, relayer, synchronizer, file I/O.
pub(crate) struct AppCraftOps {
    objects_dir: PathBuf,
    app: tauri::AppHandle,
    settings: AppSettings,
    /// Generated podlang source from craft_sdk::Helper.
    /// Contains all action predicates and IsClassName class predicates.
    podlang_src: String,
}

impl AppCraftOps {
    pub(crate) fn new(objects_dir: PathBuf, app: tauri::AppHandle, settings: AppSettings) -> Self {
        let helper = craft_sdk::Helper::new(spec::dependencies(), spec::actions());
        let podlang_src = helper.podlang_src.clone();
        Self {
            objects_dir,
            app,
            settings,
            podlang_src,
        }
    }

    fn load_objects(&self) -> anyhow::Result<Vec<(String, ObjectRecord)>> {
        let entries =
            object_store::load_object_files(&self.objects_dir).map_err(|e| anyhow::anyhow!(e))?;
        Ok(entries
            .into_iter()
            .map(|e| (e.file_name, e.record))
            .collect())
    }

    /// Resolve a path that may be a bare filename to an absolute path.
    fn resolve_object_path(&self, path_str: &str) -> PathBuf {
        let path = std::path::Path::new(path_str.trim());
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.objects_dir.join(path)
        }
    }
}

impl CraftOps for AppCraftOps {
    fn list_inventory(&self) -> anyhow::Result<Vec<mcp::InventoryObject>> {
        let objects = self.load_objects()?;
        Ok(objects
            .iter()
            .map(|(file_name, record)| to_mcp_inventory_object(record, file_name))
            .collect())
    }

    fn list_actions(&self) -> anyhow::Result<Vec<mcp::Action>> {
        Ok(spec::visible_action_descriptors()
            .into_iter()
            .map(|d| mcp::Action {
                id: d.name,
                description: d.ui.description.to_string(),
                input_classes: d.input_classes.clone(),
                output_classes: d.output_classes.clone(),
                cpu_cost: d.ui.cpu_cost.to_string(),
            })
            .collect())
    }

    fn list_classes(&self) -> anyhow::Result<Vec<mcp::ClassSummary>> {
        let objects = self.load_objects()?;
        let actions = spec::visible_action_descriptors();

        let mut classes: Vec<mcp::ClassSummary> = spec::class_names()
            .into_iter()
            .map(|name| {
                let live_count = objects
                    .iter()
                    .filter(|(_, r)| r.class_name == name && !r.is_nullified())
                    .count();
                let produced_by = actions
                    .iter()
                    .filter(|a| a.output_classes.contains(&name))
                    .map(|a| a.name.clone())
                    .collect();
                let consumed_by = actions
                    .iter()
                    .filter(|a| a.input_classes.contains(&name))
                    .map(|a| a.name.clone())
                    .collect();
                mcp::ClassSummary {
                    name,
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
        let sync_state =
            synchronizer_client::fetch_synchronizer_state(&self.settings.synchronizer_api_url)
                .map_err(|e| anyhow::anyhow!(e))?;
        Ok(synchronizer_client::encode_hash_hex(
            &sync_state.current_gsr,
        ))
    }

    fn inspect_object(&self, object_id: &str) -> anyhow::Result<mcp::ObjectDetail> {
        let objects = self.load_objects()?;
        let (_, record) = objects
            .iter()
            .find(|(_, r)| r.id == object_id)
            .ok_or_else(|| anyhow::anyhow!("object not found: {object_id}"))?;

        let class_detail = self.inspect_class(&record.class_name)?;
        let fields = object_fields(record);

        Ok(mcp::ObjectDetail {
            id: record.id.clone(),
            class_name: record.class_name.clone(),
            live: !record.is_nullified(),
            state: fields,
            predicate_source: class_detail.predicate_source,
        })
    }

    fn inspect_class(&self, class_name: &str) -> anyhow::Result<mcp::ClassDetail> {
        if !spec::class_names().contains(&class_name.to_string()) {
            anyhow::bail!("unknown class: {class_name}");
        }

        let actions = spec::visible_action_descriptors();
        let produced_by = actions
            .iter()
            .filter(|a| a.output_classes.contains(&class_name.to_string()))
            .map(|a| a.name.clone())
            .collect();
        let consumed_by = actions
            .iter()
            .filter(|a| a.input_classes.contains(&class_name.to_string()))
            .map(|a| a.name.clone())
            .collect();

        let predicate_source = extract_predicate(&self.podlang_src, &format!("Is{class_name}"))
            .unwrap_or_else(|| format!("Is{class_name}(state) = OR(...)"));

        Ok(mcp::ClassDetail {
            class_name: class_name.to_string(),
            predicate_source,
            produced_by,
            consumed_by,
        })
    }

    fn run_action(&self, input: mcp::RunActionInput) -> anyhow::Result<mcp::RunActionResult> {
        // Resolve bare filenames to absolute paths
        let absolute_paths: Vec<String> = input
            .input_object_paths
            .iter()
            .map(|p| self.resolve_object_path(p).to_string_lossy().into_owned())
            .collect();

        let tauri_input = crate::sdk::run_action::RunSdkActionInput {
            action_id: input.action_id.clone(),
            input_object_paths: absolute_paths,
        };

        // Delegate to the shared action pipeline. This is the same code path as
        // the GUI's "Run" button: run gate, progress events, proof generation,
        // relayer submission, synchronizer wait, file I/O — all with live GUI feedback.
        //
        // CraftOps::run_action is sync but the pipeline is async. We spawn a
        // dedicated thread with its own tokio runtime to avoid deadlocking
        // whichever runtime the MCP server is using.
        let app = self.app.clone();
        let run_gate = std::sync::Arc::clone(&*tauri::Manager::state::<
            std::sync::Arc<crate::sdk::runtime::ActionRunGate>,
        >(&self.app));

        // Notify the GUI so it shows the proof progress panel
        let cpu_cost = spec::visible_action_descriptors()
            .iter()
            .find(|d| d.name == input.action_id)
            .map(|d| d.ui.cpu_cost.to_string())
            .unwrap_or_default();
        let _ = self.app.emit(
            "mcp-action-started",
            serde_json::json!({
                "actionId": input.action_id,
                "cpuCost": cpu_cost,
            }),
        );

        // Spawn on Tauri's async runtime (so spawn_blocking inside
        // run_sdk_action_core resolves correctly) and wait via a channel.
        let (tx, rx) = std::sync::mpsc::channel();
        tauri::async_runtime::spawn(async move {
            let result = crate::sdk::run_sdk_action_core(app, &run_gate, tauri_input).await;
            let _ = tx.send(result);
        });
        let result = rx
            .recv()
            .map_err(|_| anyhow::anyhow!("action task dropped unexpectedly"))?
            .map_err(|e| anyhow::anyhow!(e))?;

        // Load output objects from disk to populate full details
        let outputs = result
            .output_files
            .iter()
            .map(|f| {
                let path = self.objects_dir.join(f);
                match crate::sdk::object_store::parse_object_file_from_path(&path) {
                    Ok(record) => to_mcp_inventory_object(&record, f),
                    Err(_) => mcp::InventoryObject {
                        id: String::new(),
                        class_name: String::new(),
                        file_name: f.clone(),
                        live: true,
                        fields: HashMap::new(),
                    },
                }
            })
            .collect();

        Ok(mcp::RunActionResult {
            success: result.ok,
            message: format!(
                "Action {} completed. Old root: {}, New root: {}",
                input.action_id, result.old_root, result.new_root
            ),
            outputs,
            consumed: result.nullified_files,
        })
    }

    fn check_feasibility(&self, action_id: &str) -> anyhow::Result<mcp::FeasibilityReport> {
        let descriptors = spec::action_descriptors_by_name();
        let descriptor = descriptors
            .get(action_id)
            .ok_or_else(|| anyhow::anyhow!("unknown action: {action_id}"))?;

        let objects = self.load_objects()?;
        let live_objects: Vec<_> = objects.iter().filter(|(_, r)| !r.is_nullified()).collect();

        let mut available = Vec::new();
        let mut missing = Vec::new();
        let mut used_ids = std::collections::HashSet::new();

        for required_class in &descriptor.input_classes {
            if let Some((file_name, record)) = live_objects
                .iter()
                .find(|(_, r)| &r.class_name == required_class && !used_ids.contains(&r.id))
            {
                used_ids.insert(record.id.clone());
                available.push(mcp::FeasibilityInput {
                    class_name: record.class_name.clone(),
                    object_id: record.id.clone(),
                    file_name: file_name.clone(),
                });
            } else {
                missing.push(required_class.clone());
            }
        }

        Ok(mcp::FeasibilityReport {
            feasible: missing.is_empty(),
            action_id: action_id.to_string(),
            available_inputs: available,
            missing_inputs: missing,
        })
    }

    fn generated_podlang(&self) -> Option<String> {
        Some(self.podlang_src.clone())
    }
}

fn to_mcp_inventory_object(record: &ObjectRecord, file_name: &str) -> mcp::InventoryObject {
    mcp::InventoryObject {
        id: record.id.clone(),
        class_name: record.class_name.clone(),
        file_name: file_name.to_string(),
        live: !record.is_nullified(),
        fields: object_fields(record),
    }
}

fn object_fields(record: &ObjectRecord) -> HashMap<String, serde_json::Value> {
    match serde_json::to_value(&record.obj) {
        Ok(serde_json::Value::Object(map)) => map.into_iter().collect(),
        Ok(val) => {
            let mut m = HashMap::new();
            m.insert("_raw".to_string(), val);
            m
        }
        Err(_) => HashMap::new(),
    }
}

/// Extract a named predicate definition from podlang source.
///
/// Podlang definitions have the form:
///   Name(args) = AND/OR(\n  ...\n)\n
///
/// We find `name(` at a line start, then scan forward to find the
/// `= AND(` or `= OR(` combiner, and track paren depth from there
/// to find the closing `)`.
fn extract_predicate(podlang_src: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}(");
    let start = podlang_src.find(&prefix)?;
    let after_prefix = &podlang_src[start..];

    // Find the combiner `= AND(` or `= OR(`
    let combiner_pos = after_prefix
        .find("= AND(")
        .or_else(|| after_prefix.find("= OR("))?;

    // Find the opening `(` of the combiner
    let open = start + combiner_pos + after_prefix[combiner_pos..].find('(')?;

    // Track depth from the combiner's `(`
    let mut depth = 0;
    for (i, ch) in podlang_src[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(podlang_src[start..open + i + 1].trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}
