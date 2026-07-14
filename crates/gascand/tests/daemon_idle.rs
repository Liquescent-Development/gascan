use gascan_proto::v1::{HandshakeRequest, UpRequest, gas_can_client::GasCanClient};
use gascand::{ActivityTracker, Daemon, DaemonConfig, SocketPaths};
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;
use tower::service_fn;

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
    let paths = SocketPaths::from_runtime_root(temp.path().canonicalize()?.join("runtime"));
    let config = DaemonConfig::new(paths.clone(), Duration::from_millis(20));
    Daemon::serve_idle(config).await?;
    assert!(!paths.socket().exists());
    Ok(())
}

#[tokio::test]
async fn sigterm_stops_daemon_and_removes_owned_socket() -> TestResult {
    let temp = TempDir::new()?;
    let runtime_root = temp.path().canonicalize()?;
    let expected = runtime_root.join("gascan/gascand.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", runtime_root.join("state.sqlite3"))
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

#[tokio::test]
async fn real_uds_tonic_lifecycle_request_reaches_sandbox_service() -> TestResult {
    let temp = TempDir::new()?;
    let runtime_root = temp.path().canonicalize()?;
    let project = runtime_root.join("project");
    std::fs::create_dir(&project)?;
    let socket = runtime_root.join("gascan/gascand.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", runtime_root.join("state.sqlite3"))
        .env("GASCAN_IDLE_TIMEOUT_MS", "30000")
        .spawn()?;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let endpoint = tonic::transport::Endpoint::try_from("http://[::]:50051")?;
    let connect_path = socket.clone();
    let channel = endpoint
        .connect_with_connector(service_fn(move |_| {
            let path = connect_path.clone();
            async move {
                tokio::net::UnixStream::connect(path)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
            }
        }))
        .await?;
    let mut client = GasCanClient::new(channel);
    let handshake = client
        .handshake(HandshakeRequest {
            api_major: 1,
            api_minor: 0,
            requested_capabilities: Vec::new(),
        })
        .await?
        .into_inner();
    assert!(handshake.rejection.is_none());
    let root = project
        .to_str()
        .ok_or("project path is not UTF-8")?
        .to_owned();
    let mut events = client
        .up(UpRequest { project_root: root })
        .await?
        .into_inner();
    let mut terminal = None;
    while let Some(event) = events.message().await? {
        terminal = Some(event.status);
    }
    assert_eq!(
        terminal,
        Some(gascan_proto::v1::OperationStatus::Completed as i32)
    );
    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon has no process id")? as i32)
        .ok_or("daemon process id is zero")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );
    assert!(!socket.exists());
    Ok(())
}

#[tokio::test]
async fn sigterm_waits_for_active_durable_operation_then_closes_connection() -> TestResult {
    let temp = TempDir::new()?;
    let runtime_root = temp.path().canonicalize()?;
    let project = runtime_root.join("project");
    std::fs::create_dir(&project)?;
    let socket = runtime_root.join("gascan/gascand.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", runtime_root.join("state.sqlite3"))
        .env("GASCAN_IDLE_TIMEOUT_MS", "30000")
        .env("GASCAN_FAKE_PROVISION_DELAY_MS", "300")
        .spawn()?;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let endpoint = tonic::transport::Endpoint::try_from("http://[::]:50051")?;
    let connect_path = socket.clone();
    let channel = endpoint
        .connect_with_connector(service_fn(move |_| {
            let path = connect_path.clone();
            async move {
                tokio::net::UnixStream::connect(path)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
            }
        }))
        .await?;
    let root = project
        .to_str()
        .ok_or("project path is not UTF-8")?
        .to_owned();
    let operation = tokio::spawn(async move {
        let mut events = GasCanClient::new(channel)
            .up(UpRequest { project_root: root })
            .await?
            .into_inner();
        while events.message().await?.is_some() {}
        Ok::<(), tonic::Status>(())
    });
    tokio::time::sleep(Duration::from_millis(75)).await;
    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon has no process id")? as i32)
        .ok_or("daemon process id is zero")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(
        child.try_wait()?.is_none(),
        "daemon exited before durable operation drained"
    );
    tokio::time::timeout(Duration::from_secs(2), operation).await???;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );
    assert!(!socket.exists());
    Ok(())
}
