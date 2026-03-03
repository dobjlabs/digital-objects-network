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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateDobjInput {
    pub(crate) dobj_id: String,
    pub(crate) input_files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateDobjResult {
    pub(crate) ok: bool,
    pub(crate) old_root: String,
    pub(crate) new_root: String,
    pub(crate) output_file: String,
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
