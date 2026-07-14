mod common;

use common::{capabilities, create_request};
use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::runtime::{
    ExecRequest, RemoveRequest, ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeCall,
    RuntimeError,
};
use gascan_core::sandbox::SandboxId;

pub async fn backend_contract(backend: &dyn RuntimeBackend) {
    let fixture = create_request("contract");
    let id = fixture.id().clone();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
    let created = backend.create(fixture.request()).await.unwrap();
    assert!(
        created
            .created
            .iter()
            .any(|resource| resource.kind() == ResourceKind::Container)
    );
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
    backend
        .remove(RemoveRequest::from_resources(created.created).unwrap())
        .await
        .unwrap();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
}

#[tokio::test]
async fn inventory_reports_owned_foreign_and_mismatched_resources() {
    let backend = FakeRuntime::new(capabilities());
    let owned = SandboxId::test("owned-inventory");
    let foreign = SandboxId::test("foreign-inventory");
    let mismatch = SandboxId::test("mismatch-inventory");
    backend.seed_owned(owned.clone()).await;
    backend.seed_unowned(foreign.clone()).await;
    backend.seed_mismatched(mismatch.clone()).await;

    let resources = backend.list_resources().await.unwrap();
    assert!(resources.iter().any(|resource| {
        resource.sandbox_id() == Some(&owned)
            && resource.ownership() == ResourceOwnership::GasCanOwned
    }));
    assert!(resources.iter().any(|resource| {
        resource.sandbox_id() == Some(&foreign)
            && resource.ownership() == ResourceOwnership::Foreign
    }));
    assert!(resources.iter().any(|resource| {
        resource.sandbox_id() == Some(&mismatch)
            && resource.ownership() == ResourceOwnership::Mismatched
    }));
}

#[tokio::test]
async fn exact_remove_revalidates_ownership_and_refuses_foreign_resources() {
    let backend = FakeRuntime::new(capabilities());
    let foreign = SandboxId::test("foreign-remove");
    backend.seed_unowned(foreign.clone()).await;
    let resource = backend
        .list_resources()
        .await
        .unwrap()
        .into_iter()
        .find(|resource| resource.sandbox_id() == Some(&foreign))
        .unwrap();

    let error = backend
        .remove(RemoveRequest::from_resources(vec![resource]).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "foreign_resource_refused");
    assert!(backend.inspect(&foreign).await.unwrap().is_some());
}

#[tokio::test]
async fn fake_runtime_satisfies_backend_contract_through_trait_object() {
    let backend: Box<dyn RuntimeBackend> = Box::new(FakeRuntime::new(capabilities()));
    backend_contract(backend.as_ref()).await;
}

#[test]
fn validated_fixture_keeps_its_canonical_bind_source_alive() {
    let fixture = create_request("live-root");

    assert!(fixture.bind_mounts()[0].source.exists());
}

#[tokio::test]
async fn duplicate_create_is_rejected_and_start_stop_are_idempotent() {
    let backend = FakeRuntime::new(capabilities());
    let fixture = create_request("lifecycle");
    let id = fixture.id().clone();
    backend.create(fixture.request()).await.unwrap();
    let error = backend.create(fixture.request()).await.unwrap_err();
    assert_eq!(error.code(), "resource_conflict");

    backend.start(&id).await.unwrap();
    backend.start(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
}

#[tokio::test]
async fn inventory_keeps_unowned_resources_observable() {
    let backend = FakeRuntime::new(capabilities());
    let fixture = create_request("owned");
    let owned = fixture.id().clone();
    backend.create(fixture.request()).await.unwrap();
    let foreign = SandboxId::test("foreign");
    backend.seed_unowned(foreign.clone()).await;

    let foreign_runtime = backend.inspect(&foreign).await.unwrap().unwrap();
    assert_eq!(foreign_runtime.id, foreign);
    assert_ne!(foreign_runtime.ownership.managed_by, "gascan");

    let resources = backend.list_resources().await.unwrap();
    assert!(
        resources
            .iter()
            .any(|resource| resource.sandbox_id() == Some(&owned))
    );
    assert!(
        resources
            .iter()
            .any(|resource| resource.sandbox_id() == Some(&foreign))
    );
}

#[tokio::test]
async fn exec_and_logs_preserve_binary_bytes_and_exact_exit_code() {
    let backend = FakeRuntime::new(capabilities());
    let fixture = create_request("binary");
    let id = fixture.id().clone();
    backend.create(fixture.request()).await.unwrap();
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
    let fixture = create_request("recording");
    let create = fixture.request();
    let id = create.id().clone();
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
    let fixture = create_request("failure");
    let id = fixture.id().clone();
    backend.create(fixture.request()).await.unwrap();

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
            FailureBoundary::Create => {
                let fixture = create_request(boundary.as_str());
                backend.create(fixture.request()).await.unwrap_err()
            }
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
        FailureBoundary::ListResources,
    ] {
        let backend = FakeRuntime::failing_once(boundary);
        let fixture = create_request(boundary.as_str());
        let id = fixture.id().clone();
        let created = backend.create(fixture.request()).await.unwrap();
        if matches!(boundary, FailureBoundary::Stop | FailureBoundary::Exec) {
            backend.start(&id).await.unwrap();
        }
        let error = match boundary {
            FailureBoundary::Start => backend.start(&id).await.unwrap_err(),
            FailureBoundary::Stop => backend.stop(&id).await.unwrap_err(),
            FailureBoundary::Remove => backend
                .remove(RemoveRequest::from_resources(created.created).unwrap())
                .await
                .unwrap_err(),
            FailureBoundary::Exec => backend
                .exec(ExecRequest::fixture(id.clone(), ["true"]))
                .await
                .unwrap_err(),
            FailureBoundary::Logs => backend.logs(&id).await.unwrap_err(),
            FailureBoundary::ListResources => backend.list_resources().await.unwrap_err(),
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
