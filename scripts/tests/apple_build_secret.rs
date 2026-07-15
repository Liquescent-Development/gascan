use std::{fs, os::unix::fs::{symlink, PermissionsExt}, path::Path, process::Command, thread, time::{Duration, Instant}};

const SECRET: &str = "synthetic-apple-build-secret-0011223344556677";

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn probe_requires_private_external_file_and_checks_non_retention() {
    let probe = include_str!("../probe-apple-build-secret.sh");
    for required in [
        "test \"$uid\" = \"$(stat -f %u \"$secret\")\"",
        "test \"600\" = \"$(stat -f %Lp \"$secret\")\"",
        "printf '%s\\n' '.build-secrets' >\"$context/.dockerignore\"",
        "--secret \"id=gascamp_read_token,src=$staged_secret\"",
        "RUN --mount=type=secret,id=gascamp_read_token,required=true",
        "test ! -e /run/secrets/gascamp_read_token",
        "container image inspect --format json",
        "test ! -L \"$GASCAN_TEST_SECRET_FILE\"",
        "com.gascan.build-secret-probe",
        "run_bounded",
    ] {
        assert!(probe.contains(required), "missing secret safeguard: {required}");
    }
    assert!(probe.contains("EXPECTED_SECRET_SHA256"));
    assert!(probe.contains("build failed; sanitized transcript follows"));
    for forbidden in ["GASCAMP_READ_TOKEN=", "ENV GASCAMP", "ARG GASCAMP"] {
        assert!(!probe.contains(forbidden), "secret-bearing channel: {forbidden}");
    }
}

#[test]
fn probe_rejects_original_symlink_before_invoking_container() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let secret = fixture.path().join("secret");
    let link = fixture.path().join("secret-link");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&secret, &link).unwrap();
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("GASCAN_TEST_SECRET_FILE", &link)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symbolic link"));
}

#[test]
fn container_timeout_is_bounded_nonzero_and_cleans_private_context() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin = fixture.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("container");
    fs::write(&fake, "#!/bin/sh\ncase \"$1\" in build) sleep 3 ;; esac\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let started = Instant::now();
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", path)
        .env("TMPDIR", fixture.path())
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("GASCAN_PROBE_TIMEOUT_SECONDS", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(started.elapsed() < Duration::from_millis(2500), "timeout was not bounded");
    assert!(fs::read_dir(fixture.path()).unwrap().all(|entry| {
        !entry.unwrap().file_name().to_string_lossy().starts_with("gascan-build-secret-probe.")
    }));
}

#[test]
fn ownership_mismatch_fails_cleanup_without_mutating_foreign_resources() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin = fixture.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("container");
    fs::write(&fake, r#"#!/bin/sh
set -eu
case "$1 $2" in
  "image inspect")
    test "${3:-}" = --help && exit 0
    count=0; test ! -f "$INSPECT_COUNT" || count=$(cat "$INSPECT_COUNT"); count=$((count + 1)); printf '%s' "$count" >"$INSPECT_COUNT"
    name=$(cat "$TAG"); test "$count" = 1 || name=foreign:image
    printf '[{"id":"sha256:fixture","configuration":{"name":"%s"},"variants":[{"config":{"config":{"Labels":{"com.gascan.build-secret-probe":"%s"}}}}]}]\n' "$name" "$(cat "$MARKER")"
    ;;
  "image delete") touch "$IMAGE_DELETED" ;;
  "build --secret")
    previous=""
    for argument in "$@"; do
      case "$previous" in
        --label) printf '%s' "${argument#*=}" >"$MARKER" ;;
        --tag) printf '%s' "$argument" >"$TAG" ;;
      esac
      previous="$argument"
    done
    ;;
  "create --name")
    printf '%s' "$3" >"$CONTAINER_NAME"
    ;;
  "inspect "*)
    printf '[{"id":"foreign-container","configuration":{"id":"foreign-container","labels":{"com.gascan.build-secret-probe":"%s"}}}]\n' "$(cat "$MARKER")"
    ;;
  "export "*) printf '%s' clean >"$5" ;;
  "delete "*) touch "$CONTAINER_DELETED" ;;
esac
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", path)
        .env("TMPDIR", fixture.path())
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("TAG", fixture.path().join("tag"))
        .env("MARKER", fixture.path().join("marker"))
        .env("CONTAINER_NAME", fixture.path().join("container-name"))
        .env("INSPECT_COUNT", fixture.path().join("inspect-count"))
        .env("IMAGE_DELETED", fixture.path().join("image-deleted"))
        .env("CONTAINER_DELETED", fixture.path().join("container-deleted"))
        .output().unwrap();
    assert!(!output.status.success(), "ownership mismatch unexpectedly passed");
    assert!(!fixture.path().join("container-deleted").exists(), "foreign container was deleted");
    assert!(!fixture.path().join("image-deleted").exists(), "cleanup continued after ownership mismatch");
}

fn assert_signal_during_active_cli_is_bounded_reaped_and_non_mutating(signal: &str) {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin = fixture.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("container");
    fs::write(&fake, r#"#!/bin/sh
set -eu
case "$1" in
  build)
    printf '%s' "$$" >"$CHILD_PID"
    trap 'exit 143' TERM INT
    while :; do sleep 1; done
    ;;
  delete) touch "$MUTATED" ;;
  image) test "$2" != delete || touch "$MUTATED" ;;
esac
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let child_pid = fixture.path().join("child-pid");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut probe = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", path)
        .env("TMPDIR", fixture.path())
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("GASCAN_PROBE_TIMEOUT_SECONDS", "10")
        .env("CHILD_PID", &child_pid)
        .env("MUTATED", fixture.path().join("mutated"))
        .spawn().unwrap();
    for _ in 0..100 {
        if child_pid.exists() { break; }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child_pid.exists(), "fake CLI did not start");
    let fake_pid: i32 = fs::read_to_string(&child_pid).unwrap().parse().unwrap();
    let started = Instant::now();
    assert!(Command::new("kill").args([signal, &probe.id().to_string()]).status().unwrap().success());
    let status = probe.wait().unwrap();
    assert!(!status.success(), "{signal} returned success");
    assert!(started.elapsed() < Duration::from_secs(3), "{signal} cleanup was not bounded");
    thread::sleep(Duration::from_millis(100));
    assert!(!Command::new("kill").args(["-0", &fake_pid.to_string()]).status().unwrap().success(), "fake CLI child survived {signal}");
    assert!(!fixture.path().join("mutated").exists(), "resource mutation occurred after {signal}");
    assert!(fs::read_dir(fixture.path()).unwrap().all(|entry| {
        !entry.unwrap().file_name().to_string_lossy().starts_with("gascan-build-secret-probe.")
    }), "private context survived {signal}");
}

#[test]
fn term_during_active_cli_is_bounded_reaped_and_non_mutating() {
    assert_signal_during_active_cli_is_bounded_reaped_and_non_mutating("-TERM");
}

#[test]
fn int_during_active_cli_is_bounded_reaped_and_non_mutating() {
    assert_signal_during_active_cli_is_bounded_reaped_and_non_mutating("-INT");
}

#[test]
fn failed_build_attempt_cleans_image_created_before_failure() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin = fixture.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("container");
    fs::write(&fake, r#"#!/bin/sh
set -eu
case "$1 $2" in
  "build --secret")
    previous=""; for argument in "$@"; do
      case "$previous" in --label) printf '%s' "${argument#*=}" >"$MARKER" ;; --tag) printf '%s' "$argument" >"$TAG" ;; esac
      previous="$argument"
    done
    exit 1
    ;;
  "image inspect")
    printf '[{"id":"sha256:created-before-failure","configuration":{"name":"%s"},"variants":[{"config":{"config":{"Labels":{"com.gascan.build-secret-probe":"%s"}}}}]}]' "$(cat "$TAG")" "$(cat "$MARKER")"
    ;;
  "image delete") touch "$DELETED" ;;
esac
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
        .env("TMPDIR", fixture.path())
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("MARKER", fixture.path().join("marker"))
        .env("TAG", fixture.path().join("tag"))
        .env("DELETED", fixture.path().join("deleted"))
        .output().unwrap();
    assert!(!output.status.success());
    assert!(fixture.path().join("deleted").exists(), "owned image survived failed build attempt");
}

#[test]
fn failed_create_attempt_cleans_container_created_before_failure() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin = fixture.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("container");
    fs::write(&fake, r#"#!/bin/sh
set -eu
case "$1 $2" in
  "build --secret")
    previous=""; for argument in "$@"; do
      case "$previous" in --label) printf '%s' "${argument#*=}" >"$MARKER" ;; --tag) printf '%s' "$argument" >"$TAG" ;; esac
      previous="$argument"
    done
    ;;
  "image inspect")
    test "${3:-}" = --help && exit 0
    printf '[{"id":"sha256:fixture","configuration":{"name":"%s"},"variants":[{"config":{"config":{"Labels":{"com.gascan.build-secret-probe":"%s"}}}}]}]' "$(cat "$TAG")" "$(cat "$MARKER")"
    ;;
  "image delete") : ;;
  "create --name")
    printf '%s' "$3" >"$NAME"
    exit 1
    ;;
  "inspect "*)
    printf '[{"id":"%s","configuration":{"id":"%s","labels":{"com.gascan.build-secret-probe":"%s"}}}]' "$(cat "$NAME")" "$(cat "$NAME")" "$(cat "$MARKER")"
    ;;
  "stop "*) : ;;
  "delete "*) touch "$DELETED" ;;
esac
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
        .env("TMPDIR", fixture.path())
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("MARKER", fixture.path().join("marker"))
        .env("TAG", fixture.path().join("tag"))
        .env("NAME", fixture.path().join("name"))
        .env("DELETED", fixture.path().join("deleted"))
        .output().unwrap();
    assert!(!output.status.success());
    assert!(fixture.path().join("deleted").exists(), "owned container survived failed create attempt");
}

#[test]
fn fake_container_proves_secret_stays_out_of_observable_channels() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let bin_dir = fixture.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let calls = fixture.path().join("calls");
    let retained = fixture.path().join("retained-context");
    let staged_path = fixture.path().join("staged-path");
    let marker_path = fixture.path().join("marker");
    let tag_path = fixture.path().join("tag");
    let container_name_path = fixture.path().join("container-name");
    let fake = bin_dir.join("container");
    fs::write(
        &fake,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$CALLS"
case "$1" in
  build)
    context=""
    previous=""
    for argument in "$@"; do
      case "$previous" in
        --secret) printf '%s\n' "${argument#*,src=}" >"$STAGED_PATH" ;;
        --label) printf '%s' "${argument#*=}" >"$MARKER_PATH" ;;
        --tag) printf '%s' "$argument" >"$TAG_PATH" ;;
      esac
      previous="$argument"
      context="$argument"
    done
    test "$(cat "$context/.dockerignore")" = .build-secrets || { printf '%s\n' bad-dockerignore >&2; exit 1; }
    test -f "$(cat "$STAGED_PATH")" || { printf '%s\n' missing-staged-secret >&2; exit 1; }
    test "$(cat "$(cat "$STAGED_PATH")")" = "$SECRET_VALUE" || { printf '%s\n' wrong-staged-secret >&2; exit 1; }
    mkdir "$RETAINED_CONTEXT"
    cp "$context/Dockerfile" "$context/.dockerignore" "$RETAINED_CONTEXT/"
    test ! -e "$RETAINED_CONTEXT/.build-secrets"
    ;;
  image)
    case "$2" in
      inspect) printf '[{"id":"sha256:fixture","configuration":{"name":"%s"},"variants":[{"config":{"config":{"Labels":{"com.gascan.build-secret-probe":"%s"}},"history":[]}}]}]' "$(cat "$TAG_PATH")" "$(cat "$MARKER_PATH")" ;;
      delete) : ;;
    esac
    ;;
  create)
    previous=""
    for argument in "$@"; do
      case "$previous" in --name) printf '%s' "$argument" >"$CONTAINER_NAME_PATH" ;; esac
      previous="$argument"
    done
    ;;
  inspect) printf '[{"id":"%s","configuration":{"id":"%s","labels":{"com.gascan.build-secret-probe":"%s"}}}]' "$(cat "$CONTAINER_NAME_PATH")" "$(cat "$CONTAINER_NAME_PATH")" "$(cat "$MARKER_PATH")" ;;
  export)
    test "$3" = --output
    printf '%s\n' synthetic-export-without-token >"$4"
    ;;
  delete) : ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let secret = fixture.path().join("secret");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
    let output = Command::new("bash")
        .arg(repository_root().join("scripts/probe-apple-build-secret.sh"))
        .env("PATH", path)
        .env("GASCAN_TEST_SECRET_FILE", &secret)
        .env("CALLS", &calls)
        .env("RETAINED_CONTEXT", &retained)
        .env("STAGED_PATH", &staged_path)
        .env("SECRET_VALUE", SECRET)
        .env("MARKER_PATH", &marker_path)
        .env("TAG_PATH", &tag_path)
        .env("CONTAINER_NAME_PATH", &container_name_path)
        .env("TMPDIR", format!("{}/", fixture.path().display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    for (name, bytes) in [
        ("argv", fs::read(&calls).unwrap()),
        ("stdout", output.stdout),
        ("stderr", output.stderr),
        ("Dockerfile", fs::read(retained.join("Dockerfile")).unwrap()),
        ("transmitted context", fs::read(retained.join(".dockerignore")).unwrap()),
    ] {
        assert!(!bytes.windows(SECRET.len()).any(|window| window == SECRET.as_bytes()), "secret retained in {name}");
    }
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains("build --secret id=gascamp_read_token,src="));
    assert!(calls.contains("delete gascan-build-secret-probe-"));
    assert!(calls.contains("image delete"));
    let staged = fs::read_to_string(staged_path).unwrap();
    let staged = Path::new(staged.trim());
    assert!(!staged.to_string_lossy().contains("//"), "staged path was not normalized");
    assert!(staged.ends_with(".build-secrets/gascamp_read_token"));
    assert!(!staged.exists(), "staged secret survived cleanup");
    assert!(!staged.parent().unwrap().parent().unwrap().exists(), "private context survived cleanup");
}
