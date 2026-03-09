use crate::state::CpuMonitor;
use crate::types::CpuSample;
use std::fs;
use std::time::Instant;
use sysinfo::ProcessesToUpdate;
use tauri::Manager;

fn cpu_stats_file_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve app data dir: {err}"))?;
    fs::create_dir_all(&base).map_err(|err| format!("failed to create app data dir: {err}"))?;
    Ok(base.join("cpu_stats.json"))
}

fn load_total_cpu_secs(app: &tauri::AppHandle) -> Result<f64, String> {
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

fn save_total_cpu_secs(app: &tauri::AppHandle, total: f64) -> Result<(), String> {
    let path = cpu_stats_file_path(app)?;
    let payload = serde_json::json!({ "totalCpuSecs": total });
    let serialized = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize cpu stats: {err}"))?;
    fs::write(&path, serialized).map_err(|err| format!("failed to write cpu stats file: {err}"))?;
    Ok(())
}

#[tauri::command]
pub fn sample_app_cpu(app: tauri::AppHandle, monitor: tauri::State<'_, CpuMonitor>) -> CpuSample {
    let mut system = match monitor.system.lock() {
        Ok(system) => system,
        Err(_) => {
            return CpuSample {
                usage_pct: 0.0,
                total_cpu_secs: 0.0,
            }
        }
    };

    let mut loaded = match monitor.total_loaded.lock() {
        Ok(loaded) => loaded,
        Err(_) => {
            return CpuSample {
                usage_pct: 0.0,
                total_cpu_secs: 0.0,
            }
        }
    };
    let mut total_cpu_secs = match monitor.total_cpu_secs.lock() {
        Ok(total) => total,
        Err(_) => {
            return CpuSample {
                usage_pct: 0.0,
                total_cpu_secs: 0.0,
            }
        }
    };
    let mut last_sample_at = match monitor.last_sample_at.lock() {
        Ok(last) => last,
        Err(_) => {
            return CpuSample {
                usage_pct: 0.0,
                total_cpu_secs: *total_cpu_secs,
            }
        }
    };

    if !*loaded {
        if let Ok(total) = load_total_cpu_secs(&app) {
            *total_cpu_secs = total.max(0.0);
        }
        *loaded = true;
    }

    // Refresh this process snapshot so `cpu_usage()` reflects current values
    // instead of stale data from the previous sample.
    let _ = system.refresh_processes(ProcessesToUpdate::Some(&[monitor.pid]), true);

    let raw_cpu = system
        .process(monitor.pid)
        .map(|process| process.cpu_usage())
        .unwrap_or(0.0);
    // `sysinfo` reports per-process CPU where 100 ~= one saturated core.
    // Convert to a host-normalized percentage for the dashboard line chart.
    let core_usage_pct = raw_cpu.clamp(0.0, 100.0);
    let cpu_count = system.cpus().len().max(1) as f32;
    let usage_pct = (core_usage_pct / cpu_count).clamp(0.0, 100.0);

    let now = Instant::now();
    if let Some(prev) = *last_sample_at {
        let dt_secs = (now - prev).as_secs_f64();
        *total_cpu_secs += (core_usage_pct as f64 / 100.0) * dt_secs;
    } else {
        *last_sample_at = Some(now);
        let clamped_total = (*total_cpu_secs).max(0.0);
        let _ = save_total_cpu_secs(&app, clamped_total);
        return CpuSample {
            usage_pct: 0.0,
            total_cpu_secs: clamped_total,
        };
    }
    *last_sample_at = Some(now);

    if *total_cpu_secs < 0.0 {
        *total_cpu_secs = 0.0;
    }
    let clamped_total = *total_cpu_secs;
    let _ = save_total_cpu_secs(&app, clamped_total);

    CpuSample {
        usage_pct,
        total_cpu_secs: clamped_total,
    }
}
