use crate::common::{LiveEngine, policy_request, retained_for};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{
    ExecRequest, NetworkIsolation, RecreateRequest, RemoveRequest, ResourceIdentity, ResourceKind,
    ResourceOwnership, RetainedResources, RuntimeBackend, RuntimeResource,
};
use gascan_core::sandbox::SandboxId;

/// The backend over a real engine, not a fake. Everything below goes through
/// ChannelTransport and the real gRPC stack.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn capabilities_report_only_what_this_engine_build_implements() {
    let engine = LiveEngine::start().await;
    let capabilities = backend(&engine).await.capabilities().await.unwrap();

    // Milestone 1 creates nothing and execs nothing, so it claims nothing.
    // Milestone 4 replaces this test with one asserting every flag is true.
    assert!(!capabilities.bind_mounts);
    assert!(!capabilities.named_volumes);
    assert!(!capabilities.tty);
    assert!(!capabilities.signals);
    assert!(!capabilities.loopback_publish);
    assert!(!capabilities.resource_limits);
    assert_eq!(capabilities.offline, NetworkIsolation::Unverified);
}

/// Every method this build does not implement must ANSWER, and every one of
/// them must answer the same way.
///
/// A gRPC status would reach the consumer as an unreachable engine, which is a
/// different fact from "this build cannot do that" and would send a reconciler
/// down the wrong path. That is the PR's claim, and until now it was asserted
/// for one method out of the ten: a comment reading "the eight unimplemented
/// methods must ANSWER" sat above a body that called only `start`. The two
/// streaming methods were the ones that mattered most, because their error
/// arrives in a different message entirely -- `LogsChunk.outcome.error` and
/// `ExecServerFrame.frame.error` -- and nothing in this tier touched either. A
/// regression that made `Logs` answer with a status would have passed.
///
/// Ten, not eight. `Inspect` and `ListResources` joined the list: an engine
/// that loads no state cannot report an absence or an emptiness, and answering
/// `Absent` without having looked is what makes a reconciler create a duplicate
/// of a running sandbox. When a later milestone loads state, they leave this
/// list and get real assertions -- and this test's own count must drop to eight
/// on that day, which is why the count is asserted rather than implied.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn every_unimplemented_method_answers_unsupported_capability_not_a_transport_fault() {
    let engine = LiveEngine::start().await;
    let backend = backend(&engine).await;
    let id = SandboxId::test("never-created");

    // Held for the whole test: the compiled requests name canonical roots that
    // must still exist when the calls are made.
    let (_create_root, create) = policy_request("never-created");
    let (_recreate_root, recreate_create) = policy_request("never-recreated");
    let retained = RetainedResources::new(&recreate_create, retained_for(&recreate_create))
        .expect("the retained set matches the requested topology exactly");
    let recreate = RecreateRequest::new(recreate_create, retained).expect("a recreate request");
    let remove = RemoveRequest::from_resources(vec![RuntimeResource::discovered(
        ResourceIdentity::new(ResourceKind::Volume, "never-created-data")
            .expect("a valid identity"),
        Some(id.clone()),
        ResourceOwnership::GasCanOwned,
    )])
    .expect("one owned resource");

    // Each arm reduces to the wire code, so the ten shapes -- Option, unit,
    // CreateFailure, ExecSession, Vec<u8>, Vec<RuntimeResource> -- become one
    // comparable list. `CreateFailure` carries its code separately from
    // `RuntimeError` because a partial create must report what it made.
    let answers: Vec<(&str, String)> = vec![
        (
            "Inspect",
            backend.inspect(&id).await.expect_err("Inspect").code().to_owned(),
        ),
        (
            "ListResources",
            backend.list_resources().await.expect_err("ListResources").code().to_owned(),
        ),
        (
            "Create",
            backend.create(create).await.expect_err("Create").code().to_owned(),
        ),
        (
            "CreateContainer",
            backend
                .create_container(recreate)
                .await
                .expect_err("CreateContainer")
                .code()
                .to_owned(),
        ),
        (
            "PrepareImage",
            backend
                .prepare_image("registry.example/workspace@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .await
                .expect_err("PrepareImage")
                .code()
                .to_owned(),
        ),
        (
            "Start",
            backend.start(&id).await.expect_err("Start").code().to_owned(),
        ),
        (
            "Stop",
            backend.stop(&id).await.expect_err("Stop").code().to_owned(),
        ),
        (
            "Remove",
            backend.remove(remove).await.expect_err("Remove").code().to_owned(),
        ),
        (
            "Exec",
            {
                // Exec is the one that does not refuse at the call. The session
                // OPENS -- `exec()` returns Ok -- and the refusal arrives as the
                // stream's first frame, because the engine's answer lives in
                // `ExecServerFrame.frame.error` rather than in a response
                // outcome. MEASURED here: an earlier draft of this test called
                // `expect_err` on `exec()` itself and failed with a perfectly
                // healthy `ExecSession`. Anything that asserts against the call
                // and not the frame is testing the wrong half.
                let mut session = backend
                    .exec(ExecRequest::fixture(id.clone(), ["true"]))
                    .await
                    .expect("Exec opens a session; the refusal is its first frame");
                match session.next().await {
                    Some(Err(error)) => error.code().to_owned(),
                    other => panic!("Exec's first frame must be an error, got {other:?}"),
                }
            },
        ),
        (
            "Logs",
            backend.logs(&id, None).await.expect_err("Logs").code().to_owned(),
        ),
    ];

    assert_eq!(
        answers.len(),
        10,
        "this build implements one of the eleven contract methods; \
         a method that becomes real must leave this list"
    );
    for (rpc, code) in &answers {
        assert_eq!(
            code, "unsupported_capability",
            "{rpc} must answer in its outcome, not as a status: got {code}"
        );
    }
}
