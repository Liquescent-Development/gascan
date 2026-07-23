use async_trait::async_trait;
use camino::Utf8Path;
use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::manifest::Manifest;
use gascan_core::runtime::{ContainerState, RuntimeBackend, RuntimeCall};
use gascan_core::sandbox::SandboxSpec;
use gascand::{
    ActualState, NoopProvisioner, ProvisionRequest, ProvisionResolution, Provisioner,
    SandboxService, ServiceError, UpRequest,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_setup(root: &Utf8Path, relative: &str, bytes: &[u8]) -> TestResult {
    if let Some(parent) = root.join(relative).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(root.join(relative), bytes)?;
    std::fs::write(
        root.join("gascan.toml"),
        format!("version = 1\nsetup = {relative:?}\n"),
    )?;
    Ok(())
}

fn spec(root: &Utf8Path, name: &str) -> TestResult<SandboxSpec> {
    Ok(SandboxSpec::from_root(name, root, Manifest::load(root)?)?)
}

fn setup_resolution(record: &gascand::SandboxRecord) -> Option<&Value> {
    record.setup_resolution.as_ref()?.details.get("resolution")
}

fn digest_stdout(bytes: &[u8], relative: &str) -> Vec<u8> {
    format!(
        "{}  /workspace/{relative}\n",
        digest(bytes).trim_start_matches("sha256:")
    )
    .into_bytes()
}

async fn queue_successful_setup(runtime: &FakeRuntime, bytes: &[u8], relative: &str) {
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (digest_stdout(bytes, relative), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
        ])
        .await;
}

#[tokio::test]
async fn setup_uses_literal_guest_argv_empty_environments_and_refreshes_moved_path() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let bytes = b"printf safe\n";
    write_setup(root, ".gascan/first.sh", bytes)?;
    let runtime = FakeRuntime::default();
    queue_successful_setup(&runtime, bytes, ".gascan/first.sh").await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    service
        .up(UpRequest::new(spec(root, "setup-argv")?))
        .await?;
    let calls = runtime.calls().await;
    let execs = calls
        .iter()
        .filter_map(|call| match call {
            RuntimeCall::Exec(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        execs[1].argv,
        ["/usr/bin/sha256sum", "/workspace/.gascan/first.sh"]
    );
    assert_eq!(execs[2].argv, ["/bin/bash", "/workspace/.gascan/first.sh"]);
    assert!(execs[1].environment.is_empty());
    assert!(execs[2].environment.is_empty());

    let before_apply = calls.len();
    write_setup(root, ".gascan/moved.sh", bytes)?;
    runtime
        .queue_exec_results([(Vec::new(), Vec::new(), 0), (Vec::new(), Vec::new(), 0)])
        .await;
    service
        .apply(UpRequest::new(spec(root, "setup-argv")?))
        .await?;
    let apply_calls = runtime.calls().await;
    assert!(apply_calls[before_apply..].iter().all(|call| {
        !matches!(call, RuntimeCall::Exec(request) if request.argv.first().is_some_and(|arg| arg == "/bin/bash" || arg == "/usr/bin/sha256sum"))
    }));
    let record = service
        .status(spec(root, "setup-argv")?.id())?
        .ok_or("record")?;
    assert_eq!(
        setup_resolution(&record).and_then(|value| value.get("canonical_relative_path")),
        Some(&Value::String(".gascan/moved.sh".to_owned()))
    );
    assert_eq!(
        setup_resolution(&record).and_then(|value| value.get("sha256")),
        Some(&Value::String(digest(bytes)))
    );
    Ok(())
}

#[tokio::test]
async fn digest_mismatch_stops_retains_digest_and_retry_succeeds() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let first = b"printf first\n";
    let second = b"printf second\n";
    write_setup(root, "setup.sh", first)?;
    let runtime = FakeRuntime::default();
    queue_successful_setup(&runtime, first, "setup.sh").await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let make_spec = || spec(root, "setup-race");
    service.up(UpRequest::new(make_spec()?)).await?;
    let id = make_spec()?.id().clone();
    let prior = service.status(&id)?.ok_or("prior")?.setup_resolution;
    write_setup(root, "setup.sh", second)?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (b"0000000000000000000000000000000000000000000000000000000000000000  /workspace/setup.sh\n".to_vec(), Vec::new(), 0),
        ])
        .await;

    let error = match service.apply(UpRequest::new(make_spec()?)).await {
        Ok(_) => return Err("digest mismatch unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "mounted setup script changed before execution"
    );
    let failed = service.status(&id)?.ok_or("failed record")?;
    assert_eq!(failed.setup_resolution, prior);
    assert_eq!(failed.actual_state, ActualState::Stopped);
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("runtime")?.state,
        ContainerState::Stopped
    );
    let operation = service.latest_operation()?.ok_or("operation")?;
    let details = service
        .store()
        .operation_events(operation.id)?
        .into_iter()
        .filter_map(|event| event.details)
        .find(|details| details.get("phase") == Some(&Value::String("setup".to_owned())))
        .ok_or("setup failure metadata")?;
    assert_eq!(details.get("retryable"), Some(&Value::Bool(true)));
    assert!(details.get("exit_code").is_none());

    queue_successful_setup(&runtime, second, "setup.sh").await;
    service.apply(UpRequest::new(make_spec()?)).await?;
    let retried = service.status(&id)?.ok_or("retried")?;
    assert_eq!(
        setup_resolution(&retried).and_then(|value| value.get("sha256")),
        Some(&Value::String(digest(second)))
    );
    Ok(())
}

#[tokio::test]
async fn changed_setup_apply_restarts_running_container_before_guest_digest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let first = b"printf first\n";
    let second = b"printf second\n";
    write_setup(root, "setup.sh", first)?;
    let runtime = FakeRuntime::default();
    queue_successful_setup(&runtime, first, "setup.sh").await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let make_spec = || spec(root, "setup-refresh");
    service.up(UpRequest::new(make_spec()?)).await?;
    write_setup(root, "setup.sh", second)?;
    queue_successful_setup(&runtime, second, "setup.sh").await;
    let before = runtime.calls().await.len();

    service.apply(UpRequest::new(make_spec()?)).await?;

    let calls = runtime.calls().await;
    let refresh = &calls[before..];
    let stop = refresh
        .iter()
        .position(|call| matches!(call, RuntimeCall::Stop(_)))
        .ok_or("refresh stop")?;
    let start = refresh
        .iter()
        .position(|call| matches!(call, RuntimeCall::Start(_)))
        .ok_or("refresh start")?;
    let digest = refresh
        .iter()
        .position(|call| matches!(call, RuntimeCall::Exec(request) if request.argv.first().is_some_and(|arg| arg == "/usr/bin/sha256sum")))
        .ok_or("guest digest")?;
    assert!(stop < start && start < digest);
    Ok(())
}

#[tokio::test]
async fn nonzero_setup_exit_is_structured_sanitized_stopped_and_retryable() -> TestResult {
    const DIAGNOSTIC: &str = "write failed: No space left on device";
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let bytes = b"exit 23\n";
    write_setup(root, "setup.sh", bytes)?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (digest_stdout(bytes, "setup.sh"), Vec::new(), 0),
            (
                Vec::new(),
                [
                    vec![b'x'; 70 * 1024],
                    b"\nwrite failed: No space left on device\x1b".to_vec(),
                ]
                .concat(),
                23,
            ),
        ])
        .await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    let error = match service.up(UpRequest::new(spec(root, "setup-exit")?)).await {
        Ok(_) => return Err("setup exit unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("exit code 23"));
    assert!(error.to_string().contains(DIAGNOSTIC));
    assert!(!error.to_string().contains('\u{1b}'));
    let id = spec(root, "setup-exit")?.id().clone();
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("runtime")?.state,
        ContainerState::Stopped
    );
    let operation = service.latest_operation()?.ok_or("operation")?;
    let details = operation.error_details.ok_or("error details")?;
    assert_eq!(details["phase"], "setup");
    assert_eq!(details["retryable"], true);
    assert_eq!(details["action"], "run_setup");
    assert_eq!(details["exit_code"], 23);
    assert_eq!(details["signal"], 0);
    assert!(details["stderr_tail"].as_str().is_some_and(|tail| {
        tail.as_bytes().len() <= 64 * 1024 && tail.contains(DIAGNOSTIC) && !tail.contains('\u{1b}')
    }));
    Ok(())
}

#[tokio::test]
async fn signaled_setup_preserves_signal_and_sanitized_stderr() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let bytes = b"kill -TERM $$\n";
    write_setup(root, "setup.sh", bytes)?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results_with_signals([
            (Vec::new(), Vec::new(), 0, 0),
            (digest_stdout(bytes, "setup.sh"), Vec::new(), 0, 0),
            (Vec::new(), b"terminated\x00\n".to_vec(), 143, 15),
        ])
        .await;
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    let error = match service
        .up(UpRequest::new(spec(root, "setup-signal")?))
        .await
    {
        Ok(_) => return Err("signaled setup unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("signal 15"));
    let details = service
        .latest_operation()?
        .ok_or("operation")?
        .error_details
        .ok_or("error details")?;
    assert_eq!(details["action"], "run_setup");
    assert_eq!(details["exit_code"], 143);
    assert_eq!(details["signal"], 15);
    assert_eq!(details["stderr_tail"], "terminated  ");
    Ok(())
}

#[tokio::test]
async fn stop_failure_preserves_setup_failure_and_reports_unconfirmed_state() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let bytes = b"exit 29\n";
    write_setup(root, "setup.sh", bytes)?;
    let runtime = FakeRuntime::default();
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (digest_stdout(bytes, "setup.sh"), Vec::new(), 0),
            (Vec::new(), Vec::new(), 29),
        ])
        .await;
    runtime.inject_failure(FailureBoundary::Stop).await;
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    let error = match service
        .up(UpRequest::new(spec(root, "setup-stop-failure")?))
        .await
    {
        Ok(_) => return Err("setup and stop unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("exit code 29"));
    assert!(
        error
            .to_string()
            .contains("stopped state could not be confirmed")
    );
    let operation = service.latest_operation()?.ok_or("operation")?;
    let details = service
        .store()
        .operation_events(operation.id)?
        .into_iter()
        .filter_map(|event| event.details)
        .find(|details| details.get("phase") == Some(&Value::String("setup".to_owned())))
        .ok_or("setup details")?;
    assert_eq!(details.get("exit_code"), Some(&Value::from(29)));
    assert_eq!(details.get("stopped"), Some(&Value::Bool(false)));
    Ok(())
}

#[derive(Default)]
struct FailingProvisioner(AtomicBool);

#[async_trait]
impl Provisioner for FailingProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        if self.0.load(Ordering::SeqCst) {
            Err(ServiceError::Provision("later boundary failed".to_owned()))
        } else {
            Ok(ProvisionResolution::default())
        }
    }

    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        Ok(())
    }
}

#[tokio::test]
async fn later_provision_failure_does_not_advance_setup_digest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let first = b"printf first\n";
    let second = b"printf second\n";
    write_setup(root, "setup.sh", first)?;
    let runtime = FakeRuntime::default();
    queue_successful_setup(&runtime, first, "setup.sh").await;
    let provisioner = Arc::new(FailingProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    let make_spec = || spec(root, "setup-later-failure");
    service.up(UpRequest::new(make_spec()?)).await?;
    let id = make_spec()?.id().clone();
    let prior = service.status(&id)?.ok_or("prior")?.setup_resolution;
    write_setup(root, "setup.sh", second)?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (digest_stdout(second, "setup.sh"), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
        ])
        .await;
    provisioner.0.store(true, Ordering::SeqCst);

    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert_eq!(
        service.status(&id)?.ok_or("failed")?.setup_resolution,
        prior
    );
    Ok(())
}
