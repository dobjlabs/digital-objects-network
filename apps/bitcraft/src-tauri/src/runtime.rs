use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bitcraft_launchers::{Connection, connection_path};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub use bitcraft_launchers::DEFAULT_HTTP_PORT as HTTP_PORT;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub release_id: String,
    pub plugin_sha256: String,
}

pub struct Runtime {
    child: Mutex<Option<Child>>,
    pub state_root: PathBuf,
    connection: Connection,
    connection_path: PathBuf,
}

fn state_root(home: &Path, release_id: &str) -> Result<PathBuf> {
    if release_id.is_empty()
        || !release_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        || release_id.starts_with('.')
    {
        bail!("Invalid packaged release ID");
    }
    Ok(home.join("environments").join(release_id).join("state"))
}

pub fn plugin_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn initialize(state: &Path, resources: &Path, release: &Release) -> Result<()> {
    let plugin = fs::read(resources.join("bitcraft.pexe"))?;
    if plugin_hash(&plugin) != release.plugin_sha256 {
        bail!("The bundled Bitcraft plugin does not match the release manifest");
    }
    fs::create_dir_all(state.join("actions"))?;
    let installed_plugin = state.join("actions/bitcraft.pexe");
    if installed_plugin.exists() {
        if plugin_hash(&fs::read(&installed_plugin)?) != release.plugin_sha256 {
            bail!("The installed Bitcraft plugin has changed; refusing to replace it");
        }
    } else {
        write_new(&installed_plugin, &plugin)?;
    }
    let settings = state.join("settings.json");
    if !settings.exists() {
        write_new(&settings, &fs::read(resources.join("settings.json"))?)?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}

impl Runtime {
    pub fn launch(home: &Path, resources: &Path, binary: &Path) -> Result<Self> {
        let release: Release = serde_json::from_slice(&fs::read(resources.join("release.json"))?)?;
        let state_root = state_root(home, &release.release_id)?;
        initialize(&state_root, resources, &release)?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(state_root.join("dobjd.log"))?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        let connection = Connection::new(state_root.clone(), instance_id.clone(), HTTP_PORT)?;
        let mut command = Command::new(binary);
        command
            .env("DOBJ_HOME", &state_root)
            .env("DOBJD_PORT", HTTP_PORT.to_string())
            .env("DOBJ_INSTANCE_ID", &instance_id)
            .current_dir(&state_root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command
            .spawn()
            .context("Could not start the bundled Digital Objects daemon")?;
        Ok(Self {
            child: Mutex::new(Some(child)),
            state_root,
            connection,
            connection_path: connection_path(home),
        })
    }

    pub async fn wait_until_ready(&self) -> Result<()> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            {
                let mut child = self.child.lock().unwrap();
                let Some(child) = child.as_mut() else {
                    bail!("The daemon has stopped");
                };
                if let Some(status) = child.try_wait()? {
                    bail!(
                        "The daemon exited ({status}). See {}",
                        self.state_root.join("dobjd.log").display()
                    );
                }
            }
            if let Ok(response) = client
                .get(format!("http://127.0.0.1:{HTTP_PORT}/healthz"))
                .send()
                .await
                && response.status().is_success()
                && let Ok(health) = response.json::<serde_json::Value>().await
            {
                if health["instance_id"].as_str() != Some(&self.connection.instance_id) {
                    bail!("Another daemon is using Bitcraft's port {HTTP_PORT}");
                }
                if health["ok"].as_bool() == Some(true) {
                    let mut child = self.child.lock().unwrap();
                    let child = child
                        .as_mut()
                        .context("The daemon stopped during startup")?;
                    if child.try_wait()?.is_some() {
                        bail!("The daemon exited during startup");
                    }
                    self.connection.publish(&self.connection_path)?;
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                bail!(
                    "Daemon startup timed out. See {}",
                    self.state_root.join("dobjd.log").display()
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub fn stop(&self) -> Result<()> {
        let mut child = self.child.lock().unwrap();
        let stopped = (|| -> Result<()> {
            if let Some(mut child) = child.take() {
                if child.try_wait()?.is_none() {
                    child.kill()?;
                }
                child.wait()?;
            }
            Ok(())
        })();
        let removed = self.connection.remove(&self.connection_path);
        stopped.and(removed)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_changes_get_fresh_state_and_reject_path_escape() {
        let home = Path::new("/tmp/bitcraft");
        assert_ne!(
            state_root(home, "0.1.0").unwrap(),
            state_root(home, "0.2.0").unwrap()
        );
        for release in ["", "..", "../other", "/other", "C:\\other"] {
            assert!(state_root(home, release).is_err());
        }
    }

    #[test]
    fn reinstall_preserves_objects_and_settings_and_rejects_plugin_drift() {
        let temp = tempfile::tempdir().unwrap();
        let resources = temp.path().join("runtime");
        let state = temp.path().join("state");
        fs::create_dir(&resources).unwrap();
        fs::write(resources.join("bitcraft.pexe"), b"plugin").unwrap();
        fs::write(resources.join("settings.json"), b"initial settings").unwrap();
        let release = Release {
            release_id: "test".into(),
            plugin_sha256: plugin_hash(b"plugin"),
        };
        initialize(&state, &resources, &release).unwrap();
        fs::create_dir(state.join("objects")).unwrap();
        fs::write(state.join("objects/object.dobj"), b"object").unwrap();
        fs::write(state.join("settings.json"), b"custom settings").unwrap();
        initialize(&state, &resources, &release).unwrap();
        assert_eq!(
            fs::read(state.join("settings.json")).unwrap(),
            b"custom settings"
        );
        assert_eq!(
            fs::read(state.join("objects/object.dobj")).unwrap(),
            b"object"
        );
        fs::write(state.join("actions/bitcraft.pexe"), b"changed").unwrap();
        assert!(initialize(&state, &resources, &release).is_err());
        assert_eq!(
            fs::read(state.join("actions/bitcraft.pexe")).unwrap(),
            b"changed"
        );
    }
}
