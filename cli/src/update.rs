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

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Deserialize;

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
    let target_tag = match &version {
        Some(tag) => tag.clone(),
        None => latest_release_tag().await?,
    };

    if check {
        return report(client, &target_tag, version.is_some()).await;
    }

    if CURRENT_TAG == "dev" {
        bail!("this is a dev build; update it by rebuilding, not via dobj update");
    }

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

    println!("updating {CURRENT_TAG} -> {target_tag}");
    let staging = dobj_home()?.join("staging").join(&target_tag);
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
            let _ = daemon::start(client).await;
        }
        return Err(err.context("new binaries failed validation; rolled back"));
    }

    if was_running {
        println!(
            "restarting dobjd (a new version may rebuild proving circuits - this can take a few minutes)"
        );
        if let Err(err) = daemon::start(client).await {
            rollback(&journal);
            let restored = daemon::start(client).await.is_ok();
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

/// Resolve the latest published tag by reading the redirect target of
/// `releases/latest` instead of the GitHub API: no auth, no rate limits.
async fn latest_release_tag() -> Result<String> {
    let url = format!("https://github.com/{RELEASES_REPO}/releases/latest");
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let res = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !res.status().is_redirection() {
        bail!(
            "{url} returned {} instead of a redirect; has any release been published?",
            res.status()
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
// --check

#[derive(Deserialize)]
struct DaemonHealth {
    #[allow(dead_code)]
    ok: bool,
    #[serde(default)]
    version: Option<String>,
}

async fn daemon_version(client: &DobjdClient) -> Option<String> {
    client
        .get_json::<DaemonHealth>("/healthz")
        .await
        .ok()
        .and_then(|h| h.version)
}

async fn report(client: &DobjdClient, target_tag: &str, pinned: bool) -> Result<()> {
    println!("dobj:   {CURRENT_TAG} ({TARGET_TRIPLE})");
    let daemon = daemon_version(client).await;
    match &daemon {
        Some(v) => println!("dobjd:  {v} (running)"),
        None => println!("dobjd:  not running"),
    }
    let label = if pinned { "target" } else { "latest" };
    println!("{label}: {target_tag}");

    if let Some(v) = &daemon
        && v != CURRENT_TAG
    {
        println!("note: daemon and CLI versions differ - run `dobj update` to reconcile");
    }
    match compare_tags(CURRENT_TAG, target_tag) {
        Ordering::Newer => println!("update available: run `dobj update`"),
        Ordering::Same => println!("up to date"),
        Ordering::Older => println!("installed version is newer than {label}"),
        Ordering::Unknown => println!("cannot compare versions"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// download + stage

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

    let http = reqwest::Client::new();
    let mut staged = Vec::new();
    for (artifact, binaries) in ARTIFACTS {
        let url = format!(
            "https://github.com/{RELEASES_REPO}/releases/download/{tag}/{artifact}-{TARGET_TRIPLE}.tar.gz"
        );
        println!("  fetching {artifact}-{TARGET_TRIPLE}.tar.gz ...");
        let res = http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| {
                format!("download failed: {url} (does release {tag} exist and is it published?)")
            })?;
        let bytes = res.bytes().await?;
        verify_artifact(&bytes, &url)?;

        let dir = staging.join(artifact);
        fs::create_dir_all(&dir)?;
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_ref()))
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

// ---------------------------------------------------------------------------
// guard, swap, validate, rollback

fn dobj_home() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("could not resolve home directory"))?
        .join(".dobj"))
}

/// Self-update only manages binaries it installed: `current_exe` must live in
/// `~/.dobj/bin`. Anything else (homebrew cellar, cargo target dir, ...) is
/// owned by whatever put it there.
fn ensure_managed_install() -> Result<PathBuf> {
    let bin_dir = dobj_home()?.join("bin");
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

/// Spawn the freshly installed binaries with `--version` and require the new
/// tag in their output. Catches wrong-architecture artifacts, missing shared
/// libraries, and Gatekeeper/AV kills before the daemon restart commits us.
fn validate_installed(bin_dir: &Path, tag: &str) -> Result<()> {
    for binary in ["dobj", "dobjd"] {
        let path = bin_dir.join(format!("{binary}{}", std::env::consts::EXE_SUFFIX));
        let output = std::process::Command::new(&path)
            .arg("--version")
            .output()
            .with_context(|| format!("failed to run {} --version", path.display()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        ensure!(
            output.status.success() && stdout.contains(tag),
            "{} --version reported {:?}, expected it to contain {tag}",
            path.display(),
            stdout.trim()
        );
    }
    Ok(())
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
