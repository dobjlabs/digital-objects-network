use std::{
    fs,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

fn executable() -> &'static Path {
    static DIRECTORY: OnceLock<tempfile::TempDir> = OnceLock::new();
    static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
    EXECUTABLE.get_or_init(|| {
        let directory = DIRECTORY.get_or_init(|| tempfile::tempdir().unwrap());
        let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/parent_lifetime.rs");
        let source = format!(
            "#[path = {module:?}]\nmod parent_lifetime;\n{}",
            include_str!("../tests/fixtures/lifetime.rs")
        );
        let source_path = directory.path().join("lifetime.rs");
        fs::write(&source_path, source).unwrap();
        let executable = directory
            .path()
            .join(format!("lifetime{}", std::env::consts::EXE_SUFFIX));
        assert!(
            Command::new("rustc")
                .arg("--edition=2024")
                .arg(source_path)
                .arg("-o")
                .arg(&executable)
                .status()
                .unwrap()
                .success()
        );
        executable
    })
}

struct Process(Child);

impl Process {
    fn spawn(directory: &Path, role: &str, stage: &str, port: u16) -> Self {
        Self(
            Command::new(executable())
                .arg(role)
                .arg(directory)
                .arg(stage)
                .arg(port.to_string())
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn kill(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn wait_for(mut condition: impl FnMut() -> bool, description: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "Timed out: {description}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_stage(directory: &Path, stage: &str) -> u16 {
    wait_for(
        || fs::read_to_string(directory.join("stage")).is_ok_and(|value| value == stage),
        "daemon stage",
    );
    fs::read_to_string(directory.join("port"))
        .unwrap()
        .parse()
        .unwrap()
}

fn parent_crash(stage: &str) {
    let directory = tempfile::tempdir().unwrap();
    let mut host = Process::spawn(directory.path(), "host", stage, 0);
    let port = wait_for_stage(directory.path(), stage);
    assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err());
    host.kill();
    wait_for(
        || TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok(),
        "orphan daemon exits after host crash",
    );

    fs::remove_file(directory.path().join("stage")).unwrap();
    let mut restarted = Process::spawn(directory.path(), "host", "ready", port);
    assert_eq!(wait_for_stage(directory.path(), "ready"), port);
    restarted.kill();
    wait_for(
        || TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok(),
        "restarted daemon exits after host crash",
    );
}

#[test]
fn host_crash_terminates_daemon_during_blocking_startup_and_allows_restart() {
    parent_crash("startup");
}

#[test]
fn host_crash_terminates_ready_daemon_and_allows_restart() {
    parent_crash("ready");
}

#[test]
fn standalone_daemon_does_not_exit_on_stdin_eof() {
    let directory = tempfile::tempdir().unwrap();
    let mut daemon = Process::spawn(directory.path(), "daemon", "ready", 0);
    let port = wait_for_stage(directory.path(), "ready");
    std::thread::sleep(Duration::from_millis(100));
    assert!(daemon.0.try_wait().unwrap().is_none());
    assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err());
}
