use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ChannelTransport;
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
struct SocketRoot(Utf8PathBuf);

impl Drop for SocketRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
/// wrapper cannot reliably reap. MEASURED: `arca-engine` exits **0.00s** after
/// `SIGTERM`, with status 1.
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
        let binary = required_path(
            "GASCAN_ARCA_ENGINE_BIN",
            "a built arca-engine",
            "run scripts/build-arca-engine.sh and use its second output line",
        );
        let kernel = required_path(
            "GASCAN_ARCA_KERNEL_PATH",
            "the vmlinux the engine boots guests with",
            "an installed Arca.app carries one at \
             Contents/Resources/vmlinux; ~/.arca/vmlinux symlinks it",
        );
        let vminit = required_path(
            "GASCAN_ARCA_VMINIT_LAYOUT",
            "an OCI layout holding arca-vminit:latest",
            "an installed Arca.app populates ~/.arca/vminit",
        );
        let root = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(root.path()).unwrap().to_owned();
        let state = path.join("state");
        std::fs::create_dir_all(&state).unwrap();

        for layout in layouts {
            load_image(&binary, &state, layout).await;
        }
        let images = stored_images(&state);

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

        // All four options, none defaulted. The engine made `--kernel-path` and
        // `--vminit-layout` required when it took ownership of its own state
        // root, and a tier passing only the first two spawns nothing: MEASURED
        // against the branch binary as `Missing expected argument
        // '--kernel-path'`, exit 64. Every live test here was `#[ignore]`d, so
        // nothing ran them and nothing noticed -- a tier that cannot start its
        // subject and a tier nobody runs look identical from outside.
        let child = supervised(
            &binary,
            &[
                "--socket-path",
                socket.as_str(),
                "--state-root",
                state.as_str(),
                "--kernel-path",
                &kernel,
                "--vminit-layout",
                &vminit,
            ],
        )
        .spawn()
        .unwrap_or_else(|error| panic!("could not spawn {binary}: {error}"));

        let mut engine = Self {
            child,
            socket,
            images,
            _socket_root: socket_root,
            _root: root,
        };
        engine.await_socket().await;
        engine
    }

    /// The immutable reference naming what the store holds under `tag`.
    ///
    /// **THE DIGEST A REQUEST MUST NAME IS THE STORE'S, NOT THE LAYOUT'S.** The
    /// store re-wraps what it ingests: a layout whose `index.json` carries
    /// manifest `sha256:45e09956…` is recorded in
    /// `<state-root>/images/state.json` as an image *index* under
    /// `sha256:a019d0ba…`. A test that derived the digest from the layout it
    /// loaded would name content the engine does not hold, and hear
    /// `not_found` from a store that has the image.
    pub fn image(&self, tag: &str) -> String {
        let digest = self.images.get(tag).unwrap_or_else(|| {
            panic!(
                "the engine's store holds no image tagged {tag}; it holds {:?}",
                self.images.keys().collect::<Vec<_>>()
            )
        });
        format!("{}@{digest}", repository_of(tag))
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
        let status = self.stop().await;
        assert!(
            status.success(),
            "the engine on {socket} exited with {status} rather than cleanly. \
             Exit 133 is `Trace/BPT trap: 5` -- the graceful-shutdown abort; run \
             `shutdown::the_engine_exits_cleanly_after_a_container_has_been_created` \
             to see it as a rate rather than as one sample"
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
    pub async fn stop(mut self) -> std::process::ExitStatus {
        drop(self.child.stdin.take());
        let stopped = tokio::time::timeout(Duration::from_secs(30), self.child.wait()).await;
        match stopped {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!("could not wait on the engine supervisor: {error}"),
            // The wrapper is still up 30s after the pipe closed, so its watcher
            // did not run or the engine ignored `SIGTERM`. Say so rather than
            // hang; `kill_on_drop` cannot report it and would look like a pass.
            Err(_) => panic!(
                "the engine supervisor for {} did not exit within 30s of its pipe closing",
                self.socket
            ),
        }
    }
}

/// Seeds one OCI layout into an engine state root, before any engine serves it.
///
/// Failure is a panic carrying the subcommand's own output: a test whose store
/// is empty fails later as a `not_found` from `Create`, which reads as an
/// engine defect and is not one.
async fn load_image(binary: &str, state: &Utf8Path, layout: &Utf8Path) {
    let output = tokio::process::Command::new(binary)
        .arg("image")
        .arg("load")
        .arg("--state-root")
        .arg(state.as_str())
        .arg("--oci-layout")
        .arg(layout.as_str())
        .output()
        .await
        .unwrap_or_else(|error| panic!("could not run {binary} image load: {error}"));
    assert!(
        output.status.success(),
        "{binary} image load --state-root {state} --oci-layout {layout} exited with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Every tag the engine's own image store records, mapped to its digest.
///
/// Read from the store rather than from the layout, for the reason
/// [`LiveEngine::image`] records. An absent file is an empty store, which is
/// what an engine started with no layouts has.
fn stored_images(state: &Utf8Path) -> BTreeMap<String, String> {
    let path = state.join("images").join("state.json");
    let Ok(source) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&source)
        .unwrap_or_else(|error| panic!("could not parse the engine's image store {path}: {error}"));
    parsed
        .into_iter()
        .map(|(tag, descriptor)| {
            let digest = descriptor
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("{path} records {tag} with no digest: {descriptor}"))
                .to_owned();
            (tag, digest)
        })
        .collect()
}

/// The repository half of a reference, split the way both sides of the wire do.
///
/// The rule is `immutable_image_identity`'s
/// (`crates/gascan-core/src/runtime.rs`), mirrored by Arca's
/// `ImageIdentity.repository(of:)`: drop anything from `@sha256:` onward, then
/// drop a tag -- the last `:` that comes *after* the last `/`, so the port in
/// `registry.example:5000/repo` is not mistaken for one. `heldImageReferences`
/// compares the request's repository against the store's, so a split that
/// disagreed with Arca's would be refused as `not_found` for content the
/// engine holds.
fn repository_of(reference: &str) -> &str {
    let reference = reference.split_once("@sha256:").map_or(reference, |a| a.0);
    match reference.rfind(':') {
        Some(colon) if !reference[colon..].contains('/') => &reference[..colon],
        _ => reference,
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
/// **The published port is the only channel out of an Arca guest this build
/// has.** `Exec` and `Logs` are milestone 3's and both answer
/// `unsupported_capability` here (`read_rpcs.rs`), and `Inspect` reports what
/// the STORE holds rather than what the guest sees. So every test that needs a
/// fact *from inside* the sandbox -- what is mounted, how much memory the
/// kernel found -- reads it the way `ports.rs` reads its token, and takes the
/// same dependency on a working publish. That coupling is stated in each of
/// those tests rather than hidden: a broken publish fails them all, and
/// `ports.rs` is what says whether the publish is the cause.
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

/// A policy-validated `CreateRequest`, which is the only kind that exists.
///
/// `CreateRequest`'s fields are `pub(crate)` to `gascan-core` and it derives no
/// `Deserialize`, so `PolicyCompiler` is the only construction path -- there is
/// deliberately no fixture constructor. This mirrors `policy_request` in
/// `tests/backend_unary.rs`, which solves the same problem the same way against
/// the fake transport. The two cannot share code: each `tests/*.rs` is its own
/// crate, and this one is reachable only from the live tier.
///
/// The `TempDir` must outlive the request: the compiled request names its
/// canonical root.
pub fn policy_request(name: &str) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    policy_request_for_image(name, gascan_core::policy::PolicyCompiler::workspace_image())
}

/// The same request, against an image the engine's own store actually holds.
///
/// `PolicyCompiler::compile` pins the approved workspace image, which no engine
/// under test has: the live tier seeds a store with `arca-engine image load` and
/// must then ask for what it seeded. `compile_for_image` is the existing seam
/// for exactly this and needs no widening.
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

/// A one-image OCI layout that runs `command`, written beside a base layout.
///
/// **`CreateRequest` carries no argv, so this is the only way the tier can
/// decide what a sandbox runs.** `engine.proto`'s `CreateRequest` has no
/// command and no entrypoint field, and `SandboxEngineService` passes
/// `entrypoint: nil, command: nil` deliberately -- the image's own config
/// decides. The environment is no way in either: `policy.rs` sets it from
/// `guest_environment()`, a fixed map with no manifest passthrough. So
/// `gascan-apple`'s `guest_argv` technique does not transfer at all, and the
/// published-port test's responder has to be baked into an image. The port it
/// listens on is therefore known only at image-build time, which is why the
/// image is built during the test rather than prepared by a maintainer.
///
/// This is not an image builder. It reuses the base layout's layers verbatim
/// and writes three small blobs: a config with a new `Cmd`, a manifest naming
/// that config, and an index naming that manifest under `tag`. The rootfs is
/// untouched, so the `diff_ids` still describe it.
///
/// The base layout's `index.json` must name exactly one manifest. Anything
/// else would make "which image is this derived from" a choice this function
/// would have to guess at.
pub fn layout_running(
    base: &Utf8Path,
    destination: &Utf8Path,
    tag: &str,
    command: &[&str],
) -> Utf8PathBuf {
    layout_running_with_directories(base, destination, tag, command, &[])
}

/// The same, with a layer of its own creating each of `directories`.
///
/// **A mount target that does not exist in the image is not mounted, and
/// nothing says so.** MEASURED against this engine: a sandbox whose three
/// managed volumes target `/home/workspace/.local`, `.cache` and `.config` on a
/// stock alpine rootfs starts successfully, `Inspect` reports it running, and
/// the guest's `/proc/partitions` shows all three block devices attached at
/// exactly their declared sizes -- `vdd` 262144 blocks, `vde` 524288, `vdf`
/// 1048576 -- while `/proc/mounts` lists none of them and `/home` is empty. The
/// engine logs no warning. `/workspace` mounts on the same guest, so the
/// difference is the depth: `/workspace` needs one directory under `/` and
/// `/home/workspace/.local` needs two under an existing `/home`.
///
/// The production workspace image creates all three
/// (`images/workspace/Dockerfile:142-143`), so this is what the tier needs to
/// resemble it. Ancestors are derived rather than demanded from the caller: a
/// list that named a leaf and forgot its parent would reproduce the very
/// failure this exists to remove.
pub fn layout_running_with_directories(
    base: &Utf8Path,
    destination: &Utf8Path,
    tag: &str,
    command: &[&str],
    directories: &[&str],
) -> Utf8PathBuf {
    use serde_json::{Value, json};

    copy_tree(base, destination);

    let index: Value = read_json(&destination.join("index.json"));
    let manifests = index["manifests"]
        .as_array()
        .unwrap_or_else(|| panic!("{base}/index.json has no manifests array"));
    assert_eq!(
        manifests.len(),
        1,
        "{base}/index.json must name exactly one manifest; it names {}",
        manifests.len()
    );
    let mut manifest: Value = read_json(&blob_path(destination, digest_of(&manifests[0])));
    let mut config: Value = read_json(&blob_path(destination, digest_of(&manifest["config"])));

    // `Entrypoint` is cleared as well as `Cmd` being set. A base image that
    // carried one would prepend it to the command below, and the responder
    // would run as arguments to something else.
    config["config"]["Cmd"] = json!(command);
    config["config"]["Entrypoint"] = Value::Null;

    if !directories.is_empty() {
        // The layer goes on top, and `diff_ids` is appended in the same
        // position: the two lists are parallel by ordinal, so a layer added to
        // one and not the other describes a rootfs the image does not have.
        let archive = directory_archive(directories);
        let diff_id = format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&archive)
        );
        let compressed = gzip(&archive);
        let digest = write_bytes(destination, &compressed);
        manifest["layers"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("{base}'s manifest has no layers array"))
            .push(json!({
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": digest,
                "size": compressed.len(),
            }));
        config["rootfs"]["diff_ids"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("{base}'s config has no rootfs.diff_ids array"))
            .push(json!(diff_id));
    }

    let config_blob = write_blob(destination, &config);
    manifest["config"]["digest"] = json!(config_blob.0);
    manifest["config"]["size"] = json!(config_blob.1);
    let manifest_blob = write_blob(destination, &manifest);

    std::fs::write(
        destination.join("index.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [{
                "mediaType": manifest["mediaType"],
                "digest": manifest_blob.0,
                "size": manifest_blob.1,
                "annotations": { "org.opencontainers.image.ref.name": tag },
            }],
        }))
        .expect("an index serialises"),
    )
    .unwrap_or_else(|error| panic!("could not write {destination}/index.json: {error}"));
    destination.to_owned()
}

/// A POSIX `ustar` archive holding `directories` and every ancestor of each.
///
/// Written by hand rather than with a crate, and it is 40 lines because a
/// directory entry is a header and no data at all. The alternative was a
/// `tar` dependency for the one thing this tier needs from it, or shelling out
/// to the host's `tar` -- which on macOS writes AppleDouble entries into the
/// archive unless told not to, and would put a host tool's defaults inside the
/// image under test.
///
/// Mode 0755 and root ownership, which is what the workspace image gives these
/// same three directories before it hands two of them to the `workspace` group
/// (`images/workspace/Dockerfile:142-143`). The tier's guests run as root, so
/// nothing here needs the finer ownership and asserting it would be asserting
/// against a fixture.
fn directory_archive(directories: &[&str]) -> Vec<u8> {
    let mut names: Vec<String> = Vec::new();
    for directory in directories {
        let mut ancestor = String::new();
        for component in directory.trim_matches('/').split('/') {
            ancestor.push_str(component);
            ancestor.push('/');
            if !names.contains(&ancestor) {
                names.push(ancestor.clone());
            }
        }
    }

    let mut archive = Vec::new();
    for name in &names {
        let mut header = [0_u8; 512];
        assert!(
            name.len() < 100,
            "{name} does not fit a ustar header's 100-byte name field"
        );
        header[..name.len()].copy_from_slice(name.as_bytes());
        // Every numeric field is octal, NUL-terminated, and zero-padded to one
        // less than its width. `chksum` is the exception: it is spaces while
        // the sum is taken, then six digits, a NUL and a space.
        header[100..108].copy_from_slice(b"0000755\0"); // mode
        header[108..116].copy_from_slice(b"0000000\0"); // uid
        header[116..124].copy_from_slice(b"0000000\0"); // gid
        header[124..136].copy_from_slice(b"00000000000\0"); // size
        header[136..148].copy_from_slice(b"00000000000\0"); // mtime
        header[148..156].copy_from_slice(b"        "); // chksum, while summing
        header[156] = b'5'; // typeflag: directory
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[265..269].copy_from_slice(b"root");
        header[297..301].copy_from_slice(b"root");
        let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        archive.extend_from_slice(&header);
    }
    // Two zero blocks end the archive, then the whole thing is padded to a
    // 10240-byte record. Both are what `tar` itself writes.
    archive.extend_from_slice(&[0_u8; 1024]);
    archive.resize(archive.len().div_ceil(10240) * 10240, 0);
    archive
}

/// `data` in gzip form, with every deflate block stored rather than compressed.
///
/// A stored block is the one deflate encoding that needs no compressor: five
/// bytes of header and the bytes themselves. The layer this wraps is a few
/// kilobytes of mostly zeroes, so the size costs nothing, and the media type
/// the manifest declares is the same `tar+gzip` the base layout's own layer
/// carries -- the unpacker takes exactly the path it already takes.
fn gzip(data: &[u8]) -> Vec<u8> {
    // Magic, deflate, no flags, no mtime, no extra flags, unix.
    let mut out = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0x03];
    let mut chunks = data.chunks(0xffff).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let length = u16::try_from(chunk.len()).expect("a chunk is at most 0xffff bytes");
        out.push(u8::from(chunks.peek().is_none())); // BFINAL, BTYPE = stored
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    #[expect(
        clippy::cast_possible_truncation,
        reason = "gzip's ISIZE is defined as the length modulo 2^32"
    )]
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// The CRC-32 gzip's trailer carries, computed a bit at a time.
///
/// No table: this runs over a few kilobytes once per test, and a table would be
/// a second thing to get right.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn digest_of(descriptor: &serde_json::Value) -> &str {
    descriptor["digest"]
        .as_str()
        .unwrap_or_else(|| panic!("an OCI descriptor with no digest: {descriptor}"))
}

fn blob_path(layout: &Utf8Path, digest: &str) -> Utf8PathBuf {
    let (algorithm, hex) = digest
        .split_once(':')
        .unwrap_or_else(|| panic!("{digest} is not an OCI digest"));
    layout.join("blobs").join(algorithm).join(hex)
}

fn read_json(path: &Utf8Path) -> serde_json::Value {
    let source =
        std::fs::read(path).unwrap_or_else(|error| panic!("could not read {path}: {error}"));
    serde_json::from_slice(&source)
        .unwrap_or_else(|error| panic!("could not parse {path} as json: {error}"))
}

/// Writes `value` as a content-addressed blob, returning its digest and size.
///
/// The bytes that are hashed are the bytes that are written -- one
/// serialisation, used for both -- because a digest taken over a second
/// rendering would name content the layout does not contain, and the engine
/// verifies blobs it loads.
fn write_blob(layout: &Utf8Path, value: &serde_json::Value) -> (String, usize) {
    let bytes = serde_json::to_vec(value).expect("a blob serialises");
    let digest = write_bytes(layout, &bytes);
    (digest, bytes.len())
}

/// Writes `bytes` under their own digest, and returns it.
fn write_bytes(layout: &Utf8Path, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    let path = blob_path(layout, &digest);
    std::fs::write(&path, bytes).unwrap_or_else(|error| panic!("could not write {path}: {error}"));
    digest
}

fn copy_tree(from: &Utf8Path, to: &Utf8Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|error| panic!("could not create {to}: {error}"));
    let entries = std::fs::read_dir(from)
        .unwrap_or_else(|error| panic!("could not read the base layout {from}: {error}"));
    for entry in entries {
        let entry = entry.expect("a directory entry");
        let name = entry.file_name();
        let name = name.to_str().expect("a utf-8 layout entry name");
        let source = from.join(name);
        let target = to.join(name);
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::copy(&source, &target)
                .unwrap_or_else(|error| panic!("could not copy {source} to {target}: {error}"));
        }
    }
}
