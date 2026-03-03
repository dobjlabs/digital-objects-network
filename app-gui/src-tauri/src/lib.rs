use serde::{Deserialize, Serialize};
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::Manager;

#[derive(Default)]
struct AppState {
    posts: Mutex<Vec<PostDto>>,
    next_id: AtomicU64,
}

struct CpuMonitor {
    pid: Pid,
    system: Mutex<System>,
    total_cpu_secs: Mutex<f64>,
    last_sample_at: Mutex<Option<Instant>>,
    total_loaded: Mutex<bool>,
}

impl CpuMonitor {
    fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        let mut system = System::new_all();
        let _ = system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        Self {
            pid,
            system: Mutex::new(system),
            total_cpu_secs: Mutex::new(0.0),
            last_sample_at: Mutex::new(None),
            total_loaded: Mutex::new(false),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProofClaimDto {
    name: String,
    validity: String,
    hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PostDto {
    id: String,
    title: String,
    peer: String,
    time: String,
    desc: String,
    proofs: Vec<ProofClaimDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MockStateDto {
    post_count: usize,
    supported_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunMethodInput {
    id: String,
    method_name: String,
    input_files: Vec<String>,
    cpu_cost: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProofRunResult {
    success: bool,
    method_name: String,
    old_root: String,
    new_root: String,
    stage_messages: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyPostInput {
    post_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResult {
    post_id: String,
    status: String,
    checked_block: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePostInput {
    title: String,
    desc: String,
    proof_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondPostInput {
    post_id: String,
    desc: String,
    proof_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachClaimInput {
    file_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachClaimResult {
    name: String,
    validity: String,
    hash: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericActionResult {
    ok: bool,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CpuSampleDto {
    usage_pct: f32,
    total_cpu_secs: f64,
}

fn cpu_stats_file_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve app data dir: {err}"))?;
    fs::create_dir_all(&base).map_err(|err| format!("failed to create app data dir: {err}"))?;
    Ok(base.join("cpu_stats.json"))
}

fn load_total_cpu_secs(app: &tauri::AppHandle) -> Result<f64, String> {
    let path = cpu_stats_file_path(app)?;
    if !path.exists() {
        return Ok(0.0);
    }

    let contents =
        fs::read_to_string(&path).map_err(|err| format!("failed to read cpu stats file: {err}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&contents).map_err(|err| format!("failed to parse cpu stats file: {err}"))?;
    Ok(parsed
        .get("totalCpuSecs")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0))
}

fn save_total_cpu_secs(app: &tauri::AppHandle, total: f64) -> Result<(), String> {
    let path = cpu_stats_file_path(app)?;
    let payload = serde_json::json!({ "totalCpuSecs": total });
    let serialized = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize cpu stats: {err}"))?;
    fs::write(&path, serialized).map_err(|err| format!("failed to write cpu stats file: {err}"))?;
    Ok(())
}

#[tauri::command]
fn get_things_dir(app: tauri::AppHandle) -> Result<String, String> {
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
fn ensure_things_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = get_things_dir(app)?;
    fs::create_dir_all(&dir).map_err(|err| format!("failed to create things dir: {err}"))?;
    Ok(dir)
}

#[tauri::command]
fn open_things_dir(app: tauri::AppHandle) -> Result<String, String> {
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

fn new_id(state: &AppState) -> String {
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    format!("post-{id}")
}

fn now_label() -> String {
    "mock-now".to_string()
}

fn fake_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!("0x{:02x}{:02x}...{:02x}{:02x}", bytes[0], bytes[1], bytes[6], bytes[7])
}

#[tauri::command]
fn get_mock_state(state: tauri::State<'_, AppState>) -> MockStateDto {
    let post_count = state.posts.lock().map(|posts| posts.len()).unwrap_or(0);
    MockStateDto {
        post_count,
        supported_methods: vec![
            "extract".to_string(),
            "feed".to_string(),
            "transfer".to_string(),
            "mint".to_string(),
        ],
    }
}

#[tauri::command]
fn run_method(input: RunMethodInput) -> ProofRunResult {
    let seed = format!(
        "{}-{}-{}-{}",
        input.id,
        input.method_name,
        input.input_files.join(","),
        input.cpu_cost
    );
    let old_root = fake_hash(&format!("{seed}-old"));
    let new_root = fake_hash(&format!("{seed}-new"));
    ProofRunResult {
        success: true,
        method_name: input.method_name,
        old_root: old_root.clone(),
        new_root: new_root.clone(),
        stage_messages: vec![
            format!("Generating recursive proof for {}", old_root),
            "Nullifying old state root".to_string(),
            format!("Committing new state root {}", new_root),
        ],
    }
}

#[tauri::command]
fn verify_post_proofs(input: VerifyPostInput) -> VerifyResult {
    VerifyResult {
        post_id: input.post_id,
        status: "verified".to_string(),
        checked_block: "18,442,731".to_string(),
    }
}

#[tauri::command]
fn create_post(
    state: tauri::State<'_, AppState>,
    input: CreatePostInput,
) -> Result<PostDto, String> {
    let id = new_id(&state);
    let post = PostDto {
        id,
        title: input.title,
        peer: "127.0.0.1".to_string(),
        time: now_label(),
        desc: input.desc,
        proofs: input
            .proof_names
            .iter()
            .map(|name| ProofClaimDto {
                name: name.clone(),
                validity: "live".to_string(),
                hash: fake_hash(name),
            })
            .collect(),
    };
    let mut posts = state
        .posts
        .lock()
        .map_err(|_| "failed to acquire post state lock".to_string())?;
    posts.push(post.clone());
    Ok(post)
}

#[tauri::command]
fn respond_post(
    state: tauri::State<'_, AppState>,
    input: RespondPostInput,
) -> Result<GenericActionResult, String> {
    let posts = state
        .posts
        .lock()
        .map_err(|_| "failed to acquire post state lock".to_string())?;
    let target_exists = posts.iter().any(|post| post.id == input.post_id);
    drop(posts);

    if !target_exists {
        return Err(format!("post {} not found", input.post_id));
    }

    Ok(GenericActionResult {
        ok: true,
        message: format!(
            "mock response accepted ({} proofs attached): {}",
            input.proof_names.len(),
            input.desc
        ),
    })
}

#[tauri::command]
fn attach_claim(input: AttachClaimInput) -> AttachClaimResult {
    let name = input.file_name.trim_end_matches(".dobj").to_string();
    AttachClaimResult {
        name: name.clone(),
        validity: "live".to_string(),
        hash: fake_hash(&name),
    }
}

#[tauri::command]
fn sample_app_cpu(app: tauri::AppHandle, monitor: tauri::State<'_, CpuMonitor>) -> CpuSampleDto {
    let mut system = match monitor.system.lock() {
        Ok(system) => system,
        Err(_) => {
            return CpuSampleDto {
                usage_pct: 0.0,
                total_cpu_secs: 0.0,
            }
        }
    };

    let mut loaded = match monitor.total_loaded.lock() {
        Ok(loaded) => loaded,
        Err(_) => {
            return CpuSampleDto {
                usage_pct: 0.0,
                total_cpu_secs: 0.0,
            }
        }
    };
    let mut total_cpu_secs = match monitor.total_cpu_secs.lock() {
        Ok(total) => total,
        Err(_) => {
            return CpuSampleDto {
                usage_pct: 0.0,
                total_cpu_secs: 0.0,
            }
        }
    };
    let mut last_sample_at = match monitor.last_sample_at.lock() {
        Ok(last) => last,
        Err(_) => {
            return CpuSampleDto {
                usage_pct: 0.0,
                total_cpu_secs: *total_cpu_secs,
            }
        }
    };

    if !*loaded {
        if let Ok(total) = load_total_cpu_secs(&app) {
            *total_cpu_secs = total.max(0.0);
        }
        *loaded = true;
    }

    let _ = system.refresh_processes(ProcessesToUpdate::Some(&[monitor.pid]), true);

    let raw_cpu = system
        .process(monitor.pid)
        .map(|process| process.cpu_usage())
        .unwrap_or(0.0);
    let cpu_count = system.cpus().len().max(1) as f32;
    let usage_pct = (raw_cpu / cpu_count).max(0.0);

    let now = Instant::now();
    if let Some(prev) = *last_sample_at {
        let dt_secs = (now - prev).as_secs_f64();
        // usage_pct is normalized 0..100 for this app; convert to core-seconds.
        *total_cpu_secs += (usage_pct as f64 / 100.0) * dt_secs;
    }
    *last_sample_at = Some(now);

    if *total_cpu_secs < 0.0 {
        *total_cpu_secs = 0.0;
    }
    let clamped_total = *total_cpu_secs;
    let _ = save_total_cpu_secs(&app, clamped_total);

    CpuSampleDto {
        usage_pct,
        total_cpu_secs: clamped_total,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .manage(CpuMonitor::new())
        .invoke_handler(tauri::generate_handler![
            get_things_dir,
            ensure_things_dir,
            open_things_dir,
            get_mock_state,
            sample_app_cpu,
            run_method,
            verify_post_proofs,
            create_post,
            respond_post,
            attach_claim
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
