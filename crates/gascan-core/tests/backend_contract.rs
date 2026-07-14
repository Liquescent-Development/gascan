use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::runtime::{
    CreateRequest, ExecRequest, RuntimeBackend, RuntimeCall, RuntimeCapabilities, RuntimeError,
    RuntimeVersion,
};
use gascan_core::sandbox::SandboxId;

fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 0, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: gascan_core::runtime::NetworkIsolation::Proven,
    }
}

pub async fn backend_contract(backend: &dyn RuntimeBackend) {
    let id = SandboxId::test("contract");
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
    backend
        .create(CreateRequest::fixture(id.clone()))
        .await
        .unwrap();
    assert_eq!(
        backend.inspect(&id).await.unwrap().unwrap().state,
        gascan_core::runtime::ContainerState::Stopped
    );
    backend.start(&id).await.unwrap();
    assert_eq!(
        backend
            .exec(ExecRequest::fixture(id.clone(), ["true"]))
            .await
            .unwrap()
            .exit_code(),
        0
    );
    backend.stop(&id).await.unwrap();
    backend.remove(&id).await.unwrap();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
}

#[tokio::test]
async fn fake_runtime_satisfies_backend_contract_through_trait_object() {
    let backend: Box<dyn RuntimeBackend> = Box::new(FakeRuntime::new(capabilities()));
    backend_contract(backend.as_ref()).await;
}

#[tokio::test]
async fn duplicate_create_is_rejected_and_start_stop_are_idempotent() {
    let backend = FakeRuntime::new(capabilities());
    let id = SandboxId::test("lifecycle");
    let request = CreateRequest::fixture(id.clone());
    backend.create(request.clone()).await.unwrap();
    let error = backend.create(request).await.unwrap_err();
    assert_eq!(error.code(), "resource_conflict");

    backend.start(&id).await.unwrap();
    backend.start(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
}

#[tokio::test]
async fn owned_listing_filters_unowned_resources() {
    let backend = FakeRuntime::new(capabilities());
    let owned = SandboxId::test("owned");
    backend
        .create(CreateRequest::fixture(owned.clone()))
        .await
        .unwrap();
    let foreign = SandboxId::test("foreign");
    backend.seed_unowned(foreign.clone()).await;

    let foreign_runtime = backend.inspect(&foreign).await.unwrap().unwrap();
    assert_eq!(foreign_runtime.id, foreign);
    assert_ne!(foreign_runtime.ownership.managed_by, "gascan");

    let resources = backend.list_owned().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].id, owned);
}

#[tokio::test]
async fn exec_and_logs_preserve_binary_bytes_and_exact_exit_code() {
    let backend = FakeRuntime::new(capabilities());
    let id = SandboxId::test("binary");
    backend
        .create(CreateRequest::fixture(id.clone()))
        .await
        .unwrap();
    backend.start(&id).await.unwrap();
    backend
        .set_exec_result(vec![0, 255, 1], vec![254, 0], 42)
        .await;
    backend.set_logs(vec![0, 255, 10]).await;

    let session = backend
        .exec(ExecRequest::fixture(id.clone(), ["binary-command"]))
        .await
        .unwrap();
    assert_eq!(session.stdout(), &[0, 255, 1]);
    assert_eq!(session.stderr(), &[254, 0]);
    assert_eq!(session.exit_code(), 42);
    assert_eq!(backend.logs(&id).await.unwrap(), vec![0, 255, 10]);
}

#[tokio::test]
async fn literal_requests_are_recorded_in_order() {
    let backend = FakeRuntime::new(capabilities());
    let id = SandboxId::test("recording");
    let create = CreateRequest::fixture(id.clone());
    let exec = ExecRequest::fixture(id.clone(), ["printf", "%s", "literal value"]);
    backend.create(create.clone()).await.unwrap();
    backend.start(&id).await.unwrap();
    backend.exec(exec.clone()).await.unwrap();

    assert_eq!(
        backend.calls().await,
        vec![
            RuntimeCall::Create(create),
            RuntimeCall::Start(id.clone()),
            RuntimeCall::Exec(exec),
        ]
    );
}

#[tokio::test]
async fn named_failure_is_injected_once_at_the_call_boundary() {
    let backend = FakeRuntime::failing_once(FailureBoundary::Start);
    let id = SandboxId::test("failure");
    backend
        .create(CreateRequest::fixture(id.clone()))
        .await
        .unwrap();

    let error = backend.start(&id).await.unwrap_err();
    assert_eq!(error.code(), "injected_failure");
    backend.start(&id).await.unwrap();
}

#[tokio::test]
async fn every_backend_boundary_supports_fail_once_injection() {
    for boundary in [
        FailureBoundary::Capabilities,
        FailureBoundary::Inspect,
        FailureBoundary::Create,
    ] {
        let backend = FakeRuntime::failing_once(boundary);
        let id = SandboxId::test(boundary.as_str());
        let error = match boundary {
            FailureBoundary::Capabilities => backend.capabilities().await.unwrap_err(),
            FailureBoundary::Inspect => backend.inspect(&id).await.unwrap_err(),
            FailureBoundary::Create => backend
                .create(CreateRequest::fixture(id))
                .await
                .unwrap_err(),
            _ => continue,
        };
        assert_eq!(error.code(), "injected_failure");
    }

    for boundary in [
        FailureBoundary::Start,
        FailureBoundary::Stop,
        FailureBoundary::Remove,
        FailureBoundary::Exec,
        FailureBoundary::Logs,
        FailureBoundary::ListOwned,
    ] {
        let backend = FakeRuntime::failing_once(boundary);
        let id = SandboxId::test(boundary.as_str());
        backend
            .create(CreateRequest::fixture(id.clone()))
            .await
            .unwrap();
        if matches!(boundary, FailureBoundary::Stop | FailureBoundary::Exec) {
            backend.start(&id).await.unwrap();
        }
        let error = match boundary {
            FailureBoundary::Start => backend.start(&id).await.unwrap_err(),
            FailureBoundary::Stop => backend.stop(&id).await.unwrap_err(),
            FailureBoundary::Remove => backend.remove(&id).await.unwrap_err(),
            FailureBoundary::Exec => backend
                .exec(ExecRequest::fixture(id.clone(), ["true"]))
                .await
                .unwrap_err(),
            FailureBoundary::Logs => backend.logs(&id).await.unwrap_err(),
            FailureBoundary::ListOwned => backend.list_owned().await.unwrap_err(),
            _ => continue,
        };
        assert_eq!(error.code(), "injected_failure");
    }
}

#[test]
fn runtime_errors_have_stable_codes() {
    let error = RuntimeError::OwnershipMismatch {
        resource: "fixture".to_owned(),
    };
    assert_eq!(error.code(), "ownership_mismatch");

    let error = RuntimeError::UnknownActualState {
        resource: "fixture".to_owned(),
        state: "paused-by-future-runtime".to_owned(),
    };
    assert_eq!(error.code(), "unknown_actual_state");
}
