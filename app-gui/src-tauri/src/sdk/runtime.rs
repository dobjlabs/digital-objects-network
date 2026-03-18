use anyhow::{anyhow, Result};
use std::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub(crate) struct ActionRunState {
    pub(crate) run_in_progress: bool,
}

pub(crate) struct ActionRunGate {
    pub(crate) inner: Mutex<ActionRunState>,
}

impl ActionRunGate {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ActionRunState {
                run_in_progress: false,
            }),
        }
    }
}

pub(super) fn lock_runtime_state<'a>(
    state_lock: &'a Mutex<ActionRunState>,
) -> MutexGuard<'a, ActionRunState> {
    match state_lock.lock() {
        Ok(inner) => inner,
        Err(poisoned) => {
            eprintln!("zk-craft: runtime lock poisoned, recovering state");
            poisoned.into_inner()
        }
    }
}

pub(super) fn lock_runtime<'a>(
    runtime: &'a tauri::State<'_, ActionRunGate>,
) -> MutexGuard<'a, ActionRunState> {
    lock_runtime_state(&runtime.inner)
}

pub(super) struct RunInProgressGuard<'a> {
    state_lock: &'a Mutex<ActionRunState>,
}

impl Drop for RunInProgressGuard<'_> {
    fn drop(&mut self) {
        let mut inner = lock_runtime_state(self.state_lock);
        inner.run_in_progress = false;
    }
}

pub(super) fn acquire_run_in_progress_guard<'a, 'b>(
    runtime: &'a tauri::State<'b, ActionRunGate>,
) -> Result<RunInProgressGuard<'a>> {
    let mut inner = lock_runtime(runtime);
    if inner.run_in_progress {
        return Err(anyhow!("another action run is already in progress"));
    }
    inner.run_in_progress = true;
    Ok(RunInProgressGuard {
        state_lock: &runtime.inner,
    })
}
