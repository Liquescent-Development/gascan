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
    printf '[{"configuration":{"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"%s"}}}]\n' "$LABEL_ID"
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
            "daemon_cli": "/missing/gascan",
            "runtime_root": temp.path(),
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
        .env("LABEL_ID", "gate4-test-123456789abc")
        .output()
        .unwrap()
}

#[test]
fn exact_name_collision_is_retained_and_never_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([id, format!("gascan-mise-{id}"), format!("gascan-cache-{id}"), format!("gascan-config-{id}")]),
    );
    let inherited = std::env::var("PATH").unwrap_or_default();
    let output = Command::new(script())
        .arg(&path)
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("CALLS", &calls)
        .env("ID", id)
        .env("LABEL_ID", "somebody-else-123456789abc")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(path.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(!calls.lines().any(|line| line.starts_with("delete ") || line.starts_with("volume delete ")));
}

#[test]
fn invalid_daemon_pid_is_refused_without_signalling() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([id, format!("gascan-mise-{id}"), format!("gascan-cache-{id}"), format!("gascan-config-{id}")]),
    );
    let instance = temp.path().join("instance.json");
    fs::write(&instance, r#"{"owner_token":"test-owner","pid":"-1","executable":"/missing/gascand","start_identity":"start","instance_token":"instance"}"#).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["daemon_instance_path"] = serde_json::json!(instance);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = run(&temp, &path, &calls);
    assert_eq!(output.status.code(), Some(65));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid daemon pid"));
    assert!(path.exists());
}

fn daemon_cleanup_fixture(temp: &tempfile::TempDir, stuck_term: bool, unkillable: bool) -> (PathBuf, PathBuf) {
    let id = "gate4-test-123456789abc";
    let path = manifest(temp, serde_json::json!([id, format!("gascan-mise-{id}"), format!("gascan-cache-{id}"), format!("gascan-config-{id}")]));
    let daemon = temp.path().join("gascand");
    fs::write(&daemon, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&daemon, fs::Permissions::from_mode(0o755)).unwrap();
    let daemon = fs::canonicalize(daemon).unwrap();
    let instance = temp.path().join("instance.json");
    fs::write(&instance, serde_json::to_vec(&serde_json::json!({
        "owner_token":"test-owner", "pid":4242, "executable":daemon,
        "start_identity":"START", "instance_token":"INSTANCE"
    })).unwrap()).unwrap();
    let gascan = temp.path().join("gascan");
    fs::write(&gascan, format!("#!/bin/sh\nprintf '%s\\n' '{{\"instance_token\":\"INSTANCE\",\"pid\":4242,\"executable\":\"{}\",\"start_identity\":\"START\"}}'\n", daemon.display())).unwrap();
    fs::set_permissions(&gascan, fs::Permissions::from_mode(0o755)).unwrap();
    let ps = temp.path().join("ps");
    fs::write(&ps, format!(r#"#!/bin/sh
test ! -f "$STATE" || exit 1
case "$*" in *command=*) printf '%s\n' '{}' ;; *) printf '%s\n' START ;; esac
"#, daemon.display())).unwrap();
    fs::set_permissions(&ps, fs::Permissions::from_mode(0o755)).unwrap();
    let kill = temp.path().join("kill");
    fs::write(&kill, format!(r#"#!/bin/sh
printf '%s\n' "$*" >>"$KILL_CALLS"
case "$1" in
  -TERM) test {} = true || : >"$STATE" ;;
  -KILL) test {} = true || : >"$STATE" ;;
esac
"#, stuck_term, unkillable)).unwrap();
    fs::set_permissions(&kill, fs::Permissions::from_mode(0o755)).unwrap();
    let sleep = temp.path().join("sleep");
    fs::write(&sleep, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&sleep, fs::Permissions::from_mode(0o755)).unwrap();
    let container = temp.path().join("container");
    fs::write(&container, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&container, fs::Permissions::from_mode(0o755)).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["daemon_instance_path"] = serde_json::json!(instance);
    value["daemon_executable"] = serde_json::json!(daemon);
    value["daemon_cli"] = serde_json::json!(gascan);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    (path, temp.path().join("kill-calls"))
}

fn run_daemon_cleanup(temp: &tempfile::TempDir, path: &PathBuf, calls: &PathBuf) -> std::process::Output {
    let inherited = std::env::var("PATH").unwrap_or_default();
    Command::new(script()).arg(path)
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("STATE", temp.path().join("dead"))
        .env("KILL_CALLS", calls)
        .output().unwrap()
}

#[test]
fn validated_daemon_gets_bounded_term_then_revalidated_kill() {
    let temp = tempfile::tempdir().unwrap();
    let (path, calls) = daemon_cleanup_fixture(&temp, true, false);
    let output = run_daemon_cleanup(&temp, &path, &calls);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(calls).unwrap(), "-TERM 4242\n-KILL 4242\n");
    assert!(!path.exists());
}

#[test]
fn daemon_residue_retains_manifest_after_term_and_kill() {
    let temp = tempfile::tempdir().unwrap();
    let (path, calls) = daemon_cleanup_fixture(&temp, true, true);
    let output = run_daemon_cleanup(&temp, &path, &calls);
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(calls).unwrap(), "-TERM 4242\n-KILL 4242\n");
    assert!(path.exists());
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
