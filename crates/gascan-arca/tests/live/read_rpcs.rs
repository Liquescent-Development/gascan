use crate::common::{LiveEngine, policy_request, retained_for};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{
    ExecRequest, NetworkIsolation, RecreateRequest, RetainedResources, RuntimeBackend,
};
use gascan_core::sandbox::SandboxId;

/// The backend over a real engine, not a fake. Everything below goes through
/// ChannelTransport and the real gRPC stack.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

/// What the engine claims, read over the wire, flag by flag.
///
/// **The four negatives are as load-bearing as the three positives, and this
/// test exists mostly for them.** A flag that turns true before its capability
/// works makes `PolicyCompiler` compile a request the engine cannot honour --
/// `policy.rs` gates on what the runtime CLAIMS, refusing with
/// `bind_mounts_unavailable` or `resource_limits_unavailable` when it does not
/// -- so a flag drifting true is not a documentation error, it is a sandbox
/// that comes up wrong.
///
/// **Nothing here corroborates a single flag.** It reads what the engine says.
/// What makes the three positives honest is elsewhere in this tier, and each
/// one names its instrument:
///
/// - `bind_mounts` (`project_mount` on the wire; see `translate.rs:323`, the
///   same capability under two names) by
///   `mounts::the_project_root_is_readable_in_the_guest_and_writable_back_to_the_host`.
/// - `loopback_publish` by `ports::a_published_port_is_reachable_from_the_test_process`.
/// - `resource_limits` by
///   `limits::the_requested_cpu_and_memory_limits_are_the_guests_own_cgroup_limits`.
///
/// And each negative names why it is one:
///
/// - `named_volumes` by
///   `mounts::the_managed_volumes_are_attached_to_the_guest_but_this_engine_mounts_none_of_them`,
///   which measures the engine attaching all three volumes as block devices and
///   mounting none of them.
/// - `tty` and `signals` are milestone 3's, with `Exec` --
///   `every_unimplemented_method_answers_unsupported_capability_not_a_transport_fault`
///   below still lists it among the three this build refuses.
/// - `offline` stays `Unverified` until milestone 4's proof exercise.
///
/// CORRECTED: this test used to assert every flag false and its comment said
/// milestone 4 would replace it "with one asserting every flag is true". That
/// forward reference was wrong in both directions -- three flags moved here in
/// milestone 2, and milestone 4 has no authority to turn on `named_volumes`,
/// `tty` or `signals`, which belong to an Arca fix and to milestone 3. What
/// milestone 4 owns is `offline`.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn capabilities_report_only_what_this_engine_build_implements() {
    let engine = LiveEngine::start().await;
    let capabilities = backend(&engine).await.capabilities().await.unwrap();

    assert!(capabilities.bind_mounts);
    assert!(!capabilities.named_volumes);
    assert!(!capabilities.tty);
    assert!(!capabilities.signals);
    assert!(capabilities.loopback_publish);
    assert!(capabilities.resource_limits);
    assert_eq!(capabilities.offline, NetworkIsolation::Unverified);
}

/// Every method this build does not implement must ANSWER, and every one of
/// them must answer the same way.
///
/// A gRPC status would reach the consumer as an unreachable engine, which is a
/// different fact from "this build cannot do that" and would send a reconciler
/// down the wrong path. That is the PR's claim, and it was once asserted for
/// one method out of ten: a comment reading "the eight unimplemented methods
/// must ANSWER" sat above a body that called only `start`. The two streaming
/// methods were the ones that mattered most, because their error arrives in a
/// different message entirely -- `LogsChunk.outcome.error` and
/// `ExecServerFrame.frame.error` -- and nothing in this tier touched either. A
/// regression that made `Logs` answer with a status would have passed.
///
/// **THREE, and it was ten. The count is asserted rather than implied precisely
/// so that this happens.** Milestone 2 implemented `Inspect`, `ListResources`,
/// `PrepareImage`, `Create`, `Start`, `Stop` and `Remove`, and the old list
/// FAILED against the branch engine the day the first one landed: `expect_err`
/// on `Inspect` got a perfectly good `absent`. That failure is the mechanism
/// working. `CreateContainer` is the one method left that is neither streaming
/// nor milestone 3's -- `Exec` and `Logs` are.
///
/// A method that becomes real leaves this list and gets real assertions: for
/// the seven that already did, they are in `lifecycle.rs` and `ports.rs`.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn every_unimplemented_method_answers_unsupported_capability_not_a_transport_fault() {
    let engine = LiveEngine::start().await;
    let backend = backend(&engine).await;
    let id = SandboxId::test("never-created");

    // Held for the whole test: the compiled request names a canonical root that
    // must still exist when the call is made.
    let (_recreate_root, recreate_create) = policy_request("never-recreated");
    let retained = RetainedResources::new(&recreate_create, retained_for(&recreate_create))
        .expect("the retained set matches the requested topology exactly");
    let recreate = RecreateRequest::new(recreate_create, retained).expect("a recreate request");

    // Each arm reduces to the wire code, so the three shapes -- CreateFailure,
    // ExecSession, Vec<u8> -- become one comparable list. `CreateFailure`
    // carries its code separately from `RuntimeError` because a partial create
    // must report what it made.
    let answers: Vec<(&str, String)> = vec![
        (
            "CreateContainer",
            backend
                .create_container(recreate)
                .await
                .expect_err("CreateContainer")
                .code()
                .to_owned(),
        ),
        ("Exec", {
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
        }),
        (
            "Logs",
            backend
                .logs(&id, None)
                .await
                .expect_err("Logs")
                .code()
                .to_owned(),
        ),
    ];

    assert_eq!(
        answers.len(),
        3,
        "this build implements eight of the eleven contract methods; \
         a method that becomes real must leave this list"
    );
    for (rpc, code) in &answers {
        assert_eq!(
            code, "unsupported_capability",
            "{rpc} must answer in its outcome, not as a status: got {code}"
        );
    }
}
