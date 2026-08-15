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
    /// `Option` rather than a panic, because the volume test turns on both
    /// answers: the three targets must resolve and `/home/workspace`, its
    /// control, must not.
    fn mount_at(&self, target: &str) -> Option<(&str, &str)> {
        report_section(&self.text, "mounts")
            .into_iter()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let device = fields.next()?;
                (fields.next()? == target).then(|| (device, fields.next().unwrap_or_default()))
            })
    }

    /// The size in 1K blocks that the kernel gives `device`, if it names one.
    ///
    /// `device` is a `/proc/mounts` source such as `/dev/vde`; `/proc/partitions`
    /// names the same device `vde`. This is the block device's own size, not the
    /// filesystem's usable capacity -- `df` would report the latter, which is
    /// smaller than the declared capacity by whatever ext4 spent on metadata and
    /// is therefore no use for telling a 256MiB volume from a 512MiB one.
    fn blocks_of(&self, device: &str) -> Option<u64> {
        let name = device.strip_prefix("/dev/")?;
        report_section(&self.text, "partitions")
            .into_iter()
            .find_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                // major minor #blocks name
                (fields.len() == 4 && fields[3] == name).then(|| fields[2].parse().ok())?
            })
    }

    /// What the guest read back out of the probe file it wrote under `target`.
    ///
    /// The guest emits `<target> <whatever came back>`, with the write and the
    /// read run inside a subshell whose stderr is captured, so a read-only mount
    /// arrives here as the shell's own complaint rather than as silence. `None`
    /// means the guest never mentioned `target` at all.
    fn readback(&self, target: &str) -> Option<&str> {
        report_section(&self.text, "readback")
            .into_iter()
            .find_map(|line| match line.trim().split_once(char::is_whitespace) {
                Some((key, value)) => (key == target).then_some(value.trim()),
                // The guest printed the key and nothing else. That is a distinct
                // finding from never printing the key, and the caller reports it
                // as such.
                None => (line.trim() == target).then_some(""),
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

/// Waits for the guest to write `path` on the host side of the project mount.
///
/// The guest writes it at boot and the host has no signal for when that
/// happened, so this polls. It exists so the guest -> host direction can be
/// asserted **without** a working publish -- see the ordering note in
/// `the_project_root_is_readable_in_the_guest_and_writable_back_to_the_host`.
///
/// The panic carries the last io error rather than just the path: "no such
/// file" and "permission denied" are different findings, and a mount that
/// appeared read-only would otherwise read as a mount that never appeared.
async fn await_host_file(path: &Utf8Path, bound: Duration) -> String {
    let started = std::time::Instant::now();
    // Declared without an initialiser: every path through the match below
    // assigns it before the assert reads it, and a placeholder here is a value
    // no run can ever observe -- which clippy says, correctly.
    let mut last;
    loop {
        match std::fs::read_to_string(path) {
            Ok(text) if !text.trim().is_empty() => return text,
            Ok(_) => last = String::from("present but empty"),
            Err(error) => last = error.to_string(),
        }
        assert!(
            started.elapsed() < bound,
            "the guest wrote nothing back to {path} within {:.1}s; last attempt: {last}",
            bound.as_secs_f64()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
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
/// has -- see `common::answering`. MEASURED: with `translate.rs`'s `ports:`
/// emptied, this test fails with `Connection refused` alongside `ports.rs` and
/// `limits.rs`, so a publish regression reddens four tests and `ports.rs` is the
/// one that names the cause.
///
/// **The write half does NOT depend on a publish, and is asserted FIRST so that
/// it genuinely does not.** CORRECTED after review: this comment used to call it
/// "the independent one" while the code read it *after* `read_from_loopback`,
/// so a publish outage panicked before it ever ran. Independent in what it
/// observes is not independent in whether it executes, and the reviewer proved
/// the difference by emptying `binds` at the engine's own `createContainer` call
/// site -- the test died on `Connection refused` and never reached the disk
/// assertion at all.
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

    let created = backend
        .create(request.clone())
        .await
        .expect("create must succeed against a seeded store");
    backend
        .start(request.id())
        .await
        .expect("start must boot the sandbox");

    // THE GUEST -> HOST DIRECTION IS ASSERTED FIRST, AND THE ORDER IS THE POINT.
    //
    // It used to run after `read_from_loopback`, which made it unreachable
    // whenever publishing broke: the loopback read panics, and the direction
    // that needs no publish at all never got tested. MEASURED by Task 14's
    // reviewer -- with `binds` emptied at the engine's `createContainer` call
    // site the test died on `Connection refused` and never reached this
    // assertion. An instrument that only runs when an unrelated capability is
    // healthy is not independent, however independent what it observes may be.
    //
    // Polled rather than read once, because the guest writes this at boot and
    // the host has no signal for when that happened. This is the only wait in
    // the test that is not a publish.
    let written = await_host_file(&host_root.join(FROM_GUEST), Duration::from_secs(180)).await;
    assert_eq!(
        written.trim(),
        MARK,
        "a writable project mount must carry the guest's write back to the host"
    );

    let report = GuestReport {
        text: read_from_loopback(port, Duration::from_secs(180)).await,
    };
    tear_down(&backend, &request, created.created().to_vec()).await;
    assert_eq!(
        report.text.trim(),
        token,
        "the guest must serve the bytes the host wrote into the project root"
    );

    engine.kill().await;
}

/// The three managed volumes must be mounted at the three targets the policy
/// declared, each backed by its own block device of the declared size, and each
/// writable.
///
/// **This is `named_volumes`'s instrument.** It replaces
/// `the_managed_volumes_are_attached_to_the_guest_but_this_engine_mounts_none_of_them`,
/// which asserted the absence and failed the day Arca gained the mount, exactly
/// as its failure message instructed.
///
/// **Three claims, and the second is the one that costs anything.** That
/// something is mounted at each target; that it is *the right* something; and
/// that it is writable.
///
/// - *Mounted*: `/proc/mounts` names the target.
/// - *The right one*: the device at each target has the size that target's
///   volume was declared with. The capacities are 256MiB, 512MiB and 1GiB --
///   deliberately unequal, because the gascan defaults are 10GiB, 10GiB and 1GiB
///   (`manifest.rs:9-11`) and a `tools`/`cache` swap under those would be
///   invisible. The size comes from `/proc/partitions`, which reports the block
///   device's own size, rather than from `df`, which reports the filesystem's
///   usable capacity after ext4 metadata and would need a tolerance.
/// - *Writable*: the guest writes a token unique to each target into that
///   target and reads it back. A volume mounted read-only returns the shell's
///   complaint instead, captured by the subshell redirect so it arrives here
///   rather than vanishing onto stderr.
///
/// **The writability half proves the least, and the run below is why it is said
/// out loud.** Against the unfixed engine it PASSED -- the three targets exist
/// in the image, so the write landed in the container's own overlay and read
/// back perfectly while no volume was mounted anywhere. It catches a read-only
/// mount; it cannot catch a missing one. The mount and size assertions carry
/// the claim.
///
/// **Two controls, because presence assertions have their own way of lying.**
/// `/` must resolve, or the mount parser is seeing nothing and would have failed
/// the presence checks for the wrong reason. `/home/workspace` -- the parent the
/// image creates for the three targets, and not itself a volume -- must NOT
/// resolve, or the parser is matching everything and "mounted at the declared
/// target" means nothing. And the three devices must be distinct from each other
/// and from the device at `/`: three targets that all reported the rootfs would
/// otherwise satisfy every assertion above except the sizes.
///
/// **SEEN TO FAIL, three times, and each failure names a different part.**
///
/// - Against `~/code/arca` at 6c77bb8 with its `containerization` submodule at
///   f02cdf9 -- engine and vminit both rebuilt from those trees -- it failed at
///   the first target: `/home/workspace/.local is not mounted in the guest`.
///   The guest's overlay read
///   `lowerdir=/mnt/layer4:/mnt/layer3:/mnt/layer2:/mnt/layer1:/mnt/layer0` for
///   a two-layer image, because vminitd counted `/dev/vd` letters upward and
///   took the three volume devices for layers, and `/proc/partitions` listed
///   `vde` 262144, `vdf` 524288 and `vdg` 1048576 -- attached, sized correctly,
///   mounted nowhere.
/// - With Arca's OCI-spec filter widened back to dropping every `/dev/vd`
///   source (`LinuxContainer.swift:797`), so the volumes never reach the
///   container's spec: same first failure, and it was the ONLY live test in
///   this tier that failed.
/// - With the engine's `createContainer` handoff giving each volume another
///   volume's capacity (`SandboxEngineService.swift:336`): it failed on the
///   size, `/home/workspace/.local is backed by /dev/vde, which is 1048576
///   1K-blocks; the volume declared for /home/workspace/.local is 268435456
///   bytes`, and again it was the only live test that failed. That is the
///   mutation the unequal capacities exist for.
///
/// **What this does NOT prove.** Persistence across a stop/start or across
/// containers; read-only volumes, which gascan never compiles
/// (`policy.rs` gives every volume a capacity, and a capacity selects the
/// `block` driver -- `EngineCreate.swift:321-326`); and capacity *enforcement*,
/// which is a quota question the device size does not answer.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn the_managed_volumes_are_mounted_at_their_declared_targets_and_writable() {
    const MIB: u64 = 1024 * 1024;
    const PROBE: &str = "gascan-live-volume-probe";
    /// The parent the image creates for all three targets. It is not a volume,
    /// so nothing may be mounted on it.
    const UNMOUNTED_PARENT: &str = "/home/workspace";

    let port = reserved_loopback_port();
    let images = tempfile::tempdir().expect("a temporary layout root");

    // A token per target, so a readback cannot be satisfied by another target's
    // write, and the port keeps them distinct from a previous run's leftovers.
    let tokens: Vec<(&str, String)> = vec![
        (TOOLS_ROOT, format!("tools-{port}")),
        (CACHE_ROOT, format!("cache-{port}")),
        (CONFIG_ROOT, format!("config-{port}")),
    ];

    // Write then read, inside a subshell whose stderr is folded into the
    // captured output: a read-only or absent mount must arrive in the report as
    // the shell's own message, not as an empty field.
    let probes: String = tokens
        .iter()
        .map(|(target, token)| {
            format!(
                "printf '{target} %s\\n' \"$( (printf %s {token} > {target}/{PROBE} \
                 && cat {target}/{PROBE}) 2>&1 )\"; "
            )
        })
        .collect();
    let layout = layout_running_with_directories(
        &base_oci_layout(),
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        "gascan-live-volumes:latest",
        &[
            "sh",
            "-c",
            &answering(
                port,
                &format!(
                    "{{ echo ---mounts---; cat /proc/mounts; \
                     echo ---partitions---; cat /proc/partitions; \
                     echo ---readback---; {probes} }}"
                ),
            ),
        ],
        // The mount points, for the reason `layout_running_with_directories`
        // records: a target that does not exist in the image would confound a
        // missing mount with a missing directory.
        &[TOOLS_ROOT, CACHE_ROOT, CONFIG_ROOT],
    );

    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = ArcaBackend::new(engine.transport().await);
    let (_root, request) = policy_request_from_manifest(
        "volumes",
        &engine.image("gascan-live-volumes:latest"),
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

    // The engine's own record of the three, kept from the negative test this
    // replaces: a mount that appeared without a volume behind it would be a
    // different finding entirely.
    let held = backend
        .list_resources()
        .await
        .expect("list_resources must answer");
    for volume in request.volumes() {
        assert!(
            held.iter().any(|resource| {
                resource.name() == volume.name && resource.kind() == ResourceKind::Volume
            }),
            "the engine must hold volume {}, or this test is measuring the wrong thing",
            volume.name
        );
    }

    // CONTROL, first half: the parser can see a mount.
    let (root_device, _) = report.mount_at("/").unwrap_or_else(|| {
        panic!(
            "the mount parser resolved nothing at /, so it can see no mounts at all \
             and everything below would be a parse failure wearing a mount failure's \
             clothes. The guest reported:\n{}",
            report.text
        )
    });

    // CONTROL, second half: and it does not see one everywhere. `/home/workspace`
    // exists in the image and carries no volume.
    assert!(
        report.mount_at(UNMOUNTED_PARENT).is_none(),
        "{UNMOUNTED_PARENT} resolved to a mount, but no volume targets it. The \
         parser is matching more than it should and the assertions below prove \
         nothing. The guest reported:\n{}",
        report.text
    );

    let mut devices: Vec<&str> = Vec::new();
    for (target, capacity) in &expected {
        let (device, filesystem) = report.mount_at(target).unwrap_or_else(|| {
            panic!(
                "{target} is not mounted in the guest, so this engine does not mount \
                 named volumes. The guest reported:\n{}",
                report.text
            )
        });
        assert_eq!(
            filesystem, "ext4",
            "{target} is mounted from {device} as {filesystem}, not the ext4 block \
             device the volume driver formats. The guest reported:\n{}",
            report.text
        );

        // The size is what identifies WHICH volume landed here. Equal capacities
        // would make this assertion pass under a swap; they are unequal on
        // purpose.
        let blocks = report.blocks_of(device).unwrap_or_else(|| {
            panic!(
                "{device} is mounted at {target} but /proc/partitions does not list it, \
                 so its size cannot be checked and a swapped volume would go unnoticed. \
                 The guest reported:\n{}",
                report.text
            )
        });
        assert_eq!(
            blocks,
            capacity / 1024,
            "{target} is backed by {device}, which is {blocks} 1K-blocks; the volume \
             declared for {target} is {capacity} bytes. A volume of the wrong capacity \
             is mounted here. The guest reported:\n{}",
            report.text
        );

        devices.push(device);
    }

    // CONTROL, third part: three targets reporting one device would satisfy the
    // presence checks and only the sizes would catch it -- and only then because
    // the capacities differ. Say it directly.
    assert!(
        !devices.contains(&root_device),
        "a managed volume target resolved to {root_device}, the device at /. \
         These are not volume mounts. The guest reported:\n{}",
        report.text
    );
    let mut distinct = devices.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        devices.len(),
        "the three targets resolved to {devices:?}, which is not three distinct \
         devices. The guest reported:\n{}",
        report.text
    );

    // Writable, and each target its own. A volume mounted read-only -- what the
    // old boot sequence did to these devices -- fails here with the shell's
    // message in place of the token.
    for (target, token) in &tokens {
        assert_eq!(
            report.readback(target),
            Some(token.as_str()),
            "{target} did not return the token the guest wrote into it, so it is not \
             writable. The guest reported:\n{}",
            report.text
        );
    }

    tear_down(&backend, &request, created).await;

    engine.kill().await;
}
