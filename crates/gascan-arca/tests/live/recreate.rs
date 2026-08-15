//! `CreateContainer` against a real engine.
//!
//! **Data survival is the assertion, not a successful return.** A recreate that
//! quietly rebuilt its volumes would also return `Ok` and would also report one
//! container; what distinguishes reuse from a rebuild that happened to succeed
//! is that what was written before the recreate is still there after it. An
//! assertion on the response alone would pass against the defect.

use crate::common::{
    LiveEngine, answering, await_state, base_oci_layout, layout_running_with_directories,
    policy_request_from_manifest, read_from_loopback, report_section, reserved_loopback_port,
    retained_for,
};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ArcaBackend;
use gascan_core::policy::TOOLS_ROOT;
use gascan_core::runtime::{
    ContainerState, RecreateRequest, RemoveRequest, ResourceKind, RetainedResources,
    RuntimeBackend, RuntimeResource,
};
use std::time::Duration;

/// The backend over a real engine, not a fake.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

/// The tag the derived layout in this file is loaded under.
const TAG: &str = "gascan-live-recreate:latest";

/// The manifest this test compiles from, with the caller's port declared.
///
/// `user = 'root'` for the reason `lifecycle.rs` records: a stock alpine has no
/// `workspace` user. `networked` because an offline sandbox may not publish a
/// port, and the port is the only way this test can hear from the guest --
/// `sandboxContainerSpec` refuses the combination outright
/// (`EngineCreate.swift:264-273`).
fn manifest(port: u16) -> String {
    format!("version = 1\nnetwork = 'networked'\nuser = 'root'\n\n[ports]\nboots = {port}\n")
}

/// Appends a line to a file on a managed volume, then serves the whole file.
///
/// Append rather than overwrite: after the recreate the file must hold BOTH
/// boots' lines, which distinguishes "the volume survived" from "the volume was
/// rebuilt and the second boot rewrote it" -- an overwrite makes those two
/// outcomes identical.
///
/// The file sits under `TOOLS_ROOT` because that is a target `policy.rs`
/// compiles a managed volume for, and the directory is created in a layer of its
/// own for the reason `layout_running_with_directories` records: a mount target
/// absent from the image is silently not mounted, which would leave this test
/// appending to the container's own rootfs and failing for the wrong reason.
fn appending_and_reporting(destination: &Utf8Path, port: u16) -> Utf8PathBuf {
    let boots = format!("{TOOLS_ROOT}/boots");
    let script = format!(
        "date +%s%N >> {boots}; {}",
        answering(port, &format!("{{ echo ---boots---; cat {boots}; }}"))
    );
    layout_running_with_directories(
        &base_oci_layout(),
        destination,
        TAG,
        &["sh", "-c", &script],
        &[TOOLS_ROOT],
    )
}

/// A recreate rebuilds the container against resources it did not touch, and the
/// data on them survives.
///
/// **The container is removed ALONE, which is what makes this a recreate.** The
/// volumes and the network stay on the host between the two boots, and
/// `retained` names exactly them -- so an engine that rebuilt one would be
/// rebuilding a resource that already existed, and the first boot's line would
/// be gone.
///
/// **THIS TEST IS THE ONLY THING STANDING BEHIND THE ENGINE'S NETWORK GUARD IN
/// THE HELD DIRECTION, AND IT CANNOT BE REPLACED BY A SWIFT TEST.** Arca's
/// `reusedTopologyRefusal` requires every retained resource -- volumes *and* the
/// managed network -- to be held by the engine and owned by the caller.
/// `ArcaEngineTests` covers the network's refused half
/// (`CreateTests.testCreateContainerRefusesAManagedNetworkTheEngineDoesNotHold`),
/// but not the held half: `preparedEngine()` leaves `NetworkManager`
/// uninitialised deliberately, so `getNetworkByName` returns nil for *every*
/// name and "the engine holds this network" is unreachable in that target by
/// construction. Here it is reachable and load-bearing: `retained_for` puts the
/// managed network in the retained set (`common/mod.rs:699-706`), so a guard that
/// wrongly refused a network it does hold would fail the `create_container` call
/// below outright. **Deleting this test removes the only evidence that half of
/// the guard works.**
///
/// **What this does NOT prove.** Nothing about the refusal path: an engine that
/// checked no retained resource at all passes this test, because everything this
/// names is genuinely held.
/// `CreateTests.testCreateContainerRefusesARetainedResourceTheEngineDoesNotHold`
/// is what covers that, and it is the test the guard's mutation was measured
/// against. It also takes the same dependency on a working publish that every
/// other guest-reading test here does -- see `common::answering` -- so a broken
/// publish fails this too, and `ports.rs` is what says whether the publish is the
/// cause.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn a_recreate_reuses_its_retained_volumes_rather_than_rebuilding_them() {
    let port = reserved_loopback_port();
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = appending_and_reporting(
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        port,
    );
    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = backend(&engine).await;

    let (_root, request) =
        policy_request_from_manifest("recreating", &engine.image(TAG), &manifest(port));
    backend
        .prepare_image(request.image())
        .await
        .expect("the store holds the image the request names");

    let created = backend
        .create(request.clone())
        .await
        .expect("create against a seeded store must succeed");
    backend.start(request.id()).await.expect("start must boot");
    await_state(
        &backend,
        &request,
        ContainerState::Running,
        Duration::from_secs(180),
    )
    .await;

    let first = read_from_loopback(port, Duration::from_secs(120)).await;
    let before = report_section(&first, "boots");
    assert_eq!(before.len(), 1, "the first boot writes one line: {first}");

    // Remove the container ALONE. Everything else is what `retained` names.
    let container: Vec<RuntimeResource> = created
        .created()
        .iter()
        .filter(|resource| resource.kind() == ResourceKind::Container)
        .cloned()
        .collect();
    assert_eq!(container.len(), 1, "create reports exactly one container");

    backend.stop(request.id()).await.expect("stop must answer");
    await_state(
        &backend,
        &request,
        ContainerState::Stopped,
        Duration::from_secs(120),
    )
    .await;
    backend
        .remove(RemoveRequest::from_resources(container).expect("gascan-owned resources"))
        .await
        .expect("removing the container alone must succeed");

    let retained = RetainedResources::new(&request, retained_for(&request))
        .expect("the retained set matches the requested topology exactly");
    let recreate = RecreateRequest::new(request.clone(), retained).expect("a recreate request");

    let rebuilt = backend
        .create_container(recreate)
        .await
        .expect("CreateContainer must rebuild the container against retained resources");
    // Kept for the failure message, not for the evidence: `CreateOutcome
    // ::for_recreate` runs `validate_recreated_container`, which already requires
    // exactly one container, so a whole-topology answer would have made the
    // `expect` above panic first. The claim this test carries is the boot count
    // below, not this line.
    assert_eq!(
        rebuilt.created().len(),
        1,
        "a recreate rebuilds the container alone: {:?}",
        rebuilt.created()
    );

    backend
        .start(request.id())
        .await
        .expect("the rebuilt container must start");
    await_state(
        &backend,
        &request,
        ContainerState::Running,
        Duration::from_secs(180),
    )
    .await;

    let second = read_from_loopback(port, Duration::from_secs(120)).await;
    let after = report_section(&second, "boots");
    assert_eq!(
        after.len(),
        2,
        "the retained volume must still hold the first boot's line; \
         a rebuilt volume would show only the second: {second}"
    );

    backend.stop(request.id()).await.expect("stop must answer");
    await_state(
        &backend,
        &request,
        ContainerState::Stopped,
        Duration::from_secs(120),
    )
    .await;
    // The rebuilt container and the resources it reused, in one request: the
    // recreate reported only the container, so a teardown that passed just
    // `rebuilt.created()` would leave the volumes and the network on the host.
    let everything: Vec<RuntimeResource> = rebuilt
        .created()
        .iter()
        .cloned()
        .chain(retained_for(&request))
        .collect();
    backend
        .remove(RemoveRequest::from_resources(everything).expect("gascan-owned resources"))
        .await
        .expect("remove must delete the container and the resources it retained");

    engine.kill().await;
}
