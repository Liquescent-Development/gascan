use std::path::PathBuf;

use gascan_apple::{AppleAttach, AttachInput, AttachOutput};

type TestError = Box<dyn std::error::Error + Send + Sync>;

fn fake_helper() -> AppleAttach {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fake-attach-helper/Cargo.toml");
    AppleAttach::new(env!("CARGO")).with_helper_args([
        "run".to_owned(),
        "--quiet".to_owned(),
        "--manifest-path".to_owned(),
        manifest.to_string_lossy().into_owned(),
    ])
}

#[tokio::test]
async fn fake_helper_proves_binary_streams_control_and_exact_exit() -> Result<(), TestError> {
    let mut session = fake_helper()
        .exec("container-id", ["guest", "literal arg"], false)
        .await?;
    session.send(AttachInput::Stdin(vec![0, 255])).await?;
    session.send(AttachInput::Close).await?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = loop {
        match session.recv().await? {
            Some(AttachOutput::Stdout(bytes)) => stdout.extend(bytes),
            Some(AttachOutput::Stderr(bytes)) => stderr.extend(bytes),
            Some(AttachOutput::Exit(code)) => break code,
            None => return Err("fake helper closed without terminal event".into()),
        }
    };
    assert_eq!(stdout, [0, 255]);
    assert_eq!(stderr, [254, 1]);
    assert_eq!(exit, 42);

    let mut missing = fake_helper().exec("exit-127", ["missing"], false).await?;
    assert_eq!(missing.exit().await?, 127);
    Ok(())
}

#[tokio::test]
async fn fake_helper_proves_resize_signal_and_protocol_validation() -> Result<(), TestError> {
    let mut session = fake_helper().exec("container-id", ["guest"], true).await?;
    session
        .send(AttachInput::Resize {
            rows: 41,
            cols: 113,
        })
        .await?;
    assert!(session.read_until(b"41 113").await?.is_some());
    session.send(AttachInput::Signal(15)).await?;
    assert_eq!(session.exit().await?, 42);

    let mut mismatch = fake_helper().exec("bad-version", ["guest"], false).await?;
    assert!(mismatch.exit().await.is_err());
    let mut absent = fake_helper().exec("no-terminal", ["guest"], false).await?;
    assert!(absent.exit().await.is_err());
    Ok(())
}

#[tokio::test]
async fn bridge_rejects_unportable_controls_before_writing() -> Result<(), TestError> {
    let session = fake_helper().exec("container-id", ["guest"], false).await?;
    assert!(
        session
            .send(AttachInput::Resize { rows: 1, cols: 1 })
            .await
            .is_err()
    );
    assert!(session.send(AttachInput::Signal(9)).await.is_err());
    Ok(())
}
