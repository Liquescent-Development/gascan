use std::{fs, path::Path};

fn dockerfile() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::read_to_string(root.join("images/workspace/Dockerfile")).unwrap()
}

#[test]
fn dockerfile_has_no_network_capable_build_steps() {
    let dockerfile = dockerfile();
    for forbidden in [
        "http://",
        "https://",
        "apt-get update",
        "git fetch",
        "git clone",
        "mise install",
        "cargo fetch",
        "curl ",
        "wget ",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "network-capable Dockerfile token: {forbidden}"
        );
    }
}

#[test]
fn dockerfile_assembles_only_verified_local_inputs() {
    let dockerfile = dockerfile();
    for required in [
        "bundles/ubuntu_packages/repository",
        "bundles/ubuntu_packages/package-manifest.tsv",
        "Dir::Etc::sourcelist=/dev/null",
        "Dir::Etc::sourceparts=-",
        "Dir::Bin::methods=/nonexistent",
        "Acquire::Retries=0",
        "test -s /tmp/gascan-packages.list",
        "dpkg-query --show --showformat='${Version}\\t${Architecture}'",
        "comm -13 /tmp/gascan-packages.before /tmp/gascan-packages.after",
        "comm -23 - /tmp/gascan-packages.locked",
        "bundles/mise_runtimes/mise-runtimes-linux-arm64.tar.zst",
        "bundles/mise_runtimes/mise-runtimes-linux-arm64.manifest.tsv",
        "bundles/mise_runtimes/mise-current.json",
        "tar --zstd --extract",
        "tar --zstd --compare",
        "cmp /tmp/mise-archive-paths /tmp/mise-manifest-paths",
        "bundles/gascamp_source_vendor/tree/source/",
        "bundles/gascamp_source_vendor/tree/vendor/",
        "bundles/gascamp_source_vendor/tree/.cargo/config.toml",
        "/opt/gascan/mise/installs/rust/1.97.0/bin/cargo test --locked --offline --frozen",
        "/opt/gascan/mise/installs/rust/1.97.0/bin/cargo build --locked --offline --frozen --release --bin camp",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing offline assembly contract: {required}"
        );
    }
}
