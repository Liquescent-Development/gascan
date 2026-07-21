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
