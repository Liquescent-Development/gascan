use std::{fs, path::Path};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn dockerfile_declares_workspace_user_init_and_persistent_layout() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "sudo tini",
        "COPY --chmod=0440 images/workspace/etc/sudoers.d/workspace /etc/sudoers.d/workspace",
        "groupadd --gid 1000 workspace",
        "useradd --uid 1000 --gid 1000",
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
        "\"$container_bin\" stop --time 5",
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
