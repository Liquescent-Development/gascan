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
    env.assert_exit_code(&exit, 42)?;

    let shell = env.success(["--sandbox", env.id(), "shell", "--", "sh", "-c", "id -u"])?;
    assert_eq!(shell.stdout, b"1000\r\n");

    let tty = env.run_pty(&["sh", "-c", "test -t 0 && test -t 1"])?;
    assert!(
        tty.status.success(),
        "TTY shell failed: {}",
        String::from_utf8_lossy(&tty.stderr)
    );

    let resized = env.run_pty_resize(
        &[
            "sh",
            "-c",
            "initial=$(stty size); printf '%s\\n' \"$initial\"; test \"$initial\" = '24 80' || exit 1; trap 'size=$(stty size); printf \"%s\\n\" \"$size\"; test \"$size\" = \"47 132\" && exit 0' WINCH; printf GASCAN_RESIZE_READY; while :; do sleep 1; done",
        ],
        47,
        132,
    )?;
    assert!(
        resized.status.success(),
        "resized TTY shell failed: stdout={} stderr={}",
        String::from_utf8_lossy(&resized.stdout),
        String::from_utf8_lossy(&resized.stderr)
    );
    assert!(
        resized
            .stdout
            .windows(b"24 80".len())
            .any(|window| window == b"24 80"),
        "guest did not start at exact 24x80 size: stdout={} stderr={}",
        String::from_utf8_lossy(&resized.stdout),
        String::from_utf8_lossy(&resized.stderr)
    );
    assert!(
        resized
            .stdout
            .windows(b"47 132".len())
            .any(|window| window == b"47 132"),
        "guest did not observe exact 47x132 resize: stdout={} stderr={}",
        String::from_utf8_lossy(&resized.stdout),
        String::from_utf8_lossy(&resized.stderr)
    );

    let interrupt = env.run_pty_signal(
        rustix::process::Signal::INT,
        &[
            "sh",
            "-c",
            "trap 'printf GASCAN_INT_TRAP\\n; exit 130' INT; printf GASCAN_SIGNAL_READY\\n; while :; do sleep 1; done",
        ],
    )?;
    assert_eq!(interrupt.status.code(), Some(130));
    assert!(
        interrupt
            .stdout
            .windows(b"GASCAN_INT_TRAP".len())
            .any(|window| window == b"GASCAN_INT_TRAP"),
        "guest SIGINT trap marker missing: stdout={} stderr={}",
        String::from_utf8_lossy(&interrupt.stdout),
        String::from_utf8_lossy(&interrupt.stderr)
    );

    let term_started = std::time::Instant::now();
    let unsupported_term = env.run_pty_signal(
        rustix::process::Signal::TERM,
        &[
            "sh",
            "-c",
            "trap 'printf GASCAN_TERM_TRAP\\n; exit 143' TERM; printf GASCAN_SIGNAL_READY\\n; while :; do sleep 1; done",
        ],
    )?;
    assert_eq!(unsupported_term.status.code(), Some(70));
    assert!(
        term_started.elapsed() < std::time::Duration::from_secs(2),
        "unsupported TTY SIGTERM was not rejected promptly: {:?}",
        term_started.elapsed()
    );
    assert!(
        unsupported_term
            .stdout
            .windows(b"unsupported_capability".len())
            .any(|window| window == b"unsupported_capability"),
        "typed unsupported-capability error missing: stdout={} stderr={}",
        String::from_utf8_lossy(&unsupported_term.stdout),
        String::from_utf8_lossy(&unsupported_term.stderr)
    );
    assert!(
        !unsupported_term
            .stdout
            .windows(b"GASCAN_TERM_TRAP".len())
            .any(|window| window == b"GASCAN_TERM_TRAP"),
        "unsupported TTY SIGTERM unexpectedly reached the guest: stdout={} stderr={}",
        String::from_utf8_lossy(&unsupported_term.stdout),
        String::from_utf8_lossy(&unsupported_term.stderr)
    );

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
