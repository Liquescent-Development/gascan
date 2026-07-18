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
        "tests/image/system-tools.txt",
        "install --yes --no-install-recommends",
        ".artifacts/mise-linux-arm64",
        "/usr/local/bin/mise",
        "images/workspace/etc/mise/config.toml",
        "images/workspace/etc/profile.d/mise.sh",
        "mise install --yes",
        "mise ls --current --installed --json",
        "/opt/gascan/image-tool-versions.json",
        ".artifacts/playwright-chromium-reviewed",
        "/opt/gascan/tests/playwright-smoke.mjs",
        "/tmp/resolved-tool-versions.json",
        "USER root",
        "cmp --silent /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json",
        "install -o root -g root -m 0444",
        "rm -rf /var/lib/apt/lists/*",
        "git remote add origin https://github.com/Liquescent-Development/gascamp.git",
        "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
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
        "bundles/gascamp_source_vendor",
        "ARG GASCAMP_READ_TOKEN",
        "ENV GASCAMP_READ_TOKEN",
        "--mount=type=secret",
        "credential.helper",
        "http.extraHeader",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "unlocked install path: {forbidden}"
        );
    }
    let build = fs::read_to_string(root().join("scripts/prefetch-workspace-image.sh")).unwrap();
    for required in ["extract-reviewed-chromium", "validate-tool-versions"] {
        assert!(
            build.contains(required),
            "missing pre-build validator: {required}"
        );
    }
}

#[test]
fn dockerfile_restores_only_reviewed_chromium_executable_modes() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(
            "COPY .artifacts/playwright-chromium-reviewed /opt/gascan/chromium"
        ),
        "Chromium parent directory must be copied so chrome-linux nesting is retained"
    );
    assert!(!dockerfile.contains(
        "COPY .artifacts/playwright-chromium-reviewed/chrome-linux /opt/gascan/chromium/chrome-linux"
    ));
    for executable in [
        "chrome",
        "chrome-wrapper",
        "chrome_crashpad_handler",
        "chrome_sandbox",
        "libEGL.so",
        "libGLESv2.so",
        "libvulkan.so.1",
        "libvk_swiftshader.so",
    ] {
        assert!(
            dockerfile.contains(&format!(
                "chmod 0555 /opt/gascan/chromium/chrome-linux/{executable}"
            )),
            "missing reviewed Chromium executable mode: {executable}"
        );
    }
    for forbidden in [
        "chmod -R a+x",
        "chmod -R 0555",
        "COPY --chmod=0555 .artifacts/playwright-chromium-reviewed",
    ] {
        assert!(
            !dockerfile.contains(forbidden),
            "Chromium data files must not be made executable: {forbidden}"
        );
    }
    assert!(dockerfile.contains("chmod -R a-w /opt/gascan/chromium"));
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
        "erl -noshell",
        "otp_release",
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
    assert!(smoke.contains(r#"erlang:system_info(otp_release) =:= "29""#));
    assert!(!smoke.contains(r#"otp_release) =:= <<"29">>"#));
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
