use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn version_flags_are_exact_and_do_not_require_the_daemon() -> TestResult {
    let cli = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;

    for flag in ["--version", "-V"] {
        let runtime = tempfile::tempdir()?;
        let output = Command::new(&cli)
            .arg(flag)
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("GASCAN_STATE_PATH", runtime.path().join("state.sqlite3"))
            .env("GASCAN_PID_PATH", runtime.path().join("daemon.pid"))
            .env("GASCAN_DAEMON", runtime.path().join("missing-gascand"))
            .output()?;

        assert_eq!(
            output.status.code(),
            Some(0),
            "flag {flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            format!("gascan {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
        );
        assert!(output.stderr.is_empty(), "flag {flag} wrote stderr");
        assert_eq!(
            std::fs::read_dir(runtime.path())?.count(),
            0,
            "flag {flag} created runtime state"
        );
    }
    Ok(())
}
