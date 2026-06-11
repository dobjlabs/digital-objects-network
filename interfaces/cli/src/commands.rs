use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde_json::Value;
use tokio::time::sleep;

use crate::client::DobjdClient;
use wire_types::{
    ActionSummary, CheckActionReport, ClassRef, ClassSummary, DriverSettings, ImportObjectRequest,
    ObjectSummary, ObjectsDirInfo, QualifiedName, RunAccepted, RunActionInput, RunActionRequest,
    RunState, RunStatus,
};

const MAX_CONSECUTIVE_RUN_POLL_ERRORS: usize = 5;

fn parse_qualified(id: &str) -> Result<QualifiedName> {
    QualifiedName::parse(id).map_err(|err| anyhow!("{err}"))
}

fn render_inputs(refs: &[ClassRef]) -> String {
    if refs.is_empty() {
        return "(no inputs)".to_string();
    }
    refs.iter()
        .map(|r| r.class.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_outputs(refs: &[ClassRef]) -> String {
    if refs.is_empty() {
        return "(no outputs)".to_string();
    }
    refs.iter()
        .map(|r| r.class.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn objects(client: &DobjdClient, json: bool) -> Result<()> {
    let objects: Vec<ObjectSummary> = client.get_json("/objects").await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &objects
                    .iter()
                    .map(|o| serde_json::json!({
                        "contentHash": o.content_hash, "fileName": o.file_name, "class": o.class,
                        "status": o.status, "txHash": o.tx_hash,
                    }))
                    .collect::<Vec<_>>()
            )?
        );
        return Ok(());
    }

    if objects.is_empty() {
        println!("(no objects)");
        return Ok(());
    }
    for obj in &objects {
        println!(
            "[{:<10}] {} {:<28} content_hash={}",
            obj.status,
            obj.emoji,
            obj.class,
            short_hex(&obj.content_hash),
        );
    }
    Ok(())
}

pub async fn actions(client: &DobjdClient, json: bool) -> Result<()> {
    // `/actions` is a pure-local read of the plugin catalog — no
    // synchronizer round-trip, unlike `/objects`.
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
    let root: String = client.get_json("/state-root").await?;
    println!("{root}");
    Ok(())
}

pub async fn objects_dir(client: &DobjdClient) -> Result<()> {
    let dir: ObjectsDirInfo = client.get_json("/objects/dir").await?;
    println!("{}", dir.path);
    Ok(())
}

pub async fn import(client: &DobjdClient, path: PathBuf, json: bool) -> Result<()> {
    let dobj =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let obj: ObjectSummary = client
        .post_json("/objects/import", &ImportObjectRequest { dobj })
        .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "contentHash": obj.content_hash, "fileName": obj.file_name, "class": obj.class,
                "status": obj.status, "txHash": obj.tx_hash,
            }))?
        );
        return Ok(());
    }
    println!(
        "imported {} {} [{}]",
        obj.class,
        short_hex(&obj.content_hash),
        obj.status
    );
    println!("file:   {}", obj.file_name);
    Ok(())
}

fn print_settings(settings: &DriverSettings) {
    println!("synchronizer = {}", settings.synchronizer_api_url);
    println!("relayer      = {}", settings.relayer_api_url);
    println!(
        "mcp          = {}",
        if settings.mcp_enabled { "on" } else { "off" }
    );
}

pub async fn settings_get(client: &DobjdClient, json: bool) -> Result<()> {
    let settings: DriverSettings = client.get_json("/settings").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&settings)?);
    } else {
        print_settings(&settings);
    }
    Ok(())
}

pub async fn settings_set(
    client: &DobjdClient,
    synchronizer: Option<String>,
    relayer: Option<String>,
    mcp: Option<bool>,
) -> Result<()> {
    let mut current: DriverSettings = client.get_json("/settings").await?;
    if let Some(s) = synchronizer {
        current.synchronizer_api_url = s;
    }
    if let Some(r) = relayer {
        current.relayer_api_url = r;
    }
    if let Some(m) = mcp {
        current.mcp_enabled = m;
    }
    let saved: DriverSettings = client.put_json("/settings", &current).await?;
    print_settings(&saved);
    Ok(())
}

pub async fn run(
    client: &DobjdClient,
    action_id: String,
    input_paths: Vec<String>,
    quiet: bool,
) -> Result<()> {
    let action = parse_qualified(&action_id)?;

    // Start the run. dobjd registers it, runs proof generation and commit on a
    // background worker, and returns the run handle immediately.
    let accepted: RunAccepted = client
        .post_json(
            "/actions/run",
            &RunActionRequest {
                input: RunActionInput {
                    action: action.clone(),
                    input_object_paths: input_paths,
                },
            },
        )
        .await?;
    let run_id = accepted.run_id;
    if !quiet {
        eprintln!("run id:   {run_id}");
    }

    // Follow progress over the run's own SSE stream. `EventSource` reconnects
    // on a dropped connection, resending the last event id it saw, and the
    // endpoint replays from there — so a hiccup mid-run just resumes where it
    // left off. We stop at the first terminal event; the authoritative outcome
    // is read below either way.
    let events_url = format!("{}/actions/runs/{}/events", client.base_url(), run_id);
    let mut es = EventSource::get(&events_url);
    // Bound consecutive reconnect failures so a genuinely-down daemon falls
    // through to the poll (which surfaces the error) instead of retrying
    // forever. A successful (re)connect resets the count.
    const MAX_RECONNECTS: u32 = 5;
    let mut failures = 0u32;
    while let Some(event) = es.next().await {
        match event {
            Ok(SseEvent::Open) => failures = 0,
            Ok(SseEvent::Message(msg)) => {
                failures = 0;
                let Ok(value) = serde_json::from_str::<Value>(&msg.data) else {
                    continue;
                };
                let phase = value.get("phase").and_then(|v| v.as_str()).unwrap_or("?");
                let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if !quiet {
                    eprintln!("[{phase}/{status}] {message}");
                }
                let terminal = status == "failed" || (phase == "commit" && status == "done");
                if terminal {
                    es.close();
                    break;
                }
            }
            // A hiccup isn't a run failure: let EventSource reconnect and
            // resume from the last event id. Only give up after repeated
            // failures (dobjd is likely down), then poll for the result.
            Err(err) => {
                failures += 1;
                if failures > MAX_RECONNECTS {
                    if !quiet {
                        eprintln!("(progress stream lost: {err}; polling for result)");
                    }
                    break;
                }
            }
        }
    }

    // Authoritative outcome, and the recovery path if the stream dropped:
    // poll the run until it reaches a terminal state.
    let state = poll_run_to_terminal(client, &run_id, quiet).await?;
    match state.status {
        RunStatus::Succeeded => {
            let result = state
                .result
                .ok_or_else(|| anyhow!("run {run_id} reported success but no result"))?;
            println!("action:   {action}");
            println!("run id:   {run_id}");
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
        RunStatus::Failed => Err(anyhow!(
            "run {run_id} failed: {}",
            state.error.unwrap_or_else(|| "unknown error".to_string())
        )),
        other => Err(anyhow!("run {run_id} ended in unexpected state: {other:?}")),
    }
}

/// Poll `GET /actions/runs/{run_id}` until the run reaches a terminal state,
/// tolerating transient follow-up failures so a brief HTTP hiccup is not
/// reported as an action failure.
async fn poll_run_to_terminal(client: &DobjdClient, run_id: &str, quiet: bool) -> Result<RunState> {
    let path = format!("/actions/runs/{run_id}");
    let mut consecutive_errors = 0usize;
    loop {
        match client.get_json::<RunState>(&path).await {
            Ok(state) => {
                consecutive_errors = 0;
                match state.status {
                    RunStatus::Succeeded | RunStatus::Failed => return Ok(state),
                    _ => sleep(Duration::from_millis(500)).await,
                }
            }
            Err(err) => {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_RUN_POLL_ERRORS {
                    return Err(anyhow!(
                        "lost contact while following run {run_id}; it may still complete. Run `dobj objects` to reconcile. Last error: {err:#}"
                    ));
                }
                if !quiet {
                    eprintln!(
                        "(poll failed {consecutive_errors}/{MAX_CONSECUTIVE_RUN_POLL_ERRORS}: {err}; retrying)"
                    );
                }
                let delay_ms = 500 * consecutive_errors.min(10) as u64;
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
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
                "contentHash": obj.content_hash, "fileName": obj.file_name, "class": obj.class,
                "status": obj.status, "txHash": obj.tx_hash,
                "fields": obj.fields,
            }))?
        );
        return Ok(());
    }
    println!("{:<13} {}", "content hash:", obj.content_hash);
    println!("{:<13} {}", "class:", obj.class);
    println!("{:<13} {}", "status:", obj.status);
    println!("{:<13} {}", "file:", obj.file_name);
    if let Some(tx) = obj.tx_hash {
        println!("{:<13} {}", "tx hash:", tx);
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
                    "class": c.class, "contentHash": c.content_hash, "fileName": c.file_name,
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
                short_hex(&c.content_hash),
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

/// Install a plugin from a local path or an http(s) URL. The bytes are sent to
/// dobjd, which writes the `.pexe` into `~/.dobj/actions/` and hot-reloads the
/// catalog so the plugin is usable immediately (no restart).
pub async fn install(client: &DobjdClient, source: String, json: bool) -> Result<()> {
    let bytes = read_or_download(&source).await?;
    let plugin: String = client.post_bytes("/actions/install", bytes).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({ "plugin": plugin }))?
        );
    } else {
        println!("installed plugin '{plugin}' - run `dobj actions` to see what it added");
    }
    Ok(())
}

/// Mirror of `pexe::MAX_PEXE_BYTES`, to avoid pulling in pexe's dependency tree.
const MAX_PEXE_BYTES: u64 = 8 * 1024 * 1024;

/// Resolve the install source: an http(s) URL is downloaded, anything else is
/// read as a local file path. Both are bounded by `MAX_PEXE_BYTES` so an
/// over-large source fails before the whole thing is buffered in memory.
async fn read_or_download(source: &str) -> Result<Vec<u8>> {
    let is_url = reqwest::Url::parse(source)
        .map(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or(false);
    if is_url {
        let mut res = reqwest::get(source)
            .await
            .with_context(|| format!("GET {source}"))?
            .error_for_status()
            .with_context(|| format!("download failed: {source}"))?;
        let mut buf = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .with_context(|| format!("reading from {source}"))?
        {
            if buf.len() as u64 + chunk.len() as u64 > MAX_PEXE_BYTES {
                bail!("{source} exceeds the {MAX_PEXE_BYTES}-byte plugin limit");
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    } else {
        // Check size before reading so a wrong path can't pull a huge file
        // into memory.
        let len = std::fs::metadata(source)
            .with_context(|| format!("reading {source}"))?
            .len();
        if len > MAX_PEXE_BYTES {
            bail!("{source} ({len} bytes) exceeds the {MAX_PEXE_BYTES}-byte plugin limit");
        }
        std::fs::read(source).with_context(|| format!("reading {source}"))
    }
}

fn short_hex(hex: &str) -> String {
    let trimmed = hex.trim_start_matches("0x");
    if trimmed.len() <= 12 {
        return hex.to_string();
    }
    format!("0x{}…{}", &trimmed[..6], &trimmed[trimmed.len() - 4..])
}
