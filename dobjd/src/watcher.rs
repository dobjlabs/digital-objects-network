use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use anyhow::{Result, anyhow};
use notify::{Config, Event as FsEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::events::{Event, EventTx};

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

fn is_objects_change(event: &FsEvent, watch_dir: &Path) -> bool {
    if !is_relevant_kind(&event.kind) {
        return false;
    }
    event.paths.iter().any(|path| path.starts_with(watch_dir))
}

/// Spawn a background OS thread that watches `watch_dir` and forwards
/// relevant filesystem events to the broadcast hub as
/// [`Event::ObjectsChanged`].
///
/// The watcher uses a blocking `mpsc::channel` (the `notify` crate's preferred
/// API) on a dedicated OS thread, so it stays out of the tokio runtime.
pub fn start_objects_watcher(events: EventTx, watch_dir: PathBuf) -> Result<()> {
    fs::create_dir_all(&watch_dir)
        .map_err(|err| anyhow!("failed to create objects directory for watcher: {err}"))?;

    thread::spawn(move || {
        let watch_dir_for_event = watch_dir.clone();
        let (tx, rx) = mpsc::channel::<notify::Result<FsEvent>>();

        let mut watcher = match RecommendedWatcher::new(
            move |result| {
                let _ = tx.send(result);
            },
            Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(err) => {
                eprintln!("dobjd: failed to create objects watcher: {err}");
                return;
            }
        };

        if let Err(err) = watcher.watch(&watch_dir, RecursiveMode::Recursive) {
            eprintln!(
                "dobjd: failed to watch objects directory {}: {err}",
                watch_dir.display()
            );
            return;
        }

        while let Ok(result) = rx.recv() {
            match result {
                Ok(event) => {
                    if is_objects_change(&event, &watch_dir_for_event) {
                        let _ = events.send(Event::ObjectsChanged);
                    }
                }
                Err(err) => eprintln!("dobjd: watcher event error: {err}"),
            }
        }
    });

    Ok(())
}
