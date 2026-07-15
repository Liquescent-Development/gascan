use std::{fs, path::Path};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn build_uses_only_a_privileged_verified_snapshot() {
    let build = fs::read_to_string(root().join("scripts/build-workspace-image.sh")).unwrap();
    for required in [
        "snapshot_helper='/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context'",
        "snapshot-helper-identity",
        "sudo -n \"$snapshot_helper\" --self \"$helper_sha256\" \"$helper_device\" \"$helper_inode\" create",
        "\"$helper_inode\" create \"$context\" \"$context_manifest\"",
        "\"$helper_inode\" path",
        "\"$helper_inode\" finish",
        "--file \"$build_context_snapshot/Dockerfile\"",
        "\"$build_context_snapshot\"",
    ] {
        assert!(
            build.contains(required),
            "missing snapshot boundary: {required}"
        );
    }
    assert!(!build.contains("--file \"$context/Dockerfile\""));
    assert!(build.contains("ubuntu_snapshot=$(top_value ubuntu_snapshot)"));
    assert!(build.contains("--build-arg \"UBUNTU_SNAPSHOT=$ubuntu_snapshot\""));
    assert!(build.find("snapshot-helper-identity").unwrap() < build.find("sudo -n").unwrap());
    assert!(
        build.find("\"$helper_inode\" create").unwrap()
            < build.find("container image inspect").unwrap()
    );
}

#[test]
fn install_contract_fixes_helper_and_sudoers_paths() {
    let install = fs::read_to_string(root().join("scripts/install-snapshot-helper.sh")).unwrap();
    assert!(
        install.contains("/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context")
    );
    assert!(install.contains("-o root -g wheel"));
    assert!(install.contains("0555"));
    assert!(install.contains("shasum -a 256"));
    assert!(install.contains("staged helper digest mismatch"));
    assert!(install.contains("staged sudoers digest mismatch"));
    assert!(install.contains("visudo -cf"));
    assert!(install.contains("/etc/sudoers.d/dev.gascan.snapshot-workspace-context"));
    let sudoers =
        fs::read_to_string(root().join("scripts/snapshot-workspace-context.sudoers")).unwrap();
    assert!(
        sudoers.contains("/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context")
    );
    assert!(!sudoers.contains(" secure_path"));
}

#[test]
fn snapshot_helper_is_a_compiled_audited_boundary() {
    let helper = env!("CARGO_BIN_EXE_snapshot-workspace-context");
    let output = std::process::Command::new(helper)
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("create SOURCE"));
}
