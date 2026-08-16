use crate::common::{
    LiveEngine, await_state, base_oci_layout, layout_running, policy_request_from_manifest,
};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{
    ContainerState, CreateRequest, ExecInput, ExecOutput, ExecRequest, ExecSession, RuntimeBackend,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The backend over a real engine, not a fake.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

/// `user = 'root'` because the base layout is a stock alpine with no
/// `workspace` user, exactly as `lifecycle.rs` records.
const MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

/// An image whose PID 1 outlives the test, because `Exec` needs a container in
/// state `running` and nothing else in this tier does for as long.
///
/// The same shape as `lifecycle.rs`'s `staying_up`, and for one of its two
/// reasons: the teardown. A container whose PID 1 has exited cannot be stopped
/// -- `lifecycle.rs` measured that as `CancellationError()` -- so a short-lived
/// `Cmd` would leave every test in this file failing at its own cleanup.
fn staying_up(destination: &Utf8Path, tag: &str) -> Utf8PathBuf {
    layout_running(
        &base_oci_layout(),
        destination,
        tag,
        &["sh", "-c", "while :; do sleep 1; done"],
    )
}

/// A booted sandbox, and everything needed to exec into it.
struct Sandbox {
    engine: LiveEngine,
    backend: ArcaBackend<gascan_arca::ChannelTransport>,
    request: CreateRequest,
    _project: tempfile::TempDir,
    _images: tempfile::TempDir,
}

impl Sandbox {
    /// Boots one, which costs a virtual machine -- so each test in this file
    /// runs several execs against one of these rather than one exec each.
    async fn boot(name: &str) -> Self {
        let tag = format!("gascan-live-exec-{name}:latest");
        let images = tempfile::tempdir().expect("a temporary layout root");
        let layout = staying_up(
            Utf8Path::from_path(images.path()).expect("a utf-8 path"),
            &tag,
        );

        let engine = LiveEngine::start_with_images(&[&layout]).await;
        let backend = backend(&engine).await;
        let (project, request) = policy_request_from_manifest(name, &engine.image(&tag), MANIFEST);

        backend
            .prepare_image(request.image())
            .await
            .expect("the store holds the image the request names");
        backend
            .create(request.clone())
            .await
            .expect("create against a seeded store must succeed");
        backend
            .start(request.id())
            .await
            .expect("start must boot the sandbox");
        // Generous: a first start boots a virtual machine from a kernel and a
        // vminit layout. Bounded all the same.
        await_state(
            &backend,
            &request,
            ContainerState::Running,
            Duration::from_secs(180),
        )
        .await;

        Self {
            engine,
            backend,
            request,
            _project: project,
            _images: images,
        }
    }

    /// Opens an exec. The session is live: nothing has been read from it.
    ///
    /// **Bounded, and this is the await that has to be.** It is the one that hung
    /// for ten minutes against the engine before `acceptRPC`: tonic does not hand
    /// its caller a stream until the response headers arrive, so an engine that
    /// says nothing leaves the test here, upstream of every other bound in this
    /// file. A regression of that one line must fail with a message that names
    /// this call, not sit silently until the harness is killed.
    async fn exec(&self, argv: &[&str], tty: bool, stdin: &[u8]) -> ExecSession {
        let opening = self.backend.exec(ExecRequest {
            id: self.request.id().clone(),
            argv: argv.iter().map(|part| (*part).to_owned()).collect(),
            stdin: stdin.to_vec(),
            environment: BTreeMap::new(),
            tty,
        });
        match tokio::time::timeout(Duration::from_secs(60), opening).await {
            Err(_) => panic!(
                "Exec did not open a session for {argv:?} within 60s; the engine accepts the \
                 RPC before it has anything to say, so this is where a missing acceptRPC lands"
            ),
            Ok(Err(error)) => panic!("Exec must open a session for {argv:?}: {error}"),
            Ok(Ok(session)) => session,
        }
    }

    /// Stops the sandbox and asserts the engine's own exit status.
    async fn teardown(self) {
        self.backend
            .stop(self.request.id())
            .await
            .expect("stop must answer for a running sandbox");
        await_state(
            &self.backend,
            &self.request,
            ContainerState::Stopped,
            Duration::from_secs(120),
        )
        .await;
        self.engine.kill().await;
    }
}

/// Everything one exec produced, drained to its terminal frame.
#[derive(Debug)]
struct Completed {
    stdout: String,
    stderr: String,
    code: i32,
    signal: i32,
}

/// Reads a session to its `Exit`, or panics saying what arrived instead.
///
/// The bound is what turns a missing effect into a failure rather than a hang,
/// and that matters here more than anywhere else in this tier: the signal test
/// below is a `sleep 300`, so an engine that accepted a signal and sent nothing
/// would otherwise sit for five minutes and then fail for the wrong reason.
async fn drain(session: &mut ExecSession, bound: Duration) -> Completed {
    let started = Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    loop {
        let remaining = bound.saturating_sub(started.elapsed());
        match tokio::time::timeout(remaining, session.next()).await {
            Err(_) => panic!(
                "no Exit frame within {:.1}s; stdout so far {:?} and stderr {:?}",
                bound.as_secs_f64(),
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ),
            Ok(None) => panic!(
                "the exec stream ended with no Exit frame; stdout {:?}, stderr {:?}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ),
            Ok(Some(Err(error))) => panic!(
                "the engine refused the exec: {error}; stdout so far {:?}",
                String::from_utf8_lossy(&stdout)
            ),
            Ok(Some(Ok(ExecOutput::Stdout(bytes)))) => stdout.extend_from_slice(&bytes),
            Ok(Some(Ok(ExecOutput::Stderr(bytes)))) => stderr.extend_from_slice(&bytes),
            Ok(Some(Ok(ExecOutput::Exit { code, signal }))) => {
                return Completed {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    code,
                    signal,
                };
            }
        }
    }
}

/// Sends one client frame under a bound.
///
/// **Bounded because an unbounded await here hid a defect once.** The first run
/// of this file sat for ten minutes with no panic at all: `drain`'s bound had
/// not been reached because the test was still upstream of it, and an await with
/// no bound cannot say which one it was.
///
/// **Every await that touches a live session is bounded: this one, `drain`,
/// `refusal`, `read_until`, and `Sandbox::exec`.** `Sandbox::boot`'s calls are
/// deliberately NOT -- `start_with_images`, `prepare_image`, `create` and `start`
/// keep the bounds the rest of this tier uses, `await_state`'s 180s among them,
/// so that a boot failure reads the same here as in `lifecycle.rs`. An earlier
/// version of this comment claimed every await in the file was bounded, which
/// was false of the very call that had hung.
async fn send(session: &mut ExecSession, input: ExecInput, bound: Duration) {
    let described = format!("{input:?}");
    match tokio::time::timeout(bound, session.send(input)).await {
        Err(_) => panic!(
            "sending {described} to the session did not return within {:.1}s",
            bound.as_secs_f64()
        ),
        Ok(Err(error)) => panic!("a live session must accept {described}: {error}"),
        Ok(Ok(())) => {}
    }
}

/// The same, for a session that is expected to be refused rather than to run.
///
/// **The very next frame, not the next error frame**, and the difference is the
/// assertion: gascan stops reading at an error frame, so a refusal that arrived
/// after output would mean the engine had begun answering something it was
/// about to refuse. Anything but an error here fails.
async fn refusal(session: &mut ExecSession, bound: Duration) -> String {
    match tokio::time::timeout(bound, session.next()).await {
        Err(_) => panic!("no refusal within {:.1}s", bound.as_secs_f64()),
        Ok(None) => panic!("the exec stream ended without refusing"),
        Ok(Some(Err(error))) => error.code().to_owned(),
        Ok(Some(Ok(output))) => panic!("expected a refusal, got {output:?}"),
    }
}

/// Reads stdout until `marker` appears, so a test can act on a guest process it
/// knows is running.
///
/// The handshake is not decoration. `signalExec` refuses an exec whose process
/// has not started (`ExecManager.swift:400-402`), so a test that signalled
/// immediately would be racing the guest and would sometimes measure that
/// refusal instead of the signal.
async fn read_until(session: &mut ExecSession, marker: &str, bound: Duration) -> String {
    let started = Instant::now();
    let mut stdout = Vec::new();
    loop {
        let remaining = bound.saturating_sub(started.elapsed());
        match tokio::time::timeout(remaining, session.next()).await {
            Err(_) => panic!(
                "{marker:?} never arrived within {:.1}s; stdout held {:?}",
                bound.as_secs_f64(),
                String::from_utf8_lossy(&stdout)
            ),
            Ok(None) => panic!("the exec stream ended before {marker:?} arrived"),
            Ok(Some(Err(error))) => panic!("the engine refused the exec: {error}"),
            Ok(Some(Ok(ExecOutput::Stdout(bytes)))) => {
                stdout.extend_from_slice(&bytes);
                let text = String::from_utf8_lossy(&stdout).into_owned();
                if text.contains(marker) {
                    return text;
                }
            }
            Ok(Some(Ok(ExecOutput::Stderr(bytes)))) => stdout.extend_from_slice(&bytes),
            Ok(Some(Ok(ExecOutput::Exit { code, signal }))) => {
                panic!("the exec exited (code {code}, signal {signal}) before {marker:?} arrived")
            }
        }
    }
}

/// `Exec` over a real container: both streams kept apart, stdin carried in, and
/// the command's own exit status coming back.
///
/// **The exit status is the assertion that makes the rest mean anything.** A
/// stdout frame proves an adapter; `exit 3` coming back as 3 proves the engine
/// ran the command the request named and waited for it, which is what
/// distinguishes `Exec` from a stream that echoes.
///
/// **`Exit.signal` is asserted to be 0 and that is not a placeholder.** Nothing
/// on the engine's path carries a signal number: the guest reaps with `wait4`
/// and collapses the status to `128 + N` for a signalled process
/// (`ContainerizationOS/Command.swift:306-315`), and `ExitStatus` carries no
/// signal number at all -- only `exitCode` and `exitedAt`
/// (`ExitStatus.swift:23`, `:25`). gascan's Apple backend reports the same zero
/// (`gascan-apple/src/backend.rs:604`). A signal delivered to a guest process is
/// therefore observed in `code`, which is what the third test below does.
///
/// RUN, against Arca `af22685` on 2026-08-16: the full live tier reported
/// `19 passed; 0 failed` in 244.60s, these three among them.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout"]
async fn exec_carries_both_streams_and_the_commands_own_exit_status() {
    let sandbox = Sandbox::boot("exec").await;

    let mut session = sandbox
        .exec(&["sh", "-c", "echo out; echo err 1>&2; exit 3"], false, b"")
        .await;
    let completed = drain(&mut session, Duration::from_secs(60)).await;
    assert_eq!(
        completed.stdout, "out\n",
        "stdout must carry exactly what the command wrote to fd 1: {completed:?}"
    );
    assert_eq!(
        completed.stderr, "err\n",
        "without a tty the two streams stay apart: {completed:?}"
    );
    assert_eq!(
        completed.code, 3,
        "the command's own exit status must come back: {completed:?}"
    );
    assert_eq!(completed.signal, 0, "see this test's note: {completed:?}");

    // `cat` is the whole stdin proof in one command: it ends only on EOF, so an
    // engine that carried the bytes but never closed stdin hangs here instead of
    // passing. The first chunk rides in on the request, the second is sent
    // mid-session, and both paths through the adapter are therefore driven.
    let mut session = sandbox.exec(&["cat"], false, b"hello\n").await;
    send(
        &mut session,
        ExecInput::Stdin(b"there\n".to_vec()),
        Duration::from_secs(30),
    )
    .await;
    send(&mut session, ExecInput::Close, Duration::from_secs(30)).await;
    let completed = drain(&mut session, Duration::from_secs(60)).await;
    assert_eq!(
        completed.stdout, "hello\nthere\n",
        "cat must read both chunks in order and see EOF: {completed:?}"
    );
    assert_eq!(completed.code, 0, "cat exits 0 on EOF: {completed:?}");

    sandbox.teardown().await;
}

/// **The `tty` capability's instrument, and the guest is what settles it.**
///
/// One command, run twice, with `tty` as the only difference:
///
/// ```sh
/// if test -t 1; then echo isatty; else echo notatty; fi; echo err 1>&2
/// ```
///
/// `test -t 1` is the guest process asking the kernel whether its own fd 1 is a
/// terminal, which is not a thing this engine can answer on its behalf. The
/// merge is the second half: with `tty` set, `startExec` sets
/// `processConfig.terminal` and then attaches no stderr writer at all
/// (`ExecManager.swift:211`, `:231`), so `err` reaching this process on the
/// **stdout** stream can only be the guest's own fds having been merged onto one
/// pty. An engine that merely dropped the stderr stream would show `isatty`
/// missing and `err` nowhere.
///
/// **Both halves are asserted in both directions**, because either alone is
/// satisfiable by an accident: a build that always allocated a terminal would
/// pass the tty half, and one that never did would pass the control.
///
/// RUN, against Arca `af22685` on 2026-08-16: the full live tier reported
/// `19 passed; 0 failed` in 244.60s, these three among them.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout"]
async fn a_tty_exec_gives_the_guest_a_terminal_and_merges_stderr_into_stdout() {
    const PROBE: &str = "if test -t 1; then echo isatty; else echo notatty; fi; echo err 1>&2";
    let sandbox = Sandbox::boot("tty").await;

    let mut session = sandbox.exec(&["sh", "-c", PROBE], false, b"").await;
    let plain = drain(&mut session, Duration::from_secs(60)).await;
    assert!(
        plain.stdout.contains("notatty"),
        "without tty the guest must find fd 1 is not a terminal: {plain:?}"
    );
    assert!(
        plain.stderr.contains("err"),
        "without tty stderr arrives on its own stream: {plain:?}"
    );

    let mut session = sandbox.exec(&["sh", "-c", PROBE], true, b"").await;
    let terminal = drain(&mut session, Duration::from_secs(60)).await;
    assert!(
        terminal.stdout.contains("isatty"),
        "with tty the guest itself must report fd 1 is a terminal: {terminal:?}"
    );
    assert!(
        terminal.stdout.contains("err"),
        "with tty stderr must arrive merged into stdout: {terminal:?}"
    );
    assert!(
        terminal.stderr.is_empty(),
        "with tty there is no stderr stream at all: {terminal:?}"
    );

    sandbox.teardown().await;
}

/// **The `signals` capability's instrument, and it asserts an effect on the
/// guest process rather than a call that returned.**
///
/// `ExecManager.signalExec`'s own note says why, and it is the requirement Task
/// 4 left for this file: Arca's VM-free tests pin the guards and not the send,
/// so `try await process.kill(resolved)` can be deleted outright and
/// `swift test --filter ExecSignalTests` stays green. A live test asserting
/// "no error" would leave that mutation green at every tier and the `signals`
/// flag would be raised over a path that sends nothing.
///
/// So the assertion is the exit status, and it is asserted **twice with
/// different numbers**. `sleep` has no handler for either signal, so the guest
/// reports `128 + N` -- 143 for SIGTERM and 137 for SIGKILL. One number alone
/// would be satisfied by an engine that hardcoded a signal; two say the number
/// the client sent is the number that arrived. An engine that sends nothing
/// fails by the 60s bound rather than by assertion, which the panic message
/// says out loud.
///
/// The third exec is requirement 4 of the design: a number Containerization's
/// Linux signal map has no entry for is refused as `invalid_state` naming it,
/// never coerced to a default.
///
/// RUN, against Arca `af22685` on 2026-08-16: the full live tier reported
/// `19 passed; 0 failed` in 244.60s, these three among them.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout"]
async fn a_signal_reaches_the_guest_process_and_decides_how_it_exits() {
    let sandbox = Sandbox::boot("signals").await;

    for (number, expected) in [(15, 143), (9, 137)] {
        let mut session = sandbox
            .exec(&["sh", "-c", "echo ready; sleep 300"], false, b"")
            .await;
        read_until(&mut session, "ready", Duration::from_secs(60)).await;
        send(
            &mut session,
            ExecInput::Signal(number),
            Duration::from_secs(30),
        )
        .await;

        let completed = drain(&mut session, Duration::from_secs(60)).await;
        assert_eq!(
            completed.code, expected,
            "signal {number} must reach the guest process and end it as 128+{number}: {completed:?}"
        );
    }

    let mut session = sandbox
        .exec(&["sh", "-c", "echo ready; sleep 300"], false, b"")
        .await;
    read_until(&mut session, "ready", Duration::from_secs(60)).await;
    // The frame is accepted; the refusal is the engine's answer to it.
    send(
        &mut session,
        ExecInput::Signal(999),
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(
        refusal(&mut session, Duration::from_secs(60)).await,
        "invalid_state",
        "a signal number outside the guest's map must be refused, not coerced"
    );

    sandbox.teardown().await;
}
