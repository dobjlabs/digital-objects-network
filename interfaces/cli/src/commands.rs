use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde_json::Value;

use crate::client::{DobjdClient, SLOW_REQUEST_TIMEOUT};
use wire_types::transactions::TransactionRecord;
use wire_types::{
    ActionArgValues, ActionInput, ActionSummary, CheckActionReport, ClassRef, ClassSummary,
    DriverSettings, ImportObjectRequest, ObjectSummary, ObjectsDirInfo, ProofJobState,
    QualifiedName, RunAccepted, StartRunRequest, SubmitTransactionsRequest,
};

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
    let objects: Vec<ObjectSummary> = client
        .with_timeout(SLOW_REQUEST_TIMEOUT)
        .get_json("/objects")
        .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &objects
                    .iter()
                    .map(|o| serde_json::json!({
                        "contentHash": o.content_hash, "fileName": o.file_name, "class": o.class,
                        "status": o.status, "txHash": o.tx_hash,
                        "fields": o.fields, "heldByRunId": o.held_by_run_id,
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
            "[{:<10}] {} {:<28} content_hash={} file={}",
            obj.status,
            obj.emoji,
            obj.class,
            short_hex(&obj.content_hash),
            obj.file_name,
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
                        "totalOutputs": a.total_outputs.iter().map(|r| &r.class).collect::<Vec<_>>(),
                        "args": a.args, "mutatedObjects": a.mutated_objects,
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
    raw_args: Vec<String>,
    quiet: bool,
    json: bool,
) -> Result<()> {
    let action = parse_qualified(&action_id)?;

    let args = parse_action_args(&raw_args)?;

    // Start the run. dobjd registers it, runs proof generation and commit on a
    // background worker, and returns the run handle immediately.
    let accepted: RunAccepted = client
        .post_json(
            "/runs",
            &StartRunRequest {
                input: ActionInput {
                    action: action.clone(),
                    input_object_paths: input_paths,
                    args,
                },
            },
        )
        .await?;
    let run_id = accepted.run_id;
    if !quiet {
        eprintln!("run id:   {run_id}");
    }

    let state = crate::runs::follow(client, &run_id, quiet).await?;
    crate::runs::finish(&state, json)
}

/// Timeout for each daemon wait request; repeat until the state is terminal.
const WAIT_STEP_MS: u64 = 30_000;

pub async fn prove(
    client: &DobjdClient,
    action_id: String,
    input_paths: Vec<String>,
    raw_args: Vec<String>,
    wait: bool,
    json: bool,
) -> Result<()> {
    let action = parse_qualified(&action_id)?;
    let args = parse_action_args(&raw_args)?;
    let mut job: ProofJobState = client
        .post_json(
            "/proof-jobs",
            &ActionInput {
                action,
                input_object_paths: input_paths,
                args,
            },
        )
        .await?;
    if wait {
        while !job.status.is_terminal() {
            let path = format!(
                "/proof-jobs/{}?wait=true&timeoutMs={WAIT_STEP_MS}",
                job.job_id
            );
            job = client.get_json(&path).await?;
            if let Some(message) = &job.message
                && !json
            {
                eprintln!("[{}] {message}", job.status);
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&job)?);
        return Ok(());
    }
    print_proof_job(&job);
    match (job.status, job.error) {
        (wire_types::ProofJobStatus::Failed, Some(error)) => Err(anyhow!("proof failed: {error}")),
        _ => Ok(()),
    }
}

fn print_proof_job(job: &ProofJobState) {
    println!("{:<10} {}", "job id:", job.job_id);
    println!("{:<10} {}", "action:", job.action);
    println!("{:<10} {}", "status:", job.status);
    if let Some(tx_final) = &job.tx_final {
        println!("{:<10} {}", "txFinal:", tx_final);
    }
    if let Some(error) = &job.error {
        println!("{:<10} {}", "error:", error);
    }
}

pub async fn proof_jobs(client: &DobjdClient, active: bool, json: bool) -> Result<()> {
    let path = if active {
        "/proof-jobs?status=active"
    } else {
        "/proof-jobs"
    };
    let jobs: Vec<ProofJobState> = client.get_json(path).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
        return Ok(());
    }
    if jobs.is_empty() {
        println!("(no proof jobs)");
        return Ok(());
    }
    for job in &jobs {
        println!(
            "[{:<9}] {} {} {}",
            job.status,
            job.job_id,
            job.action,
            job.tx_final.as_deref().map(short_hex).unwrap_or_default()
        );
    }
    Ok(())
}

pub async fn submit(client: &DobjdClient, tx_finals: Vec<String>, json: bool) -> Result<()> {
    let timeout = SLOW_REQUEST_TIMEOUT * u32::try_from(tx_finals.len().max(1))?;
    let records: Vec<TransactionRecord> = client
        .with_timeout(timeout)
        .post_json(
            "/transactions/submit",
            &SubmitTransactionsRequest { tx_finals },
        )
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }
    for record in &records {
        println!(
            "{} submitted as relayer job {}",
            short_hex(&record.tx_final),
            record.relayer_job_id.as_deref().unwrap_or("?")
        );
    }
    Ok(())
}

pub async fn transactions(client: &DobjdClient, json: bool) -> Result<()> {
    let records: Vec<TransactionRecord> = client.get_json("/transactions").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }
    if records.is_empty() {
        println!("(no transactions)");
        return Ok(());
    }
    for record in &records {
        println!(
            "[{:<9}] {} {} block {}{}",
            record.state,
            record.tx_final,
            record.action,
            record.grounding_block,
            record
                .tx_hash
                .as_deref()
                .map(|tx| format!(" tx {}", short_hex(tx)))
                .unwrap_or_default()
        );
    }
    Ok(())
}

pub async fn inspect_transaction(
    client: &DobjdClient,
    tx_final: String,
    wait: bool,
    json: bool,
) -> Result<()> {
    let base = format!("/transactions/{}", urlencoding::encode(&tx_final));
    let mut record: TransactionRecord = client.get_json(&base).await?;
    while wait && !record.state.is_terminal() {
        let path = format!("{base}?wait=true&timeoutMs={WAIT_STEP_MS}");
        record = client.get_json(&path).await?;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(());
    }
    print_transaction(&record);
    Ok(())
}

fn print_transaction(record: &TransactionRecord) {
    println!("{:<16} {}", "txFinal:", record.tx_final);
    println!("{:<16} {}", "action:", record.action);
    println!("{:<16} {}", "state:", record.state);
    println!("{:<16} {}", "grounding block:", record.grounding_block);
    println!("{:<16} {}", "grounding root:", record.grounding_root);
    if let Some(job) = &record.relayer_job_id {
        println!("{:<16} {}", "relayer job:", job);
    }
    if let Some(tx) = &record.tx_hash {
        println!("{:<16} {}", "tx hash:", tx);
    }
    if let Some(failure) = &record.relayer_failure {
        println!("{:<16} {}", "relayer failure:", failure);
    }
    if let Some(reason) = &record.reason {
        println!("{:<16} {}", "reason:", reason);
    }
    if !record.input_files.is_empty() {
        println!("inputs:");
        for f in &record.input_files {
            println!("  - {f}");
        }
    }
    if !record.output_files.is_empty() {
        println!("outputs:");
        for f in &record.output_files {
            println!("  + {f}");
        }
    }
}

pub async fn delete_transaction(client: &DobjdClient, tx_final: String, json: bool) -> Result<()> {
    let path = format!("/transactions/{}", urlencoding::encode(&tx_final));
    let record: TransactionRecord = client
        .with_timeout(SLOW_REQUEST_TIMEOUT)
        .delete_json(&path)
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(());
    }
    println!(
        "deleted {} ({} output file(s) removed)",
        record.tx_final,
        record.output_files.len()
    );
    Ok(())
}

fn parse_action_args(raw: &[String]) -> Result<ActionArgValues> {
    let mut args = BTreeMap::new();
    for item in raw {
        let Some((name, value)) = item.split_once('=') else {
            bail!("--arg expects NAME=JSON, got {item:?}");
        };
        let json = serde_json::from_str(value)
            .with_context(|| format!("argument `{name}`: expected a POD2 value in JSON form"))?;
        if args.insert(name.to_string(), json).is_some() {
            bail!("argument `{name}` given more than once");
        }
    }
    Ok(args)
}

/// Stream dobjd's shared event hub as JSON lines, including activity from
/// the desktop, browser UI, MCP, and other CLI invocations.
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
                "fields": obj.fields, "heldByRunId": obj.held_by_run_id,
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
                "args": action.args,
            }))?
        );
        return Ok(());
    }
    println!("action:       {} {}", action.emoji, action.action);
    println!("hash:         {}", action.hash);
    println!("description:  {}", action.description);
    println!("inputs:       {}", render_inputs(&action.total_inputs));
    println!("outputs:      {}", render_outputs(&action.total_outputs));
    if !action.args.is_empty() {
        println!("arguments:");
        for name in &action.args {
            println!("  {name}");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_args_from_json() {
        let raw = [
            r#"n={"Int":"-3"}"#,
            r#"text="a=b""#,
            r#"key={"Untyped":"00"}"#,
        ]
        .map(str::to_string);
        let args = parse_action_args(&raw).unwrap();
        assert_eq!(args["n"], serde_json::json!({"Int": "-3"}));
        assert_eq!(args["text"], serde_json::json!("a=b"));
        assert_eq!(args["key"], serde_json::json!({"Untyped": "00"}));
        assert!(parse_action_args(&["key".to_string()]).is_err());
        assert!(parse_action_args(&["n=1".to_string(), "n=2".to_string()]).is_err());
        let err = parse_action_args(&["n=invalid".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("argument `n`"), "{err}");
    }
}
