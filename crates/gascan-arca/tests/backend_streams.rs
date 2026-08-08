mod fake_transport;

use fake_transport::{Call, FakeEngine};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::RuntimeBackend;
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
