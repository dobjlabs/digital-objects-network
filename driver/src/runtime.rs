use anyhow::{Result, anyhow};
use std::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub(crate) struct ActionRunState {
    pub(crate) run_in_progress: bool,
}

#[derive(Debug)]
pub(crate) struct ActionRunGate {
    inner: Mutex<ActionRunState>,
}

impl ActionRunGate {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ActionRunState {
                run_in_progress: false,
            }),
        }
    }

    pub(crate) fn acquire(&self) -> Result<RunInProgressGuard<'_>> {
        let mut inner = lock_runtime_state(&self.inner);
        if inner.run_in_progress {
            return Err(anyhow!("another action run is already in progress"));
        }
        inner.run_in_progress = true;
        Ok(RunInProgressGuard {
            state_lock: &self.inner,
        })
    }
}

fn lock_runtime_state<'a>(state_lock: &'a Mutex<ActionRunState>) -> MutexGuard<'a, ActionRunState> {
    match state_lock.lock() {
        Ok(inner) => inner,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct RunInProgressGuard<'a> {
    state_lock: &'a Mutex<ActionRunState>,
}

impl Drop for RunInProgressGuard<'_> {
    fn drop(&mut self) {
        let mut inner = lock_runtime_state(self.state_lock);
        inner.run_in_progress = false;
    }
}
