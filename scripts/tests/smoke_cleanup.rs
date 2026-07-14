use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

const TOKEN: &str = "00112233445566778899aabbccddeeff";

fn fake_container(mode: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("container");
    let log = temp.path().join("calls");
    fs::write(
        &bin,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$1" in
  create)
    if [ "$MODE" = collision ]; then exit 1; fi
    if [ "$MODE" = signal ]; then kill -TERM "$PPID"; fi
    ;;
  inspect)
    if [ "$MODE" = collision ]; then owner=ffeeddccbbaa99887766554433221100; else owner="$OWNER"; fi
    printf '[{"configuration":{"id":"%s","name":"%s","labels":{"dev.gascan.test":"true","dev.gascan.test.owner":"%s"}}}]\n' "$NAME" "$NAME" "$owner"
    ;;
  start)
    [ "$MODE" != start-fails ]
    ;;
  exec) exit 0 ;;
  stop) exit 0 ;;
  delete) exit 0 ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&bin).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bin, permissions).unwrap();
    fs::write(
        temp.path().join("ref"),
        "gascan-workspace:test@sha256:abc\n",
    )
    .unwrap();
    let _ = mode;
    (temp, bin, log)
}

fn run(mode: &str) -> (std::process::Output, String) {
    let (temp, bin, log) = fake_container(mode);
    let name = format!("gascan-image-user-test-{TOKEN}");
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts directory has repository parent")
        .join("tests/image/user-and-volumes.sh");
    let output = Command::new("bash")
        .arg(script)
        .env("CONTAINER_BIN", &bin)
        .env("GASCAN_IMAGE_REF_FILE", temp.path().join("ref"))
        .env("GASCAN_TEST_OWNER_TOKEN", TOKEN)
        .env("MODE", mode)
        .env("CALLS", &log)
        .env("OWNER", TOKEN)
        .env("NAME", name)
        .output()
        .unwrap();
    let calls = fs::read_to_string(log).unwrap_or_default();
    (output, calls)
}

#[test]
fn create_success_then_start_failure_is_owned_and_deleted() {
    let (output, calls) = run("start-fails");
    assert!(!output.status.success());
    assert!(calls.contains("create "), "stderr={}", String::from_utf8_lossy(&output.stderr));
    assert!(calls.contains("start "));
    assert!(calls.contains("delete "));
}

#[test]
fn signal_after_create_side_effect_still_cleans_exact_owner() {
    let (output, calls) = run("signal");
    assert!(!output.status.success());
    assert!(calls.contains("create "), "stderr={}", String::from_utf8_lossy(&output.stderr));
    assert!(calls.contains("inspect "));
    assert!(calls.contains("delete "));
}

#[test]
fn wrong_label_collision_is_retained_without_delete() {
    let (output, calls) = run("collision");
    assert!(!output.status.success());
    assert!(calls.contains("create "), "stderr={}", String::from_utf8_lossy(&output.stderr));
    assert!(!calls.contains("delete "));
}
