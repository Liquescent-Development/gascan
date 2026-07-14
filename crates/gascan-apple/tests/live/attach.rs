use gascan_apple::{AttachInput, AttachOutput};
use std::time::Duration;

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
    session.send(AttachInput::Signal(libc::SIGTERM)).await?;
    assert_eq!(session.exit().await?, 42);
    ctx.cleanup().await
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn attach_preserves_binary_streams_and_exact_exit_codes() -> Result<(), TestError> {
    eprintln!("attach diagnostic: creating live context");
    let ctx = LiveContext::new("attach-pipes").await?;
    eprintln!("attach diagnostic: starting non-TTY guest process");
    let mut session = tokio::time::timeout(
        Duration::from_secs(15),
        ctx.attach(
            [
                "sh",
                "-c",
                "printf '\\000\\377'; printf '\\376\\001' >&2; exit 42",
            ],
            false,
        ),
    )
    .await
    .map_err(|_| "timed out starting helper session")??;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), session.recv())
            .await
            .map_err(|_| {
                format!("timed out waiting for helper event; stdout={stdout:?}, stderr={stderr:?}")
            })??;
        match event {
            Some(AttachOutput::Stdout(bytes)) => {
                eprintln!("attach diagnostic: stdout {} byte(s)", bytes.len());
                stdout.extend(bytes);
            }
            Some(AttachOutput::Stderr(bytes)) => {
                eprintln!("attach diagnostic: stderr {} byte(s)", bytes.len());
                stderr.extend(bytes);
            }
            Some(AttachOutput::Exit(code)) => {
                eprintln!("attach diagnostic: exit {code}");
                break code;
            }
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
            [
                "sh",
                "-c",
                "trap 'exit 42' INT; printf 'signal-ready\\n'; while :; do :; done",
            ],
            true,
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), signal.read_until(b"signal-ready"))
        .await
        .map_err(|_| "timed out waiting for guest signal readiness")??
        .ok_or("guest exited before signal readiness")?;
    eprintln!("attach diagnostic: guest signal trap is ready");
    signal.send(AttachInput::Signal(libc::SIGINT)).await?;
    let signal_exit = tokio::time::timeout(Duration::from_secs(10), signal.exit())
        .await
        .map_err(|_| "timed out waiting for SIGINT guest exit")??;
    assert_eq!(signal_exit, 42);

    let mut close = ctx
        .attach(["sh", "-c", "cat >/dev/null; exit 0"], false)
        .await?;
    close.send(AttachInput::Close).await?;
    let close_exit = tokio::time::timeout(Duration::from_secs(10), close.exit())
        .await
        .map_err(|_| "timed out waiting for stdin-close guest exit")??;
    assert_eq!(close_exit, 0);
    ctx.cleanup().await
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn client_process_kill_term_reaches_single_target() -> Result<(), TestError> {
    let ctx = LiveContext::new("attach-term").await?;
    let mut session = ctx
        .attach(
            [
                "sh",
                "-c",
                "trap 'exit 42' TERM; printf 'term-ready\\n'; while :; do :; done",
            ],
            false,
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), session.read_until(b"term-ready"))
        .await
        .map_err(|_| "timed out waiting for SIGTERM guest readiness")??
        .ok_or("guest exited before SIGTERM readiness")?;
    session.send(AttachInput::Signal(libc::SIGTERM)).await?;
    let exit = tokio::time::timeout(Duration::from_secs(10), session.exit())
        .await
        .map_err(|_| "timed out waiting for SIGTERM guest exit")??;
    assert_eq!(exit, 42);
    ctx.cleanup().await
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn tty_ctrl_c_byte_reaches_foreground_process_group() -> Result<(), TestError> {
    let ctx = LiveContext::new("attach-ctrl-c").await?;
    let mut session = ctx
        .attach(
            [
                "sh",
                "-c",
                "trap 'exit 42' INT; printf 'ctrl-c-ready\\n'; while :; do :; done",
            ],
            true,
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), session.read_until(b"ctrl-c-ready"))
        .await
        .map_err(|_| "timed out waiting for Ctrl-C guest readiness")??
        .ok_or("guest exited before Ctrl-C readiness")?;
    session.send(AttachInput::Stdin(vec![0x03])).await?;
    let exit = tokio::time::timeout(Duration::from_secs(10), session.exit())
        .await
        .map_err(|_| "timed out waiting for Ctrl-C guest exit")??;
    assert_eq!(exit, 42);
    ctx.cleanup().await
}
