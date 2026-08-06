use std::process::Command;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const OUTDATED_RELEASE: &str = "0.1.10-e2e";

struct OwnedChild(Option<std::process::Child>);

impl OwnedChild {
    fn new(child: std::process::Child) -> Self {
        Self(Some(child))
    }

    fn id(&self) -> TestResult<u32> {
        Ok(self.0.as_ref().ok_or("owned child missing")?.id())
    }

    fn try_wait(&mut self) -> TestResult<Option<std::process::ExitStatus>> {
        Ok(self.0.as_mut().ok_or("owned child missing")?.try_wait()?)
    }

    fn wait_with_output(&mut self) -> TestResult<std::process::Output> {
        Ok(self
            .0
            .take()
            .ok_or("owned child missing")?
            .wait_with_output()?)
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn process_start_identity(pid: u32) -> TestResult<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let remainder = stat.rsplit_once(") ").ok_or("malformed process stat")?.1;
        let start = remainder
            .split_whitespace()
            .nth(19)
            .ok_or("process stat lacks start identity")?;
        Ok(format!("linux:{start}"))
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("TZ", "UTC")
            .output()?;
        if !output.status.success() {
            return Err("could not inspect fixture process start identity".into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err("unsupported E2E process inspection platform".into())
    }
}

#[cfg(target_os = "macos")]
fn process_matches(
    pid: u32,
    expected_start_identity: &str,
    expected_executable: &std::path::Path,
) -> TestResult<bool> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let Ok(first_start) = process_start_identity(pid) else {
        return Ok(false);
    };
    if first_start != expected_start_identity {
        return Ok(false);
    }
    let output = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-F0pfn"])
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let mut matched_process = false;
    let mut awaiting_text_name = false;
    let mut executable = None;
    for raw_field in output.stdout.split(|byte| *byte == 0) {
        let field = raw_field.strip_prefix(b"\n").unwrap_or(raw_field);
        let field = field.strip_prefix(b"\r").unwrap_or(field);
        if let Some(value) = field.strip_prefix(b"p") {
            matched_process = std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                == Some(pid);
            awaiting_text_name = false;
        } else if field == b"ftxt" && matched_process {
            awaiting_text_name = true;
        } else if awaiting_text_name && let Some(path) = field.strip_prefix(b"n") {
            executable = Some(std::path::PathBuf::from(OsString::from_vec(path.to_vec())));
            break;
        }
    }
    if executable.as_deref() != Some(expected_executable) {
        return Ok(false);
    }
    Ok(process_start_identity(pid)
        .is_ok_and(|second_start| second_start == expected_start_identity))
}

struct UpgradeEnvironment {
    cli: std::ffi::OsString,
    daemon: std::ffi::OsString,
    runtime: tempfile::TempDir,
    root: std::path::PathBuf,
    account_home: std::path::PathBuf,
    cleanup_daemon: std::path::PathBuf,
    fixture: Option<std::process::Child>,
}

impl UpgradeEnvironment {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let cli = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
        let daemon =
            std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
        let runtime = tempfile::tempdir()?;
        let root = runtime.path().canonicalize()?;
        let account_home = root.join("account-home");
        std::fs::create_dir(&account_home)?;
        let cleanup_daemon = std::path::PathBuf::from(&daemon);
        Ok(Self {
            cli,
            daemon,
            runtime,
            root,
            account_home,
            cleanup_daemon,
            fixture: None,
        })
    }

    fn configure(&self, command: &mut Command) {
        command
            .env("XDG_RUNTIME_DIR", &self.root)
            .env("GASCAN_STATE_PATH", self.root.join("state.sqlite3"))
            .env("GASCAN_FAKE_STATE_PATH", self.root.join("runtime.json"))
            .env("GASCAN_PID_PATH", self.root.join("daemon.pid"))
            .env("GASCAN_DAEMON_STDERR_PATH", self.root.join("daemon.stderr"))
            .env("GASCAN_E2E_ACCOUNT_HOME", &self.account_home)
            .env("GASCAN_DAEMON", &self.daemon)
            .env("GASCAN_TEST_FAKE_BACKEND", "1");
    }

    fn cli(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.cli);
        command.args(args);
        self.configure(&mut command);
        command
    }

    fn cli_with_daemon(&self, args: &[&str], daemon: &std::path::Path) -> Command {
        let mut command = self.cli(args);
        command.env("GASCAN_DAEMON", daemon);
        command
    }

    fn spawn_outdated(&mut self, hold_operation: bool) -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = self.root.join("gascan");
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let mut command = Command::new(&self.daemon);
        self.configure(&mut command);
        command
            .current_dir(&directory)
            .env(
                "GASCAN_DAEMON_INSTANCE_PATH",
                directory.join("daemon-instance.json"),
            )
            .env("GASCAN_DAEMON_OWNER_TOKEN", "11".repeat(32))
            .env("GASCAN_E2E_RELEASE_VERSION", OUTDATED_RELEASE)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(self.root.join("fixture.stderr"))?);
        if hold_operation {
            command.env("GASCAN_E2E_HOLD_OPERATION", "1");
        }
        self.fixture = Some(command.spawn()?);
        self.wait_for_running_release(OUTDATED_RELEASE)
    }

    #[cfg(target_os = "macos")]
    fn spawn_installed_outdated(&mut self, installed_daemon: &std::path::Path) -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        self.cleanup_daemon = installed_daemon.to_owned();
        let directory = self.root.join("gascan");
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let mut command = Command::new(installed_daemon);
        self.configure(&mut command);
        command
            .env("GASCAN_DAEMON", installed_daemon)
            .current_dir(&directory)
            .env(
                "GASCAN_DAEMON_INSTANCE_PATH",
                directory.join("daemon-instance.json"),
            )
            .env("GASCAN_DAEMON_OWNER_TOKEN", "installed-predecessor-owner")
            .env("GASCAN_E2E_RELEASE_VERSION", OUTDATED_RELEASE)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(self.root.join("fixture.stderr"))?);
        self.fixture = Some(command.spawn()?);
        self.wait_for_running_release_at(OUTDATED_RELEASE, installed_daemon)
    }

    fn spawn_legacy(
        &mut self,
        flip_token_on_reattestation: bool,
        handshake_gate: Option<(&std::path::Path, &std::path::Path)>,
    ) -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = self.root.join("gascan");
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let mut command = Command::new(&self.daemon);
        self.configure(&mut command);
        command
            .current_dir(&directory)
            .env("GASCAN_E2E_LEGACY_WIRE_IDENTITY", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(self.root.join("fixture.stderr"))?);
        if flip_token_on_reattestation {
            command.env("GASCAN_E2E_FLIP_TOKEN_ON_REATTESTATION", "1");
        }
        if let Some((marker, release)) = handshake_gate {
            command
                .env("GASCAN_E2E_REATTESTATION_MARKER", marker)
                .env("GASCAN_E2E_REATTESTATION_RELEASE", release);
        }
        self.fixture = Some(command.spawn()?);
        self.wait_for_socket()
    }

    fn wait_for_socket(&mut self) -> TestResult {
        use std::os::unix::fs::FileTypeExt as _;
        let socket = self.root.join("gascan/gascand.sock");
        let pid = self.root.join("daemon.pid");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if socket
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_socket())
                && pid.exists()
            {
                return Ok(());
            }
            if let Some(status) = self
                .fixture
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
            {
                let stderr =
                    std::fs::read_to_string(self.root.join("fixture.stderr")).unwrap_or_default();
                return Err(
                    format!("legacy fixture exited before binding: {status}: {stderr}").into(),
                );
            }
            if std::time::Instant::now() >= deadline {
                return Err("legacy fixture did not bind its socket".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn wait_for_path(&self, path: &std::path::Path) -> TestResult {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !path.exists() {
            if std::time::Instant::now() >= deadline {
                return Err(format!("timed out waiting for {}", path.display()).into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }

    fn wait_for_running_release(&mut self, expected: &str) -> TestResult {
        let daemon = std::path::PathBuf::from(self.daemon.clone());
        self.wait_for_running_release_at(expected, &daemon)
    }

    fn wait_for_running_release_at(
        &mut self,
        expected: &str,
        daemon: &std::path::Path,
    ) -> TestResult {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let output = self
                .cli_with_daemon(&["daemon", "status", "--json"], daemon)
                .output()?;
            let last_status = format!(
                "status={:?}, stdout={}, stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if output.status.success() {
                let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
                if status["state"] == "running" && status["running_version"] == expected {
                    return Ok(());
                }
            }
            if let Some(status) = self
                .fixture
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
            {
                let fixture_stderr =
                    std::fs::read_to_string(self.root.join("fixture.stderr")).unwrap_or_default();
                return Err(format!(
                    "fixture exited before reporting release {expected}: {status}; {}; fixture stderr: {fixture_stderr}",
                    last_status
                )
                .into());
            }
            if std::time::Instant::now() >= deadline {
                let daemon_stderr =
                    std::fs::read_to_string(self.root.join("daemon.stderr")).unwrap_or_default();
                return Err(format!(
                    "fixture did not report release {expected}; {}; daemon stderr: {daemon_stderr}",
                    last_status
                )
                .into());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    fn pid(&self) -> TestResult<u32> {
        Ok(std::fs::read_to_string(self.root.join("daemon.pid"))?
            .trim()
            .parse()?)
    }

    fn fixture_is_running(&mut self) -> TestResult<bool> {
        Ok(self
            .fixture
            .as_mut()
            .ok_or("fixture process missing")?
            .try_wait()?
            .is_none())
    }

    fn reap_fixture(&mut self) -> TestResult {
        let mut child = self.fixture.take().ok_or("fixture process missing")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(format!("outdated fixture exited unsuccessfully: {status}").into());
                }
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                self.fixture = Some(child);
                return Err("outdated fixture did not exit after recovery".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for UpgradeEnvironment {
    fn drop(&mut self) {
        let cleanup_daemon = self.cleanup_daemon.clone();
        let _ = self
            .cli_with_daemon(&["daemon", "stop", "--force", "--json"], &cleanup_daemon)
            .output();
        if let Some(child) = &mut self.fixture {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _keep_runtime_alive = &self.runtime;
    }
}

fn doctor(json: bool) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let env = UpgradeEnvironment::new()?;
    let args = if json {
        &["doctor", "--json"][..]
    } else {
        &["doctor"][..]
    };
    Ok(env.cli(args).output()?)
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
fn doctor_json_recovers_an_outdated_compatible_daemon() -> TestResult {
    let mut env = UpgradeEnvironment::new()?;
    env.spawn_outdated(false)?;
    let old_pid = env.pid()?;

    let output = env.cli(&["doctor", "--json"]).output()?;
    assert!(
        output.status.success(),
        "status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(report["checks"].is_array());
    env.reap_fixture()?;
    let fixture_stderr = std::fs::read_to_string(env.root.join("fixture.stderr"))?;
    assert!(
        fixture_stderr.contains("daemon shutdown began: rpc"),
        "outdated daemon did not use the supported shutdown RPC: {fixture_stderr}"
    );
    let status = env.cli(&["daemon", "status", "--json"]).output()?;
    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(status["state"], "running");
    assert_eq!(status["health"], "healthy");
    assert_eq!(status["running_version"], env!("CARGO_PKG_VERSION"));
    let current_pid = status["pid"].as_u64().ok_or("current daemon PID missing")?;
    assert_ne!(current_pid, u64::from(old_pid));
    assert!(
        !Command::new("kill")
            .args(["-0", &old_pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success(),
        "outdated daemon remained live"
    );
    assert!(
        Command::new("kill")
            .args(["-0", &current_pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success(),
        "current daemon is not live"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn doctor_recovers_after_atomic_installed_daemon_replacement() -> TestResult {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mut env = UpgradeEnvironment::new()?;
    let installed_fixture = tempfile::tempdir()?;
    let installed_directory = installed_fixture.path().canonicalize()?;
    let installed_daemon = installed_directory.join("gascand");
    std::fs::copy(&env.daemon, &installed_daemon)?;
    std::fs::set_permissions(&installed_daemon, std::fs::Permissions::from_mode(0o755))?;
    env.spawn_installed_outdated(&installed_daemon)?;
    let predecessor_pid = env.pid()?;
    let predecessor_vnode = std::fs::metadata(&installed_daemon)?.ino();

    let staged = installed_directory.join("gascand.current");
    std::fs::copy(&env.daemon, &staged)?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&staged, &installed_daemon)?;
    assert_ne!(
        std::fs::metadata(&installed_daemon)?.ino(),
        predecessor_vnode,
        "test fixture did not atomically replace the installed executable vnode"
    );

    let output = env
        .cli_with_daemon(&["doctor", "--json"], &installed_daemon)
        .output()?;
    assert!(
        output.status.success(),
        "upgrade recovery failed: status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(report["checks"].is_array());
    env.reap_fixture()?;

    let status = env
        .cli_with_daemon(&["daemon", "status", "--json"], &installed_daemon)
        .output()?;
    assert!(
        status.status.success(),
        "replacement status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
    let current_pid = status["pid"]
        .as_u64()
        .ok_or("replacement daemon PID missing")?;
    assert_eq!(status["state"], "running");
    assert_eq!(status["health"], "healthy");
    assert_eq!(status["running_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        status["executable"].as_str(),
        installed_daemon.to_str(),
        "replacement did not attest the stable installed pathname"
    );
    assert_ne!(current_pid, u64::from(predecessor_pid));
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", &predecessor_pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success(),
        "predecessor remained live after replacement"
    );
    assert!(
        Command::new("/bin/kill")
            .args(["-0", &current_pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success(),
        "current replacement is not live"
    );
    let current_pid = u32::try_from(current_pid)?;
    let current_start_identity = process_start_identity(current_pid)?;
    drop(env);
    let replacement_survived_cleanup =
        process_matches(current_pid, &current_start_identity, &installed_daemon)?;
    if replacement_survived_cleanup {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &current_pid.to_string()])
            .status();
    }
    assert!(
        !replacement_survived_cleanup,
        "failure-safe environment cleanup left the replacement daemon live"
    );
    let _keep_installed_fixture_alive = installed_fixture;
    Ok(())
}

#[test]
fn doctor_recovers_a_legacy_daemon_through_double_attested_sigterm() -> TestResult {
    let mut env = UpgradeEnvironment::new()?;
    env.spawn_legacy(false, None)?;
    let old_pid = env.pid()?;
    let before = env.cli(&["daemon", "status", "--json"]).output()?;
    assert!(before.status.success());
    let before: serde_json::Value = serde_json::from_slice(&before.stdout)?;
    assert_eq!(before["state"], "running");
    assert_eq!(before["health"], "outdated");
    assert_eq!(before["legacy"], true);
    assert!(before["running_version"].is_null());

    let output = env.cli(&["doctor", "--json"]).output()?;
    assert!(
        output.status.success(),
        "status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(report["checks"].is_array());
    env.reap_fixture()?;
    let fixture_stderr = std::fs::read_to_string(env.root.join("fixture.stderr"))?;
    assert!(
        fixture_stderr.contains("daemon shutdown began: terminated"),
        "legacy daemon was not terminated by the attested signal path: {fixture_stderr}"
    );
    let current = env.cli(&["daemon", "status", "--json"]).output()?;
    assert!(current.status.success());
    let current: serde_json::Value = serde_json::from_slice(&current.stdout)?;
    assert_eq!(current["health"], "healthy");
    assert_eq!(current["running_version"], env!("CARGO_PKG_VERSION"));
    assert_ne!(current["pid"].as_u64(), Some(u64::from(old_pid)));
    Ok(())
}

#[test]
fn doctor_recovery_does_not_force_a_held_durable_operation() -> TestResult {
    let mut env = UpgradeEnvironment::new()?;
    env.spawn_outdated(true)?;
    let old_pid = env.pid()?;
    let started = std::time::Instant::now();

    let output = env.cli(&["doctor", "--json"]).output()?;
    assert!(!output.status.success(), "held operation was interrupted");
    assert!(
        output.stdout.is_empty(),
        "failed Doctor emitted partial JSON"
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("did not exit after graceful shutdown"));
    assert!(stderr.contains("--force"));
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert_eq!(env.pid()?, old_pid, "a replacement daemon was spawned");
    assert!(
        env.fixture_is_running()?,
        "automatic recovery force-killed held work"
    );
    Ok(())
}

#[test]
fn forged_instance_record_never_signals_an_unrelated_process() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let env = UpgradeEnvironment::new()?;
    let mut sleeper = OwnedChild::new(
        Command::new("/bin/sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?,
    );
    let pid = sleeper.id()?;
    let directory = env.root.join("gascan");
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let record = serde_json::json!({
        "pid": pid,
        "owner_token": "33".repeat(32),
        "executable": std::fs::canonicalize("/bin/sleep")?,
        "start_identity": process_start_identity(pid)?,
        "instance_token": "44".repeat(32),
        "release_version": env!("CARGO_PKG_VERSION"),
        "started_at": {"seconds": 1, "nanos": 0},
    });
    let instance = directory.join("daemon-instance.json");
    std::fs::write(&instance, serde_json::to_vec(&record)?)?;
    std::fs::set_permissions(&instance, std::fs::Permissions::from_mode(0o600))?;

    let output = env.cli(&["daemon", "stop", "--force", "--json"]).output()?;
    assert!(
        !output.status.success(),
        "forged daemon record was accepted"
    );
    assert!(
        sleeper.try_wait()?.is_none(),
        "unrelated fixture process received a signal"
    );
    Ok(())
}

#[test]
fn changing_instance_token_between_attestations_aborts_shutdown() -> TestResult {
    let mut env = UpgradeEnvironment::new()?;
    env.spawn_legacy(true, None)?;

    let output = env.cli(&["daemon", "stop", "--force", "--json"]).output()?;
    assert!(!output.status.success(), "changed token was accepted");
    assert!(
        env.fixture_is_running()?,
        "daemon was signaled after its token changed"
    );
    Ok(())
}

#[test]
fn replacing_endpoint_after_inspection_aborts_shutdown() -> TestResult {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
    let mut env = UpgradeEnvironment::new()?;
    let marker = env.root.join("second-handshake.started");
    let release = env.root.join("second-handshake.release");
    env.spawn_legacy(false, Some((&marker, &release)))?;
    let socket = env.root.join("gascan/gascand.sock");
    let original_socket = env.root.join("gascan/original.sock");
    let mut stopping = env.cli(&["daemon", "stop", "--force", "--json"]);
    stopping
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut stopping = OwnedChild::new(stopping.spawn()?);
    env.wait_for_path(&marker)?;
    let runtime_uid = std::fs::metadata(env.root.join("gascan"))?.uid();
    let original = std::fs::symlink_metadata(&socket)?;
    assert!(original.file_type().is_socket());
    assert_eq!(original.permissions().mode() & 0o777, 0o600);
    assert_eq!(original.uid(), runtime_uid);
    std::fs::rename(&socket, &original_socket)?;
    let _replacement_listener = std::os::unix::net::UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    std::fs::write(&release, b"continue")?;

    let output = stopping.wait_with_output()?;
    assert!(
        !output.status.success(),
        "replacement endpoint was accepted"
    );
    let replacement = std::fs::symlink_metadata(&socket)?;
    assert!(replacement.file_type().is_socket());
    assert_eq!(replacement.permissions().mode() & 0o777, 0o600);
    assert_eq!(replacement.uid(), runtime_uid);
    assert_ne!(
        replacement.ino(),
        original.ino(),
        "replacement socket reused the inspected endpoint inode"
    );
    assert!(
        env.fixture_is_running()?,
        "daemon was signaled after endpoint replacement"
    );
    Ok(())
}

#[test]
fn unsafe_socket_symlink_fails_closed() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let env = UpgradeEnvironment::new()?;
    let directory = env.root.join("gascan");
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let foreign = env.root.join("foreign-endpoint");
    std::fs::write(&foreign, b"retain")?;
    std::os::unix::fs::symlink(&foreign, directory.join("gascand.sock"))?;

    let status = env.cli(&["daemon", "status", "--json"]).output()?;
    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(status["state"], "unsafe");
    let start = env.cli(&["daemon", "start", "--json"]).output()?;
    assert!(
        !start.status.success(),
        "daemon replaced an unsafe endpoint"
    );
    assert_eq!(std::fs::read(foreign)?, b"retain");
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
    let env = UpgradeEnvironment::new()?;
    let caller_workspace = env.root.join("caller-workspace");
    let launch_directory = tempfile::tempdir()?;
    std::fs::create_dir(&caller_workspace)?;

    let command = |args: &[&str], directory: &std::path::Path| {
        let mut command = env.cli(args);
        command.current_dir(directory);
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
