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
//! On Windows we spawn with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
//! creation flags. DETACHED_PROCESS prevents the child from inheriting our
//! console (so closing the parent terminal won't take dobjd down with it);
//! CREATE_NEW_PROCESS_GROUP isolates the child from Ctrl+C/Break events
//! routed to our group. stdio handles still attach to the log file the same
//! way — DETACHED_PROCESS only suppresses console inheritance, not file
//! handles. Note: Windows has no equivalent of SIGTERM's "ask politely"
//! semantics, so `dobj stop` reaches straight for `TerminateProcess` (hard
//! kill). The graceful timeout still runs but in practice exits on the first
//! poll iteration since TerminateProcess is synchronous.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use wire_types::DriverSettings;

use crate::client::DobjdClient;

// First-ever start compiles the pod2/plonky2 circuits and writes them to the
// disk cache before the HTTP listener binds — a one-time cost that's run on
// every fresh machine (and every CI runner, which always starts cold). On a
// modest/Windows box this comfortably exceeds a minute, so the gate has to be
// generous; subsequent starts hit the cache and come up in seconds. The wait
// loop early-exits the instant the process dies, so a genuinely-crashed dobjd
// still fails fast — this ceiling only applies while it's alive but not yet
// serving (i.e. still building circuits).
const READY_TIMEOUT: Duration = Duration::from_secs(300);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Filesystem layout under `~/.dobj/`.
pub struct DaemonPaths {
    pub home: PathBuf,
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
}

/// Resolve `~/.dobj` (without creating it). The single source for the dobj
/// home directory, shared by the daemon lifecycle here and the updater
/// (`crate::update`) so the launcher and the installer can't disagree on where
/// the install lives.
pub(crate) fn dobj_home() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("could not resolve home directory"))?
        .join(".dobj"))
}

impl DaemonPaths {
    pub fn resolve() -> Result<Self> {
        let home = dobj_home()?;
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
/// 2. `~/.dobj/bin/dobjd[.exe]` (where SKILL.md installs it)
/// 3. `dobjd[.exe]` next to the running `dobj` binary (works for `cargo install`)
/// 4. Bare `dobjd[.exe]` — let `Command::new` resolve via `$PATH`
///
/// `EXE_SUFFIX` resolves to `".exe"` on Windows and `""` on Unix, so a
/// single string drives both platforms.
fn find_dobjd_binary(paths: &DaemonPaths) -> OsString {
    if let Some(explicit) = std::env::var_os("DOBJD_BIN") {
        return explicit;
    }
    let exe_name = format!("dobjd{}", std::env::consts::EXE_SUFFIX);
    let in_dobj_home = paths.home.join("bin").join(&exe_name);
    if in_dobj_home.exists() {
        return in_dobj_home.into_os_string();
    }
    if let Ok(self_exe) = std::env::current_exe()
        && let Some(dir) = self_exe.parent()
    {
        let sibling = dir.join(&exe_name);
        if sibling.exists() {
            return sibling.into_os_string();
        }
    }
    OsString::from(exe_name)
}

/// Read a pid from `pid_file`, returning `None` if the file is missing or
/// malformed (treated the same — "no record of a running daemon").
fn read_pidfile(pid_file: &Path) -> Option<i32> {
    let contents = fs::read_to_string(pid_file).ok()?;
    contents.trim().parse::<i32>().ok()
}

/// Is a process with `pid` alive?
///
/// Unix: sends signal 0 — POSIX-defined to do no work, just check existence
/// + permissions.
///
/// Windows: tries to open the process handle with the minimum-rights flag
/// (`PROCESS_QUERY_LIMITED_INFORMATION`). A null handle means the pid is
/// dead or inaccessible; for pids we ourselves spawned, permission isn't
/// the issue, so non-null ≈ alive.
fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        // Safety: kill(pid, 0) is signal-free; only checks the target process.
        unsafe { libc::kill(pid, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // Safety: OpenProcess returns a null handle on failure (which we
        // treat as "not alive"); on success we close the handle immediately
        // so it can't leak.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                false
            } else {
                CloseHandle(handle);
                true
            }
        }
    }
}

/// Ask `pid` to exit gracefully.
///
/// Unix: SIGTERM. Caller polls `process_alive` until either the process
/// exits or the STOP_TIMEOUT elapses, then escalates to `terminate_force`.
///
/// Windows: no graceful equivalent exists, so this is just a hard kill via
/// `TerminateProcess`. The caller's polling loop still runs but exits on
/// iteration 1 because TerminateProcess is synchronous.
fn terminate_graceful(pid: i32) -> Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
                .with_context(|| format!("kill({pid}, SIGTERM) failed"))
        }
    }
    #[cfg(windows)]
    {
        terminate_force(pid)
    }
}

/// Force-terminate `pid`. Unix: SIGKILL. Windows: `TerminateProcess`.
fn terminate_force(pid: i32) -> Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
                .with_context(|| format!("kill({pid}, SIGKILL) failed"))
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        // Safety: OpenProcess + TerminateProcess + CloseHandle is the
        // standard 3-step Win32 pattern; we close on every exit path so the
        // handle can't leak.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
            if handle.is_null() {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("OpenProcess({pid}) failed"));
            }
            let rc = TerminateProcess(handle, 1);
            CloseHandle(handle);
            if rc == 0 {
                Err(std::io::Error::last_os_error())
                    .with_context(|| format!("TerminateProcess({pid}) failed"))
            } else {
                Ok(())
            }
        }
    }
}

/// How long a single liveness GET is allowed to hang. Short — we'd rather
/// fail fast and retry on the next loop tick than block forever on a
/// listener that ate the connection but never replied.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn dobjd_port_from_url(url: &str) -> Result<u16> {
    let parsed = reqwest::Url::parse(url).with_context(|| format!("invalid dobjd URL: {url}"))?;
    parsed
        .port_or_known_default()
        .ok_or_else(|| anyhow!("dobjd URL must include a port: {url}"))
}

fn mcp_port_for_http_port(port: u16) -> Result<u16> {
    port.checked_add(1)
        .ok_or_else(|| anyhow!("DOBJD_PORT={port} cannot derive an adjacent MCP port"))
}

/// Probe whether dobjd is operable.
///
/// Hits `/healthz` rather than just any cheap endpoint: a daemon whose
/// HTTP listener bound but whose plugin catalog failed to load is *not*
/// ready to serve, and `dobj start`'s readiness gate should reflect that.
/// `/healthz` returns 503 in those cases. Network-free on the server side
/// (no synchronizer round-trip) so a slow synchronizer doesn't break the
/// probe — that's a separate failure mode.
///
/// A per-call timeout caps each probe so a stuck server can't hang the
/// polling loop forever.
pub(crate) async fn http_alive(client: &DobjdClient) -> bool {
    let url = format!("{}/healthz", client.base_url());
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
/// still working — a first-ever start builds the pod2/plonky2 circuits and
/// can take a few minutes on a cold machine (see `READY_TIMEOUT`); cached
/// starts are seconds.
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
    start_dobjd(client, None).await
}

/// Start a specific dobjd binary, bypassing the `$DOBJD_BIN` / PATH resolution
/// in `find_dobjd_binary`. The updater uses this so it restarts exactly the
/// binary it just installed and validated under `~/.dobj/bin`, rather than
/// whatever `$DOBJD_BIN` happens to point at.
pub async fn start_binary(client: &DobjdClient, dobjd_bin: &Path) -> Result<()> {
    start_dobjd(client, Some(dobjd_bin.as_os_str().to_os_string())).await
}

async fn start_dobjd(client: &DobjdClient, dobjd_bin: Option<OsString>) -> Result<()> {
    let paths = DaemonPaths::resolve()?;
    let dobjd_port = dobjd_port_from_url(client.base_url())?;
    let mcp_port = mcp_port_for_http_port(dobjd_port)?;

    // Already running? Probe HTTP first — some other process may own the port
    // (a `cargo run -p dobjd`, a previous `dobj start` whose pidfile was
    // wiped, etc.) and if we don't catch that here, we'd spawn a duplicate
    // that immediately dies on EADDRINUSE while wait_until_ready happily
    // sees the *original* dobjd answering and reports success — leaving a
    // pidfile pointing at a corpse.
    if http_alive(client).await {
        match read_pidfile(&paths.pid_file) {
            Some(pid) if process_alive(pid) => {
                println!("dobjd already running (pid {pid})");
            }
            _ => {
                println!(
                    "dobjd already running on {} (not under our pidfile)",
                    client.base_url()
                );
            }
        }
        return Ok(());
    }

    // Nothing answering on the requested port. If a pidfile exists, it's stale.
    if paths.pid_file.exists() {
        let _ = fs::remove_file(&paths.pid_file);
    }

    // Open log file for append; child inherits as stdout/stderr.
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_file)
        .with_context(|| format!("failed to open {}", paths.log_file.display()))?;
    let log_clone = log.try_clone()?;

    let bin = dobjd_bin.unwrap_or_else(|| find_dobjd_binary(&paths));
    let mut cmd = Command::new(&bin);
    cmd.env("DOBJD_PORT", dobjd_port.to_string());
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        // DETACHED_PROCESS: child gets no inherited console, so closing
        // our terminal doesn't propagate. CREATE_NEW_PROCESS_GROUP: child
        // is in its own process group, immune to Ctrl+C/Break routed to
        // ours. Stdio file handles still pass through normally.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
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

    // Drop the Child without reaping. Rust's Child only kills on an explicit
    // `.kill()` (not on drop), so the spawned process keeps running after we
    // exit. The pidfile is what later `dobj stop` / `status` use to reach it.
    drop(child);

    println!(
        "starting dobjd (pid {pid}, http {}, log {})…",
        client.base_url(),
        paths.log_file.display(),
    );
    wait_until_ready(client, pid, READY_TIMEOUT).await?;

    // MCP is off by default and toggled via settings, so report what the
    // daemon actually serves rather than assuming the adjacent port is live.
    // dobjd binds (or skips) the MCP port before it answers HTTP, so by the
    // time we're ready `/settings` reflects the running state.
    match client.get_json::<DriverSettings>("/settings").await {
        Ok(settings) if settings.mcp_enabled => {
            println!("dobjd is up (mcp http://127.0.0.1:{mcp_port}/mcp)");
        }
        Ok(_) => {
            println!("dobjd is up (mcp disabled; enable with `dobj settings set --mcp on`)");
        }
        Err(_) => println!("dobjd is up"),
    }
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

    // Polite first: SIGTERM (Unix) / TerminateProcess (Windows). On Windows
    // there's no graceful equivalent so this is already the hard kill — the
    // poll loop below exits on iteration 1.
    terminate_graceful(pid).with_context(|| format!("failed to terminate pid {pid}"))?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            let _ = fs::remove_file(&paths.pid_file);
            println!("dobjd stopped");
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Didn't exit in time — force. Unreachable on Windows in practice
    // (TerminateProcess already happened above) but harmless.
    eprintln!("dobjd did not exit after stop signal; forcing kill");
    terminate_force(pid).with_context(|| format!("failed to force-kill pid {pid}"))?;
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

pub async fn logs(follow: bool, lines: usize) -> Result<()> {
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
    //
    // The loop is sync std I/O (a stat + a small read every 250ms is fine
    // here) but the sleep yields back to the runtime so we don't hold a
    // worker thread across the whole follow session.
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
        tokio::time::sleep(Duration::from_millis(250)).await;
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
    // Preserve the original file's trailing-newline state.
    if !out.is_empty() && contents.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_dobjd_port_from_url() {
        let port = dobjd_port_from_url("http://127.0.0.1:7727").unwrap();
        assert_eq!(port, 7727);
    }

    #[test]
    fn derives_mcp_port_from_http_port() {
        assert_eq!(mcp_port_for_http_port(7727).unwrap(), 7728);
    }

    #[test]
    fn rejects_mcp_port_overflow() {
        assert!(mcp_port_for_http_port(u16::MAX).is_err());
    }
}
