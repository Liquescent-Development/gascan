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

struct OwnedChild(Option<std::process::Child>);

impl OwnedChild {
    fn spawn(command: &mut Command) -> TestResult<Self> {
        Ok(Self(Some(command.spawn()?)))
    }

    fn id(&self) -> TestResult<u32> {
        Ok(self.0.as_ref().ok_or("owned daemon child missing")?.id())
    }

    fn try_wait(&mut self) -> TestResult<Option<std::process::ExitStatus>> {
        Ok(self
            .0
            .as_mut()
            .ok_or("owned daemon child missing")?
            .try_wait()?)
    }

    fn wait_for_exit(&mut self) -> TestResult {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let child = self.0.as_mut().ok_or("owned daemon child missing")?;
            if child.try_wait()?.is_some() {
                self.0 = None;
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(
                    format!("replaced daemon child PID {} remained live", child.id()).into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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

    fn configure_command<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        command
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

    fn command_for(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.gascan);
        command.args(args);
        self.configure_command(&mut command);
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

    fn spawn_owned_daemon(&self) -> TestResult<OwnedChild> {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = self.runtime_root.join("gascan");
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.runtime_root.join("daemon.stderr"))?;
        let mut command = Command::new(&self.gascand);
        self.configure_command(&mut command);
        command
            .current_dir(&directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .env(
                "GASCAN_DAEMON_INSTANCE_PATH",
                directory.join("daemon-instance.json"),
            )
            .env("GASCAN_DAEMON_OWNER_TOKEN", "owned-restart-e2e");
        let mut child = OwnedChild::spawn(&mut command)?;
        let pid = u64::from(child.id()?);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait()? {
                child.0 = None;
                return Err(format!("owned daemon exited before readiness: {status}").into());
            }
            let output = self.command_for(&["daemon", "status", "--json"]).output()?;
            if output.status.success() {
                let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
                if status["state"] == "running" && status["pid"].as_u64() == Some(pid) {
                    return Ok(child);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("owned daemon did not become ready".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn spawn_direct_daemon_without_launcher_record_environment(&self) -> TestResult<OwnedChild> {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = self.runtime_root.join("gascan");
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.runtime_root.join("daemon.stderr"))?;
        let mut command = Command::new(&self.gascand);
        self.configure_command(&mut command);
        command
            .current_dir(&directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .env_remove("GASCAN_DAEMON_INSTANCE_PATH")
            .env_remove("GASCAN_DAEMON_OWNER_TOKEN");
        let mut child = OwnedChild::spawn(&mut command)?;
        let pid = u64::from(child.id()?);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait()? {
                child.0 = None;
                return Err(format!("direct daemon exited before readiness: {status}").into());
            }
            let output = self.command_for(&["daemon", "status", "--json"]).output()?;
            if output.status.success() {
                let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
                if status["state"] == "running" && status["pid"].as_u64() == Some(pid) {
                    return Ok(child);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("direct daemon did not become ready".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn shutdown_daemon(&self) -> TestResult {
        let output = self
            .command_for(&["daemon", "stop", "--force", "--json"])
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "public daemon cleanup failed: status={:?}, stdout={}, stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into())
        }
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
fn direct_daemon_startup_publishes_the_standard_protected_record() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let env = Environment::new()?;
    let mut daemon = env.spawn_direct_daemon_without_launcher_record_environment()?;
    let record_path = env.runtime_root.join("gascan/daemon-instance.json");
    let metadata = std::fs::symlink_metadata(&record_path)?;
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(&record_path)?)?;
    assert_eq!(record["pid"].as_u64(), Some(u64::from(daemon.id()?)));
    assert_eq!(
        record["owner_token"].as_str().map(str::len),
        Some(64),
        "direct startup did not generate a fresh owner token"
    );
    assert_eq!(record["release_version"], env!("CARGO_PKG_VERSION"));

    env.shutdown_daemon()?;
    daemon.wait_for_exit()?;
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn daemon_start_identity_is_stable_across_caller_locale_and_timezone() -> TestResult {
    let env = Environment::new()?;
    let started = env
        .command_for(&["daemon", "start", "--json"])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TZ", "America/Phoenix")
        .env("GASCAN_DAEMON_OWNER_TOKEN", "locale-start-owner")
        .output()?;
    assert!(
        started.status.success(),
        "daemon start failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&started.stdout),
        String::from_utf8_lossy(&started.stderr)
    );
    let started: serde_json::Value = serde_json::from_slice(&started.stdout)?;
    let pid = started["pid"].as_u64().ok_or("started PID missing")?;

    for (locale, timezone) in [("en_US.UTF-8", "UTC"), ("fr_FR.UTF-8", "Asia/Tokyo")] {
        let status = env
            .command_for(&["daemon", "status", "--json"])
            .env("LC_ALL", locale)
            .env("LANG", locale)
            .env("TZ", timezone)
            .output()?;
        assert!(
            status.status.success(),
            "status under {locale}/{timezone} failed: stdout={}, stderr={}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );
        let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
        assert_eq!(status["state"], "running");
        assert_eq!(status["health"], "healthy");
        assert_eq!(status["pid"].as_u64(), Some(pid));
    }
    Ok(())
}

#[test]
fn daemon_restart_replaces_pid_and_returns_healthy() -> TestResult {
    let env = Environment::new()?;
    let mut old_process = env.spawn_owned_daemon()?;
    let old_pid = u64::from(old_process.id()?);

    let restarted = env.daemon_json("restart")?;
    assert_eq!(restarted["state"], "running");
    assert_eq!(restarted["health"], "healthy");
    assert_eq!(restarted["transition"], "restarted");
    let current_pid = restarted["pid"]
        .as_u64()
        .ok_or("restarted daemon PID missing")?;
    assert_ne!(current_pid, old_pid);
    old_process.wait_for_exit()?;
    Ok(())
}

#[test]
fn owned_child_cleanup_is_scoped_to_its_process_handle() -> TestResult {
    let mut first = Command::new("/bin/sleep");
    first.arg("30");
    let owned = OwnedChild::spawn(&mut first)?;
    let mut second = Command::new("/bin/sleep");
    second.arg("30");
    let mut neighbor = OwnedChild::spawn(&mut second)?;

    drop(owned);

    assert!(
        neighbor.try_wait()?.is_none(),
        "dropping one child guard affected a different owned child"
    );
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
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
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
fn daemon_attest_rejects_a_symlink_without_sending_protocol_bytes() -> TestResult {
    use std::io::Read as _;
    use std::os::unix::fs::PermissionsExt as _;

    let env = Environment::new()?;
    let runtime_directory = env.runtime_root.join("gascan");
    std::fs::create_dir(&runtime_directory)?;
    std::fs::set_permissions(&runtime_directory, std::fs::Permissions::from_mode(0o700))?;
    let foreign_socket = env.runtime_root.join("foreign.sock");
    let foreign = std::os::unix::net::UnixListener::bind(&foreign_socket)?;
    std::fs::set_permissions(&foreign_socket, std::fs::Permissions::from_mode(0o600))?;
    std::os::unix::fs::symlink(&foreign_socket, runtime_directory.join("gascand.sock"))?;
    foreign.set_nonblocking(true)?;
    let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let (mut stream, _) = loop {
            match foreign.accept() {
                Ok(accepted) => break accepted,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return Ok(Vec::new());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => return Err(error),
            }
        };
        stream.set_nonblocking(true)?;
        let mut bytes = vec![0_u8; 64];
        let read_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let read = loop {
            match stream.read(&mut bytes) {
                Ok(read) => break read,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= read_deadline {
                        break 0;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => return Err(error),
            }
        };
        bytes.truncate(read);
        Ok(bytes)
    });

    let output = env.command_for(&["daemon-attest"]).output()?;
    let observed = reader
        .join()
        .map_err(|_| "foreign daemon-attest reader panicked")??;
    assert!(!output.status.success());
    assert!(
        observed.is_empty(),
        "daemon-attest sent protocol bytes through an unauthenticated symlink: {observed:?}"
    );
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
