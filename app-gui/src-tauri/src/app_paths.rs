use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub(crate) fn home_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))
}

pub(crate) fn objects_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(home_dir(app)?.join(".objects"))
}
