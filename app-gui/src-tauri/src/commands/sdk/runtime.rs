use std::{
    collections::HashSet,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use txlib::StateRoot;

use crate::state::{ObjectsRuntime, ObjectsRuntimeState};

use super::object_store::{ensure_objects_dirs, load_object_files};

pub(super) fn ensure_non_empty_url(name: &str, value: String) -> Result<String, String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(format!("{name} is empty"));
    }
    Ok(trimmed)
}

pub(super) fn empty_state_root() -> StateRoot {
    let empty = HashSet::new();
    StateRoot::new(0, &empty, &empty, &[])
}

pub(super) fn refresh_runtime_objects(
    inner: &mut ObjectsRuntimeState,
    objects_dir: &Path,
) -> Result<(), String> {
    inner.objects = load_object_files(objects_dir)?;
    Ok(())
}

pub(super) fn ensure_runtime_loaded(
    inner: &mut ObjectsRuntimeState,
    objects_dir: &Path,
) -> Result<(), String> {
    if inner.loaded {
        return Ok(());
    }
    ensure_objects_dirs(objects_dir)?;
    refresh_runtime_objects(inner, objects_dir)?;
    inner.state_root = empty_state_root();
    inner.loaded = true;
    Ok(())
}

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
