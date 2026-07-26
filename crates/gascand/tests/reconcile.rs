use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::fake_runtime::{FailureBoundary, FakeExecHangPhase, FakeRuntime};
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{ResourceKind, ResourceOwnership, RuntimeBackend, RuntimePort};
use gascan_core::sandbox::SandboxSpec;
use gascand::{
    ActualState, DesiredState, NoopProvisioner, OperationKind, OperationStatus, ProvisionRequest,
    ProvisionResolution, Provisioner, ReconcileFinding, SandboxRecord, SandboxService,
    ServiceError, SshPaths, SshResolution, Store, UpRequest, ensure_host_identity,
};
use serde_json::json;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn ssh_paths(root: &Utf8Path, name: &str) -> TestResult<SshPaths> {
    let home = root.join(name);
    std::fs::create_dir(&home)?;
    let home = std::fs::canonicalize(home)?;
    Ok(SshPaths::for_environment(None, Some(home.as_os_str()))?)
}

fn readiness_program(root: &Utf8Path, failing_port: Option<u16>) -> TestResult<Utf8PathBuf> {
    let path = root.join(format!(
        "readiness-{}",
        failing_port.map_or_else(|| "ok".to_owned(), |port| port.to_string())
    ));
    let failure = failing_port.map_or_else(String::new, |port| {
        format!("for arg do [ \"$arg\" = \"Port={port}\" ] && exit 19; done\n")
    });
    std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{failure}exit 0\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

fn ssh_service(
    runtime: FakeRuntime,
    store: Store,
    paths: SshPaths,
    readiness: Utf8PathBuf,
) -> SandboxService<FakeRuntime> {
    SandboxService::new_with_ssh_for_tests(
        runtime,
        store,
        Arc::new(NoopProvisioner),
        paths,
        readiness,
    )
}

fn ssh_mapping(port: u16) -> RuntimePort {
    RuntimePort {
        host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        host_port: port,
        guest_port: 22,
    }
}

#[tokio::test]
async fn reconcile_reports_unknown_owned_resources_without_deleting() -> Result<(), Box<dyn Error>>
{
    let temp = tempfile::tempdir()?;
    let runtime = FakeRuntime::default();
    let unknown = gascan_core::sandbox::SandboxId::test("unknown");
    runtime.seed_owned(unknown.clone()).await;
    let network = PolicyCompiler::managed_network_name(&unknown);
    runtime
        .seed_network(
            &network,
            Some(unknown.clone()),
            ResourceOwnership::GasCanOwned,
        )
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        Store::open(temp.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let report = service.reconcile().await?;
    assert!(
        report
            .findings
            .iter()
            .any(|finding| matches!(finding, ReconcileFinding::UnknownOwned(resource) if resource.sandbox_id() == Some(&unknown)))
    );
    assert!(runtime.inspect(&unknown).await?.is_some());
    assert!(report.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::UnknownOwned(resource)
            if resource.kind() == ResourceKind::Network
                && resource.name() == network
                && resource.sandbox_id() == Some(&unknown)
    )));
    assert!(runtime.network_exists(&network).await);
    Ok(())
}

#[tokio::test]
async fn reconcile_does_not_report_a_known_sandbox_network_as_unknown() -> Result<(), Box<dyn Error>>
{
    let temp = tempfile::tempdir()?;
    let root = Utf8PathBuf::from_path_buf(temp.path().join("project")).map_err(|_| "utf8 root")?;
    std::fs::create_dir(&root)?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nenabled = false\n",
    )?;
    let spec = SandboxSpec::from_root("known-network", &root, Manifest::load(&root)?)?;
    let id = spec.id().clone();
    let network = PolicyCompiler::managed_network_name(&id);
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        Store::open(temp.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(spec)).await?;

    let report = service.reconcile().await?;

    assert!(runtime.network_exists(&network).await);
    assert!(!report.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::UnknownOwned(resource)
            | ReconcileFinding::UnknownUnowned(resource)
            | ReconcileFinding::OwnershipMismatch(resource)
            if resource.kind() == ResourceKind::Network && resource.name() == network
    )));
    Ok(())
}

#[tokio::test]
async fn reconcile_reports_all_unknown_ownership_classes_and_retains_them()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let runtime = FakeRuntime::default();
    let owned = gascan_core::sandbox::SandboxId::test("unknown-owned");
    let foreign = gascan_core::sandbox::SandboxId::test("unknown-foreign");
    let mismatch = gascan_core::sandbox::SandboxId::test("unknown-mismatch");
    runtime.seed_owned(owned.clone()).await;
    runtime.seed_unowned(foreign.clone()).await;
    runtime.seed_mismatched(mismatch.clone()).await;
    runtime
        .seed_volume("orphan-volume", None, ResourceOwnership::Foreign)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        Store::open(temp.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    let report = service.reconcile().await?;
    assert!(report.findings.iter().any(|finding| matches!(finding, ReconcileFinding::UnknownOwned(resource) if resource.sandbox_id() == Some(&owned))));
    assert!(report.findings.iter().any(|finding| matches!(finding, ReconcileFinding::UnknownUnowned(resource) if resource.sandbox_id() == Some(&foreign))));
    assert!(report.findings.iter().any(|finding| matches!(finding, ReconcileFinding::OwnershipMismatch(resource) if resource.sandbox_id() == Some(&mismatch))));
    assert!(report.findings.iter().any(|finding| matches!(finding, ReconcileFinding::UnknownUnowned(resource) if resource.name() == "orphan-volume")));
    assert_eq!(runtime.list_resources().await?.len(), 4);
    Ok(())
}

#[tokio::test]
async fn reopen_reconciliation_terminalizes_every_pending_operation_kind()
-> Result<(), Box<dyn Error>> {
    for (kind, stored, runtime_state) in [
        (OperationKind::Create, ActualState::Creating, Some(true)),
        (OperationKind::Apply, ActualState::Running, Some(true)),
        (OperationKind::Start, ActualState::Stopped, Some(true)),
        (OperationKind::Stop, ActualState::Running, Some(false)),
        (OperationKind::Destroy, ActualState::Destroying, None),
    ] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("state.db");
        let id = gascan_core::sandbox::SandboxId::test(&format!("pending-{kind:?}"));
        let record = SandboxRecord {
            id: id.clone(),
            canonical_root: Utf8PathBuf::from(format!("/pending/{kind:?}")),
            desired_state: if kind == OperationKind::Destroy {
                DesiredState::Absent
            } else {
                DesiredState::Running
            },
            actual_state: stored,
            setup_resolution: None,
            tool_resolution: None,
            image_resolution: None,
            storage_resolution: None,
            ssh_resolution: None,
            last_operation_id: None,
            updated_at_millis: 0,
        };
        let store = Store::open(&path)?;
        let pending = store.begin_operation(&record, kind)?;
        drop(store);
        let runtime = FakeRuntime::default();
        if let Some(running) = runtime_state {
            runtime.seed_owned(id.clone()).await;
            if running {
                runtime.start(&id).await?;
            }
        }
        let service = SandboxService::new(runtime, Store::open(&path)?, Arc::new(NoopProvisioner));

        service.reconcile().await?;

        assert!(service.store().pending_operations()?.is_empty());
        let operation = service.store().latest_operation()?.ok_or("operation")?;
        assert_eq!(operation.id, pending.id);
        let expected = match kind {
            OperationKind::Create | OperationKind::Apply => OperationStatus::Failed,
            OperationKind::Start | OperationKind::Stop | OperationKind::Destroy => {
                OperationStatus::Completed
            }
            OperationKind::Reconcile => return Err("unexpected reconcile fixture".into()),
        };
        assert_eq!(operation.status, expected);
    }
    Ok(())
}

#[tokio::test]
async fn pending_inspect_failure_is_reported_and_other_recovery_continues() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let broken_id = gascan_core::sandbox::SandboxId::test("pending-inspect-broken");
    let continued_id = gascan_core::sandbox::SandboxId::test("pending-inspect-continued");
    let pending_record = |id: gascan_core::sandbox::SandboxId| SandboxRecord {
        canonical_root: Utf8PathBuf::from(format!("/pending/{id}")),
        id,
        desired_state: DesiredState::Running,
        actual_state: ActualState::Stopped,
        setup_resolution: None,
        tool_resolution: None,
        image_resolution: None,
        storage_resolution: None,
        ssh_resolution: None,
        last_operation_id: None,
        updated_at_millis: 0,
    };
    let broken = store.begin_operation(&pending_record(broken_id.clone()), OperationKind::Start)?;
    let continued =
        store.begin_operation(&pending_record(continued_id.clone()), OperationKind::Start)?;
    let runtime = FakeRuntime::default();
    runtime.seed_owned(broken_id.clone()).await;
    runtime.seed_owned(continued_id.clone()).await;
    runtime.start(&broken_id).await?;
    runtime.start(&continued_id).await?;
    let unknown = gascan_core::sandbox::SandboxId::test("pending-inspect-unknown");
    runtime.seed_owned(unknown.clone()).await;
    runtime.inject_failure(FailureBoundary::Inspect).await;
    let service = SandboxService::new(runtime, store.clone(), Arc::new(NoopProvisioner));

    let report = service.reconcile().await?;

    assert!(report.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::InspectionUnavailable { sandbox_id, reason }
            if sandbox_id == &broken_id && reason == "injected_failure"
    )));
    assert!(report.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::UnknownOwned(resource)
            if resource.sandbox_id() == Some(&unknown)
    )));
    let pending = store.pending_operations()?;
    assert!(pending.iter().any(|operation| operation.id == broken.id));
    assert!(!pending.iter().any(|operation| operation.id == continued.id));
    Ok(())
}

#[tokio::test]
async fn pending_create_completes_only_with_durable_resolution_and_health_evidence()
-> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("state.db");
    let id = gascan_core::sandbox::SandboxId::test("evidenced-create");
    let record = SandboxRecord {
        id: id.clone(),
        canonical_root: Utf8PathBuf::from("/pending/evidenced"),
        desired_state: DesiredState::Running,
        actual_state: ActualState::Creating,
        setup_resolution: None,
        tool_resolution: None,
        image_resolution: None,
        storage_resolution: None,
        ssh_resolution: None,
        last_operation_id: None,
        updated_at_millis: 0,
    };
    let store = Store::open(&path)?;
    let pending = store.begin_operation(&record, OperationKind::Create)?;
    store.append_operation_event(
        pending.id,
        json!({"phase":"before_provision","desired_fingerprint":"sha256:test"}),
    )?;
    store.append_operation_event(pending.id, json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":"sha256:test","setup":null,"tools":null}))?;
    store.append_operation_event(pending.id, json!({"phase":"before_health"}))?;
    store.append_operation_event(
        pending.id,
        json!({"phase":"after_health","desired_fingerprint":"sha256:test"}),
    )?;
    let runtime = FakeRuntime::default();
    runtime.seed_owned(id.clone()).await;
    runtime.start(&id).await?;
    let service = SandboxService::new(runtime, store, Arc::new(NoopProvisioner));
    service.reconcile().await?;
    assert_eq!(
        service
            .store()
            .latest_operation()?
            .ok_or("operation")?
            .status,
        OperationStatus::Completed
    );
    Ok(())
}

#[tokio::test]
async fn pending_create_rejects_out_of_order_hook_evidence() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let id = gascan_core::sandbox::SandboxId::test("out-of-order");
    let record = SandboxRecord {
        id: id.clone(),
        canonical_root: Utf8PathBuf::from("/pending/out-of-order"),
        desired_state: DesiredState::Running,
        actual_state: ActualState::Creating,
        setup_resolution: None,
        tool_resolution: None,
        image_resolution: None,
        storage_resolution: None,
        ssh_resolution: None,
        last_operation_id: None,
        updated_at_millis: 0,
    };
    let store = Store::open(temp.path().join("state.db"))?;
    let pending = store.begin_operation(&record, OperationKind::Create)?;
    for phase in [
        json!({"phase":"after_health","desired_fingerprint":"sha256:test"}),
        json!({"phase":"before_provision","desired_fingerprint":"sha256:test"}),
        json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":"sha256:test","setup":null,"tools":null}),
        json!({"phase":"before_health"}),
    ] {
        store.append_operation_event(pending.id, phase)?;
    }
    let runtime = FakeRuntime::default();
    runtime.seed_owned(id.clone()).await;
    runtime.start(&id).await?;
    let service = SandboxService::new(runtime, store, Arc::new(NoopProvisioner));
    service.reconcile().await?;
    assert_eq!(
        service
            .store()
            .latest_operation()?
            .ok_or("operation")?
            .status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn provision_and_health_kill_point_phase_matrix_has_exact_recovery_status()
-> Result<(), Box<dyn Error>> {
    for (label, target, delay_ms, expected) in [
        (
            "before-provision",
            "before_provision",
            0,
            OperationStatus::Failed,
        ),
        (
            "during-provision",
            "before_provision",
            50,
            OperationStatus::Failed,
        ),
        (
            "after-provision",
            "after_provision",
            0,
            OperationStatus::Failed,
        ),
        ("before-health", "before_health", 0, OperationStatus::Failed),
        (
            "during-health",
            "before_health",
            50,
            OperationStatus::Failed,
        ),
        (
            "after-health",
            "after_health",
            0,
            OperationStatus::Completed,
        ),
    ] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("state.db");
        let status = Command::new(std::env::current_exe()?)
            .args(["--exact", "hook_phase_crash_child"])
            .env("GASCAN_HOOK_CRASH_DB", &path)
            .env("GASCAN_HOOK_CRASH_LABEL", label)
            .env("GASCAN_HOOK_CRASH_TARGET", target)
            .env("GASCAN_HOOK_CRASH_DELAY_MS", delay_ms.to_string())
            .status()?;
        assert_eq!(
            status.signal(),
            Some(6),
            "child must terminate via SIGABRT at the kill point"
        );
        let store = Store::open(&path)?;
        let id = store
            .list_sandboxes()?
            .into_iter()
            .next()
            .ok_or("sandbox")?
            .id;
        let runtime = FakeRuntime::default();
        runtime.seed_owned(id.clone()).await;
        runtime.start(&id).await?;
        let service = SandboxService::new(runtime, store, Arc::new(NoopProvisioner));
        service.reconcile().await?;
        assert_eq!(
            service
                .store()
                .latest_operation()?
                .ok_or("operation")?
                .status,
            expected,
            "{label}"
        );
    }
    Ok(())
}

#[test]
fn hook_phase_crash_child() -> Result<(), Box<dyn Error>> {
    let Ok(path) = std::env::var("GASCAN_HOOK_CRASH_DB") else {
        return Ok(());
    };
    let label = std::env::var("GASCAN_HOOK_CRASH_LABEL")?;
    let target = std::env::var("GASCAN_HOOK_CRASH_TARGET")?;
    let delay_ms = std::env::var("GASCAN_HOOK_CRASH_DELAY_MS")?.parse::<u64>()?;
    let db_path = std::path::PathBuf::from(&path);
    std::thread::spawn(move || {
        loop {
            if let Ok(connection) = rusqlite::Connection::open(&db_path) {
                let found = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM operation_events WHERE json_extract(details, '$.phase') = ?1)",
                    [&target],
                    |row| row.get::<_, bool>(0),
                ).unwrap_or(false);
                if found {
                    if delay_ms > 0 {
                        std::thread::sleep(Duration::from_millis(delay_ms));
                    }
                    std::process::abort();
                }
            }
            std::thread::yield_now();
        }
    });
    let root = std::path::Path::new(&path).parent().ok_or("db parent")?;
    let root = camino::Utf8Path::from_path(root).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root(&label, root, Manifest::load(root)?)?;
    let service = SandboxService::new(
        FakeRuntime::default(),
        Store::open(path)?,
        Arc::new(SlowProvisioner),
    );
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let _ = runtime.block_on(service.up(UpRequest::new(spec)));
    Err("service completed before crash watcher fired".into())
}

struct SlowProvisioner;

#[async_trait]
impl Provisioner for SlowProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        std::thread::sleep(Duration::from_millis(150));
        Ok(ProvisionResolution {
            setup: Some(json!({"blob":"x".repeat(2_000_000)})),
            tools: None,
        })
    }
    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }
}

#[tokio::test]
async fn ssh_reconcile_reconstructs_owned_running_verified_alias_from_inspected_mapping()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )?;
    let spec = SandboxSpec::from_root("ssh-restart", root, Manifest::load(root)?)?;
    let state_path = root.join("state.db");
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
    runtime.queue_created_ssh_host_port(25_001).await;
    let first = ssh_service(
        runtime.clone(),
        Store::open(&state_path)?,
        paths.clone(),
        readiness_program(root, None)?,
    );
    first.up(UpRequest::new(spec.clone())).await?;
    std::fs::remove_file(paths.config())?;
    drop(first);

    let restarted = ssh_service(
        runtime,
        Store::open(&state_path)?,
        paths.clone(),
        readiness_program(root, None)?,
    );
    let report = restarted.reconcile().await?;

    assert!(!report.findings.iter().any(
        |finding| matches!(finding, ReconcileFinding::SshUnavailable { sandbox_id, .. } if sandbox_id == spec.id())
    ));
    let config = std::fs::read_to_string(paths.config())?;
    assert!(config.contains(&format!("Host gascan-{}", spec.id())));
    assert!(config.contains("    Port 25001\n"));
    Ok(())
}

#[tokio::test]
async fn ssh_reconcile_publishes_one_complete_generation_and_isolates_broken_records() -> TestResult
{
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )?;
    let valid_spec = SandboxSpec::from_root("ssh-valid", root, Manifest::load(root)?)?;
    let state_path = root.join("state.db");
    let paths = ssh_paths(root, "ssh-matrix-client")?;
    let host_paths = ssh_paths(root, "ssh-matrix-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    runtime.queue_created_ssh_host_port(25_001).await;
    let first = ssh_service(
        runtime.clone(),
        Store::open(&state_path)?,
        paths.clone(),
        readiness_program(root, None)?,
    );
    first.up(UpRequest::new(valid_spec.clone())).await?;
    let valid_record = first.status(valid_spec.id())?.ok_or("valid record")?;
    let verified = valid_record
        .ssh_resolution
        .clone()
        .ok_or("verified SSH resolution")?;
    std::fs::remove_file(paths.config())?;
    drop(first);

    let store = Store::open(&state_path)?;
    for (name, actual, resolution, ports, running) in [
        (
            "ssh-broken",
            ActualState::Running,
            verified.clone(),
            vec![ssh_mapping(25_005)],
            true,
        ),
        (
            "ssh-disabled",
            ActualState::Running,
            SshResolution::new(
                1,
                json!({
                    "enabled": false,
                    "host_key_fingerprint": "",
                    "client_key_fingerprint": "",
                }),
            ),
            vec![ssh_mapping(25_006)],
            true,
        ),
        (
            "ssh-malformed",
            ActualState::Running,
            verified.clone(),
            vec![ssh_mapping(25_007), ssh_mapping(25_008)],
            true,
        ),
        (
            "ssh-offline",
            ActualState::Running,
            SshResolution::new(
                1,
                json!({
                    "enabled": false,
                    "host_key_fingerprint": "",
                    "client_key_fingerprint": "",
                }),
            ),
            Vec::new(),
            true,
        ),
        (
            "ssh-stopped",
            ActualState::Stopped,
            verified.clone(),
            vec![ssh_mapping(25_009)],
            false,
        ),
    ] {
        let id = gascan_core::sandbox::SandboxId::test(name);
        store.put_sandbox(&SandboxRecord {
            id: id.clone(),
            canonical_root: Utf8PathBuf::from(format!("/fixtures/{name}")),
            desired_state: DesiredState::Running,
            actual_state: actual,
            setup_resolution: None,
            tool_resolution: None,
            image_resolution: valid_record.image_resolution.clone(),
            storage_resolution: valid_record.storage_resolution.clone(),
            ssh_resolution: Some(resolution),
            last_operation_id: None,
            updated_at_millis: 0,
        })?;
        runtime.seed_owned(id.clone()).await;
        runtime.set_sandbox_ports(&id, ports).await?;
        if running {
            runtime.start(&id).await?;
        }
    }
    runtime
        .queue_exec_results([
            (format!("{host_public_key}\n").into_bytes(), Vec::new(), 0),
            (format!("{host_public_key}\n").into_bytes(), Vec::new(), 0),
        ])
        .await;
    let restarted = ssh_service(
        runtime,
        store,
        paths.clone(),
        readiness_program(root, Some(25_005))?,
    );

    let report = restarted.reconcile().await?;

    assert!(report.findings.iter().any(
        |finding| matches!(finding, ReconcileFinding::SshUnavailable { sandbox_id, .. } if sandbox_id.as_str().starts_with("ssh-broken-"))
    ));
    assert!(report.findings.iter().any(
        |finding| matches!(finding, ReconcileFinding::SshUnavailable { sandbox_id, .. } if sandbox_id.as_str().starts_with("ssh-malformed-"))
    ));
    let config = std::fs::read_to_string(paths.config())?;
    assert!(config.contains("Host gascan-ssh-valid"));
    for unpublished in [
        "ssh-broken",
        "ssh-disabled",
        "ssh-malformed",
        "ssh-offline",
        "ssh-stopped",
    ] {
        assert!(
            !config.contains(&format!("Host gascan-{unpublished}")),
            "{unpublished} was unexpectedly published:\n{config}"
        );
    }
    assert_eq!(config.matches("\nHost ").count(), 1);
    Ok(())
}

#[tokio::test]
async fn ssh_reconcile_continues_after_one_record_inspect_failure() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "ssh-inspect-isolation")?;
    let host_paths = ssh_paths(root, "ssh-inspect-isolation-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let store = Store::open(root.join("state.db"))?;
    let service = ssh_service(
        runtime.clone(),
        store,
        paths.clone(),
        readiness_program(root, None)?,
    );
    let broken_root = root.join("broken-project");
    let valid_root = root.join("valid-project");
    std::fs::create_dir(&broken_root)?;
    std::fs::create_dir(&valid_root)?;
    std::fs::write(
        broken_root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 25201\n",
    )?;
    let broken = SandboxSpec::from_root(
        "aaa-inspect-broken",
        &broken_root,
        Manifest::load(&broken_root)?,
    )?;
    service.up(UpRequest::new(broken.clone())).await?;
    std::fs::write(
        valid_root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 25202\n",
    )?;
    let valid = SandboxSpec::from_root(
        "zzz-inspect-valid",
        &valid_root,
        Manifest::load(&valid_root)?,
    )?;
    service.up(UpRequest::new(valid.clone())).await?;
    std::fs::remove_file(paths.config())?;
    runtime.inject_failure(FailureBoundary::Inspect).await;

    let report = service.reconcile().await?;

    assert!(report.findings.iter().any(
        |finding| matches!(finding, ReconcileFinding::InspectionUnavailable { sandbox_id, .. } if sandbox_id == broken.id())
    ));
    let config = std::fs::read_to_string(paths.config())?;
    assert!(!config.contains(&format!("Host gascan-{}", broken.id())));
    assert!(config.contains(&format!("Host gascan-{}", valid.id())));
    Ok(())
}

#[tokio::test]
async fn empty_ssh_publication_failure_is_a_sanitized_finding() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    let paths = ssh_paths(root, "empty-publication-client")?;
    let host_paths = ssh_paths(root, "empty-publication-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let project = root.join("project");
    std::fs::create_dir(&project)?;
    std::fs::write(
        project.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 25251\n",
    )?;
    let desired = SandboxSpec::from_root("empty-publication", &project, Manifest::load(&project)?)?;
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let store = Store::open(root.join("state.db"))?;
    let service = ssh_service(
        runtime,
        store.clone(),
        paths.clone(),
        readiness_program(root, None)?,
    );
    service.up(UpRequest::new(desired.clone())).await?;
    store.update_ssh_resolution(
        desired.id(),
        SshResolution::new(
            1,
            json!({
                "enabled": false,
                "host_key_fingerprint": "",
                "client_key_fingerprint": "",
            }),
        ),
    )?;
    std::fs::remove_file(paths.config())?;
    let victim = root.join("publication-victim");
    std::fs::write(&victim, "unchanged")?;
    std::os::unix::fs::symlink(&victim, paths.config())?;

    let report = service.reconcile().await?;

    assert!(
        report.findings.iter().any(|finding| matches!(
            finding,
            ReconcileFinding::SshPublicationUnavailable { reason }
                if reason == "ssh_config_update_failed"
        )),
        "{:?}",
        report.findings
    );
    assert_eq!(std::fs::read_to_string(victim)?, "unchanged");
    assert!(
        !format!("{:?}", report.findings).contains(root.as_str()),
        "finding leaked a host path"
    );
    Ok(())
}

async fn assert_reconcile_bounds_host_key_hang(phase: FakeExecHangPhase) -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 25301\n",
    )?;
    let desired = SandboxSpec::from_root("ssh-host-key-hang", root, Manifest::load(root)?)?;
    let paths = ssh_paths(root, "ssh-host-key-hang-client")?;
    let host_paths = ssh_paths(root, "ssh-host-key-hang-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let store = Store::open(root.join("state.db"))?;
    let first = ssh_service(
        runtime.clone(),
        store.clone(),
        paths.clone(),
        readiness_program(root, None)?,
    );
    first.up(UpRequest::new(desired.clone())).await?;
    std::fs::remove_file(paths.config())?;
    drop(first);
    runtime.queue_exec_hang(phase).await;
    let restarted = SandboxService::new_with_ssh_timeouts_for_tests(
        runtime.clone(),
        store,
        Arc::new(NoopProvisioner),
        paths,
        readiness_program(root, None)?,
        Duration::from_millis(75),
    );

    let started = Instant::now();
    let report = restarted.reconcile().await?;

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{phase:?} host-key read was not bounded"
    );
    assert!(report.findings.iter().any(
        |finding| matches!(finding, ReconcileFinding::SshUnavailable { sandbox_id, .. } if sandbox_id.as_str() == desired.id().as_str())
    ));
    if phase != FakeExecHangPhase::Start {
        tokio::time::timeout(Duration::from_secs(1), async {
            while runtime.exec_cancellations().await == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| format!("{phase:?} session cancellation was not observed"))?;
        assert!(runtime.exec_cancellations().await >= 1);
    }
    Ok(())
}

#[tokio::test]
async fn ssh_reconcile_bounds_host_key_exec_startup_hang() -> TestResult {
    assert_reconcile_bounds_host_key_hang(FakeExecHangPhase::Start).await
}

#[tokio::test]
async fn ssh_reconcile_bounds_host_key_input_close_hang() -> TestResult {
    assert_reconcile_bounds_host_key_hang(FakeExecHangPhase::Close).await
}

#[tokio::test]
async fn ssh_reconcile_bounds_host_key_output_collection_hang() -> TestResult {
    assert_reconcile_bounds_host_key_hang(FakeExecHangPhase::Output).await
}

#[tokio::test]
async fn ssh_reconcile_bounds_host_key_cancellation_drain_hang() -> TestResult {
    assert_reconcile_bounds_host_key_hang(FakeExecHangPhase::Drain).await
}

#[tokio::test]
async fn ssh_reconcile_without_enabled_records_clears_stale_managed_aliases() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).ok_or("utf8 root")?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n[ssh]\nhost_port = 25401\n",
    )?;
    let desired = SandboxSpec::from_root("ssh-stale-alias", root, Manifest::load(root)?)?;
    let paths = ssh_paths(root, "ssh-stale-alias-client")?;
    let host_paths = ssh_paths(root, "ssh-stale-alias-host")?;
    let host_public_key = ensure_host_identity(&host_paths)
        .await?
        .public_key()
        .to_owned();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(format!("{host_public_key}\n").into_bytes(), Vec::new(), 0)
        .await;
    let service = ssh_service(
        runtime,
        Store::open(root.join("state.db"))?,
        paths.clone(),
        readiness_program(root, None)?,
    );
    service.up(UpRequest::new(desired.clone())).await?;
    assert!(
        std::fs::read_to_string(paths.config())?.contains(&format!("Host gascan-{}", desired.id()))
    );
    let mut record = service.status(desired.id())?.ok_or("record")?;
    record.ssh_resolution = Some(SshResolution::new(
        1,
        json!({
            "enabled": false,
            "host_key_fingerprint": "",
            "client_key_fingerprint": "",
        }),
    ));
    service.store().put_sandbox(&record)?;

    service.reconcile().await?;

    let config = std::fs::read_to_string(paths.config())?;
    assert!(!config.contains("\nHost "));
    Ok(())
}
