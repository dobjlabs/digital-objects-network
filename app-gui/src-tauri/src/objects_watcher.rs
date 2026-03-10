use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager};

pub const OBJECTS_CHANGED_EVENT: &str = "objects-changed";

fn objects_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))?;
    Ok(home.join(".objects"))
}

fn is_relevant_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Any
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Remove(_)
            | EventKind::Other
    )
}

fn is_objects_change(event: &Event, watch_dir: &Path) -> bool {
    if !is_relevant_kind(&event.kind) {
        return false;
    }
    event
        .paths
        .iter()
        .any(|path| path.starts_with(watch_dir))
}

pub fn start_objects_watcher(app: AppHandle) -> Result<(), String> {
    let watch_dir = objects_dir(&app)?;
    fs::create_dir_all(&watch_dir)
        .map_err(|err| format!("failed to create objects directory for watcher: {err}"))?;

    thread::spawn(move || {
        let app_handle = app.clone();
        let watch_dir_for_event = watch_dir.clone();
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watcher = match RecommendedWatcher::new(
            move |result| {
                let _ = tx.send(result);
            },
            Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(err) => {
                eprintln!("zk-craft: failed to create objects watcher: {err}");
                return;
            }
        };

        if let Err(err) = watcher.watch(&watch_dir, RecursiveMode::Recursive) {
            eprintln!(
                "zk-craft: failed to watch objects directory {}: {err}",
                watch_dir.display()
            );
            return;
        }

        while let Ok(result) = rx.recv() {
            match result {
                Ok(event) => {
                    if is_objects_change(&event, &watch_dir_for_event) {
                        let _ = app_handle.emit(OBJECTS_CHANGED_EVENT, ());
                    }
                }
                Err(err) => eprintln!("zk-craft: watcher event error: {err}"),
            }
        }
    });

    Ok(())
}
