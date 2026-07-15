use std::{collections::BTreeMap, fs, path::Path, process::Command};

use serde::Deserialize;

#[derive(Deserialize)]
struct Lock {
    tools: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct MiseConfig {
    tools: BTreeMap<String, String>,
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn mise_defaults_exactly_match_locked_polyglot_versions() {
    let lock: Lock =
        toml::from_str(&fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap())
            .unwrap();
    let config_text =
        fs::read_to_string(root().join("images/workspace/etc/mise/config.toml")).unwrap();
    let config: MiseConfig = toml::from_str(&config_text).unwrap();
    assert_eq!(config.tools, lock.tools);
    for forbidden in ["[env]", "hooks", "task", "latest", "stable", "lts"] {
        assert!(!config_text.contains(forbidden));
    }
}

#[test]
fn dockerfile_installs_only_reviewed_system_tools_and_verified_artifacts() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    for required in [
        "bundles/ubuntu_packages/repository",
        "install --yes --no-install-recommends",
        ".artifacts/mise-linux-arm64",
        "/usr/local/bin/mise",
        "images/workspace/etc/mise/config.toml",
        "images/workspace/etc/profile.d/mise.sh",
        "bundles/mise_runtimes/mise-runtimes-linux-arm64.tar.zst",
        "mise current --json",
        "/opt/gascan/image-tool-versions.json",
        ".artifacts/playwright-chromium-reviewed/chrome-linux",
        "/opt/gascan/tests/playwright-smoke.mjs",
        "/tmp/resolved-tool-versions.json",
        "USER root",
        "cmp /tmp/resolved-tool-versions.json /tmp/bundle-tool-versions.json",
        "install -o root -g root -m 0444",
        "rm -rf /var/lib/apt/lists/*",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing image contract: {required}"
        );
    }
    for forbidden in [
        "curl ",
        "wget ",
        "mise use",
        "npm install",
        "apt-get upgrade",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "unlocked install path: {forbidden}"
        );
    }
    assert!(!dockerfile.contains("mise install"));

    let build = fs::read_to_string(root().join("scripts/prefetch-workspace-image.sh")).unwrap();
    for required in ["extract-reviewed-chromium", "validate-tool-versions"] {
        assert!(
            build.contains(required),
            "missing pre-build validator: {required}"
        );
    }
}

#[test]
fn smoke_covers_every_runtime_native_tools_and_browser() {
    let smoke = fs::read_to_string(root().join("tests/image/polyglot-smoke.sh")).unwrap();
    for required in [
        "mise --version",
        "node -e",
        "python -c",
        "go run",
        "rustc",
        "javac",
        "ruby -e",
        "elixir -e",
        "/opt/gascan/tests/playwright-smoke.mjs",
        "git --version",
        "gh --version",
        "cc --version",
        "image-tool-versions.json",
        "jq --exit-status",
        "mise current elixir",
        "mise current rust",
    ] {
        assert!(
            smoke.contains(required),
            "missing smoke coverage: {required}"
        );
    }
}

#[test]
fn polyglot_smoke_fails_closed_without_exact_built_reference() {
    let missing = root().join(".artifacts/definitely-missing-polyglot-image-ref");
    let output = Command::new("bash")
        .arg(root().join("tests/image/polyglot-smoke.sh"))
        .env("GASCAN_IMAGE_REF_FILE", &missing)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("missing polyglot image reference: {}\n", missing.display())
    );
}
