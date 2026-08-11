use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ChannelTransport;
use std::time::Duration;

/// Distinguishes the socket roots of engines started by the same process.
static SOCKET_SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Owns the socket root directory and removes it when dropped.
///
/// The socket root is outside the `TempDir`, so nothing else removes it. This
/// exists as a guard rather than as a `Drop` on `LiveEngine` so that the
/// directory has an owner from the moment it is created: `start()` can still
/// panic on the `sun_path` assert or on a failed spawn, and those unwind
/// through this rather than leaving an orphan under `/tmp`.
struct SocketRoot(Utf8PathBuf);

impl Drop for SocketRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An engine process on a temporary socket, killed when the test ends.
///
/// The live tier drives the engine directly rather than through `gascand`.
/// It kills streams, resets mid-exec, and kills the engine under an open
/// call, and a supervisor whose job is to react to exactly those events
/// would be fighting the tests. Supervision is exercised by `gascan-e2e`.
pub struct LiveEngine {
    child: tokio::process::Child,
    socket: Utf8PathBuf,
    _socket_root: SocketRoot,
    _root: tempfile::TempDir,
}

impl LiveEngine {
    /// Starts the engine named by `GASCAN_ARCA_ENGINE_BIN`.
    ///
    /// Panics with a directive message when the variable is absent, because a
    /// live test that silently skips is a live test nobody notices has stopped
    /// running.
    pub async fn start() -> Self {
        let binary = std::env::var("GASCAN_ARCA_ENGINE_BIN").expect(
            "GASCAN_ARCA_ENGINE_BIN must name a built arca-engine; \
             run scripts/build-arca-engine.sh and use its second output line",
        );
        let root = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(root.path()).unwrap().to_owned();
        let state = path.join("state");
        std::fs::create_dir_all(&state).unwrap();

        // The socket does NOT live under the temp dir, and that is deliberate.
        // `sun_path` is capped at 103 bytes (swift-nio asserts it explicitly in
        // NIOCore/SocketAddresses.swift), and macOS temp dirs are
        // /var/folders/<...>/T/<...> -- a measured path came to 74 bytes, which
        // fits but leaves little room. Arca's own tests hit this exact wall
        // during Task 7 and had to move to /tmp. Build the socket path under a
        // short root and assert the length rather than meeting the cap as a
        // mystery bind failure.
        let socket_root = Utf8PathBuf::from(format!(
            "/tmp/gascan-arca-live-{}-{}",
            std::process::id(),
            SOCKET_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // `create_dir`, not `create_dir_all`: this must fail if the directory
        // already exists. An interrupted run leaves one behind, and a recycled
        // pid would otherwise adopt it -- along with a stale `engine.sock` that
        // makes the bind fail. Say which of those happened.
        std::fs::create_dir(&socket_root)
            .unwrap_or_else(|error| panic!("could not create socket root {socket_root}: {error}"));
        let socket_root = SocketRoot(socket_root);
        let socket = socket_root.0.join("engine.sock");
        assert!(
            socket.as_str().len() <= 103,
            "socket path is {} bytes, over sun_path's 103-byte cap: {socket}",
            socket.as_str().len()
        );

        let child = tokio::process::Command::new(&binary)
            .arg("--socket-path")
            .arg(socket.as_str())
            .arg("--state-root")
            .arg(state.as_str())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|error| panic!("could not spawn {binary}: {error}"));

        let mut engine = Self {
            child,
            socket,
            _socket_root: socket_root,
            _root: root,
        };
        engine.await_socket().await;
        engine
    }

    /// Waits for the socket to appear, then for a connection to succeed.
    ///
    /// Both halves are needed: the file appears before the listener accepts,
    /// so waiting only for the file races the bind. Bounded, because a hang
    /// here is a failure to report rather than a condition to wait out.
    ///
    /// An engine that died at startup is a different fact from an engine that
    /// is slow, so this checks the child every pass and says which one happened
    /// rather than letting a dead engine spend the whole bound telling the slow
    /// story.
    ///
    /// The bound is 120s and not 30s because a binary's first execution is far
    /// slower than its later ones: a freshly built `arca-engine` measured 997ms
    /// on a fresh inode against 10ms warm, and freshly linked test binaries on
    /// the same machine took ~50s each to start under load. 30s failed on a
    /// cold engine. Widening a liveness wait weakens no claim this tier makes,
    /// and a false failure on a cold CI box costs more than a late true one.
    async fn await_socket(&mut self) {
        let bound = Duration::from_secs(120);
        let started = std::time::Instant::now();
        loop {
            if self.socket.exists()
                && ChannelTransport::connect(self.socket.as_std_path().to_owned())
                    .await
                    .is_ok()
            {
                return;
            }
            match self.child.try_wait() {
                Ok(Some(status)) => panic!(
                    "engine exited with {status} before accepting a connection on {}",
                    self.socket
                ),
                Ok(None) => {}
                Err(error) => {
                    panic!("could not check on the engine for {}: {error}", self.socket)
                }
            }
            assert!(
                started.elapsed() < bound,
                "engine did not accept a connection on {} within {:.1}s",
                self.socket,
                started.elapsed().as_secs_f64()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn transport(&self) -> ChannelTransport {
        ChannelTransport::connect(self.socket.as_std_path().to_owned())
            .await
            .expect("connecting to a started engine must succeed")
    }

    pub async fn kill(mut self) {
        self.child.kill().await.unwrap();
    }
}
