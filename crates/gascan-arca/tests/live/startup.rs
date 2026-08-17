//! Whether a `SIGTERM` that lands during startup can kill the engine.
//!
//! **This is a FORCED instrument, and that is the whole reason it exists
//! separately from `shutdown.rs`.** The defect it measures reached Gas Can's
//! live tier as 2 engines in 440 exiting 143 inside
//! `shutdown::the_engine_exits_cleanly_with_nothing_holding_a_connection`, and a
//! rate that low cannot distinguish a fix from luck: at 2/440 a clean sweep of
//! 440 comes up 13% of the time against a broken engine, and a sweep of 12 comes
//! up clean 95% of the time. Waiting for the race to fire is not a measurement.
//!
//! So no arm below waits. Each signals at a chosen instant that is inside or
//! outside the window by construction, and the three are interleaved round by
//! round in one process against one binary, so a machine that drifts mid-run
//! drifts under all of them.
//!
//! **143 is always the kernel and never the engine.** `arca-engine`'s only
//! deliberate exits are `Foundation.exit` with `EXIT_SUCCESS` or `EXIT_FAILURE`,
//! so a child reported as terminated by signal 15 was terminated by the default
//! disposition -- the engine never ran a line of its own shutdown. That is why
//! [`Outcome`] records "killed by a signal" as its own count rather than folding
//! it into "did not exit 0": a fix that turned 143 into a non-zero exit would
//! satisfy the second and not the first, and would not be a fix.
//!
//! ## What the arms measure, and what they measured
//!
//! Against Arca `db11cc0` (pre-fix) and the same tree with the fix, 12 engines
//! per arm:
//!
//! | arm | pre-fix killed | post-fix killed |
//! |---|---|---|
//! | [`When::AtTheSpawnInstant`] | 12/12 | 12/12 |
//! | [`When::InsideTheEngineStartup`] | 12/12 | 0/12 |
//! | [`When::OnceServing`] | 0/12 | 0/12 |
//!
//! **The first arm's post-fix 12/12 is a real result and not a failure to fix
//! anything.** MEASURED by timestamping the engine's own `dyld` constructor --
//! the earliest instant any of its code runs -- against the parent's clock:
//! **10-13ms elapse between the spawn returning and the engine's first
//! instruction**, and every one of them is `dyld` mapping and binding 40
//! libraries. No code inside a process can shorten that, and no disposition it
//! sets can apply before it. An arm that signals at the spawn instant is
//! therefore measuring `dyld`, not the engine.
//!
//! The only thing that closes that residue is the launcher blocking `SIGTERM`
//! between `fork` and `exec`, which makes anything arriving during the load
//! pending rather than fatal; the engine unblocks it once its handler is
//! installed (`ArcaSignalCapture`), and Arca's
//! `ShutdownSignalsTests.testCaptureUnblocksASignalTheProcessInherited`
//! (`Tests/ArcaEngineTests/ShutdownSignalsTests.swift`) drives that half
//! in-process. **There is deliberately no arm for it here.**
//! It needs `Command::pre_exec`, and this workspace sets `unsafe_code =
//! "forbid"`; relaxing a workspace-wide safety lint to add one arm to one
//! `#[ignore]`d test is a worse trade than proving the same property where the
//! mechanism lives. Gas Can does not spawn engines that way today in any case --
//! `common::SUPERVISOR` is a `/bin/sh` wrapper, and a shell cannot set a signal
//! mask.
//!
//! The second arm is the defect's own window, and the one the acceptance rests
//! on: the engine's own code is demonstrably running (it has created its state
//! database) and it has not yet bound a socket, which is where the vminit load
//! and all three `initialize()` calls live.

use crate::common::{EngineInputs, SocketRoot};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ChannelTransport;
use std::collections::BTreeMap;
use std::fmt;
use std::process::ExitStatus;
use std::time::Duration;

/// When the signal is sent, relative to the engine's own startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    /// The instant `spawn` returns, before anything is waited for.
    ///
    /// `posix_spawn` returns once the child exists, so the signal is sent while
    /// the child is still in `dyld`. Nothing about this arm is probabilistic:
    /// every iteration signals before the engine's first instruction. See the
    /// module note -- this arm measures the loader, not the engine.
    AtTheSpawnInstant,
    /// Once the engine has created its state database, and before it binds.
    ///
    /// **The defect's own window.** The database is the first thing the engine
    /// puts on disk after validating its inputs, so its existence proves the
    /// engine's code is running; the socket does not exist for roughly a second
    /// afterwards, and the vminit load and all three `initialize()` calls happen
    /// in between. A signal here is inside `ServeCommand.run()` every time.
    InsideTheEngineStartup,
    /// Once the socket accepts a connection, plus a settling delay.
    ///
    /// **The control.** By this point the engine has bound and whatever it does
    /// about signals it has finished setting up. A red control says the ordinary
    /// shutdown path is broken in a way that has nothing to do with startup, and
    /// invalidates the other arms rather than adding a finding of its own.
    OnceServing,
}

/// Every arm, in the order one round runs them.
const ARMS: [When; 3] = [
    When::AtTheSpawnInstant,
    When::InsideTheEngineStartup,
    When::OnceServing,
];

/// How long the serving arm waits after the socket answers before signalling.
///
/// 300ms, and it is a settling delay rather than a measured requirement: the
/// socket answering means `EngineServer.start` returned, and the engine hands
/// its shutdown routing over immediately afterwards. The delay makes the control
/// unambiguous, so that a red control is a statement about shutdown and not
/// about having signalled a few microseconds too early.
const SETTLING: Duration = Duration::from_millis(300);

/// How many engines each arm stops.
///
/// Twelve, which is what the hand-run comparison that found this used. The arms
/// do not straddle a rate -- each is inside or outside its window on every
/// iteration -- so the count is bounded by patience rather than by the
/// probability of catching something.
const ROUNDS: usize = 12;

/// How long an engine is given to exit after its signal.
///
/// 120s for the reason `LiveEngine::await_socket` uses that bound: a freshly
/// built binary's first execution is far slower than its later ones -- MEASURED
/// here as 732ms to reach the engine's constructor on a cold inode against
/// 10-13ms warm. Exceeding it is recorded as its own outcome rather than as a
/// panic, so one stuck engine reports as one stuck engine instead of losing the
/// other eleven rounds.
const EXIT_BOUND: Duration = Duration::from_secs(120);

impl fmt::Display for When {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AtTheSpawnInstant => "signalled the instant it was spawned",
            Self::InsideTheEngineStartup => "signalled inside its own startup",
            Self::OnceServing => "signalled once it was serving",
        })
    }
}

/// How one engine ended.
///
/// Three cases and not two, because "killed by the signal" is the defect and
/// "exited non-zero" is not the same thing. See the module note on 143.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    /// The kernel's default disposition ran. The engine's own code did not.
    Killed(i32),
    /// The engine chose its own status.
    Exited(i32),
    /// Still running when [`EXIT_BOUND`] ran out, and then `SIGKILL`ed.
    ///
    /// The shape a silent no-op takes: an engine that ignores the signal and
    /// serves on is not killed and never exits, which is why the assertions
    /// below cannot be satisfied by "not killed" alone.
    NeverExited,
}

impl Outcome {
    fn of(status: ExitStatus) -> Self {
        use std::os::unix::process::ExitStatusExt as _;
        match (status.code(), status.signal()) {
            (Some(code), _) => Self::Exited(code),
            (None, Some(signal)) => Self::Killed(signal),
            (None, None) => unreachable!("a reaped child exited or was signalled: {status}"),
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Killed(signal) => write!(
                formatter,
                "killed by signal {signal} (a shell would call it {})",
                128 + signal
            ),
            Self::Exited(code) => write!(formatter, "exited {code}"),
            Self::NeverExited => formatter.write_str("never exited"),
        }
    }
}

/// How one arm came out, by outcome.
///
/// The whole distribution rather than a pass/fail, so that a before and an after
/// are comparable and so that a green says which outcomes it is made of.
struct Report {
    when: When,
    killed: usize,
    unclean: usize,
    counts: BTreeMap<Outcome, usize>,
}

impl Report {
    fn of(when: When, outcomes: &[Outcome]) -> Self {
        let mut counts: BTreeMap<Outcome, usize> = BTreeMap::new();
        let mut killed = 0;
        let mut unclean = 0;
        for outcome in outcomes {
            if matches!(outcome, Outcome::Killed(_)) {
                killed += 1;
            }
            if !matches!(outcome, Outcome::Exited(0)) {
                unclean += 1;
            }
            *counts.entry(outcome.clone()).or_default() += 1;
        }
        Self {
            when,
            killed,
            unclean,
            counts,
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let breakdown: Vec<String> = self
            .counts
            .iter()
            .map(|(outcome, count)| format!("{count} x {outcome}"))
            .collect();
        let total: usize = self.counts.values().sum();
        write!(
            formatter,
            "of {total} engines {}: {} killed by the signal, {} did not exit 0; {}",
            self.when,
            self.killed,
            self.unclean,
            breakdown.join(", "),
        )
    }
}

/// Spawns one engine, signals it at `when`, and reports how it ended.
///
/// **Spawned directly rather than under `common::SUPERVISOR`.** The wrapper is a
/// `/bin/sh` whose whole purpose is to react to a closed pipe, and both of the
/// things this instrument needs are things the wrapper takes away: the engine's
/// own pid, so the signal can be sent at a chosen instant, and the absence of a
/// shell's own startup between the spawn and that instant. The engine is instead
/// reaped here on every path, and `kill_on_drop` is the belt for a panic.
///
/// Its output goes to `/dev/null`. Twenty-four engines' worth of `info` logging
/// is not diagnosis, it is a haystack; what this instrument reads is the exit
/// status, and the arm that could fail for a fixture reason
/// ([`When::InsideTheEngineStartup`] and [`When::OnceServing`], which wait for
/// something the engine must produce) panics naming what it was waiting for.
async fn one_engine(inputs: &EngineInputs, when: When) -> Outcome {
    let root = tempfile::tempdir().expect("a temporary state root");
    let state = Utf8Path::from_path(root.path())
        .expect("a utf-8 temporary path")
        .join("state");
    std::fs::create_dir_all(&state).expect("a state root");

    let socket_root = SocketRoot::fresh();
    let socket = socket_root.socket();

    let mut child = tokio::process::Command::new(&inputs.binary)
        .args(inputs.serve_arguments(&socket, &state))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|error| panic!("could not spawn {}: {error}", inputs.binary));

    let pid = rustix::process::Pid::from_raw(
        child
            .id()
            .expect("a just-spawned child has a pid")
            .try_into()
            .expect("a pid fits in an i32"),
    )
    .expect("a spawned child's pid is not zero");

    match when {
        When::AtTheSpawnInstant => {}
        When::InsideTheEngineStartup => {
            await_engine(
                &mut child,
                Milestone::StateDatabase(&state.join("state.db")),
            )
            .await;
        }
        When::OnceServing => {
            await_engine(&mut child, Milestone::Serving(&socket)).await;
            tokio::time::sleep(SETTLING).await;
        }
    }

    rustix::process::kill_process(pid, rustix::process::Signal::TERM)
        .unwrap_or_else(|error| panic!("could not signal the engine on {socket}: {error}"));

    match tokio::time::timeout(EXIT_BOUND, child.wait()).await {
        Ok(Ok(status)) => Outcome::of(status),
        Ok(Err(error)) => panic!("could not reap the engine on {socket}: {error}"),
        Err(_elapsed) => {
            // Dropping the child `SIGKILL`s it, which is what keeps a stuck
            // engine from outliving the run.
            drop(child);
            Outcome::NeverExited
        }
    }
}

/// A point in the engine's startup that an arm waits for.
///
/// An enum rather than a closure so that one waiter serves both: the two differ
/// in what "there yet" means and in nothing else, and two copies of the
/// still-alive check and the bound is how one of them drifts.
enum Milestone<'a> {
    /// The state database file exists.
    ///
    /// The first thing `ServeCommand.run()` puts on disk after validating its
    /// inputs, and roughly a second before it binds.
    StateDatabase(&'a Utf8PathBuf),
    /// The socket exists and accepts a connection.
    ///
    /// Both halves for the reason `LiveEngine::await_socket` gives: the socket
    /// file appears before the listener accepts, so waiting only for the file
    /// races the bind.
    Serving(&'a Utf8Path),
}

impl Milestone<'_> {
    async fn reached(&self) -> bool {
        match self {
            Self::StateDatabase(database) => database.exists(),
            Self::Serving(socket) => {
                socket.exists()
                    && ChannelTransport::connect(socket.as_std_path().to_owned())
                        .await
                        .is_ok()
            }
        }
    }

    fn description(&self) -> String {
        match self {
            Self::StateDatabase(database) => format!("create its state database at {database}"),
            Self::Serving(socket) => format!("accept a connection on {socket}"),
        }
    }
}

/// Waits for `milestone`, or says why it will never arrive.
///
/// An engine that died at startup is a different fact from a slow one, and on
/// these arms a death before the signal is a fixture failure rather than a
/// finding -- so it panics rather than being counted. Saying which happened
/// beats letting a dead engine spend the whole bound telling the slow story.
async fn await_engine(child: &mut tokio::process::Child, milestone: Milestone<'_>) {
    let started = std::time::Instant::now();
    loop {
        if milestone.reached().await {
            return;
        }
        match child.try_wait() {
            Ok(Some(status)) => panic!(
                "engine exited with {status} before it could {}",
                milestone.description()
            ),
            Ok(None) => {}
            Err(error) => panic!(
                "could not check on the engine waiting for it to {}: {error}",
                milestone.description()
            ),
        }
        assert!(
            started.elapsed() < EXIT_BOUND,
            "engine did not {} within {:.1}s",
            milestone.description(),
            started.elapsed().as_secs_f64()
        );
        // A millisecond, not the 50 `LiveEngine::await_socket` uses. The whole
        // point of the startup arm is to signal INSIDE a window, and a 50ms
        // poll would spend a measurable part of it asleep.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// Runs every arm round by round and reports each.
///
/// **Interleaved, and one process against one binary.** Running one arm's twelve
/// and then the next's would put each on a different part of the machine's day;
/// the separation this instrument claims would then be indistinguishable from
/// the machine having got busier. Alternating means any drift lands on all of
/// them.
async fn interleaved() -> Vec<Report> {
    let inputs = EngineInputs::from_environment();
    let mut outcomes: Vec<Vec<Outcome>> = ARMS.iter().map(|_| Vec::with_capacity(ROUNDS)).collect();
    for _ in 0..ROUNDS {
        for (index, arm) in ARMS.iter().enumerate() {
            outcomes[index].push(one_engine(&inputs, *arm).await);
        }
    }
    ARMS.iter()
        .zip(outcomes.iter())
        .map(|(arm, outcomes)| Report::of(*arm, outcomes))
        .collect()
}

/// Finds the one arm's report, or says the arm was dropped rather than reading
/// a stale one.
fn report_for(reports: &[Report], when: When) -> &Report {
    reports
        .iter()
        .find(|report| report.when == when)
        .unwrap_or_else(|| panic!("no arm reported for {when}"))
}

/// A `SIGTERM` delivered during the engine's own startup must end it on the
/// engine's own terms.
///
/// The control is asserted FIRST for the reason `shutdown.rs` gives its own: a
/// red control means the shutdown path is broken in a way that has nothing to do
/// with startup, and reading the startup arms under those conditions would be
/// reading noise.
///
/// Each startup arm is then asserted twice. `killed == 0` is the defect itself --
/// the kernel's default disposition running instead of the engine's code -- and
/// `unclean == 0` is the requirement that closing the window did not merely
/// trade 143 for some other non-zero status, or for a silent no-op that leaves
/// the engine running until the bound expires.
///
/// **[`When::AtTheSpawnInstant`] is reported and NOT asserted, and that is a
/// finding rather than an omission.** It signals into `dyld`, 10-13ms of which
/// elapse before the engine's first instruction; no engine change can affect it,
/// and an assertion there would be an assertion about the loader. It stays in
/// the run because the number is the evidence for that claim: an arm that
/// nobody prints is an arm nobody can check.
#[tokio::test]
#[ignore = "requires a built, signed arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel and \
            a vminit layout; no image, and so no base OCI layout; starts and stops 36 engines"]
async fn a_sigterm_during_startup_does_not_kill_the_engine() {
    let reports = interleaved().await;
    for report in &reports {
        println!("{report}");
    }

    let serving = report_for(&reports, When::OnceServing);
    assert_eq!(
        serving.unclean, 0,
        "the control must be clean before the startup arms mean anything; {serving}"
    );

    let inside = report_for(&reports, When::InsideTheEngineStartup);
    assert_eq!(
        inside.killed, 0,
        "a signal during startup must never reach the default disposition; {inside}"
    );
    assert_eq!(
        inside.unclean, 0,
        "an engine asked to stop during startup must stop on its own terms; {inside}"
    );
}
