# Plan 2 Task 6 Report: Local API v1

## Scope

- Added the `gascan-proto` workspace crate and `proto/gascan/v1/gascan.proto`.
- Defined only the versioned transport contract. No listener, socket binding, daemon, or Task 7 implementation was added.
- Task 6 is implementation complete and pending independent review.

## TDD evidence

The descriptor/API test was created before the crate or generated bindings.

- Initial RED: `cargo test -p gascan-proto --test api_compatibility` exited 101 with `package ID specification gascan-proto did not match any packages`.
- Operation-ID RED: the same command exited 101 with unresolved `CheckedOperationId` and `OperationIdError` imports.
- Error-code RED: the same command exited 101 because `gascan_proto::error_code` did not exist.
- GREEN: the focused compatibility suite passed 5/5 after the minimum production APIs were added.

## API contract

`gascan.v1.GasCan` exposes exactly:

- Unary: `Handshake`, `Status`, `List`, `Doctor`.
- Server-streaming: `Up`, `Apply`, `Run`, `Shell`, `Down`, `Destroy`, `Logs`.
- Bidirectional streaming: `Attach`.

The descriptor test decodes the binary descriptor, locates the `GasCan` service, rejects extra RPCs, and checks every client/server streaming flag.

`ClientFrame` has stdin bytes, resize, signal, and close variants. `ServerFrame` has stdout bytes, stderr bytes, exit, and structured error variants. Requests and events use byte payloads where data must not be assumed to be UTF-8.

The schema explicitly carries API major/minor values, positive checked operation IDs, protobuf timestamps, desired/actual state enums, capabilities, structured errors with stable string codes, and reserved field numbers. The Rust handshake helper accepts major 1 and returns `incompatible_api_major` for any different major.

## Local transport boundary

`TransportSecurity` represents the contract that the transport is local-only, the socket directory and socket modes, and same-user authentication. The server will populate and enforce those values in Task 7; Task 6 deliberately contains no TCP or Unix listener/binding code.

## Generation design

`protoc-bin-vendored` supplies `protoc`. The build script passes its path through `prost_build::Config::protoc_executable`, avoiding process environment mutation and unsafe Rust. Tonic emits both v1 bindings and the encoded descriptor set.

## Verification

- `cargo test -p gascan-proto --test api_compatibility` — 5 passed.
- `cargo doc -p gascan-proto --no-deps` — passed.
- `cargo clippy -p gascan-proto --all-targets -- -D warnings` — passed.
- `cargo test --workspace` — passed; 9 Apple live tests remained ignored.
- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
