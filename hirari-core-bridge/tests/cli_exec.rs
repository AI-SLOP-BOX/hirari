//! `hirari exec` is the headless apply entrypoint for the command protocol.
//! These tests drive the real binary over stdin, so they cover the JSON wire
//! format a script or CI job uses rather than only the in-process executor.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(input: &str) -> (bool, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hirari"))
        .arg("exec")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("hirari binary must run");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("request is written");
    let output = child.wait_with_output().expect("binary exits");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("{}");
    let value = serde_json::from_str(first).expect("every response is one JSON object");
    (output.status.success(), value)
}

#[test]
fn exec_runs_a_read_only_inspection() {
    let request = r#"{"protocol":"hirari.command.v1","request_id":"cli-inspect","client":"cli","command":{"transaction":"inspect","permission":"read_only","actions":[{"op":"project_inspect"}]}}"#;
    let (ok, value) = run(request);
    assert!(ok, "exec must succeed: {value}");
    assert_eq!(value["ok"], true);
    assert!(value["result"]["results"][0]
        .get("midi_notes")
        .is_some_and(serde_json::Value::is_array));
}

#[test]
fn exec_applies_a_mutation_the_cli_could_not_reach_before() {
    let request = r#"{"protocol":"hirari.command.v1","request_id":"cli-add-track","client":"cli","command":{"transaction":"add-track","permission":"project_write","actions":[{"op":"add_track","name":"CLI Track"}]}}"#;
    let (ok, value) = run(request);
    assert!(ok, "exec must apply project writes: {value}");
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["applied"], 1);
}

#[test]
fn exec_rejects_a_foreign_protocol_version() {
    let request = r#"{"protocol":"hirari.command.v0","request_id":"cli-foreign","client":"cli","command":{"transaction":"inspect","permission":"read_only","actions":[{"op":"project_inspect"}]}}"#;
    let (ok, value) = run(request);
    assert!(!ok, "a foreign protocol must fail the process");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "unsupported_protocol");
}

#[test]
fn exec_reports_invalid_json_without_a_panic() {
    let (ok, value) = run("not json\n");
    assert!(!ok);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "invalid_json");
}
