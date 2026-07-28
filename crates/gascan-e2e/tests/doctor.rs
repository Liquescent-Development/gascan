use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn doctor(json: bool) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let cli = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
    let daemon = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
    let runtime = tempfile::tempdir()?;
    let root = runtime.path().canonicalize()?;
    let account_home = root.join("account-home");
    std::fs::create_dir(&account_home)?;
    let mut command = Command::new(cli);
    command
        .arg("doctor")
        .env("XDG_RUNTIME_DIR", &root)
        .env("GASCAN_STATE_PATH", root.join("state.sqlite3"))
        .env("GASCAN_FAKE_STATE_PATH", root.join("runtime.json"))
        .env("GASCAN_PID_PATH", root.join("daemon.pid"))
        .env("GASCAN_E2E_ACCOUNT_HOME", account_home)
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
    assert!(stdout.contains("Gascan is ready"));
    assert!(stdout.contains("Host"));
    assert!(stdout.contains("Runtime"));
    assert!(stdout.contains("checks passed"));
    assert!(!stdout.contains("report sha256"));
    assert!(!stdout.contains("fixture sha256"));
    assert!(!stdout.contains("runtime.offline"));
    Ok(())
}

#[test]
fn doctor_uses_the_callers_workspace_after_the_daemon_launch_directory_is_deleted() -> TestResult {
    let cli = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
    let daemon = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
    let runtime = tempfile::tempdir()?;
    let root = runtime.path().canonicalize()?;
    let account_home = root.join("account-home");
    let caller_workspace = root.join("caller-workspace");
    let launch_directory = tempfile::tempdir()?;
    std::fs::create_dir(&account_home)?;
    std::fs::create_dir(&caller_workspace)?;

    let command = |args: &[&str], directory: &std::path::Path| {
        let mut command = Command::new(&cli);
        command
            .args(args)
            .current_dir(directory)
            .env("XDG_RUNTIME_DIR", &root)
            .env("GASCAN_STATE_PATH", root.join("state.sqlite3"))
            .env("GASCAN_FAKE_STATE_PATH", root.join("runtime.json"))
            .env("GASCAN_PID_PATH", root.join("daemon.pid"))
            .env("GASCAN_E2E_ACCOUNT_HOME", &account_home)
            .env("GASCAN_DAEMON", &daemon)
            .env("GASCAN_TEST_FAKE_BACKEND", "1");
        command
    };

    let first = command(&["doctor", "--json"], launch_directory.path()).output()?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let before = command(&["daemon-attest"], launch_directory.path()).output()?;
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );
    let before: serde_json::Value = serde_json::from_slice(&before.stdout)?;
    let instance_token = before["instance_token"]
        .as_str()
        .ok_or("daemon instance token missing")?
        .to_owned();
    std::fs::remove_dir_all(launch_directory.path())?;

    let doctor = command(&["doctor", "--json"], &caller_workspace).output()?;
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout)?;
    let checks = report["checks"].as_array().ok_or("checks missing")?;
    assert!(
        checks
            .iter()
            .any(|check| { check["id"] == "workspace.access" && check["status"] == "pass" })
    );

    let attestation = command(&["daemon-attest"], &caller_workspace).output()?;
    assert!(
        attestation.status.success(),
        "{}",
        String::from_utf8_lossy(&attestation.stderr)
    );
    let attestation: serde_json::Value = serde_json::from_slice(&attestation.stdout)?;
    assert_eq!(
        attestation["instance_token"].as_str(),
        Some(instance_token.as_str()),
        "Doctor replaced the daemon that launched from the deleted directory"
    );
    Ok(())
}
