use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Reply {
    request: &'static str,
    content_type: &'static str,
    body: String,
}

fn json_reply(request: &'static str, body: Value) -> Reply {
    Reply {
        request,
        content_type: "application/json",
        body: body.to_string(),
    }
}

fn cli(args: &[&str], replies: Vec<Reply>) -> Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        for reply in replies {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "missing request: {}",
                            reply.request
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let headers = String::from_utf8(request).unwrap();
            assert_eq!(headers.lines().next().unwrap(), reply.request);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|value| value.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; content_length];
            stream.read_exact(&mut body).unwrap();
            if reply.request.starts_with("POST") {
                let submitted: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(submitted["input"]["action"]["name"], "GatherIron");
            }
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", reply.content_type, reply.body.len(), reply.body).unwrap();
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_dobj"))
        .args(["--url", &url, "--json"])
        .args(args)
        .output()
        .unwrap();
    server.join().unwrap();
    output
}

fn state(status: &str) -> Value {
    json!({
        "runId": "run-1", "action": {"pluginName": "craft-basics", "name": "GatherIron"},
        "status": status, "progress": [], "error": if status == "failed" {Some("proof failed")} else {None},
        "result": if status == "succeeded" {Some(json!({
            "runId": "run-1", "oldRoot": "old", "txFinal": "final", "outputFiles": ["iron.dobj"], "inputFiles": []
        }))} else {None}
    })
}

#[test]
fn reattach_returns_retained_result_without_submitting_or_opening_a_stream() {
    let output = cli(
        &["runs", "wait", "run-1"],
        vec![json_reply("GET /runs/run-1 HTTP/1.1", state("succeeded"))],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"]["outputFiles"][0], "iron.dobj");
}

#[test]
fn reattach_follows_progress_then_reads_the_final_snapshot() {
    let progress = json!({"runId": "run-1", "phase": "generateProof", "status": "running", "message": "Proving"});
    let output = cli(
        &["runs", "wait", "run-1"],
        vec![
            json_reply("GET /runs/run-1 HTTP/1.1", state("generateProof")),
            Reply {
                request: "GET /runs/run-1/events HTTP/1.1",
                content_type: "text/event-stream",
                body: format!("data: {progress}\n\n"),
            },
            json_reply("GET /runs/run-1 HTTP/1.1", state("succeeded")),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Proving"));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "succeeded"
    );
}

#[test]
fn submitted_run_outputs_json_and_failures_exit_nonzero() {
    for status in ["succeeded", "failed"] {
        let output = cli(
            &["run", "craft-basics::GatherIron"],
            vec![
                json_reply(
                    "POST /runs HTTP/1.1",
                    json!({"runId": "run-1", "status": "queued"}),
                ),
                json_reply("GET /runs/run-1 HTTP/1.1", state(status)),
            ],
        );
        assert_eq!(output.status.success(), status == "succeeded");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
            status
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("run-1"));
    }
}

#[test]
fn active_runs_query_and_failed_snapshot_remain_inspectable() {
    let output = cli(
        &["runs", "list", "--active"],
        vec![json_reply(
            "GET /runs?status=active HTTP/1.1",
            json!([state("generateProof")]),
        )],
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()[0]["runId"],
        "run-1"
    );
    let output = cli(
        &["runs", "show", "run-1"],
        vec![json_reply("GET /runs/run-1 HTTP/1.1", state("failed"))],
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["error"],
        "proof failed"
    );
}

#[test]
fn discovery_preserves_fields_reservations_outputs_and_arguments() {
    let class = json!({"pluginName": "craft-basics", "name": "Axe"});
    let objects = json!([{"contentHash": "abc", "fileName": "axe.dobj", "class": class, "classHash": "hash", "emoji": "", "status": "live", "txHash": null, "description": null, "fields": {"durability": 99}, "heldByRunId": "run-1"}]);
    let output = cli(
        &["objects"],
        vec![json_reply("GET /objects HTTP/1.1", objects)],
    );
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result[0]["fields"]["durability"], 99);
    assert_eq!(result[0]["heldByRunId"], "run-1");
    let actions = json!([{"action": {"pluginName": "craft-basics", "name": "GatherIron"}, "emoji": "", "hash": "hash", "description": "Gather", "totalInputs": [], "totalOutputs": [{"class": class, "hash": "hash"}], "predicateSource": "", "args": ["grind_key"], "mutatedObjects": [{"inputIndex": 0, "outputIndex": 0}]}]);
    let output = cli(
        &["actions"],
        vec![json_reply("GET /actions HTTP/1.1", actions)],
    );
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result[0]["args"][0], "grind_key");
    assert_eq!(result[0]["totalOutputs"][0], class);
    assert_eq!(result[0]["mutatedObjects"][0]["outputIndex"], 0);
}
