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
fn connected_evidence_build_bypasses_stale_apple_builder_cache() {
    let script =
        fs::read_to_string(root().join("scripts/build-connected-workspace-image.sh")).unwrap();
    let build = script
        .split("container build")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap();

    assert!(build.contains("--no-cache"), "connected build may reuse stale layers");
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
 *sanitize-build-output*) "$SANITIZER" "${{@: -2:1}}" "${{@: -1}}" ;;
 *prepare-workspace-context*) printf '{manifest}\n' ;;
 *validate-image-inspect*) printf 'sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\n' ;;
 *'validate-connected-build -- validate-receipt '*) "$VALIDATOR" validate-receipt "${{@: -4:1}}" "${{@: -3:1}}" "${{@: -2:1}}" "${{@: -1}}" ;;
 *'validate-connected-build -- gascan-workspace:fixture'*) "$VALIDATOR" "${{@: -1}}" ;;
 *) exit 90 ;;
esac
"#
        ),
    );
    executable(
        &bin.join("sudo"),
        r#"#!/bin/bash
printf 'sudo\t%s\n' "$*" >>"$CALLS"
case " $* " in *' create '*) printf 'receipt\n';; *' path '*) printf '%s\n' "$SNAPSHOT";; *' finish '*) compgen -G "$ARTIFACTS/.connected-build-diagnostic.*" >/dev/null && exit 75; exit 0;; *) exit 91;; esac
"#,
    );
    executable(
        &bin.join("container"),
        r#"#!/bin/bash
    { printf 'container'; printf '\t%s' "$@"; printf '\n'; } >>"$CALLS"
case "$*" in
 'image inspect ubuntu@sha256:'*) printf '[]\n';;
 'image inspect gascan-workspace:fixture') printf '[{"id":"%064d","configuration":{"name":"gascan-workspace:fixture","descriptor":{"digest":"sha256:%064d"}},"variants":[{"platform":{"os":"linux","architecture":"arm64"},"digest":"sha256:%064d"}]}]\n' 9 9 8;;
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
        .env("ARTIFACTS", repo.join(".artifacts"))
        .env("VALIDATOR", env!("CARGO_BIN_EXE_validate-connected-build"))
        .env("SANITIZER", env!("CARGO_BIN_EXE_sanitize-build-output"))
        .env("BENIGN_BUILD_LABEL", "public-build")
        .env("BUILD_PASSWORD_POLICY", "minimum-length-20")
        .env("BUILD_SECRETARY", "release-coordinator")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(calls).unwrap();
    let required = format!(
        "build\t--no-cache\t--arch\tarm64\t--build-arg\tBASE_IMAGE={base}\t--build-arg\tGASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456"
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
    for (name, value) in [
        ("GASCAMP_READ_TOKEN_FILE", "/tmp/token"),
        ("DOCKER_AUTH_CONFIG", "{}"),
        ("GITLAB_TOKEN", "token"),
        ("GITHUB_TOKEN", "token"),
        ("GH_TOKEN", "token"),
        ("AWS_ACCESS_KEY_ID", "key"),
        ("AWS_SECRET_ACCESS_KEY", "secret"),
        ("AWS_SESSION_TOKEN", "session"),
        ("CUSTOM_BUILD_CREDENTIAL", "credential"),
    ] {
        let _ = fs::remove_file(&called);
        let output = Command::new("bash")
            .arg(&script)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    temp.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("CALLED", &called)
            .env(name, value)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("authentication input is forbidden: "),
            "{name} was not rejected at the authentication boundary: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(value));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(value));
        assert!(!called.exists());
    }
}

#[test]
fn validator_rejects_malformed_mutable_wrong_platform_and_wrong_tag() {
    use std::io::Write;
    let digest = "a".repeat(64);
    let valid = format!(
        r#"[{{"id":"{digest}","configuration":{{"name":"gascan-workspace:locked","descriptor":{{"digest":"sha256:{digest}"}}}},"variants":[{{"platform":{{"os":"linux","architecture":"arm64"}},"digest":"sha256:{variant}"}}]}}]"#,
        variant = "b".repeat(64)
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
    assert!(!run(&valid.replace(&format!(r#""id":"{digest}""#), &format!(r#""id":"sha256:{digest}""#)), "gascan-workspace:locked").success());
    assert!(!run(&valid.replace(&format!("sha256:{}", "b".repeat(64)), "sha256:invalid"), "gascan-workspace:locked").success());
    assert!(
        !run(
            &valid.replace("gascan-workspace:locked", "gascan-workspace:"),
            "gascan-workspace:"
        )
        .success()
    );
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
fn every_image_consumer_rejects_each_receipt_identity_mismatch_before_container_use() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let artifacts = temp.path().join("artifacts");
    fs::create_dir_all(artifacts.join("connected-workspace-context")).unwrap();
    fs::write(
        artifacts.join("connected-workspace-context/context-manifest.tsv"),
        "consumer-fixture\n",
    )
    .unwrap();
    let reference_file = artifacts.join("workspace-image-ref");
    let receipt_file = artifacts.join("workspace-image-build.json");
    let tag = "gascan-workspace:consumer";
    let image = format!("sha256:{}", "a".repeat(64));
    let reference = format!("{tag}@{image}");
    fs::write(&reference_file, format!("{reference}\n")).unwrap();
    let lock_digest = format!(
        "{:x}",
        Sha256::digest(fs::read(root().join("images/workspace/versions.lock")).unwrap())
    );
    let context_digest = format!("{:x}", Sha256::digest(b"consumer-fixture\n"));
    let valid = format!(
        r#"{{"reference":"{reference}","tag":"{tag}","platform":"linux/arm64","lock_digest":"{lock_digest}","context_digest":"{context_digest}","image_digest":"{image}","status":"succeeded"}}"#
    );
    let mismatches = [
        valid.replacen(tag, "gascan-workspace:wrong", 1),
        valid.replacen(&image, &format!("sha256:{}", "b".repeat(64)), 1),
        valid.replacen(&context_digest, &"c".repeat(64), 1),
        valid.replacen(&lock_digest, &"d".repeat(64), 1),
    ];
    let container = temp.path().join("container");
    let called = temp.path().join("called");
    executable(&container, "#!/bin/sh\ntouch \"$CALLED\"\nexit 99\n");
    for mismatch in mismatches {
        fs::write(&receipt_file, mismatch).unwrap();
        for consumer in [
            "user-and-volumes.sh",
            "polyglot-smoke.sh",
            "gascamp-smoke.sh",
        ] {
            let _ = fs::remove_file(&called);
            let output = Command::new("bash")
                .arg(root().join("tests/image").join(consumer))
                .env("GASCAN_IMAGE_REF_FILE", &reference_file)
                .env("GASCAN_IMAGE_ARTIFACTS", &artifacts)
                .env("CONTAINER_BIN", &container)
                .env("CALLED", &called)
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "{consumer} accepted mismatched receipt"
            );
            assert!(
                !called.exists(),
                "{consumer} used container before rejecting receipt"
            );
        }
    }
}

#[test]
fn fake_runner_failure_matrix_cleans_snapshot_and_never_commits_an_invalid_pair() {
    for fault in [
        "create_fail",
        "path_fail",
        "public_before",
        "base_invalid",
        "public_after",
        "context_after",
        "build_fail",
        "build_fail_output",
        "build_fail_secret",
        "build_fail_large",
        "scanner_fail",
        "build_signal",
        "inspect_malformed",
        "inspect_mismatch",
        "receipt_invalid",
        "fail_json",
        "fail_ref",
    ] {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let repo = temp.path().join("repo");
        let bin = temp.path().join("bin");
        let context = repo.join(".artifacts/connected-workspace-context");
        let snapshot = temp.path().join("sealed-public-snapshot");
        fs::create_dir_all(repo.join("scripts")).unwrap();
        fs::create_dir_all(repo.join("images/workspace")).unwrap();
        fs::create_dir_all(&context).unwrap();
        fs::create_dir_all(&snapshot).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::copy(
            root().join("scripts/build-connected-workspace-image.sh"),
            repo.join("scripts/build-connected-workspace-image.sh"),
        )
        .unwrap();
        let base = "ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab";
        fs::write(repo.join("images/workspace/versions.lock"), format!("workspace_build_mode = \"connected\"\nbase_image = \"{base}\"\nworkspace_tag = \"gascan-workspace:fixture\"\n[gascamp]\nrevision = \"f6b248c5926240856dbea83d1d2c5c90ea1c1456\"\n")).unwrap();
        for directory in [&context, &snapshot] {
            fs::write(directory.join("Dockerfile"), "FROM scratch\n").unwrap();
            fs::write(directory.join("context-manifest.tsv"), "fixture\n").unwrap();
        }
        let manifest = format!("{:x}", Sha256::digest(b"fixture\n"));
        executable(
            &bin.join("cargo"),
            &format!(
                r#"#!/bin/bash
printf 'cargo\t%s\n' "$*" >>"$CALLS"
case "$*" in
 *snapshot-helper-identity*) printf 'hash\t1\t2\n' ;;
 *sanitize-build-output*) test "$FAULT" != scanner_fail || exit 70; "$SANITIZER" "${{@: -2:1}}" "${{@: -1}}" ;;
 *prepare-workspace-context*) test "$FAULT" != context_after || count=$(($(cat "$COUNT" 2>/dev/null || printf 0)+1)); test "$FAULT" != context_after || printf '%s' "$count" >"$COUNT"; test "$FAULT:$count" != context_after:2 || {{ printf '%064d\n' 7; exit; }}; printf '{manifest}\n' ;;
 *validate-image-inspect*) test "$FAULT" != base_invalid || exit 85; printf 'sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab\n' ;;
 *'validate-connected-build -- validate-receipt '*) test "$FAULT" != receipt_invalid || printf changed >>"${{@: -4:1}}"; "$VALIDATOR" validate-receipt "${{@: -4:1}}" "${{@: -3:1}}" "${{@: -2:1}}" "${{@: -1}}" ;;
 *'validate-connected-build -- gascan-workspace:fixture'*) "$VALIDATOR" "${{@: -1}}" ;;
 *) exit 90 ;;
esac
"#
            ),
        );
        executable(
            &bin.join("sudo"),
            r#"#!/bin/bash
printf 'sudo\t%s\n' "$*" >>"$CALLS"
case " $* " in *' create '*) test "$FAULT" != create_fail || exit 83; printf 'receipt\n';; *' path '*) test "$FAULT" != path_fail || exit 84; test "$FAULT" != public_before || printf changed >>"$SNAPSHOT/context-manifest.tsv"; printf '%s\n' "$SNAPSHOT";; *' finish '*) compgen -G "$ARTIFACTS/.connected-build-diagnostic.*" >/dev/null && exit 75; exit 0;; *) exit 91;; esac
"#,
        );
        executable(
            &bin.join("container"),
            &format!(
                r#"#!/bin/bash
{{ printf 'container'; printf '\t%s' "$@"; printf '\n'; }} >>"$CALLS"
case "$*" in
 'image inspect ubuntu@sha256:'*) printf '[]\n';;
 'image inspect gascan-workspace:fixture')
   test "$FAULT" != inspect_malformed || {{ printf '{{}}\n'; exit; }}
   digest={}; test "$FAULT" != inspect_mismatch || digest={}
   printf '[{{"id":"%s","configuration":{{"name":"gascan-workspace:fixture","descriptor":{{"digest":"sha256:%s"}}}},"variants":[{{"platform":{{"os":"linux","architecture":"arm64"}},"digest":"sha256:{}"}}]}}]\n' "$digest" "{}";;
 build*)
   case "$FAULT" in
     build_fail) exit 81;;
     build_fail_output) printf 'mise resolution mismatch: safe diagnostic\n' >&2; exit 81;;
     build_fail_secret) i=0; while test "$i" -lt 10000; do printf 'safe-prefix-%05d\n' "$i" >&2; i=$((i+1)); done; printf 'Authorization: Bearer should-never-escape\n' >&2; exit 82;;
     build_fail_large) i=0; while test "$i" -lt 20000; do printf 'bounded-safe-diagnostic-%05d\n' "$i" >&2; i=$((i+1)); done; exit 83;;
     build_signal) kill -TERM "$PPID"; sleep 1; exit 84;;
   esac
   test "$FAULT" != public_after || printf changed >>"$SNAPSHOT/context-manifest.tsv";;
 *) exit 92;;
esac
"#,
                "9".repeat(64),
                "8".repeat(64),
                "7".repeat(64),
                "9".repeat(64)
            ),
        );
        executable(&bin.join("sw_vers"), "#!/bin/sh\nprintf '14.0\n'\n");
        executable(
            &bin.join("mv"),
            r#"#!/bin/bash
{ printf 'mv'; printf '\t%s' "$@"; printf '\n'; } >>"$CALLS"
destination=${@: -1}; case "$FAULT:$destination" in fail_json:*/workspace-image-build.json) exit 81;; fail_ref:*/workspace-image-ref) exit 82;; esac; exec /bin/mv "$@"
"#,
        );
        let calls = temp.path().join("calls");
        let count = temp.path().join("count");
        let validator = env!("CARGO_BIN_EXE_validate-connected-build");
        let lock_digest = format!(
            "{:x}",
            Sha256::digest(fs::read(repo.join("images/workspace/versions.lock")).unwrap())
        );
        if fault == "fail_ref" {
            let old_image = format!("sha256:{}", "a".repeat(64));
            let old_reference = format!("gascan-workspace:fixture@{old_image}");
            fs::write(
                repo.join(".artifacts/workspace-image-ref"),
                format!("{old_reference}\n"),
            )
            .unwrap();
            fs::write(repo.join(".artifacts/workspace-image-build.json"), format!(r#"{{"reference":"{old_reference}","tag":"gascan-workspace:fixture","platform":"linux/arm64","lock_digest":"{lock_digest}","context_digest":"{manifest}","image_digest":"{old_image}","status":"succeeded"}}"#)).unwrap();
            assert!(
                Command::new(validator)
                    .arg("validate-receipt")
                    .arg(repo.join(".artifacts/workspace-image-ref"))
                    .arg(repo.join(".artifacts/workspace-image-build.json"))
                    .arg(&lock_digest)
                    .arg(&manifest)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let output = Command::new("bash")
            .arg(repo.join("scripts/build-connected-workspace-image.sh"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("CALLS", &calls)
            .env("COUNT", &count)
            .env("FAULT", fault)
            .env("SNAPSHOT", &snapshot)
            .env("ARTIFACTS", repo.join(".artifacts"))
            .env("VALIDATOR", validator)
            .env("SANITIZER", env!("CARGO_BIN_EXE_sanitize-build-output"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{fault} unexpectedly succeeded");
        match fault {
            "build_fail_output" => {
                assert_eq!(output.status.code(), Some(81));
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("mise resolution mismatch: safe diagnostic"));
            }
            "build_fail_secret" => {
                assert_eq!(output.status.code(), Some(1));
                assert!(!String::from_utf8_lossy(&output.stderr).contains("should-never-escape"));
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("diagnostic rejected or sanitizer failed"));
            }
            "build_fail_large" => {
                assert_eq!(output.status.code(), Some(83));
                assert!(output.stderr.len() <= 140_000, "diagnostic was not bounded");
                assert!(String::from_utf8_lossy(&output.stderr).contains("diagnostic truncated"));
            }
            "scanner_fail" => {
                assert_eq!(output.status.code(), Some(1));
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("diagnostic rejected or sanitizer failed"));
            }
            "build_signal" => assert_eq!(output.status.code(), Some(143)),
            _ => {}
        }
        let log = fs::read_to_string(&calls).unwrap();
        let expected_finish = usize::from(fault != "create_fail");
        assert_eq!(
            log.matches(" finish ").count(),
            expected_finish,
            "{fault} cleanup count differs: {log}"
        );
        if fault == "fail_ref" {
            let retained_reference =
                fs::read_to_string(repo.join(".artifacts/workspace-image-ref")).unwrap();
            let retained_receipt =
                fs::read_to_string(repo.join(".artifacts/workspace-image-build.json")).unwrap();
            assert!(
                log.lines().any(|line| {
                    line.starts_with("mv\t-f\t") && line.ends_with("/workspace-image-ref")
                }),
                "fail_ref did not reach reference publication: {log}"
            );
            assert!(retained_reference.contains(&"a".repeat(64)));
            assert!(retained_receipt.contains(&"9".repeat(64)));
            assert!(
                !Command::new(validator)
                    .arg("validate-receipt")
                    .arg(repo.join(".artifacts/workspace-image-ref"))
                    .arg(repo.join(".artifacts/workspace-image-build.json"))
                    .arg(&lock_digest)
                    .arg(&manifest)
                    .status()
                    .unwrap()
                    .success(),
                "interrupted old-reference/new-JSON pair was accepted"
            );
        } else {
            assert!(
                !repo.join(".artifacts/workspace-image-ref").exists(),
                "{fault} published reference"
            );
        }
        let retained = fs::read_dir(repo.join(".artifacts"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!retained.contains("connected-build-diagnostic"));
        for material in ["Authorization", "Bearer ", "--secret", ".build-secrets"] {
            assert!(!String::from_utf8_lossy(&output.stdout).contains(material));
            assert!(!String::from_utf8_lossy(&output.stderr).contains(material));
            assert!(!log.contains(material));
            assert!(!retained.contains(material));
        }
    }
}

#[test]
fn dispatcher_is_exact_lock_driven_without_auto_fallback() {
    let dispatcher = fs::read_to_string(root().join("scripts/build-workspace-image.sh")).unwrap();
    assert!(dispatcher.contains("workspace_build_mode"));
    assert!(dispatcher.contains("build-connected-workspace-image.sh"));
    assert!(dispatcher.contains("build-offline-workspace-image.sh"));
    assert!(!dispatcher.contains("auto"));
    assert_ne!(
        fs::metadata(root().join("scripts/build-connected-workspace-image.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0,
        "connected dispatcher target is not executable"
    );
}

#[test]
fn normal_executable_dispatch_reaches_the_connected_entrypoint() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    fs::create_dir_all(temp.path().join("scripts")).unwrap();
    fs::create_dir_all(temp.path().join("images/workspace")).unwrap();
    fs::copy(
        root().join("scripts/build-workspace-image.sh"),
        temp.path().join("scripts/build-workspace-image.sh"),
    )
    .unwrap();
    fs::copy(
        root().join("scripts/build-connected-workspace-image.sh"),
        temp.path()
            .join("scripts/build-connected-workspace-image.sh"),
    )
    .unwrap();
    fs::write(
        temp.path().join("images/workspace/versions.lock"),
        "workspace_build_mode = \"connected\"\n",
    )
    .unwrap();
    fs::write(
        temp.path()
            .join("scripts/build-connected-workspace-image.sh"),
        "#!/bin/sh\nprintf reached >\"$MARKER\"\n",
    )
    .unwrap();
    let marker = temp.path().join("marker");
    let status = Command::new(temp.path().join("scripts/build-workspace-image.sh"))
        .env("MARKER", &marker)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(marker).unwrap(), "reached");
}
