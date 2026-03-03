use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{Pid, ProcessesToUpdate, System};

/// Shared runtime state used by the CPU sampling command.
///
/// This tracks the current process in `sysinfo`, plus rolling CPU totals so
/// the frontend can render both instantaneous usage and accumulated CPU time.
pub(crate) struct CpuMonitor {
    /// PID for this Tauri process.
    pub(crate) pid: Pid,
    /// `sysinfo` system handle used to refresh process CPU stats.
    pub(crate) system: Mutex<System>,
    /// Accumulated CPU time in core-seconds.
    pub(crate) total_cpu_secs: Mutex<f64>,
    /// Wall-clock time of the previous sample.
    pub(crate) last_sample_at: Mutex<Option<Instant>>,
    /// Ensures persisted CPU totals are loaded from disk only once.
    pub(crate) total_loaded: Mutex<bool>,
}

impl CpuMonitor {
    pub(crate) fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        let mut system = System::new_all();
        let _ = system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        Self {
            pid,
            system: Mutex::new(system),
            total_cpu_secs: Mutex::new(0.0),
            last_sample_at: Mutex::new(None),
            total_loaded: Mutex::new(false),
        }
    }
}
