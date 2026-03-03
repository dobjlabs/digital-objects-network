use serde::Serialize;

/// Payload returned to the frontend for a single CPU sample tick.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpuSample {
    /// Current process CPU usage normalized to 0..100.
    pub(crate) usage_pct: f32,
    /// Running accumulated CPU time in core-seconds.
    pub(crate) total_cpu_secs: f64,
}
