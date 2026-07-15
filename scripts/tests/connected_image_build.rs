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
        "--secret \"id=gascamp_read_token,src=/private/context/.build-secrets/gascamp_read_token\"",
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
