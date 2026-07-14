use camino::Utf8PathBuf;
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::runtime::{ResourceOwnership, RuntimeBackend};
use gascand::{
    ActualState, DesiredState, NoopProvisioner, OperationKind, OperationStatus, ReconcileFinding,
    SandboxRecord, SandboxService, Store,
};
use serde_json::json;
use std::error::Error;
use std::process::Command;
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
    };
    let store = Store::open(&path)?;
    let pending = store.begin_operation(&record, OperationKind::Create)?;
    store.append_operation_event(pending.id, json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":"sha256:test","setup":null,"tools":null}))?;
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
async fn provision_and_health_kill_point_phase_matrix_has_exact_recovery_status()
-> Result<(), Box<dyn Error>> {
    for (label, phases, expected) in [
        (
            "before-provision",
            vec![json!({"phase":"before_provision","desired_fingerprint":"sha256:test"})],
            OperationStatus::Failed,
        ),
        (
            "after-provision",
            vec![
                json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":"sha256:test","setup":null,"tools":null}),
            ],
            OperationStatus::Failed,
        ),
        (
            "before-health",
            vec![
                json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":"sha256:test","setup":null,"tools":null}),
                json!({"phase":"before_health"}),
            ],
            OperationStatus::Failed,
        ),
        (
            "after-health",
            vec![
                json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":"sha256:test","setup":null,"tools":null}),
                json!({"phase":"after_health","desired_fingerprint":"sha256:test"}),
            ],
            OperationStatus::Completed,
        ),
    ] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("state.db");
        let id = gascan_core::sandbox::SandboxId::test(label);
        let phase_json = serde_json::to_string(&phases)?;
        let status = Command::new(std::env::current_exe()?)
            .args(["--exact", "hook_phase_crash_child"])
            .env("GASCAN_HOOK_CRASH_DB", &path)
            .env("GASCAN_HOOK_CRASH_LABEL", label)
            .env("GASCAN_HOOK_CRASH_PHASES", phase_json)
            .status()?;
        assert!(!status.success(), "child must terminate at the kill point");
        let store = Store::open(&path)?;
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
    let phases: Vec<serde_json::Value> =
        serde_json::from_str(&std::env::var("GASCAN_HOOK_CRASH_PHASES")?)?;
    let id = gascan_core::sandbox::SandboxId::test(&label);
    let record = SandboxRecord {
        id,
        canonical_root: Utf8PathBuf::from(format!("/pending/{label}")),
        desired_state: DesiredState::Running,
        actual_state: ActualState::Creating,
        setup_resolution: None,
        tool_resolution: None,
        image_resolution: None,
    };
    let store = Store::open(path)?;
    let pending = store.begin_operation(&record, OperationKind::Create)?;
    for phase in phases {
        store.append_operation_event(pending.id, phase)?;
    }
    std::process::abort();
}
