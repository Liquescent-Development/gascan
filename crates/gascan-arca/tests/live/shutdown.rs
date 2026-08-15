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
//! **The three workloads are a REGRESSION GUARD, not the comparison that
//! produced the finding**, and an earlier version of this paragraph claimed
//! otherwise. It said they were "a controlled comparison ... running them
//! A-then-B on a drifting machine would not [say which variable is
//! load-bearing]" -- but they are three `#[tokio::test]` functions in one
//! binary, and run the documented way they execute strictly one after another
//! over tens of minutes, which is A-then-B exactly. The comparison that
//! disproved "it only happens once containers have been created" interleaved
//! two engine BINARIES by hand, round by round; it is recorded in Gas Can
//! `3290af6` and Arca `9fac267`, and nothing in this file reproduces it.
//!
//! What the three do give, independently and without needing to be compared, is
//! coverage of the conditions the defect was thought to require: no held
//! connection, a held client channel, and a container created and removed. Each
//! passes or fails on its own rate. Re-deriving the comparison would need one
//! test that interleaves the workloads round-robin inside a single loop.
//!
//! **A clean count here means "the drains COMPLETED" only against Arca
//! `c68bd0a` or later.** Every assertion below reduces to `ExitStatus::success`,
//! and until that commit the engine exited 0 both when a drain finished and when
//! it gave up at its ten-second grace -- so against an older engine these zeros
//! mean only "did not crash", which is what they were originally built to say.
//! Nothing on this side pins which engine is under test: `GASCAN_ARCA_ENGINE_BIN`
//! names whatever the operator built.
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

/// What one engine is put through before its pipe closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Workload {
    /// Started and stopped, with nothing deliberately holding a connection.
    ///
    /// Not "nothing ever connected", and not even "nothing still connected":
    /// `await_socket` dials the engine to decide it is up and drops each probe,
    /// and nothing here waits for the engine to finish reaping the last one
    /// before the signal lands. So this workload is *no connection is held on
    /// purpose*, which is weaker than the name suggests and is why its measured
    /// pre-fix rate (1/96) is the lowest of the three.
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

impl Workload {
    /// How many engines this workload stops, and it differs per workload
    /// because the rates do.
    ///
    /// **A sample size is only justified against the rate it has to detect, and
    /// one number for all three was wrong.** The figure this file used to carry
    /// -- 32, because "at the recorded 19% a clean sweep of 32 happens 0.11% of
    /// the time" -- took the ORIGINAL mixed observation (6-in-32) and applied it
    /// to three workloads whose own measured pre-fix rates are 1/96, 5/96 and
    /// 12/32. At 1/96 a sweep of 32 comes up all clean **71% of the time**,
    /// against a broken engine: worse than the sweep of 5 that same comment
    /// rejected as proving nothing.
    ///
    /// So each workload gets the count that puts a false green under 1% against
    /// its own rate, `(1 - p)^n < 0.01`:
    ///
    /// | workload | pre-fix rate | n | false green |
    /// |---|---|---|---|
    /// | `Untouched` | 1/96 = 0.0104 | 440 | 0.9978% |
    /// | `OpenChannel` | 5/96 = 0.0521 | 96 | 0.5888% |
    /// | `RemovedContainer` | 12/32 = 0.375 | 32 | 0.0000294% |
    ///
    /// **440 is the smallest n that clears the bar** -- 439 gives 1.0083% -- which is a
    /// stronger statement than a rounded figure and is why it is written out
    /// rather than shortened. The first version of this table rounded 0.9978%
    /// down to "0.99%" and 0.5888% up to "0.60%", which is the same
    /// truncate-rather-than-round error the round before it was corrected for.
    /// **Two consecutive rounds got a probability in this file wrong; print
    /// what the expression evaluates to.**
    ///
    /// The two cheap workloads can afford theirs precisely because they boot no
    /// virtual machine: 440 engines that never create a container still cost
    /// less than 32 that do.
    fn iterations(self) -> usize {
        match self {
            Self::Untouched => 440,
            Self::OpenChannel => 96,
            Self::RemovedContainer => 32,
        }
    }

    /// Whether this workload needs an image loaded into the engine's store.
    ///
    /// Here rather than at the two places that care, because they are two
    /// copies of one rule otherwise: `rate` decides whether to build a layout
    /// and `one_shutdown` assumes the same set when it unwraps one. A workload
    /// added to the second and not the first would panic inside the live tier,
    /// which is the most expensive place in this repository to discover
    /// anything.
    fn needs_image(self) -> bool {
        matches!(self, Self::RemovedContainer)
    }
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
async fn rate(workload: Workload) -> Report {
    let iterations = workload.iterations();

    // **Only the container workload needs an image, and the other two used to
    // build one anyway.** That made the control -- whose whole job is to go red
    // only when the shutdown path itself is broken -- able to fail because
    // `GASCAN_ARCA_BASE_OCI_LAYOUT` was unset or a blob would not copy. A
    // control that can fail for a fixture reason is a weaker control.
    //
    // The `TempDir` is bound alongside the path because it owns the directory
    // the path names, and dropping it would delete the layout mid-run.
    let images = workload.needs_image().then(|| {
        let directory = tempfile::tempdir().expect("a temporary layout root");
        let layout = staying_up(Utf8Path::from_path(directory.path()).expect("a utf-8 path"));
        (directory, layout)
    });
    let layout = images.as_ref().map(|(_directory, layout)| layout.as_path());

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut unclean = 0;
    for _ in 0..iterations {
        let status = one_shutdown(workload, layout).await;
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
async fn one_shutdown(workload: Workload, layout: Option<&Utf8Path>) -> ExitStatus {
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
            let layout = layout.expect("the container workload builds a layout");
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
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel and a \
            vminit layout; no image, and so no base OCI layout; stops 440 engines"]
async fn the_engine_exits_cleanly_with_nothing_holding_a_connection() {
    let report = rate(Workload::Untouched).await;
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
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel and a \
            vminit layout; no image, and so no base OCI layout; stops 96 engines"]
async fn the_engine_exits_cleanly_with_a_client_channel_still_open() {
    let report = rate(Workload::OpenChannel).await;
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
            layout and a base OCI layout; boots 32 virtual machines"]
async fn the_engine_exits_cleanly_after_a_container_has_been_created() {
    let report = rate(Workload::RemovedContainer).await;
    println!("{report}");
    assert_eq!(report.unclean, 0, "{report}");
}
