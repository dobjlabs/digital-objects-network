use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::client::DobjdClient;
use crate::types::{
    AppSettings, CheckActionReport, ClassSummary, LoadGuiInventoryResult, ObjectSummary,
    ObjectsDir, RunActionInput, RunActionRequest, RunActionResult,
};

const TARGET_RUN_ACTION_PROGRESS: &str = "run-action-progress";

pub async fn inventory(client: &DobjdClient, json: bool) -> Result<()> {
    let result: LoadGuiInventoryResult = client.get_json("/inventory").await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "inventory": result.inventory.iter().map(|o| serde_json::json!({
                    "id": o.id, "fileName": o.file_name, "className": o.class_name,
                    "status": o.status, "txHash": o.tx_hash, "grounded": o.grounded,
                })).collect::<Vec<_>>(),
            }))?
        );
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
            obj.status,
            grounded,
            obj.emoji,
            obj.class_name,
            short_hex(&obj.id),
        );
    }
    Ok(())
}

pub async fn actions(client: &DobjdClient, json: bool) -> Result<()> {
    let result: LoadGuiInventoryResult = client.get_json("/inventory").await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &result
                    .actions
                    .iter()
                    .map(|a| serde_json::json!({
                        "id": a.id, "description": a.description, "inputs": a.total_input_classes,
                    }))
                    .collect::<Vec<_>>()
            )?
        );
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
    // before the SSE connection is established. We block on the first `Open`
    // event before posting the action so a fast `run_action` can't beat the
    // EventSource handshake.
    let events_url = format!("{}/events", client.base_url());
    let progress_run_id = action_id.clone();
    let (open_tx, open_rx) = oneshot::channel::<()>();
    let progress_handle = tokio::spawn(async move {
        let mut es = EventSource::get(&events_url);
        let mut open_tx = Some(open_tx);
        while let Some(event) = es.next().await {
            match event {
                Ok(SseEvent::Open) => {
                    if let Some(tx) = open_tx.take() {
                        let _ = tx.send(());
                    }
                }
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
                    let phase = value.get("phase").and_then(|v| v.as_str()).unwrap_or("?");
                    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                    let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("");
                    eprintln!("[{phase}/{status}] {message}");
                }
                Err(_) => break,
            }
        }
    });

    // Wait for the SSE connection to actually open before kicking off the
    // action. If the EventSource task dies before opening (e.g. dobjd
    // unreachable), the oneshot is dropped — fall through and let the POST
    // surface the real error.
    let _ = open_rx.await;

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

/// Stream every event flowing through dobjd's broadcast hub as JSON
/// lines: `objects-changed`, `run-action-progress`, `mcp-action-started`.
/// Each event prints once across all connected clients, so this is the
/// single place to see activity from the desktop, web UI, MCP, and
/// other CLI invocations at the same time.
pub async fn events(client: &DobjdClient) -> Result<()> {
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

pub async fn inspect_object(client: &DobjdClient, id: String, json: bool) -> Result<()> {
    // Object IDs are URL-safe (hex), but be defensive in case someone pastes
    // a raw filename that contains underscores etc.
    let path = format!("/objects/{}", urlencoding::encode(&id));
    let obj: ObjectSummary = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": obj.id, "fileName": obj.file_name, "className": obj.class_name,
                "status": obj.status, "txHash": obj.tx_hash, "grounded": obj.grounded,
                "fields": obj.fields,
            }))?
        );
        return Ok(());
    }
    println!("id:        {}", obj.id);
    println!("class:     {}", obj.class_name);
    println!("status:    {}", obj.status);
    println!("file:      {}", obj.file_name);
    if let Some(tx) = obj.tx_hash {
        println!("tx hash:   {tx}");
    }
    if let Some(grounded) = obj.grounded {
        println!("grounded:  {grounded}");
    }
    println!("fields:");
    println!("{}", serde_json::to_string_pretty(&obj.fields)?);
    Ok(())
}

pub async fn classes(client: &DobjdClient, json: bool) -> Result<()> {
    let classes: Vec<ClassSummary> = client.get_json("/classes").await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &classes
                    .iter()
                    .map(|c| serde_json::json!({
                        "name": c.name, "liveCount": c.live_count,
                        "producedBy": c.produced_by, "consumedBy": c.consumed_by,
                        "description": c.description,
                    }))
                    .collect::<Vec<_>>()
            )?
        );
        return Ok(());
    }
    if classes.is_empty() {
        println!("(no classes — no plugin loaded?)");
        return Ok(());
    }
    for class in &classes {
        println!(
            "{} {:<14} (live: {})  — {}",
            class.emoji, class.name, class.live_count, class.description,
        );
    }
    Ok(())
}

pub async fn inspect_class(client: &DobjdClient, name: String, json: bool) -> Result<()> {
    let path = format!("/classes/{}", urlencoding::encode(&name));
    let class: ClassSummary = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": class.name, "hash": class.hash, "description": class.description,
                "liveCount": class.live_count,
                "producedBy": class.produced_by, "consumedBy": class.consumed_by,
                "predicateSource": class.predicate_source,
            }))?
        );
        return Ok(());
    }
    println!("class:        {} {}", class.emoji, class.name);
    println!("hash:         {}", class.hash);
    println!("description:  {}", class.description);
    println!("live count:   {}", class.live_count);
    if !class.produced_by.is_empty() {
        println!("produced by:  {}", class.produced_by.join(", "));
    }
    if !class.consumed_by.is_empty() {
        println!("consumed by:  {}", class.consumed_by.join(", "));
    }
    if !class.predicate_source.is_empty() {
        println!("predicate source:");
        for line in class.predicate_source.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

pub async fn feasibility(client: &DobjdClient, action_id: String, json: bool) -> Result<()> {
    let path = format!("/actions/{}/feasibility", urlencoding::encode(&action_id));
    let report: CheckActionReport = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "actionId": report.action_id,
                "feasible": report.feasible,
                "availableInputs": report.available_inputs.iter().map(|c| serde_json::json!({
                    "className": c.class_name, "objectId": c.object_id, "fileName": c.file_name,
                })).collect::<Vec<_>>(),
                "missingInputs": report.missing_inputs,
            }))?
        );
        return Ok(());
    }
    let mark = if report.feasible { "✓" } else { "✗" };
    println!("{mark}  {}", report.action_id);
    if !report.available_inputs.is_empty() {
        println!("available inputs:");
        for c in &report.available_inputs {
            println!(
                "  • {} {} ({})",
                c.class_name,
                short_hex(&c.object_id),
                c.file_name
            );
        }
    }
    if !report.missing_inputs.is_empty() {
        println!("missing inputs:");
        for class in &report.missing_inputs {
            println!("  • {class}");
        }
    }
    Ok(())
}

fn short_hex(hex: &str) -> String {
    let trimmed = hex.trim_start_matches("0x");
    if trimmed.len() <= 12 {
        return hex.to_string();
    }
    format!("0x{}…{}", &trimmed[..6], &trimmed[trimmed.len() - 4..])
}
