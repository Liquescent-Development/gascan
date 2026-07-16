use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn connected_build_uses_the_sealed_public_snapshot_without_authentication_material() {
    let script =
        fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh")).unwrap();
    for forbidden in [
        ".build-secrets",
        "stage-secret",
        "prepare-wrapper",
        "verify-wrapper",
    ] {
        assert!(
            !script.contains(forbidden),
            "retained forbidden authentication path: {forbidden}"
        );
    }
    assert!(script.contains("create \"$context\" \"$context_manifest\""));
    assert!(script.contains("path \"$snapshot_receipt\""));
    assert!(script.contains("finish \"$snapshot_receipt\""));
    assert!(script.contains("--file \"$snapshot/Dockerfile\" \"$snapshot\""));
    let build = script
        .split("container build")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap();
    assert!(!build.contains("--secret"));
}

#[test]
fn fake_runner_builds_the_exact_public_snapshot_and_publishes_reference_last() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let repo = temp.path().join("repo");
    let bin = temp.path().join("bin");
    let context = repo.join(".artifacts/connected-workspace-context");
    fs::create_dir_all(repo.join("scripts")).unwrap();
    fs::create_dir_all(repo.join("images/workspace")).unwrap();
    fs::create_dir_all(&context).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::copy(
        root().join("scripts/build-connected-workspace-image.sh"),
        repo.join("scripts/build-connected-workspace-image.sh"),
    )
    .unwrap();
    let base = "ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab";
    fs::write(repo.join("images/workspace/versions.lock"), format!("workspace_build_mode = \"connected\"\nbase_image = \"{base}\"\nworkspace_tag = \"gascan-workspace:fixture\"\n[gascamp]\nrevision = \"f6b248c5926240856dbea83d1d2c5c90ea1c1456\"\n")).unwrap();
    fs::write(context.join("Dockerfile"), "FROM scratch\n").unwrap();
    fs::write(context.join("context-manifest.tsv"), "fixture\n").unwrap();
    let manifest = format!("{:x}", Sha256::digest(b"fixture\n"));
    executable(
        &bin.join("cargo"),
        &format!(
            r#"#!/bin/bash
printf 'cargo\t%s\n' "$*" >>"$CALLS"
case "$*" in
 *snapshot-helper-identity*) printf 'hash\t1\t2\n' ;;
 *prepare-workspace-context*) printf '{manifest}\n' ;;
 *validate-image-inspect*) printf 'sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\n' ;;
 *'validate-connected-build -- gascan-workspace:fixture'*) printf 'sha256:%064d\n' 9 ;;
 *'validate-connected-build -- validate-receipt'*) exit 0 ;;
 *) exit 90 ;;
esac
"#
        ),
    );
    executable(
        &bin.join("sudo"),
        r#"#!/bin/bash
printf 'sudo\t%s\n' "$*" >>"$CALLS"
case " $* " in *' create '*) printf 'receipt\n';; *' path '*) printf '%s\n' "$SNAPSHOT";; *' finish '*) exit 0;; *) exit 91;; esac
"#,
    );
    executable(
        &bin.join("container"),
        r#"#!/bin/bash
    { printf 'container'; printf '\t%s' "$@"; printf '\n'; } >>"$CALLS"
case "$*" in
 'image inspect --format json ubuntu@sha256:'*) printf '[]\n';;
 'image inspect --format json gascan-workspace:fixture') printf '[]\n';;
 build*) exit 0;;
 *) exit 92;;
esac
"#,
    );
    executable(&bin.join("sw_vers"), "#!/bin/sh\nprintf '14.0\n'\n");
    let calls = temp.path().join("calls");
    let output = Command::new("bash")
        .arg(repo.join("scripts/build-connected-workspace-image.sh"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("CALLS", &calls)
        .env("SNAPSHOT", &context)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(calls).unwrap();
    let required = format!(
        "build\t--arch\tarm64\t--build-arg\tBASE_IMAGE={base}\t--build-arg\tGASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456"
    );
    assert!(log.contains(&required), "{log}");
    assert!(!log.contains("--secret"));
    assert_eq!(log.matches(" create ").count(), 1);
    assert_eq!(log.matches(" path ").count(), 1);
    assert_eq!(log.matches(" finish ").count(), 1);
    let reference = fs::read_to_string(repo.join(".artifacts/workspace-image-ref")).unwrap();
    assert_eq!(
        reference,
        format!("gascan-workspace:fixture@sha256:{}\n", "0".repeat(63) + "9")
    );
    assert!(repo.join(".artifacts/workspace-image-build.json").exists());
}

#[test]
fn authentication_inputs_fail_before_container_use() {
    let script = root().join("scripts/build-connected-workspace-image.sh");
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let container = temp.path().join("container");
    executable(&container, "#!/bin/sh\ntouch \"$CALLED\"\nexit 99\n");
    let called = temp.path().join("called");
    let output = Command::new("bash")
        .arg(script)
        .env(
            "PATH",
            format!(
                "{}:{}",
                temp.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("CALLED", &called)
        .env("GASCAMP_READ_TOKEN_FILE", "/tmp/token")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!called.exists());
}

#[test]
fn validator_rejects_malformed_mutable_wrong_platform_and_wrong_tag() {
    use std::io::Write;
    let digest = "a".repeat(64);
    let valid = format!(
        r#"[{{"id":"sha256:{digest}","configuration":{{"name":"gascan-workspace:locked","descriptor":{{"digest":"sha256:{digest}"}}}},"variants":[{{"platform":{{"os":"linux","architecture":"arm64"}}}}]}}]"#
    );
    let run = |input: &str, tag: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
            .arg(tag)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait().unwrap()
    };
    assert!(run(&valid, "gascan-workspace:locked").success());
    assert!(!run("{}", "gascan-workspace:locked").success());
    assert!(!run(&valid, "gascan-workspace:latest").success());
    assert!(!run(&valid.replace("arm64", "amd64"), "gascan-workspace:locked").success());
    assert!(!run(&valid, "gascan-workspace:other").success());
}

#[test]
fn receipt_pair_validator_rejects_cross_file_identity_mismatch() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let reference = temp.path().join("ref");
    let json = temp.path().join("receipt");
    let image = format!("sha256:{}", "a".repeat(64));
    let exact = format!("gascan-workspace:locked@{image}");
    fs::write(&reference, format!("{exact}\n")).unwrap();
    let valid = format!(
        r#"{{"reference":"{exact}","tag":"gascan-workspace:locked","platform":"linux/arm64","lock_digest":"{}","context_digest":"{}","image_digest":"{image}","status":"succeeded"}}"#,
        "b".repeat(64),
        "c".repeat(64)
    );
    let run = |body: &str| {
        fs::write(&json, body).unwrap();
        Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
            .args(["validate-receipt"])
            .arg(&reference)
            .arg(&json)
            .arg("b".repeat(64))
            .arg("c".repeat(64))
            .status()
            .unwrap()
    };
    assert!(run(&valid).success());
    assert!(!run(&valid.replace(&"c".repeat(64), &"d".repeat(64))).success());
}

#[test]
fn dispatcher_is_exact_lock_driven_without_auto_fallback() {
    let dispatcher = fs::read_to_string(root().join("scripts/build-workspace-image.sh")).unwrap();
    assert!(dispatcher.contains("workspace_build_mode"));
    assert!(dispatcher.contains("build-connected-workspace-image.sh"));
    assert!(dispatcher.contains("build-offline-workspace-image.sh"));
    assert!(!dispatcher.contains("auto"));
}
