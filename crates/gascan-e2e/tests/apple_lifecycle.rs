#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult, marker_payload};

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn cli_lifecycle_survives_daemon_and_host_state_changes() -> TestResult {
    let env = AppleE2e::new_networked("gate4-lifecycle")?;
    env.install_noop_setup()?;
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;
    env.assert_managed_network_attachment()?;

    let native_shell = env.run_default_shell_pty_script(
        r#"printf 'GASCAN_STANDARD_SHELL_BEGIN\n'
printf 'BASH_VERSION=%s\n' "${BASH_VERSION:-}"
case $- in *i*) printf 'INTERACTIVE=yes\n';; *) printf 'INTERACTIVE=no\n';; esac
if shopt -q login_shell; then printf 'LOGIN=yes\n'; else printf 'LOGIN=no\n'; fi
printf 'SHELL=%s\n' "${SHELL:-}"
if test -r /usr/share/bash-completion/bash_completion; then
    printf 'COMPLETION=readable\n'
else
    printf 'COMPLETION=missing\n'
fi
printf 'TERM=%s\n' "${TERM:-}"
printf 'SELECTOR=%s\n' "$(< /home/workspace/.config/gascan/shell/prompt)"
printf 'GASCAN_STANDARD_SHELL_END\n'
exit 0
"#,
        b"GASCAN_STANDARD_SHELL_END",
        "gascan-apple-e2e-term",
    )?;
    if !native_shell.status.success() {
        return Err(format!(
            "default native shell failed with {:?}: {}",
            native_shell.status.code(),
            String::from_utf8_lossy(&native_shell.stdout)
        )
        .into());
    }
    let native_shell = marker_payload(
        &native_shell.stdout,
        "GASCAN_STANDARD_SHELL_BEGIN",
        "GASCAN_STANDARD_SHELL_END",
    )?;
    for required in [
        "INTERACTIVE=yes\n",
        "LOGIN=yes\n",
        "SHELL=/bin/bash\n",
        "COMPLETION=readable\n",
        "TERM=gascan-apple-e2e-term\n",
        "SELECTOR=standard\n",
    ] {
        if !native_shell.contains(required) {
            return Err(
                format!("default native shell omitted {required:?}: {native_shell:?}").into(),
            );
        }
    }
    let bash_version = native_shell
        .lines()
        .find_map(|line| line.strip_prefix("BASH_VERSION="))
        .ok_or("default native shell omitted BASH_VERSION")?;
    if bash_version.is_empty() {
        return Err("default native shell did not run Bash".into());
    }

    let dns = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "getent",
        "ahosts",
        "github.com",
    ])?;
    if dns.stdout.is_empty() {
        return Err("managed network DNS lookup returned no addresses".into());
    }

    env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "curl",
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        "20",
        "--output",
        "/dev/null",
        "https://github.com/",
    ])?;

    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;

    let exit = env.invoke(["--sandbox", env.id(), "run", "--", "sh", "-c", "exit 42"])?;
    env.assert_exit_code(&exit, 42)?;

    let default_shell = env.success(["--sandbox", env.id(), "shell"])?;
    assert!(default_shell.stderr.is_empty());

    let shell = env.success([
        "--sandbox",
        env.id(),
        "shell",
        "--",
        "printf",
        "[%s]",
        "gascan explicit argv",
    ])?;
    assert_eq!(shell.stdout, b"[gascan explicit argv]");

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
            "attempts=40; initial=$(stty size); while test \"$initial\" != '24 80' && test \"$attempts\" -gt 0; do sleep 0.05; attempts=$((attempts - 1)); initial=$(stty size); done; printf '%s\\n' \"$initial\"; test \"$initial\" = '24 80' || exit 1; trap 'size=$(stty size); printf \"%s\\n\" \"$size\"; test \"$size\" = \"47 132\" && exit 0' WINCH; printf GASCAN_RESIZE_READY; while :; do sleep 1; done",
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
        "guest did not receive the initial exact 24x80 size: stdout={} stderr={}",
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
