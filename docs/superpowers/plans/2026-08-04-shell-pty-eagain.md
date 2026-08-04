# Shell PTY EAGAIN Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep `gascan shell` attached through large command output without leaking `O_NONBLOCK` from terminal input onto stdout or stderr.

**Architecture:** Interactive input opens the controlling terminal through a new `/dev/tty` open file description before registering it with Tokio. Host output uses a descriptor-level async writer that preserves existing flags and waits for writability when an already-nonblocking descriptor returns `EAGAIN`; a real PTY process test covers the complete CLI attachment path.

**Tech Stack:** Rust 2024, Tokio `AsyncFd`, rustix termios/fs/io APIs, rustix-openpty, Tonic attach streaming, Cargo tests.

## Global Constraints

- Do not change the public CLI, daemon wire schema, or `gascan ssh` behavior.
- Do not set or clear file-status flags on stdout or stderr.
- Interactive input must use a separately opened controlling-terminal file description.
- Temporary output backpressure must wait for readiness without spinning, truncating, or duplicating bytes.
- Non-`EAGAIN` I/O errors remain fatal and terminal state remains recoverable.
- Production changes must follow red-green-refactor TDD.

---

### Task 1: Reproduce the full PTY failure and isolate interactive input

**Files:**
- Modify: `crates/gascan-core/src/fake_runtime.rs:590-635, 1040-1080`
- Modify: `crates/gascan-e2e/tests/fake_backend.rs:250-340, 1080-1140`
- Modify: `crates/gascan/src/guest.rs:409-449, 620-645, 686-1035`

**Interfaces:**
- Consumes: fake command argv `fake-large-ready-then-drain`, rustix `ttyname` and `open`, the existing `RawTerminal` restoration guard.
- Produces: `CancellableInput::terminal() -> std::io::Result<Self>` and test-only `CancellableInput::from_terminal_path(&Path) -> std::io::Result<Self>`.

- [ ] **Step 1: Add the fake-runtime fixture and failing real-PTY regression**

Extend the existing early-output branch in `fake_runtime.rs`: `fake-ready-then-drain` keeps sending `b"ready"`; `fake-large-ready-then-drain` sends exactly `vec![b'x'; 1024 * 1024]`; both then retain the existing wait-for-input and signal/exit behavior. This is test-fixture setup, not the product fix.

Add a test beside `real_pty_resize_signals_and_terminal_restoration_are_exact`. Give stdin, stdout, and stderr duplicates of the same PTY slave, leave the controller unread for 250 ms, and assert the CLI remains attached:

```rust
#[test]
fn real_pty_large_output_waits_for_capacity_without_exiting() -> TestResult {
    use rustix_openpty::rustix;
    let _signal_guard = signal_test_guard()?;
    let env = Environment::new()?;
    assert!(env.invoke(&["up", env.root()?])?.status.success());
    let pty = rustix_openpty::openpty(None, None)?;
    let saved = normalized_termios(&pty.user)?;
    let stdin = std::fs::File::from(rustix::io::dup(&pty.user)?);
    let stdout = std::fs::File::from(rustix::io::dup(&pty.user)?);
    let stderr = std::fs::File::from(rustix::io::dup(&pty.user)?);
    let mut child = env
        .command(&["shell", "--", "fake-large-ready-then-drain"])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;

    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(child.try_wait()?.is_none(), "large output detached the PTY shell");

    let mut controller = std::fs::File::from(pty.controller);
    let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        use std::io::Read as _;
        let mut output = Vec::new();
        let mut chunk = [0_u8; 16 * 1024];
        while output.iter().filter(|byte| **byte == b'x').count() < 1024 * 1024 {
            match controller.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&chunk[..count]),
                Err(error) if error.raw_os_error() == Some(5) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(output)
    });
    let output = reader.join().map_err(|_| "PTY reader panicked")??;
    assert!(output.iter().filter(|byte| **byte == b'x').count() >= 1024 * 1024);

    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id())?)
        .ok_or("invalid child pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    assert_eq!(child.wait()?.code(), Some(143));
    assert_termios_restored(&rustix::termios::tcgetattr(&pty.user)?, &saved);
    Ok(())
}
```

- [ ] **Step 2: Run the process regression and verify RED**

Run outside restrictive process sandboxing:

```bash
cargo test -p gascan-e2e --test fake_backend real_pty_large_output_waits_for_capacity_without_exiting -- --nocapture
```

Expected: the current CLI exits during the unread interval with `Resource temporarily unavailable (os error 35)`, so `try_wait()` returns a status and the assertion fails.

- [ ] **Step 3: Add the focused input flag test and verify RED**

In `guest.rs` tests, obtain a real PTY slave path and check a stdout duplicate while the wished-for terminal input is alive:

```rust
#[tokio::test]
async fn interactive_input_does_not_make_shared_pty_output_nonblocking() -> TestResult {
    use std::os::unix::ffi::OsStrExt as _;
    let pty = rustix_openpty::openpty(None, None)?;
    let name = rustix::termios::ttyname(&pty.user, Vec::new())?;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(name.to_bytes()));
    let output = rustix::io::dup(&pty.user)?;
    let before = rustix::fs::fcntl_getfl(&output)?;

    let _input = CancellableInput::from_terminal_path(path)?;

    assert_eq!(rustix::fs::fcntl_getfl(&output)?, before);
    assert!(!before.contains(rustix::fs::OFlags::NONBLOCK));
    Ok(())
}
```

Run:

```bash
cargo test -p gascan --lib interactive_input_does_not_make_shared_pty_output_nonblocking -- --nocapture
```

Expected: compilation fails because `from_terminal_path` does not exist.

- [ ] **Step 4: Implement independently opened terminal input**

Add `std::path::Path` and implement:

```rust
impl CancellableInput {
    fn terminal() -> std::io::Result<Self> {
        Self::from_terminal_path(Path::new("/dev/tty"))
    }

    fn from_terminal_path(path: &Path) -> std::io::Result<Self> {
        let flags = rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC;
        let fd = rustix::fs::open(path, flags, rustix::fs::Mode::empty())?;
        let original_flags = rustix::fs::fcntl_getfl(&fd)?;
        let fd = RestoringFd { fd, original_flags };
        Ok(Self {
            fd: tokio::io::unix::AsyncFd::new(fd)?,
        })
    }
}
```

Change `forward_terminal_input` to accept `stdin: CancellableInput` instead of constructing it internally. Construct it with `CancellableInput::terminal()?` in `attach_to_stdio` before building the producer future, then move it into `forward_terminal_input`. Keep `HostInput::stdin()` on the duplicate-and-restore path for noninteractive pipes. This makes `/dev/tty` open or registration failures return as concrete CLI I/O errors before the attach RPC instead of silently sending close.

- [ ] **Step 5: Verify GREEN and commit**

Run:

```bash
cargo test -p gascan --lib interactive_input_does_not_make_shared_pty_output_nonblocking -- --nocapture
cargo test -p gascan --lib cancelling_scoped_input_restores_flags_and_leaves_pty_bytes_unclaimed -- --nocapture
cargo test -p gascan-e2e --test fake_backend real_pty_large_output_waits_for_capacity_without_exiting -- --nocapture
```

Expected: all pass, the full 1 MiB output arrives exactly once, and terminal modes restore after termination.

```bash
git add crates/gascan/src/guest.rs crates/gascan-core/src/fake_runtime.rs crates/gascan-e2e/tests/fake_backend.rs
git commit -S -m "fix: isolate shell terminal input"
```

### Task 2: Retry temporary host-output backpressure

**Files:**
- Modify: `crates/gascan/src/guest.rs:499-565, 686-1035`

**Interfaces:**
- Consumes: an existing descriptor implementing `AsFd` and an immutable byte slice.
- Produces: `write_host_output(fd: impl AsFd, bytes: &[u8]) -> std::io::Result<()>`, which preserves descriptor flags and writes every byte once.

- [ ] **Step 1: Write the failing nonblocking-pipe test**

Create a real pipe, set only its writer nonblocking, fill it until `EAGAIN`, start the wished-for writer, prove it remains pending under backpressure, drain the pipe, and verify the payload appears exactly once:

```rust
#[tokio::test]
async fn host_output_waits_for_nonblocking_capacity_without_losing_bytes() -> TestResult {
    let (reader, writer) = rustix::pipe::pipe()?;
    let flags = rustix::fs::fcntl_getfl(&writer)?;
    rustix::fs::fcntl_setfl(&writer, flags | rustix::fs::OFlags::NONBLOCK)?;
    let fill = vec![b'f'; 4096];
    let mut filled = 0usize;
    loop {
        match rustix::io::write(&writer, &fill) {
            Ok(count) => filled += count,
            Err(rustix::io::Errno::AGAIN) => break,
            Err(error) => return Err(error.into()),
        }
    }
    let expected = vec![b'x'; 8192];
    let task_writer = rustix::io::dup(&writer)?;
    let task_payload = expected.clone();
    let task = tokio::spawn(async move {
        write_host_output(task_writer, &task_payload).await
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());

    let received = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut received = vec![0; filled + expected.len()];
        let mut offset = 0;
        while offset < received.len() {
            let count = rustix::io::read(&reader, &mut received[offset..])?;
            if count == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            offset += count;
        }
        Ok(received)
    });
    task.await??;
    let received = received.await??;
    assert!(received[filled..].iter().all(|byte| *byte == b'x'));
    Ok(())
}
```

- [ ] **Step 2: Run the test and verify RED**

```bash
cargo test -p gascan --lib host_output_waits_for_nonblocking_capacity_without_losing_bytes -- --nocapture
```

Expected: compilation fails because `write_host_output` does not exist.

- [ ] **Step 3: Implement the readiness-aware writer**

Implement a raw-descriptor write loop. Retry `Interrupted`. When a write returns `WouldBlock`, register the unchanged duplicate with `tokio::io::unix::AsyncFd`, wait for writability, call `try_io`, and resume at the current offset. Treat a zero-byte write as `WriteZero`; convert other rustix errors to `std::io::Error`.

```rust
async fn write_host_output(
    fd: impl std::os::fd::AsFd,
    bytes: &[u8],
) -> std::io::Result<()> {
    let fd = rustix::io::dup(fd)?;
    let mut offset = 0;
    while offset < bytes.len() {
        match rustix::io::write(&fd, &bytes[offset..]).map_err(std::io::Error::from) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    if offset == bytes.len() {
        return Ok(());
    }
    let fd = tokio::io::unix::AsyncFd::new(fd)?;
    while offset < bytes.len() {
        let mut writable = fd.writable().await?;
        match writable.try_io(|fd| {
            rustix::io::write(fd.get_ref(), &bytes[offset..]).map_err(std::io::Error::from)
        }) {
            Ok(Ok(0)) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(Ok(count)) => offset += count,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Ok(Err(error)) => return Err(error),
            Err(_) => continue,
        }
    }
    Ok(())
}
```

Replace both `write_all`/`flush` branches in `attach_to_stdio` with awaited calls to `write_host_output(std::io::stdout(), &bytes)` and `write_host_output(std::io::stderr(), &bytes)`. Do not mutate output flags.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test -p gascan --lib host_output_waits_for_nonblocking_capacity_without_losing_bytes -- --nocapture
cargo test -p gascan --lib --locked
cargo clippy -p gascan --all-targets -- -D warnings
git add crates/gascan/src/guest.rs
git commit -S -m "fix: wait for shell output capacity"
```

### Task 3: Verify, review, merge, and release 0.1.20

**Files:**
- Modify after feature merge: the six release `crates/*/Cargo.toml` files listed in `docs/release/releasing.md`
- Modify after feature merge: `Cargo.lock`
- Modify after feature merge: `README.md`
- Modify after feature merge: `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: reviewed feature branch, release runbook, signing identities, notarization profile, and Homebrew tap.
- Produces: merged fix PR, merged 0.1.20 bump PR, signed `v0.1.20`, notarized package, GitHub release, and Homebrew cask.

- [ ] **Step 1: Run exact-tree verification**

```bash
cargo fmt --all -- --check
git diff --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo test --manifest-path scripts/Cargo.toml --all-targets --locked
for c in tests/release/*-contract.sh; do bash "$c" >/dev/null; done
```

Run process-inspection tests outside restrictive sandboxing. Build the Swift helper and run `packaging/macos/release-smoke.sh` with the feature binaries before claiming integration success.

- [ ] **Step 2: Review and merge the feature PR**

Require independent review with no Critical or Important findings. Push `fix/shell-pty-eagain`, create a PR against `main`, confirm the exact reviewed head is mergeable, and squash-merge it.

- [ ] **Step 3: Prepare and verify the 0.1.20 release bump**

From updated `origin/main`, create `release/0.1.20`. Change only the nine files prescribed by `docs/release/releasing.md`, run `cargo update --workspace --offline`, and commit the signed bump before release contracts:

```bash
cargo metadata --locked --no-deps --format-version 1 \
  | jq -er '.packages[] | select(.name == "gascan") | .version == "0.1.20"'
cargo check --locked --workspace --all-targets
for c in tests/release/*-contract.sh; do bash "$c" >/dev/null; done
```

- [ ] **Step 4: Review and merge the release PR**

Require independent confirmation that only the six release crate versions, their six `Cargo.lock` entries, README references, and checklist references changed. Push, create the 0.1.20 PR, and squash-merge it.

- [ ] **Step 5: Sign, publish, verify, and clean up**

On clean updated `main`:

```bash
git tag -s v0.1.20 -m 'Gas Can 0.1.20'
git verify-tag v0.1.20
git push origin v0.1.20
./packaging/macos/release.sh 0.1.20 --check
./packaging/macos/release.sh 0.1.20
brew update
brew info --cask gascan
```

Require a public non-draft, non-prerelease GitHub release with package, checksum, and build-manifest assets; Homebrew must resolve 0.1.20. Remove the completed worktree and its build artifacts while preserving the dirty primary checkout.
