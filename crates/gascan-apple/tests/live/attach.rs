use gascan_apple::{AttachInput, AttachOutput};

use super::common::{LiveContext, TestError};

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn attached_process_reports_resize_signal_and_exit() -> Result<(), TestError> {
    let ctx = LiveContext::new("attach-tty").await?;
    let mut session = ctx
        .attach(
            [
                "sh",
                "-c",
                "trap 'stty size' WINCH; trap 'exit 42' TERM; stty size; sleep 30",
            ],
            true,
        )
        .await?;
    session
        .send(AttachInput::Resize {
            rows: 41,
            cols: 113,
        })
        .await?;
    assert!(session.read_until(b"41 113").await?.is_some());
    session
        .send(AttachInput::Signal(nix::libc::SIGTERM))
        .await?;
    assert_eq!(session.exit().await?, 42);
    ctx.cleanup().await
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn attach_preserves_binary_streams_and_exact_exit_codes() -> Result<(), TestError> {
    let ctx = LiveContext::new("attach-pipes").await?;
    let mut session = ctx
        .attach(
            [
                "sh",
                "-c",
                "printf '\\000\\377'; printf '\\376\\001' >&2; exit 42",
            ],
            false,
        )
        .await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = loop {
        match session.recv().await? {
            Some(AttachOutput::Stdout(bytes)) => stdout.extend(bytes),
            Some(AttachOutput::Stderr(bytes)) => stderr.extend(bytes),
            Some(AttachOutput::Exit(code)) => break code,
            None => return Err("attachment ended without exact exit".into()),
        }
    };
    assert_eq!(stdout, [0, 255]);
    assert_eq!(stderr, [254, 1]);
    assert_eq!(exit, 42);

    let mut missing = ctx.attach(["gascan-command-does-not-exist"], false).await?;
    assert_eq!(missing.exit().await?, 127);
    ctx.cleanup().await
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn attached_process_forwards_sigint_and_closes_stdin() -> Result<(), TestError> {
    let ctx = LiveContext::new("attach-input").await?;
    let mut signal = ctx
        .attach(
            ["sh", "-c", "trap 'exit 42' INT; while :; do sleep 1; done"],
            true,
        )
        .await?;
    signal.send(AttachInput::Signal(nix::libc::SIGINT)).await?;
    assert_eq!(signal.exit().await?, 42);

    let mut close = ctx
        .attach(["sh", "-c", "cat >/dev/null; exit 0"], false)
        .await?;
    close.send(AttachInput::Close).await?;
    assert_eq!(close.exit().await?, 0);
    ctx.cleanup().await
}
