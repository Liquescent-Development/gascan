use crate::common::{
    LiveEngine, await_state, base_oci_layout, layout_running, policy_request_for_image,
    policy_request_from_manifest,
};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ArcaBackend;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{
    ContainerState, RemoveRequest, ResourceKind, RuntimeBackend, RuntimeResource,
};
use std::time::Duration;

/// The backend over a real engine, not a fake.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

/// The tag every derived layout in this file is loaded under.
const TAG: &str = "gascan-live:latest";

/// The tag the base layout carries, and the one image this file uses unmodified.
const BASE_TAG: &str = "alpine:3.20";

/// The manifest the lifecycle tests compile their requests from.
///
/// `user = 'root'` because the base layout is a stock alpine, which has no
/// `workspace` user: the engine translates `UserMode::Workspace` to the literal
/// string `workspace` and hands it to `createContainer`, so a start would fail
/// on the image rather than on anything this tier is testing. `root` is an
/// ordinary manifest choice, not a test-only escape hatch.
const MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

/// An image whose only job is to stay up, so a container can be observed
/// `running` and acted on while it is.
///
/// `sh -c 'while :; do sleep 1; done'` rather than the base image's own `Cmd`.
/// Alpine's is `/bin/sh`, which exits immediately with no tty attached.
///
/// CORRECTED after review, because the first version of this comment claimed
/// more than the engine does. It said `Remove` "would then never meet the
/// `containerRunning` refusal this file exists to reach". **It meets it fine.**
/// MEASURED, with this `Cmd` replaced by `sh -c 'exit 0'`: the engine logs
/// `Background monitor: container exited exit_code=0` and the subsequent
/// `Remove` is still refused `invalid_state`, because the refusal reads the
/// PERSISTED state and nothing writes the guest's exit back to it. Every
/// assertion in `remove_refuses_a_running_container_rather_than_destroying_it`
/// passed against an already-exited container.
///
/// **What a long-running `Cmd` is actually for is the teardown.** Under that
/// mutation both tests failed at their own cleanup `stop`, with
/// `CancellationError()` -- stopping a container whose PID 1 has gone. So this
/// image keeps the tests' cleanup honest; it is not what makes the refusal
/// reachable.
fn staying_up(destination: &Utf8Path) -> Utf8PathBuf {
    layout_running(
        &base_oci_layout(),
        destination,
        TAG,
        &["sh", "-c", "while :; do sleep 1; done"],
    )
}

/// One `format!` shape for a resource, so a report and an expectation compare
/// as text rather than through two different `Debug` renderings.
fn described(kind: ResourceKind, name: &str) -> String {
    format!("{kind:?} {name}")
}

fn describe_all(resources: &[RuntimeResource]) -> Vec<String> {
    let mut described: Vec<String> = resources
        .iter()
        .map(|resource| described(resource.kind(), resource.name()))
        .collect();
    described.sort();
    described
}

/// The whole contract over one real container: create, start, inspect, stop,
/// remove.
///
/// **Nothing after `Create` had ever executed before this test.** `Start`,
/// `Stop` and `Remove` landed in Task 12 against VM-free unit tests, and
/// `startContainer` boots a real virtual machine, so the only evidence any of
/// them worked was that they returned the right answers to calls that never
/// reached the machinery. This drives all five over a unix socket against an
/// engine on its own state root, with an image the test loaded into that store.
///
/// **What it proves.** That `Create` makes exactly the topology the policy
/// compiled -- three volumes, a network and a container; that `Start` returns
/// and the sandbox afterwards reaches `Running`; that `Stop` returns it to
/// `Stopped`; and that `Remove` deletes every resource `Create` reported, so
/// `Inspect` afterwards is absent.
///
/// **What it does NOT prove.** Nothing about ports -- see `ports.rs`, the only
/// instrument that can see a publish. And `Inspect` reports what the STORE
/// holds, deliberately, because that is what drift detection compares against:
/// the `Running` here is the store's opinion and not an observation of the
/// guest.
///
/// **`ports.rs` IS THE ONLY TEST THAT TURNS THAT OPINION INTO EVIDENCE**, because
/// it is the only one that reads bytes the guest itself produced.
///
/// CORRECTED after review. This comment used to name
/// `remove_refuses_a_running_container_rather_than_destroying_it` as what makes
/// the `Running` real, "a refusal only a genuinely running container produces".
/// **That is false, and it was a claim resting on another claim rather than on a
/// measurement.** That refusal reads the same persisted state this `Inspect`
/// reads, and nothing writes the guest's exit back into it: MEASURED, a
/// container whose PID 1 had already exited was still refused `invalid_state`,
/// and this test's own `await_state(Running)` still passed against it. Two reads
/// of one opinion do not corroborate each other.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn create_start_inspect_stop_and_remove_drive_a_real_container() {
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = staying_up(Utf8Path::from_path(images.path()).expect("a utf-8 path"));
    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = backend(&engine).await;

    let (_root, request) = policy_request_from_manifest("lifecycle", &engine.image(TAG), MANIFEST);
    backend
        .prepare_image(request.image())
        .await
        .expect("the store holds the image the request names");

    let created = backend
        .create(request.clone())
        .await
        .expect("create against a seeded store must succeed");
    let mut expected: Vec<String> = PolicyCompiler::expected_resource_identities(request.id())
        .expect("the policy names the topology it compiled")
        .iter()
        .map(|identity| described(identity.kind(), identity.name()))
        .collect();
    expected.sort();
    assert_eq!(
        describe_all(created.created()),
        expected,
        "create must report exactly the container, the three managed volumes and the network"
    );

    backend
        .start(request.id())
        .await
        .expect("start must boot the sandbox");
    // Generous: a first start boots a virtual machine from a kernel and a
    // vminit layout. Bounded all the same.
    await_state(
        &backend,
        &request,
        ContainerState::Running,
        Duration::from_secs(180),
    )
    .await;

    let running = backend
        .inspect(request.id())
        .await
        .expect("inspect must answer")
        .expect("a started sandbox is present");
    assert_eq!(
        running.image,
        request.image(),
        "inspect must report the digest reference create recorded"
    );
    assert_eq!(running.ownership.sandbox_id, *request.id());
    assert!(
        running.ports().is_empty(),
        "this manifest declares no ports and inspect must invent none"
    );

    backend
        .stop(request.id())
        .await
        .expect("stop must answer for a running sandbox");
    await_state(
        &backend,
        &request,
        ContainerState::Stopped,
        Duration::from_secs(120),
    )
    .await;

    backend
        .remove(
            RemoveRequest::from_resources(created.created().to_vec())
                .expect("everything create reported is gascan-owned"),
        )
        .await
        .expect("remove must delete a stopped sandbox");
    assert!(
        backend
            .inspect(request.id())
            .await
            .expect("inspect must answer after a remove")
            .is_none(),
        "a removed sandbox must be absent"
    );

    engine.kill().await;
}

/// A create that fails partway must report **which** resources it made.
///
/// **The assertion is on the contents and not on the length.** That is the
/// defect shape Task 8's review measured: a `created` list asserted only to be
/// non-empty passes for a report naming the wrong resources, and a consumer
/// acting on it leaks the ones it was not told about. `CreateFailed.created`
/// exists precisely so a partial create does not leak with nothing knowing to
/// look for it, and a length check does not measure that.
///
/// **How the partial is reached, and why this arrangement.** The engine creates
/// volumes, then the network, then the container, and it asks whether it
/// already holds a network of that name *before* creating one. So: create the
/// sandbox, remove its container and its volumes but leave the network, and
/// create it again. The second create makes the three volumes, meets its own
/// network, and refuses -- with the three volumes in `created` and nothing else.
///
/// The container has to go in that first remove and cannot be left behind: a
/// volume whose container still exists is refused as `VolumeError.inUse`,
/// because `StateStore.getVolumeUsers` counts every container and not only
/// running ones. `Remove` deletes containers before volumes for exactly this
/// reason.
///
/// **What this does NOT prove.** Nothing about a *container-step* failure. The
/// arm reached here is the network-name conflict; a container step that failed
/// after a network was made would report four resources rather than three. Both
/// go through the same `createFailed` helper, which is why one arm measures
/// that the evidence is carried at all -- it is not a claim that every arm was
/// exercised.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn a_create_that_fails_partway_reports_exactly_the_resources_it_made() {
    let base = base_oci_layout();
    let engine = LiveEngine::start_with_images(&[base.as_path()]).await;
    let backend = backend(&engine).await;

    let (_root, request) = policy_request_for_image("partial", &engine.image(BASE_TAG));
    let created = backend
        .create(request.clone())
        .await
        .expect("the first create must succeed");

    // Everything except the network, which is what the second create will meet.
    let network = PolicyCompiler::managed_network_name(request.id());
    let first: Vec<RuntimeResource> = created
        .created()
        .iter()
        .filter(|resource| resource.name() != network)
        .cloned()
        .collect();
    assert_eq!(
        first.len(),
        4,
        "the container and the three volumes are what has to go first"
    );
    backend
        .remove(RemoveRequest::from_resources(first).expect("gascan-owned resources"))
        .await
        .expect("removing the container and its volumes must succeed");

    let failure = backend
        .create(request.clone())
        .await
        .expect_err("a create whose network name is already held must fail");
    let mut expected: Vec<String> = request
        .volumes()
        .iter()
        .map(|volume| described(ResourceKind::Volume, &volume.name))
        .collect();
    expected.sort();
    assert_eq!(
        describe_all(failure.created()),
        expected,
        "the failed create must report the three volumes it made and nothing else"
    );
    assert_eq!(
        failure.code(),
        "resource_conflict",
        "a name that is already taken is not a command failure: {failure}"
    );

    engine.kill().await;
}

/// `Remove` must refuse a container that is actually running.
///
/// **This refusal shipped untested and this is its first instrument.**
/// `removeContainer` refuses a running container without `force`, and Task 12
/// could not test it: a container in state `running` is unreachable VM-free,
/// because `loadPersistedState()` recovers every persisted `running` row as
/// exited/137 before a test can observe it. A test written against that path
/// would have exercised crash recovery while reading as the refusal. It becomes
/// reachable the moment containers actually run, which is what this file made
/// true. It is a destructive path, and until now it had no instrument at all.
///
/// **What it proves.** That a `Remove` naming a running container fails with
/// `invalid_state` -- retrying is futile until something stops it, which is
/// what that code says and what `command_failed` does not -- and, load-bearing,
/// that the sandbox and its volumes are **still there afterwards**. A refusal
/// that had already destroyed something satisfies the error assertion alone.
///
/// **What it does NOT prove.** That the refusal happened before *anything* was
/// deleted in general. The container is first in `Remove`'s deletion order, so
/// this arrangement cannot distinguish "refused having deleted nothing" from
/// "refused having deleted nothing else"; the volumes surviving is the part a
/// consumer can act on, and it is what is asserted.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn remove_refuses_a_running_container_rather_than_destroying_it() {
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = staying_up(Utf8Path::from_path(images.path()).expect("a utf-8 path"));
    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = backend(&engine).await;

    let (_root, request) = policy_request_from_manifest("running", &engine.image(TAG), MANIFEST);
    let created = backend
        .create(request.clone())
        .await
        .expect("create must succeed");
    backend
        .start(request.id())
        .await
        .expect("start must answer");
    await_state(
        &backend,
        &request,
        ContainerState::Running,
        Duration::from_secs(180),
    )
    .await;

    let refused = backend
        .remove(
            RemoveRequest::from_resources(created.created().to_vec())
                .expect("gascan-owned resources"),
        )
        .await
        .expect_err("removing a running container must be refused");
    assert_eq!(
        refused.code(),
        "invalid_state",
        "a running container is a state to fix, not a command to retry: {refused}"
    );
    // `invalid_state` alone does not say WHICH refusal fired: `actCatching`
    // maps `VolumeError.inUse` to the same code, and a volume still attached to
    // a live container raises exactly that. The container is removed first, so
    // the error must name the container.
    assert!(
        refused.to_string().contains(request.id().as_str()),
        "the refusal must name the running container rather than one of its volumes: {refused}"
    );

    let survived = backend
        .inspect(request.id())
        .await
        .expect("inspect must answer")
        .expect("a refused remove must leave the sandbox in place");
    assert_eq!(survived.state, ContainerState::Running);
    let held = backend
        .list_resources()
        .await
        .expect("list_resources must answer");
    for volume in request.volumes() {
        assert!(
            held.iter().any(|resource| {
                resource.name() == volume.name && resource.kind() == ResourceKind::Volume
            }),
            "the refused remove must have deleted no volume; {} is gone",
            volume.name
        );
    }

    // Stop before tearing down, or the run leaves a virtual machine behind.
    backend.stop(request.id()).await.expect("stop must answer");
    await_state(
        &backend,
        &request,
        ContainerState::Stopped,
        Duration::from_secs(120),
    )
    .await;
    backend
        .remove(
            RemoveRequest::from_resources(created.created().to_vec())
                .expect("gascan-owned resources"),
        )
        .await
        .expect("removing the same sandbox once stopped must succeed");

    engine.kill().await;
}
