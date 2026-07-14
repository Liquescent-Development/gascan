#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::process::Command;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Environment {
    gascan: std::ffi::OsString,
    gascand: std::ffi::OsString,
    runtime: tempfile::TempDir,
    runtime_root: std::path::PathBuf,
}

impl Environment {
    fn new() -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
        let gascand =
            std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
        let runtime = tempfile::tempdir()?;
        let runtime_root = runtime.path().canonicalize()?;
        Ok(Self {
            gascan,
            gascand,
            runtime,
            runtime_root,
        })
    }

    fn invoke(&self) -> Result<std::process::Output, std::io::Error> {
        Command::new(&self.gascan)
            .args(["doctor", "--json"])
            .env("XDG_RUNTIME_DIR", &self.runtime_root)
            .env("GASCAN_STATE_PATH", self.runtime_root.join("state.sqlite3"))
            .env(
                "GASCAN_FAKE_STATE_PATH",
                self.runtime_root.join("runtime.json"),
            )
            .env("GASCAN_PID_PATH", self.runtime_root.join("daemon.pid"))
            .env("GASCAN_DAEMON", &self.gascand)
            .output()
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
    let left = left.join().map_err(|_| "left thread panicked")??;
    let right = right.join().map_err(|_| "right thread panicked")??;
    for (side, output) in [("left", left), ("right", right)] {
        assert!(
            output.status.success(),
            "{side} autostart failed: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _keep_runtime_alive = &env.runtime;
    Ok(())
}
