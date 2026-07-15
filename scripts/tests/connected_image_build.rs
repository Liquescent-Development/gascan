use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
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
        "prepare-wrapper",
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
    fs::write(
        wrapper.join(".build-secrets/gascamp_read_token"),
        "changed\n",
    )
    .unwrap();
    let verify = Command::new(env!("CARGO_BIN_EXE_validate-connected-build"))
        .args(["verify-wrapper"])
        .arg(&wrapper)
        .arg(&digest)
        .arg(identity.trim())
        .status()
        .unwrap();
    assert!(!verify.success());
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
    ] {
        assert!(!run(&input, tag).status.success());
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
