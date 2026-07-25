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
fn workstation_commands_and_mutable_shims_have_reviewed_path_precedence() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    let profile =
        fs::read_to_string(root().join("images/workspace/etc/profile.d/mise.sh")).unwrap();
    let expected = "/opt/gascan/mise/shims:/usr/local/bin:/opt/gascan/workstation/bin:${PATH}";
    assert!(
        dockerfile.contains(&format!("ENV PATH={expected}")),
        "image PATH must preserve mutable mise shims before immutable workstation tools"
    );
    assert!(
        profile.contains("export PATH=\"$MISE_DATA_DIR/shims:$MISE_SYSTEM_DATA_DIR/shims:/usr/local/bin:/opt/gascan/workstation/bin:$PATH\""),
        "interactive PATH must keep writable user shims before immutable system shims"
    );
    for required in [
        "ENV CLAUDE_CONFIG_DIR=/home/workspace/.config/gascan/agents/claude",
        "ENV CODEX_HOME=/home/workspace/.config/gascan/agents/codex",
        "ENV PI_CODING_AGENT_DIR=/home/workspace/.config/gascan/agents/pi",
        "ENV PI_CODING_AGENT_SESSION_DIR=/home/workspace/.cache/pi",
        "ENV HERDR_CONFIG_PATH=/home/workspace/.config/gascan/herdr/config.toml",
        "ENV GH_CONFIG_DIR=/home/workspace/.config/gascan/gh",
        "ENV GLAB_CONFIG_DIR=/home/workspace/.config/gascan/glab",
    ] {
        assert!(
            dockerfile.contains(required),
            "missing persistent workstation environment: {required}"
        );
    }
}

#[test]
fn interactive_mise_profile_preserves_runtime_managed_storage_overrides() {
    let profile = root().join("images/workspace/etc/profile.d/mise.sh");
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            ". '{}'; printf '%s\\n%s\\n%s\\n%s\\n' \"$MISE_DATA_DIR\" \"$MISE_CACHE_DIR\" \"$MISE_GLOBAL_CONFIG_FILE\" \"$PATH\"",
            profile.display()
        ))
        .env("MISE_DATA_DIR", "/home/workspace/.local/share/mise")
        .env("MISE_CACHE_DIR", "/home/workspace/.cache/mise")
        .env(
            "MISE_GLOBAL_CONFIG_FILE",
            "/home/workspace/.config/gascan/mise.toml",
        )
        .env("PATH", "/runtime/bin")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "/home/workspace/.local/share/mise\n",
            "/home/workspace/.cache/mise\n",
            "/home/workspace/.config/gascan/mise.toml\n",
            "/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:",
            "/usr/local/bin:/opt/gascan/workstation/bin:/runtime/bin\n"
        )
    );
}

#[test]
fn interactive_mise_profile_falls_back_to_immutable_reviewed_defaults() {
    let profile = root().join("images/workspace/etc/profile.d/mise.sh");
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            ". '{}'; printf '%s\\n%s\\n%s\\n%s\\n%s\\n' \"$MISE_DATA_DIR\" \"$MISE_SYSTEM_DATA_DIR\" \"$MISE_CACHE_DIR\" \"$MISE_GLOBAL_CONFIG_FILE\" \"$PATH\"",
            profile.display()
        ))
        .env_remove("MISE_DATA_DIR")
        .env_remove("MISE_SYSTEM_DATA_DIR")
        .env_remove("MISE_CACHE_DIR")
        .env_remove("MISE_GLOBAL_CONFIG_FILE")
        .env("PATH", "/runtime/bin")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "/opt/gascan/mise\n",
            "/opt/gascan/mise\n",
            "/home/workspace/.cache/mise\n",
            "/etc/mise/config.toml\n",
            "/opt/gascan/mise/shims:/usr/local/bin:",
            "/opt/gascan/workstation/bin:/runtime/bin\n"
        )
    );
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
        dockerfile.contains("COPY .artifacts/playwright-chromium-reviewed /opt/gascan/chromium"),
        "Chromium parent directory must be copied so chrome-linux nesting is retained"
    );
    assert!(!dockerfile.contains(
        "COPY .artifacts/playwright-chromium-reviewed/chrome-linux /opt/gascan/chromium/chrome-linux"
    ));
    assert!(dockerfile.contains("/usr/local/bin/run-workstation-step chromium-mode chmod 0555"));
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
            dockerfile.contains(&format!("/opt/gascan/chromium/chrome-linux/{executable}")),
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
fn copied_smoke_assets_are_traversable_but_immutable_to_non_root_users() {
    let dockerfile = fs::read_to_string(root().join("images/workspace/Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(
            "/usr/local/bin/run-workstation-step test-directory-mode \\\n       chmod 0555 /opt/gascan/tests"
        ),
        "the image must explicitly restore directory traversal after copying read-only smoke files"
    );
    for asset in ["playwright-smoke.mjs", "workstation-contract.sh"] {
        assert!(
            dockerfile.contains(&format!(
                "COPY --chmod=0{} images/workspace/tests/{asset} /opt/gascan/tests/{asset}",
                if asset.ends_with(".sh") { "555" } else { "444" }
            )),
            "missing immutable smoke asset: {asset}"
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
