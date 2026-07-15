#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn cli_lifecycle_survives_daemon_and_host_state_changes() -> TestResult {
    let env = AppleE2e::new("gate4-lifecycle")?;
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;

    let exit = env.invoke(["--sandbox", env.id(), "run", "--", "sh", "-c", "exit 42"])?;
    assert_eq!(exit.status.code(), Some(42));

    let shell = env.success(["--sandbox", env.id(), "shell", "--", "sh", "-c", "id -u"])?;
    assert_eq!(shell.stdout, b"1000\n");

    let tty = env.run_pty(&["sh", "-c", "test -t 0 && test -t 1"])?;
    assert!(
        tty.status.success(),
        "TTY shell failed: {}",
        String::from_utf8_lossy(&tty.stderr)
    );

    env.success([
        "--sandbox",
        env.id(),
        "apply",
        env.root().to_str().ok_or("non-UTF-8 root")?,
    ])?;
    env.success(["--sandbox", env.id(), "down"])?;
    assert_eq!(env.status_json()?["actual_state"], "stopped");
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;

    env.kill_daemon()?;
    assert_eq!(env.status_json()?["actual_state"], "running");
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}
