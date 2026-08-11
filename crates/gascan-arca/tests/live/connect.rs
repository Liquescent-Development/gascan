use crate::common::LiveEngine;
use gascan_arca::ChannelTransport;

/// START-HERE recorded every error path through `connect` as unverified,
/// because no socket was ever dialed. These are those paths.
///
/// The two connect-failure tests are deliberately *not* `#[ignore]`d, unlike
/// the rest of this tier: they need a `TempDir` and nothing else -- no engine,
/// no `GASCAN_ARCA_ENGINE_BIN` -- and they are the cheapest regression on the
/// error paths design §9 recorded as unverified. Claiming an engine
/// prerequisite they do not have would keep them behind `--ignored`, where an
/// ordinary CI run never reaches them.
#[tokio::test]
async fn connect_reports_a_missing_socket_by_naming_the_path() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent.sock");

    // `expect_err` would need `ChannelTransport: Debug`, which it does not
    // implement (`src/channel.rs:17` derives only `Clone`). Destructure
    // instead, so an unexpected success still fails loudly.
    let Err(error) = ChannelTransport::connect(missing.clone()).await else {
        panic!("connecting to a path with no socket must fail");
    };
    let rendered = error.to_string();

    assert!(
        rendered.contains(missing.to_str().unwrap()),
        "must name the path it dialed: {rendered}"
    );
    assert!(
        rendered.contains("No such file or directory"),
        "must carry the io cause through the source chain rather than the \
         opaque 'transport error': {rendered}"
    );
}

#[tokio::test]
async fn connect_distinguishes_a_path_that_is_not_a_socket() {
    let root = tempfile::tempdir().unwrap();
    let regular = root.path().join("not-a-socket");
    std::fs::write(&regular, b"regular file").unwrap();

    let Err(error) = ChannelTransport::connect(regular.clone()).await else {
        panic!("connecting to a regular file must fail");
    };

    // The negative assertion alone passes for an error that is nothing like
    // what we think it is, so pin the same positive property the missing-socket
    // case asserts: the rendered error names the path it dialed.
    assert!(
        error.to_string().contains(regular.to_str().unwrap()),
        "must name the path it dialed: {error}"
    );
    // The errno this test is named for, asserted positively. The `rust` CI job
    // runs on macOS (`macos-26`, .github/workflows/ci.yml:39), which is what
    // makes this string stable enough to assert.
    assert!(
        error.to_string().contains("Socket operation on non-socket"),
        "a present non-socket must report as ENOTSOCK: {error}"
    );
    assert!(
        !error.to_string().contains("No such file or directory"),
        "a present non-socket must not report as absent: {error}"
    );
}

/// The client dials with the placeholder authority `http://[::]:50051`, which
/// the connector ignores. Whether a real server accepts it was unverified.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn a_real_engine_accepts_the_placeholder_authority() {
    let engine = LiveEngine::start().await;
    let transport = engine.transport().await;

    let response = gascan_arca::EngineTransport::capabilities(
        &transport,
        gascan_engine_proto::v1::CapabilitiesRequest {},
    )
    .await
    .expect("a real engine must answer a request carrying the placeholder authority");

    assert!(
        response.outcome.is_some(),
        "the engine answered but set no outcome"
    );
}

/// An engine that dies under an open connection must surface as a transport
/// failure, not as a hang.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn a_call_against_a_killed_engine_fails_rather_than_hanging() {
    let engine = LiveEngine::start().await;
    let transport = engine.transport().await;
    engine.kill().await;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        gascan_arca::EngineTransport::capabilities(
            &transport,
            gascan_engine_proto::v1::CapabilitiesRequest {},
        ),
    )
    .await
    .expect("a call against a dead engine must not hang");

    assert!(
        result.is_err(),
        "a dead engine must not answer successfully"
    );
}
