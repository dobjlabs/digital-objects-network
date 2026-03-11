use std::{fs, path::PathBuf};

use tauri::Manager;

fn cpu_stats_file_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve app data dir: {err}"))?;
    fs::create_dir_all(&base).map_err(|err| format!("failed to create app data dir: {err}"))?;
    Ok(base.join("cpu_stats.json"))
}

pub(super) fn load_total_cpu_secs(app: &tauri::AppHandle) -> Result<f64, String> {
    let path = cpu_stats_file_path(app)?;
    if !path.exists() {
        return Ok(0.0);
    }

    let contents =
        fs::read_to_string(&path).map_err(|err| format!("failed to read cpu stats file: {err}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|err| format!("failed to parse cpu stats file: {err}"))?;
    Ok(parsed
        .get("totalCpuSecs")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0))
}

pub(super) fn save_total_cpu_secs(app: &tauri::AppHandle, total: f64) -> Result<(), String> {
    let path = cpu_stats_file_path(app)?;
    let payload = serde_json::json!({ "totalCpuSecs": total });
    let serialized =
        serde_json::to_string(&payload).map_err(|err| format!("failed to serialize cpu stats: {err}"))?;
    fs::write(&path, serialized).map_err(|err| format!("failed to write cpu stats file: {err}"))?;
    Ok(())
}
