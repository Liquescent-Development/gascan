use crate::common::LiveEngine;
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ExecRequest, NetworkIsolation, RuntimeBackend};
use gascan_core::sandbox::SandboxId;
use std::time::Duration;

/// The backend over a real engine, not a fake. Everything below goes through
/// ChannelTransport and the real gRPC stack.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

/// What the engine claims, read over the wire, flag by flag.
///
/// **The three negatives are as load-bearing as the four positives, and this
/// test exists mostly for them.** A flag that turns true before its capability
/// works makes `PolicyCompiler` compile a request the engine cannot honour --
/// `policy.rs` gates on what the runtime CLAIMS, refusing with
/// `bind_mounts_unavailable` or `resource_limits_unavailable` when it does not
/// -- so a flag drifting true is not a documentation error, it is a sandbox
/// that comes up wrong.
///
/// **Nothing here corroborates a single flag.** It reads what the engine says.
/// What makes the four positives honest is elsewhere in this tier, and each
/// one names its instrument:
///
/// - `bind_mounts` (`project_mount` on the wire; see `translate.rs:323`, the
///   same capability under two names) by
///   `mounts::the_project_root_is_readable_in_the_guest_and_writable_back_to_the_host`.
/// - `named_volumes` by
///   `mounts::the_managed_volumes_are_mounted_at_their_declared_targets_and_writable`.
/// - `loopback_publish` by `ports::a_published_port_is_reachable_from_the_test_process`.
/// - `resource_limits` by
///   `limits::the_requested_cpu_and_memory_limits_are_the_guests_own_cgroup_limits`.
///
/// - `tty` by
///   `exec::a_tty_exec_gives_the_guest_a_terminal_and_merges_stderr_into_stdout`.
/// - `signals` by
///   `exec::a_signal_reaches_the_guest_process_and_decides_how_it_exits`.
///
/// And the one remaining negative names why it is one:
///
/// - `offline` stays `Unverified` until milestone 4's proof exercise.
///
/// CORRECTED: this test used to assert every flag false and its comment said
/// milestone 4 would replace it "with one asserting every flag is true". That
/// forward reference was wrong in both directions -- three flags moved here in
/// milestone 2, and milestone 4 has no authority to turn on `named_volumes`,
/// `tty` or `signals`, which belong to an Arca fix and to milestone 3. What
/// milestone 4 owns is `offline`, and it is now the only one left: `tty` and
/// `signals` moved with milestone 3's `Exec`.
///
/// `named_volumes` moved fourth, when Arca stopped identifying its OverlayFS
/// block devices by counting `/dev/vd` letters and started reading a role out of
/// each image's ext4 volume label. The negative that held it down --
/// `the_managed_volumes_are_attached_to_the_guest_but_this_engine_mounts_none_of_them`
/// -- failed on that build, exactly as its message said it would, and its
/// replacement is the positive named above.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn capabilities_report_only_what_this_engine_build_implements() {
    let engine = LiveEngine::start().await;
    let capabilities = backend(&engine).await.capabilities().await.unwrap();

    assert!(capabilities.bind_mounts);
    assert!(capabilities.named_volumes);
    assert!(capabilities.tty);
    assert!(capabilities.signals);
    assert!(capabilities.loopback_publish);
    assert!(capabilities.resource_limits);
    assert_eq!(capabilities.offline, NetworkIsolation::Unverified);
}

/// **The engine's self-report is checked against the pin, over the wire.**
///
/// `engine/arca-pin.json` names a signed Arca tag and the revision it must
/// resolve to; `scripts/build-arca-engine.sh` compiles that tree, regenerates
/// `BuildInfo.generated.swift` inside it, and refuses to build if the
/// regenerated constant is not the pinned one. That is a build-time assertion
/// on generated Swift source. This is the run-time half: what the engine
/// actually puts on the wire in field 20.
///
/// **The two halves are not redundant, and the gap between them is the whole
/// reason field 20 exists.** The build gate reads a file the compiler is about
/// to read; this reads what a running process says about itself. An engine
/// binary that predates the pin bump -- the state
/// `GASCAN_ARCA_ENGINE_BIN` is in every time someone forgets to rebuild --
/// passes the build gate trivially, because the build gate never ran, and
/// fails here.
///
/// MEASURED when this was written: before the pin moved to schema 2, the
/// committed `BuildInfo.generated.swift` recorded
/// `5e1170495400b25f6334c6d8ddda5d3521b7cfd8` while the tag being pinned was
/// `c545612b056e028d5885968a7b9f586d694f994c`, and it had drifted through the
/// whole of milestone 3 unnoticed -- because nothing read it that mattered.
///
/// The raw transport and not `ArcaBackend::capabilities()`: `translate.rs` maps
/// the wire message onto `gascan_core`'s `RuntimeCapabilities`, which does not
/// carry a revision. Asserting through the backend would require inventing that
/// surface here, ahead of the task that owns it.
///
/// The pin is read at run time rather than baked in with `include_str!` so that
/// a pin bump cannot leave a stale expectation compiled into this test. This is
/// NOT the certified-revision constant -- that is a separate, deliberate
/// judgement about which engine build has had its isolation proven, and it does
/// not move with the pin.
#[tokio::test]
#[ignore = "requires the engine BUILT FROM THE PIN by scripts/build-arca-engine.sh, named by GASCAN_ARCA_ENGINE_BIN"]
async fn the_engine_reports_the_revision_the_pin_names() {
    let engine = LiveEngine::start().await;
    let transport = engine.transport().await;

    let response = gascan_arca::EngineTransport::capabilities(
        &transport,
        gascan_engine_proto::v1::CapabilitiesRequest {},
    )
    .await
    .expect("a real engine must answer Capabilities");

    let capabilities = match response.outcome {
        Some(gascan_engine_proto::v1::capabilities_response::Outcome::Capabilities(
            capabilities,
        )) => capabilities,
        other => panic!("the engine answered Capabilities with {other:?}"),
    };

    let pin_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../engine/arca-pin.json");
    let pin: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(pin_path).expect("the pin file is readable"))
            .expect("the pin file is JSON");
    let pinned = pin["revision"]
        .as_str()
        .expect("the pin carries a revision");

    assert_eq!(
        capabilities.build_revision, pinned,
        "the engine under GASCAN_ARCA_ENGINE_BIN was not built from the pinned revision"
    );
}

/// **The list of unimplemented methods is EMPTY, and this is what replaced it.**
///
/// The property the old test carried is still worth keeping and is not asserted
/// anywhere else: an engine answer must arrive **in the message's own outcome**,
/// never as a gRPC status. A status reaches the consumer as an unreachable
/// engine, which is a different fact from "that could not be done" and sends a
/// reconciler down the wrong path (`engine.proto:52-58`).
///
/// `Exec` is where that property is most easily lost, which is why it is the one
/// method kept here. Its error does not live in a response outcome at all -- it
/// lives in `ExecServerFrame.frame.error`, one frame inside a stream -- so a
/// handler that threw instead of sending would produce a transport fault that no
/// other test in this tier would notice. MEASURED when this file still tested
/// the refusal: an earlier draft called `expect_err` on `exec()` itself and
/// failed with a perfectly healthy `ExecSession`, because the session opens
/// before the engine has said anything. **Anything asserting against the call
/// and not the frame is testing the wrong half.**
///
/// **What changed and why the old shape had to go.** Until milestone 3 this test
/// held a list of methods that answered `unsupported_capability`, and each
/// method that became real left it: `Inspect`, `ListResources`, `PrepareImage`,
/// `Create`, `Start`, `Stop` and `Remove` in milestone 2, `CreateContainer` in
/// task 1, `Logs` in task 5, and `Exec` in task 6. **A list of length zero
/// asserts nothing**, and the length check that guarded it was a tautology over
/// a `vec![]` literal -- it said so in its own comment. What forced a method out
/// was always the per-entry assertion, so with no entries left the whole
/// structure is gone and the surviving property is asserted directly.
///
/// The refusal below is `not_found` rather than `unsupported_capability`: the
/// sandbox was never created, and that is now what `Exec` says about one. The
/// codes this engine may use at all are a closed table
/// (`gascan-arca/src/error.rs:20-55`), so a status or an unrecognised code
/// arrives as `invalid_output` and fails here either way.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn exec_refuses_in_its_own_frame_rather_than_as_a_transport_status() {
    let engine = LiveEngine::start().await;
    let backend = backend(&engine).await;
    let id = SandboxId::test("never-created");

    // The session OPENS -- `exec()` returns Ok -- and the answer arrives as the
    // stream's first frame.
    //
    // **Both awaits are bounded, because the defect this test guards makes them
    // hang rather than fail.** A handler that threw instead of sending produces
    // no first frame, and an engine that never accepts the RPC never returns a
    // stream at all; unbounded, either one stalls this test forever, and under
    // `--test-threads=1` it stalls the whole tier with it. A test that hangs
    // instead of failing is not a guard.
    let opening = backend.exec(ExecRequest::fixture(id.clone(), ["true"]));
    let mut session = match tokio::time::timeout(Duration::from_secs(60), opening).await {
        Err(_) => panic!("Exec did not open a session within 60s"),
        Ok(result) => result.expect("Exec opens a session; the refusal is its first frame"),
    };
    let code = match tokio::time::timeout(Duration::from_secs(60), session.next()).await {
        Err(_) => panic!("Exec sent no first frame within 60s; a refusal must be answered"),
        Ok(Some(Err(error))) => error.code().to_owned(),
        Ok(other) => panic!("Exec's first frame must be an error, got {other:?}"),
    };
    assert_eq!(
        code, "not_found",
        "Exec must refuse a sandbox that was never created in its own frame, \
         naming what was wrong: got {code}"
    );
}
