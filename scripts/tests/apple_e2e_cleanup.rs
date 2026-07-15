use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf, process::Command};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("apple-e2e-cleanup.sh")
}

fn fake_container(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let bin = temp.path().join("container");
    let calls = temp.path().join("calls");
    fs::write(
        &bin,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$*" in
  inspect*|"volume inspect"*)
    printf '[{"configuration":{"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"%s"}}}]\n' "$ID"
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, calls)
}

fn manifest(temp: &tempfile::TempDir, resources: serde_json::Value) -> PathBuf {
    let path = temp.path().join("cleanup.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "sandbox_id": "gate4-test-123456789abc",
            "resources": resources,
            "managed_by": "gascan",
            "owner_token": "test-owner",
            "daemon_instance_path": temp.path().join("missing-instance"),
            "daemon_executable": "/missing/gascand",
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn run(temp: &tempfile::TempDir, path: &PathBuf, calls: &PathBuf) -> std::process::Output {
    let inherited = std::env::var("PATH").unwrap_or_default();
    Command::new(script())
        .arg(path)
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("CALLS", calls)
        .env("ID", "gate4-test-123456789abc")
        .output()
        .unwrap()
}

#[test]
fn out_of_scope_manifest_is_refused_before_runtime_commands() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let path = manifest(&temp, serde_json::json!(["somebody-elses-resource"]));
    let output = run(&temp, &path, &calls);
    assert!(!output.status.success());
    assert!(!calls.exists());
}

#[test]
fn owned_running_container_is_stopped_before_exact_delete() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}"),
        ]),
    );
    let _ = run(&temp, &path, &calls);
    let calls = fs::read_to_string(calls).unwrap();
    let stop = calls.find(&format!("stop --time 5 {id}")).unwrap();
    let delete = calls.find(&format!("delete {id}")).unwrap();
    assert!(stop < delete);
}
