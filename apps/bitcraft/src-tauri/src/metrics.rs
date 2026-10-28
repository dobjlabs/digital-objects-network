use anyhow::{Context, Result};
use bitcraft_launchers::Connection;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    time::{Duration, Instant},
};
use tokio::sync::watch;

use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InventoryMetrics {
    pub total_bits: f64,
    pub bits_per_second: f64,
    #[serde(default)]
    pub object_values: BTreeMap<String, f64>,
    #[serde(default)]
    pub classes: BTreeMap<String, ClassMetrics>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClassMetrics {
    pub count: usize,
    pub total_bits: f64,
}

impl InventoryMetrics {
    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.total_bits.is_finite() && self.total_bits >= 0.0,
            "Total BIT must be finite and nonnegative"
        );
        anyhow::ensure!(self.bits_per_second.is_finite(), "BIT/sec must be finite");
        Ok(())
    }
}

const WINDOW: Duration = Duration::from_secs(300);

#[derive(Default)]
struct Samples(VecDeque<(Instant, f64)>);

impl Samples {
    fn update(&mut self, now: Instant, total: f64) -> InventoryMetrics {
        if self.0.back().is_none_or(|(_, previous)| *previous != total) {
            self.0.push_back((now, total));
        }
        while self.0.len() > 1 && now.duration_since(self.0[1].0) >= WINDOW {
            self.0.pop_front();
        }
        InventoryMetrics {
            total_bits: total,
            bits_per_second: (total - self.0[0].1) / WINDOW.as_secs_f64(),
            ..Default::default()
        }
    }
}

fn number(object: &Value, name: &str) -> Option<f64> {
    let value = object["fields"]["kvs"]
        .as_array()?
        .iter()
        .find(|entry| entry[0].as_str() == Some(name))?
        .get(1)?;
    let value = value
        .as_object()
        .and_then(|tags| (tags.len() == 1).then(|| tags.values().next()).flatten())
        .unwrap_or(value);
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn object_value(object: &Value, rules: &Value) -> f64 {
    if object["status"] != "live" || object["class"]["pluginName"] != "bitcraft" {
        return 0.0;
    }
    let Some(class) = object["class"]["name"].as_str() else {
        return 0.0;
    };
    if let Some(levels) = rules["LEVELED_TOTAL_BIT_VALUES"][class].as_array() {
        return number(object, "level")
            .filter(|level| *level >= 1.0 && level.fract() == 0.0)
            .and_then(|level| levels.get(level as usize - 1))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    }
    let value = rules["TOTAL_BIT_VALUES"][class].as_f64().unwrap_or(0.0)
        + rules["BUCKET_CONTENT_BIT_VALUES"][class]
            .as_f64()
            .unwrap_or(0.0)
            * number(object, "n").unwrap_or(0.0);
    match rules["FULL_DURABILITY"][class].as_f64() {
        Some(full) => value * number(object, "durability").unwrap_or(full) / full,
        None => value,
    }
}

fn publish(
    objects: &HashMap<String, Value>,
    rules: &Value,
    samples: &mut Samples,
    sender: &watch::Sender<Option<InventoryMetrics>>,
) -> Result<()> {
    let mut object_values = BTreeMap::new();
    let mut classes = BTreeMap::<String, ClassMetrics>::new();
    for (file_name, object) in objects {
        if object["status"] != "live" || object["class"]["pluginName"] != "bitcraft" {
            continue;
        }
        let name = object["class"]["name"]
            .as_str()
            .context("Object has no class name")?;
        let value = object_value(object, rules);
        anyhow::ensure!(
            value.is_finite() && value >= 0.0,
            "Invalid object inventory value"
        );
        object_values.insert(file_name.clone(), value);
        let class = classes.entry(name.to_owned()).or_default();
        class.count += 1;
        class.total_bits += value;
    }
    let mut metrics = samples.update(Instant::now(), object_values.values().sum());
    metrics.object_values = object_values;
    metrics.classes = classes;
    metrics.validate()?;
    sender.send_replace(Some(metrics));
    Ok(())
}

pub async fn run(connection: Connection, sender: watch::Sender<Option<InventoryMetrics>>) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let stream_client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let rules: Value =
        serde_json::from_str(include_str!("../../ui/inventory-values.json")).unwrap();
    loop {
        let result = follow(&connection, &client, &stream_client, &rules, &sender).await;
        sender.send_replace(None);
        if let Err(error) = result {
            eprintln!("Inventory metrics unavailable: {error:#}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn snapshot(
    connection: &Connection,
    client: &reqwest::Client,
) -> Result<HashMap<String, Value>> {
    connection.verify_with_client(client).await?;
    let objects: Vec<Value> = client
        .get(format!("{}/objects/unspent", connection.http_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    objects
        .into_iter()
        .map(|object| {
            let name = object["fileName"]
                .as_str()
                .context("Object has no file name")?
                .to_owned();
            Ok((name, object))
        })
        .collect()
}

async fn follow(
    connection: &Connection,
    client: &reqwest::Client,
    stream_client: &reqwest::Client,
    rules: &Value,
    sender: &watch::Sender<Option<InventoryMetrics>>,
) -> Result<()> {
    connection.verify_with_client(client).await?;
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        stream_client
            .get(format!("{}/events", connection.http_url))
            .send(),
    )
    .await??
    .error_for_status()?;
    let mut objects = snapshot(connection, client).await?;
    let mut samples = Samples::default();
    let mut pending = Vec::new();
    let mut sequence = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut verify = tokio::time::interval(Duration::from_secs(30));
    let mut last_chunk = Instant::now();
    publish(&objects, rules, &mut samples, sender)?;
    tick.tick().await;
    verify.tick().await;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                anyhow::ensure!(last_chunk.elapsed() < Duration::from_secs(35), "Inventory event stream stalled");
                publish(&objects, rules, &mut samples, sender)?;
            }
            _ = verify.tick() => { objects = snapshot(connection, client).await?; }
            chunk = stream.chunk() => {
                let bytes = chunk?.context("Inventory event stream closed")?;
                last_chunk = Instant::now();
                pending.extend_from_slice(&bytes);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<_> = pending.drain(..=end).collect();
                    let line = std::str::from_utf8(&line)?.trim_end();
                    if let Some(data) = line.strip_prefix("data:") {
                        let event: Value = serde_json::from_str(data.trim_start())?;
                        let next = event["seq"].as_u64().context("Missing event sequence")?;
                        if sequence.is_some_and(|last| next != last + 1) {
                            anyhow::bail!("Inventory event sequence gap");
                        }
                        sequence = Some(next);
                        match event["type"].as_str() {
                            Some("object") => {
                                let name = event["fileName"].as_str().context("Missing object name")?.to_owned();
                                if event["status"] == "nullified" { objects.remove(&name); }
                                else { objects.insert(name, event); }
                            }
                            Some("object-removed") => { if let Some(name) = event["fileName"].as_str() { objects.remove(name); } }
                            _ => continue,
                        }
                        publish(&objects, rules, &mut samples, sender)?;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn native_service_follows_inventory_without_a_webview() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let home = tempfile::tempdir().unwrap();
        let connection = Connection::new(
            home.path().to_owned(),
            "metrics-test".into(),
            listener.local_addr().unwrap().port(),
        )
        .unwrap();
        let (events, event_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let event_rx = std::sync::Arc::new(tokio::sync::Mutex::new(event_rx));
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let event_rx = event_rx.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                    }
                    if request.starts_with(b"GET /events ") {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
                        while let Some(event) = event_rx.lock().await.recv().await {
                            if socket
                                .write_all(format!("data: {event}\n\n").as_bytes())
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    } else {
                        let body = if request.starts_with(b"GET /healthz ") {
                            json!({"ok":true,"instance_id":"metrics-test"})
                        } else {
                            json!([])
                        }
                        .to_string();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        socket.write_all(response.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        let (sender, mut receiver) = watch::channel(None);
        let task = tokio::spawn(run(connection, sender));
        tokio::time::timeout(Duration::from_secs(3), receiver.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receiver.borrow().as_ref().unwrap().total_bits, 0.0);
        events.send(json!({"seq":1,"type":"object","fileName":"bit.dobj","class":{"pluginName":"bitcraft","name":"Bit"},"status":"live","fields":{"kvs":[]}})).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                receiver.changed().await.unwrap();
                if receiver
                    .borrow()
                    .as_ref()
                    .is_some_and(|metrics| metrics.total_bits == 1.0)
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        events
            .send(json!({"seq":3,"type":"object-removed","fileName":"bit.dobj"}))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                receiver.changed().await.unwrap();
                if receiver.borrow().is_none() {
                    break;
                }
            }
        })
        .await
        .unwrap();
        task.abort();
        server.abort();
    }

    #[test]
    fn native_snapshot_groups_only_live_bitcraft_objects_and_includes_zero_values() {
        let rules = serde_json::from_str(include_str!("../../ui/inventory-values.json")).unwrap();
        let mut objects = HashMap::new();
        for (file, class, status, plugin) in [
            ("knife", "Pocketknife", "live", "bitcraft"),
            ("wood1", "Wood", "live", "bitcraft"),
            ("wood2", "Wood", "live", "bitcraft"),
            ("pending", "Iron", "pending", "bitcraft"),
            ("other", "Wood", "live", "other"),
        ] {
            objects.insert(file.into(), json!({"class":{"pluginName":plugin,"name":class},"status":status,"fields":{"kvs":[]}}));
        }
        let (sender, receiver) = watch::channel(None);
        publish(&objects, &rules, &mut Samples::default(), &sender).unwrap();
        let snapshot = receiver.borrow().clone().unwrap();
        assert_eq!(snapshot.total_bits, 2.0);
        assert_eq!(snapshot.object_values.len(), 3);
        assert_eq!(snapshot.object_values["knife"], 0.0);
        assert_eq!(snapshot.classes["Pocketknife"].total_bits, 0.0);
        assert_eq!(snapshot.classes["Wood"].count, 2);
        assert_eq!(snapshot.classes["Wood"].total_bits, 2.0);
        assert_eq!(
            snapshot
                .classes
                .values()
                .map(|class| class.total_bits)
                .sum::<f64>(),
            snapshot.total_bits
        );
    }

    #[test]
    fn rate_decays_without_a_page_and_resets_after_a_gap() {
        let start = Instant::now();
        let mut samples = Samples::default();
        assert_eq!(samples.update(start, 0.0).bits_per_second, 0.0);
        assert_eq!(
            samples
                .update(start + Duration::from_secs(1), 300.0)
                .bits_per_second,
            1.0
        );
        assert_eq!(
            samples
                .update(start + Duration::from_secs(301), 300.0)
                .bits_per_second,
            0.0
        );
        assert_eq!(Samples::default().update(start, 300.0).bits_per_second, 0.0);
    }
    #[test]
    fn shared_rules_value_live_objects_levels_contents_and_wear() {
        let rules = serde_json::from_str(include_str!("../../ui/inventory-values.json")).unwrap();
        for (class, fields, expected) in [
            ("Quarry", json!([["level", {"Int":"3"}]]), 300.0),
            ("BucketOfIron", json!([["n", {"Int":"10"}]]), 55.0),
            ("IronAxe", json!([["durability", {"Int":"50"}]]), 4.5),
        ] {
            let mut object = json!({"class":{"pluginName":"bitcraft","name":class},"status":"live","fields":{"kvs":fields}});
            assert_eq!(object_value(&object, &rules), expected);
            object["status"] = json!("pending");
            assert_eq!(object_value(&object, &rules), 0.0);
        }
    }
}
