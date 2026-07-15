#![allow(dead_code)]

use gascan_core::sandbox::SandboxId;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::process::{Command, Output, Stdio};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub struct AppleE2e {
    gascan: OsString,
    gascand: OsString,
    root: tempfile::TempDir,
    runtime: tempfile::TempDir,
    root_path: std::path::PathBuf,
    runtime_root: std::path::PathBuf,
    id: SandboxId,
}

impl AppleE2e {
    pub fn new(name: &str) -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli")
            .ok_or("workspace-built gascan binary is unavailable")?;
        let gascand = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon")
            .ok_or("workspace-built gascand binary is unavailable")?;
        let root = tempfile::Builder::new()
            .prefix("gascan-gate4-root-")
            .tempdir()?;
        let runtime = tempfile::Builder::new()
            .prefix("gascan-gate4-runtime-")
            .tempdir()?;
        let root_path = root.path().canonicalize()?;
        let runtime_root = runtime.path().canonicalize()?;
        let utf8_root = camino::Utf8Path::from_path(&root_path).ok_or("non-UTF-8 test root")?;
        let id = SandboxId::from_root(name, utf8_root);
        Ok(Self {
            gascan,
            gascand,
            root,
            runtime,
            root_path,
            runtime_root,
            id,
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
        Ok(self.command(args).output()?)
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
            return Err(format!(
                "gascan failed with {:?}: stdout={} stderr={} daemon_pid={} daemon_alive={} daemon_stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                daemon_pid.trim(),
                daemon_alive,
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
        let pid = std::fs::read_to_string(self.runtime_root.join("daemon.pid"))?
            .trim()
            .parse::<i32>()?;
        let pid = rustix::process::Pid::from_raw(pid).ok_or("invalid daemon pid")?;
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
        Ok(child.wait_with_output()?)
    }

    pub fn assert_no_owned_resources(&self) -> TestResult {
        for name in self.resource_names() {
            if owned_resource(&name, self.id())? {
                return Err(format!("owned Gate 4 resource remains: {name}").into());
            }
        }
        Ok(())
    }

    fn resource_names(&self) -> [String; 4] {
        [
            self.id().to_owned(),
            format!("gascan-mise-{}", self.id()),
            format!("gascan-cache-{}", self.id()),
            format!("gascan-config-{}", self.id()),
        ]
    }

    fn cleanup(&self) {
        for (index, name) in self.resource_names().into_iter().enumerate() {
            if owned_resource(&name, self.id()).unwrap_or(false) {
                let mut command = Command::new("container");
                if index == 0 {
                    command.args(["delete", &name]);
                } else {
                    command.args(["volume", "delete", &name]);
                }
                let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
        }
        let _keep_roots_alive = (&self.root, &self.runtime);
    }
}

impl Drop for AppleE2e {
    fn drop(&mut self) {
        if let Ok(raw) = std::fs::read_to_string(self.runtime_root.join("daemon.pid")) {
            if let Ok(pid) = raw.trim().parse::<i32>() {
                if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                }
            }
        }
        self.cleanup();
    }
}

fn owned_resource(name: &str, id: &str) -> TestResult<bool> {
    let output = if name == id {
        Command::new("container").args(["inspect", name]).output()?
    } else {
        Command::new("container")
            .args(["volume", "inspect", name])
            .output()?
    };
    if !output.status.success() {
        return Ok(false);
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let record = value
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(&value);
    let labels = &record["configuration"]["labels"];
    Ok(labels["dev.gascan.managed-by"] == "gascan" && labels["dev.gascan.sandbox-id"] == id)
}
