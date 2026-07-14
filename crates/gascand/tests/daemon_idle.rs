use gascand::{ActivityTracker, Daemon, DaemonConfig, SocketPaths};
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn leases_and_operations_hold_idle_shutdown_open() -> TestResult {
    let tracker = ActivityTracker::new();
    let lease = tracker.lease();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            tracker.wait_for_idle(Duration::from_millis(5))
        )
        .await
        .is_err()
    );
    drop(lease);
    tracker.operation_started();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            tracker.wait_for_idle(Duration::from_millis(5))
        )
        .await
        .is_err()
    );
    tracker.operation_finished();
    tokio::time::timeout(
        Duration::from_millis(50),
        tracker.wait_for_idle(Duration::from_millis(5)),
    )
    .await??;
    Ok(())
}

#[tokio::test]
async fn daemon_exits_when_idle_and_removes_only_its_socket() -> TestResult {
    let temp = TempDir::new()?;
    let paths = SocketPaths::from_runtime_root(temp.path().join("runtime"));
    let config = DaemonConfig::new(paths.clone(), Duration::from_millis(20));
    Daemon::serve_idle(config).await?;
    assert!(!paths.socket().exists());
    Ok(())
}

#[tokio::test]
async fn sigterm_stops_daemon_and_removes_owned_socket() -> TestResult {
    let temp = TempDir::new()?;
    let expected = temp.path().join("gascan/gascand.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("XDG_RUNTIME_DIR", temp.path())
        .env("GASCAN_IDLE_TIMEOUT_MS", "30000")
        .spawn()?;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !expected.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon has no process id")? as i32)
        .ok_or("daemon process id is zero")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait()).await??;
    assert!(status.success());
    assert!(!expected.exists());
    Ok(())
}
