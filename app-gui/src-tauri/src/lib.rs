use std::sync::Mutex;
use sysinfo::{Pid, ProcessesToUpdate, System};

struct CpuMonitor {
    pid: Pid,
    system: Mutex<System>,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(CpuMonitor::new())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![sample_app_cpu])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
