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
  "list --all --format json")
    if test -f "$CALLS.container-deleted"; then
      printf '[]\n'
    else
      printf '[{"configuration":{"id":"%s","labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"%s"}}}]\n' "$ID" "$LABEL_ID"
    fi
    ;;
  "volume list --format json")
    records=
    for name in "gascan-mise-$ID" "gascan-cache-$ID" "gascan-config-$ID"; do
      if ! test -f "$CALLS.volume-deleted" || ! grep -Fxq "$name" "$CALLS.volume-deleted"; then
        record=$(printf '{"id":"%s","configuration":{"name":"%s","labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"%s"}}}' "$name" "$name" "$LABEL_ID")
        if test -n "$records"; then records="$records,$record"; else records=$record; fi
      fi
    done
    printf '[%s]\n' "$records"
    ;;
  "delete $ID")
    : >"$CALLS.container-deleted"
    ;;
  "volume delete "*)
    printf '%s\n' "$3" >>"$CALLS.volume-deleted"
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, calls)
}

fn manifest(temp: &tempfile::TempDir, resources: serde_json::Value) -> PathBuf {
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let session = temp.path().join("session-test");
    let runtime = session.join("gascan-gate4-runtime-test");
    let project = session.join("gascan-gate4-root-test");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::set_permissions(&session, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&project, fs::Permissions::from_mode(0o700)).unwrap();
    let trusted_cli = temp.path().join("trusted-gascan");
    if !trusted_cli.exists() {
        fs::write(&trusted_cli, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&trusted_cli, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = temp.path().join("cleanup.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "sandbox_id": "gate4-test-123456789abc",
            "resources": resources,
            "managed_by": "gascan",
            "owner_token": "test-owner",
            "daemon_instance_path": fs::canonicalize(&runtime).unwrap().join("daemon-instance.json"),
            "daemon_executable": "/missing/gascand",
            "daemon_cli": fs::canonicalize(&trusted_cli).unwrap(),
            "runtime_root": fs::canonicalize(&runtime).unwrap(),
            "project_root": fs::canonicalize(&project).unwrap(),
            "session_root": fs::canonicalize(&session).unwrap(),
            "abort_evidence_path": fs::canonicalize(&runtime).unwrap().join("abort-probe-reached.json"),
            "outside_sentinel_path": null,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::canonicalize(path).unwrap()
}

fn manifest_with_dns(temp: &tempfile::TempDir, domain: &str) -> PathBuf {
    let id = "gate4-test-123456789abc";
    let path = manifest(
        temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["dns_domain"] = serde_json::Value::String(domain.to_owned());
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn mark_abort_probe_reached(path: &PathBuf) {
    let record: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let evidence = PathBuf::from(record["abort_evidence_path"].as_str().unwrap());
    fs::write(
        &evidence,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "kind": "gascan-security-abort-reached",
            "sandbox_id": record["sandbox_id"],
            "owner_token": record["owner_token"],
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(evidence, fs::Permissions::from_mode(0o600)).unwrap();
}

fn install_dns_cleanup_commands(temp: &tempfile::TempDir, inventory: &str, delete_exit: i32) {
    install_absent_container(temp);
    let sudo = temp.path().join("sudo");
    fs::write(
        &sudo,
        format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
test "$1" = -n
shift
test "$1 $2 $3 $4" = "container system dns delete"
exit {delete_exit}
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();
    let container = temp.path().join("container");
    fs::write(
        &container,
        format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$*" in
  "system dns list --format json")
    if test -f "$CALLS.dns-deleted"; then printf '[]\n'; else printf '%s\n' '{inventory}'; fi
    ;;
  "list --all --format json"|"volume list --format json") printf '[]\n' ;;
  *) exit 42 ;;
esac
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&container, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn stale_owned_dns_route_is_reconciled_and_record_cleared() {
    let temp = tempfile::tempdir().unwrap();
    let domain = "gascan-00112233445566778899aabbccddeeff.test";
    let path = manifest_with_dns(&temp, domain);
    mark_abort_probe_reached(&path);
    let calls = temp.path().join("calls");
    install_dns_cleanup_commands(&temp, &format!(r#"["{domain}"]"#), 0);
    let sudo = temp.path().join("sudo");
    fs::write(
        &sudo,
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >>\"$CALLS\"\n: >\"$CALLS.dns-deleted\"\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();

    let output = run(&temp, &path, &calls);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!path.exists());
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("Gate 4 abort recovery reconciled exact recorded resources"));
    assert!(fs::read_to_string(calls)
        .unwrap()
        .contains(&format!("container system dns delete {domain}")));
}

#[test]
fn mismatched_abort_marker_is_refused_before_inventory_or_proof() {
    let temp = tempfile::tempdir().unwrap();
    let domain = "gascan-00112233445566778899aabbccddeeff.test";
    let path = manifest_with_dns(&temp, domain);
    mark_abort_probe_reached(&path);
    let record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let evidence = PathBuf::from(record["abort_evidence_path"].as_str().unwrap());
    let mut marker: serde_json::Value =
        serde_json::from_slice(&fs::read(&evidence).unwrap()).unwrap();
    marker["owner_token"] = serde_json::Value::String("foreign-owner".to_owned());
    fs::write(&evidence, serde_json::to_vec(&marker).unwrap()).unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o600)).unwrap();
    let calls = temp.path().join("calls");
    install_dns_cleanup_commands(&temp, &format!(r#"["{domain}"]"#), 0);

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(evidence.exists());
    assert!(!calls.exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("abort recovery reconciled"));
}

#[test]
fn abort_marker_path_escape_is_refused_before_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let domain = "gascan-00112233445566778899aabbccddeeff.test";
    let path = manifest_with_dns(&temp, domain);
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["abort_evidence_path"] = serde_json::Value::String("/tmp/foreign-marker".to_owned());
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let calls = temp.path().join("calls");
    install_dns_cleanup_commands(&temp, &format!(r#"["{domain}"]"#), 0);

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(!calls.exists());
}

#[test]
fn ambiguous_dns_inventory_is_refused_and_manifest_retained() {
    let temp = tempfile::tempdir().unwrap();
    let domain = "gascan-00112233445566778899aabbccddeeff.test";
    let path = manifest_with_dns(&temp, domain);
    let calls = temp.path().join("calls");
    install_dns_cleanup_commands(&temp, &format!(r#"["{domain}","{domain}"]"#), 0);

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(!fs::read_to_string(calls).unwrap().contains("dns delete"));
}

#[test]
fn foreign_dns_record_is_refused_without_inventory_or_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let path = manifest_with_dns(&temp, "example.com");
    let calls = temp.path().join("calls");
    install_dns_cleanup_commands(&temp, r#"["example.com"]"#, 0);

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(!calls.exists());
}

#[test]
fn dns_delete_failure_is_surfaced_and_manifest_retained() {
    let temp = tempfile::tempdir().unwrap();
    let domain = "gascan-00112233445566778899aabbccddeeff.test";
    let path = manifest_with_dns(&temp, domain);
    mark_abort_probe_reached(&path);
    let calls = temp.path().join("calls");
    install_dns_cleanup_commands(&temp, &format!(r#"["{domain}"]"#), 29);

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("abort recovery reconciled"));
    assert!(fs::read_to_string(calls)
        .unwrap()
        .contains(&format!("container system dns delete {domain}")));
}

fn run(temp: &tempfile::TempDir, path: &PathBuf, calls: &PathBuf) -> std::process::Output {
    let inherited = std::env::var("PATH").unwrap_or_default();
    Command::new(script())
        .arg(path)
        .arg(fs::canonicalize(temp.path().join("trusted-gascan")).unwrap())
        .arg(fs::canonicalize(temp.path()).unwrap())
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("CALLS", calls)
        .env("ID", "gate4-test-123456789abc")
        .env("LABEL_ID", "gate4-test-123456789abc")
        .output()
        .unwrap()
}

fn install_container_script(temp: &tempfile::TempDir, body: &str) -> PathBuf {
    let container = temp.path().join("container");
    fs::write(&container, body).unwrap();
    fs::set_permissions(&container, fs::Permissions::from_mode(0o755)).unwrap();
    container
}

fn install_absent_container(temp: &tempfile::TempDir) -> PathBuf {
    install_container_script(
        temp,
        r#"#!/bin/sh
case "$*" in
  "list --all --format json"|"volume list --format json") printf '[]\n' ;;
  *) exit 42 ;;
esac
"#,
    )
}

#[test]
fn absent_recorded_children_are_an_idempotent_success() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let session = temp.path().join("session-test");
    fs::remove_dir_all(&session).unwrap();
    install_absent_container(&temp);
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!path.exists());
    assert!(!session.exists());
}

#[test]
fn legacy_owned_outside_sentinel_is_removed_before_session_rmdir() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let session = temp.path().join("session-test");
    let sentinel = session.join("synthetic-outside-00112233445566778899aabbccddeeff");
    fs::write(&sentinel, "synthetic-outside-only").unwrap();
    install_absent_container(&temp);
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!sentinel.exists());
    assert!(!session.exists());
    assert!(!path.exists());
}

#[test]
fn recorded_private_runtime_sentinel_is_identity_checked_and_removed() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let runtime = PathBuf::from(record["runtime_root"].as_str().unwrap());
    let sentinel = runtime.join("synthetic-outside-00112233445566778899aabbccddeeff");
    fs::write(&sentinel, "synthetic-outside-only").unwrap();
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o600)).unwrap();
    record["outside_sentinel_path"] =
        serde_json::Value::String(sentinel.to_string_lossy().into_owned());
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    install_absent_container(&temp);
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!sentinel.exists());
    assert!(!path.exists());
}

#[test]
fn recorded_outside_sentinel_path_escape_is_refused_before_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let escaped = temp
        .path()
        .join("synthetic-outside-00112233445566778899aabbccddeeff");
    fs::write(&escaped, "foreign").unwrap();
    record["outside_sentinel_path"] =
        serde_json::Value::String(escaped.to_string_lossy().into_owned());
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    install_absent_container(&temp);
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(escaped.exists());
    assert!(!calls.exists());
}

#[test]
fn ambiguous_legacy_outside_sentinels_are_refused_without_removal() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let session = temp.path().join("session-test");
    for token in [
        "00112233445566778899aabbccddeeff",
        "ffeeddccbbaa99887766554433221100",
    ] {
        fs::write(
            session.join(format!("synthetic-outside-{token}")),
            "synthetic-outside-only",
        )
        .unwrap();
    }
    install_absent_container(&temp);
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert_eq!(
        fs::read_dir(session)
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("synthetic-outside-"))
            .count(),
        2
    );
    assert!(!calls.exists());
}

#[test]
fn successful_json_inventories_prove_real_absence() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let session = temp.path().join("session-test");
    fs::remove_dir_all(&session).unwrap();
    install_container_script(
        &temp,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$*" in
  "list --all --format json"|"volume list --format json") printf '[]\n' ;;
  *) exit 42 ;;
esac
"#,
    );
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!path.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains("list --all --format json"));
    assert!(calls.contains("volume list --format json"));
}

#[test]
fn inventory_runtime_failure_retains_manifest_without_deletes() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    install_container_script(
        &temp,
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >>\"$CALLS\"\nprintf 'runtime unavailable\\n' >&2\nexit 42\n",
    );
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(!calls
        .lines()
        .any(|line| line.starts_with("delete ") || line.starts_with("volume delete ")));
}

#[test]
fn verification_inventory_failure_retains_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    install_container_script(
        &temp,
        r#"#!/bin/sh
set -eu
case "$*" in
  "list --all --format json")
    count=0
    test ! -f "$COUNT" || count=$(cat "$COUNT")
    count=$((count + 1))
    printf '%s' "$count" >"$COUNT"
    test "$count" -eq 1 || exit 42
    printf '[]\n'
    ;;
  "volume list --format json") printf '[]\n' ;;
  *) exit 42 ;;
esac
"#,
    );
    let calls = temp.path().join("calls");
    let inherited = std::env::var("PATH").unwrap_or_default();
    let output = Command::new(script())
        .arg(&path)
        .arg(fs::canonicalize(temp.path().join("trusted-gascan")).unwrap())
        .arg(fs::canonicalize(temp.path()).unwrap())
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("COUNT", temp.path().join("count"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(path.exists());
    assert_eq!(fs::read_to_string(temp.path().join("count")).unwrap(), "2");
    assert!(!calls.exists());
}

#[test]
fn top_level_only_container_identity_is_rejected_without_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    install_container_script(
        &temp,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$*" in
  "list --all --format json")
    printf '[{"id":"%s","configuration":{"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"%s"}}}]\n' "$ID" "$ID"
    ;;
  "volume list --format json") printf '[]\n' ;;
  *) exit 42 ;;
esac
"#,
    );
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(!calls.lines().any(|line| line.starts_with("delete ")));
}

#[test]
fn mismatched_ambiguous_volume_identity_is_rejected_without_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let volume = format!("gascan-mise-{id}");
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            volume,
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    install_container_script(
        &temp,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$*" in
  "list --all --format json") printf '[]\n' ;;
  "volume list --format json")
    printf '[{"id":"%s","configuration":{"name":"%s","labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"%s"}}},{"id":"some-other-volume","configuration":{"name":"%s","labels":{}}}]\n' "$VOLUME" "$VOLUME" "$ID" "$VOLUME"
    ;;
  *) exit 42 ;;
esac
"#,
    );
    let calls = temp.path().join("calls");
    let inherited = std::env::var("PATH").unwrap_or_default();
    let output = Command::new(script())
        .arg(&path)
        .arg(fs::canonicalize(temp.path().join("trusted-gascan")).unwrap())
        .arg(fs::canonicalize(temp.path()).unwrap())
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("CALLS", &calls)
        .env("ID", id)
        .env("VOLUME", format!("gascan-mise-{id}"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(path.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(!calls.lines().any(|line| line.starts_with("volume delete ")));
}

#[test]
fn unexpected_session_entry_is_residue_and_retains_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let session = temp.path().join("session-test");
    fs::write(session.join("unexpected"), "retain me").unwrap();
    install_container_script(
        &temp,
        r#"#!/bin/sh
case "$*" in
  "list --all --format json"|"volume list --format json") printf '[]\n' ;;
  *) exit 42 ;;
esac
"#,
    );
    let calls = temp.path().join("calls");

    let output = run(&temp, &path, &calls);

    assert!(!output.status.success());
    assert!(path.exists());
    assert!(session.join("unexpected").exists());
}

#[test]
fn absent_child_spelled_with_traversal_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let session = temp.path().join("session-test");
    value["runtime_root"] = serde_json::json!(session.join("missing/../gascan-gate4-runtime-test"));
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let output = run(&temp, &path, &calls);

    assert_eq!(output.status.code(), Some(65));
    assert!(!calls.exists());
    assert!(path.exists());
}

#[test]
fn exact_name_collision_is_retained_and_never_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let inherited = std::env::var("PATH").unwrap_or_default();
    let output = Command::new(script())
        .arg(&path)
        .arg(fs::canonicalize(temp.path().join("trusted-gascan")).unwrap())
        .arg(fs::canonicalize(temp.path()).unwrap())
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("CALLS", &calls)
        .env("ID", id)
        .env("LABEL_ID", "somebody-else-123456789abc")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(path.exists());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(!calls
        .lines()
        .any(|line| line.starts_with("delete ") || line.starts_with("volume delete ")));
}

#[test]
fn invalid_daemon_pid_is_refused_without_signalling() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let instance = temp
        .path()
        .join("session-test/gascan-gate4-runtime-test/daemon-instance.json");
    fs::write(&instance, r#"{"owner_token":"test-owner","pid":"-1","executable":"/missing/gascand","start_identity":"start","instance_token":"instance"}"#).unwrap();
    fs::set_permissions(&instance, fs::Permissions::from_mode(0o600)).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["daemon_instance_path"] = serde_json::json!(fs::canonicalize(instance).unwrap());
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = run(&temp, &path, &calls);
    assert_eq!(output.status.code(), Some(65));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid daemon pid"));
    assert!(path.exists());
}

fn daemon_cleanup_fixture(
    temp: &tempfile::TempDir,
    stuck_term: bool,
    unkillable: bool,
) -> (PathBuf, PathBuf) {
    let id = "gate4-test-123456789abc";
    let path = manifest(
        temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let daemon = temp.path().join("gascand");
    fs::write(&daemon, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&daemon, fs::Permissions::from_mode(0o755)).unwrap();
    let daemon = fs::canonicalize(daemon).unwrap();
    let instance = temp
        .path()
        .join("session-test/gascan-gate4-runtime-test/daemon-instance.json");
    fs::write(
        &instance,
        serde_json::to_vec(&serde_json::json!({
            "owner_token":"test-owner", "pid":4242, "executable":daemon,
            "start_identity":"START", "instance_token":"INSTANCE"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&instance, fs::Permissions::from_mode(0o600)).unwrap();
    let gascan = temp.path().join("trusted-gascan");
    fs::write(&gascan, format!("#!/bin/sh\nprintf '%s\\n' '{{\"instance_token\":\"INSTANCE\",\"pid\":4242,\"executable\":\"{}\",\"start_identity\":\"START\"}}'\n", daemon.display())).unwrap();
    fs::set_permissions(&gascan, fs::Permissions::from_mode(0o755)).unwrap();
    let gascan = fs::canonicalize(gascan).unwrap();
    let ps = temp.path().join("ps");
    fs::write(
        &ps,
        format!(
            r#"#!/bin/sh
test ! -f "$STATE" || exit 1
case "$*" in *command=*) printf '%s\n' '{}' ;; *) printf '%s\n' START ;; esac
"#,
            daemon.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&ps, fs::Permissions::from_mode(0o755)).unwrap();
    let kill = temp.path().join("kill");
    fs::write(
        &kill,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >>"$KILL_CALLS"
case "$1" in
  -TERM) test {} = true || : >"$STATE" ;;
  -KILL) test {} = true || : >"$STATE" ;;
esac
"#,
            stuck_term, unkillable
        ),
    )
    .unwrap();
    fs::set_permissions(&kill, fs::Permissions::from_mode(0o755)).unwrap();
    let sleep = temp.path().join("sleep");
    fs::write(&sleep, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&sleep, fs::Permissions::from_mode(0o755)).unwrap();
    install_absent_container(temp);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["daemon_instance_path"] = serde_json::json!(fs::canonicalize(instance).unwrap());
    value["daemon_executable"] = serde_json::json!(daemon);
    value["daemon_cli"] = serde_json::json!(gascan);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    (path, temp.path().join("kill-calls"))
}

fn run_daemon_cleanup(
    temp: &tempfile::TempDir,
    path: &PathBuf,
    calls: &PathBuf,
) -> std::process::Output {
    let inherited = std::env::var("PATH").unwrap_or_default();
    Command::new(script())
        .arg(path)
        .arg(fs::canonicalize(temp.path().join("trusted-gascan")).unwrap())
        .arg(fs::canonicalize(temp.path()).unwrap())
        .env("PATH", format!("{}:{inherited}", temp.path().display()))
        .env("STATE", temp.path().join("dead"))
        .env("KILL_CALLS", calls)
        .output()
        .unwrap()
}

#[test]
fn validated_daemon_gets_bounded_term_then_revalidated_kill() {
    let temp = tempfile::tempdir().unwrap();
    let (path, calls) = daemon_cleanup_fixture(&temp, true, false);
    let output = run_daemon_cleanup(&temp, &path, &calls);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&calls).unwrap(),
        "-TERM 4242\n-KILL 4242\n"
    );
    assert!(!path.exists());
}

#[test]
fn daemon_residue_retains_manifest_after_term_and_kill() {
    let temp = tempfile::tempdir().unwrap();
    let (path, calls) = daemon_cleanup_fixture(&temp, true, true);
    let output = run_daemon_cleanup(&temp, &path, &calls);
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(&calls).unwrap(),
        "-TERM 4242\n-KILL 4242\n"
    );
    assert!(path.exists());
    fs::write(temp.path().join("dead"), b"").unwrap();
    let retry = run_daemon_cleanup(&temp, &path, &calls);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(!path.exists());
    assert!(!temp
        .path()
        .join("session-test/gascan-gate4-runtime-test")
        .exists());
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
fn forged_manifest_cli_is_never_executed() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let marker = temp.path().join("forged-cli-ran");
    let forged = temp.path().join("forged-gascan");
    fs::write(&forged, format!("#!/bin/sh\n: >'{}'\n", marker.display())).unwrap();
    fs::set_permissions(&forged, fs::Permissions::from_mode(0o755)).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["daemon_cli"] = serde_json::json!(fs::canonicalize(forged).unwrap());
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = run(&temp, &path, &calls);
    assert_eq!(output.status.code(), Some(65));
    assert!(!marker.exists());
    assert!(!calls.exists());
}

#[test]
fn runtime_path_escape_is_refused_before_commands() {
    let temp = tempfile::tempdir().unwrap();
    let (_bin, calls) = fake_container(&temp);
    let id = "gate4-test-123456789abc";
    let path = manifest(
        &temp,
        serde_json::json!([
            id,
            format!("gascan-mise-{id}"),
            format!("gascan-cache-{id}"),
            format!("gascan-config-{id}")
        ]),
    );
    let escaped = tempfile::tempdir().unwrap();
    fs::set_permissions(escaped.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["runtime_root"] = serde_json::json!(fs::canonicalize(escaped.path()).unwrap());
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = run(&temp, &path, &calls);
    assert_eq!(output.status.code(), Some(65));
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
    let output = run(&temp, &path, &calls);
    assert!(
        calls.exists(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(calls).unwrap();
    let stop = calls.find(&format!("stop --time 5 {id}")).unwrap();
    let delete = calls.find(&format!("delete {id}")).unwrap();
    assert!(stop < delete);
}
