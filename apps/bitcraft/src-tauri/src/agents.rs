use std::path::Path;

use anyhow::{Context, Result};
use tauri::Url;

mod platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn from_request(url: &Url) -> Option<Self> {
        if url.scheme() != "bitcraft"
            || url.host_str() != Some("agent")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        match url.path() {
            "/claude" => Some(Self::Claude),
            "/codex" => Some(Self::Codex),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "ChatGPT/Codex",
        }
    }

    fn link(self, skill: &Path, cli: &Path, home: &Path) -> Url {
        let prompt = crate::onboarding::prompt(skill, cli, home);
        let (address, directory_key, prompt_key) = match self {
            Self::Claude => ("claude://code/new", "folder", "q"),
            Self::Codex => ("codex://new", "path", "prompt"),
        };
        let mut url = Url::parse(address).unwrap();
        url.query_pairs_mut()
            .append_pair(directory_key, &home.to_string_lossy())
            .append_pair(prompt_key, &prompt);
        url
    }

    fn open(self, url: &Url, launch: impl FnOnce(&str) -> Result<bool>) -> Result<()> {
        let opened = launch(url.as_str()).with_context(|| format!(
            "Could not open {}. You can copy the prompt below into another agent, or try again after opening or updating its desktop app.", self.name()
        ))?;
        anyhow::ensure!(
            opened,
            "{} desktop is not available to open this link. Install or update its desktop app and open it once, then try again. You can also copy the prompt below into another agent with filesystem and terminal access.",
            self.name()
        );
        Ok(())
    }
}

fn launch(app: &tauri::AppHandle, agent: Agent) -> Result<()> {
    use tauri::Manager;

    let (skill, cli) = crate::onboarding::paths(app)?;
    let url = agent.link(&skill, &cli, &app.state::<crate::Desktop>().home);
    agent.open(&url, platform::launch)
}

pub fn request(app: &tauri::AppHandle, agent: Agent) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let launch_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || launch(&launch_app, agent)).await;
        let result = result.unwrap_or_else(|error| Err(error.into()));
        if let Err(error) = result {
            let message = format!("{error:#}");
            eprintln!("{message}");
            let guide_app = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                if let Err(error) = crate::onboarding::show(&guide_app, Some(&message)) {
                    eprintln!("Could not show agent guide: {error:#}");
                }
            }) {
                eprintln!("Could not show agent guide: {error:#}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_handlers_and_launch_failures_produce_actionable_guide_errors() {
        for agent in [Agent::Claude, Agent::Codex] {
            let url = Url::parse("example://new").unwrap();
            let missing = agent.open(&url, |_| Ok(false)).unwrap_err().to_string();
            assert!(missing.contains(agent.name()));
            assert!(missing.contains("copy the prompt"));
            assert!(missing.contains("Install or update"));
            let failed = agent
                .open(&url, |_| anyhow::bail!("launch rejected"))
                .unwrap_err();
            assert!(format!("{failed:#}").contains("launch rejected"));
            assert!(failed.to_string().contains("copy the prompt"));
            assert!(agent.open(&url, |_| Ok(true)).is_ok());
        }
    }

    #[test]
    fn links_preserve_the_selected_directory_and_prompt() {
        let home = Path::new("/data/Alice's game & notes/#1");
        let skill = Path::new("/Custom Apps/Game/skills/SKILL.md");
        let cli = Path::new("/Custom Apps/Game/bin/cli");
        for (agent, scheme, host, path, directory_key, prompt_key) in [
            (Agent::Claude, "claude", "code", "/new", "folder", "q"),
            (Agent::Codex, "codex", "new", "", "path", "prompt"),
        ] {
            let url = agent.link(skill, cli, home);
            assert_eq!(url.scheme(), scheme);
            assert_eq!(url.host_str(), Some(host));
            assert_eq!(url.path(), path);
            let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
            assert_eq!(pairs.len(), 2);
            assert_eq!(pairs[directory_key], home.to_string_lossy());
            let prompt = &pairs[prompt_key];
            assert!(prompt.contains(skill.to_str().unwrap()));
            assert!(prompt.contains(cli.to_str().unwrap()));
            assert!(prompt.contains(home.to_str().unwrap()));
            assert!(!prompt.contains("--home"));
            assert!(url.fragment().is_none());
        }
    }

    #[test]
    fn only_known_toolbar_requests_are_accepted() {
        assert_eq!(
            Agent::from_request(&Url::parse("bitcraft://agent/claude").unwrap()),
            Some(Agent::Claude)
        );
        assert_eq!(
            Agent::from_request(&Url::parse("bitcraft://agent/codex").unwrap()),
            Some(Agent::Codex)
        );
        for url in [
            "https://agent/claude",
            "bitcraft://other/claude",
            "bitcraft://agent/other",
            "bitcraft://agent/claude?folder=/other",
            "bitcraft://agent/codex#prompt",
            "bitcraft://user@agent/claude",
            "bitcraft://agent:42/codex",
        ] {
            assert_eq!(
                Agent::from_request(&Url::parse(url).unwrap()),
                None,
                "{url}"
            );
        }
    }
}
