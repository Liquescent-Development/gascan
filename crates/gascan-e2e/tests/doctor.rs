use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn doctor(json: bool) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let cli = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
    let daemon = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
    let runtime = tempfile::tempdir()?;
    let root = runtime.path().canonicalize()?;
    let mut command = Command::new(cli);
    command
        .arg("doctor")
        .env("XDG_RUNTIME_DIR", &root)
        .env("GASCAN_STATE_PATH", root.join("state.sqlite3"))
        .env("GASCAN_FAKE_STATE_PATH", root.join("runtime.json"))
        .env("GASCAN_PID_PATH", root.join("daemon.pid"))
        .env("GASCAN_DAEMON", daemon)
        .env("GASCAN_TEST_FAKE_BACKEND", "1");
    if json {
        command.arg("--json");
    }
    Ok(command.output()?)
}

#[test]
fn doctor_json_contains_stable_checks_and_remedies() -> TestResult {
    let output = doctor(true)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let checks = report["checks"].as_array().ok_or("checks missing")?;
    assert!(checks.iter().any(|check| check["id"] == "runtime.offline"));
    assert!(checks.iter().all(|check| check["status"] == "pass"));
    assert!(checks.iter().all(|check| check["remedy"].is_string()));
    Ok(())
}

#[test]
fn doctor_human_output_names_each_check() -> TestResult {
    let output = doctor(false)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("runtime.version"));
    assert!(stdout.contains("runtime.offline"));
    Ok(())
}
