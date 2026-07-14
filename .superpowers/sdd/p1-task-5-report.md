# Plan 1 Task 5 Helper Report

## Status

Implemented the approved scoped Swift attach-helper architecture. Cross-platform Rust protocol/bridge tests and Swift debug protocol tests pass. The controller must complete the release helper build and ignored Apple live suite.

## RED / GREEN

- RED: `cargo test -p gascan-apple --test attach_protocol`
  - Failed because `HelperInput`, `HelperOutput`, and `HELPER_PROTOCOL_VERSION` did not exist.
- GREEN: `cargo test -p gascan-apple --test attach_protocol --test attach_session`
  - Protocol: 4 passed.
  - Fake-helper bridge: 3 passed.
- GREEN: `swift test --package-path helpers/apple-attach --skip-update`
  - Helper compiled against Apple `container` 1.1.0.
  - Swift Testing: 3 passed.
- GREEN: `cargo test --workspace`
  - All non-ignored tests passed; 7 Apple-runtime tests ignored.
- GREEN: `cargo clippy --workspace --all-targets -- -D warnings`
- GREEN: `cargo fmt --all -- --check`
- GREEN: `git diff --check`

## Implementation

- Replaced the disproven `container exec`/local-PTY implementation with a Rust bridge to `gascan-apple-attach`.
- Added version 1 single-session NDJSON frames with base64 stdin/stdout/stderr payloads.
- Rust sends exactly one start frame, validates guest argv and protocol versions, permits TTY-only resize, translates TTY SIGINT to terminal byte `0x03`, and promptly rejects every other signal combination as unsupported.
- Bounded Rust channels provide backpressure; every input write is acknowledged.
- Rust only accepts typed helper stdout/stderr/error/exit frames and never derives guest exit from helper status or diagnostic text.
- Session drop closes input and requests termination of only the owned helper child.
- Swift validates the start version before any `ContainerClient` request, creates one guest process with private pipes, retains its `ClientProcess`, and calls only `start`, `resize`, and `wait`; it never calls the broken 1.1.0 `kill` path.
- Swift emits serialized frames through one actor, ensuring at most one typed terminal error or exact exit event.
- The helper exposes no socket, image, registry, mount, network, lifecycle, or arbitrary-XPC operation.
- Removed obsolete `portable-pty`, direct `nix`, and `anyhow` dependencies. A dependency-tree check found none of those direct implementation dependencies.

## Apple API evidence

Apple `container` tag 1.1.0 declares product `ContainerAPIClient`. Its public `ClientProcess` API provides `start()`, `resize(Terminal.Size)`, `kill(Int32)`, and `wait() -> Int32`; `ContainerClient.createProcess` returns that retained process. `Package.swift` pins `container` exactly to `1.1.0`, and `Package.resolved` records revision `5973b9cc626a3e7a499bb316a958237ebe14e2ed`.

## Build and live commands for controller

Run from `/Users/kiener/code/gascan/.worktrees/macos-mvp`:

```sh
swift test --package-path helpers/apple-attach
./scripts/build-apple-attach-helper.sh
cargo test -p gascan-apple --test live attach -- --ignored --test-threads=1
```

The durable ignored live suite covers binary separation, exact exits 0/42 for every process that starts, typed Apple start failure for a missing executable, resize, TTY SIGINT, prompt rejection of unsupported signals, stdin close, and ownership-safe cleanup. Live investigation also verified that `ClientProcess.kill` in pinned `ContainerAPIClient` 1.1.0 can hang because its public client writes an integer signal while the server reads a string. The helper therefore never calls that API; non-TTY SIGINT, SIGTERM, and all other signal requests are explicitly unsupported rather than reported as false passes.

## Interrupted release build

The first sandboxed release build failed because Swift could not write `~/.cache/clang/ModuleCache`. The required escalated rerun of `./scripts/build-apple-attach-helper.sh` ran for about 318 seconds without captured output and was interrupted on request. No release-build or live-pass claim is made.

## Files

- `Cargo.toml`
- `crates/gascan-apple/Cargo.toml`
- `crates/gascan-apple/src/attach.rs`
- `crates/gascan-apple/src/helper_protocol.rs`
- `crates/gascan-apple/src/lib.rs`
- `crates/gascan-apple/tests/attach_protocol.rs`
- `crates/gascan-apple/tests/attach_session.rs`
- `crates/gascan-apple/tests/fixtures/fake-attach-helper/`
- `crates/gascan-apple/tests/live/attach.rs`
- `crates/gascan-apple/tests/live/common/mod.rs`
- `helpers/apple-attach/`
- `scripts/build-apple-attach-helper.sh`

## Concerns

- Controller verification is still required for the release build and live Apple semantics.
- The pinned Apple package has a large transitive Swift dependency graph, making a clean release build slow.
- The helper currently emits small stdout/stderr frames to preserve low-latency interactive reads; bounded pipes/channels still propagate backpressure.

## Live hang root cause and fix

Controller diagnostics proved the guest emitted all expected stdout/stderr bytes and `ClientProcess.wait()` returned 42, but neither private pipe produced EOF. The helper therefore waited forever on `stdoutTask.value` / `stderrTask.value` and never emitted the terminal exit frame.

The fix mirrors Apple 1.1.0 `ProcessIO.wait()`: allow three seconds for post-wait output drain, then cancel the reader tasks, close their handles, and emit the exact code already returned by `ClientProcess.wait()`. Bytes emitted before or during the drain remain preserved. Gated diagnostics remain available through `GASCAN_ATTACH_DIAGNOSTICS`.

TDD evidence:

- RED: focused Swift test failed because `drainAfterWait` did not exist.
- GREEN: `boundedDrainPreservesBytesWhenReaderNeverReachesEOF` uses a real pipe, receives `[0, 255]`, deliberately withholds EOF, and completes after the configured 50 ms bound without losing bytes.
- Full Swift suite: 4 passed.
- Full Rust workspace tests: all non-ignored tests passed; 7 live tests ignored.
- Workspace Clippy with warnings denied, Rust formatting, and diff checks passed.

Controller should rebuild the helper and rerun the ignored live attach suite; this subagent did not run Apple live commands.

### Cancellation-cooperative relay correction

The first bounded-drain implementation still used `FileHandle.AsyncBytes` tasks. Live evidence showed the timeout task won but `withTaskGroup` could not leave its scope because the drain child awaited reader tasks that ignored cancellation and handle closure. A focused RED regression reproduced this exact structured-concurrency behavior with a controllably cancellation-ignoring reader.

The final implementation mirrors Apple `ProcessIO` more closely: a `readabilityHandler` yields into a bounded `AsyncStream`, disables reads until the emitted chunk is consumed, and removes the handler/closes the handle on stream termination. Explicit relay cancellation therefore completes the stream and lets the timeout task group return. This preserves byte ordering and backpressure.

- Focused structured-drain regression: GREEN.
- Focused preceding-byte preservation regression: GREEN with `[0, 255]` and no EOF.
- Full Swift suite: 5 passed.
- Rust workspace tests, Clippy with warnings denied, formatting, and diff checks: passed.
