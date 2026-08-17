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
    EngineExit, LiveEngine, await_state, base_oci_layout, layout_running,
    policy_request_from_manifest,
};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, RemoveRequest, RuntimeBackend};
use std::collections::BTreeMap;
use std::fmt;
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
    /// | workload | rate it must detect | n | false green |
    /// |---|---|---|---|
    /// | `Untouched` | 1/96 = 0.0104 | 440 | 0.9978% |
    /// | `OpenChannel` | 1/288 = 0.00347 | 1324 | 0.9999% |
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
    /// **`OpenChannel` was 96, and 96 was sized against a rate that is no longer
    /// the one it has to detect.** 96 came from this workload's own pre-fix rate
    /// of 5/96, which milestone 3 closed; what remained in it was rarer -- an
    /// `exit status: 1`, seen once in 288 shutdowns -- and against 0.00347 a
    /// clean sweep of 96 comes up all clean **71.6116% of the time**. That is
    /// not a weak guard, it is a guard that passes against a broken engine four
    /// times in five, which is the thing this module's docstring opens by
    /// rejecting. **1324 is the smallest n that clears 1%**: 1323 gives 1.0034%
    /// and 1324 gives 0.9999%. It is `ln(0.01)/ln(287/288) = 1323.985` and NOT
    /// the first-order `-ln(0.01)/(1/288) = 1326.289`, which overshoots by two
    /// and is what an earlier reckoning of this number used.
    ///
    /// **The resizing is what found the defect, and 96 would not have.**
    /// MEASURED at 1324 against Arca `218343b`: `1323 x exit status: 0, 1 x exit
    /// status: 1`, slowest shutdown **10.01s** against the ten-second grace, and
    /// the unclean engine logged `connections did not drain within the grace
    /// period`. Against the fix (`SilentConnectionQuiescer`): **0 of 1324**,
    /// slowest shutdown **0.11s**.
    ///
    /// The two cheap workloads can afford theirs precisely because they boot no
    /// virtual machine: 1324 engines that never create a container cost 383.34s,
    /// against 210.67s for the 32 that do.
    fn iterations(self) -> usize {
        match self {
            Self::Untouched => 440,
            Self::OpenChannel => 1324,
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
    /// The longest any one engine took from its pipe closing to being reaped.
    ///
    /// **The status byte says whether the ten-second grace was missed; this says
    /// by how much it was not.** Arca's `shutdownGrace` is a policy nothing had
    /// ever measured against a real client -- its own docstring says so -- and
    /// the whole distribution being milliseconds is a different fact from all of
    /// it being clean. A run whose slowest shutdown crept toward ten seconds is
    /// a green about to turn red, and `ExitStatus::success` cannot tell the two
    /// apart.
    slowest: Duration,
    /// What the engine said, for each shutdown that was not clean.
    ///
    /// **`exit status: 1` names no cause, and the engine has two that produce
    /// it.** `EXIT_FAILURE` is the drain running out of its grace and it is also
    /// the listening socket closing with nothing having asked for a shutdown
    /// (Arca `ArcaEngineCommand`, the two `releaseAndExit(status: EXIT_FAILURE)`
    /// callers). Each logs a line of its own first, so the log distinguishes
    /// what the byte cannot -- and a failure that arrives without either line is
    /// a third cause, which is worth knowing immediately rather than after
    /// another 1324 engines.
    spoken: Vec<String>,
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
            "{} of {} shutdowns of {} were not clean ({percentage:.0}%): {}; \
             slowest shutdown {:.2}s against a {:.0}s grace",
            self.unclean,
            self.iterations,
            self.workload,
            breakdown.join(", "),
            self.slowest.as_secs_f64(),
            GRACE.as_secs_f64(),
        )?;
        for said in &self.spoken {
            write!(formatter, "\n--- an unclean engine said ---\n{said}")?;
        }
        Ok(())
    }
}

/// Arca's `ArcaEngineCommand.shutdownGrace`, restated here so a slowest
/// shutdown can be read against the deadline it is approaching.
///
/// A copy of a constant that lives on the other side of a process boundary, and
/// there is no way for this side to read the real one: it is a `private static
/// let` in a Swift executable, and the engine exposes no RPC that reports it.
/// Nothing here depends on the two agreeing -- this number is printed, never
/// compared -- so a drift makes a report's context stale rather than a test
/// wrong.
const GRACE: Duration = Duration::from_secs(10);

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
    let mut slowest = Duration::ZERO;
    let mut spoken = Vec::new();
    for _ in 0..iterations {
        let exit = one_shutdown(workload, layout).await;
        if !exit.status.success() {
            unclean += 1;
            spoken.push(exit.diagnostics);
        }
        slowest = slowest.max(exit.took);
        *counts.entry(format!("{}", exit.status)).or_default() += 1;
    }
    Report {
        workload,
        iterations,
        unclean,
        counts,
        slowest,
        spoken,
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

/// Drives one engine through `workload` and returns how it ended.
async fn one_shutdown(workload: Workload, layout: Option<&Utf8Path>) -> EngineExit {
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

/// A connection that has been accepted and has said nothing must not cost the
/// engine its grace period.
///
/// **This is the deterministic form of the defect the rate test measures, and it
/// is why that rate test can be trusted to have found a cause rather than a
/// coincidence.** The three workloads above meet the defect once in hundreds of
/// engines because they have to lose a race to reach the state; this one enters
/// that state on purpose, every run.
///
/// **MEASURED against Arca `218343b`, and it was the first time the ten-second
/// grace had been seen to fire against anything:** the engine exited `exit
/// status: 1` after **10.01s**, logging `connections did not drain within the
/// grace period; closing anyway`. Against the fix (`SilentConnectionQuiescer`),
/// the same test exits `0` after **0.01s**. The control is the same test without
/// the pause below: it exited `0` after 0.01s even against the broken engine,
/// because an unaccepted connection holds nothing -- so the pause is
/// load-bearing and what it guards against is a false green.
///
/// The mechanism is read out of the vendored source rather than inferred.
/// `ServerQuiescingHelper` counts an accepted channel from the moment it is
/// accepted, and `GRPCServerPipelineConfigurator` -- the only handler in that
/// channel's pipeline before its first byte arrives (grpc-swift 1.23's
/// `Server.configureAcceptedChannel`) -- handles only `TLSUserEvent` in its
/// `userInboundEventTriggered` and forwards everything else untouched. Nothing
/// acted on `ChannelShouldQuiesceEvent` and there was nothing downstream yet to
/// act, so the drain could not complete and only the grace could end it. Arca's
/// `SilentConnectionQuiescer` is the handler that now does.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel and a \
            vminit layout; against a broken engine it spends the whole ten-second grace"]
async fn a_silent_peer_does_not_hold_the_drain() {
    let engine = LiveEngine::start().await;
    // Connected and never written to, so the server has accepted a channel it
    // will never finish configuring. Held across the stop for the reason
    // `OpenChannel` holds its transport: dropping it would close the connection
    // and let the drain finish.
    let _silent = tokio::net::UnixStream::connect(engine.socket().as_std_path())
        .await
        .expect("connecting a raw socket to a started engine must succeed");

    // **`connect` returning means the connection is in the listen backlog, NOT
    // that the server has accepted it, and nothing on this side can observe the
    // difference.** That is the setup race Arca's own `EngineServer` comment
    // names, and it is why this pause is here rather than being tidied away.
    // MEASURED without it: the engine exited `exit status: 0` after **0.01s**,
    // so the drain completed and the peer held nothing.
    //
    // The race fails safe in the same direction Arca records: an unaccepted
    // connection lets the drain finish at once, so the assertions below go red
    // rather than falsely green. A pause cannot make this test lie; it can only
    // make it stop measuring nothing.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let exit = engine.stop().await;
    println!(
        "a silent peer: engine exited {} after {:.2}s against a {:.0}s grace\n{}",
        exit.status,
        exit.took.as_secs_f64(),
        GRACE.as_secs_f64(),
        exit.diagnostics,
    );

    // Both halves, because either alone can pass against a broken engine. The
    // status alone goes green if some future change makes the grace exit 0 --
    // which would be the failure silenced rather than fixed -- and the duration
    // alone goes green on the unaccepted-connection race the pause above exists
    // to close.
    assert!(
        exit.status.success(),
        "the engine exited {} after {:.2}s. If it also logged the grace period, a \
         silent peer is holding the drain again and `SilentConnectionQuiescer` is \
         not closing it: {}",
        exit.status,
        exit.took.as_secs_f64(),
        exit.diagnostics,
    );
    assert!(
        exit.took < GRACE,
        "the engine exited cleanly but took {:.2}s, which is its whole {:.0}s grace. \
         A clean status reached by waiting out the deadline is the defect surviving \
         behind a passing byte: {}",
        exit.took.as_secs_f64(),
        GRACE.as_secs_f64(),
        exit.diagnostics,
    );
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
