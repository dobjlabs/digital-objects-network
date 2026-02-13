use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use pod2::{
    backends::plonky2::{
        basetypes::DEFAULT_VD_SET, mainpod::Prover, primitives::ec::schnorr::SecretKey,
        signer::Signer,
    },
    frontend::{MainPodBuilder, Operation, SignedDictBuilder},
    lang::load_module,
    middleware::{hash_values, MainPodProver, Params, Value},
};
use sha2::{Digest, Sha256};
use std::{any::Any, fs, path::PathBuf, sync::Mutex, thread, time::Duration};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

const OBJECTS_CHANGED_EVENT: &str = "objects-changed";
const MINING_LOG_EVENT: &str = "mining-log";

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

fn short_hash_id(seed: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed);
    let digest = hasher.finalize();
    // 8 hex chars is short but still practical for local uniqueness.
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

fn emit_mining_log(app: &tauri::AppHandle, message: &str) {
    eprintln!("[mine] {message}");
    let _ = app.emit(MINING_LOG_EVENT, message.to_string());
}

fn build_copper_pod_json(log: &mut dyn FnMut(&str)) -> Result<Vec<u8>, String> {
    log("Initializing pod params...");
    let params = Params::default();
    let real_prover = Prover {};
    let vd_set = &*DEFAULT_VD_SET;
    let prover: &dyn MainPodProver = &real_prover;

    log("Generating signer key...");
    let signer = Signer(SecretKey::new_rand());
    let predicate_src = r#"
        LightSwitch_base(new_state, private: action, mid_state, old_state, new_state_hash) = AND(
            HashOf(new_state_hash, new_state, 0)
            Equal(old_state.secret, 0)
            DictUpdate(mid_state, old_state, "position", action.position)
            DictUpdate(new_state, mid_state, "secret", action.secret)
            Equal(action.type, "base")
        )
    "#;

    log("Building old_state...");
    let mut old_state_builder: SignedDictBuilder = SignedDictBuilder::new(&params);
    old_state_builder.insert("position", "");
    old_state_builder.insert("secret", 0);
    let old_state = old_state_builder
        .sign(&signer)
        .map_err(|err| format!("failed to sign old_state: {err}"))?;
    old_state
        .verify()
        .map_err(|err| format!("failed to verify old_state: {err}"))?;

    log("Building mid_state...");
    let mut mid_state_builder: SignedDictBuilder = SignedDictBuilder::new(&params);
    mid_state_builder.insert("position", "on");
    mid_state_builder.insert("secret", 0);
    let mid_state = mid_state_builder
        .sign(&signer)
        .map_err(|err| format!("failed to sign mid_state: {err}"))?;
    mid_state
        .verify()
        .map_err(|err| format!("failed to verify mid_state: {err}"))?;

    log("Building new_state...");
    let mut new_state_builder: SignedDictBuilder = SignedDictBuilder::new(&params);
    new_state_builder.insert("position", "on");
    new_state_builder.insert("secret", 42);
    let new_state = new_state_builder
        .sign(&signer)
        .map_err(|err| format!("failed to sign new_state: {err}"))?;
    new_state
        .verify()
        .map_err(|err| format!("failed to verify new_state: {err}"))?;

    log("Building action...");
    let mut action_builder: SignedDictBuilder = SignedDictBuilder::new(&params);
    action_builder.insert("position", "on");
    action_builder.insert("secret", 42);
    action_builder.insert("type", "base");
    let action = action_builder
        .sign(&signer)
        .map_err(|err| format!("failed to sign action: {err}"))?;
    action
        .verify()
        .map_err(|err| format!("failed to verify action: {err}"))?;

    log("Preparing operations...");
    let new_state_hash = hash_values(&[Value::from(new_state.dict.clone()), Value::from(0)]);

    let mut builder = MainPodBuilder::new(&params, vd_set);
    let st_new_state_hash = builder
        .priv_op(Operation::hash_of(
            new_state_hash,
            new_state.dict.clone(),
            0,
        ))
        .map_err(|err| format!("failed hash_of op: {err}"))?;
    let st_equal_secret = builder
        .priv_op(Operation::eq((&old_state, "secret"), 0))
        .map_err(|err| format!("failed eq old_state.secret op: {err}"))?;
    let st_dict_update1 = builder
        .priv_op(Operation::dict_update(
            mid_state.dict.clone(),
            old_state.dict.clone(),
            "position",
            (&action, "position"),
        ))
        .map_err(|err| format!("failed dict_update1 op: {err}"))?;
    let st_dict_update2 = builder
        .priv_op(Operation::dict_update(
            new_state.dict.clone(),
            mid_state.dict.clone(),
            "secret",
            (&action, "secret"),
        ))
        .map_err(|err| format!("failed dict_update2 op: {err}"))?;
    let st_equal_action_type = builder
        .priv_op(Operation::eq((&action, "type"), "base"))
        .map_err(|err| format!("failed eq action.type op: {err}"))?;

    log("Loading predicate module...");
    let module = load_module(predicate_src, "copper_module", &params, &[])
        .map_err(|err| format!("failed to load custom predicate module: {err}"))?;
    let batch = module.batch.clone();
    let predicate = batch
        .predicate_ref_by_name("LightSwitch_base")
        .ok_or_else(|| "custom predicate LightSwitch_base not found".to_string())?;

    builder
        .pub_op(Operation::custom(
            predicate,
            [
                st_new_state_hash,
                st_equal_secret,
                st_dict_update1,
                st_dict_update2,
                st_equal_action_type,
            ],
        ))
        .map_err(|err| format!("failed custom op: {err}"))?;

    log("Proving pod (this is the slow step)...");
    let pod = builder
        .prove(prover)
        .map_err(|err| format!("pod proving failed: {err}"))?;
    log("Verifying pod...");
    pod.pod
        .verify()
        .map_err(|err| format!("pod verification failed: {err}"))?;
    log("Serializing pod...");
    serde_json::to_vec(&pod).map_err(|err| format!("failed to serialize pod: {err}"))
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

    let _ = app.emit(MINING_LOG_EVENT, "Queued mining job...".to_string());
    let app_handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut emit_log = |message: &str| emit_mining_log(&app_handle, message);
        emit_log("Mining started.");
        let pod_bytes = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_copper_pod_json(&mut emit_log)
        })) {
            Ok(result) => result?,
            Err(payload) => {
                let panic_message = panic_payload_to_string(payload);
                emit_log(&format!("PANIC during proving: {panic_message}"));
                return Err(format!("mining panicked: {panic_message}"));
            }
        };

        let objects_path = objects_dir(&app_handle)?;
        emit_log("Selecting output filename...");
        let id = short_hash_id(&pod_bytes);
        let mut file_name = format!("copper_{id}.pod");
        for nonce in 0..1000_u32 {
            let candidate = if nonce == 0 {
                file_name.clone()
            } else {
                format!("copper_{id}_{nonce}.pod")
            };
            if !objects_path.join(&candidate).exists() {
                file_name = candidate;
                break;
            }
        }
        if file_name.is_empty() {
            return Err("failed to allocate unique copper file id".to_string());
        }

        let file_path = objects_path.join(&file_name);
        emit_log("Writing .pod file...");
        fs::write(&file_path, pod_bytes)
            .map_err(|err| format!("failed to write pod file: {err}"))?;
        emit_log("Mining complete.");

        Ok(file_name)
    })
    .await;

    if let Ok(mut active) = mining_state.active.lock() {
        *active = false;
    }

    match outcome {
        Ok(result) => result,
        Err(err) => Err(format!("mining task failed: {err}")),
    }
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
