use crate::common::{
    LiveEngine, answering, await_state, base_oci_layout, layout_running,
    layout_running_with_directories, policy_request_from_manifest, read_from_loopback,
    report_section, reserved_loopback_port,
};
use camino::Utf8Path;
use gascan_arca::ArcaBackend;
use gascan_core::policy::{CACHE_ROOT, CONFIG_ROOT, TOOLS_ROOT};
use gascan_core::runtime::{
    ContainerState, CreateRequest, RemoveRequest, ResourceKind, RuntimeBackend, RuntimeResource,
};
use gascan_core::sandbox::WORKSPACE_TARGET;
use std::time::Duration;

/// The manifest both tests compile from, with the caller's port declared.
///
/// `user = 'root'` for the reason `lifecycle.rs` records: a stock alpine has no
/// `workspace` user. `networked` because an offline sandbox may not publish a
/// port, and the port is the only way either test can hear from the guest --
/// `sandboxContainerSpec` refuses the combination outright
/// (`EngineCreate.swift:264-273`).
fn manifest(port: u16, extra: &str) -> String {
    format!(
        "version = 1\nnetwork = 'networked'\nuser = 'root'\n\n[ports]\nreport = {port}\n{extra}"
    )
}

/// Everything a guest said, so a failed lookup reports the whole answer.
///
/// See `common::report_section` for why the guest sends raw text rather than a
/// summary it computed itself.
struct GuestReport {
    text: String,
}

impl GuestReport {
    /// The device and filesystem type mounted at `target`, if anything is.
    ///
    /// `Option` rather than a panic, because both tests below turn on the
    /// absent case and one of them expects it.
    fn mount_at(&self, target: &str) -> Option<(&str, &str)> {
        report_section(&self.text, "mounts")
            .into_iter()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let device = fields.next()?;
                (fields.next()? == target).then(|| (device, fields.next().unwrap_or_default()))
            })
    }
}

/// Creates and starts `request`, and reads what its guest answers on `port`.
///
/// Split from [`tear_down`] rather than doing both, because a test that has to
/// ask the engine about a *running* sandbox -- as the volume test does -- needs
/// somewhere to stand between the two halves.
async fn start_and_read(
    backend: &ArcaBackend<gascan_arca::ChannelTransport>,
    request: &CreateRequest,
    port: u16,
) -> (Vec<RuntimeResource>, GuestReport) {
    let created = backend
        .create(request.clone())
        .await
        .expect("create must succeed against a seeded store");
    backend
        .start(request.id())
        .await
        .expect("start must boot the sandbox");
    let text = read_from_loopback(port, Duration::from_secs(180)).await;
    (created.created().to_vec(), GuestReport { text })
}

/// Stops and removes what [`start_and_read`] made.
///
/// Not a `Drop`: a sandbox left running holds a virtual machine, `Remove`
/// refuses a running container, and neither of those can be done from a
/// synchronous destructor. This order is the only one that leaves the host
/// clean.
async fn tear_down(
    backend: &ArcaBackend<gascan_arca::ChannelTransport>,
    request: &CreateRequest,
    created: Vec<RuntimeResource>,
) {
    backend.stop(request.id()).await.expect("stop must answer");
    await_state(
        backend,
        request,
        ContainerState::Stopped,
        Duration::from_secs(120),
    )
    .await;
    backend
        .remove(RemoveRequest::from_resources(created).expect("gascan-owned resources"))
        .await
        .expect("remove must delete the sandbox");
}

/// The project root must be readable inside the guest at `/workspace` and
/// writable back out to the host.
///
/// **`loopback_publish` was the only capability with an instrument, and this is
/// `project_mount`'s.** `lifecycle.rs` proves the engine reports a topology and
/// `Inspect` agrees with it, which is the store talking to itself: nothing in
/// this tier had ever established that the bind `sandboxContainerSpec` compiles
/// (`EngineCreate.swift:104`) becomes a filesystem the guest can see. A
/// `binds` array that was built correctly and then dropped on the way to
/// `createContainer` passes every other test in this crate.
///
/// **Both directions, and the second one does not go through the port.** The
/// host writes a token into the project root before `Create`; the guest's own
/// `Cmd` serves that file to whoever connects, which is the read direction. The
/// same `Cmd` writes a mark into the project root at boot, and the *host* reads
/// that file off its own disk afterwards -- no engine, no gRPC, no publish
/// involved in that half at all. A VirtioFS share that was mounted read-only,
/// or onto a copy, fails it.
///
/// **What this does NOT prove.** That the share is exactly the canonical root
/// and nothing wider: the mark lands in the directory the request names, and a
/// mount of some ancestor containing it would satisfy this. It says nothing
/// about the `writable` flag being honoured as a *refusal* -- gascan only ever
/// compiles a writable project mount (`policy.rs:392-406`), so a read-only one
/// is not a request this tier can make. And it proves nothing about a second
/// bind mount, which the contract does not permit
/// (`translate.rs:538-566` refuses more than one).
///
/// **It shares `ports.rs`'s dependency on a working publish for the read half**,
/// because a published port is the only channel out of an Arca guest this build
/// has -- see `common::answering`. The write half is the independent one.
/// MEASURED: with `translate.rs`'s `ports:` emptied, this test fails with
/// `Connection refused` alongside `ports.rs` and `limits.rs`, so a publish
/// regression reddens four tests and `ports.rs` is the one that names the cause.
///
/// **SEEN TO FAIL, twice, and the second one is what earns it a place.**
///
/// - `translate.rs`'s `guest_path` replaced by `/not-workspace`: FAILED after
///   180s with `connected and read nothing` -- the port published, the guest
///   had no `/workspace` to read.
///   `translate::tests::exactly_one_writable_project_mount_is_expressible`
///   failed too, and would have been enough on its own.
/// - Arca's `sandboxContainerSpec` starting `binds` empty, so the request
///   arrives intact and the engine drops the mount: FAILED the same way, and it
///   was the **only** test in this tier that failed. That is the mutation
///   nothing on the wire can see.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn the_project_root_is_readable_in_the_guest_and_writable_back_to_the_host() {
    const TO_GUEST: &str = "from-host.txt";
    const FROM_GUEST: &str = "from-guest.txt";
    const MARK: &str = "the-guest-wrote-this";

    let port = reserved_loopback_port();
    let token = format!("gascan-live-project-{port}");
    let images = tempfile::tempdir().expect("a temporary layout root");

    // One `Cmd`, two directions. The write happens once at boot; the read is
    // served on every connection.
    let program = format!(
        "printf %s {MARK} > {WORKSPACE_TARGET}/{FROM_GUEST}; {}",
        answering(port, &format!("cat {WORKSPACE_TARGET}/{TO_GUEST}"))
    );
    let layout = layout_running(
        &base_oci_layout(),
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        "gascan-live-project:latest",
        &["sh", "-c", &program],
    );

    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = ArcaBackend::new(engine.transport().await);
    let (_root, request) = policy_request_from_manifest(
        "project",
        &engine.image("gascan-live-project:latest"),
        &manifest(port, ""),
    );

    // The compiler's own decision, asserted before it is relied on: one mount,
    // writable, the canonical root at `/workspace`. A test that read the source
    // from somewhere else would be checking a path the engine was never given.
    assert_eq!(
        request.bind_mounts().len(),
        1,
        "the policy compiles exactly one bind mount"
    );
    let mount = &request.bind_mounts()[0];
    assert_eq!(mount.target, Utf8Path::new(WORKSPACE_TARGET));
    assert!(mount.writable, "the project mount is compiled writable");
    let host_root = mount.source.clone();

    std::fs::write(host_root.join(TO_GUEST), &token).expect("the host must seed the project root");

    let (created, report) = start_and_read(&backend, &request, port).await;
    tear_down(&backend, &request, created).await;
    assert_eq!(
        report.text.trim(),
        token,
        "the guest must serve the bytes the host wrote into the project root"
    );

    let written = std::fs::read_to_string(host_root.join(FROM_GUEST)).unwrap_or_else(|error| {
        panic!("the guest wrote nothing back to {host_root}/{FROM_GUEST}: {error}")
    });
    assert_eq!(
        written.trim(),
        MARK,
        "a writable project mount must carry the guest's write back to the host"
    );

    engine.kill().await;
}

/// This engine creates the three managed volumes, attaches all three to the
/// guest as block devices, and mounts **none** of them.
///
/// **This is the instrument that keeps `named_volumes` false, and it is a
/// negative claim on purpose.** The milestone's discipline is that no
/// capability flag moves without a live test driving the capability it names.
/// The mirror of that rule is that a flag left false needs a reason nobody has
/// to take on trust: a paragraph in a report goes stale, and the next person to
/// read `capabilities.namedVolumes = false` beside a `Create` that plainly
/// handles volumes will assume it is an oversight. **When Arca mounts them,
/// this test fails, and the failure is the instruction to flip the flag and
/// replace it with the positive assertion.**
///
/// **Creating a volume and mounting one are different capabilities, and only
/// the first works.** `lifecycle.rs` proves `Create` makes three volume
/// resources and `Remove` deletes them; that is a record in the engine's store,
/// and `list_resources` below re-checks it so this test cannot pass by the
/// volumes never having been made. `named_volumes` claims something else --
/// that `\(volume.name):\(volume.guestPath)` (`EngineCreate.swift:122`) reaches
/// the guest as a mount -- and it does not.
///
/// **MEASURED, and the request is not the part that is wrong.** The engine logs
/// `Creating EXT4 block device` for each volume at its declared size, then
/// `Resolved named volume ... format=ext4` for each, then
/// `Configured volume mounts mount_count=4 total_mounts=13`, with no warning
/// and no error. In the guest, `/proc/partitions` shows the three devices at
/// exactly the declared sizes -- 262144, 524288 and 1048576 1K-blocks for
/// 256MiB, 512MiB and 1GiB -- so the block images were built and attached
/// correctly. `/proc/mounts` names none of the three targets.
///
/// **The missing mount point was ruled out, which is why this image carries
/// one.** The first version of this test used a stock alpine, where
/// `/home/workspace` does not exist, and a missing directory is the obvious
/// explanation. So the image was given the three directories
/// (`layout_running_with_directories`), MEASURED as present in the guest --
/// `ls -la /home/workspace` lists `.cache`, `.config` and `.local` -- and
/// nothing changed: still three devices attached, still nothing mounted.
///
/// **The mechanism, read from Arca and not verified here.** Every block mount
/// Arca builds for itself passes `destination: ""`, commented "Empty to prevent
/// auto-mount by framework", and is mounted instead by vminitd at a hardcoded
/// `/mnt/writable` or `/mnt/layer{index}` during boot
/// (`OverlayFSMounter.swift:68-105`). A volume takes the other branch and
/// passes a real destination (`ContainerManager.swift:3942-3950`), which
/// nothing in that boot sequence knows about. That is a reading of the source
/// consistent with the measurement, not a second measurement: what is
/// established here is that the mount does not happen, not which line prevents
/// it.
///
/// **gascan cannot avoid the branch.** A capacity selects the `block` driver
/// and no capacity selects `local` (`EngineCreate.swift:321-326`), and
/// `policy.rs` compiles a capacity for all three volumes from `Storage`, whose
/// every field is required to be greater than zero. Whether the `local` driver
/// mounts is therefore untested and untestable from this tier.
///
/// **What this does NOT prove.** That the engine could not mount a volume under
/// any arrangement -- only that it does not under the one gascan compiles. It
/// says nothing about persistence, read-only volumes, or capacity enforcement,
/// none of which can be reached while the mount is absent.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn the_managed_volumes_are_attached_to_the_guest_but_this_engine_mounts_none_of_them() {
    const MIB: u64 = 1024 * 1024;

    let port = reserved_loopback_port();
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = layout_running_with_directories(
        &base_oci_layout(),
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        "gascan-live-volumes:latest",
        &[
            "sh",
            "-c",
            &answering(port, "{ echo ---mounts---; cat /proc/mounts; }"),
        ],
        // Without these the guest has no mount point and the result is
        // confounded -- see the note above, and
        // `layout_running_with_directories`.
        &[TOOLS_ROOT, CACHE_ROOT, CONFIG_ROOT],
    );

    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = ArcaBackend::new(engine.transport().await);
    let (_root, request) = policy_request_from_manifest(
        "volumes",
        &engine.image("gascan-live-volumes:latest"),
        // Small and unequal: the defaults are 10GiB, 10GiB and 1GiB
        // (`manifest.rs:9-11`), two of them equal, so a swap between `tools` and
        // `cache` would be invisible to the positive test this becomes.
        &manifest(
            port,
            "\n[storage]\ntools = '256MiB'\ncache = '512MiB'\nconfig = '1GiB'\n",
        ),
    );

    // What the compiler made of that manifest, asserted rather than assumed: a
    // test that guessed these would be checking the guest against paths the
    // engine was never sent.
    let expected: Vec<(String, u64)> = request
        .volumes()
        .iter()
        .map(|volume| (volume.target.to_string(), volume.capacity_bytes))
        .collect();
    assert_eq!(
        expected,
        vec![
            (TOOLS_ROOT.to_owned(), 256 * MIB),
            (CACHE_ROOT.to_owned(), 512 * MIB),
            (CONFIG_ROOT.to_owned(), 1024 * MIB),
        ],
        "the policy must compile the three declared capacities onto the three managed targets"
    );

    let (created, report) = start_and_read(&backend, &request, port).await;

    // The store really does hold all three, so "nothing is mounted" cannot be
    // read as "nothing was created". This is the half that already worked.
    let held = backend
        .list_resources()
        .await
        .expect("list_resources must answer");
    for volume in request.volumes() {
        assert!(
            held.iter().any(|resource| {
                resource.name() == volume.name && resource.kind() == ResourceKind::Volume
            }),
            "the engine must hold volume {}, or this test is measuring the wrong failure",
            volume.name
        );
    }

    for (target, _) in &expected {
        assert!(
            report.mount_at(target).is_none(),
            "{target} IS mounted in the guest. This engine has gained named-volume \
             mounting: flip `capabilities.namedVolumes` in SandboxEngineService.swift, \
             replace this test with the positive one it describes, and update \
             read_rpcs::capabilities_report_only_what_this_engine_build_implements. \
             The guest reported:\n{}",
            report.text
        );
    }

    tear_down(&backend, &request, created).await;

    engine.kill().await;
}
