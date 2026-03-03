use crate::types::PostDto;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) posts: Mutex<Vec<PostDto>>,
    pub(crate) next_id: AtomicU64,
}

pub(crate) struct CpuMonitor {
    pub(crate) pid: Pid,
    pub(crate) system: Mutex<System>,
    pub(crate) total_cpu_secs: Mutex<f64>,
    pub(crate) last_sample_at: Mutex<Option<Instant>>,
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
