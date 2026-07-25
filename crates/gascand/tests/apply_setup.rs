use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::manifest::Manifest;
use gascan_core::runtime::{
    ContainerState, ResourceKind, RuntimeBackend, RuntimeCall, RuntimeError,
};
use gascan_core::sandbox::SandboxSpec;
use gascand::{
    ActualState, NoopProvisioner, ProvisionRequest, ProvisionResolution, Provisioner,
    SandboxService, ServiceError, SshPaths, UpRequest, ensure_host_identity,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn ssh_paths(root: &Utf8Path, name: &str) -> TestResult<SshPaths> {
    let config_home = root.join(name);
    std::fs::create_dir(&config_home)?;
    let config_home = std::fs::canonicalize(config_home)?;
    Ok(SshPaths::for_environment(
        Some(config_home.as_os_str()),
        None,
    )?)
}

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

fn write_ssh_manifest(root: &Utf8Path, enabled: bool, host_port: Option<u16>) -> TestResult {
    let port = host_port.map_or_else(String::new, |port| format!("host_port = {port}\n"));
    std::fs::write(
        root.join("gascan.toml"),
        format!("version = 1\nnetwork = 'networked'\n[ssh]\nenabled = {enabled}\n{port}"),
    )?;
    Ok(())
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
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (digest_stdout(bytes, relative), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
        ])
        .await;
}

#[tokio::test]
async fn image_replace_forces_unchanged_setup_on_new_container() -> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let bytes = b"printf replacement\n";
    write_setup(root, "setup.sh", bytes)?;
    let runtime = FakeRuntime::default();
    queue_successful_setup(&runtime, bytes, "setup.sh").await;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let desired = spec(root, "image-replace-setup")?;
    service.up(UpRequest::new(desired.clone())).await?;
    let mut record = service.status(desired.id())?.ok_or("record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        serde_json::json!({"digest":old_image}),
    ));
    service.store().put_sandbox(&record)?;
    queue_successful_setup(&runtime, bytes, "setup.sh").await;
    let setup_runs_before = runtime
        .calls()
        .await
        .iter()
        .filter(|call| {
            matches!(call, RuntimeCall::Exec(request)
                if request.argv.first().map(String::as_str) == Some("/bin/bash"))
        })
        .count();

    service.apply(UpRequest::new(desired)).await?;

    let setup_runs_after = runtime
        .calls()
        .await
        .iter()
        .filter(|call| {
            matches!(call, RuntimeCall::Exec(request)
                if request.argv.first().map(String::as_str) == Some("/bin/bash"))
        })
        .count();
    assert_eq!(setup_runs_after, setup_runs_before + 1);
    Ok(())
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
        execs[2].argv,
        [
            "/usr/bin/env",
            "HOME=/home/workspace",
            "/usr/local/bin/configure-workstation-home",
        ]
    );
    assert_eq!(
        execs[3].argv,
        ["/usr/bin/sha256sum", "/workspace/.gascan/first.sh"]
    );
    assert_eq!(execs[4].argv, ["/bin/bash", "/workspace/.gascan/first.sh"]);
    assert!(execs[3].environment.is_empty());
    assert!(execs[4].environment.is_empty());

    let before_apply = calls.len();
    write_setup(root, ".gascan/moved.sh", bytes)?;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
            (Vec::new(), Vec::new(), 0),
        ])
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
            (Vec::new(), Vec::new(), 0),
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
            (Vec::new(), Vec::new(), 0),
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
        tail.len() <= 64 * 1024 && tail.contains(DIAGNOSTIC) && !tail.contains('\u{1b}')
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
            (Vec::new(), Vec::new(), 0, 0),
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
            (Vec::new(), Vec::new(), 0),
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
            (Vec::new(), Vec::new(), 0),
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

#[tokio::test]
async fn ssh_image_apply_preserves_fingerprints_while_accepting_new_inspected_automatic_port()
-> TestResult {
    let old_image = "ghcr.io/liquescent-development/gascan/workspace:old@sha256:\
                     bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )?;
    let desired = spec(root, "ssh-image-apply")?;
    let paths = ssh_paths(root, "ssh-client")?;
    let host_paths = ssh_paths(root, "ssh-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    runtime.queue_created_ssh_host_port(24_001).await;
    let service = SandboxService::new_with_ssh_for_tests(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        paths.clone(),
        Utf8PathBuf::from("/usr/bin/true"),
    );
    service.up(UpRequest::new(desired.clone())).await?;
    let prior = service
        .status(desired.id())?
        .ok_or("prior record")?
        .ssh_resolution
        .ok_or("prior SSH resolution")?;
    runtime
        .set_sandbox_image(desired.id(), old_image.to_owned())
        .await?;
    let mut record = service.status(desired.id())?.ok_or("record")?;
    record.image_resolution = Some(gascand::ImageResolution::new(
        1,
        serde_json::json!({"digest":old_image}),
    ));
    service.store().put_sandbox(&record)?;
    runtime.queue_created_ssh_host_port(24_002).await;

    service.apply(UpRequest::new(desired.clone())).await?;

    let applied = service.status(desired.id())?.ok_or("applied record")?;
    assert_eq!(applied.ssh_resolution, Some(prior));
    assert_eq!(
        runtime
            .inspect(desired.id())
            .await?
            .ok_or("replacement runtime")?
            .ports()[0]
            .host_port,
        24_002
    );
    let config = std::fs::read_to_string(paths.config())?;
    assert!(config.contains(&format!("Host gascan-{}", desired.id())));
    assert!(config.contains("    Port 24002\n"));
    Ok(())
}

async fn assert_same_image_ssh_policy_recreate(
    name: &str,
    initial_enabled: bool,
    initial_port: Option<u16>,
    next_enabled: bool,
    next_port: Option<u16>,
    initial_observed_port: Option<u16>,
    next_observed_port: Option<u16>,
) -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    write_ssh_manifest(root, initial_enabled, initial_port)?;
    let initial = spec(root, name)?;
    let paths = ssh_paths(root, &format!("{name}-client"))?;
    let host_paths = ssh_paths(root, &format!("{name}-host"))?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    if let Some(port) = initial_observed_port {
        runtime.queue_created_ssh_host_port(port).await;
    }
    let service = SandboxService::new_with_ssh_for_tests(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        paths.clone(),
        Utf8PathBuf::from("/usr/bin/true"),
    );
    service.up(UpRequest::new(initial.clone())).await?;
    let mut retained_volumes = runtime
        .list_resources()
        .await?
        .into_iter()
        .filter(|resource| resource.kind() == ResourceKind::Volume)
        .map(|resource| resource.name().to_owned())
        .collect::<Vec<_>>();
    retained_volumes.sort();
    let replacements_before = runtime
        .calls()
        .await
        .iter()
        .filter(|call| matches!(call, RuntimeCall::CreateContainer(_)))
        .count();

    write_ssh_manifest(root, next_enabled, next_port)?;
    let next = spec(root, name)?;
    if let Some(port) = next_observed_port {
        runtime.queue_created_ssh_host_port(port).await;
    }
    service.apply(UpRequest::new(next.clone())).await?;

    let replacements_after = runtime
        .calls()
        .await
        .iter()
        .filter(|call| matches!(call, RuntimeCall::CreateContainer(_)))
        .count();
    assert_eq!(replacements_after, replacements_before + 1);
    let mut replaced_volumes = runtime
        .list_resources()
        .await?
        .into_iter()
        .filter(|resource| resource.kind() == ResourceKind::Volume)
        .map(|resource| resource.name().to_owned())
        .collect::<Vec<_>>();
    replaced_volumes.sort();
    assert_eq!(replaced_volumes, retained_volumes);
    let inspected = runtime.inspect(next.id()).await?.ok_or("runtime")?;
    let ssh_ports = inspected
        .ports()
        .iter()
        .filter(|mapping| mapping.guest_port == 22)
        .map(|mapping| mapping.host_port)
        .collect::<Vec<_>>();
    let expected_port = next_observed_port.or(next_port);
    assert_eq!(ssh_ports, expected_port.into_iter().collect::<Vec<_>>());
    let record = service.status(next.id())?.ok_or("record")?;
    assert_eq!(
        record
            .ssh_resolution
            .as_ref()
            .and_then(|resolution| resolution.details["enabled"].as_bool()),
        Some(next_enabled)
    );
    let config = std::fs::read_to_string(paths.config())?;
    assert_eq!(
        config.contains(&format!("Host gascan-{}", next.id())),
        next_enabled
    );
    Ok(())
}

#[tokio::test]
async fn same_image_apply_recreates_enabled_ssh_as_disabled() -> TestResult {
    assert_same_image_ssh_policy_recreate(
        "ssh-disable-apply",
        true,
        Some(24_101),
        false,
        None,
        None,
        None,
    )
    .await
}

#[tokio::test]
async fn same_image_apply_recreates_disabled_ssh_as_enabled() -> TestResult {
    assert_same_image_ssh_policy_recreate(
        "ssh-enable-apply",
        false,
        None,
        true,
        Some(24_102),
        None,
        None,
    )
    .await
}

#[tokio::test]
async fn same_image_apply_recreates_automatic_ssh_as_explicit() -> TestResult {
    assert_same_image_ssh_policy_recreate(
        "ssh-auto-explicit-apply",
        true,
        None,
        true,
        Some(24_104),
        Some(24_103),
        None,
    )
    .await
}

#[tokio::test]
async fn same_image_apply_recreates_explicit_ssh_as_automatic() -> TestResult {
    assert_same_image_ssh_policy_recreate(
        "ssh-explicit-auto-apply",
        true,
        Some(24_107),
        true,
        None,
        None,
        Some(24_108),
    )
    .await
}

#[tokio::test]
async fn same_image_apply_recreates_changed_explicit_ssh_port() -> TestResult {
    assert_same_image_ssh_policy_recreate(
        "ssh-explicit-change-apply",
        true,
        Some(24_105),
        true,
        Some(24_106),
        None,
        None,
    )
    .await
}

#[derive(Clone, Copy)]
struct UpApplySshTransition {
    initial_port: u16,
    next_enabled: bool,
    next_port: Option<u16>,
    next_observed_port: Option<u16>,
}

fn stored_ssh_transport_policy(
    state_path: &Utf8Path,
    id: &gascan_core::sandbox::SandboxId,
) -> TestResult<(Option<i64>, Option<i64>)> {
    let connection = rusqlite::Connection::open(state_path)?;
    Ok(connection.query_row(
        "SELECT ssh_transport_enabled, ssh_transport_host_port FROM sandboxes WHERE id = ?1",
        [id.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

async fn assert_up_apply_preserves_prior_ssh_transport(
    name: &str,
    transition: UpApplySshTransition,
) -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let state_path = root.join("state.db");
    write_ssh_manifest(root, true, Some(transition.initial_port))?;
    let initial = spec(root, name)?;
    let paths = ssh_paths(root, &format!("{name}-client"))?;
    let host_paths = ssh_paths(root, &format!("{name}-host"))?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = SandboxService::new_with_ssh_for_tests(
        runtime.clone(),
        gascand::Store::open(&state_path)?,
        Arc::new(NoopProvisioner),
        paths.clone(),
        Utf8PathBuf::from("/usr/bin/true"),
    );
    service.up(UpRequest::new(initial.clone())).await?;
    let prior_resolution = service
        .status(initial.id())?
        .ok_or("prior record")?
        .ssh_resolution
        .ok_or("prior SSH resolution")?;
    assert_eq!(
        stored_ssh_transport_policy(&state_path, initial.id())?,
        (Some(1), Some(i64::from(transition.initial_port)))
    );
    let replacements_before = runtime
        .calls()
        .await
        .iter()
        .filter(|call| matches!(call, RuntimeCall::CreateContainer(_)))
        .count();

    write_ssh_manifest(root, transition.next_enabled, transition.next_port)?;
    let next = spec(root, name)?;
    let up = service.up(UpRequest::new(next.clone())).await?;
    let up_events = service.store().operation_events(up.id)?;

    assert!(up_events.iter().any(|event| {
        event
            .details
            .as_ref()
            .and_then(|details| details["phase"].as_str())
            == Some("apply_required")
    }));
    assert_eq!(
        service
            .status(next.id())?
            .ok_or("record after up")?
            .ssh_resolution,
        Some(prior_resolution)
    );
    assert_eq!(
        stored_ssh_transport_policy(&state_path, next.id())?,
        (Some(1), Some(i64::from(transition.initial_port)))
    );
    assert_eq!(
        runtime
            .inspect(next.id())
            .await?
            .ok_or("runtime after up")?
            .ports()
            .iter()
            .filter(|mapping| mapping.guest_port == 22)
            .map(|mapping| mapping.host_port)
            .collect::<Vec<_>>(),
        vec![transition.initial_port]
    );
    assert!(
        std::fs::read_to_string(paths.config())?.contains(&format!("Host gascan-{}", next.id()))
    );

    if let Some(port) = transition.next_observed_port {
        runtime.queue_created_ssh_host_port(port).await;
    }
    service.apply(UpRequest::new(next.clone())).await?;

    assert_eq!(
        runtime
            .calls()
            .await
            .iter()
            .filter(|call| matches!(call, RuntimeCall::CreateContainer(_)))
            .count(),
        replacements_before + 1
    );
    let expected_port = transition.next_observed_port.or(transition.next_port);
    assert_eq!(
        runtime
            .inspect(next.id())
            .await?
            .ok_or("runtime after apply")?
            .ports()
            .iter()
            .filter(|mapping| mapping.guest_port == 22)
            .map(|mapping| mapping.host_port)
            .collect::<Vec<_>>(),
        expected_port.into_iter().collect::<Vec<_>>()
    );
    let applied = service.status(next.id())?.ok_or("applied record")?;
    assert_eq!(
        applied
            .ssh_resolution
            .as_ref()
            .and_then(|resolution| resolution.details["enabled"].as_bool()),
        Some(transition.next_enabled)
    );
    assert_eq!(
        stored_ssh_transport_policy(&state_path, next.id())?,
        (
            Some(i64::from(transition.next_enabled)),
            transition.next_port.map(i64::from)
        )
    );
    Ok(())
}

#[tokio::test]
async fn enabled_to_disabled_change_survives_intervening_up_and_apply_recreates_without_ssh()
-> TestResult {
    assert_up_apply_preserves_prior_ssh_transport(
        "ssh-up-before-disable-apply",
        UpApplySshTransition {
            initial_port: 24_301,
            next_enabled: false,
            next_port: None,
            next_observed_port: None,
        },
    )
    .await
}

#[tokio::test]
async fn explicit_to_automatic_change_survives_intervening_up_and_apply_recreates_ssh() -> TestResult
{
    assert_up_apply_preserves_prior_ssh_transport(
        "ssh-up-before-auto-apply",
        UpApplySshTransition {
            initial_port: 24_302,
            next_enabled: true,
            next_port: None,
            next_observed_port: Some(24_303),
        },
    )
    .await
}

#[tokio::test]
async fn transport_change_up_returns_apply_required_without_preparing_desired_ssh() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    write_ssh_manifest(root, true, Some(24_304))?;
    let initial = spec(root, "ssh-up-skips-preparation")?;
    let paths = ssh_paths(root, "ssh-up-skips-preparation-client")?;
    let host_paths = ssh_paths(root, "ssh-up-skips-preparation-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = SandboxService::new_with_ssh_for_tests(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
        paths.clone(),
        Utf8PathBuf::from("/usr/bin/true"),
    );
    service.up(UpRequest::new(initial)).await?;

    write_ssh_manifest(root, true, None)?;
    let next = spec(root, "ssh-up-skips-preparation")?;
    std::fs::remove_file(paths.public_key())?;
    let victim = root.join("ssh-public-key-victim");
    std::fs::write(&victim, "not a managed public key")?;
    std::os::unix::fs::symlink(&victim, paths.public_key())?;

    let up = service
        .up(UpRequest::new(next))
        .await
        .map_err(|error| format!("apply-required up prepared desired SSH: {error}"))?;
    let events = service.store().operation_events(up.id)?;

    assert!(events.iter().any(|event| {
        event
            .details
            .as_ref()
            .and_then(|details| details["phase"].as_str())
            == Some("apply_required")
    }));
    assert_eq!(std::fs::read_to_string(victim)?, "not a managed public key");
    Ok(())
}

fn loopback_port_collision(port: u16) -> RuntimeError {
    RuntimeError::CommandFailed {
        operation: "container".to_owned(),
        exit_code: Some(1),
        stderr: format!("Error: listen tcp 127.0.0.1:{port}: bind: address already in use\n"),
    }
}

#[derive(Clone, Copy)]
struct InitialSshFixture {
    enabled: bool,
    requested_port: Option<u16>,
    observed_port: Option<u16>,
    stale_resolution_disabled: bool,
    stale_transport_disabled: bool,
}

async fn failed_ssh_policy_apply_requests(
    name: &str,
    initial: InitialSshFixture,
    next_enabled: bool,
    next_port: Option<u16>,
    next_failure: Option<RuntimeError>,
) -> TestResult<Vec<gascan_core::runtime::CreateRequest>> {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
    let state_path = root.join("state.db");
    write_ssh_manifest(root, initial.enabled, initial.requested_port)?;
    let initial_spec = spec(root, name)?;
    let paths = ssh_paths(root, &format!("{name}-client"))?;
    let host_paths = ssh_paths(root, &format!("{name}-host"))?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    if let Some(port) = initial.observed_port {
        runtime.queue_created_ssh_host_port(port).await;
    }
    let provisioner = Arc::new(FailingProvisioner::default());
    let service = SandboxService::new_with_ssh_for_tests(
        runtime.clone(),
        gascand::Store::open(&state_path)?,
        provisioner.clone(),
        paths.clone(),
        Utf8PathBuf::from("/usr/bin/true"),
    );
    service.up(UpRequest::new(initial_spec.clone())).await?;
    if initial.stale_resolution_disabled {
        service.store().update_ssh_resolution(
            initial_spec.id(),
            gascand::SshResolution::new(
                1,
                serde_json::json!({
                    "enabled": false,
                    "host_key_fingerprint": "",
                    "client_key_fingerprint": "",
                }),
            ),
        )?;
    }
    if initial.stale_transport_disabled {
        let connection = rusqlite::Connection::open(&state_path)?;
        connection.execute(
            "UPDATE sandboxes SET ssh_transport_enabled = 0, ssh_transport_host_port = NULL WHERE id = ?1",
            [initial_spec.id().as_str()],
        )?;
        assert_eq!(
            stored_ssh_transport_policy(&state_path, initial_spec.id())?,
            (Some(0), None)
        );
    }
    let replacements_before = runtime
        .calls()
        .await
        .iter()
        .filter(|call| matches!(call, RuntimeCall::CreateContainer(_)))
        .count();

    write_ssh_manifest(root, next_enabled, next_port)?;
    let next = spec(root, name)?;
    if let Some(error) = next_failure {
        runtime.queue_create_error(error).await;
    } else {
        provisioner.0.store(true, Ordering::SeqCst);
    }

    assert!(
        service.apply(UpRequest::new(next.clone())).await.is_err(),
        "transport replacement unexpectedly succeeded"
    );
    let inspected = runtime
        .inspect(next.id())
        .await?
        .ok_or("rolled back runtime")?;
    let expected_ports = initial
        .observed_port
        .or(initial.requested_port)
        .into_iter()
        .collect::<Vec<_>>();
    assert_eq!(
        inspected
            .ports()
            .iter()
            .filter(|mapping| mapping.guest_port == 22)
            .map(|mapping| mapping.host_port)
            .collect::<Vec<_>>(),
        expected_ports
    );
    let resolution = service
        .status(next.id())?
        .ok_or("rolled back record")?
        .ssh_resolution
        .ok_or("rolled back SSH resolution")?;
    assert_eq!(
        resolution.details["enabled"].as_bool(),
        Some(initial.enabled)
    );
    let config_has_alias = paths.config().exists()
        && std::fs::read_to_string(paths.config())?.contains(&format!("Host gascan-{}", next.id()));
    assert_eq!(config_has_alias, initial.enabled);

    Ok(runtime
        .calls()
        .await
        .into_iter()
        .filter_map(|call| match call {
            RuntimeCall::CreateContainer(request) => Some(request.create().clone()),
            _ => None,
        })
        .skip(replacements_before)
        .collect())
}

#[tokio::test]
async fn failed_enabled_to_disabled_apply_restores_enabled_native_ssh() -> TestResult {
    let requests = failed_ssh_policy_apply_requests(
        "ssh-enable-disable-rollback",
        InitialSshFixture {
            enabled: true,
            requested_port: None,
            observed_port: Some(24_201),
            stale_resolution_disabled: true,
            stale_transport_disabled: false,
        },
        false,
        None,
        None,
    )
    .await?;
    let rollback = requests.last().ok_or("rollback request")?;

    assert_eq!(
        rollback.environment().get("GASCAN_SSH_ENABLED"),
        Some(&"1".to_owned())
    );
    assert!(
        rollback
            .environment()
            .contains_key("GASCAN_SSH_AUTHORIZED_KEY")
    );
    assert!(
        rollback
            .ports()
            .iter()
            .any(|mapping| { mapping.guest_port == 22 && mapping.host_port == 24_201 })
    );
    Ok(())
}

#[tokio::test]
async fn failed_disabled_to_enabled_apply_restores_disabled_native_ssh() -> TestResult {
    let requests = failed_ssh_policy_apply_requests(
        "ssh-disable-enable-rollback",
        InitialSshFixture {
            enabled: false,
            requested_port: None,
            observed_port: None,
            stale_resolution_disabled: false,
            stale_transport_disabled: false,
        },
        true,
        Some(24_202),
        None,
    )
    .await?;
    let rollback = requests.last().ok_or("rollback request")?;

    assert_eq!(
        rollback.environment().get("GASCAN_SSH_ENABLED"),
        Some(&"0".to_owned())
    );
    assert!(
        !rollback
            .environment()
            .contains_key("GASCAN_SSH_AUTHORIZED_KEY")
    );
    assert!(
        rollback
            .ports()
            .iter()
            .all(|mapping| mapping.guest_port != 22)
    );
    Ok(())
}

#[tokio::test]
async fn failed_explicit_port_change_restores_old_port_without_retrying_new_port() -> TestResult {
    let requests = failed_ssh_policy_apply_requests(
        "ssh-explicit-port-rollback",
        InitialSshFixture {
            enabled: true,
            requested_port: Some(24_203),
            observed_port: None,
            stale_resolution_disabled: false,
            stale_transport_disabled: false,
        },
        true,
        Some(24_204),
        Some(loopback_port_collision(24_204)),
    )
    .await?;
    let attempted_ports = requests
        .iter()
        .filter_map(|request| {
            request
                .ports()
                .iter()
                .find(|mapping| mapping.guest_port == 22)
                .map(|mapping| mapping.host_port)
        })
        .collect::<Vec<_>>();

    assert_eq!(attempted_ports, vec![24_204, 24_203]);
    Ok(())
}

#[tokio::test]
async fn failed_replacement_restores_observed_ssh_when_durable_policy_is_stale_disabled()
-> TestResult {
    let requests = failed_ssh_policy_apply_requests(
        "ssh-stale-disabled-policy-rollback",
        InitialSshFixture {
            enabled: true,
            requested_port: Some(24_205),
            observed_port: None,
            stale_resolution_disabled: true,
            stale_transport_disabled: true,
        },
        true,
        Some(24_206),
        None,
    )
    .await?;
    let rollback = requests.last().ok_or("rollback request")?;

    assert_eq!(
        rollback.environment().get("GASCAN_SSH_ENABLED"),
        Some(&"1".to_owned())
    );
    assert!(
        rollback
            .ports()
            .iter()
            .any(|mapping| mapping.guest_port == 22 && mapping.host_port == 24_205)
    );
    Ok(())
}
