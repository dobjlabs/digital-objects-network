use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

const OBJECTS_CHANGED_EVENT: &str = "objects-changed";

struct CpuMonitor {
    pid: Pid,
    system: Mutex<System>,
}

struct MiningState {
    active: Mutex<bool>,
}

impl CpuMonitor {
    fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        let mut system = System::new_all();
        let _ = system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

        Self {
            pid,
            system: Mutex::new(system),
        }
    }
}

fn objects_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let path = app
        .path()
        .app_local_data_dir()
        .map_err(|err| format!("failed to resolve app local data dir: {err}"))?
        .join("objects");
    fs::create_dir_all(&path).map_err(|err| format!("failed to create objects dir: {err}"))?;
    Ok(path)
}

fn is_pod_change(event: &Event, watch_dir: &std::path::Path) -> bool {
    event.paths.iter().any(|path| {
        path.starts_with(watch_dir)
            && (path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pod"))
                || path == watch_dir)
    })
}

fn start_objects_watcher(app: tauri::AppHandle) -> Result<(), String> {
    let watch_dir = objects_dir(&app)?;

    thread::spawn(move || {
        let app_handle = app.clone();
        let watch_dir_for_event = watch_dir.clone();
        let mut watcher = match RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                if let Ok(event) = result {
                    if is_pod_change(&event, &watch_dir_for_event) {
                        let _ = app_handle.emit(OBJECTS_CHANGED_EVENT, ());
                    }
                }
            },
            Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(_) => return,
        };

        if watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .is_err()
        {
            return;
        }

        loop {
            thread::park_timeout(Duration::from_secs(3600));
        }
    });

    Ok(())
}

fn run_cpu_burn(duration: Duration) {
    let available_cores = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let worker_count = available_cores.saturating_sub(1).max(1);
    let deadline = Instant::now() + duration;

    let mut handles = Vec::with_capacity(worker_count);
    for worker_id in 0..worker_count {
        handles.push(thread::spawn(move || {
            let mut nonce = worker_id as u64 + 1;
            let mut rounds: u64 = 0;
            while Instant::now() < deadline {
                let mut hasher = Sha256::new();
                hasher.update(nonce.to_le_bytes());
                hasher.update(nonce.rotate_left(7).to_le_bytes());
                let digest = hasher.finalize();
                let mut next = [0_u8; 8];
                next.copy_from_slice(&digest[..8]);
                nonce = nonce.wrapping_add(u64::from_le_bytes(next));
                std::hint::black_box(nonce);
                rounds = rounds.wrapping_add(1);

                // Periodic yielding keeps the WebView thread responsive.
                if rounds % 8192 == 0 {
                    thread::yield_now();
                }
            }
        }));
    }

    for handle in handles {
        let _ = handle.join();
    }
}

fn short_hash_id(seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    // 8 hex chars is short but still practical for local uniqueness.
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[tauri::command]
fn sample_app_cpu(monitor: tauri::State<'_, CpuMonitor>) -> f32 {
    let mut system = match monitor.system.lock() {
        Ok(system) => system,
        Err(_) => return 0.0,
    };

    let _ = system.refresh_processes(ProcessesToUpdate::Some(&[monitor.pid]), true);

    let raw_cpu = system
        .process(monitor.pid)
        .map(|process| process.cpu_usage())
        .unwrap_or(0.0);
    let cpu_count = system.cpus().len().max(1) as f32;

    (raw_cpu / cpu_count).max(0.0)
}

#[tauri::command]
async fn mine_copper(
    app: tauri::AppHandle,
    mining_state: tauri::State<'_, MiningState>,
) -> Result<String, String> {
    {
        let mut active = mining_state
            .active
            .lock()
            .map_err(|_| "failed to lock mining state".to_string())?;
        if *active {
            return Err("mining is already running".to_string());
        }
        *active = true;
    }

    let app_handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_cpu_burn(Duration::from_secs(10));

        let objects_path = objects_dir(&app_handle)?;
        let mut file_name = String::new();
        for nonce in 0..1000_u32 {
            let seed = format!(
                "{}:{}:{:?}",
                std::process::id(),
                nonce,
                std::time::SystemTime::now()
            );
            let id = short_hash_id(&seed);
            let candidate = format!("copper_{id}.pod");
            if !objects_path.join(&candidate).exists() {
                file_name = candidate;
                break;
            }
        }
        if file_name.is_empty() {
            return Err("failed to allocate unique copper file id".to_string());
        }

        let file_path = objects_path.join(&file_name);
        let timestamp = format!("{:?}", std::time::SystemTime::now());
        let contents = format!("resource=copper\nstatus=mined\ncreated_at={timestamp}\n");
        fs::write(&file_path, contents)
            .map_err(|err| format!("failed to write pod file: {err}"))?;

        Ok(file_name.to_string())
    })
    .await
    .map_err(|err| format!("mining task failed: {err}"))?;

    if let Ok(mut active) = mining_state.active.lock() {
        *active = false;
    }

    result
}

#[tauri::command]
fn list_objects(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let dir = objects_dir(&app)?;
    let mut objects = Vec::new();

    let entries = fs::read_dir(dir).map_err(|err| format!("failed to read objects dir: {err}"))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("failed to read object entry: {err}"))?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pod"))
        {
            if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                objects.push(name.to_string());
            }
        }
    }

    objects.sort_unstable();
    Ok(objects)
}

#[tauri::command]
fn open_objects_folder(app: tauri::AppHandle) -> Result<(), String> {
    let dir = objects_dir(&app)?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<String>)
        .map_err(|err| format!("failed to open objects folder: {err}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(CpuMonitor::new())
        .manage(MiningState {
            active: Mutex::new(false),
        })
        .setup(|app| {
            start_objects_watcher(app.handle().clone())
                .map_err(|err| std::io::Error::other(err))?;
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            sample_app_cpu,
            mine_copper,
            list_objects,
            open_objects_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
