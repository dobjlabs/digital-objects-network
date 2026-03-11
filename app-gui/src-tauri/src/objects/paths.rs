use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

fn home_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))
}

pub(crate) fn objects_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(home_dir(app)?.join(".objects"))
}

pub(crate) fn nullified_objects_dir(objects_dir: &Path) -> PathBuf {
    objects_dir.join(".nullified")
}
