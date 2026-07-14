use std::{fs, os::unix::fs::PermissionsExt};

use gascan_apple::{AppleAttach, AttachInput, AttachOutput};

type TestError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::test]
async fn non_tty_attachment_preserves_argv_binary_streams_and_exit() -> Result<(), TestError> {
    let directory = tempfile::tempdir()?;
    let client = directory.path().join("fake-container");
    fs::write(
        &client,
        r#"#!/bin/sh
test "$1" = exec || exit 91
test "$2" = -i || exit 92
test "$3" = container-id || exit 93
test "$4" = guest-program || exit 94
test "$5" = "literal arg" || exit 95
dd bs=2 count=1 2>/dev/null
printf '\376\001' >&2
exit 42
"#,
    )?;
    let mut permissions = fs::metadata(&client)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&client, permissions)?;

    let mut session = AppleAttach::new(client.to_string_lossy())
        .exec("container-id", ["guest-program", "literal arg"], false)
        .await?;
    session.send(AttachInput::Stdin(vec![0, 255])).await?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = loop {
        match session.recv().await? {
            Some(AttachOutput::Stdout(bytes)) => stdout.extend(bytes),
            Some(AttachOutput::Stderr(bytes)) => stderr.extend(bytes),
            Some(AttachOutput::Exit(code)) => break code,
            None => return Err("fake client closed without an exit event".into()),
        }
    };
    assert_eq!(stdout, [0, 255]);
    assert_eq!(stderr, [254, 1]);
    assert_eq!(exit, 42);
    Ok(())
}

#[tokio::test]
async fn attachment_rejects_resize_without_tty_and_unportable_signals() -> Result<(), TestError> {
    let mut session = AppleAttach::new("/usr/bin/true")
        .exec("container-id", ["guest-program"], false)
        .await?;
    assert!(
        session
            .send(AttachInput::Resize { rows: 1, cols: 1 })
            .await
            .is_err()
    );
    assert!(session.send(AttachInput::Signal(9)).await.is_err());
    assert_eq!(session.exit().await?, 0);
    Ok(())
}
