# Gas Can Shell PTY EAGAIN Design

## Problem

`gascan shell` makes a duplicate of the host terminal's standard input and sets
that duplicate to `O_NONBLOCK` so its asynchronous input task can be cancelled.
File status flags belong to the open file description, not an individual file
descriptor. In a normal PTY, standard input and standard output can be
duplicates of the same open file description, so changing the input duplicate
also makes Gas Can's output nonblocking.

When a guest command produces enough output to fill the host terminal buffer,
Gas Can's synchronous `write_all` receives `EAGAIN` (`os error 35`). The CLI
treats the write error as fatal, drops the attachment, and returns the user to
the host shell. `gascan ssh` does not use this attachment input path and is not
affected.

## Requirements

- Interactive `gascan shell` must not change the file-status flags of host
  standard input, output, or error.
- Interactive input must remain cancellable when the guest process exits.
- Gas Can must correctly forward output when stdout or stderr was already
  nonblocking before Gas Can started.
- Output retry behavior must not spin when a descriptor is not writable.
- Terminal modes and any flags owned by Gas Can must be restored on success,
  error, signal, and cancellation.
- `gascan run`, piped input, and `gascan ssh` behavior must remain unchanged.

## Design

### Independent interactive input

For an interactive TTY attachment, Gas Can will open `/dev/tty` read-only with
close-on-exec to obtain a new open file description for the controlling
terminal. It will set `O_NONBLOCK` only on that independently opened handle and
register it with Tokio's `AsyncFd`. Because this handle does not share its open
file description with stdout or stderr, its flags cannot leak onto output.

If `/dev/tty` cannot be opened, Gas Can will fail the interactive attachment
with a concrete terminal-input error rather than reverting to the unsafe
duplicated-descriptor behavior. Non-interactive piped input will keep its
current pipe-specific path because the input pipe does not share an open file
description with output.

### Resilient output

Interactive stdout and stderr forwarding will use a small writer abstraction.
It will attempt the write immediately. On `WouldBlock`/`EAGAIN`, it will wait
for descriptor writability through Tokio and resume from the unwritten offset.
Other errors remain fatal. Empty writes and interrupted syscalls are handled
without losing or duplicating bytes.

The writer will borrow the existing standard output/error descriptors and will
not change their flags. If they are already nonblocking, readiness waiting
handles backpressure; if they are blocking, the ordinary write completes as it
does today.

### Error behavior

Failure to open or register `/dev/tty` will produce an actionable Gas Can
runtime error naming the controlling-terminal input failure. Output failures
other than temporary backpressure retain their existing error propagation. A
temporary lack of output capacity will no longer terminate the attachment.

## Testing

- A PTY regression test will model stdin and stdout as duplicates of the same
  terminal handle and prove constructing interactive input leaves stdout's
  `O_NONBLOCK` bit unchanged. This test must fail against the current code.
- A writer test will start with a deliberately nonblocking, capacity-limited
  descriptor, apply backpressure, then drain it and prove every byte is written
  exactly once without `EAGAIN` escaping.
- Existing cancellation tests will continue to prove prompt task shutdown and
  terminal restoration.
- Focused `gascan` tests, the full workspace suite, strict Clippy, formatting,
  and the macOS release smoke will verify integration.

## Delivery

The fix will be reviewed and merged through a feature PR. After merge, the six
release crates, root `Cargo.lock`, README release references, and macOS release
checklist will be bumped from 0.1.19 to 0.1.20 in a separate release PR. The
merged release commit will receive a signed `v0.1.20` tag, followed by the
standard signed, notarized, GitHub, and Homebrew release pipeline.
