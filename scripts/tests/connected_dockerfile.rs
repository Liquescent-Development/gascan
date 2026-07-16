use std::{collections::BTreeSet, fs, path::Path, process::Command};

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
        "cmp --silent /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json",
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
fn dockerfile_installs_pinned_erlang_before_elixir_and_validates_otp_29() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let erlang = dockerfile.find("mise install --yes erlang@29.0.3").unwrap();
    let remaining = dockerfile.find("mise install --yes \\").unwrap();
    assert!(erlang < remaining);
    assert!(dockerfile.contains("erl -noshell -eval"));
    assert!(dockerfile.contains("otp_release"));
    assert!(dockerfile.contains("=:= <<\"29\">>"));
}

#[test]
fn dockerfile_prints_safe_mise_version_metadata_only_when_the_lock_comparison_fails() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(dockerfile.contains("if ! cmp --silent"));
    assert!(!dockerfile.contains("mise version metadata mismatch"));
    assert!(!dockerfile.contains("actual resolved versions:"));
    assert!(!dockerfile.contains("expected resolved versions:"));
}

#[test]
fn mise_comparison_is_quiet_on_match_and_emits_only_both_json_documents_on_mismatch() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let block = dockerfile
        .split("if ! cmp --silent")
        .nth(1)
        .unwrap()
        .split("       fi \\")
        .next()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let actual = temp.path().join("actual.json");
    let expected = temp.path().join("expected.json");
    let script = format!(
        "if ! cmp --silent{} fi",
        block
            .replace("/tmp/resolved-tool-versions.json", actual.to_str().unwrap())
            .replace(
                "/tmp/expected-tool-versions.json",
                expected.to_str().unwrap()
            )
            .replace("\\\n", "\n")
    );
    fs::write(&actual, "{\"node\":\"20\"}\n").unwrap();
    fs::write(&expected, "{\"node\":\"20\"}\n").unwrap();
    let equal = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(equal.status.success());
    assert!(equal.stdout.is_empty());
    assert!(equal.stderr.is_empty());
    fs::write(&expected, "{\"node\":\"22\"}\n").unwrap();
    let mismatch = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(!mismatch.status.success());
    assert_eq!(mismatch.stdout, b"{\"node\":\"20\"}\n{\"node\":\"22\"}\n");
    assert!(mismatch.stderr.is_empty());
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
