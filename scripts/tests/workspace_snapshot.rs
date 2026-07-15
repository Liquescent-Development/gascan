use std::{fs, path::Path};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn build_uses_only_a_privileged_verified_snapshot() {
    let build = fs::read_to_string(root().join("scripts/build-workspace-image.sh")).unwrap();
    for required in [
        "snapshot_helper='/usr/local/libexec/gascan/snapshot-workspace-context'",
        "stat -f '%u:%Lp'",
        "sudo -n \"$snapshot_helper\" create \"$context\" \"$context_manifest\"",
        "sudo -n \"$snapshot_helper\" path",
        "sudo -n \"$snapshot_helper\" finish",
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
    assert!(
        build.find("sudo -n \"$snapshot_helper\" create").unwrap()
            < build.find("container image inspect").unwrap()
    );
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
