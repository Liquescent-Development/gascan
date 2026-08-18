//! The engine is dialed first and spawned only on a miss.
//!
//! Every case here drives `ensure_engine` against a real Unix socket and a
//! counting spawner. The socket is real because the decision under test is
//! precisely "is anything listening", and a fake that answered that question
//! would be testing itself.

use gascand::{
    EngineError, EngineLaunch, EngineReadiness, EngineSpawner, SpawnedEngine, ensure_engine,
};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Counts spawns and never starts anything.
///
/// It deliberately does NOT create the socket. A spawner that made its own
/// socket appear would let the "spawns exactly one" case pass while the
/// readiness wait was broken, and would make the dead-engine case unreachable.
#[derive(Clone, Debug, Default)]
struct CountingSpawner {
    spawns: Arc<AtomicUsize>,
}

impl CountingSpawner {
    fn count(&self) -> usize {
        self.spawns.load(Ordering::Acquire)
    }
}

impl EngineSpawner for CountingSpawner {
    fn spawn(&self, _launch: &EngineLaunch) -> io::Result<SpawnedEngine> {
        self.spawns.fetch_add(1, Ordering::Release);
        // No child handle: nothing to wait on, so the readiness loop runs to
        // its timeout rather than short-circuiting on an exit status. That is
        // what makes `spawns_exactly_one_engine_when_none_is_listening` observe
        // the spawn without needing an engine to exist.
        Ok(SpawnedEngine::default())
    }
}

/// A spawner that starts a process which exits immediately.
struct DeadSpawner;

impl EngineSpawner for DeadSpawner {
    fn spawn(&self, _launch: &EngineLaunch) -> io::Result<SpawnedEngine> {
        let child = tokio::process::Command::new("/usr/bin/false").spawn()?;
        Ok(SpawnedEngine::watching(child))
    }
}

/// A readiness bound short enough that reaching it costs the suite nothing.
///
/// The cases below have to REACH the timeout to observe the miss arm, so the
/// product's twenty seconds would be twenty seconds of test time per case. The
/// poll stays well under it so the loop still runs several times.
fn brisk() -> EngineReadiness {
    EngineReadiness {
        timeout: std::time::Duration::from_millis(150),
        poll: std::time::Duration::from_millis(5),
    }
}

fn launch(socket: PathBuf) -> EngineLaunch {
    EngineLaunch {
        executable: PathBuf::from("/nonexistent/arca-engine"),
        socket,
        state_root: PathBuf::from("/nonexistent/state"),
        kernel: PathBuf::from("/nonexistent/vmlinux"),
        vminit: PathBuf::from("/nonexistent/vminit"),
    }
}

/// **The four options the engine requires, in the argv this daemon builds.**
///
/// Read this test for what it is: it compares one hand-written list against
/// another, so it CANNOT discover that the engine wants a fifth option or a
/// different spelling. What it does is hold the pairing of flag to field still.
/// MEASURED against the pinned engine, `--socket-path` alone exits **64** on
/// `Missing expected argument '--state-root <state-root>'`, and no test in this
/// file could see that, because every spawner here is a fixture that never runs
/// an engine. **The instrument for the argv itself is the daemon-on-engine pass
/// in `gascan-e2e`**, which spawns the real binary and then uses it.
#[test]
fn the_serve_arguments_name_every_path_the_engine_requires() {
    let launch = launch(PathBuf::from("/nonexistent/engine.sock"));
    let arguments: Vec<&str> = launch
        .serve_arguments()
        .iter()
        .map(|argument| argument.to_str().unwrap_or("<non-utf8>"))
        .collect();
    assert_eq!(
        arguments,
        [
            "--socket-path",
            "/nonexistent/engine.sock",
            "--state-root",
            "/nonexistent/state",
            "--kernel-path",
            "/nonexistent/vmlinux",
            "--vminit-layout",
            "/nonexistent/vminit",
        ]
    );
}

/// **A live engine is adopted, not duplicated.**
///
/// This is the property the whole dial-first shape exists for. The parent
/// design's own argument for a launchd job was that a `SIGKILL`ed daemon leaves
/// its engine holding the socket -- so if this case ever spawned, the second
/// engine could not bind, and a restarted daemon would fail to reach sandboxes
/// that are still running.
#[tokio::test]
async fn adopts_a_listening_engine_without_spawning_a_second() -> TestResult {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("engine.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let spawner = CountingSpawner::default();

    ensure_engine(&launch(socket.clone()), &spawner, brisk()).await?;

    assert_eq!(
        spawner.count(),
        0,
        "an engine that is already listening must not be duplicated"
    );
    drop(listener);
    Ok(())
}

/// **With nothing listening, exactly one engine is started.**
///
/// The readiness wait then times out, because `CountingSpawner` starts nothing.
/// That is the assertion's point: the spawn is observed on the way to a failure
/// that names the socket, so a supervisor that spawned twice -- once to probe
/// and once to serve, say -- would be caught by the count and not excused by
/// the eventual error.
#[tokio::test]
async fn spawns_exactly_one_engine_when_none_is_listening() -> TestResult {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("engine.sock");
    let spawner = CountingSpawner::default();

    let error = ensure_engine(&launch(socket.clone()), &spawner, brisk())
        .await
        .expect_err("nothing is listening and nothing was really started");

    assert_eq!(spawner.count(), 1, "exactly one engine must be started");
    assert!(
        matches!(error, EngineError::NotListening { .. }),
        "expected a readiness failure naming the socket, got {error}"
    );
    assert!(
        error.to_string().contains("engine.sock"),
        "the failure must name the socket: {error}"
    );
    Ok(())
}

/// **A stale socket file is a miss, not a connection.**
///
/// An ordinary file where the socket should be, or a socket whose engine died
/// without unlinking, both answer `ECONNREFUSED` rather than `ENOENT`. A
/// supervisor that treated only `ENOENT` as "no engine" would decide an engine
/// was present, hand the caller a dial that fails, and never start anything.
#[tokio::test]
async fn a_socket_with_nothing_behind_it_is_treated_as_no_engine() -> TestResult {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("engine.sock");
    // **Closing a listening AF_UNIX socket does not synchronously stop connects
    // from succeeding on macOS.** MEASURED while writing this: with the listener
    // dropped on the line above, an immediate `connect` returned Ok on two runs
    // out of three, and `ConnectionRefused` on the third -- so a fixture that
    // simply bound, dropped and proceeded made this case fail about as often as
    // it passed. It is the kernel and not tokio: `std`'s drop is a plain
    // `close(2)` and behaves the same way.
    //
    // So the precondition is WAITED FOR rather than assumed. The test's subject
    // is what `ensure_engine` does about a stale socket; establishing that there
    // IS one is the fixture's job, and asserting it is what keeps a fixture that
    // silently stopped producing one from being read as a pass.
    let listener = std::os::unix::net::UnixListener::bind(&socket)?;
    drop(listener);
    assert!(socket.exists(), "the stale socket file must remain");
    let settled = std::time::Instant::now();
    loop {
        match std::os::unix::net::UnixStream::connect(&socket) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => break,
            _ if settled.elapsed() > std::time::Duration::from_secs(5) => {
                return Err("the closed socket never began refusing connections".into());
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
        }
    }
    let spawner = CountingSpawner::default();

    let error = ensure_engine(&launch(socket.clone()), &spawner, brisk())
        .await
        .expect_err("nothing was really started");

    assert_eq!(
        spawner.count(),
        1,
        "a socket with no listener must be treated as no engine"
    );
    assert!(matches!(error, EngineError::NotListening { .. }));
    Ok(())
}

/// **An engine that dies is reported as having died, not as having been slow.**
///
/// The two are a long way apart for whoever reads the message: a timeout sends
/// them looking at the socket path, an exit status sends them to the engine's
/// own output. The check runs before the timeout for exactly that reason.
#[tokio::test]
async fn an_engine_that_exits_is_reported_as_exited_rather_than_slow() -> TestResult {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("engine.sock");

    let error = ensure_engine(&launch(socket), &DeadSpawner, brisk())
        .await
        .expect_err("the engine exits immediately");

    assert!(
        matches!(error, EngineError::Exited { .. }),
        "expected an exit status, got {error}"
    );
    Ok(())
}

/// **A socket owned by another user is refused before it is dialed.**
///
/// Reaching an engine someone else owns is not a degraded connection, it is the
/// wrong engine: every answer after the dial would be trusted output from a
/// process this user does not control. Nothing in this repository validated
/// this direction before -- `validate_peer_uid` was used only for the daemon
/// checking clients of its own socket.
///
/// Driven through the readable half of the pair, `/var/run/`-style system paths
/// being unavailable to a test: this asserts against a path owned by root that
/// certainly exists on macOS.
#[tokio::test]
async fn a_socket_owned_by_another_user_is_refused_before_dialing() -> TestResult {
    let root_owned = PathBuf::from("/usr/bin/false");
    let metadata = std::fs::metadata(&root_owned)?;
    {
        use std::os::unix::fs::MetadataExt as _;
        assert_ne!(
            metadata.uid(),
            rustix::process::geteuid().as_raw(),
            "this test needs a path this user does not own"
        );
    }
    let spawner = CountingSpawner::default();

    let error = ensure_engine(&launch(root_owned), &spawner, brisk())
        .await
        .expect_err("a foreign socket must be refused");

    assert!(
        matches!(error, EngineError::ForeignSocket { .. }),
        "expected a foreign-socket refusal, got {error}"
    );
    assert_eq!(
        spawner.count(),
        0,
        "a foreign socket must not cause a spawn either"
    );
    Ok(())
}
