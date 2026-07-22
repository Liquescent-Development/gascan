#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::io::{Read, Write};
use std::process::{Command, Stdio};

async fn api_client(
    runtime_root: std::path::PathBuf,
) -> Result<
    gascan_proto::v1::gas_can_client::GasCanClient<tonic::transport::Channel>,
    Box<dyn std::error::Error>,
> {
    let socket = runtime_root.join("gascan/gascand.sock");
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_| {
            let socket = socket.clone();
            async move {
                tokio::net::UnixStream::connect(socket)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
            }
        }))
        .await?;
    Ok(gascan_proto::v1::gas_can_client::GasCanClient::new(channel))
}

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn signal_test_guard() -> Result<std::sync::MutexGuard<'static, ()>, &'static str> {
    static SIGNAL_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SIGNAL_TESTS
        .lock()
        .map_err(|_| "signal test mutex poisoned")
}

struct Environment {
    gascan: std::ffi::OsString,
    gascand: std::ffi::OsString,
    root: tempfile::TempDir,
    runtime: tempfile::TempDir,
    runtime_root: std::path::PathBuf,
}

impl Environment {
    fn new() -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
        let gascand =
            std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
        let root = tempfile::tempdir()?;
        let runtime = tempfile::tempdir()?;
        let runtime_root = runtime.path().canonicalize()?;
        Ok(Self {
            gascan,
            gascand,
            root,
            runtime,
            runtime_root,
        })
    }
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.gascan);
        command
            .args(arguments)
            .env("XDG_RUNTIME_DIR", &self.runtime_root)
            .env("GASCAN_STATE_PATH", self.runtime_root.join("state.sqlite3"))
            .env(
                "GASCAN_FAKE_STATE_PATH",
                self.runtime_root.join("runtime.json"),
            )
            .env("GASCAN_PID_PATH", self.runtime_root.join("daemon.pid"))
            .env("GASCAN_DAEMON", &self.gascand);
        command.env("GASCAN_TEST_FAKE_BACKEND", "1");
        command
    }
    fn invoke(&self, arguments: &[&str]) -> Result<std::process::Output, std::io::Error> {
        self.command(arguments).output()
    }
    fn root(&self) -> Result<&str, &'static str> {
        self.root.path().to_str().ok_or("non UTF-8 root")
    }
    fn shutdown_daemon(&self) -> TestResult {
        let socket = self.runtime_root.join("gascan/gascand.sock");
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            return Ok(());
        }
        let raw_pid = std::fs::read_to_string(self.runtime_root.join("daemon.pid"))?;
        let pid = raw_pid.parse::<i32>()?;
        let pid =
            rustix_openpty::rustix::process::Pid::from_raw(pid).ok_or("invalid daemon pid")?;
        rustix_openpty::rustix::process::kill_process(
            pid,
            rustix_openpty::rustix::process::Signal::TERM,
        )?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            if std::time::Instant::now() >= deadline {
                return Err("daemon did not remove its socket during teardown".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        let _ = self.shutdown_daemon();
    }
}

fn assert_termios_restored(
    restored: &rustix_openpty::rustix::termios::Termios,
    saved: &rustix_openpty::rustix::termios::Termios,
) {
    let mut restored_local = restored.local_modes;
    let mut saved_local = saved.local_modes;
    // PENDIN reports kernel input-queue state rather than a persistent terminal setting.
    restored_local.remove(rustix_openpty::rustix::termios::LocalModes::PENDIN);
    saved_local.remove(rustix_openpty::rustix::termios::LocalModes::PENDIN);
    assert_eq!(restored.input_modes, saved.input_modes);
    assert_eq!(restored.output_modes, saved.output_modes);
    assert_eq!(restored.control_modes, saved.control_modes);
    assert_eq!(restored_local, saved_local);
}

fn normalized_termios(
    fd: impl std::os::fd::AsFd,
) -> std::io::Result<rustix_openpty::rustix::termios::Termios> {
    use rustix_openpty::rustix;
    let mut initial = rustix::termios::tcgetattr(fd.as_fd())?;
    initial
        .local_modes
        .remove(rustix::termios::LocalModes::PENDIN);
    let mut raw = initial.clone();
    raw.make_raw();
    rustix::termios::tcsetattr(fd.as_fd(), rustix::termios::OptionalActions::Now, &raw)?;
    rustix::termios::tcsetattr(fd.as_fd(), rustix::termios::OptionalActions::Now, &initial)?;
    let mut normalized = rustix::termios::tcgetattr(fd.as_fd())?;
    normalized
        .local_modes
        .remove(rustix::termios::LocalModes::PENDIN);
    rustix::termios::tcsetattr(
        fd.as_fd(),
        rustix::termios::OptionalActions::Now,
        &normalized,
    )?;
    Ok(rustix::termios::tcgetattr(fd.as_fd())?)
}

fn run_pty_to_eof(env: &Environment, arguments: &[&str]) -> TestResult<std::process::Output> {
    use rustix_openpty::rustix;
    let pty = rustix_openpty::openpty(None, None)?;
    let saved = normalized_termios(&pty.user)?;
    let stdin = std::fs::File::from(rustix::io::dup(&pty.user)?);
    let mut command = env.command(arguments);
    let child = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(150));
    drop(pty.controller);
    let output = child.wait_with_output()?;
    assert_termios_restored(&rustix::termios::tcgetattr(&pty.user)?, &saved);
    Ok(output)
}

fn invoke_with_stderr_pty(
    env: &Environment,
    arguments: &[&str],
    no_color: bool,
) -> TestResult<(std::process::ExitStatus, Vec<u8>)> {
    let pty = rustix_openpty::openpty(None, None)?;
    let stderr = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let mut command = env.command(arguments);
    command.stdout(Stdio::piped()).stderr(stderr);
    if no_color {
        command.env("NO_COLOR", "1");
    }
    let mut child = command.spawn()?;
    drop(command);
    drop(pty.user);
    let mut controller = std::fs::File::from(pty.controller);
    let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut output = Vec::new();
        match controller.read_to_end(&mut output) {
            Ok(_) => {}
            Err(error) if error.raw_os_error() == Some(5) => {}
            Err(error) => return Err(error),
        }
        Ok(output)
    });
    let status = child.wait()?;
    let output = reader.join().map_err(|_| "PTY reader panicked")??;
    Ok((status, output))
}

fn assert_no_sgr(output: &[u8]) {
    let mut index = 0;
    while index + 1 < output.len() {
        if output[index] != b'\x1b' || output[index + 1] != b'[' {
            index += 1;
            continue;
        }
        let Some(final_offset) = output[index + 2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
        else {
            break;
        };
        let final_byte = output[index + 2 + final_offset];
        assert_ne!(final_byte, b'm', "ANSI SGR color sequence leaked");
        index += final_offset + 3;
    }
}

fn assert_static_lifecycle_output(stderr: &str, initial: &str, completion: &str) {
    assert!(stderr.contains(&format!("{initial}\n")));
    assert!(stderr.contains(&format!("{completion}\n")));
    for raw in [
        "operation",
        "before_provision",
        "after_provision",
        "provision_step",
    ] {
        assert!(!stderr.contains(raw), "raw phase leaked: {raw}");
    }
    assert!(!stderr.contains('\u{1b}'));
}

#[test]
fn complete_cli_lifecycle_uses_daemon_api() -> TestResult {
    let env = Environment::new()?;
    let up = env.invoke(&["up", env.root()?])?;
    assert!(
        up.status.success(),
        "up failed: {}",
        String::from_utf8_lossy(&up.stderr)
    );
    let stderr = String::from_utf8(up.stderr)?;
    assert!(stderr.contains("Preparing sandbox\n"));
    assert!(stderr.contains("Validating configuration\n"));
    assert!(stderr.contains("Sandbox is running\n"));
    for raw in [
        "operation",
        "before_provision",
        "after_provision",
        "provision_step",
    ] {
        assert!(!stderr.contains(raw), "raw phase leaked: {raw}");
    }
    assert!(!stderr.contains('\u{1b}'));
    let running = env.invoke(&["status", "--json"])?;
    let sandbox_id = serde_json::from_slice::<serde_json::Value>(&running.stdout)?["sandbox_id"]
        .as_str()
        .ok_or("sandbox id missing")?
        .to_owned();
    assert_eq!(
        env.invoke(&["run", "--", "fake-exit", "42"])?.status.code(),
        Some(42)
    );
    let apply = env.invoke(&["apply", env.root()?, "--json"])?;
    assert!(apply.status.success());
    assert!(
        apply
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .all(|line| serde_json::from_slice::<serde_json::Value>(line).is_ok())
    );
    let down = env.invoke(&["down"])?;
    assert!(down.status.success());
    assert_static_lifecycle_output(
        &String::from_utf8(down.stderr)?,
        "Stopping sandbox",
        &format!("Sandbox {sandbox_id} is stopped"),
    );
    let status = env.invoke(&["status", "--json"])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout)?["actual_state"],
        "stopped"
    );
    let destroy = env.invoke(&["destroy", "--yes"])?;
    assert!(destroy.status.success());
    assert_static_lifecycle_output(
        &String::from_utf8(destroy.stderr)?,
        "Destroying sandbox",
        &format!("Sandbox {sandbox_id} is destroyed"),
    );
    Ok(())
}

#[test]
fn interactive_lifecycle_progress_updates_in_place_and_finishes_cleanly() -> TestResult {
    let env = Environment::new()?;
    assert!(env.invoke(&["doctor", "--json"])?.status.success());
    for no_color in [false, true] {
        let (status, output) = invoke_with_stderr_pty(&env, &["up", env.root()?], no_color)?;
        assert!(status.success());
        let stderr = String::from_utf8(output)?;
        let completion_offset = stderr
            .find("✓ Sandbox is running")
            .ok_or("completion line missing from PTY transcript")?;
        assert!(
            stderr[..completion_offset].contains("\r\u{1b}[2K"),
            "in-place redraw missing before completion: {}",
            stderr.escape_debug()
        );
        assert!(
            stderr
                .chars()
                .any(|character| { "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(character) })
        );
        assert!(stderr.contains("Preparing sandbox"));
        assert!(stderr.contains("Validating configuration"));
        assert!(stderr.contains("Starting sandbox"));
        assert!(
            console::strip_ansi_codes(&stderr).ends_with("✓ Sandbox is running\r\n"),
            "unexpected PTY transcript: {}",
            stderr.escape_debug()
        );
        if no_color {
            assert_no_sgr(stderr.as_bytes());
        }
    }
    Ok(())
}

#[test]
fn interactive_streamed_operation_failure_clears_spinner_before_error() -> TestResult {
    let env = Environment::new()?;
    assert!(
        env.command(&["doctor", "--json"])
            .env("GASCAN_FAKE_PROVISION_FAIL", "1")
            .output()?
            .status
            .success()
    );
    let (status, output) = invoke_with_stderr_pty(&env, &["up", env.root()?], false)?;

    let stderr = String::from_utf8(output)?;
    assert_eq!(
        status.code(),
        Some(70),
        "unexpected failure status; transcript: {}",
        stderr.escape_debug()
    );
    let error_offset = stderr
        .rfind("Error: ")
        .ok_or("error line missing from PTY transcript")?;
    assert!(
        stderr[..error_offset].ends_with("\r\u{1b}[2K"),
        "spinner was not cleared immediately before error: {}",
        stderr.escape_debug()
    );
    let after_clear = &stderr[error_offset..];
    assert!(
        after_clear.starts_with("Error: ") && after_clear.ends_with("\r\n"),
        "error is not a clean newline-terminated line: {}",
        after_clear.escape_debug()
    );
    assert_eq!(after_clear.lines().count(), 1);
    assert!(!after_clear.contains("Preparing sandbox"));
    assert!(!after_clear.contains("Running project setup"));
    assert!(
        !after_clear
            .chars()
            .any(|character| { "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(character) })
    );
    Ok(())
}

#[test]
fn binary_stdin_stdout_stderr_and_environment_are_exact() -> TestResult {
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    let mut child = env
        .command(&["run", "--", "fake-echo-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("stdin missing")?
        .write_all(&[0, 0xff, b'x'])?;
    let output = child.wait_with_output()?;
    assert_eq!(output.stdout, [0, 0xff, b'x']);
    let stderr = env.invoke(&["run", "--", "fake-stderr", "literal error"])?;
    assert_eq!(stderr.stderr, b"literal error");
    let term = env
        .command(&["run", "--", "fake-env", "TERM"])
        .env("TERM", "gascan-test-term")
        .env("LC_MESSAGES", "gascan-test-messages")
        .env("SECRET_TOKEN", "must-not-cross")
        .output()?;
    assert_eq!(term.stdout, b"gascan-test-term");
    let locale = env
        .command(&["run", "--", "fake-env", "LC_MESSAGES"])
        .env("LC_MESSAGES", "gascan-test-messages")
        .output()?;
    assert_eq!(locale.stdout, b"gascan-test-messages");
    assert!(
        env.command(&["run", "--", "fake-env", "SECRET_TOKEN"])
            .env("SECRET_TOKEN", "must-not-cross")
            .output()?
            .stdout
            .is_empty()
    );
    let logs = env.invoke(&["logs"])?;
    assert!(
        logs.stdout
            .windows(3)
            .any(|window| window == [0, 0xff, b'x'])
    );
    assert!(
        logs.stdout
            .windows(b"literal error".len())
            .any(|window| window == b"literal error")
    );
    Ok(())
}

#[test]
fn unbounded_piped_stdin_gets_early_output_and_cancels_promptly() -> TestResult {
    use std::io::Read as _;
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    let mut producer = Command::new("yes").stdout(Stdio::piped()).spawn()?;
    let producer_stdout = producer.stdout.take().ok_or("producer stdout missing")?;
    let mut cli = env
        .command(&["run", "--", "fake-ready-then-drain"])
        .stdin(producer_stdout)
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdout = cli.stdout.take().ok_or("CLI stdout missing")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut ready = [0_u8; 5];
        let result = stdout.read_exact(&mut ready).map(|()| ready);
        let _ = sender.send(result);
    });
    let ready = receiver.recv_timeout(std::time::Duration::from_secs(2))??;
    assert_eq!(&ready, b"ready");
    assert!(cli.try_wait()?.is_none());
    cli.kill()?;
    let _ = cli.wait()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if producer.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            producer.kill()?;
            return Err("stdin producer did not observe cancellation".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

#[test]
fn environment_teardown_terminates_its_exact_live_daemon() -> TestResult {
    let env = Environment::new()?;
    assert!(env.invoke(&["doctor", "--json"])?.status.success());
    let socket = env.runtime_root.join("gascan/gascand.sock");
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());
    env.shutdown_daemon()?;
    assert!(std::os::unix::net::UnixStream::connect(socket).is_err());
    Ok(())
}

#[test]
fn two_unrelated_sandboxes_require_explicit_selection() -> TestResult {
    let env = Environment::new()?;
    let second = tempfile::tempdir()?;
    let second_root = second.path().to_str().ok_or("non UTF-8 root")?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    assert!(env.invoke(&["up", second_root])?.status.success());
    let list = env.invoke(&["list", "--json"])?;
    let values = serde_json::from_slice::<Vec<serde_json::Value>>(&list.stdout)?;
    assert_eq!(values.len(), 2);
    assert_eq!(env.invoke(&["status"])?.status.code(), Some(64));
    for value in values {
        let id = value["sandbox_id"].as_str().ok_or("sandbox id missing")?;
        assert!(
            env.invoke(&["--sandbox", id, "status", "--json"])?
                .status
                .success()
        );
    }
    Ok(())
}

#[test]
fn daemon_idle_restart_uses_independent_fake_runtime_truth() -> TestResult {
    let env = Environment::new()?;
    assert!(
        env.command(&["up", env.root()?])
            .env("GASCAN_IDLE_TIMEOUT_MS", "50")
            .output()?
            .status
            .success()
    );
    std::thread::sleep(std::time::Duration::from_millis(250));
    let output = env
        .command(&["run", "--", "fake-exit", "23"])
        .env("GASCAN_IDLE_TIMEOUT_MS", "50")
        .output()?;
    assert_eq!(output.status.code(), Some(23));
    Ok(())
}

#[test]
fn daemon_kill_and_restart_preserve_runtime_truth() -> TestResult {
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    let pid = std::fs::read_to_string(env.runtime_root.join("daemon.pid"))?;
    assert!(
        Command::new("kill")
            .args(["-KILL", pid.trim()])
            .status()?
            .success()
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        env.invoke(&["run", "--", "fake-exit", "31"])?.status.code(),
        Some(31)
    );
    Ok(())
}

#[test]
fn inspection_confirmation_and_remaining_commands_are_stable() -> TestResult {
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    assert!(env.invoke(&["shell", "--", "sh"])?.status.success());
    assert!(env.invoke(&["logs"])?.status.success());
    assert!(env.invoke(&["doctor", "--json"])?.status.success());
    let status = String::from_utf8(env.invoke(&["status"])?.stdout)?;
    let sandbox_id = status
        .strip_prefix("Sandbox: ")
        .and_then(|status| status.strip_suffix("\nState:   Running\n"))
        .ok_or("unexpected human status output")?;
    let list = String::from_utf8(env.invoke(&["list"])?.stdout)?;
    let width = sandbox_id.len().max("SANDBOX".len());
    assert_eq!(
        list,
        format!(
            "{:<width$}  STATE\n{sandbox_id:<width$}  Running\n",
            "SANDBOX"
        )
    );
    assert_eq!(env.invoke(&["destroy"])?.status.code(), Some(64));
    assert_eq!(
        serde_json::from_slice::<Vec<serde_json::Value>>(&env.invoke(&["list", "--json"])?.stdout)?
            .len(),
        1
    );
    let socket = env.runtime_root.join("gascan/gascand.sock");
    assert!(socket.exists());
    let _keep_runtime_alive = &env.runtime;
    Ok(())
}

#[test]
fn api_major_mismatch_has_stable_exit_and_error() -> TestResult {
    let env = Environment::new()?;
    let output = env
        .command(&["doctor", "--json"])
        .env("GASCAN_API_MAJOR", "99")
        .output()?;
    assert_eq!(output.status.code(), Some(76));
    assert!(String::from_utf8_lossy(&output.stderr).contains("incompatible_api_major"));
    Ok(())
}

#[test]
fn no_sandbox_status_error_is_actionable_and_keeps_usage_exit() -> TestResult {
    let env = Environment::new()?;
    let output = env.invoke(&["status"])?;
    assert_eq!(output.status.code(), Some(64));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.starts_with("Error: no sandbox is available\n"));
    assert!(stderr.contains("Try: gascan up <project-root>\n"));
    Ok(())
}

#[test]
fn real_pty_resize_signals_and_terminal_restoration_are_exact() -> TestResult {
    use rustix_openpty::rustix;
    let _signal_guard = signal_test_guard()?;
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    for (signal, expected) in [
        (rustix::process::Signal::INT, 130),
        (rustix::process::Signal::TERM, 143),
    ] {
        let pty = rustix_openpty::openpty(
            None,
            Some(&rustix::termios::Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        )?;
        let saved = normalized_termios(&pty.user)?;
        let stdin = std::fs::File::from(rustix::io::dup(&pty.user)?);
        let stdout = std::fs::File::from(rustix::io::dup(&pty.user)?);
        let stderr = std::fs::File::from(rustix::io::dup(&pty.user)?);
        let mut command = env.command(&["shell", "--", "fake-last-resize"]);
        let mut child = command.stdin(stdin).stdout(stdout).stderr(stderr).spawn()?;
        std::thread::sleep(std::time::Duration::from_millis(150));
        rustix::termios::tcsetwinsize(
            &pty.controller,
            rustix::termios::Winsize {
                ws_row: 47,
                ws_col: 132,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
        let pid =
            rustix::process::Pid::from_raw(i32::try_from(child.id())?).ok_or("invalid pid")?;
        assert!(
            child.try_wait()?.is_none(),
            "PTY CLI exited before SIGWINCH"
        );
        rustix::process::kill_process(pid, rustix::process::Signal::WINCH)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            child.try_wait()?.is_none(),
            "PTY CLI exited before terminating signal"
        );
        rustix::process::kill_process(pid, signal)?;
        assert_eq!(child.wait()?.code(), Some(expected));
        let restored = rustix::termios::tcgetattr(&pty.user)?;
        assert_termios_restored(&restored, &saved);
    }
    let logs = env.invoke(&["logs"])?;
    assert!(logs.stdout.windows(6).any(|window| window == b"132x47"));
    Ok(())
}

#[test]
fn real_pty_success_nonzero_and_connection_error_restore_terminal() -> TestResult {
    use rustix_openpty::rustix;
    let _signal_guard = signal_test_guard()?;
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    assert_eq!(
        run_pty_to_eof(&env, &["shell", "--", "fake-exit", "0"])?
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run_pty_to_eof(&env, &["shell", "--", "fake-exit", "37"])?
            .status
            .code(),
        Some(37)
    );

    let pty = rustix_openpty::openpty(None, None)?;
    let saved = normalized_termios(&pty.user)?;
    let stdin = std::fs::File::from(rustix::io::dup(&pty.user)?);
    let mut command = env.command(&["shell", "--", "fake-exit", "0"]);
    let child = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(150));
    let pid = std::fs::read_to_string(env.runtime_root.join("daemon.pid"))?;
    assert!(
        Command::new("kill")
            .args(["-KILL", pid.trim()])
            .status()?
            .success()
    );
    drop(pty.controller);
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(70));
    assert_termios_restored(&rustix::termios::tcgetattr(&pty.user)?, &saved);
    Ok(())
}

#[tokio::test]
async fn attach_tokens_are_one_use_atomic_and_mismatch_safe() -> TestResult {
    use gascan_proto::v1;
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    let mut client = api_client(env.runtime_root.clone()).await?;
    let id = client
        .list(v1::ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("sandbox missing")?
        .sandbox_id;
    let rejected = client
        .run(v1::RunRequest {
            sandbox: Some(v1::SandboxSelector {
                sandbox_id: id.clone(),
            }),
            command: Some(v1::CommandPayload {
                argv: vec![b"true".to_vec()],
                environment: vec![v1::EnvironmentVariable {
                    name: "SECRET_TOKEN".to_owned(),
                    value: "blocked".to_owned(),
                }],
                tty: false,
            }),
        })
        .await
        .err()
        .ok_or("direct secret environment was accepted")?;
    assert_eq!(
        rejected.message(),
        gascan_proto::error_code::INVALID_REQUEST
    );
    let allocate = |id: String| v1::RunRequest {
        sandbox: Some(v1::SandboxSelector { sandbox_id: id }),
        command: Some(v1::CommandPayload {
            argv: vec![b"true".to_vec()],
            environment: Default::default(),
            tty: false,
        }),
    };
    let mut events = client.run(allocate(id.clone())).await?.into_inner();
    let token = events
        .message()
        .await?
        .ok_or("token missing")?
        .session_token;
    let close = |token: Vec<u8>| v1::ClientFrame {
        frame: Some(v1::client_frame::Frame::Close(v1::Close {})),
        session_token: token,
    };
    let mut attached = client
        .attach(tokio_stream::iter([close(token.clone())]))
        .await?
        .into_inner();
    while attached.message().await?.is_some() {}
    let replay = match client.attach(tokio_stream::iter([close(token)])).await {
        Ok(_) => return Err("replayed token was accepted".into()),
        Err(error) => error,
    };
    assert_eq!(
        replay.message(),
        gascan_proto::error_code::UNKNOWN_SESSION_TOKEN
    );

    let mut events = client.run(allocate(id.clone())).await?.into_inner();
    let token = events
        .message()
        .await?
        .ok_or("token missing")?
        .session_token;
    let mut left = client.clone();
    let mut right = client.clone();
    let (left_result, right_result) = tokio::join!(
        left.attach(tokio_stream::iter([close(token.clone())])),
        right.attach(tokio_stream::iter([close(token)])),
    );
    assert_ne!(left_result.is_ok(), right_result.is_ok());

    let mut events = client.run(allocate(id)).await?.into_inner();
    let token = events
        .message()
        .await?
        .ok_or("token missing")?
        .session_token;
    let frames = [
        v1::ClientFrame {
            frame: Some(v1::client_frame::Frame::Resize(v1::Resize {
                columns: 90,
                rows: 30,
            })),
            session_token: token,
        },
        close(b"different-token".to_vec()),
    ];
    let mut mismatch = client
        .attach(tokio_stream::iter(frames))
        .await?
        .into_inner();
    let mut saw_mismatch = false;
    while let Some(frame) = mismatch.message().await? {
        if let Some(v1::server_frame::Frame::Error(error)) = frame.frame {
            saw_mismatch |= error.code == gascan_proto::error_code::SESSION_TOKEN_MISMATCH;
        }
    }
    assert!(saw_mismatch);
    let sandbox_id = client
        .list(v1::ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("sandbox missing")?
        .sandbox_id;
    let mut events = client.run(allocate(sandbox_id)).await?.into_inner();
    let token = events
        .message()
        .await?
        .ok_or("token missing")?
        .session_token;
    let frames = [
        v1::ClientFrame {
            frame: Some(v1::client_frame::Frame::Resize(v1::Resize {
                columns: 80,
                rows: 24,
            })),
            session_token: token,
        },
        close(Vec::new()),
    ];
    let mut empty = client
        .attach(tokio_stream::iter(frames))
        .await?
        .into_inner();
    let mut saw_empty = false;
    while let Some(frame) = empty.message().await? {
        if let Some(v1::server_frame::Frame::Error(error)) = frame.frame {
            saw_empty |= error.code == gascan_proto::error_code::EMPTY_SESSION_TOKEN;
        }
    }
    assert!(saw_empty);
    Ok(())
}

#[tokio::test]
async fn logs_since_and_follow_emit_new_byte_exact_records_then_cancel() -> TestResult {
    use gascan_proto::v1;
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    assert!(
        env.invoke(&["run", "--", "fake-stdout", "before-marker"])?
            .status
            .success()
    );
    let since_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .checked_add(1)
        .ok_or("millisecond boundary overflow")?;
    while std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        < since_millis
    {
        tokio::task::yield_now().await;
    }
    let since = since_millis.to_string();
    assert!(
        env.invoke(&["run", "--", "fake-stdout", "after-marker"])?
            .status
            .success()
    );
    let filtered = env.invoke(&["logs", "--since-millis", &since])?;
    assert!(
        !filtered
            .stdout
            .windows(13)
            .any(|window| window == b"before-marker")
    );
    assert!(
        filtered
            .stdout
            .windows(12)
            .any(|window| window == b"after-marker")
    );

    let mut client = api_client(env.runtime_root.clone()).await?;
    let id = client
        .list(v1::ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("sandbox missing")?
        .sandbox_id;
    let mut follow = client
        .logs(v1::LogsRequest {
            sandbox: Some(v1::SandboxSelector { sandbox_id: id }),
            since: None,
            follow: true,
        })
        .await?
        .into_inner();
    let initial = follow.message().await?.ok_or("initial logs missing")?;
    assert_eq!(initial.sequence, 1);
    assert_eq!(initial.status, v1::OperationStatus::Pending as i32);
    let mut command = env.command(&["run", "--", "fake-echo-stdin"]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("stdin missing")?
        .write_all(b"follow\0record")?;
    assert!(child.wait()?.success());
    let appended = tokio::time::timeout(std::time::Duration::from_secs(2), follow.message())
        .await??
        .ok_or("follow record missing")?;
    assert_eq!(appended.payload, b"follow\0record");
    assert_eq!(appended.sequence, 2);
    assert_eq!(appended.status, v1::OperationStatus::Pending as i32);
    drop(follow);
    Ok(())
}

#[tokio::test]
async fn follow_logs_emit_exactly_one_terminal_for_shutdown_or_backend_error() -> TestResult {
    use gascan_proto::v1;
    for fail_backend in [false, true] {
        let env = Environment::new()?;
        let mut start = env.command(&["up", env.root()?]);
        if fail_backend {
            start.env("GASCAN_FAKE_LOGS_FAIL_AFTER_MS", "500");
        }
        assert!(start.output()?.status.success());
        let mut client = api_client(env.runtime_root.clone()).await?;
        let id = client
            .list(v1::ListRequest {})
            .await?
            .into_inner()
            .sandboxes
            .into_iter()
            .next()
            .ok_or("sandbox missing")?
            .sandbox_id;
        let mut follow = client
            .logs(v1::LogsRequest {
                sandbox: Some(v1::SandboxSelector { sandbox_id: id }),
                since: None,
                follow: true,
            })
            .await?
            .into_inner();
        let initial = follow.message().await?.ok_or("initial logs missing")?;
        assert_eq!(initial.sequence, 1);
        if !fail_backend {
            let pid = std::fs::read_to_string(env.runtime_root.join("daemon.pid"))?;
            let pid = rustix_openpty::rustix::process::Pid::from_raw(pid.parse::<i32>()?)
                .ok_or("invalid daemon pid")?;
            rustix_openpty::rustix::process::kill_process(
                pid,
                rustix_openpty::rustix::process::Signal::TERM,
            )?;
        }
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event = follow.message().await?.ok_or("terminal missing")?;
                if event.status == v1::OperationStatus::Completed as i32
                    || event.status == v1::OperationStatus::Failed as i32
                {
                    return Ok::<_, Box<dyn std::error::Error>>(event);
                }
            }
        })
        .await??;
        assert_eq!(terminal.sequence, 2);
        assert_eq!(
            terminal.status,
            if fail_backend {
                v1::OperationStatus::Failed as i32
            } else {
                v1::OperationStatus::Completed as i32
            }
        );
        assert!(follow.message().await?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn cancelling_follow_logs_releases_daemon_activity_without_terminal() -> TestResult {
    use gascan_proto::v1;
    let env = Environment::new()?;
    assert!(
        env.command(&["up", env.root()?])
            .env("GASCAN_IDLE_TIMEOUT_MS", "300")
            .output()?
            .status
            .success()
    );
    let mut client = api_client(env.runtime_root.clone()).await?;
    let id = client
        .list(v1::ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("sandbox missing")?
        .sandbox_id;
    let mut follow = client
        .logs(v1::LogsRequest {
            sandbox: Some(v1::SandboxSelector { sandbox_id: id }),
            since: None,
            follow: true,
        })
        .await?
        .into_inner();
    let initial = follow.message().await?.ok_or("initial logs missing")?;
    assert_eq!(initial.status, v1::OperationStatus::Pending as i32);
    drop(follow);
    drop(client);
    let socket = env.runtime_root.join("gascan/gascand.sock");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn slow_up_returns_live_operation_and_survives_disconnect() -> TestResult {
    use gascan_proto::v1;
    let env = Environment::new()?;
    assert!(
        env.command(&["doctor", "--json"])
            .env("GASCAN_FAKE_PROVISION_DELAY_MS", "600")
            .output()?
            .status
            .success()
    );
    let mut client = api_client(env.runtime_root.clone()).await?;
    let response = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        client.up(v1::UpRequest {
            project_root: env.root()?.to_owned(),
        }),
    )
    .await??;
    let mut events = response.into_inner();
    let pending = events.message().await?.ok_or("pending event missing")?;
    assert!(pending.operation_id.is_some());
    assert_eq!(pending.status, v1::OperationStatus::Pending as i32);
    drop(events);
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let status = env.invoke(&["status", "--json"])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout)?["actual_state"],
        "running"
    );
    Ok(())
}

#[tokio::test]
async fn post_begin_failure_keeps_operation_id_and_streams_failed_terminal() -> TestResult {
    use gascan_proto::v1;
    let env = Environment::new()?;
    assert!(
        env.command(&["doctor", "--json"])
            .env("GASCAN_FAKE_PROVISION_FAIL", "1")
            .output()?
            .status
            .success()
    );
    let mut client = api_client(env.runtime_root.clone()).await?;
    let mut events = client
        .up(v1::UpRequest {
            project_root: env.root()?.to_owned(),
        })
        .await?
        .into_inner();
    let first = events.message().await?.ok_or("pending event missing")?;
    let operation_id = first.operation_id.ok_or("operation id missing")?;
    assert_eq!(first.status, v1::OperationStatus::Pending as i32);
    let mut failed = None;
    while let Some(event) = events.message().await? {
        if event.status == v1::OperationStatus::Failed as i32 {
            failed = Some(event);
            break;
        }
    }
    let failed = failed.ok_or("failed terminal missing")?;
    assert_eq!(failed.operation_id, Some(operation_id));
    assert!(failed.error.is_some());
    Ok(())
}

#[tokio::test]
async fn pre_begin_rpc_failures_keep_stable_statuses() -> TestResult {
    use gascan_proto::v1;
    let env = Environment::new()?;
    assert!(
        env.command(&["doctor", "--json"])
            .env("GASCAN_FAKE_PROVISION_DELAY_MS", "600")
            .output()?
            .status
            .success()
    );
    let mut client = api_client(env.runtime_root.clone()).await?;
    let missing = gascan_core::sandbox::SandboxId::test("missing").to_string();
    let down = client
        .down(v1::DownRequest {
            sandbox: Some(v1::SandboxSelector {
                sandbox_id: missing.clone(),
            }),
        })
        .await
        .err()
        .ok_or("missing stop unexpectedly succeeded")?;
    assert_eq!(down.code(), tonic::Code::NotFound);
    assert_eq!(down.message(), gascan_proto::error_code::SANDBOX_NOT_FOUND);
    let destroy = client
        .destroy(v1::DestroyRequest {
            sandbox: Some(v1::SandboxSelector {
                sandbox_id: missing,
            }),
        })
        .await
        .err()
        .ok_or("missing destroy unexpectedly succeeded")?;
    assert_eq!(destroy.code(), tonic::Code::NotFound);

    std::fs::write(
        env.root.path().join("gascan.toml"),
        "version = 1\nnetwork = 'offline'\n[ports]\nweb = 3000\n",
    )?;
    let invalid = client
        .up(v1::UpRequest {
            project_root: env.root()?.to_owned(),
        })
        .await
        .err()
        .ok_or("invalid policy unexpectedly succeeded")?;
    assert_eq!(invalid.code(), tonic::Code::InvalidArgument);

    std::fs::write(
        env.root.path().join("gascan.toml"),
        "version = 1\nnetwork = 'offline'\n",
    )?;
    let mut first = client
        .up(v1::UpRequest {
            project_root: env.root()?.to_owned(),
        })
        .await?
        .into_inner();
    while let Some(event) = first.message().await? {
        if event.status == v1::OperationStatus::Completed as i32 {
            break;
        }
    }
    let id = client
        .list(v1::ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("sandbox missing")?
        .sandbox_id;
    let id = gascan_core::sandbox::SandboxId::try_from(id)?;
    let store = gascand::Store::open(env.runtime_root.join("state.sqlite3"))?;
    let record = store.sandbox(&id)?.ok_or("sandbox record missing")?;
    let _pending = store.begin_operation(&record, gascand::OperationKind::Apply)?;
    let conflict = client
        .down(v1::DownRequest {
            sandbox: Some(v1::SandboxSelector {
                sandbox_id: id.to_string(),
            }),
        })
        .await
        .err()
        .ok_or("pending conflict unexpectedly succeeded")?;
    assert_eq!(conflict.code(), tonic::Code::AlreadyExists);
    assert_eq!(
        conflict.message(),
        gascan_proto::error_code::OPERATION_CONFLICT
    );

    let doctor_failure = Environment::new()?;
    assert!(
        doctor_failure
            .command(&["doctor", "--json"])
            .env("GASCAN_FAKE_CAPABILITIES_FAIL", "1")
            .output()?
            .status
            .success()
    );
    let failing = Environment::new()?;
    let unavailable = failing
        .command(&["up", failing.root()?])
        .env("GASCAN_FAKE_CAPABILITIES_FAIL", "1")
        .output()?;
    assert_eq!(unavailable.status.code(), Some(70));

    Ok(())
}
