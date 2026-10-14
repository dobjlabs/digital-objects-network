use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use bitcraft_launchers::{Connection, connection_path};

fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let binary = directory.join(format!("tool{}", std::env::consts::EXE_SUFFIX));
        let status = Command::new("rustc")
            .arg("--edition=2024")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tool.rs"))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap();
        assert!(status.success());
        binary
    })
}

struct Package {
    directory: tempfile::TempDir,
}

impl Package {
    fn new() -> Self {
        let package = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        for (name, executable) in [
            ("bitcraft-dobj", env!("CARGO_BIN_EXE_bitcraft-dobj")),
            (
                "bitcraft-dobj-mcp-proxy",
                env!("CARGO_BIN_EXE_bitcraft-dobj-mcp-proxy"),
            ),
        ] {
            fs::copy(executable, package.binary(name)).unwrap();
            fs::copy(fixture(), package.binary(&format!("{name}-inner"))).unwrap();
        }
        package
    }

    fn binary(&self, name: &str) -> PathBuf {
        self.directory
            .path()
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    fn command(&self, name: &str, home: &Path) -> Command {
        let mut command = Command::new(self.binary(name));
        command
            .arg("--home")
            .arg(home)
            .env("DOBJD_URL", "http://127.0.0.1:7717")
            .env("DOBJ_HOME", "standalone-state")
            .env_remove("BITCRAFT_HOME");
        command
    }
}

fn daemon(home: &Path, identity: &str) -> (Connection, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let connection =
        Connection::new(home.join("active-state"), "bitcraft-instance".into(), port).unwrap();
    connection.publish(&connection_path(home)).unwrap();
    let body = serde_json::json!({ "ok": true, "instance_id": identity }).to_string();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "Wrapper did not verify the daemon"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("Health listener failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if stream.read(&mut byte).unwrap() == 0 {
                break;
            }
            request.push(byte[0]);
        }
        assert!(request.starts_with(b"GET /healthz "));
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    (connection, worker)
}

fn error(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn cli_forwards_arguments_and_active_state_and_preserves_exit_status() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let (connection, daemon) = daemon(home.path(), "bitcraft-instance");
    let output = package
        .command("bitcraft-dobj", home.path())
        .env("BITCRAFT_HOME", home.path().join("wrong-home"))
        .args(["--json", "inspect-object", "file with spaces.dobj"])
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", error(&output));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with(&format!(
            "--url\n{}\n--json\ninspect-object\nfile with spaces.dobj\n",
            connection.http_url
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("DOBJ_HOME={}\n", connection.state_root.display())),
        "{text}"
    );
    assert!(text.ends_with("DOBJD_URL=\n"), "{text}");
}

#[test]
fn proxy_preserves_stdio_and_child_exit_status() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let (_, daemon) = daemon(home.path(), "bitcraft-instance");
    let mut child = package
        .command("bitcraft-dobj-mcp-proxy", home.path())
        .env("TEST_ECHO_STDIO", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", error(&output));
    assert_eq!(output.stdout, input);
    assert_eq!(output.stderr, b"fixture stderr\n");
}

#[test]
fn proxy_selects_the_mcp_endpoint_from_the_connection_file() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let (connection, daemon) = daemon(home.path(), "bitcraft-instance");
    let output = package
        .command("bitcraft-dobj-mcp-proxy", home.path())
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", error(&output));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with(&format!("--url\n{}\n", connection.mcp_url))
    );
}

#[test]
fn rejects_an_unrelated_daemon_without_starting_the_tool() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let (_, daemon) = daemon(home.path(), "standalone-instance");
    let output = package
        .command("bitcraft-dobj", home.path())
        .arg("objects")
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        error(&output).contains("does not match"),
        "{}",
        error(&output)
    );
}

#[test]
fn missing_and_stale_connections_never_fall_back_to_standalone() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let output = package
        .command("bitcraft-dobj", home.path())
        .arg("objects")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        error(&output).contains("No active Bitcraft connection"),
        "{}",
        error(&output)
    );
    assert!(!connection_path(home.path()).exists());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let connection = Connection::new(
        home.path().to_path_buf(),
        "old-instance".into(),
        listener.local_addr().unwrap().port(),
    )
    .unwrap();
    drop(listener);
    connection.publish(&connection_path(home.path())).unwrap();
    let output = package
        .command("bitcraft-dobj-mcp-proxy", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        error(&output).contains("not reachable"),
        "{}",
        error(&output)
    );
}

#[test]
fn help_and_version_work_without_a_running_app() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    for name in ["bitcraft-dobj", "bitcraft-dobj-mcp-proxy"] {
        for argument in ["--help", "--version"] {
            let output = package
                .command(name, home.path())
                .arg(argument)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(23), "{}", error(&output));
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .starts_with(argument)
            );
        }
    }
}

#[test]
fn help_after_separator_does_not_bypass_connection_verification() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let output = package
        .command("bitcraft-dobj", home.path())
        .args(["inspect-object", "--", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        error(&output).contains("No active Bitcraft connection"),
        "{}",
        error(&output)
    );
}

#[test]
fn global_home_environment_is_shared_with_the_host() {
    let package = Package::new();
    let home = tempfile::tempdir().unwrap();
    let (connection, daemon) = daemon(home.path(), "bitcraft-instance");
    let output = Command::new(package.binary("bitcraft-dobj"))
        .env("BITCRAFT_HOME", home.path())
        .arg("objects")
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", error(&output));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with(&format!("--url\n{}\nobjects", connection.http_url))
    );
}
