use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bitcraft_launchers::{Connection, connection_path};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub use bitcraft_launchers::DEFAULT_HTTP_PORT as HTTP_PORT;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub plugin_sha256: String,
}

pub struct Runtime {
    child: Mutex<Option<Child>>,
    pub state_root: PathBuf,
    connection: Connection,
    connection_path: PathBuf,
}

pub fn plugin_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn initialize(state: &Path, resources: &Path, release: &Release) -> Result<()> {
    let plugin = fs::read(resources.join("bitcraft.pexe"))?;
    if plugin_hash(&plugin) != release.plugin_sha256 {
        bail!("The bundled Bitcraft plugin does not match the release manifest");
    }
    let actions = state.join("actions");
    fs::create_dir_all(&actions)?;
    let installed_plugin = actions.join("bitcraft.pexe");
    let installed = match fs::read(&installed_plugin) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if installed.as_deref() != Some(plugin.as_slice()) {
        if let Some(previous) = installed {
            let archive = actions.join("archive");
            fs::create_dir_all(&archive)?;
            write_atomic(
                &archive.join(format!("bitcraft-{}.pexe", plugin_hash(&previous))),
                &previous,
            )?;
        }
        write_atomic(&installed_plugin, &plugin)?;
    }
    let settings = state.join("settings.json");
    if !settings.exists() {
        write_new(&settings, &fs::read(resources.join("settings.json"))?)?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent().context("File path has no parent directory")?,
    )?;
    temporary.write_all(bytes)?;
    temporary.persist(path)?;
    Ok(())
}

pub(crate) fn daemon_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
}

pub(crate) async fn get_verified_json<T: DeserializeOwned>(
    connection: &Connection,
    client: &reqwest::Client,
    path: &str,
) -> Result<T> {
    connection.verify_with_client(client).await?;
    Ok(client
        .get(format!(
            "{}{path}",
            connection.http_url.trim_end_matches('/')
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)) {
            Ok(_) => return Ok(()),
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Bitcraft's HTTP port {port} is unavailable"));
            }
        }
    }
}

impl Runtime {
    pub fn connection(&self) -> Connection {
        self.connection.clone()
    }

    pub fn launch(home: &Path, resources: &Path, binary: &Path) -> Result<Self> {
        let release: Release = serde_json::from_slice(&fs::read(resources.join("release.json"))?)?;
        let state_root = home.to_path_buf();
        wait_for_port(HTTP_PORT, Duration::from_secs(3))?;
        initialize(&state_root, resources, &release)?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(state_root.join("dobjd.log"))?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        let connection = Connection::new(state_root.clone(), instance_id.clone(), HTTP_PORT)?;
        let mut command = Command::new(binary);
        command
            .arg("--exit-on-stdin-close")
            .env("DOBJ_HOME", &state_root)
            .env("DOBJD_PORT", HTTP_PORT.to_string())
            .env("DOBJ_INSTANCE_ID", &instance_id)
            .current_dir(&state_root)
            .stdin(Stdio::piped())
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
        let client = daemon_client()?;
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
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
    fn startup_waits_for_a_port_to_be_released() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(wait_for_port(port, Duration::from_millis(30)).is_err());
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(listener);
        });
        wait_for_port(port, Duration::from_secs(1)).unwrap();
        release.join().unwrap();
    }

    #[test]
    fn plugin_updates_preserve_objects_settings_and_other_actions() {
        let temp = tempfile::tempdir().unwrap();
        let resources = temp.path().join("runtime");
        let state = temp.path().join("state");
        fs::create_dir(&resources).unwrap();
        fs::write(resources.join("bitcraft.pexe"), b"plugin").unwrap();
        fs::write(resources.join("settings.json"), b"initial settings").unwrap();
        let mut release = Release {
            plugin_sha256: plugin_hash(b"plugin"),
        };
        initialize(&state, &resources, &release).unwrap();
        fs::create_dir(state.join("objects")).unwrap();
        fs::write(state.join("objects/object.dobj"), b"object").unwrap();
        fs::write(state.join("settings.json"), b"custom settings").unwrap();
        fs::write(state.join("actions/other.pexe"), b"other plugin").unwrap();
        fs::write(resources.join("settings.json"), b"new defaults").unwrap();
        fs::write(resources.join("bitcraft.pexe"), b"new plugin").unwrap();
        release.plugin_sha256 = plugin_hash(b"new plugin");
        initialize(&state, &resources, &release).unwrap();
        initialize(&state, &resources, &release).unwrap();
        assert_eq!(
            fs::read(state.join("settings.json")).unwrap(),
            b"custom settings"
        );
        assert_eq!(
            fs::read(state.join("objects/object.dobj")).unwrap(),
            b"object"
        );
        assert_eq!(
            fs::read(state.join("actions/bitcraft.pexe")).unwrap(),
            b"new plugin"
        );
        assert_eq!(
            fs::read(state.join("actions/other.pexe")).unwrap(),
            b"other plugin"
        );
        let archive = state.join("actions/archive");
        assert_eq!(
            fs::read(archive.join(format!("bitcraft-{}.pexe", plugin_hash(b"plugin")))).unwrap(),
            b"plugin"
        );
        assert_eq!(fs::read_dir(&archive).unwrap().count(), 1);
    }

    #[test]
    fn invalid_bundled_plugin_leaves_installed_files_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let resources = temp.path().join("runtime");
        let state = temp.path().join("state");
        fs::create_dir(&resources).unwrap();
        fs::write(resources.join("bitcraft.pexe"), b"plugin").unwrap();
        fs::write(resources.join("settings.json"), b"initial settings").unwrap();
        let release = Release {
            plugin_sha256: plugin_hash(b"plugin"),
        };
        initialize(&state, &resources, &release).unwrap();
        fs::write(state.join("settings.json"), b"custom settings").unwrap();
        fs::write(resources.join("bitcraft.pexe"), b"tampered plugin").unwrap();
        assert!(initialize(&state, &resources, &release).is_err());
        assert_eq!(
            fs::read(state.join("actions/bitcraft.pexe")).unwrap(),
            b"plugin"
        );
        assert_eq!(
            fs::read(state.join("settings.json")).unwrap(),
            b"custom settings"
        );
        assert!(!state.join("actions/archive").exists());
    }
}
