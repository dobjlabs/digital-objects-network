//! Lifecycle subcommands: `dobj start | stop | status | logs`.
//!
//! These wrap dobjd in the same idioms as `redis-server --daemonize`,
//! `convex dev`, `supabase start` etc.: a CLI verb that handles detaching,
//! pidfile + logfile management, and health-checking. The user never has to
//! type `nohup`, juggle file paths, or remember kill commands.
//!
//! ## Layout
//!
//! - pidfile: `~/.dobj/dobjd.pid`
//! - log:     `~/.dobj/dobjd.log` (append; we don't rotate)
//!
//! ## Process detachment
//!
//! On Unix we spawn dobjd with `setsid()` in `pre_exec` so the child becomes
//! its own session leader and survives the launching terminal closing —
//! same effect as `nohup`. stdin is `/dev/null`; stdout/stderr go to the
//! log file. This is the same shape `redis-server` uses for `daemonize yes`.
//!
//! Windows isn't supported here. Add a `cfg(windows)` branch using
//! `CREATE_NEW_PROCESS_GROUP` + detached flags if needed.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::client::DobjdClient;

const READY_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Filesystem layout under `~/.dobj/`.
pub struct DaemonPaths {
    pub home: PathBuf,
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
}

impl DaemonPaths {
    pub fn resolve() -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow!("could not resolve home directory"))?
            .join(".dobj");
        fs::create_dir_all(&home)
            .with_context(|| format!("failed to create {}", home.display()))?;
        Ok(Self {
            pid_file: home.join("dobjd.pid"),
            log_file: home.join("dobjd.log"),
            home,
        })
    }
}

/// Locate the dobjd binary.
///
/// Resolution order:
/// 1. `$DOBJD_BIN` env var (explicit override)
/// 2. `~/.dobj/bin/dobjd` (where SKILL.md installs it)
/// 3. `dobjd` next to the running `dobj` binary (works for `cargo install`)
/// 4. Bare `dobjd` — let `Command::new` resolve via `$PATH`
fn find_dobjd_binary(paths: &DaemonPaths) -> OsString {
    if let Some(explicit) = std::env::var_os("DOBJD_BIN") {
        return explicit;
    }
    let in_dobj_home = paths.home.join("bin").join("dobjd");
    if in_dobj_home.exists() {
        return in_dobj_home.into_os_string();
    }
    if let Ok(self_exe) = std::env::current_exe()
        && let Some(dir) = self_exe.parent()
    {
        let sibling = dir.join("dobjd");
        if sibling.exists() {
            return sibling.into_os_string();
        }
    }
    OsString::from("dobjd")
}

/// Read a pid from `pid_file`, returning `None` if the file is missing or
/// malformed (treated the same — "no record of a running daemon").
fn read_pidfile(pid_file: &Path) -> Option<i32> {
    let contents = fs::read_to_string(pid_file).ok()?;
    contents.trim().parse::<i32>().ok()
}

/// Is a process with `pid` alive? Sends signal 0 — POSIX-defined to do no
/// work, just check existence + permissions.
fn process_alive(pid: i32) -> bool {
    // Safety: kill(pid, 0) is signal-free; only checks the target process.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn signal(pid: i32, sig: libc::c_int) -> Result<()> {
    let rc = unsafe { libc::kill(pid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).with_context(|| format!("kill({pid}, {sig}) failed"))
    }
}

/// How long a single liveness GET is allowed to hang. Short — we'd rather
/// fail fast and retry on the next loop tick than block forever on a
/// listener that ate the connection but never replied.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Probe whether dobjd's HTTP listener is up.
///
/// Uses `/objects/dir` because it's the cheapest local endpoint —
/// `/inventory` and `/state-root` round-trip to the synchronizer and can
/// hang for 30s when it's unreachable, which is useless for a liveness
/// probe. We also wrap the request in a per-call timeout so a stuck server
/// can't hang the loop forever.
async fn http_alive(client: &DobjdClient) -> bool {
    let url = format!("{}/objects/dir", client.base_url());
    reqwest::Client::new()
        .get(&url)
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Block until the HTTP API responds, the process dies, or the timeout
/// elapses. Prints a dot every couple seconds so the user knows we're
/// still working — cold start can take 15-30s while plugins compile and
/// RocksDB initializes.
async fn wait_until_ready(client: &DobjdClient, pid: i32, timeout: Duration) -> Result<()> {
    use std::io::Write as _;

    let start = Instant::now();
    let deadline = start + timeout;
    let mut last_dot = start;
    let mut printed_dots = false;

    loop {
        if !process_alive(pid) {
            if printed_dots {
                println!();
            }
            bail!("dobjd exited before becoming ready (check `dobj logs`)");
        }
        if http_alive(client).await {
            if printed_dots {
                println!();
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            if printed_dots {
                println!();
            }
            bail!("dobjd did not become ready within {}s", timeout.as_secs());
        }
        if last_dot.elapsed() >= Duration::from_secs(2) {
            print!(".");
            let _ = std::io::stdout().flush();
            last_dot = Instant::now();
            printed_dots = true;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

pub async fn start(client: &DobjdClient) -> Result<()> {
    let paths = DaemonPaths::resolve()?;

    // Already running? Idempotent: succeed without spawning a duplicate.
    if let Some(pid) = read_pidfile(&paths.pid_file) {
        if process_alive(pid) {
            // Confirm via HTTP — pidfile could be stale from a different binary.
            if http_alive(client).await {
                println!("dobjd already running (pid {pid})");
                return Ok(());
            }
        }
        // Stale pidfile (process gone or unreachable). Wipe it and start fresh.
        let _ = fs::remove_file(&paths.pid_file);
    }

    // Open log file for append; child inherits as stdout/stderr.
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_file)
        .with_context(|| format!("failed to open {}", paths.log_file.display()))?;
    let log_clone = log.try_clone()?;

    let bin = find_dobjd_binary(&paths);
    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_clone));

    // Detach from the launching terminal. Without this, `dobj start` in a
    // terminal would die when the terminal closes.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // Become a new session leader, decoupled from the parent's
            // controlling terminal. Idiomatic for a daemon.
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn().with_context(|| {
        format!(
            "failed to spawn dobjd ({}). Set $DOBJD_BIN or install dobjd to ~/.dobj/bin/.",
            Path::new(&bin).display()
        )
    })?;

    let pid = child.id() as i32;
    fs::write(&paths.pid_file, pid.to_string())
        .with_context(|| format!("failed to write {}", paths.pid_file.display()))?;

    // Don't reap the child — we want it to keep running after we exit.
    std::mem::forget(child);

    println!(
        "starting dobjd (pid {pid}, log {})…",
        paths.log_file.display()
    );
    wait_until_ready(client, pid, READY_TIMEOUT).await?;
    println!("dobjd is up");
    Ok(())
}

pub async fn stop() -> Result<()> {
    let paths = DaemonPaths::resolve()?;
    let Some(pid) = read_pidfile(&paths.pid_file) else {
        println!("dobjd is not running (no pidfile)");
        return Ok(());
    };

    if !process_alive(pid) {
        println!("dobjd is not running (stale pidfile, removed)");
        let _ = fs::remove_file(&paths.pid_file);
        return Ok(());
    }

    // Polite first: SIGTERM and wait.
    signal(pid, libc::SIGTERM).with_context(|| format!("failed to SIGTERM pid {pid}"))?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            let _ = fs::remove_file(&paths.pid_file);
            println!("dobjd stopped");
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Didn't exit in time — force.
    eprintln!("dobjd did not exit after SIGTERM; sending SIGKILL");
    signal(pid, libc::SIGKILL).with_context(|| format!("failed to SIGKILL pid {pid}"))?;
    let _ = fs::remove_file(&paths.pid_file);
    println!("dobjd killed");
    Ok(())
}

pub async fn status(client: &DobjdClient) -> Result<()> {
    let paths = DaemonPaths::resolve()?;
    let pid = read_pidfile(&paths.pid_file);

    let alive = pid.map(process_alive).unwrap_or(false);
    let healthy = http_alive(client).await;

    match (pid, alive, healthy) {
        (Some(pid), true, true) => {
            println!("dobjd is running (pid {pid})");
            println!("  http: {} ✓", client.base_url());
            println!("  log:  {}", paths.log_file.display());
        }
        (Some(pid), true, false) => {
            println!("dobjd process is alive (pid {pid}) but not responding on HTTP");
            println!("  expected: {}", client.base_url());
            println!("  log:      {}", paths.log_file.display());
        }
        (Some(pid), false, _) => {
            println!("dobjd is not running (pidfile points at dead pid {pid})");
            println!("  log: {}", paths.log_file.display());
        }
        (None, _, true) => {
            println!("dobjd is running but not under our pidfile");
            println!("  http: {} ✓", client.base_url());
        }
        (None, _, false) => {
            println!("dobjd is not running");
            println!("  start with: dobj start");
        }
    }
    Ok(())
}

pub fn logs(follow: bool, lines: usize) -> Result<()> {
    let paths = DaemonPaths::resolve()?;
    if !paths.log_file.exists() {
        eprintln!("no log file yet at {}", paths.log_file.display());
        return Ok(());
    }

    // Print the tail of the file once.
    let tail = read_tail(&paths.log_file, lines)?;
    print!("{tail}");

    if !follow {
        return Ok(());
    }

    // Follow new lines. We re-open and seek to the end to avoid re-printing
    // what we just emitted; further reads stream as the file grows.
    let mut file = File::open(&paths.log_file)?;
    let mut pos = file.metadata()?.len();
    file.seek(SeekFrom::Start(pos))?;

    loop {
        let new_len = file.metadata()?.len();
        if new_len < pos {
            // File truncated / rotated. Re-open from the start.
            pos = 0;
            file = File::open(&paths.log_file)?;
        }
        if new_len > pos {
            file.seek(SeekFrom::Start(pos))?;
            let mut reader = BufReader::new(&file);
            let mut buf = String::new();
            while reader.read_line(&mut buf)? > 0 {
                print!("{buf}");
                buf.clear();
            }
            pos = new_len;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Read the last `n` lines of `path`. Naive for small log files; the dobjd
/// log isn't expected to grow beyond a few MB before the user rotates it
/// manually, so a single full read is fine.
fn read_tail(path: &Path, n: usize) -> Result<String> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut lines: Vec<&str> = contents.lines().collect();
    if lines.len() > n {
        lines.drain(0..lines.len() - n);
    }
    let mut out = lines.join("\n");
    if !contents.is_empty() && !contents.ends_with('\n') {
        // Preserve original trailing-newline state.
    } else if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}
