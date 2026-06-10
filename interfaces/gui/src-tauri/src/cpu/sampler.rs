use std::time::Instant;

use serde::Serialize;
use sysinfo::ProcessesToUpdate;

use super::monitor::CpuMonitor;
use super::store::{load_total_cpu_secs, save_total_cpu_secs};

/// Payload returned to the frontend for a single CPU sample tick.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpuSample {
    /// Current process CPU usage normalized to 0..100.
    pub(crate) usage_pct: f32,
    /// Running accumulated CPU time in core-seconds.
    pub(crate) total_cpu_secs: f64,
}

fn zero_sample() -> CpuSample {
    CpuSample {
        usage_pct: 0.0,
        total_cpu_secs: 0.0,
    }
}

fn sample_with_total(usage_pct: f32, total_cpu_secs: f64) -> CpuSample {
    CpuSample {
        usage_pct,
        total_cpu_secs,
    }
}

fn normalize_usage_pct(raw_cpu: f32, cpu_count: usize) -> f32 {
    // `sysinfo` reports per-process CPU where 100 ~= one saturated core.
    // We normalize to host-level utilization in the 0..100 range.
    let cores = cpu_count.max(1) as f32;
    (raw_cpu.max(0.0) / cores).min(100.0)
}

#[tauri::command]
pub fn sample_app_cpu(app: tauri::AppHandle, monitor: tauri::State<'_, CpuMonitor>) -> CpuSample {
    let mut system = match monitor.system.lock() {
        Ok(system) => system,
        Err(_) => return zero_sample(),
    };

    let mut loaded = match monitor.total_loaded.lock() {
        Ok(loaded) => loaded,
        Err(_) => return zero_sample(),
    };
    let mut total_cpu_secs = match monitor.total_cpu_secs.lock() {
        Ok(total) => total,
        Err(_) => return zero_sample(),
    };
    let mut last_sample_at = match monitor.last_sample_at.lock() {
        Ok(last) => last,
        Err(_) => return sample_with_total(0.0, *total_cpu_secs),
    };

    // Load persisted CPU total only once per process lifetime.
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
    let core_usage_pct = raw_cpu.max(0.0);
    let usage_pct = normalize_usage_pct(core_usage_pct, system.cpus().len());

    let now = Instant::now();
    let Some(prev) = *last_sample_at else {
        // First sample has no previous timestamp, so just seed the clock.
        // We return usage=0 for this tick to avoid charging unknown prior time.
        *last_sample_at = Some(now);
        let total = (*total_cpu_secs).max(0.0);
        // Keep the persisted accumulator non-negative.
        *total_cpu_secs = total;
        let _ = save_total_cpu_secs(&app, total);
        return sample_with_total(0.0, total);
    };

    // Integrate process CPU over elapsed wall time into core-seconds.
    let dt_secs = (now - prev).as_secs_f64();
    *total_cpu_secs += (core_usage_pct as f64 / 100.0) * dt_secs;
    // Advance the sampling cursor after consuming this interval.
    *last_sample_at = Some(now);

    let total = (*total_cpu_secs).max(0.0);
    // Keep the persisted accumulator non-negative.
    *total_cpu_secs = total;
    let _ = save_total_cpu_secs(&app, total);

    sample_with_total(usage_pct, total)
}
