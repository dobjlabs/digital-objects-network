use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest_eventsource::Event;
use wire_types::{RunActionProgress, RunState, RunStatus};

use crate::client::DobjdClient;

fn terminal(state: &RunState) -> bool {
    matches!(state.status, RunStatus::Succeeded | RunStatus::Failed)
}

fn run_path(run_id: &str) -> String {
    format!("/runs/{}", urlencoding::encode(run_id))
}

pub async fn list(client: &DobjdClient, active: bool, json: bool) -> Result<()> {
    let path = if active {
        "/runs?status=active"
    } else {
        "/runs"
    };
    let states: Vec<RunState> = client.get_json(path).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&states)?);
    } else {
        for state in states {
            println!("{} {:?} {}", state.run_id, state.status, state.action);
        }
    }
    Ok(())
}

pub async fn show(client: &DobjdClient, run_id: &str, json: bool) -> Result<()> {
    let state: RunState = client.get_json(&run_path(run_id)).await?;
    print_state(&state, json)
}

pub async fn follow(client: &DobjdClient, run_id: &str, quiet: bool) -> Result<RunState> {
    follow_inner(client, run_id, quiet).await.with_context(|| {
        format!("following run {run_id}; inspect `runs show {run_id}` or reattach with `runs wait {run_id}` before resubmitting")
    })
}

async fn follow_inner(client: &DobjdClient, run_id: &str, quiet: bool) -> Result<RunState> {
    let path = run_path(run_id);
    let state: RunState = client.get_json(&path).await?;
    if terminal(&state) {
        return Ok(state);
    }
    let mut stream = client.event_stream(&format!("{path}/events"))?;
    let mut failures = 0;
    while let Some(event) = stream.next().await {
        match event {
            Ok(Event::Open) => {}
            Ok(Event::Message(message)) => {
                let progress: RunActionProgress = serde_json::from_str(&message.data)?;
                failures = 0;
                if !quiet {
                    eprintln!(
                        "[{:?}/{:?}] {}",
                        progress.phase, progress.status, progress.message
                    );
                }
            }
            Err(error) => {
                // EOF also prompts a state read: the terminal snapshot can
                // become visible just after the final progress event.
                let snapshot: Result<RunState> = client.get_json(&path).await;
                if let Ok(snapshot) = snapshot
                    && terminal(&snapshot)
                {
                    stream.close();
                    return Ok(snapshot);
                }
                failures += 1;
                if failures >= 5 || matches!(error, reqwest_eventsource::Error::StreamEnded) {
                    stream.close();
                    break;
                }
                if !quiet {
                    eprintln!("progress stream interrupted for {run_id}: {error}; reconnecting");
                }
            }
        }
    }
    let mut failures = 0;
    loop {
        match client.get_json::<RunState>(&path).await {
            Ok(state) => {
                failures = 0;
                if terminal(&state) {
                    return Ok(state);
                }
            }
            Err(error) => {
                failures += 1;
                if failures >= 5 {
                    bail!(
                        "lost contact with run {run_id}; it may still complete. Reattach with `runs wait {run_id}` or inspect `runs show {run_id}` before resubmitting. Last error: {error:#}"
                    );
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn print_state(state: &RunState, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(state)?);
        return Ok(());
    }
    println!(
        "action: {}\nrun id: {}\nstatus: {:?}",
        state.action, state.run_id, state.status
    );
    if let Some(error) = &state.error {
        println!("error: {error}");
    }
    if let Some(result) = &state.result {
        println!("grounding root: {}", result.old_root);
        for file in &result.output_files {
            println!("output: {file}");
        }
        for file in &result.input_files {
            println!("nullified: {file}");
        }
    }
    Ok(())
}

pub fn finish(state: &RunState, json: bool) -> Result<()> {
    print_state(state, json)?;
    match state.status {
        RunStatus::Succeeded if state.result.is_some() => Ok(()),
        RunStatus::Failed => Err(anyhow!(
            "run {} failed: {}",
            state.run_id,
            state.error.as_deref().unwrap_or("unknown error")
        )),
        _ => bail!(
            "run {} has no terminal result: {:?}",
            state.run_id,
            state.status
        ),
    }
}
