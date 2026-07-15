use std::{collections::BTreeSet, fs, path::Path};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn assert_sole_reviewed_package_install(dockerfile: &str) -> Result<(), &'static str> {
    if dockerfile.lines().any(|line| {
        line.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|tokens| tokens == ["apt", "install"])
    }) {
        return Err("direct apt install bypasses the reviewed package file");
    }
    let apt_get_lines: Vec<_> = dockerfile
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("apt-get"))
        .collect();
    if apt_get_lines
        != [
            "&& apt-get -o Acquire::Retries=0 update \\",
            "&& DEBIAN_FRONTEND=noninteractive xargs apt-get \\",
            "&& apt-get clean \\",
        ]
    {
        return Err("apt-get must only update, install from the reviewed file, and clean");
    }
    if !dockerfile.contains(
        "&& DEBIAN_FRONTEND=noninteractive xargs apt-get \\\n         -o Acquire::Retries=0 install --yes --no-install-recommends </tmp/system-tools.txt \\",
    ) {
        return Err("package install must consume only the reviewed file");
    }
    Ok(())
}

#[test]
fn dockerfile_assembles_the_connected_workspace_base() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "FROM ${BASE_IMAGE} AS workspace-base",
        "apt-get -o Acquire::Retries=0 update",
        "install --yes --no-install-recommends",
        "rm -rf /var/lib/apt/lists/*",
        "COPY --chmod=0555 .artifacts/mise-linux-arm64 /usr/local/bin/mise",
        "mise install --yes",
        "mise current --json",
        "cmp /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing connected contract: {required}"
        );
    }
    for forbidden in [
        "bundles/ubuntu_packages",
        "bundles/mise_runtimes",
        "Dir::Bin::methods=/nonexistent",
        "apt-get upgrade",
        "latest",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "deferred/unlocked path: {forbidden}"
        );
    }
}

#[test]
fn dockerfile_installs_exactly_the_sorted_unique_reviewed_package_list() {
    let package_text = fs::read_to_string(root().join("tests/image/system-tools.txt")).unwrap();
    assert!(package_text.ends_with('\n'));
    assert!(package_text.lines().all(|line| !line.is_empty()));
    let packages: Vec<_> = package_text.lines().collect();
    let sorted_unique: BTreeSet<_> = packages.iter().copied().collect();
    assert_eq!(packages, sorted_unique.into_iter().collect::<Vec<_>>());

    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "COPY --chmod=0444 tests/image/system-tools.txt /tmp/system-tools.txt",
        "xargs apt-get \\",
        "--no-install-recommends </tmp/system-tools.txt",
        "done </tmp/system-tools.txt",
        "rm -rf /var/lib/apt/lists/* /tmp/system-tools.txt",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing package contract: {required}"
        );
    }
    assert_sole_reviewed_package_install(&dockerfile).unwrap();
}

#[test]
fn package_contract_rejects_an_inline_unreviewed_install() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let mutated = format!("{dockerfile}\nRUN apt-get install arbitrary-package\n");
    assert!(assert_sole_reviewed_package_install(&mutated).is_err());
}

#[test]
fn package_contract_rejects_an_inline_unreviewed_apt_install() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let mutated = format!("{dockerfile}\nRUN apt install arbitrary-package\n");
    assert!(assert_sole_reviewed_package_install(&mutated).is_err());
}
