use camino::Utf8PathBuf;
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::runtime::{ResourceOwnership, RuntimeBackend};
use gascand::{
    ActualState, DesiredState, NoopProvisioner, OperationKind, OperationStatus, ReconcileFinding,
    SandboxRecord, SandboxService, Store,
};
use std::error::Error;
use std::sync::Arc;

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
        assert!(matches!(
            operation.status,
            OperationStatus::Completed | OperationStatus::Failed
        ));
    }
    Ok(())
}
