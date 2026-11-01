use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The player prompt, edited as Markdown in `apps/bitcraft/AGENT_PROMPT.md`
/// and built into the app. `{skill}`, `{cli}`, `{grinder}` and `{home}` in
/// it stand for the installed player skill, the installed CLI, the installed
/// proof-of-work grinder and the game's home directory.
const PROMPT_TEMPLATE: &str = include_str!("../../AGENT_PROMPT.md");

/// What the player prompt points an agent to in the installed app.
pub struct AgentTools {
    pub skill: PathBuf,
    pub cli: PathBuf,
    pub grinder: PathBuf,
}

pub fn prompt(tools: &AgentTools, home: &Path) -> String {
    PROMPT_TEMPLATE
        .replace("{skill}", &tools.skill.display().to_string())
        .replace("{cli}", &tools.cli.display().to_string())
        .replace("{grinder}", &tools.grinder.display().to_string())
        .replace("{home}", &home.display().to_string())
}

pub fn agent_tools(app: &tauri::AppHandle) -> Result<AgentTools> {
    Ok(AgentTools {
        skill: crate::runtime_resources(app)?.join("skills/bitcraft-play/SKILL.md"),
        cli: crate::bundled_binary("bitcraft-dobj")?,
        grinder: crate::bundled_binary("bitcraft-grind")?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Guide<'a> {
    prompt: String,
    error: Option<&'a str>,
}

pub fn show(app: &tauri::AppHandle, error: Option<&str>) -> Result<()> {
    let tools = agent_tools(app)?;
    let state = app.state::<crate::Desktop>();
    let guide = Guide {
        prompt: prompt(&tools, &state.home),
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
        let placeholders = ["{skill}", "{cli}", "{grinder}", "{home}"];
        for placeholder in placeholders {
            assert!(
                PROMPT_TEMPLATE.contains(placeholder),
                "{placeholder} is unused"
            );
        }
        let tools = AgentTools {
            skill: "/skills/SKILL.md".into(),
            cli: "/bin/bitcraft-dobj".into(),
            grinder: "/bin/bitcraft-grind".into(),
        };
        let prompt = prompt(&tools, Path::new("/home/player/.bitcraft"));
        for path in [
            "/skills/SKILL.md",
            "/bin/bitcraft-dobj",
            "/bin/bitcraft-grind",
            "/home/player/.bitcraft",
        ] {
            assert!(prompt.contains(path), "{path} is missing");
        }
        for placeholder in placeholders {
            assert!(!prompt.contains(placeholder), "{placeholder} remains");
        }
    }
}
