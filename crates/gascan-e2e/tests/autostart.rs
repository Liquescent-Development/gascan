#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::process::Command;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Environment {
    gascan: std::ffi::OsString,
    gascand: std::ffi::OsString,
    runtime: tempfile::TempDir,
    runtime_root: std::path::PathBuf,
    account_home: std::path::PathBuf,
}

#[derive(Clone)]
struct ProcessIdentity {
    pid: u64,
    executable: std::path::PathBuf,
    started: String,
}

impl ProcessIdentity {
    fn capture(pid: u64, expected_executable: &std::path::Path) -> TestResult<Self> {
        let executable = expected_executable.canonicalize()?;
        let observed_command =
            process_field(pid, "command=")?.ok_or("old daemon process disappeared")?;
        let observed_executable = observed_command
            .split_whitespace()
            .next()
            .ok_or("old daemon command is empty")?;
        if std::path::Path::new(observed_executable).canonicalize()? != executable {
            return Err("old daemon executable did not match the test fixture".into());
        }
        let started = process_field(pid, "lstart=")?.ok_or("old daemon start time missing")?;
        Ok(Self {
            pid,
            executable,
            started,
        })
    }

    fn is_running(&self) -> TestResult<bool> {
        let Some(state) = process_field(self.pid, "state=")? else {
            return Ok(false);
        };
        if state.starts_with('Z') {
            return Ok(false);
        }
        let Some(command) = process_field(self.pid, "command=")? else {
            return Ok(false);
        };
        let Some(observed_executable) = command.split_whitespace().next() else {
            return Ok(false);
        };
        let Ok(observed_executable) = std::path::Path::new(observed_executable).canonicalize()
        else {
            return Ok(false);
        };
        if observed_executable != self.executable {
            return Ok(false);
        }
        Ok(process_field(self.pid, "lstart=")?.as_deref() == Some(self.started.as_str()))
    }
}

fn process_field(pid: u64, field: &str) -> TestResult<Option<String>> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let field = String::from_utf8(output.stdout)?.trim().to_owned();
    Ok((!field.is_empty()).then_some(field))
}

struct OwnedPid {
    identity: Option<ProcessIdentity>,
}

impl OwnedPid {
    fn capture(pid: u64, executable: &std::path::Path) -> TestResult<Self> {
        Ok(Self {
            identity: Some(ProcessIdentity::capture(pid, executable)?),
        })
    }

    fn disarm(&mut self) {
        self.identity = None;
    }

    fn wait_for_exit(&self) -> TestResult {
        let identity = self.identity.as_ref().ok_or("old daemon guard disarmed")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while identity.is_running()? {
            if std::time::Instant::now() >= deadline {
                return Err(format!("replaced daemon PID {} remained live", identity.pid).into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }
}

impl Drop for OwnedPid {
    fn drop(&mut self) {
        let Some(identity) = self.identity.as_ref() else {
            return;
        };
        if !identity.is_running().unwrap_or(false) {
            return;
        }
        let _ = Command::new("kill")
            .args(["-TERM", &identity.pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        while std::time::Instant::now() < deadline {
            if !identity.is_running().unwrap_or(false) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !identity.is_running().unwrap_or(false) {
            return;
        }
        let _ = Command::new("kill")
            .args(["-KILL", &identity.pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

impl Environment {
    fn new() -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
        let gascand =
            std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
        let runtime = tempfile::tempdir()?;
        let runtime_root = runtime.path().canonicalize()?;
        let account_home = runtime_root.join("home");
        std::fs::create_dir(&account_home)?;
        Ok(Self {
            gascan,
            gascand,
            runtime,
            runtime_root,
            account_home,
        })
    }

    fn command_for(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.gascan);
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.runtime_root)
            .env("GASCAN_STATE_PATH", self.runtime_root.join("state.sqlite3"))
            .env(
                "GASCAN_FAKE_STATE_PATH",
                self.runtime_root.join("runtime.json"),
            )
            .env("GASCAN_PID_PATH", self.runtime_root.join("daemon.pid"))
            .env(
                "GASCAN_DAEMON_STDERR_PATH",
                self.runtime_root.join("daemon.stderr"),
            )
            .env("GASCAN_E2E_ACCOUNT_HOME", &self.account_home)
            .env("GASCAN_DAEMON", &self.gascand);
        command.env("GASCAN_TEST_FAKE_BACKEND", "1");
        command
    }

    fn command(&self) -> Command {
        self.command_for(&["doctor", "--json"])
    }

    fn invoke(&self) -> Result<std::process::Output, std::io::Error> {
        self.command().output()
    }

    fn daemon_json(&self, action: &str) -> TestResult<serde_json::Value> {
        let output = self.command_for(&["daemon", action, "--json"]).output()?;
        if !output.status.success() {
            let daemon_stderr = std::fs::read_to_string(self.runtime_root.join("daemon.stderr"))
                .unwrap_or_else(|error| format!("<unavailable: {error}>"));
            return Err(format!(
                "daemon {action} failed: status={:?}, stdout={}, stderr={}, daemon_stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                daemon_stderr
            )
            .into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
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

#[test]
fn daemon_status_reports_stopped_without_autostart() -> TestResult {
    let env = Environment::new()?;
    let status = env.daemon_json("status")?;
    assert_eq!(status["state"], "stopped");
    assert_eq!(status["health"], "stopped");
    assert!(status["pid"].is_null());
    assert!(!env.runtime_root.join("daemon.pid").exists());
    assert!(!env.runtime_root.join("gascan/gascand.sock").exists());
    Ok(())
}

#[test]
fn daemon_start_and_stop_are_idempotent() -> TestResult {
    let env = Environment::new()?;
    let started = env.daemon_json("start")?;
    assert_eq!(started["state"], "running");
    assert_eq!(started["health"], "healthy");
    assert_eq!(started["transition"], "started");
    let pid = started["pid"]
        .as_u64()
        .ok_or("started daemon PID missing")?;

    let unchanged = env.daemon_json("start")?;
    assert_eq!(unchanged["state"], "running");
    assert_eq!(unchanged["health"], "healthy");
    assert_eq!(unchanged["transition"], "none");
    assert_eq!(unchanged["pid"].as_u64(), Some(pid));

    let stopped = env.daemon_json("stop")?;
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["transition"], "stopped");
    assert_eq!(stopped["forced"], false);

    let still_stopped = env.daemon_json("stop")?;
    assert_eq!(still_stopped["state"], "stopped");
    assert_eq!(still_stopped["transition"], "none");
    assert_eq!(still_stopped["forced"], false);
    Ok(())
}

#[test]
fn daemon_restart_replaces_pid_and_returns_healthy() -> TestResult {
    let env = Environment::new()?;
    let started = env.daemon_json("start")?;
    let old_pid = started["pid"]
        .as_u64()
        .ok_or("started daemon PID missing")?;
    let mut old_process = OwnedPid::capture(old_pid, std::path::Path::new(&env.gascand))?;

    let restarted = env.daemon_json("restart")?;
    assert_eq!(restarted["state"], "running");
    assert_eq!(restarted["health"], "healthy");
    assert_eq!(restarted["transition"], "restarted");
    let current_pid = restarted["pid"]
        .as_u64()
        .ok_or("restarted daemon PID missing")?;
    assert_ne!(current_pid, old_pid);
    old_process.wait_for_exit()?;
    old_process.disarm();
    Ok(())
}

#[test]
fn daemon_status_works_outside_a_project() -> TestResult {
    let env = Environment::new()?;
    let unrelated = tempfile::tempdir()?;
    let output = env
        .command_for(&["daemon", "status", "--json"])
        .current_dir(unrelated.path())
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(status["state"], "stopped");
    Ok(())
}

#[test]
fn daemon_status_and_stop_survive_deleted_launch_directory() -> TestResult {
    let env = Environment::new()?;
    let launch = tempfile::tempdir()?;
    let caller = tempfile::tempdir()?;
    let started = env
        .command_for(&["daemon", "start", "--json"])
        .current_dir(launch.path())
        .output()?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    std::fs::remove_dir_all(launch.path())?;

    let status = env
        .command_for(&["daemon", "status", "--json"])
        .current_dir(caller.path())
        .output()?;
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(status["state"], "running");
    assert_eq!(status["health"], "healthy");

    let stopped = env
        .command_for(&["daemon", "stop", "--json"])
        .current_dir(caller.path())
        .output()?;
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let stopped: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
    assert_eq!(stopped["state"], "stopped");
    Ok(())
}

#[test]
fn accepted_socket_without_http2_cannot_block_initial_probe() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let env = Environment::new()?;
    let directory = env.runtime_root.join("gascan");
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let socket = directory.join("gascand.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket)?;
    let held_socket = socket.clone();
    let holder = std::thread::spawn(move || -> std::io::Result<()> {
        let (stream, _) = listener.accept()?;
        std::fs::remove_file(held_socket)?;
        std::thread::sleep(std::time::Duration::from_secs(3));
        drop(stream);
        Ok(())
    });
    let started = std::time::Instant::now();
    let mut command = env.command();
    let mut cli = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = started + std::time::Duration::from_secs(2);
    let status = loop {
        if let Some(status) = cli.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            cli.kill()?;
            let _ = cli.wait()?;
            return Err("initial readiness probe exceeded its bound".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(status.success());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    holder
        .join()
        .map_err(|_| "withholding socket thread panicked")??;
    Ok(())
}

#[test]
fn autostart_waits_for_a_slow_but_healthy_daemon() -> TestResult {
    let env = Environment::new()?;
    let started = std::time::Instant::now();
    let output = env
        .command()
        .env("GASCAN_E2E_DAEMON_START_DELAY_MS", "5500")
        .output()?;
    if !output.status.success() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        return Err(format!(
            "slow healthy daemon was abandoned: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    if started.elapsed() >= std::time::Duration::from_secs(15) {
        return Err("slow healthy daemon exceeded the bounded readiness window".into());
    }
    Ok(())
}

impl Drop for Environment {
    fn drop(&mut self) {
        let _ = self.shutdown_daemon();
    }
}

#[test]
fn concurrent_clients_converge_on_one_private_daemon() -> TestResult {
    let env = std::sync::Arc::new(Environment::new()?);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let spawn = |env: std::sync::Arc<Environment>, barrier: std::sync::Arc<std::sync::Barrier>| {
        std::thread::spawn(move || {
            barrier.wait();
            env.invoke()
        })
    };
    let left = spawn(env.clone(), barrier.clone());
    let right = spawn(env.clone(), barrier.clone());
    barrier.wait();
    let started_at = std::time::Instant::now();
    let left = left.join().map_err(|_| "left thread panicked")??;
    let right = right.join().map_err(|_| "right thread panicked")??;
    let daemon_stderr = std::fs::read_to_string(env.runtime_root.join("daemon.stderr"))
        .unwrap_or_else(|error| format!("<unavailable: {error}>"));
    let daemon_pid = std::fs::read_to_string(env.runtime_root.join("daemon.pid"))
        .unwrap_or_else(|error| format!("<unavailable: {error}>"));
    let daemon_alive = Command::new("kill")
        .args(["-0", daemon_pid.trim()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    let socket_live =
        std::os::unix::net::UnixStream::connect(env.runtime_root.join("gascan/gascand.sock"))
            .is_ok();
    let diagnostic = format!(
        "elapsed={:?}, daemon_pid={}, alive={}, socket_live={}, daemon_stderr={}",
        started_at.elapsed(),
        daemon_pid,
        daemon_alive,
        socket_live,
        daemon_stderr
    );
    for (side, output) in [("left", left), ("right", right)] {
        assert!(
            output.status.success(),
            "{side} autostart failed: status={:?}, stdout={}, stderr={}, {diagnostic}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _keep_runtime_alive = &env.runtime;
    Ok(())
}
