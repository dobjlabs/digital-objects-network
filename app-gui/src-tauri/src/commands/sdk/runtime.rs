use std::sync::{Mutex, MutexGuard};

use crate::state::{ObjectsRuntime, ObjectsRuntimeState};

pub(super) fn lock_runtime_state<'a>(
    state_lock: &'a Mutex<ObjectsRuntimeState>,
) -> MutexGuard<'a, ObjectsRuntimeState> {
    match state_lock.lock() {
        Ok(inner) => inner,
        Err(poisoned) => {
            eprintln!("zk-craft: runtime lock poisoned, recovering state");
            poisoned.into_inner()
        }
    }
}

pub(super) fn lock_runtime<'a>(
    runtime: &'a tauri::State<'_, ObjectsRuntime>,
) -> MutexGuard<'a, ObjectsRuntimeState> {
    lock_runtime_state(&runtime.inner)
}

pub(super) struct RunInProgressGuard<'a> {
    state_lock: &'a Mutex<ObjectsRuntimeState>,
}

impl Drop for RunInProgressGuard<'_> {
    fn drop(&mut self) {
        let mut inner = lock_runtime_state(self.state_lock);
        inner.run_in_progress = false;
    }
}

pub(super) fn acquire_run_in_progress_guard<'a, 'b>(
    runtime: &'a tauri::State<'b, ObjectsRuntime>,
) -> Result<RunInProgressGuard<'a>, String> {
    let mut inner = lock_runtime(runtime);
    if inner.run_in_progress {
        return Err("another action run is already in progress".to_string());
    }
    inner.run_in_progress = true;
    Ok(RunInProgressGuard {
        state_lock: &runtime.inner,
    })
}
