use std::fs;
use std::process::Command;
use tauri::Manager;

#[tauri::command]
pub fn get_things_dir(app: tauri::AppHandle) -> Result<String, String> {
    if let Ok(path) = std::env::var("THINGS_DIR") {
        if !path.trim().is_empty() {
            return Ok(path);
        }
    }

    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve app data dir: {err}"))?;
    let things = base.join("things");
    Ok(things.to_string_lossy().to_string())
}

#[tauri::command]
pub fn ensure_things_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = get_things_dir(app)?;
    fs::create_dir_all(&dir).map_err(|err| format!("failed to create things dir: {err}"))?;
    Ok(dir)
}

#[tauri::command]
pub fn open_things_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = ensure_things_dir(app)?;

    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(&dir).status();

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer").arg(&dir).status();

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(&dir).status();

    let status = status.map_err(|err| format!("failed to launch folder open command: {err}"))?;
    if !status.success() {
        return Err(format!("folder open command exited with status {status}"));
    }
    Ok(dir)
}
