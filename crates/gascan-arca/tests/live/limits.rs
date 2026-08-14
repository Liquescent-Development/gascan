use crate::common::{
    LiveEngine, answering, await_state, base_oci_layout, layout_running,
    policy_request_from_manifest, read_from_loopback, report_section, reserved_loopback_port,
};
use camino::Utf8Path;
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, RemoveRequest, RuntimeBackend};
use std::time::Duration;

/// The one line of a guest section, or the whole report and what was missing.
fn only_line<'a>(report: &'a str, section: &str) -> &'a str {
    let lines = report_section(report, section);
    assert_eq!(
        lines.len(),
        1,
        "the guest's {section} section must be one line; it said {lines:?} in:\n{report}"
    );
    lines[0].trim()
}

/// The requested CPU count and memory must reach the guest as the limits it
/// actually runs under.
///
/// **`Inspect` cannot serve as evidence here, for exactly the reason it could
/// not serve for a published port.** It reports what the STORE holds, including
/// limits that were recorded and never applied, because that is what drift
/// detection compares against. `EngineCreate.swift:205-206` turns the request's
/// `cpus` and `memory_bytes` into `nanoCpus` and `memory`, and every step after
/// that is somewhere they can be lost silently: `createContainer` takes them as
/// two of its thirty-odd optional parameters, `containerConfig.cpus` is only
/// assigned `if cpus > 0`, and `memoryInBytes` falls back to a 4GiB default
/// (`ContainerManager.swift:1453-1512`). An engine that dropped both reports a
/// successful `Create` and an `Inspect` naming the limits it was asked for.
///
/// **The instrument is the guest's own cgroup, and it is exact -- no band, no
/// tolerance.** MEASURED against this engine with `cpus = 3` and
/// `memory = '3GiB'`: `/sys/fs/cgroup/cpu.max` reads `300000 100000` and
/// `/sys/fs/cgroup/memory.max` reads `3221225472`, which is 3GiB to the byte.
/// The guest is in a cgroup namespace -- `/proc/mounts` shows `cgroup2` at
/// `/sys/fs/cgroup` -- so that path is its *own* limit rather than the host's
/// or the VM's root. This is the instrument `gascan-apple`'s
/// `resources::cpu_and_memory_limits_are_observable_in_guest` uses, which is
/// what lets the two backends be compared at all.
///
/// **`nproc` and `/proc/meminfo` were tried first and are the wrong
/// instruments**, recorded here because they look like the obvious ones.
/// MEASURED: a sandbox asking for 1 CPU boots a guest reporting **2**, and one
/// asking for 3 reports **4** -- the VM carries a vCPU beyond the container's
/// allowance, which is the `cpuOverhead: 1` the sibling backend's own fixture
/// records (`gascan-apple/tests/live/resources.rs:18`). MEASURED likewise, a
/// 1GiB sandbox reports `MemTotal: 1125564 kB`, *larger* than the GiB it asked
/// for, while a 3GiB one reports about `3118360 kB`, smaller. **`MemTotal` is
/// not even stable across boots** -- Task 14's reviewer measured the 3GiB case
/// at `3118352`, `3118360` and `3118364 kB` over four boots, which is the second
/// reason it cannot be asserted on. The cgroup values below were exact and
/// identical across all four. VM sizing tracks the
/// request loosely and in both directions; the cgroup tracks it exactly,
/// because the cgroup is the limit.
///
/// **The declared numbers are unlike every default.** One CPU and 1GiB against
/// gascan's own defaults of 4 CPUs and 8GiB (`policy.rs:14-16`) and against
/// ContainerBridge's 4GiB fallback, so a build that ignored the request fails
/// an equality rather than sitting inside a band.
///
/// **SEEN TO FAIL, twice, and the second one is what earns it a place.**
///
/// - `translate.rs`'s `resources: Some(resource_limits(...))` replaced by
///   `None`, so the engine is told nothing: FAILED with the guest reporting
///   `400000 100000`. `backend_unary::create_sends_every_field_of_the_compiled_request`
///   failed too, and would have been enough on its own.
/// - Arca's `sandboxContainerSpec` forced to pass `nanoCpus: nil, memory: nil`,
///   so the request arrives intact and the engine drops it: FAILED with
///   `400000 100000` and `4294967296`, and it was the **only** test in this
///   tier that failed. That is the mutation nothing on the wire can see, and it
///   is the one this test exists for.
///
/// **What this does NOT prove.** That the limit is *enforced* against a guest
/// trying to exceed it -- nothing here spawns until it is throttled or
/// allocates until it is killed, and either would be a test of the Linux
/// scheduler and OOM killer rather than of the engine. It says nothing about
/// disk or process-count limits, which this engine refuses outright as
/// `unsupported_capability` (`EngineCreate.swift:187-200`) and which `policy.rs`
/// never compiles. And it reads through a published port, like every guest
/// observation in this build; see `common::answering`.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; the pinned engine implements none of these RPCs"]
async fn the_requested_cpu_and_memory_limits_are_the_guests_own_cgroup_limits() {
    const DECLARED_CPUS: u64 = 1;
    const DECLARED_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;

    let port = reserved_loopback_port();
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = layout_running(
        &base_oci_layout(),
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        "gascan-live-limits:latest",
        &[
            "sh",
            "-c",
            // Raw, and read in Rust: a guest that answered `yes` to a question
            // it evaluated itself would be indistinguishable from a guest that
            // answered `yes` because the limits were applied. `2>&1` so that an
            // absent cgroup file arrives as the message saying so rather than
            // as an empty section.
            &answering(
                port,
                "{ echo ---cpu---; cat /sys/fs/cgroup/cpu.max 2>&1; \
                   echo ---memory---; cat /sys/fs/cgroup/memory.max 2>&1; }",
            ),
        ],
    );

    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = ArcaBackend::new(engine.transport().await);
    let (_root, request) = policy_request_from_manifest(
        "limits",
        &engine.image("gascan-live-limits:latest"),
        &format!(
            "version = 1\nnetwork = 'networked'\nuser = 'root'\n\n\
             [ports]\nreport = {port}\n\n\
             [resources]\ncpus = {DECLARED_CPUS}\nmemory = '1GiB'\n"
        ),
    );
    // What the compiler made of that manifest, asserted before the guest is
    // checked against it: a test that guessed these would be measuring numbers
    // the engine was never sent.
    assert_eq!(
        (
            request.resources().cpus,
            request.resources().memory_bytes,
            request.resources().disk_bytes,
            request.resources().process_count
        ),
        (
            Some(u16::try_from(DECLARED_CPUS).expect("a small cpu count")),
            Some(DECLARED_MEMORY_BYTES),
            None,
            None
        ),
        "the policy must compile exactly the two limits this engine can apply"
    );

    let created = backend
        .create(request.clone())
        .await
        .expect("create with resource limits must succeed");
    backend
        .start(request.id())
        .await
        .expect("start must boot the sandbox");
    let report = read_from_loopback(port, Duration::from_secs(180)).await;

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
        .expect("remove must delete the sandbox");

    // `cpu.max` is `<quota> <period>`, both in microseconds, and the CPU count
    // is their quotient. The period is read rather than assumed to be 100000:
    // a kernel using another one would make a hardcoded quota compare against
    // the wrong number.
    let quota_and_period: Vec<u64> = only_line(&report, "cpu")
        .split_whitespace()
        .map(|field| {
            field.parse::<u64>().unwrap_or_else(|error| {
                panic!("the guest's cpu.max is not two numbers ({error}):\n{report}")
            })
        })
        .collect();
    assert_eq!(
        quota_and_period.len(),
        2,
        "cpu.max must be a quota and a period:\n{report}"
    );
    assert_eq!(
        quota_and_period[0],
        quota_and_period[1] * DECLARED_CPUS,
        "the guest's CPU quota must be exactly the {DECLARED_CPUS} CPU(s) the manifest \
         declared, at its own period of {}us:\n{report}",
        quota_and_period[1]
    );

    let memory = only_line(&report, "memory");
    assert_eq!(
        memory.parse::<u64>().ok(),
        Some(DECLARED_MEMORY_BYTES),
        "the guest's memory limit must be the declared {DECLARED_MEMORY_BYTES} bytes to the \
         byte; 4294967296 would mean ContainerBridge's 4GiB default was used instead:\n{report}"
    );

    engine.kill().await;
}
