mod common;

use common::{capabilities, create_request};
use gascan_core::fake_runtime::{FailureBoundary, FakeRuntime};
use gascan_core::runtime::{
    CreateOutcome, ExecInput, ExecOutput, ExecRequest, RemoveRequest, ResourceIdentity,
    ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeCall, RuntimeError, RuntimeOutcome,
    RuntimeResource,
};
use gascan_core::sandbox::SandboxId;

#[tokio::test]
async fn exec_session_is_live_bidirectional_and_emits_one_exit() {
    let backend = FakeRuntime::new(capabilities());
    let fixture = create_request("live-exec");
    let id = fixture.id().clone();
    backend.create(fixture.request()).await.unwrap();
    backend.start(&id).await.unwrap();
    let mut session = backend
        .exec(ExecRequest::fixture(id, ["fake-echo-stdin"]))
        .await
        .unwrap();
    session
        .send(ExecInput::Stdin(vec![0, 0xff, b'a']))
        .await
        .unwrap();
    session
        .send(ExecInput::Resize {
            columns: 123,
            rows: 45,
        })
        .await
        .unwrap();
    session.send(ExecInput::Signal(15)).await.unwrap();
    session.send(ExecInput::Close).await.unwrap();
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Stdout(vec![0, 0xff, b'a'])
    );
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Exit {
            code: 143,
            signal: 15
        }
    );
    assert!(session.next().await.is_none());
}

#[tokio::test]
async fn persistent_fake_runtime_reopens_runtime_truth_without_controller_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.json");
    let fixture = create_request("persistent-fake");
    let id = fixture.id().clone();
    let backend = FakeRuntime::persistent(capabilities(), &path)
        .await
        .unwrap();
    backend.create(fixture.request()).await.unwrap();
    backend.start(&id).await.unwrap();
    drop(backend);
    let reopened = FakeRuntime::persistent(capabilities(), &path)
        .await
        .unwrap();
    assert_eq!(
        reopened.inspect(&id).await.unwrap().unwrap().state,
        gascan_core::runtime::ContainerState::Running
    );
    assert!(!reopened.list_resources().await.unwrap().is_empty());
}

#[tokio::test]
async fn persistent_logs_are_isolated_by_exact_sandbox_id() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.json");
    let backend = FakeRuntime::persistent(capabilities(), &path)
        .await
        .unwrap();
    let left = create_request("logs-left");
    let right = create_request("logs-right");
    for (fixture, marker) in [(&left, "left-marker"), (&right, "right-marker")] {
        let id = fixture.id().clone();
        backend.create(fixture.request()).await.unwrap();
        backend.start(&id).await.unwrap();
        let mut session = backend
            .exec(ExecRequest::fixture(id, ["fake-stdout", marker]))
            .await
            .unwrap();
        session.send(ExecInput::Close).await.unwrap();
        while session.next().await.is_some() {}
    }
    drop(backend);
    let backend = FakeRuntime::persistent(capabilities(), &path)
        .await
        .unwrap();
    assert_eq!(backend.logs(left.id(), None).await.unwrap(), b"left-marker");
    assert_eq!(
        backend.logs(right.id(), None).await.unwrap(),
        b"right-marker"
    );
}

pub async fn backend_contract(backend: &dyn RuntimeBackend) {
    let fixture = create_request("contract");
    let id = fixture.id().clone();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
    let created = backend.create(fixture.request()).await.unwrap();
    assert!(
        created
            .created()
            .iter()
            .any(|resource| resource.kind() == ResourceKind::Container)
    );
    assert_eq!(
        backend.inspect(&id).await.unwrap().unwrap().state,
        gascan_core::runtime::ContainerState::Stopped
    );
    backend.start(&id).await.unwrap();
    let mut session = backend
        .exec(ExecRequest::fixture(id.clone(), ["true"]))
        .await
        .unwrap();
    session.send(ExecInput::Close).await.unwrap();
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Exit { code: 0, signal: 0 }
    );
    backend.stop(&id).await.unwrap();
    backend
        .remove(RemoveRequest::from_resources(created.created().to_vec()).unwrap())
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
async fn remove_request_refuses_foreign_inventory_resources() {
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

    let error = RemoveRequest::from_resources(vec![resource]).unwrap_err();
    assert_eq!(error.code(), "ownership_mismatch");
    assert!(backend.inspect(&foreign).await.unwrap().is_some());
}

#[tokio::test]
async fn exact_remove_rejects_a_forged_owned_resource_without_inventory_proof() {
    let backend = FakeRuntime::new(capabilities());
    let id = SandboxId::test("forged-remove");
    backend.seed_owned(id.clone()).await;
    let forged_identity = ResourceIdentity::new(ResourceKind::Container, id.to_string()).unwrap();
    let forged = RuntimeResource::discovered(
        forged_identity,
        Some(id.clone()),
        ResourceOwnership::GasCanOwned,
    );

    let error = backend
        .remove(RemoveRequest::from_resources(vec![forged]).unwrap())
        .await
        .unwrap_err();

    assert_eq!(error.code(), "ownership_mismatch");
    assert!(backend.inspect(&id).await.unwrap().is_some());
}

#[test]
fn create_outcome_rejects_resources_not_authorized_by_the_request() {
    let fixture = create_request("outcome-validation");
    let other = SandboxId::test("other-sandbox");
    let identity = ResourceIdentity::new(ResourceKind::Container, other.to_string()).unwrap();
    let resource =
        RuntimeResource::discovered(identity, Some(other), ResourceOwnership::GasCanOwned);

    let error = CreateOutcome::new(&fixture.request(), vec![resource]).unwrap_err();
    assert_eq!(error.code(), "ownership_mismatch");
}

#[test]
fn create_outcome_rejects_duplicate_resource_identities() {
    let fixture = create_request("duplicate-outcome");
    let container = RuntimeResource::discovered(
        ResourceIdentity::new(ResourceKind::Container, fixture.id().to_string()).unwrap(),
        Some(fixture.id().clone()),
        ResourceOwnership::GasCanOwned,
    );
    let error =
        CreateOutcome::new(&fixture.request(), vec![container.clone(), container]).unwrap_err();
    assert_eq!(error.code(), "ownership_mismatch");
}

#[tokio::test]
async fn create_collision_reports_resources_created_before_the_collision() {
    for collision_index in [1, 2] {
        let backend = FakeRuntime::new(capabilities());
        let fixture = create_request(&format!("partial-collision-{collision_index}"));
        let collision = &fixture.volumes()[collision_index];
        backend
            .seed_volume(
                &collision.name,
                Some(SandboxId::test("foreign-volume-owner")),
                ResourceOwnership::Foreign,
            )
            .await
            .unwrap();

        let failure = backend.create(fixture.request()).await.unwrap_err();

        assert_eq!(failure.code(), "resource_conflict");
        assert_eq!(failure.created().len(), collision_index);
        assert_eq!(failure.created()[0].name(), fixture.volumes()[0].name);
    }
}

#[tokio::test]
async fn injected_post_mutation_create_failure_reports_partial_resources() {
    let backend = FakeRuntime::new(capabilities());
    backend.fail_create_after_mutations(2).await;
    let fixture = create_request("partial-injected");

    let failure = backend.create(fixture.request()).await.unwrap_err();

    assert_eq!(failure.code(), "injected_failure");
    assert_eq!(failure.created().len(), 2);
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

    let mut session = backend
        .exec(ExecRequest::fixture(id.clone(), ["binary-command"]))
        .await
        .unwrap();
    session.send(ExecInput::Close).await.unwrap();
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Stdout(vec![0, 255, 1])
    );
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Stderr(vec![254, 0])
    );
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Exit {
            code: 42,
            signal: 0
        }
    );
    assert_eq!(
        backend.logs(&id, None).await.unwrap(),
        vec![0, 255, 10, 0, 255, 1, 254, 0]
    );
    // `since` is inclusive: the fixture record stamped at the exact boundary is retained.
    assert_eq!(
        backend.logs(&id, Some(0)).await.unwrap(),
        vec![0, 255, 10, 0, 255, 1, 254, 0]
    );
    assert!(backend.logs(&id, Some(i64::MAX)).await.unwrap().is_empty());
}

#[tokio::test]
async fn literal_requests_are_recorded_in_order() {
    let backend = FakeRuntime::new(capabilities());
    let fixture = create_request("recording");
    let create = fixture.request();
    let id = create.id().clone();
    let exec = ExecRequest::fixture(id.clone(), ["printf", "%s", "literal value"]);
    let outcome = backend.create(create.clone()).await.unwrap();
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
    assert_eq!(
        backend.outcomes().await,
        vec![RuntimeOutcome::Created(outcome)]
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
                let error = backend.create(fixture.request()).await.unwrap_err();
                assert_eq!(error.code(), "injected_failure");
                continue;
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
                .remove(RemoveRequest::from_resources(created.created().to_vec()).unwrap())
                .await
                .unwrap_err(),
            FailureBoundary::Exec => backend
                .exec(ExecRequest::fixture(id.clone(), ["true"]))
                .await
                .unwrap_err(),
            FailureBoundary::Logs => backend.logs(&id, None).await.unwrap_err(),
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
