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
case " $* " in *' create '*) printf 'receipt\n';; *' path '*) printf '%s\n' "$SNAPSHOT";; *' finish '*) exit 0;; *) exit 91;; esac
"#,
    );
    executable(
        &bin.join("container"),
        r#"#!/bin/bash
    { printf 'container'; printf '\t%s' "$@"; printf '\n'; } >>"$CALLS"
case "$*" in
 'image inspect --format json ubuntu@sha256:'*) printf '[]\n';;
 'image inspect --format json gascan-workspace:fixture') printf '[{"id":"sha256:%064d","configuration":{"name":"gascan-workspace:fixture","descriptor":{"digest":"sha256:%064d"}},"variants":[{"platform":{"os":"linux","architecture":"arm64"}}]}]\n' 9 9;;
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
        .env("VALIDATOR", env!("CARGO_BIN_EXE_validate-connected-build"))
        .env("BENIGN_BUILD_LABEL", "public-build")
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
    for (name, value) in [
        ("GASCAMP_READ_TOKEN_FILE", "/tmp/token"),
        ("DOCKER_AUTH_CONFIG", "{}"),
        ("GITLAB_TOKEN", "token"),
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
fn fake_runner_failure_matrix_cleans_snapshot_and_never_commits_an_invalid_pair() {
    for fault in [
        "create_fail",
        "path_fail",
        "public_before",
        "base_invalid",
        "public_after",
        "context_after",
        "build_fail",
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
case " $* " in *' create '*) test "$FAULT" != create_fail || exit 83; printf 'receipt\n';; *' path '*) test "$FAULT" != path_fail || exit 84; test "$FAULT" != public_before || printf changed >>"$SNAPSHOT/context-manifest.tsv"; printf '%s\n' "$SNAPSHOT";; *' finish '*) exit 0;; *) exit 91;; esac
"#,
        );
        executable(
            &bin.join("container"),
            &format!(
                r#"#!/bin/bash
{{ printf 'container'; printf '\t%s' "$@"; printf '\n'; }} >>"$CALLS"
case "$*" in
 'image inspect --format json ubuntu@sha256:'*) printf '[]\n';;
 'image inspect --format json gascan-workspace:fixture')
   test "$FAULT" != inspect_malformed || {{ printf '{{}}\n'; exit; }}
   digest={}; test "$FAULT" != inspect_mismatch || digest={}
   printf '[{{"id":"sha256:%s","configuration":{{"name":"gascan-workspace:fixture","descriptor":{{"digest":"sha256:%s"}}}},"variants":[{{"platform":{{"os":"linux","architecture":"arm64"}}}}]}}]\n' "$digest" "{}";;
 build*) test "$FAULT" != build_fail || exit 81; test "$FAULT" != public_after || printf changed >>"$SNAPSHOT/context-manifest.tsv";;
 *) exit 92;;
esac
"#,
                "9".repeat(64),
                "8".repeat(64),
                "9".repeat(64)
            ),
        );
        executable(&bin.join("sw_vers"), "#!/bin/sh\nprintf '14.0\n'\n");
        executable(
            &bin.join("mv"),
            r#"#!/bin/bash
destination=${@: -1}; case "$FAULT:$destination" in fail_json:*/workspace-image-build.json) exit 81;; fail_ref:*/workspace-image-ref) exit 82;; esac; exec /bin/mv "$@"
"#,
        );
        let calls = temp.path().join("calls");
        let count = temp.path().join("count");
        let validator = env!("CARGO_BIN_EXE_validate-connected-build");
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
            .env("VALIDATOR", validator)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{fault} unexpectedly succeeded");
        let log = fs::read_to_string(&calls).unwrap();
        let expected_finish = usize::from(fault != "create_fail");
        assert_eq!(
            log.matches(" finish ").count(),
            expected_finish,
            "{fault} cleanup count differs: {log}"
        );
        assert!(
            !repo.join(".artifacts/workspace-image-ref").exists(),
            "{fault} published reference"
        );
        let retained = fs::read_dir(repo.join(".artifacts"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
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
