use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts has repository parent")
        .to_path_buf()
}

fn executable(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fixture(publication: &str) -> (tempfile::TempDir, Command, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("images/workspace")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("tests/image")).unwrap();
    fs::create_dir_all(root.join(".artifacts/bundles")).unwrap();
    fs::write(
        root.join("images/workspace/versions.lock"),
        format!("base_image = \"ubuntu@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\nworkspace_tag = \"gascan-workspace:test\"\n[workspace_bundles]\npublication = \"{publication}\"\n"),
    )
    .unwrap();
    for bundle in ["ubuntu_packages", "mise_runtimes", "gascamp_source_vendor"] {
        fs::write(
            root.join(format!(".artifacts/bundles/{bundle}.tar.zst")),
            bundle,
        )
        .unwrap();
    }
    fs::write(root.join(".artifacts/mise-linux-arm64"), "mise").unwrap();
    fs::write(
        root.join(".artifacts/playwright-chromium-linux-arm64.zip"),
        "chromium",
    )
    .unwrap();
    fs::write(root.join(".artifacts/expected-tool-versions.json"), "{}\n").unwrap();
    fs::create_dir_all(root.join(".artifacts/workspace-context")).unwrap();
    fs::write(
        root.join(".artifacts/workspace-context/context-manifest.tsv"),
        "context\n",
    )
    .unwrap();
    fs::write(root.join(".artifacts/.base-present"), "").unwrap();
    let calls = temp.path().join("calls");
    for script in ["prefetch-workspace-image.sh", "build-workspace-image.sh"] {
        executable(
            &root.join("scripts").join(script),
            &format!(
                "#!/bin/sh\nset -eu\nprintf '{}\\n' >>\"$CALLS\"\n{}\n",
                script,
                if script.starts_with("build") {
                    "mkdir -p \"$GASCAN_GATE_ARTIFACTS\"; printf 'gascan-workspace:test@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n' | tee \"$GASCAN_GATE_ARTIFACTS/workspace-image-ref\""
                } else {
                    "mkdir -p \"$GASCAN_GATE_ARTIFACTS/bundles\" \"$GASCAN_GATE_ARTIFACTS/workspace-context\"; for b in ubuntu_packages mise_runtimes gascamp_source_vendor; do printf %s \"$b\" >\"$GASCAN_GATE_ARTIFACTS/bundles/$b.tar.zst\"; done; printf mise >\"$GASCAN_GATE_ARTIFACTS/mise-linux-arm64\"; printf chromium >\"$GASCAN_GATE_ARTIFACTS/playwright-chromium-linux-arm64.zip\"; printf '{}\\n' >\"$GASCAN_GATE_ARTIFACTS/expected-tool-versions.json\"; printf 'context\\n' >\"$GASCAN_GATE_ARTIFACTS/workspace-context/context-manifest.tsv\"; touch \"$GASCAN_GATE_ARTIFACTS/.base-present\""
                }
            ),
        );
    }
    executable(
        &root.join("scripts/verify-workspace-image-inputs.sh"),
        "#!/bin/sh\nset -eu\nprintf 'verify-inputs\\n' >>\"$CALLS\"\nexit 1\n",
    );
    for smoke in [
        "user-and-volumes.sh",
        "polyglot-smoke.sh",
        "gascamp-smoke.sh",
    ] {
        executable(
            &root.join("tests/image").join(smoke),
            &format!(
                "#!/bin/sh\nset -eu\nprintf 'smoke:{smoke}:%s:%s\\n' \"$GASCAN_IMAGE_REF_FILE\" \"$GASCAN_TEST_OWNER_TOKEN\" >>\"$CALLS\"\n"
            ),
        );
    }
    let container = temp.path().join("container");
    executable(
        &container,
        r#"#!/bin/sh
set -eu
printf 'container:%s\n' "$*" >>"$CALLS"
if [ "$1 $2" = "image inspect" ]; then
  [ -f "$GASCAN_GATE_ARTIFACTS/.base-present" ] || exit 1
  printf '[{"configuration":{"descriptor":{"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},"variants":[{"platform":{"os":"linux","architecture":"arm64"},"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}]\n'
  exit 0
fi
case "$1" in
  create) touch "$CREATED"; rm -f "$DELETED" ;;
  list) printf '[]\n' ;;
  inspect) [ -f "$CREATED" ] && [ ! -f "$DELETED" ] || exit 1; printf '[{"id":"%s","configuration":{"id":"%s","name":"%s","labels":{"dev.gascan.test":"true","dev.gascan.test.owner":"%s"}}}]\n' "$NAME" "$NAME" "$NAME" "$OWNER" ;;
  delete) touch "$DELETED"; rm -f "$CREATED" ;;
esac
"#,
    );
    let sandbox = temp.path().join("sandbox-exec");
    executable(
        &sandbox,
        "#!/bin/sh\nset -eu\nprintf 'isolator:%s\\n' \"$*\" >>\"$CALLS\"\nshift 2\nexec \"$@\"\n",
    );
    let mut command = Command::new("bash");
    command
        .arg(repository_root().join("scripts/run-offline-image-gate.sh"))
        .env("GASCAN_GATE_TEST_ROOT", &root)
        .env("GASCAN_GATE_ARTIFACTS", root.join(".artifacts"))
        .env("CONTAINER_BIN", container)
        .env("GASCAN_GATE_SANDBOX_BIN", sandbox)
        .env(
            "GASCAN_TEST_OWNER_TOKEN",
            "00112233445566778899aabbccddeeff",
        )
        .env("OWNER", "00112233445566778899aabbccddeeff")
        .env(
            "NAME",
            "gascan-image-gascamp-test-00112233445566778899aabbccddeeff",
        )
        .env("DELETED", temp.path().join("deleted"))
        .env("CREATED", temp.path().join("created"))
        .env("CALLS", &calls);
    (temp, command, calls)
}

#[test]
fn pending_publication_fails_before_prefetch_or_build() {
    let (_temp, mut command, calls) = fixture("pending");
    let output = command.arg("cold").output().unwrap();
    assert!(!output.status.success());
    assert!(!calls.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not published"));
}

#[test]
fn cold_separates_prefetch_then_exact_reference_smoke_handoff() {
    let (temp, mut command, calls) = fixture("published");
    fs::remove_dir_all(temp.path().join("repo/.artifacts")).unwrap();
    let output = command.arg("cold").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(calls).unwrap();
    let prefetch = calls.find("prefetch-workspace-image.sh").unwrap();
    let build = calls.find("build-workspace-image.sh").unwrap();
    assert!(prefetch < build);
    assert_eq!(calls.matches("smoke:").count(), 3);
    assert!(calls.contains("workspace-image-ref:00112233445566778899aabbccddeeff"));
}

#[test]
fn warm_never_prefetches_and_rejects_artifact_mutation() {
    let (_temp, mut command, calls) = fixture("published");
    command.env("GASCAN_GATE_TEST_MUTATE_AFTER_BUILD", "ubuntu_packages");
    let output = command.arg("warm").output().unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(!calls.contains("prefetch"));
    assert!(calls.contains("build"));
    assert!(calls.contains("isolator:run -- env"));
    assert!(!calls.contains("smoke:"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("artifact hashes changed"));
}

#[test]
fn missing_or_malformed_build_evidence_is_nonzero() {
    let (_temp, mut command, _calls) = fixture("published");
    command.env("GASCAN_GATE_TEST_BUILD_EVIDENCE", "missing");
    let output = command.arg("warm").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("image reference"));
}

#[test]
fn every_bundle_corruption_stops_before_apple_build() {
    for bundle in ["ubuntu_packages", "mise_runtimes", "gascamp_source_vendor"] {
        let (_temp, mut command, calls) = fixture("published");
        let output = command.args(["corrupt", bundle]).output().unwrap();
        assert!(!output.status.success(), "{bundle}");
        let calls = fs::read_to_string(calls).unwrap_or_default();
        assert!(calls.contains("verify-inputs"), "{bundle}: {calls}");
        assert!(
            !calls.contains("build-workspace-image.sh"),
            "{bundle}: {calls}"
        );
    }
}

#[test]
fn signal_after_owned_resource_creation_runs_exact_cleanup() {
    let (_temp, mut command, calls) = fixture("published");
    command.env("GASCAN_GATE_TEST_SIGNAL", "TERM");
    let output = command.arg("warm").output().unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains("inspect gascan-image-gascamp-test-00112233445566778899aabbccddeeff"));
    assert!(calls.contains("delete gascan-image-gascamp-test-00112233445566778899aabbccddeeff"));
}
