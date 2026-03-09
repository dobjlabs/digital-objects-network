use serde::{Deserialize, Serialize};

/// Payload returned to the frontend for a single CPU sample tick.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpuSample {
    /// Current process CPU usage normalized to 0..100.
    pub(crate) usage_pct: f32,
    /// Running accumulated CPU time in core-seconds.
    pub(crate) total_cpu_secs: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateDobjProgress {
    pub(crate) dobj_id: String,
    pub(crate) phase: String,
    pub(crate) status: String,
    pub(crate) message: String,
    pub(crate) verify_index: Option<usize>,
    pub(crate) detail: Option<String>,
    pub(crate) old_root: Option<String>,
    pub(crate) new_root: Option<String>,
    pub(crate) output_file: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MethodArgDto {
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) class_hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ObjectMethodDto {
    pub(crate) method_name: String,
    pub(crate) cpu_cost: String,
    pub(crate) reads_block: bool,
    pub(crate) args: Vec<MethodArgDto>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClassMetaDto {
    pub(crate) name: String,
    pub(crate) hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceActionMetaDto {
    pub(crate) name: String,
    pub(crate) hash: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ItemStatDto {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) tone: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InventoryItemDto {
    pub(crate) id: String,
    pub(crate) file_name: String,
    pub(crate) emoji: String,
    pub(crate) validity: String,
    pub(crate) state_root: String,
    pub(crate) nullifier: Option<String>,
    pub(crate) class_meta: ClassMetaDto,
    pub(crate) source_action: Option<SourceActionMetaDto>,
    pub(crate) description: Option<String>,
    pub(crate) methods: Vec<ObjectMethodDto>,
    pub(crate) stats: Vec<ItemStatDto>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecipeDto {
    pub(crate) id: String,
    pub(crate) group: String,
    pub(crate) name: String,
    pub(crate) emoji: String,
    pub(crate) hash: String,
    pub(crate) verb: String,
    pub(crate) desc: String,
    pub(crate) cpu: String,
    pub(crate) reads_block: bool,
    pub(crate) args: Vec<MethodArgDto>,
    pub(crate) unlocked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoadGuiBootstrapResult {
    pub(crate) objects: Vec<InventoryItemDto>,
    pub(crate) actions: Vec<RecipeDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSdkActionInput {
    pub(crate) action_id: String,
    pub(crate) input_object_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSdkActionResult {
    pub(crate) ok: bool,
    pub(crate) old_root: String,
    pub(crate) new_root: String,
    pub(crate) output_files: Vec<String>,
    pub(crate) nullified_files: Vec<String>,
    pub(crate) objects: Vec<InventoryItemDto>,
}
