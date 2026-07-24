use gascan_proto::v1::{
    ApplyRequest, HandshakeRequest, ListRequest, SandboxSelector, StatusRequest, UpRequest,
    gas_can_client::GasCanClient,
};
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
    for _ in 0..20 {
        let temp = TempDir::new()?;
        let runtime_root = temp.path().canonicalize()?;
        let expected = runtime_root.join("gascan/gascand.sock");
        let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
            .env("GASCAN_TEST_FAKE_BACKEND", "1")
            .env("XDG_RUNTIME_DIR", &runtime_root)
            .env("GASCAN_STATE_PATH", runtime_root.join("state.sqlite3"))
            .env("GASCAN_IDLE_TIMEOUT_MS", "30000")
            .spawn()?;
        tokio::time::timeout(Duration::from_secs(2), async {
            while !expected.exists() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
        let pid =
            rustix::process::Pid::from_raw(child.id().ok_or("daemon has no process id")? as i32)
                .ok_or("daemon process id is zero")?;
        rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
        let status = tokio::time::timeout(Duration::from_secs(2), child.wait()).await??;
        assert!(status.success());
        assert!(!expected.exists());
    }
    Ok(())
}

#[tokio::test]
async fn raw_liveness_probe_disconnect_does_not_end_server() -> TestResult {
    for _ in 0..20 {
        let temp = TempDir::new()?;
        let runtime_root = temp.path().canonicalize()?;
        let socket = runtime_root.join("gascan/gascand.sock");
        let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
            .env("GASCAN_TEST_FAKE_BACKEND", "1")
            .env("XDG_RUNTIME_DIR", &runtime_root)
            .env("GASCAN_STATE_PATH", runtime_root.join("state.sqlite3"))
            .env("GASCAN_IDLE_TIMEOUT_MS", "30000")
            .spawn()?;
        tokio::time::timeout(Duration::from_secs(2), async {
            while !socket.exists() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
        drop(std::os::unix::net::UnixStream::connect(&socket)?);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(child.try_wait()?.is_none());
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
        let response = GasCanClient::new(channel)
            .handshake(HandshakeRequest {
                api_major: 1,
                api_minor: 0,
                requested_capabilities: Vec::new(),
            })
            .await?
            .into_inner();
        assert!(response.rejection.is_none());
        assert_eq!(response.api_major, 1);
        assert_eq!(response.api_minor, 2);
        assert_eq!(response.daemon_instance_token.len(), 64);
        assert_eq!(
            response.daemon_pid,
            child.id().ok_or("daemon has no process id")?
        );
        assert!(!response.daemon_executable.is_empty());
        assert!(!response.daemon_start_identity.is_empty());
        let pid =
            rustix::process::Pid::from_raw(child.id().ok_or("daemon has no process id")? as i32)
                .ok_or("daemon process id is zero")?;
        rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await??
                .success()
        );
        assert!(!socket.exists());
    }
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
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
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
    let sandbox_id = client
        .list(ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("created sandbox missing from list")?
        .sandbox_id;
    let current = "registry.example/workspace:old@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let connection = rusqlite::Connection::open(runtime_root.join("state.sqlite3"))?;
    connection.execute(
        "UPDATE sandboxes SET image_resolution_version = 1, image_resolution_details = ?1",
        [serde_json::json!({"digest": current}).to_string()],
    )?;
    drop(connection);
    let status = client
        .status(StatusRequest {
            sandbox: Some(SandboxSelector { sandbox_id }),
        })
        .await?
        .into_inner()
        .sandbox
        .ok_or("status response missing sandbox")?;
    assert_eq!(status.apply_requirements.len(), 1);
    assert_eq!(status.apply_requirements[0].reason, "image_changed");
    assert_eq!(status.apply_requirements[0].current, current);
    assert_eq!(
        status.apply_requirements[0].requested,
        include_str!("../../../images/workspace/approved-image.txt")
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
async fn rejected_request_details_survive_the_real_transport() -> TestResult {
    let temp = TempDir::new()?;
    let runtime_root = temp.path().canonicalize()?;
    let socket = runtime_root.join("gascan/gascand.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
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
    let missing = runtime_root.join("no-such-project");
    let root = missing
        .to_str()
        .ok_or("project path is not UTF-8")?
        .to_owned();
    let status = client
        .up(UpRequest {
            project_root: root.clone(),
        })
        .await
        .err()
        .ok_or("a missing project root must be rejected before the operation starts")?;
    assert_eq!(
        status.message(),
        gascan_proto::error_code::INVALID_PROJECT_ROOT
    );
    let cause = gascan_proto::error_detail::decode_message(status.details())
        .ok_or("the status details must survive the real transport")?;
    assert!(
        cause.contains(&root),
        "the cause must name the rejected root: {cause}"
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
async fn storage_mismatch_is_failed_precondition_across_real_up_and_apply_clients() -> TestResult {
    let temp = TempDir::new()?;
    let runtime_root = temp.path().canonicalize()?;
    let project = runtime_root.join("project");
    std::fs::create_dir(&project)?;
    let socket = runtime_root.join("gascan/gascand.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
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
    let root = project
        .to_str()
        .ok_or("project path is not UTF-8")?
        .to_owned();
    let mut initial = client
        .up(UpRequest {
            project_root: root.clone(),
        })
        .await?
        .into_inner();
    while initial.message().await?.is_some() {}
    std::fs::write(
        project.join("gascan.toml"),
        "version = 1\n[storage]\ntools = \"20GiB\"\n",
    )?;

    for status in [
        client
            .apply(ApplyRequest {
                project_root: root.clone(),
            })
            .await
            .err()
            .ok_or("apply mismatch unexpectedly started an operation")?,
        client
            .up(UpRequest {
                project_root: root.clone(),
            })
            .await
            .err()
            .ok_or("up mismatch unexpectedly started an operation")?,
    ] {
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            status.message(),
            gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE
        );
        assert_eq!(
            gascan_proto::error_detail::decode_message(status.details()).as_deref(),
            Some(
                "storage settings changed for tools (10GiB → 20GiB); run `gascan destroy --yes` and `gascan up` to recreate the sandbox"
            )
        );
        let details = gascan_proto::error_detail::decode_details(status.details())
            .ok_or("missing structured failure details")?;
        let details: serde_json::Value = serde_json::from_slice(&details)?;
        assert_eq!(
            details["changes"],
            serde_json::json!([{
                "volume": "tools",
                "recorded_bytes": 10 * 1024_u64.pow(3),
                "requested_bytes": 20 * 1024_u64.pow(3),
            }])
        );
    }

    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon has no process id")? as i32)
        .ok_or("daemon process id is zero")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );
    Ok(())
}

#[tokio::test]
async fn image_upgrade_errors_are_structured_across_real_transport() -> TestResult {
    let temp = TempDir::new()?;
    let runtime_root = temp.path().canonicalize()?;
    let project = runtime_root.join("project");
    std::fs::create_dir(&project)?;
    let socket = runtime_root.join("gascan/gascand.sock");
    let state_path = runtime_root.join("state.sqlite3");
    let fake_path = runtime_root.join("fake-runtime.json");
    let old_image = "registry.example/workspace:old@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", &state_path)
        .env("GASCAN_FAKE_STATE_PATH", &fake_path)
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
    let root = project
        .to_str()
        .ok_or("project path is not UTF-8")?
        .to_owned();
    let mut initial = client
        .up(UpRequest {
            project_root: root.clone(),
        })
        .await?
        .into_inner();
    while initial.message().await?.is_some() {}
    let sandbox_id = client
        .list(ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .next()
        .ok_or("created sandbox missing")?
        .sandbox_id;
    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon pid")? as i32)
        .ok_or("zero daemon pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );

    let connection = rusqlite::Connection::open(&state_path)?;
    connection.execute(
        "UPDATE sandboxes SET image_resolution_version = 1, image_resolution_details = ?1",
        [serde_json::json!({"digest": old_image}).to_string()],
    )?;
    drop(connection);
    let mut snapshot: serde_json::Value = serde_json::from_slice(&std::fs::read(&fake_path)?)?;
    snapshot["sandboxes"][0]["image"] = serde_json::Value::String(old_image.to_owned());
    let resources = snapshot["resources"]
        .as_array_mut()
        .ok_or("fake resources missing")?;
    let mut duplicate = resources
        .iter()
        .find(|resource| resource["name"] == sandbox_id)
        .cloned()
        .ok_or("container resource missing")?;
    duplicate["name"] = serde_json::Value::String("unexpected-container".to_owned());
    resources.push(duplicate);
    std::fs::write(&fake_path, serde_json::to_vec(&snapshot)?)?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", &state_path)
        .env("GASCAN_FAKE_STATE_PATH", &fake_path)
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
    let status = GasCanClient::new(channel)
        .apply(ApplyRequest {
            project_root: root.clone(),
        })
        .await
        .err()
        .ok_or("unsafe replacement unexpectedly started")?;
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        status.message(),
        gascan_proto::error_code::IMAGE_UPGRADE_REQUIRED
    );
    let details = gascan_proto::error_detail::decode_details(status.details())
        .ok_or("missing image precondition details")?;
    let details: serde_json::Value = serde_json::from_slice(&details)?;
    assert_eq!(details["reason"], "image_changed");
    assert_eq!(details["current"], old_image);
    assert_eq!(
        details["requested"],
        include_str!("../../../images/workspace/approved-image.txt")
    );
    assert_eq!(details["recovery"], "run `gascan apply` again");

    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon pid")? as i32)
        .ok_or("zero daemon pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );

    let mut snapshot: serde_json::Value = serde_json::from_slice(&std::fs::read(&fake_path)?)?;
    snapshot["resources"]
        .as_array_mut()
        .ok_or("fake resources missing")?
        .retain(|resource| resource["name"] != "unexpected-container");
    std::fs::write(&fake_path, serde_json::to_vec(&snapshot)?)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_gascand"))
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", &state_path)
        .env("GASCAN_FAKE_STATE_PATH", &fake_path)
        .env("GASCAN_FAKE_PROVISION_FAIL", "1")
        .env("GASCAN_FAKE_ROLLBACK_REMOVE_FAIL", "1")
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
    let mut events = GasCanClient::new(channel)
        .apply(ApplyRequest { project_root: root })
        .await?
        .into_inner();
    let mut terminal = None;
    while let Some(event) = events.message().await? {
        terminal = Some(event);
    }
    let terminal = terminal.ok_or("replacement stream missing terminal event")?;
    assert_eq!(
        terminal.status,
        gascan_proto::v1::OperationStatus::Failed as i32
    );
    let error = terminal.error.ok_or("replacement failure missing error")?;
    assert_eq!(
        error.code,
        gascan_proto::error_code::IMAGE_REPLACEMENT_FAILED
    );
    let details: serde_json::Value = serde_json::from_slice(&error.details)?;
    assert!(details["primary"].is_object());
    assert_eq!(details["primary"]["code"], "provision_failed");
    assert!(details["rollback"].is_object());
    assert_eq!(details["rollback"]["code"], "injected_failure");

    let pid = rustix::process::Pid::from_raw(child.id().ok_or("daemon pid")? as i32)
        .ok_or("zero daemon pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );
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
        .env("GASCAN_TEST_FAKE_BACKEND", "1")
        .env("XDG_RUNTIME_DIR", &runtime_root)
        .env("GASCAN_STATE_PATH", runtime_root.join("state.sqlite3"))
        .env("GASCAN_IDLE_TIMEOUT_MS", "30000")
        .env("GASCAN_FAKE_PROVISION_DELAY_MS", "2500")
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
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(
        child.try_wait()?.is_none(),
        "daemon applied its former two-second timeout to durable work"
    );
    tokio::time::timeout(Duration::from_secs(3), operation).await???;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await??
            .success()
    );
    assert!(!socket.exists());
    Ok(())
}
