mod ipc;

use anyhow::{Context, Result};
use bitcraft_launchers::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

const UPDATE_INTERVAL: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const IPC_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    #[serde(default)]
    share_activity: bool,
}

pub struct Presence {
    path: PathBuf,
    application_id: Option<String>,
    enabled: watch::Sender<Option<bool>>,
    receiver: Mutex<Option<watch::Receiver<Option<bool>>>>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl Presence {
    pub fn new(home: &std::path::Path) -> Self {
        let path = home.join("desktop.json");
        let preferences = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes).map_err(anyhow::Error::from),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Preferences::default())
            }
            Err(error) => Err(error.into()),
        }
        .unwrap_or_else(|error| {
            eprintln!("Could not read desktop preferences; Discord sharing is off: {error}");
            Preferences::default()
        });
        let raw_id = std::env::var("BITCRAFT_DISCORD_APPLICATION_ID")
            .ok()
            .or_else(|| option_env!("BITCRAFT_DISCORD_APPLICATION_ID").map(str::to_owned));
        let application_id = raw_id.and_then(|id| {
            if valid_application_id(&id) {
                Some(id)
            } else {
                eprintln!("BITCRAFT_DISCORD_APPLICATION_ID must be a positive numeric Discord application ID");
                None
            }
        });
        let (enabled, receiver) = watch::channel(Some(preferences.share_activity));
        Self {
            path,
            application_id,
            enabled,
            receiver: Mutex::new(Some(receiver)),
            task: Mutex::new(None),
        }
    }

    pub fn available(&self) -> bool {
        self.application_id.is_some()
    }

    pub fn enabled(&self) -> bool {
        self.enabled.borrow().unwrap_or(false)
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("No desktop preferences directory")?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&serde_json::to_vec_pretty(&Preferences {
            share_activity: enabled,
        })?)?;
        temporary.persist(&self.path)?;
        self.enabled.send_replace(Some(enabled));
        Ok(())
    }

    pub fn start(&self, connection: Connection) {
        let Some(application_id) = self.application_id.clone() else {
            return;
        };
        let Some(receiver) = self.receiver.lock().unwrap().take() else {
            return;
        };
        *self.task.lock().unwrap() = Some(tauri::async_runtime::spawn(worker(
            connection,
            receiver,
            move || {
                let application_id = application_id.clone();
                async move { ipc::Client::connect(&application_id).await }
            },
        )));
    }

    pub fn stop(&self) {
        self.enabled.send_replace(None);
        if let Some(mut task) = self.task.lock().unwrap().take() {
            tauri::async_runtime::block_on(async {
                if tokio::time::timeout(Duration::from_secs(2), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                }
            });
        }
    }
}

fn valid_application_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && id.parse::<u64>().is_ok_and(|number| number > 0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    action: Action,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Action {
    plugin_name: String,
    name: String,
}

fn activity(runs: &[Run], started_at: u64) -> Value {
    let active: Vec<_> = runs
        .iter()
        .filter(|run| {
            run.action.plugin_name == "bitcraft"
                && matches!(
                    run.status.as_str(),
                    "queued" | "generateProof" | "committing"
                )
        })
        .collect();
    let details = match active.as_slice() {
        [] => "Managing inventory".to_owned(),
        [run] => {
            let name = action_label(&run.action.name);
            match run.status.as_str() {
                "queued" => format!("Queued: {name}"),
                "committing" => format!("Confirming: {name}"),
                _ => name,
            }
        }
        _ => "Crafting resources".to_owned(),
    };
    json!({
        "type": 0,
        "details": details.chars().take(128).collect::<String>(),
        "state": match active.len() {
            0 => "No active jobs".to_owned(),
            1 => "1 active job".to_owned(),
            count => format!("{count} active jobs"),
        },
        "timestamps": {"start": started_at},
        "assets": {"large_image": "bitcraft", "large_text": "Bitcraft"},
        "buttons": [
            {"label": "Website", "url": "https://bitcraft.sh/"},
            {"label": "Join our Discord", "url": "https://discord.gg/cWtZZYrer"}
        ]
    })
}

fn action_label(name: &str) -> String {
    let mut label = String::new();
    let mut previous_lowercase = false;
    for character in name.chars() {
        if character.is_uppercase() && previous_lowercase {
            label.push(' ');
        }
        label.push(character);
        previous_lowercase = character.is_lowercase() || character.is_ascii_digit();
    }
    label
}

#[derive(Default)]
struct Updates {
    last_attempt: Option<Instant>,
    published: Option<Value>,
}

impl Updates {
    fn due(&self, now: Instant, activity: &Value) -> bool {
        self.published.as_ref() != Some(activity)
            && self
                .last_attempt
                .is_none_or(|last| now.duration_since(last) >= UPDATE_INTERVAL)
    }
}

async fn worker<F>(
    connection: Connection,
    mut enabled: watch::Receiver<Option<bool>>,
    mut connect: impl FnMut() -> F,
) where
    F: std::future::Future<Output = Result<ipc::Client>>,
{
    let client = match reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Could not initialize Discord presence: {error}");
            return;
        }
    };
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut updates = Updates::default();
    let mut delay = UPDATE_INTERVAL;
    let mut last_error = String::new();
    loop {
        let Some(sharing) = *enabled.borrow_and_update() else {
            return;
        };
        if !sharing {
            if enabled.changed().await.is_err() {
                return;
            }
            continue;
        }
        let mut ipc = None;
        let outcome = tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() { return; }
                None
            }
            result = drive(connect(), &connection, &client, &mut ipc, &mut updates, started_at) => Some(result),
        };
        let retry_after = outcome
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .and_then(|error| error.downcast_ref::<ipc::Rejected>())
            .map(|error| error.retry_after);
        if retry_after.is_none()
            && let Some(ipc) = ipc.as_mut()
        {
            let _ =
                tokio::time::timeout(Duration::from_secs(1), ipc.set_activity(Value::Null)).await;
        }
        drop(ipc);
        updates.published = None;
        match outcome {
            None => {
                delay = UPDATE_INTERVAL;
            }
            Some(Err(error)) => {
                let message = error.to_string();
                if message != last_error {
                    eprintln!("Discord presence unavailable: {message}");
                    last_error = message;
                }
                let wait = delay.max(retry_after.unwrap_or_default());
                tokio::select! {
                    changed = enabled.changed() => {
                        if changed.is_err() { return; }
                    }
                    _ = tokio::time::sleep(wait) => {}
                }
                delay = (delay * 2).min(Duration::from_secs(60));
            }
            Some(Ok(())) => return,
        }
    }
}

async fn drive(
    connect: impl std::future::Future<Output = Result<ipc::Client>>,
    connection: &Connection,
    client: &reqwest::Client,
    ipc: &mut Option<ipc::Client>,
    updates: &mut Updates,
    started_at: u64,
) -> Result<()> {
    *ipc = Some(tokio::time::timeout(IPC_TIMEOUT, connect).await??);
    let ipc = ipc.as_mut().unwrap();
    let mut last_contact = Instant::now();
    loop {
        connection.verify().await?;
        let runs: Vec<Run> = client
            .get(format!("{}/runs?status=active", connection.http_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let activity = activity(&runs, started_at);
        let now = Instant::now();
        if updates.due(now, &activity) {
            updates.last_attempt = Some(now);
            tokio::time::timeout(IPC_TIMEOUT, ipc.set_activity(activity.clone())).await??;
            updates.published = Some(activity);
            last_contact = Instant::now();
        } else if last_contact.elapsed() >= UPDATE_INTERVAL {
            tokio::time::timeout(IPC_TIMEOUT, ipc.ping()).await??;
            last_contact = Instant::now();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(value: Value) -> Vec<Run> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn status_tracks_concurrent_bitcraft_work_without_exposing_inputs() {
        let work = runs(json!([
            {"action": {"pluginName": "other", "name": "Secret"}, "status": "generateProof"},
            {"action": {"pluginName": "bitcraft", "name": "CraftWoodenAxe"}, "status": "generateProof", "inputObjectPaths": ["private.dobj"]},
            {"action": {"pluginName": "bitcraft", "name": "GatherWood"}, "status": "succeeded"}
        ]));
        let single = activity(&work, 42);
        assert_eq!(single["details"], "Craft Wooden Axe");
        assert_eq!(single["state"], "1 active job");
        assert_eq!(single["timestamps"]["start"], 42);
        assert!(!single.to_string().contains("private"));
        let mut concurrent = runs(json!([
            {"action": {"pluginName": "bitcraft", "name": "CraftBit"}, "status": "committing"}
        ]));
        assert_eq!(
            activity(&concurrent, 42)["details"],
            "Confirming: Craft Bit"
        );
        concurrent.extend(work);
        assert_eq!(activity(&concurrent, 42)["state"], "2 active jobs");
        assert_eq!(activity(&[], 42)["state"], "No active jobs");
    }

    #[test]
    fn updates_coalesce_to_the_latest_state_and_skip_duplicates() {
        let start = Instant::now();
        let mut updates = Updates::default();
        let idle = activity(&[], 42);
        assert!(updates.due(start, &idle));
        updates.last_attempt = Some(start);
        updates.published = Some(idle.clone());
        let busy = json!({"details": "Crafting resources"});
        assert!(!updates.due(start + Duration::from_secs(14), &busy));
        assert!(updates.due(start + UPDATE_INTERVAL, &busy));
        assert!(!updates.due(start + UPDATE_INTERVAL, &idle));
        updates.published = None;
        assert!(!updates.due(start + Duration::from_secs(1), &idle));
        assert!(updates.due(start + UPDATE_INTERVAL, &idle));
    }

    #[test]
    fn sharing_defaults_off_and_persists_in_the_selected_home() {
        let home = tempfile::tempdir().unwrap();
        let presence = Presence::new(home.path());
        assert!(!presence.enabled());
        assert!(!home.path().join("desktop.json").exists());
        presence.set_enabled(true).unwrap();
        assert!(Presence::new(home.path()).enabled());
        presence.set_enabled(false).unwrap();
        assert!(!Presence::new(home.path()).enabled());
        fs::write(home.path().join("desktop.json"), b"invalid").unwrap();
        assert!(!Presence::new(home.path()).enabled());
        for id in ["", "0", "-1", "12x", "123 ", "18446744073709551616"] {
            assert!(!valid_application_id(id), "{id}");
        }
        assert!(valid_application_id("123456789012345678"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opt_in_opt_out_reenable_and_shutdown_control_the_live_connection() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connection =
            Connection::new(home.path().to_path_buf(), "test-instance".into(), port).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_count = requests.clone();
        let daemon = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                request_count.fetch_add(1, Ordering::Relaxed);
                let body = if request.starts_with(b"GET /healthz ") {
                    json!({"ok": true, "instance_id": "test-instance"})
                } else {
                    assert!(request.starts_with(b"GET /runs?status=active "));
                    json!([{"action": {"pluginName": "bitcraft", "name": "GatherWood"}, "status": "generateProof"}])
                }.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let (sharing, receiver) = watch::channel(Some(false));
        let (activity_sender, mut activities) = tokio::sync::mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let connection_count = connections.clone();
        let task = tokio::spawn(worker(connection, receiver, move || {
            connection_count.fetch_add(1, Ordering::Relaxed);
            let client = ipc::mock_client(activity_sender.clone());
            async move { Ok(client) }
        }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(connections.load(Ordering::Relaxed), 0);
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        sharing.send_replace(Some(true));
        let first = tokio::time::timeout(Duration::from_secs(2), activities.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first["details"], "Gather Wood");
        sharing.send_replace(Some(false));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), activities.recv())
                .await
                .unwrap(),
            Some(Value::Null)
        );
        let stopped_requests = requests.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(requests.load(Ordering::Relaxed), stopped_requests);
        sharing.send_replace(Some(true));
        let second = tokio::time::timeout(Duration::from_secs(20), activities.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second["details"], "Gather Wood");
        assert_eq!(second["timestamps"], first["timestamps"]);
        assert_eq!(connections.load(Ordering::Relaxed), 2);
        sharing.send_replace(None);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), activities.recv())
                .await
                .unwrap(),
            Some(Value::Null)
        );
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        daemon.abort();
    }
}
