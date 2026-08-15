//! Whether the engine survives its own `SIGTERM`, measured as a rate.
//!
//! **A single clean shutdown proves nothing here, which is why this module
//! exists at all.** The defect it measures was recorded at 6 crashes in 32
//! shutdowns, so four runs in five are clean by chance: any test that stopped
//! one engine and asserted its status would have passed 81% of the time against
//! a broken engine. Every test below drives one engine per iteration and reports
//! the whole distribution, so the number it prints is a rate rather than a
//! sample.
//!
//! **The three workloads are a controlled comparison and not three copies of one
//! test.** The defect was reported as happening "once containers have been
//! created", and that description names a correlate rather than a cause. These
//! vary one thing at a time -- whether anything ever connected, whether a client
//! channel is still open when the signal lands, and whether a container was
//! created and removed first -- so a rate that differs between them says which
//! of the three is load-bearing. Running them A-then-B on a drifting machine
//! would not: each is its own single run of its own `iterations`.
//!
//! **They are deliberately not `#[ignore]`-free and not fast.** The container
//! workload boots a real virtual machine per iteration.

use crate::common::{
    LiveEngine, await_state, base_oci_layout, layout_running, policy_request_from_manifest,
};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, RemoveRequest, RuntimeBackend};
use std::collections::BTreeMap;
use std::fmt;
use std::process::ExitStatus;
use std::time::Duration;

/// The tag the container workload loads its image under.
const TAG: &str = "gascan-live-shutdown:latest";

/// `user = 'root'` for the reason `lifecycle.rs` records: a stock alpine has no
/// `workspace` user, so a start would fail on the image rather than on anything
/// under test.
const MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

/// How many engines each test stops.
///
/// **32 is the recorded baseline's own denominator**, so a zero here is directly
/// comparable to the 6-in-32 that opened this. It is also enough for a zero to
/// mean something: at the recorded 19% a clean sweep of 32 happens 0.11% of the
/// time, where a sweep of 5 happens 37% of the time and would prove nothing.
const ITERATIONS: usize = 32;

/// What one engine is put through before its pipe closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Workload {
    /// Started and stopped, with nothing holding a connection.
    ///
    /// Not "nothing ever connected": `await_socket` dials the engine to decide
    /// it is up and drops each probe, so even this engine has accepted and
    /// closed connections. What it has never had is one still open.
    Untouched,
    /// A client channel opened and still held when the signal lands.
    OpenChannel,
    /// A container created, started, stopped and removed, channel still held.
    ///
    /// The full round trip rather than a live container, because that is what
    /// the tests the 6-in-32 was measured across do: `ports.rs` and
    /// `lifecycle.rs` both remove before they kill.
    RemovedContainer,
}

impl fmt::Display for Workload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let described = match self {
            Self::Untouched => "an engine nothing was holding a connection to",
            Self::OpenChannel => "an engine with a client channel still open",
            Self::RemovedContainer => "an engine that had created and removed a container",
        };
        formatter.write_str(described)
    }
}

/// How a run of one workload came out, by exit status.
///
/// The whole distribution and not a pass/fail, because the failure mode is a
/// rate: a report naming which statuses appeared and how often is what makes a
/// before and an after comparable, and what distinguishes "fixed" from "did not
/// happen to fire this time".
struct Report {
    workload: Workload,
    iterations: usize,
    unclean: usize,
    /// Rendered status to the number of shutdowns that ended with it.
    counts: BTreeMap<String, usize>,
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let breakdown: Vec<String> = self
            .counts
            .iter()
            .map(|(status, count)| format!("{count} x {status}"))
            .collect();
        #[expect(
            clippy::cast_precision_loss,
            reason = "both counts are tens; the percentage is for a human to read"
        )]
        let percentage = 100.0 * self.unclean as f64 / self.iterations as f64;
        write!(
            formatter,
            "{} of {} shutdowns of {} were not clean ({percentage:.0}%): {}",
            self.unclean,
            self.iterations,
            self.workload,
            breakdown.join(", "),
        )
    }
}

/// Stops `iterations` engines, one at a time, and reports how each exited.
///
/// Sequential on purpose. Concurrent engines contend for vmnet subnets and for
/// the machine, which would make the rate a measurement of the load rather than
/// of the engine.
async fn rate(workload: Workload, iterations: usize) -> Report {
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = staying_up(Utf8Path::from_path(images.path()).expect("a utf-8 path"));

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut unclean = 0;
    for _ in 0..iterations {
        let status = one_shutdown(workload, &layout).await;
        if !status.success() {
            unclean += 1;
        }
        *counts.entry(format!("{status}")).or_default() += 1;
    }
    Report {
        workload,
        iterations,
        unclean,
        counts,
    }
}

/// An image whose only job is to stay up, so the container can be observed
/// `Running` and then stopped, the way `lifecycle.rs` does it.
fn staying_up(destination: &Utf8Path) -> Utf8PathBuf {
    layout_running(
        &base_oci_layout(),
        destination,
        TAG,
        &["sh", "-c", "while :; do sleep 1; done"],
    )
}

/// Drives one engine through `workload` and returns the status it exited with.
async fn one_shutdown(workload: Workload, layout: &Utf8Path) -> ExitStatus {
    match workload {
        Workload::Untouched => LiveEngine::start().await.stop().await,
        Workload::OpenChannel => {
            let engine = LiveEngine::start().await;
            // Held across the stop, which is the whole point of this workload:
            // dropping it here would make this the `Untouched` case again.
            let _transport = engine.transport().await;
            engine.stop().await
        }
        Workload::RemovedContainer => {
            let engine = LiveEngine::start_with_images(&[layout]).await;
            let backend = ArcaBackend::new(engine.transport().await);
            let (_root, request) =
                policy_request_from_manifest("shutdown", &engine.image(TAG), MANIFEST);

            backend
                .prepare_image(request.image())
                .await
                .expect("the store holds the image the request names");
            let created = backend
                .create(request.clone())
                .await
                .expect("create against a seeded store must succeed");
            backend
                .start(request.id())
                .await
                .expect("start must boot the sandbox");
            await_state(
                &backend,
                &request,
                ContainerState::Running,
                Duration::from_secs(120),
            )
            .await;

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
                        .expect("gascan-owned resources"),
                )
                .await
                .expect("remove must delete the sandbox");

            engine.stop().await
        }
    }
}

/// The control: without a connection to reap, the engine must always exit clean.
///
/// **A control that goes red invalidates the other two rather than adding a
/// finding**, because it would mean the shutdown path is broken in a way that
/// has nothing to do with what those vary. It is here for that reason and not
/// because anyone doubts it.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; stops 32 engines and takes minutes"]
async fn the_engine_exits_cleanly_with_nothing_holding_a_connection() {
    let report = rate(Workload::Untouched, ITERATIONS).await;
    println!("{report}");
    assert_eq!(report.unclean, 0, "{report}");
}

/// A client channel still open when the signal lands must not change the outcome.
///
/// **This is the discriminator.** The defect was described as happening once
/// containers had been created; an accepted connection outliving the listening
/// socket is the other candidate, and it needs no container at all. A rate here
/// that matches the container workload's says the container was a correlate.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; stops 32 engines and takes minutes"]
async fn the_engine_exits_cleanly_with_a_client_channel_still_open() {
    let report = rate(Workload::OpenChannel, ITERATIONS).await;
    println!("{report}");
    assert_eq!(report.unclean, 0, "{report}");
}

/// The reported case: an engine that has created a container must still exit
/// cleanly.
///
/// **This is the one the ruling is about**, and it is the test `LiveEngine::kill`
/// points at. It boots and tears down a real virtual machine 32 times, which is
/// what makes it the slowest thing in this tier and why the two cheaper
/// workloads above exist beside it rather than inside it.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout; boots 32 virtual machines and takes tens of minutes"]
async fn the_engine_exits_cleanly_after_a_container_has_been_created() {
    let report = rate(Workload::RemovedContainer, ITERATIONS).await;
    println!("{report}");
    assert_eq!(report.unclean, 0, "{report}");
}
