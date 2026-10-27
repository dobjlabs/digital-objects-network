use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

pub fn prompt(skill: &Path, cli: &Path, home: &Path) -> String {
    format!(
        "Help me play the installed Bitcraft game.\n\nFirst read and follow the player skill at:\n{}\n\nThe installed CLI executable is:\n{}\n\nWork in my Bitcraft home directory:\n{}\n\nRun the CLI from this directory or a subdirectory; it discovers the game's connection automatically. Keep Bitcraft open.\n\nStart by checking --help, my current inventory, and available actions, then help me choose a goal and make progress. Use the installed app; no source checkout is needed. You need local filesystem and shell access.\n",
        skill.display(),
        cli.display(),
        home.display(),
    )
}

pub fn paths(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf)> {
    let (resources, _) = crate::runtime_paths(app)?;
    let cli = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!(
                "bitcraft-dobj-{}{}",
                env!("BITCRAFT_TARGET"),
                std::env::consts::EXE_SUFFIX
            ))
    } else {
        std::env::current_exe()?
            .parent()
            .context("Cannot locate Bitcraft's installed CLI")?
            .join(format!("bitcraft-dobj{}", std::env::consts::EXE_SUFFIX))
    };
    Ok((resources.join("skills/bitcraft-play/SKILL.md"), cli))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Guide<'a> {
    prompt: String,
    error: Option<&'a str>,
}

pub fn show(app: &tauri::AppHandle, error: Option<&str>) -> Result<()> {
    let (skill, cli) = paths(app)?;
    let state = app.state::<crate::Desktop>();
    let guide = Guide {
        prompt: prompt(&skill, &cli, &state.home),
        error,
    };
    let context = serde_json::to_string(&guide)?;
    let window = if let Some(window) = app.get_webview_window("agent-guide") {
        window.eval(format!(
            "window.bitcraftAgentGuide = {context}; window.dispatchEvent(new Event('bitcraft-agent-guide-updated'));"
        ))?;
        window
    } else {
        let dev_url = crate::shell_dev_url(app);
        WebviewWindowBuilder::new(app, "agent-guide", WebviewUrl::App("agent.html".into()))
            .title("Play with agents")
            .inner_size(720.0, 660.0)
            .min_inner_size(560.0, 480.0)
            .initialization_script(format!("window.bitcraftAgentGuide = {context};"))
            .on_navigation(move |url| crate::is_local_app_url(url, dev_url.as_ref()))
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .build()?
    };
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(())
}
