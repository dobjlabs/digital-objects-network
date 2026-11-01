//! Runs the bundled proof-of-work grinder on the game page's behalf.

use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use tokio::{process::Command, sync::Notify};

/// The action arguments the grinder found, one NAME=JSON line each, with
/// each value in the POD2 JSON form dobjd takes.
fn parse_output(stdout: &str) -> Result<Map<String, Value>> {
    let mut args = Map::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let (arg, value) = line
            .split_once('=')
            .with_context(|| format!("The grinder printed {line:?}, not an argument"))?;
        let value = serde_json::from_str(value)
            .with_context(|| format!("The grinder printed an invalid value for {arg}"))?;
        args.insert(arg.to_string(), value);
    }
    if args.is_empty() {
        bail!("The grinder printed no arguments");
    }
    Ok(args)
}

async fn search(
    grinder: &Path,
    home: &Path,
    action: &str,
    input_object_paths: &[String],
    cancel: &Notify,
) -> Result<Map<String, Value>> {
    let cancelled = cancel.notified();
    tokio::pin!(cancelled);
    cancelled.as_mut().enable();
    let mut command = Command::new(grinder);
    command.arg("--home").arg(home).arg("--exit-on-stdin-close");
    for path in input_object_paths {
        command.arg(format!("--input={path}"));
    }
    command
        .arg("--")
        .arg(action)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(crate::runtime::CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .context("Could not start the bundled grinder")?;
    // Held open until the search ends; the grinder stops when it closes.
    let _stdin = child.stdin.take();
    let output = tokio::select! {
        output = child.wait_with_output() => output.context("The grinder failed")?,
        () = &mut cancelled => bail!("Mining stopped because the game page reloaded"),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{}",
            stderr
                .lines()
                .last()
                .unwrap_or("The grinder exited without finishing its search")
        );
    }
    parse_output(&String::from_utf8_lossy(&output.stdout))
}

#[tauri::command]
pub async fn grind_pow(
    desktop: tauri::State<'_, crate::Desktop>,
    action: String,
    input_object_paths: Vec<String>,
) -> Result<Map<String, Value>, String> {
    let grinder = crate::bundled_binary("bitcraft-grind").map_err(|error| format!("{error:#}"))?;
    search(
        &grinder,
        &desktop.home,
        &action,
        &input_object_paths,
        &desktop.grind_cancel,
    )
    .await
    .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_output_line_becomes_an_argument_and_its_pod2_value() {
        assert_eq!(
            Value::Object(
                parse_output(
                    "grind_nonce={\"Untyped\":\"00ff\"}\npocketknife_key={\"Untyped\":\"0a\"}\n"
                )
                .unwrap()
            ),
            serde_json::json!({
                "grind_nonce": { "Untyped": "00ff" },
                "pocketknife_key": { "Untyped": "0a" }
            })
        );
        assert!(parse_output("").is_err());
        assert!(parse_output("grind_nonce=not json").is_err());
        assert!(parse_output("grind_nonce={\"Untyped\":\"00ff\"}\nnoise\n").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_stops_a_running_search() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let grinder = directory.path().join("grinder");
        std::fs::write(&grinder, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&grinder, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cancel = Notify::new();
        let started = std::time::Instant::now();
        let search = search(
            &grinder,
            directory.path(),
            "bitcraft::GatherWood",
            &[],
            &cancel,
        );
        tokio::pin!(search);
        tokio::select! {
            _ = &mut search => panic!("the search ended on its own"),
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        }
        cancel.notify_waiters();
        let error = search.await.unwrap_err();
        assert!(error.to_string().contains("reloaded"), "{error:#}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
