#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn cli_lifecycle_survives_daemon_and_host_state_changes() -> TestResult {
    let env = AppleE2e::new("gate4-lifecycle")?;
    env.install_noop_setup()?;
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

    for (signal, expected) in [
        (rustix::process::Signal::INT, Some(130)),
        (rustix::process::Signal::TERM, Some(143)),
    ] {
        let output = env.run_pty_signal(
            signal,
            &[
                "sh",
                "-c",
                "trap 'printf GASCAN_INT_TRAP\\n; exit 130' INT; trap 'printf GASCAN_TERM_TRAP\\n; exit 143' TERM; printf GASCAN_SIGNAL_READY\\n; while :; do sleep 1; done",
            ],
        )?;
        assert_eq!(output.status.code(), expected);
        let marker = if signal == rustix::process::Signal::INT {
            b"GASCAN_INT_TRAP".as_slice()
        } else {
            b"GASCAN_TERM_TRAP".as_slice()
        };
        assert!(
            output
                .stdout
                .windows(marker.len())
                .any(|window| window == marker),
            "guest trap marker missing: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    env.stop_owned_container()?;
    env.success([
        "--sandbox",
        env.id(),
        "apply",
        env.root().to_str().ok_or("non-UTF-8 root")?,
    ])?;
    assert_eq!(env.status_json()?["actual_state"], "running");
    env.success(["--sandbox", env.id(), "down"])?;
    assert_eq!(env.status_json()?["actual_state"], "stopped");
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;

    env.kill_daemon()?;
    assert_eq!(env.status_json()?["actual_state"], "running");
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}
