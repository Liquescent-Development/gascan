use std::{fs, process::Command};

fn root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
}

const CONFIG: &str = r#"[tools]
elixir = "1.20.2-otp-29"
erlang = "29.0.3"
go = "1.26.5"
java = "25.0.2"
node = "24.18.0"
python = "3.14.6"
ruby = "3.4.10"
rust = "1.97.0"
"#;

const LOCK: &str = r#"[tools]
elixir = "1.20.2-otp-29"
erlang = "29.0.3"
go = "1.26.5"
java = "25.0.2"
node = "24.18.0"
python = "3.14.6"
ruby = "3.4.10"
rust = "1.97.0"
"#;

const EXACT: &str = r#"{"elixir":"1.20.2-otp-29","erlang":"29.0.3","go":"1.26.5","java":"25.0.2","node":"24.18.0","python":"3.14.6","ruby":"3.4.10","rust":"1.97.0"}"#;

fn validate(resolved: Option<&str>) -> std::process::Output {
    let temp = tempfile::tempdir().unwrap();
    let lock = temp.path().join("lock.toml");
    let config = temp.path().join("config.toml");
    fs::write(&lock, LOCK).unwrap();
    fs::write(&config, CONFIG).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_validate-tool-versions"));
    command.args([&lock, &config]);
    if let Some(json) = resolved {
        let path = temp.path().join("resolved.json");
        fs::write(&path, json).unwrap();
        command.arg(path);
    }
    command.output().unwrap()
}

#[test]
fn exact_seven_runtimes_and_erlang_dependency_are_emitted_and_accepted() {
    let emitted = validate(None);
    assert!(emitted.status.success());
    let actual: serde_json::Value = serde_json::from_slice(&emitted.stdout).unwrap();
    let expected: serde_json::Value = serde_json::from_str(EXACT).unwrap();
    assert_eq!(actual, expected);
    assert!(validate(Some(EXACT)).status.success());
}

#[test]
fn mismatch_missing_and_extra_resolved_versions_fail_closed() {
    for invalid in [
        EXACT.replace("24.18.0", "24.18.1"),
        EXACT.replace(r#","rust":"1.97.0""#, ""),
        EXACT.replace('}', r#","unexpected":"1"}"#),
    ] {
        assert!(!validate(Some(&invalid)).status.success());
    }
}

#[test]
fn version_source_pins_erlang_dependency_exactly() {
    let source = fs::read_to_string(root().join("images/workspace/versions.toml")).unwrap();
    let source: toml::Value = toml::from_str(&source).unwrap();
    assert_eq!(source["tools"]["erlang"].as_str(), Some("29.0.3"));
}

#[test]
fn workstation_source_contains_only_reviewed_resolver_intent() {
    let source = fs::read_to_string(root().join("images/workspace/versions.toml")).unwrap();
    let source: toml::Value = toml::from_str(&source).unwrap();
    let workstation = source["workstation"].as_table().unwrap();
    assert_eq!(workstation.len(), 7);
    for tool in ["claude", "codex", "pi", "herdr", "glab"] {
        assert_eq!(workstation[tool].as_str(), Some("latest"));
    }
    assert_eq!(workstation["neovim"].as_str(), Some("0.11"));
    assert_eq!(workstation["starship"].as_str(), Some("1.25.1"));
}

#[test]
fn workstation_update_preserves_preexisting_runtime_locks() {
    let lock = fs::read_to_string(root().join("images/workspace/versions.lock")).unwrap();
    let lock: toml::Value = toml::from_str(&lock).unwrap();
    let expected: toml::Value = toml::from_str(LOCK).unwrap();
    assert_eq!(lock["tools"], expected["tools"]);
    assert_eq!(lock["workspace_build_mode"].as_str(), Some("connected"));
    assert_eq!(
        lock["ubuntu_snapshot"].as_str(),
        Some("2026-07-13T00:00:00Z")
    );
    assert_eq!(
        lock["workspace_bundles"],
        toml::Value::Table(toml::toml! {
            media_type = "application/vnd.gascan.workspace-bundle.v1+tar.zstd"
            platform = "linux/arm64"
            publication = "pending"
        })
    );
    assert_eq!(
        lock["base_image"].as_str(),
        Some("ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab")
    );
    assert_eq!(lock["mise"]["version"].as_str(), Some("2026.5.0"));
    assert_eq!(
        lock["mise"]["sha256"].as_str(),
        Some("fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a")
    );
    assert_eq!(
        lock["playwright_chromium"]["version"].as_str(),
        Some("149.0.7827.55+1228")
    );
    assert_eq!(
        lock["playwright_chromium"]["sha256"].as_str(),
        Some("ec044b50ed065adeb4c5ffdb42d1529901cbaf897cdf542bfef8af01d6e0cc79")
    );
    assert_eq!(
        lock["gascamp"]["revision"].as_str(),
        Some("f6b248c5926240856dbea83d1d2c5c90ea1c1456")
    );
}

#[test]
fn updater_pins_npm_and_disables_inherited_script_configuration() {
    let source = fs::read_to_string(root().join("scripts/src/bin/update-image-lock.rs")).unwrap();
    assert!(source.contains("11.12.1"));
    assert!(source.contains("--ignore-scripts"));
    assert!(source.contains("npm_config_"));
    assert!(source.contains("--globalconfig="));
    assert!(source.contains("--install-strategy=hoisted"));
    assert!(source.contains("--include=optional"));
}
