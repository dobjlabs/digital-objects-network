use std::path::PathBuf;

use crate::error::CommandError;
use anyhow::anyhow;
use rfd::FileDialog;

/// Native file picker for `.dobj` files. Returns the absolute path of the
/// chosen file. Desktop-only convenience — the website uses drag-and-drop
/// against `dobjd`'s `POST /objects/parse` endpoint instead.
#[tauri::command]
pub fn pick_dobj_file_path() -> Result<String, CommandError> {
    let Some(path) = FileDialog::new()
        .add_filter("Digital Object (.dobj)", &[::driver::paths::DOBJ_EXTENSION])
        .pick_file()
    else {
        return Err(anyhow!("No file selected").into());
    };
    Ok(path.to_string_lossy().to_string())
}

/// Parse a `.dobj` file from disk, without going through the driver
/// process. Used by the desktop GUI to inspect a freshly-picked file before
/// passing its name to a `runAction` call (which still goes through dobjd).
#[tauri::command]
pub fn read_dobj_file(path: String) -> Result<::driver::ObjectRecord, CommandError> {
    let path = PathBuf::from(path.trim());
    if !path.exists() {
        return Err(anyhow!("selected file does not exist: {}", path.display()).into());
    }
    Ok(::driver::parse_object_record_file(&path)?)
}
