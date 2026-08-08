mod fake_transport;

use fake_transport::{Call, FakeEngine};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ExecInput, ExecOutput, ExecRequest, RuntimeBackend};
use gascan_core::sandbox::SandboxId;
use gascan_engine_proto::v1;

fn data(bytes: &[u8]) -> Result<v1::LogsChunk, gascan_arca::TransportError> {
    Ok(v1::LogsChunk {
        outcome: Some(v1::logs_chunk::Outcome::Data(bytes.to_vec())),
    })
}

#[tokio::test]
async fn logs_concatenate_every_chunk_in_order() {
    let engine = FakeEngine::default();
    *engine.logs_chunks.lock().expect("test lock") =
        vec![data(b"first "), data(b"second "), data(b"third")];

    let id = SandboxId::test("logging");
    let backend = ArcaBackend::new(engine);
    let logs = backend.logs(&id, Some(1_234)).await.expect("three chunks");

    assert_eq!(logs, b"first second third");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::Logs(v1::LogsRequest {
            sandbox_id: id.as_str().to_owned(),
            since_unix_millis: Some(1_234),
        })],
        "since_millis passes through, and absent means from the beginning",
    );
}

#[tokio::test]
async fn a_mid_stream_error_discards_the_partial_buffer() {
    let engine = FakeEngine::default();
    *engine.logs_chunks.lock().expect("test lock") = vec![
        data(b"this much arrived"),
        Ok(v1::LogsChunk {
            outcome: Some(v1::logs_chunk::Outcome::Error(FakeEngine::engine_error(
                "not_found",
            ))),
        }),
        data(b"and this never should"),
    ];

    let error = ArcaBackend::new(engine)
        .logs(&SandboxId::test("logging"), None)
        .await
        .expect_err("a broken log is a failure, not a short read");
    assert_eq!(
        error.code(),
        "not_found",
        "the engine's own code survives, which a hardcoded variant could not fake",
    );
}

#[tokio::test]
async fn a_transport_fault_mid_stream_is_not_a_short_read() {
    let engine = FakeEngine::default();
    *engine.logs_chunks.lock().expect("test lock") = vec![
        data(b"this much arrived"),
        Err(gascan_arca::TransportError::rpc("logs", "the stream broke")),
    ];

    let error = ArcaBackend::new(engine)
        .logs(&SandboxId::test("logging"), None)
        .await
        .expect_err("a broken transport is a failure, not a short read");
    assert_eq!(
        error.code(),
        "command_io",
        "a transport fault is I/O against the engine, and stays distinct from the \
         engine-error chunk the test above covers",
    );
}

#[tokio::test]
async fn a_chunk_carrying_no_outcome_is_refused() {
    let engine = FakeEngine::default();
    *engine.logs_chunks.lock().expect("test lock") = vec![
        data(b"this much arrived"),
        Ok(v1::LogsChunk { outcome: None }),
    ];

    let error = ArcaBackend::new(engine)
        .logs(&SandboxId::test("logging"), None)
        .await
        .expect_err("an unset oneof is not a chunk");
    assert_eq!(error.code(), "invalid_output");
    let rendered = error.to_string();
    assert_eq!(
        rendered, "invalid output from logs: response carried no outcome",
        "the refusal names the RPC it came from, so a misrouted operation label shows",
    );
}

#[tokio::test]
async fn an_empty_log_is_empty_rather_than_an_error() {
    let engine = FakeEngine::default();
    let id = SandboxId::test("logging");
    let backend = ArcaBackend::new(engine);
    assert!(
        backend
            .logs(&id, None)
            .await
            .expect("no chunks is a valid empty log")
            .is_empty(),
    );
    assert_eq!(
        backend.into_transport().calls(),
        [Call::Logs(v1::LogsRequest {
            sandbox_id: id.as_str().to_owned(),
            since_unix_millis: None,
        })],
        "an absent since_millis stays absent rather than becoming a floor",
    );
}

fn server_frame(
    frame: v1::exec_server_frame::Frame,
) -> Result<v1::ExecServerFrame, gascan_arca::TransportError> {
    Ok(v1::ExecServerFrame { frame: Some(frame) })
}

type SentFrames = std::sync::Arc<std::sync::Mutex<Vec<v1::ExecClientFrame>>>;
type StreamClosed = std::sync::Arc<std::sync::atomic::AtomicBool>;

/// How long a bounded poll waits before it gives up and lets the assertion speak.
const POLLS: usize = 200;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Waits until the fake has captured at least `count` client frames.
///
/// A bounded poll on the condition, never a fixed sleep: the frames cross a
/// channel into the fake's capture task, so a wall-clock wait either flakes
/// under load or pays its worst case on every run. It returns whatever was
/// captured when the bound runs out rather than panicking, so a shortfall is
/// reported by the caller's assertion, with the frames it did get.
async fn captured(sent: &SentFrames, count: usize) -> Vec<v1::ExecClientFrame> {
    for _ in 0..POLLS {
        let frames = sent.lock().expect("test lock").clone();
        if frames.len() >= count {
            return frames;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    sent.lock().expect("test lock").clone()
}

/// Waits for the client→engine stream to close, then returns every captured frame.
///
/// Closure means the pump has ended and the capture task has drained it, so the
/// list is final. That is what an assertion of the form "and nothing else was
/// sent" needs; [`captured`] cannot supply it, because a frame count that has
/// been reached says nothing about a frame still in flight.
async fn settled(closed: &StreamClosed, sent: &SentFrames) -> Vec<v1::ExecClientFrame> {
    for _ in 0..POLLS {
        if closed.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    sent.lock().expect("test lock").clone()
}

#[tokio::test]
async fn exec_opens_with_a_start_frame_and_reports_stdout_then_exit() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") = vec![
        server_frame(v1::exec_server_frame::Frame::Stdout(b"hello".to_vec())),
        server_frame(v1::exec_server_frame::Frame::Exit(v1::Exit {
            code: 0,
            signal: 0,
        })),
    ];
    let sent = std::sync::Arc::clone(&engine.exec_sent);
    let closed = std::sync::Arc::clone(&engine.exec_client_stream_closed);

    let id = SandboxId::test("execing");
    let mut session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(id.clone(), ["/bin/true"]))
        .await
        .expect("the session opens");

    assert_eq!(
        session.next().await.expect("a frame").expect("stdout"),
        ExecOutput::Stdout(b"hello".to_vec()),
    );
    assert_eq!(
        session.next().await.expect("a frame").expect("exit"),
        ExecOutput::Exit { code: 0, signal: 0 },
    );

    let frames = settled(&closed, &sent).await;
    assert!(
        matches!(
            frames.first().and_then(|frame| frame.frame.as_ref()),
            Some(v1::exec_client_frame::Frame::Start(start)) if start.sandbox_id == id.as_str()
        ),
        "the first frame must be the one ExecStart: {frames:?}",
    );
    assert_eq!(
        frames.len(),
        1,
        "an empty stdin buffer sends no stdin frame: {frames:?}"
    );
}

#[tokio::test]
async fn a_non_empty_stdin_buffer_is_sent_once_and_no_close_is_forged() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") =
        vec![server_frame(v1::exec_server_frame::Frame::Exit(v1::Exit {
            code: 0,
            signal: 0,
        }))];
    let sent = std::sync::Arc::clone(&engine.exec_sent);
    let closed = std::sync::Arc::clone(&engine.exec_client_stream_closed);

    let mut request = ExecRequest::fixture(SandboxId::test("execing"), ["/bin/cat"]);
    request.stdin = b"piped".to_vec();

    let mut session = ArcaBackend::new(engine).exec(request).await.expect("opens");
    session.next().await.expect("a frame").expect("exit");

    let frames = settled(&closed, &sent).await;
    let stdin: Vec<_> = frames
        .iter()
        .filter_map(|frame| match frame.frame.as_ref() {
            Some(v1::exec_client_frame::Frame::Stdin(bytes)) => Some(bytes.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        stdin,
        [b"piped".to_vec()],
        "the initial buffer is sent exactly once"
    );
    assert!(
        !frames.iter().any(|frame| matches!(
            frame.frame.as_ref(),
            Some(v1::exec_client_frame::Frame::Close(_))
        )),
        "Close is the consumer's to send: {frames:?}",
    );
}

#[tokio::test]
async fn live_input_reaches_the_engine_as_its_own_frame() {
    // No server frames, deliberately. A terminal frame scripted here would be
    // buffered and ready before the pump's first poll, and the pump's select
    // chooses among its ready branches at random, so the exit would race the
    // four inputs and truncate this assertion at an arbitrary prefix. What this
    // test is about is the client half; the engine has nothing to say on it.
    let engine = FakeEngine::default();
    let sent = std::sync::Arc::clone(&engine.exec_sent);

    let session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(
            SandboxId::test("execing"),
            ["/bin/sh"],
        ))
        .await
        .expect("opens");

    session
        .send(ExecInput::Stdin(b"typed".to_vec()))
        .await
        .expect("stdin");
    session
        .send(ExecInput::Resize {
            columns: 120,
            rows: 40,
        })
        .await
        .expect("resize");
    session.send(ExecInput::Signal(2)).await.expect("signal");
    session.send(ExecInput::Close).await.expect("close");

    let frames = captured(&sent, 5).await;
    let shapes: Vec<&str> = frames
        .iter()
        .map(|frame| match frame.frame.as_ref() {
            Some(v1::exec_client_frame::Frame::Start(_)) => "start",
            Some(v1::exec_client_frame::Frame::Stdin(_)) => "stdin",
            Some(v1::exec_client_frame::Frame::Resize(_)) => "resize",
            Some(v1::exec_client_frame::Frame::Signal(_)) => "signal",
            Some(v1::exec_client_frame::Frame::Close(_)) => "close",
            None => "unset",
        })
        .collect();
    assert_eq!(shapes, ["start", "stdin", "resize", "signal", "close"]);
}

#[tokio::test]
async fn a_server_error_frame_is_terminal_and_carries_its_code() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") = vec![
        server_frame(v1::exec_server_frame::Frame::Stderr(b"before".to_vec())),
        server_frame(v1::exec_server_frame::Frame::Error(
            FakeEngine::engine_error("invalid_state"),
        )),
        server_frame(v1::exec_server_frame::Frame::Stdout(b"never".to_vec())),
    ];

    let mut session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(
            SandboxId::test("execing"),
            ["/bin/false"],
        ))
        .await
        .expect("opens");

    assert_eq!(
        session.next().await.expect("a frame").expect("stderr"),
        ExecOutput::Stderr(b"before".to_vec()),
    );
    let error = session
        .next()
        .await
        .expect("a frame")
        .expect_err("the error frame");
    assert_eq!(error.code(), "invalid_state");
    assert!(
        session.next().await.is_none(),
        "an error frame ends the session; nothing after it is delivered",
    );
}

#[tokio::test]
async fn dropping_the_session_cancels_the_pump_and_closes_the_stream() {
    // The end-to-end guarantee: an abandoned session must not leave the stream to
    // the engine open, or a dropped exec leaves guest work running with nothing to
    // reap it. This pins the outcome and stays deliberately silent about which
    // mechanism delivers it — dropping the session both cancels and closes the
    // consumer's input channel, and either alone ends the pump. Isolating the
    // cancellation wiring therefore needs a session that is *not* dropped, which
    // is `cancelling_a_held_session_closes_the_stream_to_the_engine` below.
    let engine = FakeEngine::default();
    // No server frames: the engine holds its half open with nothing to say, so
    // the pump has nothing to deliver and sits in its select.
    let closed = std::sync::Arc::clone(&engine.exec_client_stream_closed);

    let session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(
            SandboxId::test("execing"),
            ["/bin/sleep"],
        ))
        .await
        .expect("opens");
    assert!(
        !closed.load(std::sync::atomic::Ordering::SeqCst),
        "the stream is live while the session is held",
    );

    drop(session);

    // A bounded poll on the condition, not a fixed sleep. `autostart.rs` already
    // has a test that fails open because it waited on a wall clock instead of the
    // thing it cared about; do not repeat that here.
    let mut observed = false;
    for _ in 0..200 {
        if closed.load(std::sync::atomic::Ordering::SeqCst) {
            observed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        observed,
        "dropping the session must cancel the pump and close the stream to the engine, \
         or a dropped exec leaves guest work running with nothing to reap it",
    );
}

#[tokio::test]
async fn cancelling_a_held_session_closes_the_stream_to_the_engine() {
    // The reason `ExecSession::live_cancellable` exists, and the only test here
    // that can tell. The session is held for the whole test, so its input channel
    // stays open; the engine holds its half open with nothing to say, so the
    // server stream stays open too. Both of the pump's other exits are therefore
    // unavailable and cancellation is the only thing that can end it. Hand the
    // pump a session built by `ExecSession::live` instead and this is the one
    // assertion that fails.
    let engine = FakeEngine::default();
    let closed = std::sync::Arc::clone(&engine.exec_client_stream_closed);

    let session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(
            SandboxId::test("execing"),
            ["/bin/sleep"],
        ))
        .await
        .expect("opens");
    assert!(
        !closed.load(std::sync::atomic::Ordering::SeqCst),
        "the stream is live until something cancels it",
    );

    session.cancel();

    let mut observed = false;
    for _ in 0..POLLS {
        if closed.load(std::sync::atomic::Ordering::SeqCst) {
            observed = true;
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        observed,
        "cancelling must reach the pump and release the engine stream, with the \
         session still held and neither of the pump's other exits available",
    );
    drop(session);
}
