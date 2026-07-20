use std::{error::Error, sync::Arc, time::Duration};

use camino::Utf8Path;
use gascan_core::{fake_runtime::FakeRuntime, manifest::Manifest, sandbox::SandboxSpec};
use gascan_proto::v1::{self, gas_can_server::GasCan};
use gascand::{ActivityTracker, NoopProvisioner, SandboxApi, SandboxService, Store, UpRequest};
use prost::Message;
use tokio_stream::StreamExt;

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn bridge_preserves_binary_streams_and_exact_exit() -> TestResult {
    let root = tempfile::tempdir()?;
    let root_path = Utf8Path::from_path(root.path()).ok_or("temporary path is not UTF-8")?;
    let spec = SandboxSpec::from_root("attach-bridge", root_path, Manifest::load(root_path)?)?;
    let id = spec.id().to_string();
    let runtime = FakeRuntime::default();
    runtime
        .set_exec_result(vec![0, 255], vec![254, 1], 42)
        .await;
    runtime
        .queue_exec_results([
            (Vec::new(), Vec::new(), 0),
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
        ])
        .await;
    let service = Arc::new(SandboxService::new(
        runtime,
        Store::open(root_path.join("state.db"))?,
        Arc::new(NoopProvisioner),
    ));
    service.up(UpRequest::new(spec)).await?;

    let api = SandboxApi::new(service, ActivityTracker::new());
    let mut events = api
        .run(tonic::Request::new(v1::RunRequest {
            sandbox: Some(v1::SandboxSelector { sandbox_id: id }),
            command: Some(v1::CommandPayload {
                argv: vec![b"configured-result".to_vec()],
                environment: Vec::new(),
                tty: false,
            }),
        }))
        .await?
        .into_inner();
    let token = events
        .next()
        .await
        .ok_or("session token missing")??
        .session_token;
    let close = v1::ClientFrame {
        frame: Some(v1::client_frame::Frame::Close(v1::Close {})),
        session_token: token,
    };
    let mut encoded = Vec::with_capacity(close.encoded_len() + 5);
    encoded.push(0);
    encoded.extend_from_slice(&u32::try_from(close.encoded_len())?.to_be_bytes());
    close.encode(&mut encoded)?;
    let input = tonic::Streaming::new_request(
        tonic::codec::ProstCodec::<v1::ServerFrame, v1::ClientFrame>::raw_decoder(
            tonic::codec::BufferSettings::default(),
        ),
        http_body_util::Full::new(prost::bytes::Bytes::from(encoded)),
        None,
        None,
    );
    let mut attached = api.attach(tonic::Request::new(input)).await?.into_inner();

    let stdout = tokio::time::timeout(Duration::from_secs(2), attached.next())
        .await?
        .ok_or("stdout missing")??;
    assert_eq!(
        stdout.frame,
        Some(v1::server_frame::Frame::Stdout(vec![0, 255]))
    );
    let stderr = attached.next().await.ok_or("stderr missing")??;
    assert_eq!(
        stderr.frame,
        Some(v1::server_frame::Frame::Stderr(vec![254, 1]))
    );
    let exit = attached.next().await.ok_or("exit missing")??;
    assert_eq!(
        exit.frame,
        Some(v1::server_frame::Frame::Exit(v1::Exit {
            code: 42,
            signal: 0
        }))
    );
    assert!(attached.next().await.is_none());
    Ok(())
}
