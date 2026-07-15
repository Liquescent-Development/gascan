use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

struct FakeRun {
    temporary: tempfile::TempDir,
    repository: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
    transmitted: PathBuf,
    secret: PathBuf,
}

impl FakeRun {
    fn new() -> Self {
        let temporary = tempfile::tempdir_in("/tmp").unwrap();
        let repository = temporary.path().join("repo");
        let bin = temporary.path().join("bin");
        let calls = temporary.path().join("calls");
        let transmitted = temporary.path().join("transmitted");
        fs::create_dir_all(repository.join("scripts")).unwrap();
        fs::create_dir_all(repository.join("images/workspace")).unwrap();
        fs::create_dir_all(repository.join(".artifacts/connected-workspace-context")).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(
            repository.join("scripts/build-connected-workspace-image.sh"),
            include_str!("../build-connected-workspace-image.sh"),
        )
        .unwrap();
        fs::write(
            repository.join("images/workspace/versions.lock"),
            format!("base_image = \"ubuntu@sha256:{}\"\nworkspace_build_mode = \"connected\"\nworkspace_tag = \"gascan-workspace:fixture\"\n[gascamp]\nrevision = \"f6b248c5926240856dbea83d1d2c5c90ea1c1456\"\n", "7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab"),
        )
        .unwrap();
        fs::write(
            repository.join(".artifacts/connected-workspace-context/Dockerfile"),
            "FROM scratch\n",
        )
        .unwrap();
        fs::write(
            repository.join(".artifacts/connected-workspace-context/context-manifest.tsv"),
            "fixture\n",
        )
        .unwrap();
        let cargo = r#"#!/usr/bin/env bash
set -eu
printf 'cargo\t%s\n' "$*" >>"$CALLS"
case "$*" in
  *snapshot-helper-identity*) printf 'hash\t1\t2\n' ;;
  *'prepare-workspace-context -- --verify-connected'*) printf '%064d\n' 0 ;;
  *'validate-image-inspect'*) cat >/dev/null; printf 'sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\n' ;;
  *'validate-connected-build -- stage-secret'*)
    case "${FAULT:-}" in stage_create|stage_permission|exclusion) exit 81 ;; esac
    "$VALIDATOR" stage-secret "${@: -3:1}" "${@: -2:1}" "${@: -1}" ;;
  *'validate-connected-build -- copy-public'*)
    test "${FAULT:-}" != public_before || exit 81
    snapshot=${@: -3:1}; wrapper=${@: -2:1}; cp -R "$snapshot"/. "$wrapper"/ ;;
  *'validate-connected-build -- prepare-wrapper'*)
    snapshot=${@: -4:1}; wrapper=${@: -3:1}; secret=${@: -2:1}
    case "${FAULT:-}" in public_before|stage_create|stage_permission|exclusion) exit 81 ;; esac
    cp -R "$snapshot"/. "$wrapper"/; mkdir "$wrapper/.build-secrets"
    cp "$secret" "$wrapper/.build-secrets/gascamp_read_token"
    chmod 700 "$wrapper/.build-secrets"; chmod 600 "$wrapper/.build-secrets/gascamp_read_token"
    printf '.build-secrets\n' >"$wrapper/.dockerignore"; printf '%064d\n' 8 ;;
  *'validate-connected-build -- verify-wrapper'*)
    wrapper=${@: -3:1}
    test "$(cat "$wrapper/context-manifest.tsv")" = fixture
    test "$(cat "$wrapper/.dockerignore")" = .build-secrets
    test ! -L "$wrapper/.build-secrets/gascamp_read_token"
    test "$(stat -f %Lp "$wrapper/.build-secrets/gascamp_read_token")" = 600
    test "$(cat "$wrapper/.build-secrets/gascamp_read_token")" = matrix-synthetic-token ;;
  *'validate-connected-build -- validate-receipt'*) "$VALIDATOR" validate-receipt "${@: -4:1}" "${@: -3:1}" "${@: -2:1}" "${@: -1}" ;;
  *'validate-connected-build -- gascan-workspace:fixture'*)
    cat >/dev/null
    test "${FAULT:-}" != inspect_invalid || exit 82
    printf 'sha256:%064d\n' 9 ;;
  *) exit 91 ;;
esac
"#;
        let sudo = r#"#!/usr/bin/env bash
set -eu
printf 'sudo\t%s\n' "$*" >>"$CALLS"
case " $* " in *' create '*) printf 'receipt\n' ;; *' path '*) printf '%s\n' "$SNAPSHOT" ;; *' finish '*) test "${FAULT:-}" != finish_hang || sleep 30 ;; *) exit 92 ;; esac
"#;
        let container = r#"#!/usr/bin/env bash
set -eu
printf 'container' >>"$CALLS"; printf '\t%s' "$@" >>"$CALLS"; printf '\n' >>"$CALLS"
case "$*" in
  'image inspect --format json '*) printf '{}\n' ;;
  build*)
    context=${@: -1}
    printf 'modes\t%s\t%s\t%s\n' "$(stat -f %Lp "$context")" "$(stat -f %Lp "$context/.build-secrets/gascamp_read_token")" "$(cat "$context/.dockerignore")" >>"$CALLS"
    find "$context" -path "$context/.build-secrets" -prune -o -type f -print >"$TRANSMITTED"
    case "${FAULT:-}" in
      build_fail) exit 83 ;;
      public_after) printf 'changed\n' >"$context/context-manifest.tsv" ;;
      secret_content) printf 'changed\n' >"$context/.build-secrets/gascamp_read_token" ;;
      secret_mode) chmod 644 "$context/.build-secrets/gascamp_read_token" ;;
      secret_symlink) rm "$context/.build-secrets/gascamp_read_token"; ln -s /dev/null "$context/.build-secrets/gascamp_read_token" ;;
      signal_int) kill -INT "$PPID"; sleep 1 ;;
      signal_term) kill -TERM "$PPID"; sleep 1 ;;
    esac ;;
  *) exit 93 ;;
esac
"#;
        let sw_vers = "#!/bin/sh\nprintf '14.0\n'\n";
        let mv = r#"#!/usr/bin/env bash
set -eu
printf 'mv\t%s\n' "$*" >>"$CALLS"
destination=${@: -1}
case "${FAULT:-}:$destination" in
  fail_json:*/workspace-image-build.json|interrupt_json:*/workspace-image-build.json)
    test "${FAULT:-}" != interrupt_json || kill -INT "$PPID"
    exit 84 ;;
  fail_ref:*/workspace-image-ref|interrupt_ref:*/workspace-image-ref)
    test "${FAULT:-}" != interrupt_ref || kill -TERM "$PPID"
    exit 85 ;;
esac
exec /bin/mv "$@"
"#;
        for (name, body) in [
            ("cargo", cargo),
            ("sudo", sudo),
            ("container", container),
            ("sw_vers", sw_vers),
            ("mv", mv),
        ] {
            let path = bin.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::set_permissions(
            repository.join("scripts/build-connected-workspace-image.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let secret = temporary.path().join("token-source");
        fs::write(&secret, "matrix-synthetic-token\n").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            temporary,
            repository,
            bin,
            calls,
            transmitted,
            secret,
        }
    }

    fn run_with_secret(&self, secret: &Path) -> std::process::Output {
        self.run(secret, "")
    }

    fn run(&self, secret: &Path, fault: &str) -> std::process::Output {
        let mut command = Command::new("bash");
        command
            .arg(
                self.repository
                    .join("scripts/build-connected-workspace-image.sh"),
            )
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("CALLS", &self.calls)
            .env("TRANSMITTED", &self.transmitted)
            .env("VALIDATOR", env!("CARGO_BIN_EXE_validate-connected-build"))
            .env(
                "SNAPSHOT",
                self.repository
                    .join(".artifacts/connected-workspace-context"),
            )
            .env("TMPDIR", self.temporary.path())
            .env("GASCAMP_READ_TOKEN_FILE", secret)
            .env("GASCAN_CONNECTED_TIMEOUT_SECONDS", "1")
            .env("FAULT", fault);
        command.output().unwrap()
    }

    fn run_missing_secret(&self) -> std::process::Output {
        let mut command = Command::new("bash");
        command
            .arg(
                self.repository
                    .join("scripts/build-connected-workspace-image.sh"),
            )
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("CALLS", &self.calls)
            .env("TRANSMITTED", &self.transmitted)
            .env("VALIDATOR", env!("CARGO_BIN_EXE_validate-connected-build"))
            .env(
                "SNAPSHOT",
                self.repository
                    .join(".artifacts/connected-workspace-context"),
            )
            .env("TMPDIR", self.temporary.path())
            .env("GASCAN_CONNECTED_TIMEOUT_SECONDS", "1")
            .env_remove("GASCAMP_READ_TOKEN_FILE");
        command.output().unwrap()
    }
}

#[test]
fn successful_fake_run_enforces_build_helper_wrapper_receipt_and_secrecy_contracts() {
    let fixture = FakeRun::new();
    let output = fixture.run_with_secret(&fixture.secret);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(&fixture.calls).unwrap();
    let stage = calls
        .find("validate-connected-build -- stage-secret")
        .expect("secret was not descriptor-staged");
    let helper_create = calls.find("sudo\t").unwrap();
    assert!(
        stage < helper_create,
        "secret staging followed helper creation"
    );
    let helper: Vec<_> = calls
        .lines()
        .filter(|line| line.starts_with("sudo\t"))
        .collect();
    assert_eq!(helper.len(), 3);
    for (line, operation) in helper.iter().zip([" create ", " path ", " finish "]) {
        assert!(line.contains(operation), "wrong helper operation: {line}");
        assert!(!line.contains(fixture.secret.to_str().unwrap()));
        assert!(!line.contains(".build-secrets"));
    }
    let build = calls
        .lines()
        .find(|line| line.starts_with("container\tbuild\t"))
        .unwrap();
    assert!(calls
        .lines()
        .any(|line| line == "modes\t700\t600\t.build-secrets"));
    let args: Vec<_> = build.split('\t').skip(1).collect();
    assert_eq!(args.iter().filter(|arg| **arg == "--secret").count(), 1);
    let secret_index = args.iter().position(|arg| *arg == "--secret").unwrap();
    let mut public_args = args.clone();
    public_args.drain(secret_index..=secret_index + 1);
    assert_eq!(
        &public_args[..7],
        [
            "build",
            "--arch",
            "arm64",
            "--build-arg",
            "BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab",
            "--build-arg",
            "GASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456",
        ]
    );
    for required in [
        "build",
        "--arch",
        "arm64",
        "BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab",
        "GASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456",
    ] {
        assert!(
            args.contains(&required),
            "missing build argument {required}"
        );
    }
    let wrapper = Path::new(args.last().unwrap());
    assert!(wrapper.starts_with(fs::canonicalize(fixture.temporary.path()).unwrap()));
    assert!(args.contains(
        &format!(
            "id=gascamp_read_token,src={}/.build-secrets/gascamp_read_token",
            wrapper.display()
        )
        .as_str()
    ));
    assert!(!wrapper.exists(), "wrapper retained after success");
    let transmitted = fs::read_to_string(&fixture.transmitted).unwrap();
    assert!(!transmitted.contains(".build-secrets"));
    let reference =
        fs::read_to_string(fixture.repository.join(".artifacts/workspace-image-ref")).unwrap();
    let receipt = fs::read_to_string(
        fixture
            .repository
            .join(".artifacts/workspace-image-build.json"),
    )
    .unwrap();
    assert!(receipt.contains(reference.trim()));
    let moves: Vec<_> = calls
        .lines()
        .filter(|line| line.starts_with("mv\t"))
        .collect();
    assert_eq!(moves.len(), 2);
    assert!(moves[0].ends_with("workspace-image-build.json"));
    assert!(moves[1].ends_with("workspace-image-ref"));
    for channel in [
        calls.as_bytes(),
        &output.stdout,
        &output.stderr,
        reference.as_bytes(),
        receipt.as_bytes(),
        transmitted.as_bytes(),
    ] {
        assert!(!String::from_utf8_lossy(channel).contains("matrix-synthetic-token"));
    }
}

#[test]
fn every_precommit_fault_leaves_no_receipts_and_cleans_only_owned_resources() {
    for fault in [
        "public_before",
        "stage_create",
        "stage_permission",
        "exclusion",
        "build_fail",
        "public_after",
        "secret_content",
        "secret_mode",
        "secret_symlink",
        "inspect_invalid",
        "signal_int",
        "signal_term",
    ] {
        let fixture = FakeRun::new();
        let unrelated = fixture.temporary.path().join("unrelated");
        fs::write(&unrelated, "keep\n").unwrap();
        let output = fixture.run(&fixture.secret, fault);
        assert!(
            !output.status.success(),
            "fault {fault} unexpectedly succeeded"
        );
        assert!(!fixture
            .repository
            .join(".artifacts/workspace-image-ref")
            .exists());
        assert!(!fixture
            .repository
            .join(".artifacts/workspace-image-build.json")
            .exists());
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "keep\n");
        let calls = fs::read_to_string(&fixture.calls).unwrap();
        if matches!(
            fault,
            "public_before" | "stage_create" | "stage_permission" | "exclusion"
        ) {
            assert!(!calls.contains("container\tbuild"), "build ran for {fault}");
        }
        if calls.contains("sudo\t") {
            assert!(
                calls
                    .lines()
                    .any(|line| { line.starts_with("sudo\t") && line.contains(" finish ") }),
                "helper receipt not finished for {fault}"
            );
        }
        for line in calls
            .lines()
            .filter(|line| line.starts_with("container\tbuild\t"))
        {
            let wrapper = Path::new(line.split('\t').next_back().unwrap());
            assert!(!wrapper.exists(), "wrapper retained for {fault}");
        }
    }
}

#[test]
fn publication_faults_never_publish_a_new_reference_with_missing_or_stale_json() {
    let old_reference = format!("gascan-workspace:old@sha256:{}\n", "a".repeat(64));
    let old_json = format!(
        "{{\"reference\":\"{}\",\"image_digest\":\"sha256:{}\"}}\n",
        old_reference.trim(),
        "a".repeat(64)
    );
    for fault in ["fail_json", "interrupt_json", "fail_ref", "interrupt_ref"] {
        let fixture = FakeRun::new();
        fs::write(
            fixture.repository.join(".artifacts/workspace-image-ref"),
            &old_reference,
        )
        .unwrap();
        fs::write(
            fixture
                .repository
                .join(".artifacts/workspace-image-build.json"),
            &old_json,
        )
        .unwrap();
        let output = fixture.run(&fixture.secret, fault);
        assert!(
            !output.status.success(),
            "publication fault {fault} succeeded; calls={}",
            fs::read_to_string(&fixture.calls).unwrap_or_default()
        );
        let reference =
            fs::read_to_string(fixture.repository.join(".artifacts/workspace-image-ref")).unwrap();
        assert_eq!(
            reference, old_reference,
            "new commit marker escaped at {fault}"
        );
        let json = fs::read_to_string(
            fixture
                .repository
                .join(".artifacts/workspace-image-build.json"),
        )
        .unwrap();
        let receipt: serde_json::Value = serde_json::from_str(&json).unwrap();
        let pair_valid = receipt["reference"].as_str() == Some(reference.trim());
        if fault.contains("ref") {
            assert!(!pair_valid, "stale reference accepted new JSON at {fault}");
        } else {
            assert!(pair_valid, "previous committed pair was damaged at {fault}");
        }
    }
}

#[test]
fn hanging_finish_cleanup_is_bounded_and_wrapper_is_removed() {
    let fixture = FakeRun::new();
    let started = Instant::now();
    let output = fixture.run(&fixture.secret, "finish_hang");
    assert!(!output.status.success());
    assert!(started.elapsed() < Duration::from_secs(4));
    let calls = fs::read_to_string(&fixture.calls).unwrap();
    let build = calls
        .lines()
        .find(|line| line.starts_with("container\tbuild\t"))
        .unwrap();
    let wrapper = Path::new(build.split('\t').next_back().unwrap());
    assert!(!wrapper.exists());
    assert!(calls
        .lines()
        .any(|line| line.starts_with("sudo\t") && line.contains(" finish ")));
}

#[test]
fn complete_unsafe_source_matrix_is_rejected_before_privileged_helper_and_build() {
    let missing_environment = FakeRun::new();
    assert!(!missing_environment.run_missing_secret().status.success());
    assert!(!fs::read_to_string(&missing_environment.calls)
        .unwrap_or_default()
        .contains("sudo\t"));
    assert!(!missing_environment
        .repository
        .join(".artifacts/workspace-image-ref")
        .exists());
    for kind in [
        "relative",
        "missing",
        "empty",
        "readable",
        "symlink",
        "repository",
    ] {
        let fixture = FakeRun::new();
        let rejected = match kind {
            "relative" => PathBuf::from("relative-token"),
            "missing" => fixture.temporary.path().join("missing"),
            "empty" => {
                fs::write(&fixture.secret, "").unwrap();
                fixture.secret.clone()
            }
            "readable" => {
                fs::set_permissions(&fixture.secret, fs::Permissions::from_mode(0o644)).unwrap();
                fixture.secret.clone()
            }
            "symlink" => {
                let link = fixture.temporary.path().join("token-link");
                std::os::unix::fs::symlink(&fixture.secret, &link).unwrap();
                link
            }
            "repository" => {
                let path = fixture.repository.join("token");
                fs::write(&path, "synthetic\n").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                path
            }
            _ => unreachable!(),
        };
        let output = fixture.run_with_secret(&rejected);
        assert!(!output.status.success(), "accepted {kind}");
        let calls = fs::read_to_string(&fixture.calls).unwrap_or_default();
        assert!(!calls.contains("sudo\t"), "helper ran for {kind}");
        assert!(!calls.contains("container\tbuild"), "build ran for {kind}");
        assert!(!fixture
            .repository
            .join(".artifacts/workspace-image-ref")
            .exists());
        assert!(!fixture
            .repository
            .join(".artifacts/workspace-image-build.json")
            .exists());
    }
    if rustix::process::getuid().is_root() {
        let fixture = FakeRun::new();
        assert!(Command::new("chown")
            .args(["1", fixture.secret.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        let output = fixture.run_with_secret(&fixture.secret);
        assert!(!output.status.success());
        let calls = fs::read_to_string(&fixture.calls).unwrap_or_default();
        assert!(!calls.contains("sudo\t"));
        assert!(!calls.contains("container\tbuild"));
    }
}

#[test]
fn connected_orchestrator_has_exact_locked_build_shape() {
    let script = fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh"))
        .expect("connected build orchestrator must exist");
    for required in [
        "--arch arm64",
        "id=gascamp_read_token,src=$wrapper/.build-secrets/gascamp_read_token",
        "--build-arg \"BASE_IMAGE=$base_image\"",
        "--build-arg \"GASCAMP_REVISION=$gascamp_revision\"",
        "validate-connected-build",
        "workspace-image-build.json",
    ] {
        assert!(
            script.contains(required),
            "missing connected safeguard: {required}"
        );
    }
}

#[test]
fn wrapper_is_dynamic_unprivileged_and_helper_is_credential_blind() {
    let script =
        fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh")).unwrap();
    for required in [
        "mktemp -d \"$tmp_base/gascan-connected-build.XXXXXX\"",
        "chmod 0700 \"$wrapper\"",
        "stage-secret",
        "copy-public",
        "verify-wrapper",
    ] {
        assert!(
            script.contains(required),
            "missing wrapper boundary: {required}"
        );
    }
    assert!(!script.contains("/private/context"));
    for line in script
        .lines()
        .filter(|line| line.contains("snapshot_helper"))
    {
        assert!(
            !line.contains("secret"),
            "helper received credential path: {line}"
        );
    }
}

#[test]
fn every_privileged_helper_operation_is_bounded() {
    let script =
        fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh")).unwrap();
    assert!(script.contains("run_bounded"));
    for operation in [" create ", " path ", " finish "] {
        for line in script
            .lines()
            .filter(|line| line.contains("snapshot_helper") && line.contains(operation))
        {
            assert!(
                line.contains("run_bounded"),
                "unbounded helper call: {line}"
            );
        }
    }
}

#[test]
fn interrupt_and_termination_preserve_failure_status_while_running_cleanup() {
    let script =
        fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh")).unwrap();
    assert!(script.contains("trap cleanup EXIT"));
    assert!(script.contains("trap 'exit 130' INT"));
    assert!(script.contains("trap 'exit 143' TERM"));
}

#[test]
fn hanging_snapshot_create_is_bounded_before_container_build() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let repo = fixture.path().join("repo");
    let scripts = repo.join("scripts");
    let bin = fixture.path().join("bin");
    fs::create_dir_all(&scripts).unwrap();
    fs::create_dir_all(repo.join("images/workspace")).unwrap();
    fs::create_dir_all(repo.join(".artifacts/connected-workspace-context")).unwrap();
    fs::write(
        scripts.join("build-connected-workspace-image.sh"),
        include_str!("../build-connected-workspace-image.sh"),
    )
    .unwrap();
    fs::write(repo.join("images/workspace/versions.lock"), format!("base_image = \"ubuntu@sha256:{}\"\nworkspace_build_mode = \"connected\"\nworkspace_tag = \"gascan-workspace:fixture\"\n[gascamp]\nrevision = \"f6b248c5926240856dbea83d1d2c5c90ea1c1456\"\n", "7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab")).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("cargo"), "#!/bin/sh\ncase \"$*\" in *snapshot-helper-identity*) printf 'hash\\t1\\t2\\n' ;; *) printf '%064d\\n' 0 ;; esac\n").unwrap();
    fs::write(bin.join("sudo"), "#!/bin/sh\nsleep 30\n").unwrap();
    fs::write(bin.join("container"), "#!/bin/sh\ntouch \"$CALLED\"\n").unwrap();
    for executable in [
        scripts.join("build-connected-workspace-image.sh"),
        bin.join("cargo"),
        bin.join("sudo"),
        bin.join("container"),
    ] {
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let secret = fixture.path().join("token");
    fs::write(&secret, "synthetic\n").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let called = fixture.path().join("called");
    let started = Instant::now();
    let output = Command::new("bash")
        .arg(scripts.join("build-connected-workspace-image.sh"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("GASCAMP_READ_TOKEN_FILE", &secret)
        .env("GASCAN_CONNECTED_TIMEOUT_SECONDS", "1")
        .env("CALLED", &called)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "helper timeout was unbounded"
    );
    assert!(!called.exists());
}

#[test]
fn receipt_reference_is_the_last_atomic_commit_marker() {
    let script =
        fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh")).unwrap();
    let json = script.find("mv -f \"$json_tmp\"").unwrap();
    let reference = script.find("mv -f \"$ref_tmp\"").unwrap();
    assert!(json < reference);
    assert!(script[..reference].contains("validate-connected-build \"$tag\""));
    assert!(script.contains("\"reference\":\"%s\""));
    assert!(script.contains("\"context_digest\":\"%s\""));
    assert!(script.contains("\"lock_digest\":\"%s\""));
}

#[test]
fn wrapper_helper_detects_post_stage_secret_mutation() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let public = fixture.path().join("public");
    let wrapper = fixture.path().join("wrapper");
    fs::create_dir(&public).unwrap();
    fs::write(public.join("context-manifest.tsv"), "fixture\n").unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o555)).unwrap();
    fs::create_dir(&wrapper).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let secret = fixture.path().join("token");
    fs::write(&secret, "synthetic\n").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let digest = format!("{:x}", Sha256::digest(b"fixture\n"));
    let prepare = Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
        .args(["prepare-wrapper"])
        .arg(&public)
        .arg(&wrapper)
        .arg(&secret)
        .arg(&digest)
        .output()
        .unwrap();
    assert!(
        prepare.status.success(),
        "{}",
        String::from_utf8_lossy(&prepare.stderr)
    );
    let identity = String::from_utf8(prepare.stdout).unwrap();
    let run_verify = |identity: &str| {
        Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
            .args(["verify-wrapper"])
            .arg(&wrapper)
            .arg(&digest)
            .arg(identity)
            .status()
            .unwrap()
    };
    assert!(!run_verify(&"0".repeat(64)).success());
    fs::write(
        wrapper.join(".build-secrets/gascamp_read_token"),
        "changed\n",
    )
    .unwrap();
    assert!(!run_verify(identity.trim()).success());
    fs::write(
        wrapper.join(".build-secrets/gascamp_read_token"),
        "synthetic\n",
    )
    .unwrap();
    fs::set_permissions(
        wrapper.join(".build-secrets/gascamp_read_token"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(!run_verify(identity.trim()).success());
    fs::remove_file(wrapper.join(".build-secrets/gascamp_read_token")).unwrap();
    std::os::unix::fs::symlink(
        "/dev/null",
        wrapper.join(".build-secrets/gascamp_read_token"),
    )
    .unwrap();
    assert!(!run_verify(identity.trim()).success());
    fs::remove_file(wrapper.join(".build-secrets/gascamp_read_token")).unwrap();
    fs::write(
        wrapper.join(".build-secrets/gascamp_read_token"),
        "synthetic\n",
    )
    .unwrap();
    fs::set_permissions(
        wrapper.join(".build-secrets/gascamp_read_token"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(wrapper.join(".dockerignore"), "wrong\n").unwrap();
    assert!(!run_verify(identity.trim()).success());
    fs::write(wrapper.join(".dockerignore"), ".build-secrets\n").unwrap();
    fs::write(wrapper.join("context-manifest.tsv"), "changed\n").unwrap();
    assert!(!run_verify(identity.trim()).success());
}

#[test]
fn descriptor_safe_wrapper_helper_rejects_a_source_symlink() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let public = fixture.path().join("public");
    let wrapper = fixture.path().join("wrapper");
    fs::create_dir(&public).unwrap();
    fs::write(public.join("context-manifest.tsv"), "fixture\n").unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o555)).unwrap();
    fs::create_dir(&wrapper).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let target = fixture.path().join("token");
    fs::write(&target, "synthetic\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = fixture.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
        .args(["prepare-wrapper"])
        .arg(&public)
        .arg(&wrapper)
        .arg(&link)
        .arg(format!("{:x}", Sha256::digest(b"fixture\n")))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!wrapper.join(".build-secrets/gascamp_read_token").exists());
}

#[test]
fn source_path_swap_cannot_mix_validated_and_staged_secret_bytes() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let public = fixture.path().join("public");
    let wrapper = fixture.path().join("wrapper");
    fs::create_dir(&public).unwrap();
    fs::write(public.join("context-manifest.tsv"), "fixture\n").unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o555)).unwrap();
    fs::create_dir(&wrapper).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let source = fixture.path().join("token");
    let original = [vec![b'a'; 8 * 1024 * 1024], vec![b'\n']].concat();
    let replacement = [vec![b'b'; 8 * 1024 * 1024], vec![b'\n']].concat();
    fs::write(&source, &original).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let replacement_path = fixture.path().join("replacement-token");
    fs::write(&replacement_path, &replacement).unwrap();
    fs::set_permissions(&replacement_path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
        .args(["stage-secret"])
        .arg(&wrapper)
        .arg(&source)
        .arg(&public)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    fs::rename(&replacement_path, &source).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let staged = fs::read(wrapper.join(".build-secrets/gascamp_read_token")).unwrap();
    assert!(staged == original || staged == replacement);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("{:x}", Sha256::digest(&staged))
    );
}

#[test]
fn validator_rejects_malformed_mutable_wrong_platform_and_wrong_tag() {
    let validator = env!("CARGO_BIN_EXE_validate-connected-build");
    let digest = "a".repeat(64);
    let valid = format!(
        r#"[{{"id":"sha256:{digest}","configuration":{{"name":"gascan-workspace:locked","descriptor":{{"digest":"sha256:{digest}"}}}},"variants":[{{"platform":{{"os":"linux","architecture":"arm64"}}}}]}}]"#
    );
    let run = |input: &str, tag: &str| {
        let mut child = Command::new(validator)
            .arg(tag)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    assert!(run(&valid, "gascan-workspace:locked").status.success());
    assert!(!run(&valid, "gascan-workspace:latest").status.success());
    for (input, tag) in [
        ("{}".to_owned(), "gascan-workspace:locked"),
        (
            valid.replace(
                "linux\",\"architecture\":\"arm64",
                "linux\",\"architecture\":\"amd64",
            ),
            "gascan-workspace:locked",
        ),
        (valid.clone(), "gascan-workspace:other"),
        (
            valid.replace(&format!("sha256:{digest}"), "gascan-workspace:mutable"),
            "gascan-workspace:locked",
        ),
        (
            valid.replacen(
                &format!(r#""id":"sha256:{digest}""#),
                &format!(r#""id":"sha256:{}""#, "b".repeat(64)),
                1,
            ),
            "gascan-workspace:locked",
        ),
    ] {
        assert!(!run(&input, tag).status.success());
    }
}

#[test]
fn receipt_pair_validator_rejects_every_cross_file_identity_mismatch() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let reference = format!("gascan-workspace:locked@sha256:{}", "a".repeat(64));
    let reference_file = fixture.path().join("reference");
    let json_file = fixture.path().join("receipt.json");
    fs::write(&reference_file, format!("{reference}\n")).unwrap();
    let valid = format!(
        "{{\"reference\":\"{reference}\",\"tag\":\"gascan-workspace:locked\",\"platform\":\"linux/arm64\",\"lock_digest\":\"{}\",\"context_digest\":\"{}\",\"image_digest\":\"sha256:{}\",\"status\":\"succeeded\"}}\n",
        "b".repeat(64),
        "c".repeat(64),
        "a".repeat(64)
    );
    let run = |json: &str| {
        fs::write(&json_file, json).unwrap();
        Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
            .args(["validate-receipt"])
            .arg(&reference_file)
            .arg(&json_file)
            .arg("b".repeat(64))
            .arg("c".repeat(64))
            .status()
            .unwrap()
    };
    assert!(run(&valid).success());
    for invalid in [
        valid.replacen("gascan-workspace:locked@", "gascan-workspace:other@", 1),
        valid.replacen("gascan-workspace:locked\"", "gascan-workspace:other\"", 1),
        valid.replacen(
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "d".repeat(64)),
            1,
        ),
        valid.replacen(&"b".repeat(64), &"d".repeat(64), 1),
        valid.replacen(&"c".repeat(64), &"d".repeat(64), 1),
    ] {
        assert!(!run(&invalid).success());
    }
}

#[test]
fn dispatcher_is_exact_lock_driven_without_auto_fallback() {
    let dispatcher = fs::read_to_string(root().join("scripts/build-workspace-image.sh")).unwrap();
    assert!(dispatcher.contains("workspace_build_mode"));
    assert!(dispatcher.contains("exec \"$root/scripts/build-connected-workspace-image.sh\""));
    assert!(!dispatcher.contains("auto"));
}

#[test]
fn secret_source_rejections_happen_before_container_build() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let fake = fixture.path().join("container");
    fs::write(&fake, "#!/bin/sh\ntouch \"$CALLED\"\nexit 99\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let called = fixture.path().join("called");
    let empty = fixture.path().join("empty");
    fs::write(&empty, "").unwrap();
    fs::set_permissions(&empty, fs::Permissions::from_mode(0o600)).unwrap();
    let readable = fixture.path().join("readable");
    fs::write(&readable, "synthetic\n").unwrap();
    fs::set_permissions(&readable, fs::Permissions::from_mode(0o644)).unwrap();
    let target = fixture.path().join("target");
    fs::write(&target, "synthetic\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = fixture.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let missing = fixture.path().join("missing");
    let repository_file = root().join("scripts/Cargo.toml");
    for rejected in [
        "relative-secret".to_owned(),
        missing.to_string_lossy().into_owned(),
        empty.to_string_lossy().into_owned(),
        readable.to_string_lossy().into_owned(),
        link.to_string_lossy().into_owned(),
        repository_file.to_string_lossy().into_owned(),
    ] {
        let _ = fs::remove_file(&called);
        let output = Command::new("bash")
            .arg(root().join("scripts/build-connected-workspace-image.sh"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    fixture.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("CALLED", &called)
            .env("GASCAMP_READ_TOKEN_FILE", rejected)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!called.exists(), "container invoked for rejected secret");
    }
}
