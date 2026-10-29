use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The player prompt, edited as Markdown in `apps/bitcraft/AGENT_PROMPT.md`
/// and built into the app. `{skill}`, `{cli}` and `{home}` in it stand for
/// the installed player skill, the installed CLI and the game's home
/// directory.
const PROMPT_TEMPLATE: &str = include_str!("../../AGENT_PROMPT.md");

pub fn prompt(skill: &Path, cli: &Path, home: &Path) -> String {
    PROMPT_TEMPLATE
        .replace("{skill}", &skill.display().to_string())
        .replace("{cli}", &cli.display().to_string())
        .replace("{home}", &home.display().to_string())
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
        let appearance = crate::appearance::current(app);
        WebviewWindowBuilder::new(app, "agent-guide", WebviewUrl::App("agent.html".into()))
            .title("Play with agents")
            .inner_size(720.0, 660.0)
            .min_inner_size(560.0, 480.0)
            .theme(Some(appearance.theme()))
            .background_color(appearance.background())
            .initialization_script(format!("window.bitcraftAgentGuide = {context};"))
            .initialization_script(appearance.initialization_script())
            .on_navigation(move |url| crate::is_local_app_url(url, dev_url.as_ref()))
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .build()?
    };
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_fills_in_every_placeholder_from_the_template() {
        for placeholder in ["{skill}", "{cli}", "{home}"] {
            assert!(
                PROMPT_TEMPLATE.contains(placeholder),
                "{placeholder} is unused"
            );
        }
        let prompt = prompt(
            Path::new("/skills/SKILL.md"),
            Path::new("/bin/bitcraft-dobj"),
            Path::new("/home/player/.bitcraft"),
        );
        for path in [
            "/skills/SKILL.md",
            "/bin/bitcraft-dobj",
            "/home/player/.bitcraft",
        ] {
            assert!(prompt.contains(path), "{path} is missing");
        }
        assert!(
            !prompt.contains("{skill}") && !prompt.contains("{cli}") && !prompt.contains("{home}")
        );
    }
}
