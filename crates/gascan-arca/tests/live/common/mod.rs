use camino::{Utf8Path, Utf8PathBuf};
// The OCI layout builders moved to `gascan-oci-fixture` when the
// daemon-on-engine tier in `gascan-e2e` came to need exactly them, and neither
// tier can import the other's `tests/` tree. Re-exported here under the names
// this tier already uses, so the move is not also a rename at fifteen call
// sites.
use gascan_arca::ChannelTransport;
pub use gascan_oci_fixture::{layout_running, layout_running_with_directories};
use std::collections::BTreeMap;
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
pub struct SocketRoot(Utf8PathBuf);

impl SocketRoot {
    /// A fresh root under `/tmp`, distinct from every other one this process
    /// has made.
    ///
    /// The socket does NOT live under a `TempDir`, and that is deliberate.
    /// `sun_path` is capped at 103 bytes (swift-nio asserts it explicitly in
    /// `NIOCore/SocketAddresses.swift`), and macOS temp dirs are
    /// `/var/folders/<...>/T/<...>` -- a measured path came to 74 bytes, which
    /// fits but leaves little room. Arca's own tests hit this exact wall during
    /// Task 7 and had to move to `/tmp`.
    pub fn fresh() -> Self {
        let path = Utf8PathBuf::from(format!(
            "/tmp/gascan-arca-live-{}-{}",
            std::process::id(),
            SOCKET_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // `create_dir`, not `create_dir_all`: this must fail if the directory
        // already exists. An interrupted run leaves one behind, and a recycled
        // pid would otherwise adopt it -- along with a stale `engine.sock` that
        // makes the bind fail. Say which of those happened.
        std::fs::create_dir(&path)
            .unwrap_or_else(|error| panic!("could not create socket root {path}: {error}"));
        Self(path)
    }

    /// The socket path inside this root, asserted to fit `sun_path`.
    ///
    /// The assertion is here rather than at the callers so a path that meets
    /// the cap says so, rather than arriving as a mystery bind failure.
    pub fn socket(&self) -> Utf8PathBuf {
        let socket = self.0.join("engine.sock");
        assert!(
            socket.as_str().len() <= 103,
            "socket path is {} bytes, over sun_path's 103-byte cap: {socket}",
            socket.as_str().len()
        );
        socket
    }
}

impl Drop for SocketRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The three paths named by the environment that any engine in this tier needs.
///
/// One type rather than three `required_path` calls per spawn site, because
/// there are two such sites now -- [`LiveEngine::start_with_images`] and the
/// forced startup instrument -- and a second copy of the three variable names
/// and their directives is a second thing to keep in step with the engine's
/// options.
pub struct EngineInputs {
    /// `GASCAN_ARCA_ENGINE_BIN`.
    pub binary: String,
    /// `GASCAN_ARCA_KERNEL_PATH`.
    pub kernel: String,
    /// `GASCAN_ARCA_VMINIT_LAYOUT`.
    pub vminit: String,
}

impl EngineInputs {
    pub fn from_environment() -> Self {
        Self {
            binary: required_path(
                "GASCAN_ARCA_ENGINE_BIN",
                "a built arca-engine",
                "run scripts/build-arca-engine.sh and use its second output line",
            ),
            kernel: required_path(
                "GASCAN_ARCA_KERNEL_PATH",
                "the vmlinux the engine boots guests with",
                "an installed Arca.app carries one at \
                 Contents/Resources/vmlinux; ~/.arca/vmlinux symlinks it",
            ),
            vminit: required_path(
                "GASCAN_ARCA_VMINIT_LAYOUT",
                "an OCI layout holding arca-vminit:latest",
                "an installed Arca.app populates ~/.arca/vminit",
            ),
        }
    }

    /// The `serve` invocation Gas Can ships: all four options, none defaulted.
    ///
    /// The engine made `--kernel-path` and `--vminit-layout` required when it
    /// took ownership of its own state root, and a tier passing only the first
    /// two spawns nothing: MEASURED against the branch binary as `Missing
    /// expected argument '--kernel-path'`, exit 64. Every live test here was
    /// `#[ignore]`d, so nothing ran them and nothing noticed -- a tier that
    /// cannot start its subject and a tier nobody runs look identical from
    /// outside.
    pub fn serve_arguments<'a>(
        &'a self,
        socket: &'a Utf8Path,
        state: &'a Utf8Path,
    ) -> Vec<&'a str> {
        vec![
            "--socket-path",
            socket.as_str(),
            "--state-root",
            state.as_str(),
            "--kernel-path",
            &self.kernel,
            "--vminit-layout",
            &self.vminit,
        ]
    }
}

/// Reads a required path from the environment, or panics saying how to get one.
///
/// Absence is a panic and never a skip, for the reason
/// `GASCAN_ARCA_ENGINE_BIN` was given one: a live test that silently skips is a
/// live test nobody notices has stopped running.
///
/// It deliberately does NOT check that the path exists. The engine validates
/// its own three inputs and refuses to start naming which one is missing and
/// the path it tried (design §2.3), and a second copy of that check here would
/// be a guard no test can measure -- delete either copy and the other still
/// catches it. What this owes the reader is the variable's absence, which the
/// engine cannot report because it never runs.
fn required_path(variable: &str, what: &str, directive: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must name {what}; {directive}"))
}

/// The OCI layout every live test derives its images from.
///
/// Same shape as the other three, for the reason `required_path` records. The
/// tier never fetches: `arca-engine` refuses to, and a test that reached the
/// network would fail for reasons that have nothing to do with the engine.
pub fn base_oci_layout() -> Utf8PathBuf {
    Utf8PathBuf::from(required_path(
        "GASCAN_ARCA_BASE_OCI_LAYOUT",
        "an OCI layout holding one small linux/arm64 image with a shell and nc",
        "build one with 'skopeo copy --override-os linux --override-arch arm64 \
         docker://docker.io/library/alpine:3.20 oci:/tmp/alpine-oci:alpine:3.20'",
    ))
}

/// A `/bin/sh` program that runs its arguments and kills them when this
/// process stops holding the other end of stdin.
///
/// **This exists because `kill_on_drop(true)` does not survive the parent being
/// killed rather than dropped.** An `arca-engine` was found on this machine
/// still running four days after the live run that spawned it, orphaned to PID
/// 1 (recorded in `docs/status/START-HERE.md`). Nothing in-process can run
/// after `SIGKILL`, so the guarantee has to live in a process that outlives
/// this one and watches for its death; the pipe is the watch, because the
/// kernel closes it however the holder dies.
///
/// `exec 3<&0` and the watcher's `<&3` are load-bearing, and MEASURED to be. A
/// background command in a non-interactive shell has its stdin reassigned to
/// `/dev/null` before any explicit redirection (POSIX XCU 2.9.3), so a watcher
/// without `<&3` reads EOF at once and kills the child before the pipe is ever
/// closed. Dropping just the `<&3` fails
/// `supervision::a_supervised_child_dies_when_its_parent_stops_holding_the_pipe`
/// on `the supervisor started no child within 10s` -- `pgrep` never sees the
/// child at all, because it is already dead.
///
/// `wait "$enginepid"` rather than `exec`, so the wrapper exits with the
/// engine's own status: `await_socket`'s `try_wait()` fast-fail depends on
/// seeing it, and it is what reported `Missing expected argument
/// '--kernel-path'` as `exit status: 64` in 0.06s. MEASURED through this
/// wrapper by pointing `GASCAN_ARCA_ENGINE_BIN` at a script that exits 64:
/// `engine exited with exit status: 64 before accepting a connection on
/// /tmp/gascan-arca-live-23172-0/engine.sock`, in 0.26s.
///
/// `SIGTERM` and no escalation, because escalation would need a `sleep` this
/// wrapper cannot reliably reap.
///
/// **This used to end "MEASURED: `arca-engine` exits 0.00s after `SIGTERM`,
/// with status 1", and that had gone stale in a way that mattered.** It
/// described a build with no graceful-shutdown path at all. If it were still
/// true, [`LiveEngine::kill`]'s assertion would fail every test in this tier,
/// every run. MEASURED against the current engine instead: one `SIGTERM` exits
/// **0** once the accepted connections have drained, **1** if they have not
/// drained within the engine's ten-second grace, and a crash arrives here as
/// **133**. The wrapper propagates each verbatim -- see the `wait` note below.
///
/// **What this does NOT guarantee**, and each of these leaks an engine exactly
/// as before: the wrapper itself being `SIGKILL`ed, a `kill -9` delivered to
/// the whole process group, or a machine that loses power. It also cannot
/// help in the window between the wrapper starting and the watcher being
/// backgrounded -- microseconds, and long before `await_socket` returns.
const SUPERVISOR: &str = r#"
exec 3<&0
engine=$1
shift
"$engine" "$@" &
enginepid=$!
{ while read -r _; do :; done; kill -TERM "$enginepid" 2>/dev/null; } <&3 &
watcher=$!
wait "$enginepid"
status=$?
kill -TERM "$watcher" 2>/dev/null
exit "$status"
"#;

/// `program` with `arguments`, under [`SUPERVISOR`], with the pipe held here.
///
/// `stdin` is piped and never written to. Closing it -- deliberately, or by
/// this process dying -- is the whole signal.
pub fn supervised(program: &str, arguments: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(SUPERVISOR)
        // `$0`. It is what `ps` shows for the wrapper, so it says what the
        // process is rather than leaving a bare `sh` beside the engine.
        .arg("gascan-live-supervisor")
        .arg(program)
        .args(arguments)
        .stdin(std::process::Stdio::piped())
        // Belt for the ordinary case: a dropped `Child` kills the wrapper here
        // and now, rather than waiting for the watcher to notice the pipe.
        .kill_on_drop(true);
    command
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
    /// Every image the engine's private store holds, by the tag it was loaded
    /// under, mapped to the digest the STORE recorded rather than the one the
    /// layout carried. See [`LiveEngine::image`].
    images: BTreeMap<String, String>,
    /// The task draining this engine's own stdout and stderr. See [`Diagnostics`].
    diagnostics: tokio::task::JoinHandle<String>,
    _socket_root: SocketRoot,
    _root: tempfile::TempDir,
}

/// How one engine ended, and what it said on the way.
///
/// **The status alone cannot say WHY an engine exited non-zero, and until this
/// type existed nothing here could.** `arca-engine` has exactly two deliberate
/// failure exits, both `EXIT_FAILURE` and so both `exit status: 1` from outside:
/// a drain that ran out of its ten-second grace, and a listening socket that
/// closed with no shutdown requested (Arca's `ArcaEngineCommand.releaseAndExit`
/// callers). They are different defects with different fixes and the byte does
/// not distinguish them -- but each logs its own line first, and those lines are
/// what [`Diagnostics`] carries here.
pub struct EngineExit {
    pub status: std::process::ExitStatus,
    /// Everything the engine wrote to stdout and stderr over its whole life.
    ///
    /// Both streams because the two things worth reading arrive on different
    /// ones: swift-log's default handler writes the engine's own lines to
    /// stdout, and a Swift crash trace -- the exit-133 abort this tier already
    /// reasons about -- goes to stderr.
    pub diagnostics: String,
    /// How long the engine took from its pipe closing to being reaped.
    ///
    /// **Recorded because the grace period is a deadline and a status byte only
    /// says whether it was missed.** A run whose slowest shutdown is
    /// milliseconds has orders of magnitude of headroom; one whose slowest is
    /// 9.9s is a green that was about to go red, and the two are
    /// indistinguishable from `ExitStatus::success` alone.
    pub took: Duration,
}

/// Reads a child's stdout and stderr to EOF, interleaved as they arrive.
///
/// **A reader task rather than a read after `wait`, because the pipe is a
/// bounded buffer.** An engine that wrote more than a pipeful with nothing
/// draining it would block in `write` forever, and the test waiting for it would
/// hang rather than fail -- a deadlock introduced into every test in this tier
/// by a helper meant only to explain failures in one of them.
///
/// The two streams are concatenated in arrival order per stream and not
/// globally, which is enough for what reads this: the engine's own log lines are
/// all on stdout, so their order among themselves is preserved.
struct Diagnostics;

impl Diagnostics {
    /// Takes both pipes off `child` and drains them until the engine is gone.
    fn draining(child: &mut tokio::process::Child) -> tokio::task::JoinHandle<String> {
        use tokio::io::AsyncReadExt as _;

        let mut out = child.stdout.take().expect("the engine's stdout is piped");
        let mut err = child.stderr.take().expect("the engine's stderr is piped");
        tokio::spawn(async move {
            let mut spoken = Vec::new();
            let mut shouted = Vec::new();
            // Concurrently, so neither pipe can fill while the other is being
            // read: reading one to EOF first is the same deadlock this type
            // exists to avoid, one stream along.
            let (spoke, shouted_result) =
                tokio::join!(out.read_to_end(&mut spoken), err.read_to_end(&mut shouted));
            spoke.expect("reading the engine's stdout must succeed");
            shouted_result.expect("reading the engine's stderr must succeed");
            let mut both = String::from_utf8_lossy(&spoken).into_owned();
            both.push_str(&String::from_utf8_lossy(&shouted));
            both
        })
    }
}

impl LiveEngine {
    /// Starts the engine named by `GASCAN_ARCA_ENGINE_BIN`.
    ///
    /// Panics with a directive message when the variable is absent, because a
    /// live test that silently skips is a live test nobody notices has stopped
    /// running.
    pub async fn start() -> Self {
        Self::start_with_images(&[]).await
    }

    /// The same engine, with each named OCI layout loaded into its store first.
    ///
    /// The engine's state root is created fresh per test, so every engine
    /// starts with an empty image store and a `Create` against it would be
    /// refused as `not_found`. `arca-engine image load` binds no socket, starts
    /// no VM and needs no kernel, so the store is seeded by running the same
    /// binary to completion **before** the server is spawned.
    pub async fn start_with_images(layouts: &[&Utf8Path]) -> Self {
        let inputs = EngineInputs::from_environment();
        let root = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(root.path()).unwrap().to_owned();
        let state = path.join("state");
        std::fs::create_dir_all(&state).unwrap();

        for layout in layouts {
            gascan_oci_fixture::load_image(&inputs.binary, &state, layout);
        }
        let images = gascan_oci_fixture::stored_images(&state);

        let socket_root = SocketRoot::fresh();
        let socket = socket_root.socket();

        let mut child = supervised(&inputs.binary, &inputs.serve_arguments(&socket, &state))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("could not spawn {}: {error}", inputs.binary));
        let diagnostics = Diagnostics::draining(&mut child);

        let mut engine = Self {
            child,
            socket,
            images,
            diagnostics,
            _socket_root: socket_root,
            _root: root,
        };
        engine.await_socket().await;
        engine
    }

    /// The immutable reference naming what the store holds under `tag`.
    ///
    /// The store's digest and not the layout's, for the reason
    /// [`gascan_oci_fixture::stored_image_reference`] records.
    pub fn image(&self, tag: &str) -> String {
        gascan_oci_fixture::stored_image_reference(&self.images, tag)
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

    /// The path this engine is listening on.
    ///
    /// Exposed so a test can dial the engine as something other than a gRPC
    /// client. Everything in this tier that speaks the protocol goes through
    /// [`Self::transport`]; what needs this is the case that deliberately does
    /// NOT speak it.
    pub fn socket(&self) -> &Utf8Path {
        &self.socket
    }

    pub async fn transport(&self) -> ChannelTransport {
        ChannelTransport::connect(self.socket.as_std_path().to_owned())
            .await
            .expect("connecting to a started engine must succeed")
    }

    /// Stops the engine and waits for it to be gone.
    ///
    /// **Closing stdin rather than killing the child, and the difference
    /// matters.** The child is [`SUPERVISOR`], not the engine: killing it would
    /// leave the engine running until the watcher noticed the pipe close, which
    /// is exactly the race `a_call_against_a_killed_engine_fails_rather_than_hanging`
    /// must not have. Closing the pipe is the one signal the wrapper is built
    /// around, and `wait` then returns only once the engine itself is reaped.
    ///
    /// **The exit status IS asserted, and until 2026-08-14 it deliberately was
    /// not.** Arca's engine aborted on its own graceful-shutdown path --
    /// `Cannot schedule tasks on an EventLoop that has already shut down`, then
    /// `NIOExtras/QuiescingHelper.swift:141: Fatal error: leaking promise`, then
    /// `Trace/BPT trap: 5`, arriving here as exit 133. Every one happened
    /// strictly after its test's assertions, so asserting then would have failed
    /// runs whose subject behaved correctly. The engine now waits for its
    /// ACCEPTED connections rather than for its listening socket (Arca's
    /// `ArcaEngineCommand.serve`), and `shutdown.rs` measured the change: **6
    /// crashes in 192 before, 0 in 192 after**, interleaved.
    ///
    /// **This assertion is what stops that regressing**, and it is here rather
    /// than only in `shutdown.rs` because every test in this tier stops an
    /// engine: a regression fails whichever test meets it first, at whatever
    /// rate it comes back at, instead of waiting for the one module built to
    /// look for it. `shutdown.rs` is what says how *often*; this says *whether*.
    ///
    /// A failure here is about the shutdown and nothing before it. A crash any
    /// earlier surfaces as a transport error mid-call or through
    /// `await_socket`'s `try_wait()`, and both of those paths are exercised.
    pub async fn kill(self) {
        let socket = self.socket.clone();
        let exit = self.stop().await;
        assert!(
            exit.status.success(),
            "the engine on {socket} exited with {} rather than cleanly, after {:.2}s. \
             Exit 133 is `Trace/BPT trap: 5` -- the graceful-shutdown abort; run \
             `shutdown::the_engine_exits_cleanly_after_a_container_has_been_created` \
             to see it as a rate rather than as one sample. The engine said:\n{}",
            exit.status,
            exit.took.as_secs_f64(),
            exit.diagnostics,
        );
    }

    /// Stops the engine and reports the status it exited with.
    ///
    /// **Closing stdin rather than killing the child, and the difference
    /// matters.** The child is [`SUPERVISOR`], not the engine: killing it would
    /// leave the engine running until the watcher noticed the pipe close, which
    /// is exactly the race `a_call_against_a_killed_engine_fails_rather_than_hanging`
    /// must not have. Closing the pipe is the one signal the wrapper is built
    /// around, and `wait` then returns only once the engine itself is reaped.
    ///
    /// The status is the engine's own and not the wrapper's: [`SUPERVISOR`]
    /// ends `wait "$enginepid"; status=$?; ...; exit "$status"`, so an engine
    /// killed by a signal arrives here as the shell's `128 + signo`.
    ///
    /// The diagnostics are collected AFTER the wait, because the reader task
    /// ends when both pipes reach EOF and they reach it when the last process
    /// holding the write end is gone.
    pub async fn stop(mut self) -> EngineExit {
        let signalled = std::time::Instant::now();
        drop(self.child.stdin.take());
        let stopped = tokio::time::timeout(Duration::from_secs(30), self.child.wait()).await;
        let status = match stopped {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!("could not wait on the engine supervisor: {error}"),
            // The wrapper is still up 30s after the pipe closed, so its watcher
            // did not run or the engine ignored `SIGTERM`. Say so rather than
            // hang; `kill_on_drop` cannot report it and would look like a pass.
            Err(_) => panic!(
                "the engine supervisor for {} did not exit within 30s of its pipe closing",
                self.socket
            ),
        };
        let took = signalled.elapsed();
        let diagnostics = self
            .diagnostics
            .await
            .expect("the task draining the engine's output must not panic");
        EngineExit {
            status,
            diagnostics,
            took,
        }
    }
}

/// A loopback port nothing else on this host is listening on.
///
/// Reserved by binding `127.0.0.1:0` and dropping the listener, which is the
/// technique `crates/gascan-apple/tests/live/resources.rs` uses. There is a
/// race with anything else that binds an ephemeral port in the meantime, and
/// it is the same race that tier accepts: the alternative is a fixed port that
/// collides with whatever is already listening, every run, on purpose.
pub fn reserved_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("binding an ephemeral loopback port must succeed");
    listener
        .local_addr()
        .expect("a bound listener has a local address")
        .port()
}

/// A `/bin/sh` program that answers every TCP connection on `port` with what
/// `report` prints.
///
/// **The published port is how the tests below this line read the guest, and
/// it was once the only channel there was.** `Exec` and `Logs` landed in
/// milestone 3 and both answer for real now (`exec.rs`, `logs.rs`), so a new
/// test that wants a fact from inside the sandbox has a second option; the
/// paragraph that said they refuse was true until this branch and is not now.
/// `Inspect` still reports what the STORE holds rather than what the guest
/// sees, so it remains no evidence about the guest. The existing callers --
/// what is mounted, how much memory the kernel found -- still read it the way
/// `ports.rs` reads its token, and take the same dependency on a working
/// publish. That coupling is stated in each of those tests rather than hidden:
/// a broken publish fails them all, and `ports.rs` is what says whether the
/// publish is the cause.
///
/// `while :;` and not a single accept: the first connection a test makes is
/// usually the first this responder ever serves, but a retry after a refused
/// connect must find it still listening rather than gone.
pub fn answering(port: u16, report: &str) -> String {
    format!("while :; do {report} | nc -l -p {port}; done")
}

/// The lines of one `---name---` section of a guest's answer.
///
/// **Guests send raw text and the reading is done here.** A `Cmd` baked into an
/// OCI layout cannot be run under a debugger or covered by a test, so a shell
/// program that computed its own summary would put the one step most likely to
/// be wrong in the one place nothing can check -- and a summary that came out
/// wrong is indistinguishable from the thing it summarised being wrong. Every
/// guest here prints `/proc/mounts`, `df` or `/proc/meminfo` verbatim under a
/// marker, and each caller's assertion carries the whole answer into its own
/// failure message.
pub fn report_section<'a>(report: &'a str, name: &str) -> Vec<&'a str> {
    let marker = format!("---{name}---");
    report
        .lines()
        .skip_while(|line| line.trim() != marker)
        .skip(1)
        .take_while(|line| !line.trim().starts_with("---"))
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// Connects to `127.0.0.1:<port>` until something answers, and returns what it
/// said.
///
/// Retried rather than attempted once: `Start` returns before the guest's own
/// PID 1 has run the image's `Cmd`, so the first connects are refused by a host
/// proxy with nothing behind it yet. The bound is what makes this a test and
/// not a wait -- a publish that never happens fails here, naming the port.
pub async fn read_from_loopback(port: u16, bound: Duration) -> String {
    use std::io::Read as _;

    let deadline = std::time::Instant::now() + bound;
    let mut last = String::from("never attempted");
    while std::time::Instant::now() < deadline {
        let attempt = tokio::task::spawn_blocking(move || {
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            let mut stream =
                std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut answer = String::new();
            stream.read_to_string(&mut answer)?;
            Ok::<String, std::io::Error>(answer)
        })
        .await
        .expect("the blocking connect task must not panic");
        match attempt {
            Ok(answer) if !answer.trim().is_empty() => return answer,
            Ok(_) => last = "connected and read nothing".to_owned(),
            Err(error) => last = error.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!(
        "nothing answered on 127.0.0.1:{port} within {:.1}s; last attempt: {last}. \
         If this reads `connected and read nothing`, the engine publishing nothing \
         and an unrelated process having taken {port} between the reservation and \
         the Create are INDISTINGUISHABLE from here -- both accept and stay silent. \
         Check what is listening on {port} before treating this as a publish failure",
        bound.as_secs_f64()
    );
}

/// Waits for `Inspect` to report `state`, or says what it reported instead.
///
/// A bounded poll and not a sleep: `Start` boots a real virtual machine, so the
/// transition is not synchronous with the `Ack`, and a fixed sleep would be
/// either slower than it needs to be or flaky on a loaded machine. The bound is
/// a failure to report rather than a condition to wait out.
///
/// **This reads the store's opinion and nothing else**, which is all it is for:
/// sequencing a test's own teardown, since a running container cannot be
/// removed. No claim in this tier rests on it.
pub async fn await_state(
    backend: &gascan_arca::ArcaBackend<ChannelTransport>,
    request: &gascan_core::runtime::CreateRequest,
    state: gascan_core::runtime::ContainerState,
    bound: Duration,
) {
    use gascan_core::runtime::RuntimeBackend as _;

    let started = std::time::Instant::now();
    let mut last = None;
    while started.elapsed() < bound {
        let seen = backend
            .inspect(request.id())
            .await
            .expect("inspect of a created sandbox must answer");
        if seen.as_ref().map(|sandbox| sandbox.state) == Some(state) {
            return;
        }
        last = seen;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!(
        "{} did not reach {state:?} within {:.1}s; inspect last reported {:?}",
        request.id(),
        bound.as_secs_f64(),
        last.map(|sandbox| sandbox.state),
    );
}

/// A policy-validated `CreateRequest`, which is the only kind that exists,
/// against an image the engine's own store actually holds.
///
/// `CreateRequest`'s fields are `pub(crate)` to `gascan-core` and it derives no
/// `Deserialize`, so `PolicyCompiler` is the only construction path -- there is
/// deliberately no fixture constructor. This mirrors `policy_request` in
/// `tests/backend_unary.rs`, which solves the same problem the same way against
/// the fake transport. The two cannot share code: each `tests/*.rs` is its own
/// crate, and this one is reachable only from the live tier.
///
/// `PolicyCompiler::compile` pins the approved workspace image, which no engine
/// under test has: the live tier seeds a store with `arca-engine image load` and
/// must then ask for what it seeded. `compile_for_image` is the existing seam
/// for exactly this and needs no widening.
///
/// **There is no unpinned variant any more.** A `policy_request(name)` taking
/// the approved workspace image stood here until `read_rpcs.rs` stopped calling
/// `create_container` against a sandbox that was never created -- see that
/// file's note on `CreateContainer` leaving the unimplemented list -- and it was
/// then the only caller. Every request this tier builds now names an image the
/// test seeded, which is the only kind an engine under test can honour.
///
/// The `TempDir` must outlive the request: the compiled request names its
/// canonical root.
pub fn policy_request_for_image(
    name: &str,
    image: &str,
) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    policy_request_from_manifest(name, image, "version = 1\nnetwork = 'networked'\n")
}

/// The same request again, over a manifest the caller wrote.
///
/// The manifest is the *only* knob, deliberately. Ports and the guest user are
/// manifest facts and nothing else in this tier may set them: `compile_ports`
/// is what decides that a declared port becomes `127.0.0.1:<port>:<port>` with
/// no mapping, and a test that reached around it would be asserting against a
/// request gascan itself cannot produce.
pub fn policy_request_from_manifest(
    name: &str,
    image: &str,
    manifest: &str,
) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
    use gascan_core::sandbox::SandboxSpec;

    let root = tempfile::tempdir().expect("a temporary project root");
    let path = Utf8Path::from_path(root.path()).expect("a utf-8 temporary path");
    std::fs::write(path.join("gascan.toml"), manifest).expect("a manifest");
    let spec = SandboxSpec::from_root(name, path, Manifest::load(path).expect("a manifest"))
        .expect("a spec");
    // Every flag true, which is the opposite of what the engine reports. The
    // compiler gates on what the runtime CLAIMS it can do, and this request only
    // has to be well-formed enough to send: what is under test is the engine's
    // refusal, and a request the compiler rejected would never reach it.
    let capabilities = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let request =
        PolicyCompiler::compile_for_image(spec, &capabilities, image).expect("a validated request");
    (root, request)
}

/// The exact retained set for `request`, derived from it rather than hardcoded.
///
/// `RetainedResources::new` requires an exact match against the request's
/// topology, and the manifest decides how many volumes and networks that is --
/// so a fixed list is a test that breaks when the manifest changes for reasons
/// unrelated to what it is testing. Same shape as `retained_for` in
/// `tests/backend_unary.rs`, for the same reason, and separate for the same
/// reason: each `tests/*.rs` is its own crate.
pub fn retained_for(
    request: &gascan_core::runtime::CreateRequest,
) -> Vec<gascan_core::runtime::RuntimeResource> {
    use gascan_core::runtime::{
        ResourceIdentity, ResourceKind, ResourceOwnership, RuntimeResource,
    };

    let mut retained: Vec<RuntimeResource> = request
        .volumes()
        .iter()
        .map(|volume| {
            RuntimeResource::discovered(
                ResourceIdentity::new(ResourceKind::Volume, volume.name.clone())
                    .expect("a policy-compiled volume name is valid"),
                Some(request.id().clone()),
                ResourceOwnership::GasCanOwned,
            )
        })
        .collect();
    if let Some(name) = request.network().managed_name() {
        retained.push(RuntimeResource::discovered(
            ResourceIdentity::new(ResourceKind::Network, name.to_owned())
                .expect("a policy-compiled network name is valid"),
            Some(request.id().clone()),
            ResourceOwnership::GasCanOwned,
        ));
    }
    retained
}
