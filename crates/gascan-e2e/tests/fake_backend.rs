#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn complete_cli_lifecycle_uses_daemon_api() -> TestResult {
    let gascan =
        std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan binary is not built")?;
    let gascand =
        std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand binary is not built")?;
    let root = tempfile::tempdir()?;
    let runtime = tempfile::tempdir()?;
    let runtime_root = runtime.path().canonicalize()?;
    let state = runtime_root.join("state.sqlite3");
    let invoke = |arguments: &[&str]| -> Result<std::process::Output, std::io::Error> {
        Command::new(&gascan)
            .args(arguments)
            .env("XDG_RUNTIME_DIR", &runtime_root)
            .env("GASCAN_STATE_PATH", &state)
            .env("GASCAN_DAEMON", &gascand)
            .output()
    };
    let root_arg = root.path().to_str().ok_or("non UTF-8 root")?;
    let up = invoke(&["up", root_arg])?;
    assert!(
        up.status.success(),
        "up failed: {}",
        String::from_utf8_lossy(&up.stderr)
    );
    assert_eq!(
        invoke(&["run", "--", "sh", "-c", "exit 42"])?.status.code(),
        Some(42)
    );
    let binary = invoke(&["run", "--", "sh", "-c", "printf 'hello'"])?;
    assert!(binary.status.success());
    assert_eq!(binary.stdout, b"hello");
    assert!(invoke(&["shell", "--", "sh"])?.status.success());
    assert!(invoke(&["apply", root_arg])?.status.success());
    assert!(invoke(&["logs"])?.status.success());
    assert!(invoke(&["doctor", "--json"])?.status.success());
    let list = invoke(&["list", "--json"])?;
    assert!(list.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&list.stdout)?
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(invoke(&["destroy"])?.status.code(), Some(64));
    assert!(invoke(&["down"])?.status.success());
    let status = invoke(&["status", "--json"])?;
    assert!(status.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout)?["actual_state"],
        "stopped"
    );
    assert!(invoke(&["destroy", "--yes"])?.status.success());
    Ok(())
}
