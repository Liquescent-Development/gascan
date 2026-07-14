use async_trait::async_trait;
use camino::Utf8PathBuf;
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::manifest::Manifest;
use gascan_core::runtime::{ResourceOwnership, RuntimeBackend};
use gascan_core::sandbox::SandboxSpec;
use gascand::{
    ActualState, DesiredState, NoopProvisioner, OperationKind, OperationStatus, ProvisionRequest,
    ProvisionResolution, Provisioner, ReconcileFinding, SandboxRecord, SandboxService,
    ServiceError, Store, UpRequest,
};
use serde_json::json;
use std::error::Error;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn reconcile_reports_unknown_owned_resources_without_deleting() -> Result<(), Box<dyn Error>>
{
    let temp = tempfile::tempdir()?;
    let runtime = FakeRuntime::default();
    let unknown = gascan_core::sandbox::SandboxId::test("unknown");
    runtime.seed_owned(unknown.clone()).await;
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
