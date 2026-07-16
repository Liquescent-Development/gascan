use std::{fs, path::Path, process::Command};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn dockerfile_declares_workspace_user_init_and_persistent_layout() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let system_tools = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    for required in ["sudo", "tini"] {
        assert!(
            system_tools.lines().any(|package| package == required),
            "missing image package: {required}"
        );
    }
    for required in [
        "COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace",
        "COPY --chmod=0555 images/workspace/bin/migrate-workspace-identity /usr/local/bin/migrate-workspace-identity",
        "/usr/local/bin/migrate-workspace-identity",
        "chown workspace:workspace /opt/gascan/mise",
        "/opt/gascan/mise",
        "/home/workspace/.cache",
        "/home/workspace/.config/gascan",
        "visudo -cf /etc/sudoers.d/workspace",
        "USER workspace:workspace",
        "WORKDIR /workspace",
        "ENTRYPOINT [\"/usr/bin/tini\", \"--\", \"/usr/local/bin/gascan-entrypoint\"]",
        "VOLUME [\"/opt/gascan/mise\", \"/home/workspace/.cache\", \"/home/workspace/.config/gascan\"]",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing image contract: {required}"
        );
    }
}

#[test]
fn identity_migration_is_exact_and_fail_closed() {
    let migration =
        fs::read_to_string(root().join("images/workspace/bin/migrate-workspace-identity")).unwrap();

    for required in [
        "ubuntu:x:1000:1000:Ubuntu:/home/ubuntu:/bin/bash",
        "ubuntu:x:1000:",
        "usermod --login workspace --home /home/workspace --move-home ubuntu",
        "groupmod --new-name workspace ubuntu",
        "workspace:x:1000:1000:Ubuntu:/home/workspace:/bin/bash",
        "workspace:x:1000:",
        "test ! -e /home/ubuntu",
    ] {
        assert!(
            migration.contains(required),
            "missing exact identity contract: {required}"
        );
    }
    for forbidden in ["--non-unique", "userdel", "groupdel", "useradd", "groupadd"] {
        assert!(
            !migration.contains(forbidden),
            "unsafe identity migration: {forbidden}"
        );
    }
}

#[test]
fn sudoers_and_entrypoint_are_exact_and_non_bootstrapping() {
    let sudoers = root().join("images/workspace/etc/sudoers.d/workspace");
    assert_eq!(
        fs::read_to_string(&sudoers).unwrap(),
        "workspace ALL=(ALL:ALL) NOPASSWD: ALL\n"
    );

    let entrypoint =
        fs::read_to_string(root().join("images/workspace/bin/gascan-entrypoint")).unwrap();
    assert!(entrypoint.contains("exec \"$@\""));
    assert!(entrypoint.contains("exec sleep infinity"));
    for forbidden in [
        "curl",
        "wget",
        "http://",
        "https://",
        "mise install",
        "git clone",
    ] {
        assert!(
            !entrypoint.contains(forbidden),
            "entrypoint contains bootstrap behavior: {forbidden}"
        );
    }
}

#[test]
fn smoke_fixture_uses_built_ref_and_checks_signal_and_zombies() {
    let smoke = fs::read_to_string(root().join("tests/image/user-and-volumes.sh")).unwrap();
    for required in [
        ".artifacts/workspace-image-ref",
        "\"$container_bin\" create",
        "--label dev.gascan.test=true",
        "dev.gascan.test.owner=$owner_token",
        "--mount \"type=bind,source=$root,target=/workspace\"",
        "--bin validate-owned-container",
        "\"$container_bin\" start",
        "\"$container_bin\" exec",
        "/proc/[0-9]*/status",
        "bounded_container stop --time 5",
        "test \"$elapsed\" -le 5",
    ] {
        assert!(
            smoke.contains(required),
            "missing live smoke contract: {required}"
        );
    }
    assert_eq!(smoke.matches("--mount ").count(), 1);
    assert!(!smoke.contains("container run"));
}

#[test]
fn gascamp_smoke_fails_closed_without_a_built_image_reference() {
    let missing = root().join(".artifacts/definitely-missing-gascamp-image-ref");
    let output = Command::new("bash")
        .arg(root().join("tests/image/gascamp-smoke.sh"))
        .env("GASCAN_IMAGE_REF_FILE", &missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("missing Gascamp image reference: {}\n", missing.display())
    );
}
