use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentGuide {
    prompt: String,
    skill_path: String,
    enable_mcp: String,
    mcp_config: String,
}

fn quote_argument(value: &str) -> String {
    #[cfg(target_os = "windows")]
    let quoted = value.replace('\'', "''");
    #[cfg(not(target_os = "windows"))]
    let quoted = value.replace('\'', "'\"'\"'");
    format!("'{quoted}'")
}

fn guide(skill: &Path, cli: &Path, home: &Path) -> AgentGuide {
    let path_text = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    let prompt = format!(
        "Help me play the installed Bitcraft game.\n\nFirst read and follow the player skill at:\n{}\n\nThe installed CLI executable is:\n{}\n\nFor every CLI invocation, pass --home {} before the subcommand.\n\nStart by checking --help, my current inventory, and available actions, then help me choose a goal and make progress. Use the installed app; no source checkout is needed. You need local filesystem and shell access.\n",
        path_text(skill),
        path_text(cli),
        path_text(home),
    );
    let prefix = if cfg!(target_os = "windows") {
        "& "
    } else {
        ""
    };
    let enable_mcp = format!(
        "{prefix}{} --home {} settings set --mcp on",
        quote_argument(&cli.to_string_lossy()),
        quote_argument(&home.to_string_lossy())
    );
    AgentGuide {
        prompt,
        skill_path: skill.to_string_lossy().into_owned(),
        enable_mcp,
        mcp_config: format!(
            "[mcp_servers.bitcraft]\nurl = \"http://127.0.0.1:{}/mcp\"\nrequired = true\n",
            crate::runtime::HTTP_PORT + 1
        ),
    }
}

pub fn show(app: &tauri::AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window("agent-guide") {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    let (resources, _) = crate::runtime_paths(app)?;
    let cli = if cfg!(debug_assertions) {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!(
                "bitcraft-dobj-{}{}",
                env!("BITCRAFT_TARGET"),
                std::env::consts::EXE_SUFFIX
            ))
    } else {
        std::env::current_exe()?
            .parent()
            .unwrap()
            .join(format!("bitcraft-dobj{}", std::env::consts::EXE_SUFFIX))
    };
    let skill = resources.join("skills/bitcraft-play/SKILL.md");
    let context = guide(&skill, &cli, &app.state::<crate::Desktop>().home);
    let window =
        WebviewWindowBuilder::new(app, "agent-guide", WebviewUrl::App("agent.html".into()))
            .title("Play Bitcraft with an agent")
            .inner_size(760.0, 700.0)
            .min_inner_size(560.0, 480.0)
            .initialization_script(format!(
                "window.bitcraftAgent = {};",
                serde_json::to_string(&context)?
            ))
            .on_navigation(crate::is_local_app_url)
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .build()?;
    window.show()?;
    window.set_focus()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_prompt_uses_the_selected_installation_and_home() {
        let context = guide(
            Path::new("/Custom Apps/Game/skills/SKILL.md"),
            Path::new("/Custom Apps/Game/bin/cli"),
            Path::new("/data/Alice's game"),
        );
        assert!(context.prompt.contains("/Custom Apps/Game/skills/SKILL.md"));
        assert!(context.prompt.contains("/Custom Apps/Game/bin/cli"));
        assert!(context.prompt.contains("--home \"/data/Alice's game\""));
        #[cfg(not(target_os = "windows"))]
        assert!(context.enable_mcp.contains("'/data/Alice'\"'\"'s game'"));
        #[cfg(target_os = "windows")]
        assert!(context.enable_mcp.contains("'/data/Alice''s game'"));
        assert!(context.mcp_config.contains("127.0.0.1:7728/mcp"));
    }
}
