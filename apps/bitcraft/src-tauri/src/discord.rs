mod ipc;

use anyhow::{Context, Result};
use bitcraft_launchers::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

use crate::{metrics::InventoryMetrics, runtime};

const UPDATE_INTERVAL: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const IPC_TIMEOUT: Duration = Duration::from_secs(2);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

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
            .filter(|id| !id.trim().is_empty())
            .or_else(|| {
                option_env!("BITCRAFT_DISCORD_APPLICATION_ID")
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_owned)
            });
        let application_id = raw_id.and_then(|id| {
            if valid_application_id(&id) {
                Some(id)
            } else {
                eprintln!("BITCRAFT_DISCORD_APPLICATION_ID must be a positive numeric Discord application ID");
                None
            }
        });
        Self {
            path,
            application_id,
            enabled: watch::Sender::new(Some(preferences.share_activity)),
            task: Mutex::new(None),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.borrow().unwrap_or(false)
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.enabled.send_replace(Some(enabled));
        fs::create_dir_all(
            self.path
                .parent()
                .context("No desktop preferences directory")?,
        )?;
        runtime::write_atomic(
            &self.path,
            &serde_json::to_vec_pretty(&Preferences {
                share_activity: enabled,
            })?,
        )
    }

    pub fn start(
        &self,
        connection: Connection,
        inventory_metrics: watch::Receiver<Option<InventoryMetrics>>,
    ) {
        let Some(application_id) = self.application_id.clone() else {
            return;
        };
        *self.task.lock().unwrap() = Some(tauri::async_runtime::spawn(worker(
            connection,
            self.enabled.subscribe(),
            inventory_metrics,
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

fn format_bits(value: f64) -> String {
    if value.abs() < 0.005 {
        "0.00".into()
    } else {
        format!("{value:.2}")
    }
}

fn activity(runs: &[Run], started_at: u64, inventory_metrics: Option<&InventoryMetrics>) -> Value {
    let active: Vec<_> = runs
        .iter()
        .filter(|run| run.action.plugin_name == "bitcraft")
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
    let jobs = match active.len() {
        0 => "No active jobs".to_owned(),
        1 => "1 active job".to_owned(),
        count => format!("{count} active jobs"),
    };
    // The leaderboard bot (apps/bitcraft/discord-bot) reads the total from this state.
    let state = match inventory_metrics {
        Some(metrics) => format!(
            "{} BIT | {} BIT/sec | {jobs}",
            format_bits(metrics.total_bits),
            format_bits(metrics.bits_per_second)
        ),
        None => jobs,
    };
    json!({
        "type": 0,
        "details": details.chars().take(128).collect::<String>(),
        "state": state,
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
    inventory_metrics: watch::Receiver<Option<InventoryMetrics>>,
    mut connect: impl FnMut() -> F,
) where
    F: std::future::Future<Output = Result<ipc::Client>>,
{
    let client = match runtime::daemon_client() {
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
    let mut delay = RECONNECT_INITIAL_DELAY;
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
            Err(error) = drive(connect(), &connection, &client, &mut ipc, started_at, &inventory_metrics, &mut delay) => Some(error),
        };
        let retry_after = outcome
            .as_ref()
            .and_then(|error| error.downcast_ref::<ipc::Rejected>())
            .map(|error| error.retry_after);
        if retry_after.is_none()
            && let Some(ipc) = ipc.as_mut()
        {
            let _ =
                tokio::time::timeout(Duration::from_secs(1), ipc.set_activity(Value::Null)).await;
        }
        drop(ipc);
        match outcome {
            None => {
                delay = RECONNECT_INITIAL_DELAY;
            }
            Some(error) => {
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
                delay = (delay * 2).min(RECONNECT_MAX_DELAY);
            }
        }
    }
}

async fn drive(
    connect: impl std::future::Future<Output = Result<ipc::Client>>,
    connection: &Connection,
    client: &reqwest::Client,
    ipc: &mut Option<ipc::Client>,
    started_at: u64,
    inventory_metrics: &watch::Receiver<Option<InventoryMetrics>>,
    retry_delay: &mut Duration,
) -> Result<Infallible> {
    let ipc = ipc.insert(connect.await?);
    let mut updates = Updates::default();
    let mut daemon_failed_at: Option<Instant> = None;
    loop {
        let snapshot =
            runtime::get_verified_json::<Vec<Run>>(connection, client, "/runs?status=active").await;
        let activity = match snapshot {
            Ok(runs) => {
                daemon_failed_at = None;
                activity(&runs, started_at, inventory_metrics.borrow().as_ref())
            }
            Err(_) => {
                let since = daemon_failed_at.get_or_insert_with(Instant::now);
                if since.elapsed() >= UPDATE_INTERVAL && updates.published.is_some() {
                    tokio::time::timeout(IPC_TIMEOUT, ipc.set_activity(Value::Null)).await??;
                    updates.published = None;
                } else {
                    tokio::time::timeout(IPC_TIMEOUT, ipc.ping()).await??;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
        };
        let now = Instant::now();
        if updates.due(now, &activity) {
            updates.last_attempt = Some(now);
            tokio::time::timeout(IPC_TIMEOUT, ipc.set_activity(activity.clone())).await??;
            updates.published = Some(activity);
        } else {
            tokio::time::timeout(IPC_TIMEOUT, ipc.ping()).await??;
        }
        *retry_delay = RECONNECT_INITIAL_DELAY;
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
            {"action": {"pluginName": "bitcraft", "name": "CraftWoodenAxe"}, "status": "generateProof", "inputObjectPaths": ["private.dobj"]}
        ]));
        let single = activity(&work, 42, None);
        assert_eq!(single["details"], "Craft Wooden Axe");
        assert_eq!(single["state"], "1 active job");
        assert_eq!(single["timestamps"]["start"], 42);
        assert!(!single.to_string().contains("private"));
        let mut concurrent = runs(json!([
            {"action": {"pluginName": "bitcraft", "name": "CraftBit"}, "status": "committing"}
        ]));
        assert_eq!(
            activity(&concurrent, 42, None)["details"],
            "Confirming: Craft Bit"
        );
        concurrent.extend(work);
        assert_eq!(activity(&concurrent, 42, None)["state"], "2 active jobs");
        assert_eq!(activity(&[], 42, None)["state"], "No active jobs");
    }

    #[test]
    fn updates_coalesce_to_the_latest_state_and_skip_duplicates() {
        let start = Instant::now();
        let mut updates = Updates::default();
        let idle = activity(&[], 42, None);
        assert!(updates.due(start, &idle));
        updates.last_attempt = Some(start);
        updates.published = Some(idle.clone());
        let value = InventoryMetrics {
            total_bits: 123.45,
            bits_per_second: 0.37,
        };
        let busy = activity(&[], 42, Some(&value));
        assert!(!updates.due(start + Duration::from_secs(14), &busy));
        assert!(updates.due(start + UPDATE_INTERVAL, &busy));
        assert!(!updates.due(start + UPDATE_INTERVAL, &idle));
        updates.published = None;
        assert!(!updates.due(start + Duration::from_secs(1), &idle));
        assert!(updates.due(start + UPDATE_INTERVAL, &idle));
    }

    #[test]
    fn presence_includes_total_and_rate() {
        let metrics = InventoryMetrics {
            total_bits: 31.125,
            bits_per_second: -0.375,
        };
        assert_eq!(
            activity(&[], 42, Some(&metrics))["state"],
            "31.12 BIT | -0.38 BIT/sec | No active jobs"
        );
        for value in [0.0, -0.0, 0.001, -0.001] {
            let metrics = InventoryMetrics {
                total_bits: 0.0,
                bits_per_second: value,
            };
            assert_eq!(
                activity(&[], 42, Some(&metrics))["state"],
                "0.00 BIT | 0.00 BIT/sec | No active jobs"
            );
        }
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

    #[test]
    fn opting_out_takes_effect_even_when_preferences_cannot_be_saved() {
        let home = tempfile::tempdir().unwrap();
        let presence = Presence::new(home.path());
        presence.set_enabled(true).unwrap();
        fs::remove_file(home.path().join("desktop.json")).unwrap();
        fs::create_dir(home.path().join("desktop.json")).unwrap();
        assert!(presence.set_enabled(false).is_err());
        assert!(!presence.enabled());
    }

    #[cfg(unix)]
    async fn mock_daemon(
        home: &std::path::Path,
    ) -> (
        Connection,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connection = Connection::new(home.to_path_buf(), "test-instance".into(), port).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_count = requests.clone();
        let fail_health = home.join("fail-health");
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
                let status = if request.starts_with(b"GET /healthz ") && fail_health.exists() {
                    "503 Unavailable"
                } else {
                    "200 OK"
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (connection, requests, daemon)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn transient_daemon_failure_keeps_the_discord_connection_and_activity() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let home = tempfile::tempdir().unwrap();
        let (connection, requests, daemon) = mock_daemon(home.path()).await;
        let (sharing, receiver) = watch::channel(Some(true));
        let (_metrics, metrics) = watch::channel(None);
        let (sender, mut activities) = tokio::sync::mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let task = tokio::spawn(worker(connection, receiver, metrics, move || {
            count.fetch_add(1, Ordering::Relaxed);
            let (client, _) = ipc::mock_client(sender.clone());
            async move { Ok(client) }
        }));
        tokio::time::timeout(Duration::from_secs(3), activities.recv())
            .await
            .unwrap()
            .unwrap();
        let before = requests.load(Ordering::Relaxed);
        fs::write(home.path().join("fail-health"), b"").unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            while requests.load(Ordering::Relaxed) == before {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(connections.load(Ordering::Relaxed), 1);
        assert!(activities.try_recv().is_err());
        fs::remove_file(home.path().join("fail-health")).unwrap();
        sharing.send_replace(None);
        task.await.unwrap();
        daemon.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discord_restarts_republish_unchanged_activity_without_toggling_sharing() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let home = tempfile::tempdir().unwrap();
        let (connection, _, daemon) = mock_daemon(home.path()).await;
        let (sharing, receiver) = watch::channel(Some(true));
        let (_metrics_sender, inventory_metrics) = watch::channel(Some(InventoryMetrics {
            total_bits: 31.0,
            bits_per_second: 0.37,
        }));
        let online = Arc::new(AtomicBool::new(false));
        let discord_online = online.clone();
        let (activity_sender, mut activities) = tokio::sync::mpsc::unbounded_channel();
        let (server_sender, mut servers) = tokio::sync::mpsc::unbounded_channel();
        let (attempt_sender, mut attempts) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(worker(connection, receiver, inventory_metrics, move || {
            let online = discord_online.load(Ordering::Relaxed);
            attempt_sender.send(()).unwrap();
            let activity_sender = activity_sender.clone();
            let server_sender = server_sender.clone();
            async move {
                anyhow::ensure!(online, "Discord is not running");
                let (client, server) = ipc::mock_client(activity_sender);
                server_sender.send(server).unwrap();
                Ok(client)
            }
        }));
        assert!(attempts.recv().await.is_some());
        tokio::time::timeout(Duration::from_secs(4), attempts.recv())
            .await
            .unwrap()
            .unwrap();
        online.store(true, Ordering::Relaxed);
        let first = tokio::time::timeout(Duration::from_secs(4), activities.recv())
            .await
            .unwrap()
            .unwrap();
        let server = servers.recv().await.unwrap();
        assert_eq!(first["state"], "31.00 BIT | 0.37 BIT/sec | 1 active job");
        while attempts.try_recv().is_ok() {}

        online.store(false, Ordering::Relaxed);
        server.abort();
        tokio::time::timeout(Duration::from_secs(9), attempts.recv())
            .await
            .unwrap()
            .unwrap();
        online.store(true, Ordering::Relaxed);
        let second = tokio::time::timeout(Duration::from_secs(4), activities.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second, first);
        let server = servers.recv().await.unwrap();
        server.abort();
        let third = tokio::time::timeout(Duration::from_secs(9), activities.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(third, first);
        assert_eq!(*sharing.borrow(), Some(true));

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

    #[cfg(unix)]
    #[tokio::test]
    async fn opt_in_opt_out_reenable_and_shutdown_control_the_live_connection() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let home = tempfile::tempdir().unwrap();
        let (connection, requests, daemon) = mock_daemon(home.path()).await;
        let (sharing, receiver) = watch::channel(Some(false));
        let (activity_sender, mut activities) = tokio::sync::mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let connection_count = connections.clone();
        let (inventory_sender, inventory_receiver) = watch::channel(Some(InventoryMetrics {
            total_bits: 12.5,
            bits_per_second: 0.25,
        }));
        let task = tokio::spawn(worker(
            connection,
            receiver,
            inventory_receiver,
            move || {
                connection_count.fetch_add(1, Ordering::Relaxed);
                let (client, _) = ipc::mock_client(activity_sender.clone());
                async move { Ok(client) }
            },
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(connections.load(Ordering::Relaxed), 0);
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        sharing.send_replace(Some(true));
        let first = tokio::time::timeout(Duration::from_secs(2), activities.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first["details"], "Gather Wood");
        assert_eq!(first["state"], "12.50 BIT | 0.25 BIT/sec | 1 active job");
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
        inventory_sender.send_replace(Some(InventoryMetrics {
            total_bits: 100.125,
            bits_per_second: -0.5,
        }));
        sharing.send_replace(Some(true));
        let second = tokio::time::timeout(Duration::from_secs(20), activities.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second["details"], "Gather Wood");
        assert_eq!(second["state"], "100.12 BIT | -0.50 BIT/sec | 1 active job");
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
