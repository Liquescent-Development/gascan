use async_trait::async_trait;
use camino::Utf8Path;
use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeCall};
use gascan_core::sandbox::SandboxSpec;
use gascand::{NoopProvisioner, OperationStatus, SandboxService, UpRequest};
use gascand::{ProvisionRequest, ProvisionResolution, Provisioner, ServiceError};
use serde_json::json;
use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct ControlledProvisioner {
    fail_provision: AtomicBool,
    fail_health: AtomicBool,
    provisions: AtomicUsize,
}

#[async_trait]
impl Provisioner for ControlledProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        self.provisions.fetch_add(1, Ordering::SeqCst);
        if self.fail_provision.load(Ordering::SeqCst) {
            return Err(ServiceError::Provision(
                "injected provision failure".to_owned(),
            ));
        }
        Ok(ProvisionResolution {
            setup: Some(json!({"resolved":"prior-setup"})),
            tools: Some(json!({"resolved":"prior-tools"})),
        })
    }

    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        if self.fail_health.load(Ordering::SeqCst) {
            return Err(ServiceError::Provision(
                "injected health failure".to_owned(),
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn failed_initial_up_retry_runs_provision_and_persists_actual_resolution() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("retry-hooks", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    provisioner.fail_provision.store(false, Ordering::SeqCst);
    service.up(UpRequest::new(make_spec()?)).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(provisioner.provisions.load(Ordering::SeqCst), 2);
    let record = service.status(make_spec()?.id())?.ok_or("record")?;
    assert_eq!(
        record
            .setup_resolution
            .as_ref()
            .and_then(|value| value.details.get("resolution")),
        Some(&json!({"resolved":"prior-setup"}))
    );
    Ok(())
}

#[tokio::test]
async fn stopped_apply_that_starts_then_fails_records_running_reality() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("apply-running-reality", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.stop(&id).await?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[tools]\nnode = '22'\n",
    )?;
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Failed
    );
    assert_eq!(
        service.status(&id)?.ok_or("record")?.actual_state,
        gascand::ActualState::Running
    );
    Ok(())
}

#[tokio::test]
async fn unchanged_apply_inspects_and_starts_stopped_runtime_without_rerunning_hooks() -> TestResult
{
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("unchanged-apply", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime.stop(&id).await?;
    service.apply(UpRequest::new(make_spec()?)).await?;
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("runtime")?.state,
        gascan_core::runtime::ContainerState::Running
    );
    assert_eq!(provisioner.provisions.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn up_after_destroy_reprovisions_the_fresh_runtime() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("recreate-hooks", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        FakeRuntime::default(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.destroy(&id).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(provisioner.provisions.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn provision_and_health_failures_roll_back_new_resources() -> TestResult {
    for health in [false, true] {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let spec = SandboxSpec::from_root("hook-failure", root, Manifest::load(root)?)?;
        let id = spec.id().clone();
        let runtime = FakeRuntime::default();
        let provisioner = Arc::new(ControlledProvisioner::default());
        provisioner.fail_provision.store(!health, Ordering::SeqCst);
        provisioner.fail_health.store(health, Ordering::SeqCst);
        let service = SandboxService::new(
            runtime.clone(),
            gascand::Store::open(root.join("state.db"))?,
            provisioner,
        );
        assert!(service.up(UpRequest::new(spec)).await.is_err());
        assert!(runtime.inspect(&id).await?.is_none());
        assert!(runtime.list_resources().await?.is_empty());
        assert!(service.store().pending_operations()?.is_empty());
        let calls = runtime.calls().await;
        let started = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Start(call_id) if call_id == &id))
            .ok_or("start call")?;
        let stopped = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Stop(call_id) if call_id == &id))
            .ok_or("rollback stop call")?;
        let removed = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Remove(_)))
            .ok_or("rollback remove call")?;
        assert!(started < stopped && stopped < removed);
        assert!(matches!(
            &calls[stopped - 1],
            RuntimeCall::Inspect(call_id) if call_id == &id
        ));
    }
    Ok(())
}

#[tokio::test]
async fn rollback_failure_preserves_provision_error_and_stops_before_remove() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root("rollback-diagnostic", root, Manifest::load(root)?)?;
    let id = spec.id().clone();
    let runtime = FakeRuntime::failing_once(FailureBoundary::Remove);
    let provisioner = Arc::new(ControlledProvisioner::default());
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner,
    );

    let error = match service.up(UpRequest::new(spec)).await {
        Ok(_) => return Err("provisioning unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "provisioning failed: injected provision failure; rollback failed: injected failure at remove"
    );
    let calls = runtime.calls().await;
    let stopped = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::Stop(call_id) if call_id == &id))
        .ok_or("rollback stop call")?;
    let removed = calls
        .iter()
        .position(|call| matches!(call, RuntimeCall::Remove(_)))
        .ok_or("rollback remove call")?;
    assert!(stopped < removed);
    assert!(matches!(
        &calls[stopped - 1],
        RuntimeCall::Inspect(call_id) if call_id == &id
    ));
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("retained runtime")?.state,
        gascan_core::runtime::ContainerState::Stopped
    );
    let operation = service.latest_operation()?.ok_or("operation")?;
    assert_eq!(operation.error_code.as_deref(), Some("provision_failed"));
    assert_eq!(
        operation
            .error_details
            .as_ref()
            .ok_or("operation error details")?["message"],
        "provisioning failed: injected provision failure; rollback failed: injected failure at remove"
    );
    Ok(())
}

#[tokio::test]
async fn failed_apply_retains_prior_setup_and_tool_resolutions() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("apply-retain", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        provisioner.clone(),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    let prior = service.status(&id)?.ok_or("record")?;
    provisioner.fail_provision.store(true, Ordering::SeqCst);
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[tools]\nnode = '22'\n",
    )?;

    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    let retained = service.status(&id)?.ok_or("record")?;
    assert_eq!(retained.setup_resolution, prior.setup_resolution);
    assert_eq!(retained.tool_resolution, prior.tool_resolution);
    Ok(())
}

#[tokio::test]
async fn synchronous_runtime_failures_after_begin_are_terminal_not_pending() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("terminal-errors", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.stop(&id).await?;

    runtime.inject_failure(FailureBoundary::Start).await;
    assert!(service.start(&id).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    service.start(&id).await?;
    runtime.inject_failure(FailureBoundary::Stop).await;
    assert!(service.stop(&id).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    runtime.inject_failure(FailureBoundary::Inspect).await;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\n[tools]\nnode = '22'\n",
    )?;
    assert!(service.apply(UpRequest::new(make_spec()?)).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    runtime.inject_failure(FailureBoundary::Remove).await;
    assert!(service.destroy(&id).await.is_err());
    assert!(service.store().pending_operations()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn stopped_up_auto_starts_and_apply_status_list_complete() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("surface", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let provisioner = Arc::new(ControlledProvisioner::default());
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        provisioner,
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime.stop(&id).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(
        runtime.inspect(&id).await?.ok_or("runtime")?.state,
        gascan_core::runtime::ContainerState::Running
    );
    let applied = service.apply(UpRequest::new(make_spec()?)).await?;
    assert_eq!(
        service.store().latest_operation()?.ok_or("operation")?.id,
        applied.id
    );
    assert_eq!(
        service.status(&id)?.ok_or("status")?.actual_state,
        gascand::ActualState::Running
    );
    assert!(service.list()?.iter().any(|record| record.id == id));
    Ok(())
}

#[tokio::test]
async fn event_stream_matches_ordered_durable_events_and_receiver_drop_does_not_deadlock()
-> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("events", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let service = SandboxService::new(
        FakeRuntime::default(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let mut operation = service.up(UpRequest::new(make_spec()?)).await?;
    let durable = service.store().operation_events(operation.id)?;
    let mut streamed = Vec::new();
    while let Some(event) = operation.events.recv().await {
        streamed.push(event);
    }
    assert_eq!(streamed, durable);
    assert!(
        streamed
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(
        streamed.last().map(|event| event.status),
        Some(OperationStatus::Completed)
    );
    let dropped = service.stop(&id).await?;
    drop(dropped.events);
    service.start(&id).await?;
    Ok(())
}

async fn volume_names(
    runtime: &FakeRuntime,
    spec: SandboxSpec,
) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(
        PolicyCompiler::compile(spec, &runtime.capabilities().await?)?
            .volumes()
            .iter()
            .map(|volume| volume.name.clone())
            .collect(),
    )
}

#[tokio::test]
async fn failed_create_preserves_preexisting_volume_and_removes_only_new_resources() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("rollback-volumes", root, Manifest::load(root)?);
    let runtime = FakeRuntime::failing_once(FailureBoundary::Start);
    let id = make_spec()?.id().clone();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[0], Some(id), ResourceOwnership::GasCanOwned)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    assert!(runtime.volume_exists(&names[0]).await);
    for name in &names[1..] {
        assert!(!runtime.volume_exists(name).await);
    }
    assert!(
        runtime
            .list_resources()
            .await?
            .iter()
            .all(|resource| resource.kind() == ResourceKind::Volume && resource.name() == names[0])
    );
    Ok(())
}

#[tokio::test]
async fn successful_create_accepts_expected_preexisting_owned_volume_without_claiming_it_created()
-> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("preexisting-success", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let id = make_spec()?.id().clone();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[0], Some(id), ResourceOwnership::GasCanOwned)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    assert!(runtime.volume_exists(&names[0]).await);
    Ok(())
}

#[tokio::test]
async fn foreign_volume_collision_is_refused_and_preserved() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("volume-collision", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[0], None, ResourceOwnership::Foreign)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    let error = match service.up(UpRequest::new(make_spec()?)).await {
        Ok(_) => return Err("volume collision unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("volume exists with different ownership")
    );
    assert!(runtime.volume_exists(&names[0]).await);
    Ok(())
}

#[tokio::test]
async fn partial_create_collision_rolls_back_only_resources_created_by_failed_call() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("partial-collision", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let names = volume_names(&runtime, make_spec()?).await?;
    runtime
        .seed_volume(&names[1], None, ResourceOwnership::Foreign)
        .await?;
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    assert!(service.up(UpRequest::new(make_spec()?)).await.is_err());
    assert!(!runtime.volume_exists(&names[0]).await);
    assert!(runtime.volume_exists(&names[1]).await);
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn destroy_removes_exact_owned_resources_and_retains_foreign_inventory() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("destroy-exact", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let id = make_spec()?.id().clone();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime
        .seed_volume(
            "foreign-neighbor",
            Some(id.clone()),
            ResourceOwnership::Foreign,
        )
        .await?;

    service.destroy(&id).await?;

    let inventory = runtime.list_resources().await?;
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].name(), "foreign-neighbor");
    assert_eq!(inventory[0].ownership(), ResourceOwnership::Foreign);
    Ok(())
}

#[tokio::test]
async fn destroy_retains_extra_owned_volume_with_known_sandbox_association() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("destroy-extra", root, Manifest::load(root)?);
    let id = make_spec()?.id().clone();
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    runtime
        .seed_volume(
            "gascan-extra-owned",
            Some(id.clone()),
            ResourceOwnership::GasCanOwned,
        )
        .await?;
    service.destroy(&id).await?;
    assert!(runtime.volume_exists("gascan-extra-owned").await);
    let report = service.reconcile().await?;
    assert!(report.findings.iter().any(|finding| matches!(finding, gascand::ReconcileFinding::UnknownOwned(resource) if resource.name() == "gascan-extra-owned")));
    Ok(())
}

async fn wait_for_start_calls(runtime: &FakeRuntime, expected: usize) -> TestResult {
    for _ in 0..10_000 {
        let count = runtime
            .calls()
            .await
            .iter()
            .filter(|call| matches!(call, gascan_core::runtime::RuntimeCall::Start(_)))
            .count();
        if count >= expected {
            return Ok(());
        }
        tokio::task::yield_now().await;
    }
    Err(format!("timed out waiting for {expected} start calls").into())
}

#[tokio::test]
async fn same_key_mutations_serialize_at_the_runtime_boundary() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let first = SandboxSpec::from_root("same-key", root, Manifest::load(root)?)?;
    let second = SandboxSpec::from_root("same-key", root, Manifest::load(root)?)?;
    let runtime = FakeRuntime::default();
    runtime.gate(FailureBoundary::Start).await;
    let service = Arc::new(SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    let one = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(first)).await }
    });
    wait_for_start_calls(&runtime, 1).await?;
    let two = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(second)).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(
        runtime
            .calls()
            .await
            .iter()
            .filter(|call| matches!(call, gascan_core::runtime::RuntimeCall::Start(_)))
            .count(),
        1
    );
    runtime.release(FailureBoundary::Start, 1).await;
    one.await??;
    two.await??;
    Ok(())
}

#[tokio::test]
async fn different_keys_reach_the_runtime_concurrently() -> TestResult {
    let one_root = tempfile::tempdir()?;
    let two_root = tempfile::tempdir()?;
    let db_root = tempfile::tempdir()?;
    let one_root = Utf8Path::from_path(one_root.path()).ok_or("utf8 root")?;
    let two_root = Utf8Path::from_path(two_root.path()).ok_or("utf8 root")?;
    let one = SandboxSpec::from_root("one", one_root, Manifest::load(one_root)?)?;
    let two = SandboxSpec::from_root("two", two_root, Manifest::load(two_root)?)?;
    let runtime = FakeRuntime::default();
    runtime.gate(FailureBoundary::Start).await;
    let service = Arc::new(SandboxService::new(
        runtime.clone(),
        gascand::Store::open(db_root.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    let first = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(one)).await }
    });
    let second = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(two)).await }
    });
    wait_for_start_calls(&runtime, 2).await?;
    runtime.release(FailureBoundary::Start, 2).await;
    first.await??;
    second.await??;
    Ok(())
}

#[tokio::test]
async fn reconcile_waits_for_live_same_sandbox_mutation_instead_of_terminalizing_it() -> TestResult
{
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root("reconcile-live", root, Manifest::load(root)?)?;
    let runtime = FakeRuntime::default();
    runtime.gate(FailureBoundary::Start).await;
    let service = Arc::new(SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    let up = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(spec)).await }
    });
    wait_for_start_calls(&runtime, 1).await?;
    let reconcile = tokio::spawn({
        let service = service.clone();
        async move { service.reconcile().await }
    });
    tokio::task::yield_now().await;
    assert!(!reconcile.is_finished());
    runtime.release(FailureBoundary::Start, 1).await;
    up.await??;
    reconcile.await??;
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Completed
    );
    Ok(())
}

#[tokio::test]
async fn failed_start_rolls_back_new_sandbox_and_records_failure() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let manifest = Manifest::load(root)?;
    let spec = SandboxSpec::from_root("lifecycle", root, manifest)?;
    let runtime = FakeRuntime::failing_once(FailureBoundary::Start);
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );

    assert!(service.up(UpRequest::new(spec)).await.is_err());
    assert!(
        runtime
            .inspect(&service.list()?.first().ok_or("record")?.id)
            .await?
            .is_none()
    );
    assert_eq!(
        service.latest_operation()?.ok_or("operation")?.status,
        OperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn repeated_up_is_idempotent() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("repeat", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    service.up(UpRequest::new(make_spec()?)).await?;
    service.up(UpRequest::new(make_spec()?)).await?;
    assert_eq!(runtime.created_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn start_stop_destroy_are_idempotent_and_emit_terminal_events() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let make_spec = || SandboxSpec::from_root("states", root, Manifest::load(root)?);
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime,
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    let id = make_spec()?.id().clone();
    service.up(UpRequest::new(make_spec()?)).await?;
    service.stop(&id).await?;
    service.stop(&id).await?;
    let mut started = service.start(&id).await?;
    let mut statuses = Vec::new();
    while let Some(event) = started.events.recv().await {
        statuses.push(event.status);
    }
    assert_eq!(statuses.last(), Some(&OperationStatus::Completed));
    service.start(&id).await?;
    service.destroy(&id).await?;
    service.destroy(&id).await?;
    assert_eq!(
        service.status(&id)?.ok_or("record")?.actual_state,
        gascand::ActualState::Absent
    );
    Ok(())
}

#[tokio::test]
async fn missing_start_stop_and_apply_are_refused_without_runtime_mutation() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let spec = SandboxSpec::from_root("missing", root, Manifest::load(root)?)?;
    let id = spec.id().clone();
    let runtime = FakeRuntime::default();
    let service = SandboxService::new(
        runtime.clone(),
        gascand::Store::open(root.join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    assert!(service.start(&id).await.is_err());
    assert!(service.stop(&id).await.is_err());
    assert!(service.apply(UpRequest::new(spec)).await.is_err());
    assert!(service.destroy(&id).await.is_err());
    assert_eq!(runtime.created_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn keyed_lock_registry_does_not_retain_finished_sandbox_keys() -> TestResult {
    let db = tempfile::tempdir()?;
    let service = SandboxService::new(
        FakeRuntime::default(),
        gascand::Store::open(db.path().join("state.db"))?,
        Arc::new(NoopProvisioner),
    );
    for index in 0..64 {
        let root = tempfile::tempdir()?;
        let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
        let spec = SandboxSpec::from_root(&format!("lock-{index}"), root, Manifest::load(root)?)?;
        service.up(UpRequest::new(spec)).await?;
    }
    assert_eq!(service.keyed_lock_count()?, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_sqlite_writer_does_not_block_single_tokio_worker() -> TestResult {
    let root = tempfile::tempdir()?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let path = root.join("state.db");
    let store = gascand::Store::open(&path)?;
    let blocker = rusqlite::Connection::open(&path)?;
    blocker.execute_batch("BEGIN IMMEDIATE")?;
    let spec = SandboxSpec::from_root("blocked-db", root, Manifest::load(root)?)?;
    let service = Arc::new(SandboxService::new(
        FakeRuntime::default(),
        store,
        Arc::new(NoopProvisioner),
    ));
    let operation = tokio::spawn({
        let service = service.clone();
        async move { service.up(UpRequest::new(spec)).await }
    });
    let started = Instant::now();
    tokio::task::yield_now().await;
    let unrelated = Arc::new(AtomicBool::new(false));
    let marker = unrelated.clone();
    tokio::spawn(async move {
        marker.store(true, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert!(unrelated.load(Ordering::SeqCst));
    assert!(started.elapsed() < Duration::from_secs(1));
    blocker.execute_batch("ROLLBACK")?;
    operation.await??;
    Ok(())
}
