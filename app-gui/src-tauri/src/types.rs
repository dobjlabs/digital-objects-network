use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProofClaimDto {
    pub(crate) name: String,
    pub(crate) validity: String,
    pub(crate) hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostDto {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) peer: String,
    pub(crate) time: String,
    pub(crate) desc: String,
    pub(crate) proofs: Vec<ProofClaimDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MockStateDto {
    pub(crate) post_count: usize,
    pub(crate) supported_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunMethodInput {
    pub(crate) id: String,
    pub(crate) method_name: String,
    pub(crate) input_files: Vec<String>,
    pub(crate) cpu_cost: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProofRunResult {
    pub(crate) success: bool,
    pub(crate) method_name: String,
    pub(crate) old_root: String,
    pub(crate) new_root: String,
    pub(crate) stage_messages: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifyPostInput {
    pub(crate) post_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifyResult {
    pub(crate) post_id: String,
    pub(crate) status: String,
    pub(crate) checked_block: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePostInput {
    pub(crate) title: String,
    pub(crate) desc: String,
    pub(crate) proof_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RespondPostInput {
    pub(crate) post_id: String,
    pub(crate) desc: String,
    pub(crate) proof_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachClaimInput {
    pub(crate) file_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachClaimResult {
    pub(crate) name: String,
    pub(crate) validity: String,
    pub(crate) hash: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericActionResult {
    pub(crate) ok: bool,
    pub(crate) message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpuSampleDto {
    pub(crate) usage_pct: f32,
    pub(crate) total_cpu_secs: f64,
}
