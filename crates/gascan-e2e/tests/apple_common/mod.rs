#![allow(dead_code)]

use gascan_core::sandbox::SandboxId;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::io::Read as _;
use std::process::{Command, ExitStatus, Output, Stdio};

#[derive(serde::Deserialize)]
struct DaemonInstanceRecord {
    pid: u32,
    owner_token: String,
    executable: std::path::PathBuf,
    start_identity: String,
    instance_token: String,
}

#[derive(serde::Deserialize)]
struct DaemonAttestation {
    instance_token: String,
    pid: u32,
    executable: std::path::PathBuf,
    start_identity: String,
}

pub struct PtySignalOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn assert_exit_code(output: &Output, expected: i32) -> TestResult {
    if output.status.code() == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "expected {expected} exit code, got {:?}: stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

pub struct AppleE2e {
    gascan: OsString,
    gascand: OsString,
    root: Option<tempfile::TempDir>,
    runtime: Option<tempfile::TempDir>,
    root_path: std::path::PathBuf,
    runtime_root: std::path::PathBuf,
    id: SandboxId,
    manifest_name: String,
    owner_token: String,
}

impl AppleE2e {
    pub fn new(name: &str) -> TestResult<Self> {
        let manifest =
            std::env::var_os("GASCAN_E2E_CLEANUP_MANIFEST").map(std::path::PathBuf::from);
        let session_root = std::env::var_os("GASCAN_E2E_SESSION_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self::new_scoped(name, session_root, manifest)
    }

    fn new_scoped(
        name: &str,
        session_root: std::path::PathBuf,
        manifest: Option<std::path::PathBuf>,
    ) -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli")
            .ok_or("workspace-built gascan binary is unavailable")?;
        let gascand = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon")
            .ok_or("workspace-built gascand binary is unavailable")?;
        let root = tempfile::Builder::new()
            .prefix("gascan-gate4-root-")
            .tempdir_in(&session_root)?;
        let runtime = tempfile::Builder::new()
            .prefix("gascan-gate4-runtime-")
            .tempdir_in(&session_root)?;
        for path in [root.path(), runtime.path()] {
            std::fs::set_permissions(
                path,
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
            )?;
        }
        let root_path = root.path().canonicalize()?;
        let runtime_root = runtime.path().canonicalize()?;
        let utf8_root = camino::Utf8Path::from_path(&root_path).ok_or("non-UTF-8 test root")?;
        std::fs::write(
            root_path.join("gascan.toml"),
            format!("version = 1\nname = {}\n", serde_json::to_string(name)?),
        )?;
        let loaded_manifest = gascan_core::manifest::Manifest::load(utf8_root)?;
        let manifest_name = loaded_manifest
            .name()
            .ok_or("Gate 4 manifest must have an explicit name")?
            .to_owned();
        let id = SandboxId::from_root(&manifest_name, utf8_root);
        let cleanup_resources =
            gascan_core::policy::PolicyCompiler::expected_resource_identities(&id)?;
        let owner_token = format!(
            "gate4-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        if let Some(manifest) = manifest {
            let record = serde_json::json!({
                "version": 1,
                "sandbox_id": id.as_str(),
                "resources": cleanup_resources
                    .iter()
                    .map(gascan_core::runtime::ResourceIdentity::name)
                    .collect::<Vec<_>>(),
                "managed_by": "gascan",
                "owner_token": owner_token,
                "daemon_instance_path": runtime_root.join("daemon-instance.json"),
                "daemon_executable": std::path::PathBuf::from(&gascand).canonicalize()?,
                "daemon_cli": std::path::PathBuf::from(&gascan).canonicalize()?,
                "runtime_root": runtime_root,
                "project_root": root_path,
                "session_root": session_root.canonicalize()?,
            });
            let temporary = manifest.with_extension("tmp");
            std::fs::write(&temporary, serde_json::to_vec(&record)?)?;
            std::fs::set_permissions(
                &temporary,
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
            )?;
            std::fs::rename(temporary, manifest)?;
        }
        Ok(Self {
            gascan,
            gascand,
            root: Some(root),
            runtime: Some(runtime),
            root_path,
            runtime_root,
            id,
            manifest_name,
            owner_token,
        })
    }

    pub fn root(&self) -> &OsStr {
        self.root_path.as_os_str()
    }

    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    pub fn state_path(&self) -> std::path::PathBuf {
        self.runtime_root.join("state.sqlite3")
    }

    pub fn install_noop_setup(&self) -> TestResult {
        std::fs::create_dir(self.root_path.join(".gascan"))?;
        std::fs::write(
            self.root_path.join("gascan.toml"),
            format!(
                "version = 1\nname = {}\nsetup = './.gascan/setup.sh'\n",
                serde_json::to_string(&self.manifest_name)?
            ),
        )?;
        std::fs::write(
            self.root_path.join(".gascan/setup.sh"),
            "#!/bin/sh\nset -eu\n: # intentional Gate 4 no-op\n",
        )?;
        Ok(())
    }

    pub fn stop_owned_container(&self) -> TestResult {
        if resource_presence(self.id(), self.id())? != ResourcePresence::Owned {
            return Err("refusing host-state mutation without exact owned container".into());
        }
        let child = Command::new("container")
            .args(["stop", "--time", "5", self.id()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let output = wait_with_output_bounded(child, std::time::Duration::from_secs(15))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "owned host-state stop failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into())
        }
    }

    pub fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.gascan);
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.runtime_root)
            .env("GASCAN_STATE_PATH", self.state_path())
            .env("GASCAN_PID_PATH", self.runtime_root.join("daemon.pid"))
            .env(
                "GASCAN_DAEMON_INSTANCE_PATH",
                self.runtime_root.join("daemon-instance.json"),
            )
            .env("GASCAN_DAEMON_OWNER_TOKEN", &self.owner_token)
            .env(
                "GASCAN_DAEMON_STDERR_PATH",
                self.runtime_root.join("daemon.stderr"),
            )
            .env("GASCAN_DAEMON", &self.gascand)
            .env_remove("GASCAN_TEST_FAKE_BACKEND");
        command
    }

    pub fn invoke<I, S>(&self, args: I) -> TestResult<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let child = self
            .command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        wait_with_output_bounded(child, std::time::Duration::from_secs(90))
    }

    pub fn success<I, S>(&self, args: I) -> TestResult<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.invoke(args)?;
        if !output.status.success() {
            let daemon_stderr = std::fs::read_to_string(self.runtime_root.join("daemon.stderr"))
                .unwrap_or_else(|error| format!("<unavailable: {error}>"));
            let daemon_pid = std::fs::read_to_string(self.runtime_root.join("daemon.pid"))
                .unwrap_or_else(|error| format!("<unavailable: {error}>"));
            let daemon_alive = Command::new("kill")
                .args(["-0", daemon_pid.trim()])
                .status()
                .is_ok_and(|status| status.success());
            let socket = self.runtime_root.join("gascan/gascand.sock");
            let raw_socket_connects = std::os::unix::net::UnixStream::connect(&socket).is_ok();
            return Err(format!(
                "gascan failed with {:?}: stdout={} stderr={} daemon_pid={} daemon_alive={} socket={} raw_socket_connects={} daemon_stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                daemon_pid.trim(),
                daemon_alive,
                socket.display(),
                raw_socket_connects,
                daemon_stderr
            )
            .into());
        }
        Ok(output)
    }

    pub fn status_json(&self) -> TestResult<Value> {
        let output = self.success(["--sandbox", self.id(), "status", "--json"])?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    pub fn kill_daemon(&self) -> TestResult {
        let pid = self.validated_daemon_pid()?.pid;
        let pid =
            rustix::process::Pid::from_raw(i32::try_from(pid)?).ok_or("invalid daemon pid")?;
        rustix::process::kill_process(pid, rustix::process::Signal::KILL)?;
        let socket = self.runtime_root.join("gascan/gascand.sock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            if std::time::Instant::now() >= deadline {
                return Err("killed daemon retained a live socket".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        Ok(())
    }

    fn daemon_attestation(&self) -> TestResult<DaemonAttestation> {
        let output = self.invoke(["daemon-attest"])?;
        if !output.status.success() {
            return Err("daemon attestation endpoint is unavailable".into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn validated_daemon_pid(&self) -> TestResult<DaemonInstanceRecord> {
        let record = self.daemon_instance_record()?;
        let attestation = self.daemon_attestation()?;
        let expected_executable = std::path::PathBuf::from(&self.gascand).canonicalize()?;
        let observed_start = process_field(record.pid, "lstart=")?;
        let observed_command = process_field(record.pid, "command=")?;
        let observed_executable = observed_command
            .split_whitespace()
            .next()
            .ok_or("daemon command is empty")?;
        let observed_executable = std::path::Path::new(observed_executable).canonicalize()?;
        if !instance_matches(
            &record,
            &self.owner_token,
            &expected_executable,
            &observed_executable,
            &observed_start,
            &attestation,
        ) {
            return Err("daemon instance ownership validation refused signal".into());
        }
        Ok(record)
    }

    fn daemon_instance_record(&self) -> TestResult<DaemonInstanceRecord> {
        Ok(serde_json::from_slice(&std::fs::read(
            self.runtime_root.join("daemon-instance.json"),
        )?)?)
    }

    pub fn run_pty(&self, argv: &[&str]) -> TestResult<Output> {
        let pty = rustix_openpty::openpty(None, None)?;
        let stdin = std::fs::File::from(rustix::io::dup(&pty.user)?);
        let stdout = std::fs::File::from(rustix::io::dup(&pty.user)?);
        let mut args = vec!["--sandbox", self.id(), "shell", "--"];
        args.extend(argv);
        let child = self
            .command(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()?;
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(pty.controller);
        wait_with_output_bounded(child, std::time::Duration::from_secs(30))
    }

    pub fn run_pty_resize(
        &self,
        argv: &[&str],
        rows: u16,
        cols: u16,
    ) -> TestResult<PtySignalOutput> {
        let mut args = vec!["--sandbox", self.id(), "shell", "--"];
        args.extend(argv);
        let command = self.command(args);
        run_pty_resize_command(command, b"GASCAN_RESIZE_READY", rows, cols)
    }

    pub fn run_pty_signal(
        &self,
        signal: rustix::process::Signal,
        argv: &[&str],
    ) -> TestResult<PtySignalOutput> {
        let mut args = vec!["--sandbox", self.id(), "shell", "--"];
        args.extend(argv);
        run_pty_signal_command(self.command(args), b"GASCAN_SIGNAL_READY", signal)
    }

    pub fn assert_no_owned_resources(&self) -> TestResult {
        for identity in self.resource_identities()? {
            let name = identity.name();
            match resource_presence(name, self.id())? {
                ResourcePresence::Absent => {}
                ResourcePresence::Owned => {
                    return Err(format!("owned Gate 4 resource remains: {name}").into());
                }
                ResourcePresence::Collision => {
                    return Err(format!("exact resource name has foreign ownership: {name}").into());
                }
            }
        }
        Ok(())
    }

    fn resource_identities(&self) -> TestResult<Vec<gascan_core::runtime::ResourceIdentity>> {
        Ok(gascan_core::policy::PolicyCompiler::expected_resource_identities(&self.id)?)
    }

    fn cleanup(&self) -> TestResult {
        for identity in self.resource_identities()? {
            let name = identity.name();
            if resource_presence(name, self.id())
                .is_ok_and(|presence| presence == ResourcePresence::Owned)
            {
                if identity.kind() == gascan_core::runtime::ResourceKind::Container {
                    let _ = Command::new("container")
                        .args(["stop", "--time", "5", name])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                let mut command = Command::new("container");
                if identity.kind() == gascan_core::runtime::ResourceKind::Container {
                    command.args(["delete", name]);
                } else {
                    command.args(["volume", "delete", name]);
                }
                let status = command
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?;
                if !status.success() {
                    return Err(format!("cleanup failed for exact resource {name}").into());
                }
            }
        }
        let _keep_roots_alive = (&self.root, &self.runtime);
        self.assert_no_owned_resources()
    }
}

fn instance_matches(
    record: &DaemonInstanceRecord,
    owner_token: &str,
    expected_executable: &std::path::Path,
    observed_executable: &std::path::Path,
    observed_start: &str,
    attestation: &DaemonAttestation,
) -> bool {
    record.pid > 0
        && record.owner_token == owner_token
        && record.executable == expected_executable
        && observed_executable == expected_executable
        && record.start_identity == observed_start
        && record.instance_token == attestation.instance_token
        && record.pid == attestation.pid
        && record.executable == attestation.executable
        && record.start_identity == attestation.start_identity
}

impl Drop for AppleE2e {
    fn drop(&mut self) {
        let termination = self.terminate_daemon();
        let cleanup = self.cleanup();
        if let Err(error) = &termination {
            eprintln!("Gate 4 daemon cleanup failed: {error}");
        }
        if let Err(error) = &cleanup {
            eprintln!("Gate 4 Rust cleanup failed: {error}");
        }
        if termination.is_err() || cleanup.is_err() {
            if let Some(runtime) = self.runtime.take() {
                let _ = runtime.keep();
            }
            if let Some(root) = self.root.take() {
                let _ = root.keep();
            }
        }
    }
}

impl AppleE2e {
    fn terminate_daemon(&self) -> TestResult {
        let instance_path = self.runtime_root.join("daemon-instance.json");
        if !instance_path.try_exists()? {
            return Ok(());
        }
        let recorded = self.daemon_instance_record()?;
        let recorded_pid = rustix::process::Pid::from_raw(i32::try_from(recorded.pid)?)
            .ok_or("invalid daemon pid")?;
        match rustix::process::test_kill_process(recorded_pid) {
            Ok(()) => {}
            Err(rustix::io::Errno::SRCH) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let record = self.validated_daemon_pid()?;
        let pid = rustix::process::Pid::from_raw(i32::try_from(record.pid)?)
            .ok_or("invalid daemon pid")?;
        rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
        if wait_for_process_identity_exit(
            record.pid,
            &record.start_identity,
            std::time::Duration::from_secs(5),
        )? {
            return Ok(());
        }
        let current = self.validated_daemon_pid()?;
        if current.instance_token != record.instance_token {
            return Err("daemon instance changed before KILL".into());
        }
        rustix::process::kill_process(pid, rustix::process::Signal::KILL)?;
        if wait_for_process_identity_exit(
            record.pid,
            &record.start_identity,
            std::time::Duration::from_secs(5),
        )? {
            Ok(())
        } else {
            Err("validated daemon survived TERM and KILL".into())
        }
    }
}

fn wait_for_process_identity_exit(
    pid: u32,
    start: &str,
    timeout: std::time::Duration,
) -> TestResult<bool> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if process_identity_has_exited_with(
            start,
            || process_field(pid, "lstart="),
            || {
                let raw_pid = i32::try_from(pid).map_err(|_| rustix::io::Errno::INVAL)?;
                let pid =
                    rustix::process::Pid::from_raw(raw_pid).ok_or(rustix::io::Errno::INVAL)?;
                rustix::process::test_kill_process(pid)
            },
        )? {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn process_identity_has_exited_with(
    expected_start: &str,
    inspect: impl FnOnce() -> TestResult<String>,
    probe: impl FnOnce() -> Result<(), rustix::io::Errno>,
) -> TestResult<bool> {
    match inspect() {
        Ok(observed_start) => Ok(observed_start != expected_start),
        Err(inspect_error) => match probe() {
            Err(rustix::io::Errno::SRCH) => Ok(true),
            Ok(()) => Err(inspect_error),
            Err(probe_error) => Err(format!(
                "process identity inspection failed: {inspect_error}; process existence probe failed: {probe_error}"
            )
            .into()),
        },
    }
}

fn process_field(pid: u32, field: &str) -> TestResult<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .output()?;
    if !output.status.success() {
        return Err("daemon process identity is unavailable".into());
    }
    let value = String::from_utf8(output.stdout)?.trim().to_owned();
    if value.is_empty() {
        Err("daemon process identity is empty".into())
    } else {
        Ok(value)
    }
}

fn wait_with_output_bounded(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> TestResult<Output> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            let output = child.wait_with_output()?;
            return Err(format!(
                "child exceeded {timeout:?} and was killed/reaped: stderr={}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn run_pty_signal_command(
    command: Command,
    ready_marker: &[u8],
    signal: rustix_openpty::rustix::process::Signal,
) -> TestResult<PtySignalOutput> {
    run_pty_signal_command_after_spawn(command, ready_marker, signal, |_| Ok(()))
}

fn run_pty_signal_command_after_spawn(
    command: Command,
    ready_marker: &[u8],
    signal: rustix_openpty::rustix::process::Signal,
    after_spawn: impl FnOnce(u32) -> TestResult,
) -> TestResult<PtySignalOutput> {
    run_pty_signal_command_with_actions(
        command,
        ready_marker,
        signal,
        after_spawn,
        |child| Ok(child.try_wait()?),
        std::process::Child::kill,
    )
}

fn run_pty_signal_command_with_actions(
    mut command: Command,
    ready_marker: &[u8],
    signal: rustix_openpty::rustix::process::Signal,
    after_spawn: impl FnOnce(u32) -> TestResult,
    mut poll: impl FnMut(&mut std::process::Child) -> TestResult<Option<ExitStatus>>,
    mut kill: impl FnMut(&mut std::process::Child) -> std::io::Result<()>,
) -> TestResult<PtySignalOutput> {
    let pty = rustix_openpty::openpty(None, None)?;
    let stdin = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let stdout = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let stderr = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let mut controller = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.controller)?);
    let flags = rustix_openpty::rustix::fs::fcntl_getfl(&controller)?;
    rustix_openpty::rustix::fs::fcntl_setfl(
        &controller,
        flags | rustix_openpty::rustix::fs::OFlags::NONBLOCK,
    )?;
    let mut captured = Vec::new();
    let mut child = command.stdin(stdin).stdout(stdout).stderr(stderr).spawn()?;
    drop(pty.user);
    drop(pty.controller);

    let result = (|| -> TestResult<PtySignalOutput> {
        after_spawn(child.id())?;
        let started = std::time::Instant::now();
        let readiness_deadline = started + std::time::Duration::from_secs(5);
        let execution_deadline = started + std::time::Duration::from_secs(10);
        let mut signaled = false;
        let status = loop {
            let read_bytes = read_available_pty_batch(&mut controller, &mut captured, false)?;

            if let Some(status) = poll(&mut child)? {
                if !signaled {
                    return Err("PTY child exited before signal readiness".into());
                }
                break status;
            }

            if !signaled
                && captured
                    .windows(ready_marker.len())
                    .any(|window| window == ready_marker)
            {
                let pid =
                    rustix_openpty::rustix::process::Pid::from_raw(i32::try_from(child.id())?)
                        .ok_or("invalid CLI pid")?;
                rustix_openpty::rustix::process::kill_process(pid, signal)?;
                signaled = true;
            }

            let now = std::time::Instant::now();
            if !signaled && now >= readiness_deadline {
                return Err("PTY child did not report signal readiness".into());
            }
            if now >= execution_deadline {
                return Err("PTY child did not exit after signal".into());
            }
            if read_bytes == 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };

        drain_pty_after_exit(&mut controller, &mut captured)?;
        Ok(PtySignalOutput {
            status,
            stdout: captured,
            stderr: Vec::new(),
        })
    })();

    match result {
        Ok(output) => Ok(output),
        Err(error) => return_after_cleanup(
            error,
            kill_and_reap_pty_child(&mut child, &mut poll, &mut kill),
        ),
    }
}

fn run_pty_resize_command(
    command: Command,
    ready_marker: &[u8],
    rows: u16,
    cols: u16,
) -> TestResult<PtySignalOutput> {
    run_pty_resize_command_after_spawn(command, ready_marker, rows, cols, |_| Ok(()))
}

fn run_pty_resize_command_after_spawn(
    command: Command,
    ready_marker: &[u8],
    rows: u16,
    cols: u16,
    after_spawn: impl FnOnce(u32) -> TestResult,
) -> TestResult<PtySignalOutput> {
    run_pty_resize_command_with_actions(
        command,
        ready_marker,
        rows,
        cols,
        after_spawn,
        |child| Ok(child.try_wait()?),
        std::process::Child::kill,
    )
}

fn run_pty_resize_command_with_actions(
    mut command: Command,
    ready_marker: &[u8],
    rows: u16,
    cols: u16,
    after_spawn: impl FnOnce(u32) -> TestResult,
    mut poll: impl FnMut(&mut std::process::Child) -> TestResult<Option<ExitStatus>>,
    mut kill: impl FnMut(&mut std::process::Child) -> std::io::Result<()>,
) -> TestResult<PtySignalOutput> {
    let pty = rustix_openpty::openpty(
        None,
        Some(&rustix_openpty::rustix::termios::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
    )?;
    let stdin = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let stdout = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let stderr = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.user)?);
    let mut controller = std::fs::File::from(rustix_openpty::rustix::io::dup(&pty.controller)?);
    let flags = rustix_openpty::rustix::fs::fcntl_getfl(&controller)?;
    rustix_openpty::rustix::fs::fcntl_setfl(
        &controller,
        flags | rustix_openpty::rustix::fs::OFlags::NONBLOCK,
    )?;
    let mut captured = Vec::new();
    let mut child = command.stdin(stdin).stdout(stdout).stderr(stderr).spawn()?;
    let result = (|| -> TestResult<PtySignalOutput> {
        after_spawn(child.id())?;
        drop(pty.user);
        let started = std::time::Instant::now();
        let readiness_deadline = started + std::time::Duration::from_secs(5);
        let execution_deadline = started + std::time::Duration::from_secs(15);
        let mut resized = false;
        let status = loop {
            let read_bytes = read_available_pty_batch(&mut controller, &mut captured, false)?;

            if let Some(status) = poll(&mut child)? {
                if !resized {
                    return Err("PTY child exited before resize readiness".into());
                }
                break status;
            }

            if !resized
                && captured
                    .windows(ready_marker.len())
                    .any(|window| window == ready_marker)
            {
                rustix_openpty::rustix::termios::tcsetwinsize(
                    &controller,
                    rustix_openpty::rustix::termios::Winsize {
                        ws_row: rows,
                        ws_col: cols,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    },
                )?;
                let pid =
                    rustix_openpty::rustix::process::Pid::from_raw(i32::try_from(child.id())?)
                        .ok_or("invalid CLI pid")?;
                rustix_openpty::rustix::process::kill_process(
                    pid,
                    rustix_openpty::rustix::process::Signal::WINCH,
                )?;
                resized = true;
            }

            let now = std::time::Instant::now();
            if !resized && now >= readiness_deadline {
                return Err("PTY child did not report resize readiness".into());
            }
            if now >= execution_deadline {
                return Err("PTY child did not exit after resize".into());
            }
            if read_bytes == 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };

        drain_pty_after_exit(&mut controller, &mut captured)?;
        Ok(PtySignalOutput {
            status,
            stdout: captured,
            stderr: Vec::new(),
        })
    })();

    match result {
        Ok(output) => Ok(output),
        Err(error) => return_after_cleanup(
            error,
            kill_and_reap_pty_child(&mut child, &mut poll, &mut kill),
        ),
    }
}

fn return_after_cleanup<T>(
    original: Box<dyn std::error::Error>,
    cleanup: TestResult,
) -> TestResult<T> {
    match cleanup {
        Ok(()) => Err(original),
        Err(cleanup) => {
            Err(format!("{original}; additionally failed to kill and reap child: {cleanup}").into())
        }
    }
}

fn kill_and_reap_pty_child(
    child: &mut std::process::Child,
    poll: &mut impl FnMut(&mut std::process::Child) -> TestResult<Option<ExitStatus>>,
    kill: &mut impl FnMut(&mut std::process::Child) -> std::io::Result<()>,
) -> TestResult {
    let mut poll_error = match poll(child) {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => None,
        Err(error) => Some(error.to_string()),
    };
    let kill_error = kill(child).err();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    loop {
        match poll(child) {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(error) => {
                poll_error = Some(error.to_string());
            }
        }
        if std::time::Instant::now() >= deadline {
            let kill_context = kill_error
                .as_ref()
                .map(|error| format!("kill failed: {error}; "))
                .unwrap_or_default();
            let poll_context = poll_error
                .as_ref()
                .map(|error| format!("cleanup poll failed: {error}; "))
                .unwrap_or_default();
            return Err(format!(
                "{kill_context}{poll_context}child was not reaped before cleanup deadline"
            )
            .into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn drain_pty_after_exit(
    controller: &mut std::fs::File,
    captured: &mut Vec<u8>,
) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    let mut quiet_deadline = std::time::Instant::now() + std::time::Duration::from_millis(30);
    loop {
        let read_any = read_available_pty_batch(controller, captured, true)? > 0;
        let now = std::time::Instant::now();
        if read_any {
            quiet_deadline = now + std::time::Duration::from_millis(30);
        }
        if now >= deadline || now >= quiet_deadline {
            return Ok(());
        }
        if !read_any {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn read_available_pty_batch(
    controller: &mut std::fs::File,
    captured: &mut Vec<u8>,
    discard_overflow: bool,
) -> std::io::Result<usize> {
    const READ_BUFFER_BYTES: usize = 16 * 1024;
    const READ_BUDGET_BYTES: usize = 256 * 1024;
    const CAPTURE_LIMIT_BYTES: usize = 8 * 1024 * 1024;

    let mut chunk = [0_u8; READ_BUFFER_BYTES];
    let mut batch_bytes = 0;
    while batch_bytes < READ_BUDGET_BYTES {
        let capture_remaining = CAPTURE_LIMIT_BYTES.saturating_sub(captured.len());
        if capture_remaining == 0 && !discard_overflow {
            return Err(std::io::Error::other(format!(
                "PTY output exceeded {CAPTURE_LIMIT_BYTES} byte capture limit"
            )));
        }
        let read_limit = chunk.len().min(READ_BUDGET_BYTES - batch_bytes);
        match controller.read(&mut chunk[..read_limit]) {
            Ok(0) => break,
            Ok(count) => {
                let captured_count = count.min(capture_remaining);
                captured.extend_from_slice(&chunk[..captured_count]);
                batch_bytes += count;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.raw_os_error() == Some(5) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(batch_bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResourcePresence {
    Absent,
    Owned,
    Collision,
}

fn resource_presence(name: &str, id: &str) -> TestResult<ResourcePresence> {
    let output = if name == id {
        Command::new("container").args(["inspect", name]).output()?
    } else {
        Command::new("container")
            .args(["volume", "inspect", name])
            .output()?
    };
    if !output.status.success() {
        return Ok(ResourcePresence::Absent);
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let record = value
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(&value);
    let labels = &record["configuration"]["labels"];
    Ok(
        if labels["dev.gascan.managed-by"] == "gascan" && labels["dev.gascan.sandbox-id"] == id {
            ResourcePresence::Owned
        } else {
            ResourcePresence::Collision
        },
    )
}

#[cfg(test)]
fn cleanup_resource_identities(
    cleanup: &Value,
) -> TestResult<Vec<gascan_core::runtime::ResourceIdentity>> {
    let resources = cleanup["resources"]
        .as_array()
        .ok_or("cleanup resources must be an array")?;
    resources
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let name = value
                .as_str()
                .ok_or("cleanup resource name must be a string")?;
            let kind = if index == 0 {
                gascan_core::runtime::ResourceKind::Container
            } else {
                gascan_core::runtime::ResourceKind::Volume
            };
            Ok(gascan_core::runtime::ResourceIdentity::new(kind, name)?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OwnedChildFixture {
        pid: rustix_openpty::rustix::process::Pid,
        cleaned: bool,
    }

    impl OwnedChildFixture {
        fn new(raw_pid: u32) -> TestResult<Self> {
            let pid = rustix_openpty::rustix::process::Pid::from_raw(i32::try_from(raw_pid)?)
                .ok_or("invalid fixture pid")?;
            Ok(Self {
                pid,
                cleaned: false,
            })
        }

        fn assert_owned_and_running(&self) -> TestResult {
            match rustix_openpty::rustix::process::waitpid(
                Some(self.pid),
                rustix_openpty::rustix::process::WaitOptions::NOHANG,
            )? {
                None => Ok(()),
                Some(_) => Err("fixture child exited before explicit cleanup".into()),
            }
        }

        fn kill_and_reap(&mut self) -> TestResult {
            if self.cleaned {
                return Ok(());
            }
            self.assert_owned_and_running()?;
            rustix_openpty::rustix::process::kill_process(
                self.pid,
                rustix_openpty::rustix::process::Signal::KILL,
            )?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                if rustix_openpty::rustix::process::waitpid(
                    Some(self.pid),
                    rustix_openpty::rustix::process::WaitOptions::NOHANG,
                )?
                .is_some()
                {
                    self.cleaned = true;
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    return Err("fixture child was not reaped before cleanup deadline".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    impl Drop for OwnedChildFixture {
        fn drop(&mut self) {
            let _ = self.kill_and_reap();
        }
    }

    struct IdentifiedProcessFixture {
        pid: rustix_openpty::rustix::process::Pid,
        identity: String,
        cleaned: bool,
    }

    impl IdentifiedProcessFixture {
        fn new(raw_pid: u32, identity: String) -> TestResult<Self> {
            let pid = rustix_openpty::rustix::process::Pid::from_raw(i32::try_from(raw_pid)?)
                .ok_or("invalid descendant fixture pid")?;
            let fixture = Self {
                pid,
                identity,
                cleaned: false,
            };
            fixture.assert_identity()?;
            Ok(fixture)
        }

        fn assert_identity(&self) -> TestResult {
            let command = process_field(self.pid.as_raw_nonzero().get().try_into()?, "command=")?;
            if command.contains(&self.identity) {
                Ok(())
            } else {
                Err(format!("descendant fixture identity mismatch: {command}").into())
            }
        }

        fn kill_and_confirm_absent(&mut self) -> TestResult {
            if self.cleaned {
                return Ok(());
            }
            self.assert_identity()?;
            rustix_openpty::rustix::process::kill_process(
                self.pid,
                rustix_openpty::rustix::process::Signal::KILL,
            )?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                let raw_pid: u32 = self.pid.as_raw_nonzero().get().try_into()?;
                match process_field(raw_pid, "command=") {
                    Err(_) => {
                        self.cleaned = true;
                        return Ok(());
                    }
                    Ok(command) if !command.contains(&self.identity) => {
                        return Err("descendant fixture PID was unexpectedly reused".into());
                    }
                    Ok(_) => {}
                }
                if std::time::Instant::now() >= deadline {
                    return Err("descendant fixture survived explicit cleanup".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    impl Drop for IdentifiedProcessFixture {
        fn drop(&mut self) {
            let _ = self.kill_and_confirm_absent();
        }
    }

    fn marker_pid(output: &[u8], marker: &[u8]) -> TestResult<u32> {
        let start = output
            .windows(marker.len())
            .position(|window| window == marker)
            .map(|index| index + marker.len())
            .ok_or("descendant PID marker was absent")?;
        let digits = output[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .copied()
            .collect::<Vec<_>>();
        Ok(std::str::from_utf8(&digits)?.parse()?)
    }

    #[test]
    fn corrupt_daemon_record_is_never_signalable() -> TestResult {
        let env = AppleE2e::new("corrupt-pid")?;
        std::fs::write(env.runtime_root.join("daemon-instance.json"), b"not-json")?;
        assert!(env.validated_daemon_pid().is_err());
        std::fs::remove_file(env.runtime_root.join("daemon-instance.json"))?;
        Ok(())
    }

    #[test]
    fn lifecycle_and_recovery_resources_match_production_up_identity() -> TestResult {
        for name in ["gate4-lifecycle", "gate4-recovery"] {
            let session = tempfile::tempdir()?;
            let cleanup_manifest = session.path().join(format!("{name}.json"));
            let env = AppleE2e::new_scoped(
                name,
                session.path().to_owned(),
                Some(cleanup_manifest.clone()),
            )?;
            let root = camino::Utf8Path::from_path(&env.root_path).ok_or("non-UTF-8 root")?;
            let manifest = gascan_core::manifest::Manifest::load(root)?;
            let production_name = manifest
                .name()
                .map(ToOwned::to_owned)
                .or_else(|| root.file_name().map(ToOwned::to_owned))
                .ok_or("production sandbox name is unavailable")?;
            let spec =
                gascan_core::sandbox::SandboxSpec::from_root(&production_name, root, manifest)?;
            let cleanup: Value = serde_json::from_slice(&std::fs::read(cleanup_manifest)?)?;
            let expected =
                gascan_core::policy::PolicyCompiler::expected_resource_identities(spec.id())?;
            let actual = cleanup_resource_identities(&cleanup)?;
            let capabilities = gascan_core::runtime::RuntimeCapabilities {
                version: gascan_core::runtime::RuntimeVersion::new(1, 1, 0),
                bind_mounts: true,
                named_volumes: true,
                tty: true,
                signals: true,
                loopback_publish: true,
                resource_limits: true,
                offline: gascan_core::runtime::NetworkIsolation::Proven,
            };
            let request =
                gascan_core::policy::PolicyCompiler::compile(spec.clone(), &capabilities)?;

            assert_eq!(env.id(), spec.id().as_str());
            assert_eq!(cleanup["sandbox_id"], spec.id().as_str());
            assert_eq!(actual, expected);
            assert_eq!(env.resource_identities()?, expected);
            assert_eq!(
                expected[0].kind(),
                gascan_core::runtime::ResourceKind::Container
            );
            assert_eq!(expected[0].name(), spec.id().as_str());
            assert!(
                expected
                    .iter()
                    .skip(1)
                    .all(|identity| identity.kind() == gascan_core::runtime::ResourceKind::Volume)
            );
            assert_eq!(request.id(), spec.id());
            assert_eq!(
                request.image(),
                include_str!("../../../../images/workspace/approved-image.txt")
            );
            assert_eq!(request.image().matches('@').count(), 1);
            assert!(!request.image().chars().any(char::is_whitespace));
            assert_eq!(
                request
                    .volumes()
                    .iter()
                    .map(|volume| volume.name.as_str())
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .skip(1)
                    .map(|identity| identity.name())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                request
                    .volumes()
                    .iter()
                    .map(|volume| volume.target.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    "/home/workspace/.local/share/mise",
                    "/home/workspace/.cache",
                    "/home/workspace/.config/gascan",
                ]
            );
            assert!(request.volumes().iter().all(|volume| {
                volume.ownership.sandbox_id == *spec.id() && volume.ownership.managed_by == "gascan"
            }));
        }
        Ok(())
    }

    #[test]
    fn gate4_project_and_runtime_roots_are_owner_only() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        let session = tempfile::tempdir()?;
        let env = AppleE2e::new_scoped("secure-roots", session.path().to_owned(), None)?;

        assert_eq!(
            std::fs::metadata(&env.root_path)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&env.runtime_root)?.permissions().mode() & 0o777,
            0o700
        );
        Ok(())
    }

    #[test]
    fn cleanup_accepts_an_already_exited_recorded_daemon() -> TestResult {
        let session = tempfile::tempdir()?;
        let env = AppleE2e::new_scoped("exited-daemon", session.path().to_owned(), None)?;
        let executable = std::path::PathBuf::from(&env.gascand).canonicalize()?;
        let record = serde_json::json!({
            "pid": 2_147_483_647_u32,
            "owner_token": env.owner_token,
            "executable": executable,
            "start_identity": "already-exited",
            "instance_token": "already-exited",
        });
        std::fs::write(
            env.runtime_root.join("daemon-instance.json"),
            serde_json::to_vec(&record)?,
        )?;

        env.terminate_daemon()?;
        std::fs::remove_file(env.runtime_root.join("daemon-instance.json"))?;
        Ok(())
    }

    #[test]
    fn cleanup_propagates_instance_record_metadata_errors() -> TestResult {
        use std::os::unix::fs::symlink;

        let session = tempfile::tempdir()?;
        let env = AppleE2e::new_scoped("instance-metadata-error", session.path().to_owned(), None)?;
        let instance = env.runtime_root.join("daemon-instance.json");
        symlink("daemon-instance.json", &instance)?;

        let result = env.terminate_daemon();
        std::fs::remove_file(instance)?;
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn process_identity_inspection_failure_requires_esrch_to_count_as_exit() -> TestResult {
        let live_error = process_identity_has_exited_with(
            "recorded-start",
            || Err("forced identity inspection failure".into()),
            || Ok(()),
        );
        let live_error = match live_error {
            Ok(_) => return Err("live process inspection failure counted as exit".into()),
            Err(error) => error,
        };
        assert!(
            live_error
                .to_string()
                .contains("forced identity inspection failure")
        );

        assert!(process_identity_has_exited_with(
            "recorded-start",
            || Err("process disappeared during inspection".into()),
            || Err(rustix::io::Errno::SRCH),
        )?);
        Ok(())
    }

    #[test]
    fn changed_start_identity_counts_as_exit_without_probing_reused_pid() -> TestResult {
        let probed = std::cell::Cell::new(false);
        assert!(process_identity_has_exited_with(
            "recorded-start",
            || Ok("reused-process-start".to_owned()),
            || {
                probed.set(true);
                Ok(())
            },
        )?);
        assert!(!probed.get());
        Ok(())
    }

    #[test]
    fn cleanup_refuses_a_reused_live_pid_without_signaling_it() -> TestResult {
        let session = tempfile::tempdir()?;
        let env = AppleE2e::new_scoped("reused-live-daemon", session.path().to_owned(), None)?;
        let child = Command::new("sleep").arg("30").spawn()?;
        let mut fixture = OwnedChildFixture::new(child.id())?;
        drop(child);
        let executable = std::path::PathBuf::from(&env.gascand).canonicalize()?;
        let record = serde_json::json!({
            "pid": fixture.pid.as_raw_nonzero().get(),
            "owner_token": env.owner_token,
            "executable": executable,
            "start_identity": "not-the-live-process-start",
            "instance_token": "not-the-live-process-instance",
        });
        std::fs::write(
            env.runtime_root.join("daemon-instance.json"),
            serde_json::to_vec(&record)?,
        )?;

        assert!(env.terminate_daemon().is_err());
        fixture.assert_owned_and_running()?;
        fixture.kill_and_reap()?;
        std::fs::remove_file(env.runtime_root.join("daemon-instance.json"))?;
        Ok(())
    }

    #[test]
    fn exact_exit_failure_includes_stdout_and_stderr() -> TestResult {
        use std::os::unix::process::ExitStatusExt as _;

        let output = Output {
            status: ExitStatus::from_raw(70 << 8),
            stdout: b"sandbox-not-found".to_vec(),
            stderr: b"lookup failed".to_vec(),
        };
        let error = match assert_exit_code(&output, 42) {
            Ok(()) => return Err("wrong exit code was accepted".into()),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains("expected 42"));
        assert!(message.contains("sandbox-not-found"));
        assert!(message.contains("lookup failed"));
        Ok(())
    }

    #[test]
    fn reused_live_pid_without_owner_token_is_never_signalable() -> TestResult {
        let env = AppleE2e::new("reused-pid")?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let record = serde_json::json!({
            "pid": std::process::id(),
            "owner_token": "somebody-else",
            "executable": executable,
            "start_identity": "deliberately-reused-start",
            "instance_token": "deliberately-reused-instance",
        });
        std::fs::write(
            env.runtime_root.join("daemon-instance.json"),
            serde_json::to_vec(&record)?,
        )?;
        assert!(env.validated_daemon_pid().is_err());
        std::fs::remove_file(env.runtime_root.join("daemon-instance.json"))?;
        Ok(())
    }

    #[test]
    fn bounded_wait_kills_and_reaps_timed_out_child() -> TestResult {
        let child = Command::new("sh")
            .args(["-c", "sleep 10"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let started = std::time::Instant::now();
        assert!(wait_with_output_bounded(child, std::time::Duration::from_millis(20)).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        Ok(())
    }

    #[test]
    fn pty_resize_driver_delivers_exact_dimensions_to_child() -> TestResult {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "initial=$(stty size); printf '%s\\n' \"$initial\"; test \"$initial\" = '24 80' || exit 1; trap 'size=$(stty size); printf \"%s\\n\" \"$size\"; test \"$size\" = \"47 132\" && exit 0' WINCH; printf GASCAN_RESIZE_READY; while :; do sleep 1; done",
        ]);

        let output = run_pty_resize_command(command, b"GASCAN_RESIZE_READY", 47, 132)?;

        assert!(output.status.success());
        assert!(
            output
                .stdout
                .windows(b"24 80".len())
                .any(|window| window == b"24 80")
        );
        assert!(
            output
                .stdout
                .windows(b"47 132".len())
                .any(|window| window == b"47 132")
        );
        Ok(())
    }

    #[test]
    fn pty_resize_driver_drains_chatty_child_without_backpressure_timeout() -> TestResult {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "trap 'exit 0' WINCH; dd if=/dev/zero bs=4096 count=64 2>/dev/null; printf GASCAN_RESIZE_READY; while :; do :; done",
        ]);

        let started = std::time::Instant::now();
        let output = run_pty_resize_command(command, b"GASCAN_RESIZE_READY", 47, 132)?;

        assert!(output.status.success());
        assert_eq!(
            output.stdout.len(),
            64 * 4096 + b"GASCAN_RESIZE_READY".len()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "chatty resize child was throttled for {:?}",
            started.elapsed()
        );
        Ok(())
    }

    #[test]
    fn pty_resize_driver_does_not_wait_for_descendant_pty_eof() -> TestResult {
        let identity = format!(
            "gascan-descendant-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!(
                "trap 'exit 0' WINCH; sh -c 'while :; do :; done' {identity} & descendant=$!; printf 'GASCAN_DESCENDANT_PID=%s\\n' \"$descendant\"; printf GASCAN_RESIZE_READY; while :; do :; done"
            ),
        ]);

        let started = std::time::Instant::now();
        let output = run_pty_resize_command(command, b"GASCAN_RESIZE_READY", 47, 132)?;
        let descendant_pid = marker_pid(&output.stdout, b"GASCAN_DESCENDANT_PID=")?;
        let mut descendant = IdentifiedProcessFixture::new(descendant_pid, identity)?;

        assert!(output.status.success());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "resize helper waited {:?} for descendant-owned PTY descriptors",
            started.elapsed()
        );
        descendant.kill_and_confirm_absent()?;
        assert!(process_field(descendant_pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn pty_resize_cleanup_kill_failure_is_bounded_and_preserves_context() -> TestResult {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let spawned_pid = std::cell::Cell::new(None);

        let started = std::time::Instant::now();
        let result = run_pty_resize_command_with_actions(
            command,
            b"GASCAN_RESIZE_READY",
            47,
            132,
            |pid| {
                spawned_pid.set(Some(pid));
                Err("forced original failure".into())
            },
            |child| Ok(child.try_wait()?),
            |_| Err(std::io::Error::other("forced kill failure")),
        );
        let elapsed = started.elapsed();
        let pid = spawned_pid.get().ok_or("post-spawn hook did not run")?;
        let mut fixture = OwnedChildFixture::new(pid)?;
        fixture.assert_owned_and_running()?;
        let error = match result {
            Ok(_) => return Err("forced cleanup failure was not returned".into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("forced original failure"));
        assert!(error.to_string().contains("forced kill failure"));
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "resize cleanup blocked for {elapsed:?}"
        );
        fixture.kill_and_reap()?;
        assert!(process_field(pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn pty_resize_driver_reaps_child_after_post_spawn_failure() -> TestResult {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let spawned_pid = std::cell::Cell::new(None);

        let result =
            run_pty_resize_command_after_spawn(command, b"GASCAN_RESIZE_READY", 47, 132, |pid| {
                spawned_pid.set(Some(pid));
                Err("forced post-spawn failure".into())
            });
        let error = match result {
            Ok(_) => return Err("forced post-spawn failure was not returned".into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("forced post-spawn failure"));
        let pid = spawned_pid.get().ok_or("post-spawn hook did not run")?;
        assert!(process_field(pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn pty_resize_driver_reaps_child_after_poll_failure() -> TestResult {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let spawned_pid = std::cell::Cell::new(None);
        let polls = std::cell::Cell::new(0_u8);

        let result = run_pty_resize_command_with_actions(
            command,
            b"GASCAN_RESIZE_READY",
            47,
            132,
            |pid| {
                spawned_pid.set(Some(pid));
                Ok(())
            },
            |child| {
                polls.set(polls.get() + 1);
                if polls.get() == 1 {
                    Err("forced PTY poll failure".into())
                } else {
                    Ok(child.try_wait()?)
                }
            },
            std::process::Child::kill,
        );
        let error = match result {
            Ok(_) => return Err("forced PTY poll failure was not returned".into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("forced PTY poll failure"));
        let pid = spawned_pid.get().ok_or("post-spawn hook did not run")?;
        assert!(process_field(pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn pty_signal_driver_does_not_wait_for_inherited_slave_descriptor() -> TestResult {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "trap 'exit 130' INT; sleep 0.5 & descendant=$!; printf 'GASCAN_DESCENDANT_PID=%s\n' \"$descendant\"; printf GASCAN_SIGNAL_READY; while :; do :; done",
        ]);

        let started = std::time::Instant::now();
        let output = run_pty_signal_command(
            command,
            b"GASCAN_SIGNAL_READY",
            rustix_openpty::rustix::process::Signal::INT,
        )?;
        let descendant_pid = marker_pid(&output.stdout, b"GASCAN_DESCENDANT_PID=")?;

        assert_eq!(output.status.code(), Some(130));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "signal helper waited {:?} for an inherited PTY slave descriptor",
            started.elapsed()
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while process_field(descendant_pid, "stat=").is_ok() {
            if std::time::Instant::now() >= deadline {
                return Err("bounded inherited-descriptor fixture did not exit".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(process_field(descendant_pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn pty_signal_driver_reaps_child_after_post_spawn_failure() -> TestResult {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let spawned_pid = std::cell::Cell::new(None);

        let result = run_pty_signal_command_after_spawn(
            command,
            b"GASCAN_SIGNAL_READY",
            rustix_openpty::rustix::process::Signal::INT,
            |pid| {
                spawned_pid.set(Some(pid));
                Err("forced signal post-spawn failure".into())
            },
        );
        let error = match result {
            Ok(_) => return Err("forced signal post-spawn failure was not returned".into()),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("forced signal post-spawn failure")
        );
        let pid = spawned_pid.get().ok_or("post-spawn hook did not run")?;
        assert!(process_field(pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn pty_signal_cleanup_kill_failure_is_bounded_and_preserves_context() -> TestResult {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let spawned_pid = std::cell::Cell::new(None);

        let started = std::time::Instant::now();
        let result = run_pty_signal_command_with_actions(
            command,
            b"GASCAN_SIGNAL_READY",
            rustix_openpty::rustix::process::Signal::INT,
            |pid| {
                spawned_pid.set(Some(pid));
                Err("forced signal original failure".into())
            },
            |child| Ok(child.try_wait()?),
            |_| Err(std::io::Error::other("forced signal kill failure")),
        );
        let elapsed = started.elapsed();
        let pid = spawned_pid.get().ok_or("post-spawn hook did not run")?;
        let mut fixture = OwnedChildFixture::new(pid)?;
        fixture.assert_owned_and_running()?;
        let error = match result {
            Ok(_) => return Err("forced signal cleanup failure was not returned".into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("forced signal original failure"));
        assert!(error.to_string().contains("forced signal kill failure"));
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "signal cleanup blocked for {elapsed:?}"
        );
        fixture.kill_and_reap()?;
        assert!(process_field(pid, "stat=").is_err());
        Ok(())
    }

    #[test]
    fn exact_instance_validation_rejects_pid_reuse_prefix_and_socket_mismatch() {
        let executable = std::path::PathBuf::from("/tmp/gascand");
        let record = DaemonInstanceRecord {
            pid: 42,
            owner_token: "owner".into(),
            executable: executable.clone(),
            start_identity: "start-a".into(),
            instance_token: "instance-a".into(),
        };
        let attestation = DaemonAttestation {
            pid: 42,
            executable: executable.clone(),
            start_identity: "start-a".into(),
            instance_token: "instance-a".into(),
        };
        assert!(instance_matches(
            &record,
            "owner",
            &executable,
            &executable,
            "start-a",
            &attestation
        ));
        assert!(!instance_matches(
            &record,
            "somebody-else",
            &executable,
            &executable,
            "start-a",
            &attestation
        ));
        assert!(!instance_matches(
            &record,
            "owner",
            &executable,
            &executable,
            "start-b",
            &attestation
        ));
        assert!(!instance_matches(
            &record,
            "owner",
            &executable,
            std::path::Path::new("/tmp/gascand-evil"),
            "start-a",
            &attestation
        ));
        let wrong_socket = DaemonAttestation {
            instance_token: "instance-b".into(),
            ..attestation
        };
        assert!(!instance_matches(
            &record,
            "owner",
            &executable,
            &executable,
            "start-a",
            &wrong_socket
        ));
    }

    #[test]
    fn failed_attestation_preserves_runtime_project_and_manifest_evidence() -> TestResult {
        let cleanup = tempfile::tempdir()?;
        let session = cleanup.path().join("session-persist");
        std::fs::create_dir(&session)?;
        std::fs::set_permissions(
            &session,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )?;
        let manifest = cleanup.path().join("persist.json");
        let env = AppleE2e::new_scoped("persist", session, Some(manifest.clone()))?;
        let runtime = env.runtime_root.clone();
        let project = env.root_path.clone();
        std::fs::write(runtime.join("daemon-instance.json"), b"forged")?;
        drop(env);
        assert!(runtime.exists());
        assert!(project.exists());
        assert!(manifest.exists());
        std::fs::remove_dir_all(runtime)?;
        std::fs::remove_dir_all(project)?;
        Ok(())
    }
}
