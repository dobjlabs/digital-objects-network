use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde_json::Value;

use crate::client::DobjdClient;
use crate::types::{
    AppSettings, LoadGuiInventoryResult, ObjectsDir, RunActionInput, RunActionRequest,
    RunActionResult,
};

const TARGET_RUN_ACTION_PROGRESS: &str = "run-action-progress";

pub async fn inventory(client: &DobjdClient, json: bool) -> Result<()> {
    let result: LoadGuiInventoryResult = client.get_json("/inventory").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "inventory": result.inventory.iter().map(|o| serde_json::json!({
                "id": o.id, "fileName": o.file_name, "className": o.class_name,
                "status": o.status, "txHash": o.tx_hash, "grounded": o.grounded,
            })).collect::<Vec<_>>(),
        }))?);
        return Ok(());
    }

    if result.inventory.is_empty() {
        println!("(no objects in inventory)");
        return Ok(());
    }
    for obj in &result.inventory {
        let grounded = if obj.grounded { "✓" } else { " " };
        println!(
            "[{:<10}] {} {} {:<14} id={}",
            obj.status, grounded, obj.emoji, obj.class_name, short_hex(&obj.id),
        );
    }
    Ok(())
}

pub async fn actions(client: &DobjdClient, json: bool) -> Result<()> {
    let result: LoadGuiInventoryResult = client.get_json("/inventory").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result.actions.iter().map(|a| serde_json::json!({
            "id": a.id, "description": a.description, "inputs": a.total_input_classes,
        })).collect::<Vec<_>>())?);
        return Ok(());
    }
    for action in &result.actions {
        let inputs = if action.total_input_classes.is_empty() {
            "(no inputs)".to_string()
        } else {
            action.total_input_classes.join(", ")
        };
        println!(
            "{} {:<24} {} — {}",
            action.emoji, action.id, inputs, action.description,
        );
    }
    Ok(())
}

pub async fn state_root(client: &DobjdClient) -> Result<()> {
    let root = client.get_text("/state-root").await?;
    println!("{}", root.trim_matches('"'));
    Ok(())
}

pub async fn objects_dir(client: &DobjdClient) -> Result<()> {
    let dir: ObjectsDir = client.get_json("/objects/dir").await?;
    println!("{}", dir.path);
    Ok(())
}

pub async fn settings_get(client: &DobjdClient, json: bool) -> Result<()> {
    let settings: AppSettings = client.get_json("/settings").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&settings)?);
    } else {
        println!("synchronizer = {}", settings.synchronizer_api_url);
        println!("relayer      = {}", settings.relayer_api_url);
    }
    Ok(())
}

pub async fn settings_set(
    client: &DobjdClient,
    synchronizer: Option<String>,
    relayer: Option<String>,
) -> Result<()> {
    let mut current: AppSettings = client.get_json("/settings").await?;
    if let Some(s) = synchronizer {
        current.synchronizer_api_url = s;
    }
    if let Some(r) = relayer {
        current.relayer_api_url = r;
    }
    let saved: AppSettings = client.put_json("/settings", &current).await?;
    println!("synchronizer = {}", saved.synchronizer_api_url);
    println!("relayer      = {}", saved.relayer_api_url);
    Ok(())
}

pub async fn run(
    client: &DobjdClient,
    action_id: String,
    input_paths: Vec<String>,
    quiet: bool,
) -> Result<()> {
    // Subscribe to /events first so we don't miss progress messages emitted
    // before the SSE connection is established.
    let events_url = format!("{}/events", client.base_url());
    let progress_run_id = action_id.clone();
    let progress_handle = tokio::spawn(async move {
        let mut es = EventSource::get(&events_url);
        while let Some(event) = es.next().await {
            match event {
                Ok(SseEvent::Open) => {}
                Ok(SseEvent::Message(msg)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&msg.data) else {
                        continue;
                    };
                    if value.get("type").and_then(|v| v.as_str())
                        != Some(TARGET_RUN_ACTION_PROGRESS)
                    {
                        continue;
                    }
                    // Filter to our run only.
                    if value.get("runId").and_then(|v| v.as_str()) != Some(&progress_run_id) {
                        continue;
                    }
                    if quiet {
                        continue;
                    }
                    let phase = value
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let status = value
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let message = value
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    eprintln!("[{phase}/{status}] {message}");
                }
                Err(_) => break,
            }
        }
    });

    // Kick off the action.
    let result: RunActionResult = client
        .post_json::<_, RunActionResult>(
            "/actions/run",
            &RunActionRequest {
                input: RunActionInput {
                    action_id: action_id.clone(),
                    input_object_paths: input_paths,
                },
            },
        )
        .await?;

    progress_handle.abort();

    if !result.ok {
        return Err(anyhow!("run_action returned ok=false"));
    }

    println!("action: {action_id}");
    println!("old root: {}", result.old_root);
    println!("new root: {}", result.new_root);
    if !result.output_files.is_empty() {
        println!("outputs:");
        for f in &result.output_files {
            println!("  + {f}");
        }
    }
    if !result.nullified_files.is_empty() {
        println!("nullified:");
        for f in &result.nullified_files {
            println!("  - {f}");
        }
    }
    Ok(())
}

pub async fn watch(client: &DobjdClient) -> Result<()> {
    let url = format!("{}/events", client.base_url());
    let mut es = EventSource::get(&url);
    eprintln!("dobj: streaming {url} (Ctrl+C to stop)");
    while let Some(event) = es.next().await {
        match event {
            Ok(SseEvent::Open) => {}
            Ok(SseEvent::Message(msg)) => {
                let value: Value =
                    serde_json::from_str(&msg.data).unwrap_or(Value::String(msg.data.clone()));
                println!("{}", serde_json::to_string(&value)?);
            }
            Err(err) => {
                eprintln!("event-stream error: {err}");
                break;
            }
        }
    }
    Ok(())
}

pub async fn health(client: &DobjdClient) -> Result<()> {
    // /inventory is always cheap on the dobjd side and forces a real
    // round-trip to the driver — good liveness probe.
    let result: LoadGuiInventoryResult = client.get_json("/inventory").await?;
    println!(
        "dobjd OK ({}) — {} object(s), {} action(s) catalog",
        client.base_url(),
        result.inventory.len(),
        result.actions.len(),
    );
    Ok(())
}

fn short_hex(hex: &str) -> String {
    let trimmed = hex.trim_start_matches("0x");
    if trimmed.len() <= 12 {
        return hex.to_string();
    }
    format!("0x{}…{}", &trimmed[..6], &trimmed[trimmed.len() - 4..])
}
