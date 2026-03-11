use std::sync::Mutex;
use std::time::Instant;

use craft_sdk::SpendableObject;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Lifecycle marker for an object tracked by the runtime.
pub(crate) enum RuntimeValidity {
    /// Object is available for use as an input to actions.
    Live,
    /// Object has been consumed/nullified by a committed action.
    Nullified,
}

#[derive(Debug)]
/// In-memory representation of one object file shown in the GUI.
pub(crate) struct RuntimeObjectRecord {
    /// Stable object identifier (commitment string).
    pub(crate) id: String,
    /// Backing `.dobj` file name on disk.
    pub(crate) file_name: String,
    /// Object class/type name
    pub(crate) class_name: String,
    /// Action that produced this object, when known.
    pub(crate) source_action: Option<String>,
    /// Current lifecycle status for this record.
    pub(crate) validity: RuntimeValidity,
    /// State hash associated with this object at creation/observation time.
    pub(crate) state_hash: String,
    /// Nullifier value once object is consumed.
    pub(crate) nullifier: Option<String>,
    /// Parsed SDK object payload; absent for records loaded as metadata-only.
    pub(crate) spendable: Option<SpendableObject>,
}

#[derive(Debug)]
/// Shared mutable runtime synchronization state.
pub(crate) struct ObjectsRuntimeState {
    /// Guard to prevent concurrent action runs.
    pub(crate) run_in_progress: bool,
}

pub(crate) struct ObjectsRuntime {
    pub(crate) inner: Mutex<ObjectsRuntimeState>,
}

impl ObjectsRuntime {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ObjectsRuntimeState {
                run_in_progress: false,
            }),
        }
    }
}
