use std::sync::Mutex;
use std::time::Instant;

use craft_sdk::SpendableObject;
use sysinfo::{Pid, ProcessesToUpdate, System};
use txlib::StateRoot;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeValidity {
    Live,
    Nullified,
}

#[derive(Debug)]
pub(crate) struct RuntimeObjectRecord {
    pub(crate) id: String,
    pub(crate) file_name: String,
    pub(crate) class_name: String,
    pub(crate) source_action: Option<String>,
    pub(crate) validity: RuntimeValidity,
    pub(crate) state_hash: String,
    pub(crate) nullifier: Option<String>,
    pub(crate) spendable: Option<SpendableObject>,
}

#[derive(Debug)]
pub(crate) struct ObjectsRuntimeState {
    pub(crate) loaded: bool,
    pub(crate) run_in_progress: bool,
    pub(crate) state_root: StateRoot,
    pub(crate) objects: Vec<RuntimeObjectRecord>,
}

pub(crate) struct ObjectsRuntime {
    pub(crate) inner: Mutex<ObjectsRuntimeState>,
}

impl ObjectsRuntime {
    pub(crate) fn new() -> Self {
        let empty = std::collections::HashSet::new();
        Self {
            inner: Mutex::new(ObjectsRuntimeState {
                loaded: false,
                run_in_progress: false,
                state_root: StateRoot::new(0, &empty, &empty, &[]),
                objects: Vec::new(),
            }),
        }
    }
}
