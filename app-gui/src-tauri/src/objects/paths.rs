use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tauri::{AppHandle, Manager};

fn home_dir(app: &AppHandle) -> Result<PathBuf> {
    app.path()
        .home_dir()
        .map_err(|err| anyhow!("failed to resolve home directory: {err}"))
}

pub(crate) fn objects_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(home_dir(app)?.join(".objects"))
}

pub(crate) fn nullified_objects_dir(objects_dir: &Path) -> PathBuf {
    objects_dir.join(".nullified")
}
