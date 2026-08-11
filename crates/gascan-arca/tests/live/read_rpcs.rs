use crate::common::LiveEngine;
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{NetworkIsolation, RuntimeBackend};
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

/// Three arms, and this is the one a reconciler depends on most: an absent
/// sandbox must be Ok(None), never an error.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn inspecting_an_unknown_sandbox_answers_absent_rather_than_failing() {
    let engine = LiveEngine::start().await;
    let id = SandboxId::test("never-created");

    let observed = backend(&engine).await.inspect(&id).await;

    assert!(
        matches!(observed, Ok(None)),
        "an unknown sandbox must be Ok(None): {observed:?}"
    );
}

#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn listing_an_empty_engine_returns_an_empty_list_rather_than_an_error() {
    let engine = LiveEngine::start().await;

    let resources = backend(&engine).await.list_resources().await.unwrap();

    assert!(
        resources.is_empty(),
        "a fresh engine holds nothing: {resources:?}"
    );
}

/// The eight unimplemented methods must ANSWER. A gRPC status would reach the
/// consumer as an unreachable engine, which is a different fact from "this
/// build cannot do that", and would send a reconciler down the wrong path.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn an_unimplemented_method_reports_unsupported_capability_not_a_transport_fault() {
    let engine = LiveEngine::start().await;
    let id = SandboxId::test("never-created");

    let error = backend(&engine)
        .await
        .start(&id)
        .await
        .expect_err("Start is not implemented in this milestone");

    assert_eq!(
        error.code(),
        "unsupported_capability",
        "an unimplemented method must answer in its outcome, not as a status: {error}"
    );
}
