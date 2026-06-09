//! `dobj update`: replace the installed bundle (dobj, dobjd,
//! bitcraft-mcp-proxy) with another release from the public releases repo.
//!
//! The bundle updates as a unit: every binary comes from one release tag and
//! the swap is all-or-nothing (two-phase rename with rollback). Plugins under
//! `~/.dobj/actions/` are user state and are deliberately not touched.
//!
//! Pipeline: guard -> discover -> download + stage -> stop daemon -> swap ->
//! validate -> restart -> verify (rollback on failure). The running `dobj`
//! process survives its own file being renamed on every platform: Unix keeps
//! the old inode mapped, and Windows allows renaming (not deleting) a running
//! executable.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::client::DobjdClient;
use crate::daemon;

const RELEASES_REPO: &str = "dobjlabs/zk-craft-releases";

/// Release tag this binary was stamped with ("dev" outside a release build).
const CURRENT_TAG: &str = env!("DOBJ_RELEASE_TAG");
/// Compile-time target triple. Artifact selection must use this rather than
/// runtime detection: an x86_64 build running under Rosetta would otherwise
/// "update" itself into a different architecture.
const TARGET_TRIPLE: &str = env!("DOBJ_TARGET_TRIPLE");

/// Tarballs in a release and the binaries each must contain, in swap order.
/// `dobj` is last: the update is orchestrated by the running `dobj`, and
/// keeping its on-disk file consistent with the executing code until
/// everything else has landed keeps rollback reasoning simple.
const ARTIFACTS: &[(&str, &[&str])] = &[
    ("dobjd", &["dobjd", "bitcraft-mcp-proxy"]),
    ("dobj", &["dobj"]),
];

pub async fn run(
    client: &DobjdClient,
    check: bool,
    version: Option<String>,
    force: bool,
) -> Result<()> {
    // `--check` is a read-only status query: resolving + comparing happen
    // inside report, which bounds the network fetch and falls back to the
    // cache, so it always returns promptly even on a poor or absent
    // connection. Handle it (and the dev-build guard) before the resolve below.
    if check {
        return report(client, version).await;
    }

    if CURRENT_TAG == "dev" {
        bail!("this is a dev build; update it by rebuilding, not via dobj update");
    }

    // The user explicitly asked to update and will sit through the download
    // anyway, so this resolve is bounded only against a fully stalled
    // connection (RESOLVE_TIMEOUT), not a merely slow one.
    let target_tag = match version {
        Some(tag) => tag,
        None => fetch_latest_tag(RESOLVE_TIMEOUT).await.context(
            "could not reach the release host to resolve the latest version; \
             check your connection and retry",
        )?,
    };
    // The tag becomes a path component below (staging dir, deleted on each
    // attempt) and a release-download URL, so it must be confirmed safe before
    // either use: a `--version ../..` or absolute path would otherwise let
    // remove_dir_all escape ~/.dobj.
    validate_release_tag(&target_tag)?;

    match compare_tags(CURRENT_TAG, &target_tag) {
        Ordering::Newer => {}
        Ordering::Same => {
            if !force {
                println!("already up to date ({CURRENT_TAG})");
                return Ok(());
            }
            println!("reinstalling {CURRENT_TAG} (--force)");
        }
        Ordering::Older => {
            if !force {
                bail!(
                    "{target_tag} is older than the installed {CURRENT_TAG}; pass --force to downgrade"
                );
            }
            println!("downgrading {CURRENT_TAG} -> {target_tag} (--force)");
        }
        Ordering::Unknown => {
            if !force {
                bail!(
                    "cannot order {target_tag} against installed {CURRENT_TAG}; pass --force to install it anyway"
                );
            }
        }
    }

    let bin_dir = ensure_managed_install()?;
    // Restart the binary we install and validate under ~/.dobj/bin, not
    // whatever $DOBJD_BIN might point at (see daemon::start_binary).
    let dobjd_bin = bin_dir.join(format!("dobjd{}", std::env::consts::EXE_SUFFIX));

    println!("updating {CURRENT_TAG} -> {target_tag}");
    let staging = daemon::dobj_home()?.join("staging").join(&target_tag);
    let staged = download_and_stage(&target_tag, &staging).await?;

    let was_running = daemon::http_alive(client).await;
    if was_running {
        daemon::stop().await?;
    }

    let journal = match swap_binaries(&bin_dir, &staged) {
        Ok(journal) => journal,
        Err(err) => {
            // swap_binaries already rolled back what it had done.
            return Err(err.context("binary swap failed; previous version left in place"));
        }
    };

    if let Err(err) = validate_installed(&bin_dir, &target_tag) {
        rollback(&journal);
        if was_running {
            let _ = daemon::start_binary(client, &dobjd_bin).await;
        }
        return Err(err.context("new binaries failed validation; rolled back"));
    }

    if was_running {
        println!(
            "restarting dobjd (a new version may rebuild proving circuits - this can take a few minutes)"
        );
        if let Err(err) = daemon::start_binary(client, &dobjd_bin).await {
            // The new dobjd may have bound its ports but failed to become
            // healthy (e.g. a wedged startup), and start leaves that process
            // running - its pid is in the pidfile it wrote before the readiness
            // wait. Stop it before rolling back, or the recovery start below
            // inherits an occupied port and can't rebind, leaving a broken
            // daemon serving while we report a clean rollback.
            let _ = daemon::stop().await;
            rollback(&journal);
            let restored = daemon::start_binary(client, &dobjd_bin).await.is_ok();
            bail!(
                "updated dobjd failed to become healthy ({err:#}); rolled back to {CURRENT_TAG}{}",
                if restored {
                    ""
                } else {
                    " but the daemon did not restart - check `dobj logs`"
                }
            );
        }
        match daemon_version(client).await {
            Some(v) if v == target_tag => {}
            Some(v) => println!(
                "warning: daemon reports {v}, expected {target_tag} - another dobjd may own the port"
            ),
            None => println!("warning: daemon is healthy but reports no version"),
        }
    }

    let _ = fs::remove_dir_all(&staging);
    println!(
        "updated {CURRENT_TAG} -> {target_tag} (dobj, dobjd, bitcraft-mcp-proxy){}",
        if was_running {
            ""
        } else {
            "; daemon was not running, start with `dobj start`"
        }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// discovery + version ordering

/// Generous bound for resolving the latest tag when actually updating: the
/// user opted in and the download dwarfs this, so it only guards against a
/// fully stalled connection, not a merely slow one.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
/// Tighter bound for the interactive `dobj update --check` - a little longer
/// than the silent startup notice since the user is actively waiting, but
/// still prompt.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve the latest published tag by reading the redirect target of
/// `releases/latest` instead of the GitHub API: no auth, no rate limits.
///
/// `timeout` bounds the whole request so a poor or absent connection can't
/// hang the caller. Every caller passes a deadline (generous for an explicit
/// update, tight for the `--check` / startup paths).
async fn fetch_latest_tag(timeout: Duration) -> Result<String> {
    let url = format!("https://github.com/{RELEASES_REPO}/releases/latest");
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()?;
    let res = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    // `releases/latest` 302-redirects to the newest published non-prerelease
    // release. A 404 means there genuinely isn't one yet (drafts and
    // prereleases don't count); anything else non-3xx (429, 5xx) is the host
    // rate-limiting or down, which must not be reported as "no releases".
    let status = res.status();
    if !status.is_redirection() {
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!(
                "{url} returned 404 - no published release found \
                 (drafts and prereleases don't count as 'latest')"
            );
        }
        bail!(
            "{url} returned {status} - the release host may be rate-limiting or \
             temporarily down; try again shortly"
        );
    }
    let location = res
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow!("{url} redirect carries no Location header"))?;
    parse_tag_from_location(location)
}

fn parse_tag_from_location(location: &str) -> Result<String> {
    let tag = location
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    if tag.is_empty() || tag == "latest" || tag == "releases" {
        bail!("could not extract a release tag from redirect target {location}");
    }
    Ok(tag.to_string())
}

/// Accept only the tag grammar the release workflow enforces:
/// `vMAJOR.MINOR.PATCH` with an optional `-rc.N` / `-alpha.N` / `-beta.N`
/// prerelease. This is a security gate, not a nicety: `target_tag` is used
/// unmodified as a path component and a download URL, so rejecting anything
/// outside this shape is what stops a `--version ../..` (or absolute path)
/// from steering the staging directory -- and its remove_dir_all -- outside
/// `~/.dobj`.
fn validate_release_tag(tag: &str) -> Result<()> {
    fn all_digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    let parsed = (|| {
        let body = tag.strip_prefix('v')?;
        let (core, pre) = match body.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (body, None),
        };
        let core_parts: Vec<&str> = core.split('.').collect();
        if core_parts.len() != 3 || !core_parts.iter().all(|p| all_digits(p)) {
            return None;
        }
        if let Some(pre) = pre {
            let (channel, n) = pre.split_once('.')?;
            if !matches!(channel, "rc" | "alpha" | "beta") || !all_digits(n) {
                return None;
            }
        }
        Some(())
    })();
    ensure!(
        parsed.is_some(),
        "invalid release tag {tag:?}; expected vMAJOR.MINOR.PATCH with an \
         optional -rc.N / -alpha.N / -beta.N suffix"
    );
    Ok(())
}

enum Ordering {
    Newer,
    Same,
    Older,
    /// One side is "dev" or otherwise not a semver tag.
    Unknown,
}

/// Order `target` against `current`, semver-aware so that prerelease tags
/// (`v0.1.0-rc.34`) sort below their release (`v0.1.0`).
fn compare_tags(current: &str, target: &str) -> Ordering {
    let parse = |tag: &str| semver::Version::parse(tag.trim_start_matches('v')).ok();
    match (parse(current), parse(target)) {
        (Some(cur), Some(tgt)) => match tgt.cmp(&cur) {
            std::cmp::Ordering::Greater => Ordering::Newer,
            std::cmp::Ordering::Equal => Ordering::Same,
            std::cmp::Ordering::Less => Ordering::Older,
        },
        _ => Ordering::Unknown,
    }
}

// ---------------------------------------------------------------------------
// startup notice: "a newer release is available"

/// A successful check is reused for a day so frequent `dobj status` calls
/// don't hit the network every time.
const UPDATE_CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// After a *failed* check the passive path waits this long before retrying, so
/// an offline `dobj status` doesn't pay the network timeout on every call (but
/// recovers within the hour once back online).
const UPDATE_RETRY_AFTER_FAILURE_SECS: u64 = 60 * 60;
/// Tight bound so an offline or slow network never stalls `dobj start` /
/// `status` waiting on the release host.
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize, Deserialize, Default)]
struct UpdateCheckCache {
    /// Unix seconds of the last *successful* network check.
    #[serde(default)]
    checked_at: u64,
    /// Latest release tag from that check; empty string means never resolved.
    #[serde(default)]
    latest: String,
    /// Unix seconds of the last *attempt*, success or failure. Throttles
    /// retries after a failure (see UPDATE_RETRY_AFTER_FAILURE_SECS).
    /// `serde(default)` so caches written by older builds still parse.
    #[serde(default)]
    last_attempt: u64,
}

/// Print a one-line stderr notice when a newer release than the running CLI
/// exists. Best-effort: silent on dev builds, network failure, a throttled
/// check, or when already current, and it never returns an error - it must
/// not interfere with the command the user actually ran. stderr-only so it
/// can't pollute `--json` or other parsed stdout.
pub async fn notify_if_outdated() {
    if CURRENT_TAG == "dev" {
        return;
    }
    let Some(latest) = resolve_latest_cached(UPDATE_CHECK_TIMEOUT, true).await else {
        return;
    };
    if let Ordering::Newer = compare_tags(CURRENT_TAG, &latest) {
        eprintln!("a newer release is available: {latest} (run `dobj update`)");
    }
}

/// Resolve the latest tag with the day-cache (`~/.dobj/update-check.json`) as
/// backing. With `prefer_cache` (the silent startup notice) a fresh cache
/// short-circuits the network entirely; otherwise (an explicit `--check`) a
/// bounded fetch is always attempted for current info. Either way a failed
/// fetch falls back to whatever the cache holds, and a success refreshes it.
/// Returns None only when the network fails and there is no cache.
async fn resolve_latest_cached(timeout: Duration, prefer_cache: bool) -> Option<String> {
    let cache_path = daemon::dobj_home().ok()?.join("update-check.json");
    let now = unix_now();
    let cached = read_check_cache(&cache_path);

    // The passive path (prefer_cache) skips the network when either the last
    // success is still fresh, or the last attempt failed recently - the latter
    // is what stops an offline `dobj status` from paying the timeout on every
    // call. An explicit `--check` always attempts.
    if prefer_cache
        && let Some(c) = &cached
        && (within(c.checked_at, now, UPDATE_CHECK_INTERVAL_SECS)
            || within(c.last_attempt, now, UPDATE_RETRY_AFTER_FAILURE_SECS))
    {
        return non_empty(&c.latest);
    }

    match fetch_latest_tag(timeout).await {
        Ok(latest) => {
            write_check_cache(
                &cache_path,
                &UpdateCheckCache {
                    checked_at: now,
                    latest: latest.clone(),
                    last_attempt: now,
                },
            );
            Some(latest)
        }
        Err(_) => {
            // Record the attempt so the passive path backs off; keep the last
            // successful tag (if any) so the reminder still shows while offline.
            let mut prev = cached.unwrap_or_default();
            prev.last_attempt = now;
            write_check_cache(&cache_path, &prev);
            non_empty(&prev.latest)
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// True if `ts` is within `window` seconds of `now`. `saturating_sub` keeps
/// backwards clock skew (now < ts) reading as within-window rather than
/// panicking or treating it as infinitely stale.
fn within(ts: u64, now: u64, window: u64) -> bool {
    now.saturating_sub(ts) < window
}

/// A tag string, or None if empty (the cache stores "" for "never resolved").
fn non_empty(tag: &str) -> Option<String> {
    (!tag.is_empty()).then(|| tag.to_string())
}

fn read_check_cache(path: &Path) -> Option<UpdateCheckCache> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Best-effort: a failed write just means we re-check next time.
fn write_check_cache(path: &Path, cache: &UpdateCheckCache) {
    if let Ok(json) = serde_json::to_vec(cache) {
        let _ = fs::write(path, json);
    }
}

// ---------------------------------------------------------------------------
// --check

async fn daemon_version(client: &DobjdClient) -> Option<String> {
    client
        .get_json::<wire_types::HealthResponse>("/healthz")
        .await
        .ok()
        .and_then(|h| h.version)
}

async fn report(client: &DobjdClient, version: Option<String>) -> Result<()> {
    println!("dobj:   {CURRENT_TAG} ({TARGET_TRIPLE})");
    let daemon = daemon_version(client).await;
    match &daemon {
        Some(v) => println!("dobjd:  {v} (running)"),
        None => println!("dobjd:  not running"),
    }

    // A pinned --version is taken as-is; the common no-version check resolves
    // the latest tag under a tight bound with cache fallback, so it stays
    // prompt on a poor connection and still answers (from cache) offline.
    let (target_tag, label) = match version {
        Some(tag) => (Some(tag), "target"),
        None => (resolve_latest_cached(CHECK_TIMEOUT, false).await, "latest"),
    };
    let Some(target_tag) = target_tag else {
        println!("latest: unknown (could not reach the release host - offline?)");
        return Ok(());
    };
    println!("{label}: {target_tag}");

    if let Some(v) = &daemon
        && v != CURRENT_TAG
    {
        println!("note: daemon and CLI versions differ - run `dobj update` to reconcile");
    }
    match compare_tags(CURRENT_TAG, &target_tag) {
        Ordering::Newer => println!("update available: run `dobj update`"),
        Ordering::Same => println!("up to date"),
        Ordering::Older => println!("installed version is newer than {label}"),
        Ordering::Unknown => println!("cannot compare versions"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// download + stage

/// Connection-phase bound (DNS + TCP + TLS): fail fast if the host is
/// unreachable, without capping the body transfer.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Inactivity bound on the body: abort only if no bytes arrive for this long,
/// so a slow-but-steady link still completes while a stalled one fails. This
/// is deliberately NOT a total timeout - release tarballs are tens of MB and a
/// legitimately slow connection must be allowed to finish.
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Attempts per artifact, to ride out a transient drop on a flaky connection
/// (matches install.sh's `curl --retry 3`). Only transient failures retry; a
/// 4xx (missing release/asset) fails immediately.
const DOWNLOAD_ATTEMPTS: u32 = 3;

struct StagedBinary {
    /// File name including platform suffix, e.g. `dobjd.exe` on Windows.
    name: String,
    path: PathBuf,
}

/// Transport security (TLS to github.com) is the whole integrity story for
/// now, matching the install scripts. This seam is where heavier verification
/// (checksums, signatures) can slot in later.
fn verify_artifact(bytes: &[u8], url: &str) -> Result<()> {
    ensure!(!bytes.is_empty(), "empty artifact from {url}");
    Ok(())
}

async fn download_and_stage(tag: &str, staging: &Path) -> Result<Vec<StagedBinary>> {
    // Clear any partial staging from an earlier failed attempt.
    if staging.exists() {
        fs::remove_dir_all(staging)
            .with_context(|| format!("failed to clear stale staging {}", staging.display()))?;
    }

    // connect_timeout + read_timeout rather than a total timeout: the body can
    // be tens of MB, so a slow-but-progressing download must be allowed to run
    // as long as it keeps making progress; only a stalled stream should fail.
    let http = reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .read_timeout(DOWNLOAD_READ_TIMEOUT)
        .build()?;
    let mut staged = Vec::new();
    for (artifact, binaries) in ARTIFACTS {
        let url = format!(
            "https://github.com/{RELEASES_REPO}/releases/download/{tag}/{artifact}-{TARGET_TRIPLE}.tar.gz"
        );
        println!("  fetching {artifact}-{TARGET_TRIPLE}.tar.gz ...");
        let bytes = download_artifact(&http, &url).await?;
        verify_artifact(&bytes, &url)?;

        let dir = staging.join(artifact);
        fs::create_dir_all(&dir)?;
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()))
            .unpack(&dir)
            .with_context(|| format!("failed to extract {url}"))?;

        for binary in *binaries {
            let name = format!("{binary}{}", std::env::consts::EXE_SUFFIX);
            let path = dir.join(&name);
            ensure!(
                path.is_file(),
                "release {tag} artifact {artifact}-{TARGET_TRIPLE}.tar.gz does not contain {name}"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            }
            staged.push(StagedBinary { name, path });
        }
    }
    Ok(staged)
}

/// Transient failures (connect/read timeout, dropped connection, 5xx) are
/// worth retrying; a 4xx means the release or asset isn't there and won't
/// appear on a retry, so it is surfaced immediately.
enum DownloadError {
    Transient(anyhow::Error),
    Permanent(anyhow::Error),
}

/// Download one artifact with a few attempts so a transient drop on a flaky
/// connection doesn't fail the whole update. The connection and inactivity
/// bounds live on the client; this retries the request as a unit and gives up
/// after DOWNLOAD_ATTEMPTS.
async fn download_artifact(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let mut last_err = None;
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        match attempt_download(http, url).await {
            Ok(bytes) => return Ok(bytes),
            Err(DownloadError::Permanent(err)) => return Err(err),
            Err(DownloadError::Transient(err)) => {
                if attempt < DOWNLOAD_ATTEMPTS {
                    eprintln!(
                        "  attempt {attempt}/{DOWNLOAD_ATTEMPTS} failed ({err:#}); retrying..."
                    );
                    // Linear backoff. Each attempt is already bounded by the
                    // client's connect/read timeouts, so this only spaces out
                    // retries on a flaky link.
                    tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
                }
                last_err = Some(err);
            }
        }
    }
    Err(last_err.expect("loop runs at least once").context(format!(
        "download failed after {DOWNLOAD_ATTEMPTS} attempts: {url}"
    )))
}

async fn attempt_download(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, DownloadError> {
    let res = match http.get(url).send().await {
        Ok(res) => res,
        Err(err) => {
            return Err(DownloadError::Transient(
                anyhow::Error::new(err).context(format!("GET {url}")),
            ));
        }
    };
    let status = res.status();
    if status.is_client_error() {
        return Err(DownloadError::Permanent(anyhow!(
            "download failed: {url} returned {status} (does the release exist and is it published?)"
        )));
    }
    if !status.is_success() {
        return Err(DownloadError::Transient(anyhow!("{url} returned {status}")));
    }
    match res.bytes().await {
        Ok(bytes) => Ok(bytes.to_vec()),
        Err(err) => Err(DownloadError::Transient(
            anyhow::Error::new(err).context(format!("reading body from {url}")),
        )),
    }
}

// ---------------------------------------------------------------------------
// guard, swap, validate, rollback

/// Self-update only manages binaries it installed: `current_exe` must live in
/// `~/.dobj/bin`. Anything else (homebrew cellar, cargo target dir, ...) is
/// owned by whatever put it there.
fn ensure_managed_install() -> Result<PathBuf> {
    let bin_dir = daemon::dobj_home()?.join("bin");
    let canonical_bin = bin_dir
        .canonicalize()
        .map_err(|_| anyhow!("no managed install found at {}", bin_dir.display()))?;
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("could not resolve the running executable")?;
    if exe.parent() != Some(canonical_bin.as_path()) {
        bail!(
            "this dobj runs from {}, not {} - it was not installed by the Digital Objects installer.\n\
             Update it the way it was installed (package manager, cargo, ...).",
            exe.display(),
            canonical_bin.display()
        );
    }
    Ok(canonical_bin)
}

struct SwapEntry {
    installed: PathBuf,
    backup: PathBuf,
    had_previous: bool,
    installed_new: bool,
}

/// Two-phase swap: first rename every current binary aside to `<name>.old`,
/// then move the staged binaries into place. Both phases are same-directory
/// or same-filesystem renames (staging lives under `~/.dobj`), so each step
/// is atomic and any failure point has a simple inverse. On error the
/// completed steps are rolled back before returning.
fn swap_binaries(bin_dir: &Path, staged: &[StagedBinary]) -> Result<Vec<SwapEntry>> {
    let mut journal: Vec<SwapEntry> = staged
        .iter()
        .map(|b| SwapEntry {
            installed: bin_dir.join(&b.name),
            backup: bin_dir.join(format!("{}.old", b.name)),
            had_previous: false,
            installed_new: false,
        })
        .collect();

    let mut apply = || -> Result<()> {
        for entry in journal.iter_mut() {
            if entry.installed.exists() {
                // Drop the previous generation's backup. On Windows this can
                // fail while an old process still holds it; the rename below
                // replaces it anyway (std::fs::rename replaces existing
                // destinations), so ignore the error here.
                let _ = fs::remove_file(&entry.backup);
                fs::rename(&entry.installed, &entry.backup).with_context(|| {
                    format!("failed to move {} aside", entry.installed.display())
                })?;
                entry.had_previous = true;
            }
        }
        for (entry, binary) in journal.iter_mut().zip(staged) {
            install_file(&binary.path, &entry.installed)?;
            entry.installed_new = true;
        }
        Ok(())
    };

    match apply() {
        Ok(()) => Ok(journal),
        Err(err) => {
            rollback(&journal);
            Err(err)
        }
    }
}

/// Move a staged file into place, preferring rename (atomic). Falls back to
/// copy + rename for the cross-filesystem case so the final step at the
/// destination path is still atomic.
fn install_file(staged: &Path, dest: &Path) -> Result<()> {
    if fs::rename(staged, dest).is_ok() {
        return Ok(());
    }
    let tmp = dest.with_extension("new");
    fs::copy(staged, &tmp).with_context(|| format!("failed to copy into {}", tmp.display()))?;
    fs::rename(&tmp, dest).with_context(|| format!("failed to install {}", dest.display()))
}

/// Best-effort inverse of `swap_binaries`, in reverse order. Failures are
/// reported but not fatal: a partially restored install is still better than
/// aborting the restore halfway with an error.
fn rollback(journal: &[SwapEntry]) {
    for entry in journal.iter().rev() {
        if entry.installed_new {
            let _ = fs::remove_file(&entry.installed);
        }
        if entry.had_previous
            && let Err(err) = fs::rename(&entry.backup, &entry.installed)
        {
            eprintln!(
                "rollback: failed to restore {}: {err}",
                entry.installed.display()
            );
        }
    }
}

/// Binaries whose `--version` output carries the release tag (built with the
/// stamping build.rs). The proxy isn't stamped, so it is validated only for
/// runnability, not for a matching tag.
const STAMPED_BINARIES: &[&str] = &["dobj", "dobjd"];

/// Spawn every freshly installed binary with `--version` and require it to
/// run; stamped binaries must also report the target tag. This catches
/// wrong-architecture artifacts, missing shared libraries, and Gatekeeper/AV
/// kills before the daemon restart commits us. The binary set comes from
/// ARTIFACTS so a newly bundled binary (e.g. the proxy) is validated for
/// runnability without a second list to keep in sync.
fn validate_installed(bin_dir: &Path, tag: &str) -> Result<()> {
    for binary in ARTIFACTS
        .iter()
        .flat_map(|(_, binaries)| binaries.iter().copied())
    {
        let path = bin_dir.join(format!("{binary}{}", std::env::consts::EXE_SUFFIX));
        let output = std::process::Command::new(&path)
            .arg("--version")
            .output()
            .with_context(|| format!("failed to run {} --version", path.display()))?;
        ensure!(
            output.status.success(),
            "{} --version failed (exit {:?}); the binary may be the wrong \
             architecture or missing a dependency",
            path.display(),
            output.status.code()
        );
        if STAMPED_BINARIES.contains(&binary) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            ensure!(
                version_output_matches(&stdout, tag),
                "{} --version reported {:?}, expected the tag {tag}",
                path.display(),
                stdout.trim()
            );
        }
    }
    Ok(())
}

/// True if `tag` appears as a whole whitespace-delimited token in a
/// `<name> <tag> (<triple>)` version line. A token match (not a substring)
/// stops a superstring tag like `v0.1.0-rc.5` from satisfying a `v0.1.0`
/// target.
fn version_output_matches(stdout: &str, tag: &str) -> bool {
    stdout.split_whitespace().any(|token| token == tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tag_from_redirect_location() {
        let loc = "https://github.com/dobjlabs/zk-craft-releases/releases/tag/v0.1.0-rc.33";
        assert_eq!(parse_tag_from_location(loc).unwrap(), "v0.1.0-rc.33");
    }

    #[test]
    fn rejects_locations_without_a_tag() {
        assert!(parse_tag_from_location("https://github.com/x/y/releases/").is_err());
        assert!(parse_tag_from_location("https://github.com/x/y/releases/latest").is_err());
    }

    #[test]
    fn orders_release_candidates_and_releases() {
        assert!(matches!(
            compare_tags("v0.1.0-rc.33", "v0.1.0-rc.34"),
            Ordering::Newer
        ));
        // A release outranks its own release candidates.
        assert!(matches!(
            compare_tags("v0.1.0-rc.34", "v0.1.0"),
            Ordering::Newer
        ));
        assert!(matches!(
            compare_tags("v0.1.0", "v0.1.0-rc.34"),
            Ordering::Older
        ));
        assert!(matches!(compare_tags("v0.1.0", "v0.1.0"), Ordering::Same));
        assert!(matches!(compare_tags("dev", "v0.1.0"), Ordering::Unknown));
    }

    #[test]
    fn accepts_well_formed_release_tags() {
        for tag in [
            "v0.1.0",
            "v1.2.3",
            "v0.1.0-rc.34",
            "v0.1.0-alpha.1",
            "v0.1.0-beta.10",
        ] {
            assert!(validate_release_tag(tag).is_ok(), "should accept {tag}");
        }
    }

    #[test]
    fn rejects_path_traversal_and_malformed_tags() {
        // The path-escape cases this gate exists to stop.
        for tag in ["../..", "/etc", "..", "v0.1.0/../../x", "v0.1.0-rc.1/.."] {
            assert!(validate_release_tag(tag).is_err(), "should reject {tag}");
        }
        // Plain malformed tags, rejected for free by the same grammar.
        for tag in [
            "0.1.0",
            "v0.1",
            "v0.1.0-rc",
            "v0.1.0-nightly.1",
            "vx.y.z",
            "",
        ] {
            assert!(validate_release_tag(tag).is_err(), "should reject {tag}");
        }
    }

    #[test]
    fn update_check_cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        // A missing file reads as "no cache" rather than erroring.
        assert!(read_check_cache(&path).is_none());

        write_check_cache(
            &path,
            &UpdateCheckCache {
                checked_at: 123,
                latest: "v0.1.0".to_string(),
                last_attempt: 456,
            },
        );
        let read = read_check_cache(&path).expect("cache reads back");
        assert_eq!(read.checked_at, 123);
        assert_eq!(read.latest, "v0.1.0");
        assert_eq!(read.last_attempt, 456);
    }

    #[test]
    fn update_check_cache_reads_legacy_two_field_file() {
        // Caches written before `last_attempt` existed must still parse, with
        // the new field defaulting to 0.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        fs::write(&path, br#"{"checked_at":5,"latest":"v0.1.0"}"#).unwrap();
        let read = read_check_cache(&path).expect("legacy cache parses");
        assert_eq!(read.checked_at, 5);
        assert_eq!(read.latest, "v0.1.0");
        assert_eq!(read.last_attempt, 0);
    }

    #[test]
    fn within_respects_the_throttle_window() {
        assert!(within(1_000, 1_000, UPDATE_CHECK_INTERVAL_SECS));
        assert!(within(
            1_000,
            1_000 + UPDATE_CHECK_INTERVAL_SECS - 1,
            UPDATE_CHECK_INTERVAL_SECS
        ));
        // At/after the window: outside, trigger a re-check.
        assert!(!within(
            1_000,
            1_000 + UPDATE_CHECK_INTERVAL_SECS,
            UPDATE_CHECK_INTERVAL_SECS
        ));
        // Backwards clock skew must not read as outside-forever.
        assert!(within(1_000, 0, UPDATE_CHECK_INTERVAL_SECS));
    }

    #[test]
    fn version_output_matches_requires_a_whole_token() {
        assert!(version_output_matches(
            "dobj v0.1.0 (x86_64-unknown-linux-gnu)",
            "v0.1.0"
        ));
        assert!(version_output_matches(
            "dobjd v0.1.0 (aarch64-apple-darwin)",
            "v0.1.0"
        ));
        // A superstring tag must not satisfy a shorter target (the #6 bug).
        assert!(!version_output_matches(
            "dobj v0.1.0-rc.5 (x86_64-unknown-linux-gnu)",
            "v0.1.0"
        ));
        // The exact prerelease still matches itself.
        assert!(version_output_matches(
            "dobj v0.1.0-rc.5 (x86_64-unknown-linux-gnu)",
            "v0.1.0-rc.5"
        ));
    }

    #[test]
    fn swap_installs_new_and_keeps_backup() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let stage = dir.path().join("stage");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&stage).unwrap();
        for name in ["dobjd", "dobj"] {
            fs::write(bin.join(name), format!("old-{name}")).unwrap();
            fs::write(stage.join(name), format!("new-{name}")).unwrap();
        }
        let staged: Vec<StagedBinary> = ["dobjd", "dobj"]
            .iter()
            .map(|n| StagedBinary {
                name: n.to_string(),
                path: stage.join(n),
            })
            .collect();

        let journal = swap_binaries(&bin, &staged).unwrap();
        for name in ["dobjd", "dobj"] {
            assert_eq!(
                fs::read_to_string(bin.join(name)).unwrap(),
                format!("new-{name}")
            );
            assert_eq!(
                fs::read_to_string(bin.join(format!("{name}.old"))).unwrap(),
                format!("old-{name}")
            );
        }

        rollback(&journal);
        for name in ["dobjd", "dobj"] {
            assert_eq!(
                fs::read_to_string(bin.join(name)).unwrap(),
                format!("old-{name}")
            );
        }
    }

    #[test]
    fn swap_handles_fresh_install_without_previous_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let stage = dir.path().join("stage");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("dobj"), "new").unwrap();
        let staged = vec![StagedBinary {
            name: "dobj".to_string(),
            path: stage.join("dobj"),
        }];

        let journal = swap_binaries(&bin, &staged).unwrap();
        assert_eq!(fs::read_to_string(bin.join("dobj")).unwrap(), "new");

        // Rollback of a fresh install removes the new binary entirely.
        rollback(&journal);
        assert!(!bin.join("dobj").exists());
    }
}
