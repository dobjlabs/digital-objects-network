use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::client::DobjdClient;
use crate::types::{
    ActionSummary, AppSettings, CheckActionReport, ClassSummary, InventoryObject, ObjectSummary,
    ObjectsDir, QualifiedName, RunActionInput, RunActionRequest, RunActionResult,
};

const TARGET_RUN_ACTION_PROGRESS: &str = "run-action-progress";

fn parse_qualified(id: &str) -> Result<QualifiedName> {
    QualifiedName::parse(id).map_err(|err| anyhow!("{err}"))
}

fn render_inputs(refs: &[crate::types::ClassRef]) -> String {
    if refs.is_empty() {
        return "(no inputs)".to_string();
    }
    refs.iter()
        .map(|r| r.class.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_outputs(refs: &[crate::types::ClassRef]) -> String {
    if refs.is_empty() {
        return "(no outputs)".to_string();
    }
    refs.iter()
        .map(|r| r.class.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn inventory(client: &DobjdClient, json: bool) -> Result<()> {
    let inventory: Vec<InventoryObject> = client.get_json("/inventory").await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &inventory
                    .iter()
                    .map(|o| serde_json::json!({
                        "id": o.id, "fileName": o.file_name, "class": o.class,
                        "status": o.status, "txHash": o.tx_hash,
                    }))
                    .collect::<Vec<_>>()
            )?
        );
        return Ok(());
    }

    if inventory.is_empty() {
        println!("(no objects in inventory)");
        return Ok(());
    }
    for obj in &inventory {
        println!(
            "[{:<10}] {} {:<28} id={}",
            obj.status,
            obj.emoji,
            obj.class,
            short_hex(&obj.id),
        );
    }
    Ok(())
}

pub async fn actions(client: &DobjdClient, json: bool) -> Result<()> {
    // `/actions` is a pure-local read of the plugin catalog — no
    // synchronizer round-trip, unlike `/inventory`.
    let actions: Vec<ActionSummary> = client.get_json("/actions").await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &actions
                    .iter()
                    .map(|a| serde_json::json!({
                        "action": a.action,
                        "description": a.description,
                        "totalInputs": a.total_inputs.iter().map(|r| &r.class).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>()
            )?
        );
        return Ok(());
    }
    for action in &actions {
        println!(
            "{} {:<38} {} — {}",
            action.emoji,
            action.action,
            render_inputs(&action.total_inputs),
            action.description,
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
    let action = parse_qualified(&action_id)?;

    // Mint a per-call run id up front so the SSE filter can match against
    // it before the POST returns. We could let the daemon generate one and
    // pick it up from the response, but that response only arrives after
    // the action finishes — by then the progress events have already
    // streamed past our filter. Client-side generation closes that window.
    let run_id = uuid::Uuid::new_v4().to_string();

    // Subscribe to /events first so we don't miss progress messages emitted
    // before the SSE connection is established. We block on the first `Open`
    // event before posting the action so a fast `run_action` can't beat the
    // EventSource handshake.
    let events_url = format!("{}/events", client.base_url());
    let progress_run_id = run_id.clone();
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
                    action: action.clone(),
                    input_object_paths: input_paths,
                    run_id,
                },
            },
        )
        .await?;

    progress_handle.abort();

    println!("action:   {action}");
    println!("run id:   {}", result.run_id);
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
/// lines. Today this is `run-action-progress`.
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

pub async fn inspect_object(client: &DobjdClient, file_name: String, json: bool) -> Result<()> {
    // `.dobj` file names contain underscores and dots — both URL-safe — but
    // a paranoid encode keeps us correct if the naming scheme ever changes.
    let path = format!("/objects/{}", urlencoding::encode(&file_name));
    let obj: ObjectSummary = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": obj.id, "fileName": obj.file_name, "class": obj.class,
                "status": obj.status, "txHash": obj.tx_hash,
                "fields": obj.fields,
            }))?
        );
        return Ok(());
    }
    println!("id:        {}", obj.id);
    println!("class:     {}", obj.class);
    println!("status:    {}", obj.status);
    println!("file:      {}", obj.file_name);
    if let Some(tx) = obj.tx_hash {
        println!("tx hash:   {tx}");
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
                        "class": c.class, "liveCount": c.live_count,
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
            "{} {:<28} (live: {})  — {}",
            class.emoji, class.class, class.live_count, class.description,
        );
    }
    Ok(())
}

pub async fn inspect_class(client: &DobjdClient, name: String, json: bool) -> Result<()> {
    let _qname = parse_qualified(&name)?;
    let path = format!("/classes/{}", urlencoding::encode(&name));
    let class: ClassSummary = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "class": class.class, "hash": class.hash, "description": class.description,
                "liveCount": class.live_count,
                "producedBy": class.produced_by, "consumedBy": class.consumed_by,
                "predicateSource": class.predicate_source,
            }))?
        );
        return Ok(());
    }
    println!("class:        {} {}", class.emoji, class.class);
    println!("hash:         {}", class.hash);
    println!("description:  {}", class.description);
    println!("live count:   {}", class.live_count);
    if !class.produced_by.is_empty() {
        let names: Vec<String> = class.produced_by.iter().map(|q| q.to_string()).collect();
        println!("produced by:  {}", names.join(", "));
    }
    if !class.consumed_by.is_empty() {
        let names: Vec<String> = class.consumed_by.iter().map(|q| q.to_string()).collect();
        println!("consumed by:  {}", names.join(", "));
    }
    if !class.predicate_source.is_empty() {
        println!("predicate source:");
        for line in class.predicate_source.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

pub async fn inspect_action(client: &DobjdClient, id: String, json: bool) -> Result<()> {
    let _qname = parse_qualified(&id)?;
    let path = format!("/actions/{}", urlencoding::encode(&id));
    let action: ActionSummary = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": action.action, "hash": action.hash, "description": action.description,
                "totalInputs": action.total_inputs.iter().map(|r| &r.class).collect::<Vec<_>>(),
                "totalOutputs": action.total_outputs.iter().map(|r| &r.class).collect::<Vec<_>>(),
                "predicateSource": action.predicate_source,
            }))?
        );
        return Ok(());
    }
    println!("action:       {} {}", action.emoji, action.action);
    println!("hash:         {}", action.hash);
    println!("description:  {}", action.description);
    println!("inputs:       {}", render_inputs(&action.total_inputs));
    println!("outputs:      {}", render_outputs(&action.total_outputs));
    if !action.predicate_source.is_empty() {
        println!("predicate source:");
        for line in action.predicate_source.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

pub async fn feasibility(client: &DobjdClient, action_id: String, json: bool) -> Result<()> {
    let _qname = parse_qualified(&action_id)?;
    let path = format!("/actions/{}/feasibility", urlencoding::encode(&action_id));
    let report: CheckActionReport = client.get_json(&path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": report.action,
                "feasible": report.feasible,
                "availableInputs": report.available_inputs.iter().map(|c| serde_json::json!({
                    "class": c.class, "objectId": c.object_id, "fileName": c.file_name,
                })).collect::<Vec<_>>(),
                "missingInputs": report.missing_inputs.iter().map(|r| &r.class).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }
    let mark = if report.feasible { "✓" } else { "✗" };
    println!("{mark}  {}", report.action);
    if !report.available_inputs.is_empty() {
        println!("available inputs:");
        for c in &report.available_inputs {
            println!(
                "  • {} {} ({})",
                c.class,
                short_hex(&c.object_id),
                c.file_name
            );
        }
    }
    if !report.missing_inputs.is_empty() {
        println!("missing inputs:");
        for missing in &report.missing_inputs {
            println!("  • {}", missing.class);
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
