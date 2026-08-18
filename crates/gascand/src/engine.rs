//! Dial the engine, and spawn one only when nothing answers.
//!
//! **This is the shape `gascand` itself is already supervised with**, and it is
//! deliberately not a launchd job. The parent design ruled the engine a launchd
//! job on a structural argument: after `gascand` is `SIGKILL`ed its engine child
//! survives holding the socket, so a supervisor would need "a second case plus a
//! fallback branch to choose between them". That reasoning is right and its
//! conclusion is the opposite of what it drew -- recovery REQUIRES dialing an
//! existing engine, so dialing is the primary path and spawning is its miss arm.
//! There is no second case and no branch to choose between.
//!
//! **A surviving engine is a feature, not a leak.** Nothing here kills an engine
//! it did not start. A restarted daemon adopts running sandboxes by their owner
//! labels -- `run_daemon` calls `service.reconcile()` before serving, and
//! `ReconcileFinding` already distinguishes owned-but-missing from
//! unknown-but-owned -- and dial-then-spawn is exactly what makes that adoption
//! reachable.
//!
//! **Nothing here resurrects anything.** An engine that restored sandboxes
//! before its socket bound would move them through states no consumer ever
//! observes, which is the defect `applyRestartPolicies()` represents upstream.
//! This module starts a process and waits for it to listen. That is all.

use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::socket::{PeerUid, validate_peer_uid};

/// How long to wait for a spawned engine to bind, and how often to look.
///
/// A parameter and not a pair of constants, because the tests that exercise the
/// miss arm have to reach the timeout to observe it, and a twenty-second
/// constant would make the suite pay twenty seconds to learn something a
/// millisecond can teach. Timing policy belonging to the caller is also the
/// correct shape: this module decides WHETHER to spawn, not how patient the
/// product should be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineReadiness {
    pub timeout: Duration,
    pub poll: Duration,
}

impl Default for EngineReadiness {
    /// The bound is `gascan_core::backend::ENGINE_READINESS`, and it is there
    /// rather than here because the client has to know it too: its own wait
    /// must outlast this one, or this one's error -- the one that names the
    /// socket -- is produced for a client that has already given up. That is
    /// what a 20s bound here against the client's 15s did.
    ///
    /// 20s was also under a cold engine start this repository had already
    /// measured as failing at 30s. See the constant for the measurement.
    fn default() -> Self {
        Self {
            timeout: gascan_core::backend::ENGINE_READINESS,
            poll: Duration::from_millis(25),
        }
    }
}

#[derive(Debug)]
pub enum EngineError {
    /// The socket exists but belongs to another user.
    ///
    /// Refused BEFORE any connection is attempted. Reaching a socket someone
    /// else owns is not a degraded connection, it is the wrong engine, and
    /// every byte after the dial would be trusted output from a process this
    /// user does not control.
    ForeignSocket {
        path: PathBuf,
        owner: u32,
    },
    /// The engine was spawned and never bound its socket.
    NotListening {
        path: PathBuf,
        waited: Duration,
    },
    /// The engine exited while it was being waited for.
    Exited {
        status: String,
    },
    Io(io::Error),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForeignSocket { path, owner } => write!(
                formatter,
                "the engine socket {} is owned by uid {owner}, not by this user; \
                 refusing to dial it",
                path.display()
            ),
            Self::NotListening { path, waited } => write!(
                formatter,
                "the engine did not begin listening on {} within {:?}",
                path.display(),
                waited
            ),
            Self::Exited { status } => {
                write!(formatter, "the engine exited before it listened: {status}")
            }
            Self::Io(error) => write!(formatter, "engine supervision I/O error: {error}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<io::Error> for EngineError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// What a spawner needs to start an engine.
///
/// **All four paths, because the engine requires all four.** `arca-engine
/// --help` says it in its own words: "All four of those options are required
/// and none is defaulted, because a default is how a process silently ends up
/// pointed at another product's state". MEASURED against the pinned engine with
/// only `--socket-path` given -- the argv this struct used to describe -- it
/// exits **64** on `Missing expected argument '--state-root <state-root>'` and
/// binds nothing, so the spawn arm of [`ensure_engine`] could never succeed.
///
/// That defect survived Task 11's whole suite because every test there spawns
/// through a fixture: a counting spawner, a spawner that runs `/usr/bin/false`,
/// a spawner that binds the socket itself. **Not one of them runs the engine**,
/// and the argv is a contract only the engine can judge. The instrument that
/// judges it is the daemon-on-engine pass in `gascan-e2e`, which drives a real
/// `gascan up` through a daemon that spawned its own engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineLaunch {
    pub executable: PathBuf,
    pub socket: PathBuf,
    /// `--state-root`: where the engine keeps containers, images and volumes.
    pub state_root: PathBuf,
    /// `--kernel-path`: the uncompressed vmlinux guests boot.
    pub kernel: PathBuf,
    /// `--vminit-layout`: the OCI layout holding `arca-vminit:latest`.
    pub vminit: PathBuf,
}

impl EngineLaunch {
    /// The `serve` invocation, in the engine's own required order.
    ///
    /// A method and not four `.arg()` calls at the spawn site, so that the one
    /// place that knows the engine's command line is the one place a reader has
    /// to check against `arca-engine serve --help`.
    #[must_use]
    pub fn serve_arguments(&self) -> [&std::ffi::OsStr; 8] {
        [
            "--socket-path".as_ref(),
            self.socket.as_os_str(),
            "--state-root".as_ref(),
            self.state_root.as_os_str(),
            "--kernel-path".as_ref(),
            self.kernel.as_os_str(),
            "--vminit-layout".as_ref(),
            self.vminit.as_os_str(),
        ]
    }
}

/// Starting an engine process, behind a trait so the miss arm is testable.
///
/// The same shape `DaemonSpawner` has for `gascand` itself, and for the same
/// reason: the interesting properties here are about WHEN a spawn happens and
/// how many times, and a test that had to start a real engine to observe them
/// would be measuring an engine rather than this decision.
pub trait EngineSpawner: Send + Sync {
    fn spawn(&self, launch: &EngineLaunch) -> io::Result<SpawnedEngine>;
}

/// A spawned engine, retained only so a dead one can be told from a slow one.
///
/// Dropping this does NOT stop the engine. That is deliberate and is the
/// adoption property: the engine outlives the daemon that started it, and the
/// next daemon dials it rather than starting a second.
#[derive(Debug, Default)]
pub struct SpawnedEngine {
    child: Option<tokio::process::Child>,
}

impl SpawnedEngine {
    #[must_use]
    pub fn watching(child: tokio::process::Child) -> Self {
        Self { child: Some(child) }
    }

    /// The exit status if the engine has already stopped.
    fn exited(&mut self) -> io::Result<Option<String>> {
        match self.child.as_mut() {
            Some(child) => Ok(child.try_wait()?.map(|status| status.to_string())),
            None => Ok(None),
        }
    }
}

/// Is anything listening on this socket right now?
///
/// A plain `UnixStream` probe rather than the gRPC dial, because this needs the
/// `io::ErrorKind` and `tonic` renders every dial failure as the fixed string
/// `transport error` -- the io error that separates "no socket" from "socket
/// present, nobody listening" is reachable only by walking `source()`. Deciding
/// whether to start a process by string-matching a transport error would be a
/// brittle contract in the one place that must not have one.
async fn listening(path: &Path) -> io::Result<bool> {
    match tokio::net::UnixStream::connect(path).await {
        Ok(_) => Ok(true),
        // ENOENT: no socket at all. ECONNREFUSED: the file is there and nothing
        // is behind it, which is a stale socket from an engine that died without
        // unlinking. Both mean the same thing to this decision -- no engine is
        // serving -- and neither is repaired here. Unlinking a socket this
        // process does not own the lifecycle of would race a live engine that is
        // merely slow to accept.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// Refuses a socket owned by another user.
///
/// **Runs before the dial, never after.** Nothing in this repository validated
/// this direction before: `validate_peer_uid` is used at `api.rs` for the
/// opposite one, the daemon checking clients of its OWN socket. Here the daemon
/// is the client, and the thing being checked is the socket it is about to
/// trust.
///
/// A missing socket is not a failure of this check. There is no owner to
/// compare, and the caller's next step is to spawn an engine that will create
/// one owned by this user.
fn require_own_socket(path: &Path) -> Result<(), EngineError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(EngineError::Io(error)),
    };
    let owner = metadata.uid();
    validate_peer_uid(PeerUid::new(owner), PeerUid::current()).map_err(|_| {
        EngineError::ForeignSocket {
            path: path.to_owned(),
            owner,
        }
    })
}

/// Dials the engine socket, starting an engine only if nothing answers.
///
/// Returns the socket path to dial for real. The gRPC connection itself is the
/// caller's, so this module stays free of the transport and can be tested
/// without one.
pub async fn ensure_engine<S: EngineSpawner>(
    launch: &EngineLaunch,
    spawner: &S,
    readiness: EngineReadiness,
) -> Result<(), EngineError> {
    require_own_socket(&launch.socket)?;
    if listening(&launch.socket).await? {
        return Ok(());
    }
    let mut spawned = spawner.spawn(launch)?;
    wait_until_listening(&launch.socket, &mut spawned, readiness).await
}

async fn wait_until_listening(
    socket: &Path,
    spawned: &mut SpawnedEngine,
    readiness: EngineReadiness,
) -> Result<(), EngineError> {
    let started = Instant::now();
    loop {
        if listening(socket).await? {
            // The ownership check is repeated on the socket that now exists.
            // The first check ran when there was nothing to check, and a socket
            // that appeared in the meantime is one this daemon has not looked
            // at. It should be its own child's, and if it is not, something
            // else bound that path first.
            return require_own_socket(socket);
        }
        // Checked before the timeout so a dead engine is reported as having
        // died rather than as having been slow, which sends a reader to the
        // engine's own output instead of to a timeout that means nothing.
        //
        // Stated honestly: NO TEST DEFENDS THIS ORDER. Swapping these two
        // blocks leaves the whole supervisor suite green, because the fixture's
        // engine exits far inside the first tick and so is seen to have exited
        // whichever check runs first. The order only decides the message for an
        // engine that dies at about the moment the bound elapses, and pinning
        // that would mean racing the timeout on purpose. It is kept as
        // reasoning, not as a tested property.
        if let Some(status) = spawned.exited()? {
            return Err(EngineError::Exited { status });
        }
        if started.elapsed() >= readiness.timeout {
            return Err(EngineError::NotListening {
                path: socket.to_owned(),
                waited: started.elapsed(),
            });
        }
        tokio::time::sleep(readiness.poll).await;
    }
}

/// Starts the engine as a detached child.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioEngineSpawner;

impl EngineSpawner for TokioEngineSpawner {
    fn spawn(&self, launch: &EngineLaunch) -> io::Result<SpawnedEngine> {
        let child = tokio::process::Command::new(&launch.executable)
            .args(launch.serve_arguments())
            // The engine outlives this daemon deliberately, so it must not be
            // reaped when the handle is dropped. `kill_on_drop` defaults to
            // false; it is named here because turning it on would silently
            // delete the adoption property this whole module rests on.
            .kill_on_drop(false)
            .spawn()?;
        Ok(SpawnedEngine::watching(child))
    }
}
