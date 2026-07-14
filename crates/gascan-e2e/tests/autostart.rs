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

    fn command(&self) -> Command {
        let mut command = Command::new(&self.gascan);
        command
            .args(["doctor", "--json"])
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
            .env("GASCAN_DAEMON", &self.gascand);
        command
    }

    fn invoke(&self) -> Result<std::process::Output, std::io::Error> {
        self.command().output()
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
