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

## Independent review correction

The first review found that attachment correlation, durable event replay semantics, local transport authentication details, and descriptor assertions were underspecified.

The correction keeps the exact 12-RPC service and `Attach(stream ClientFrame)` signature. Every `ClientFrame` now carries a non-oneof opaque byte `session_token` at field 6 while field 5 remains reserved. Run/Shell operation events expose the same token at typed byte field 9. `validate_session_token` rejects empty tokens with stable code `empty_session_token`.

`OperationEvent` now carries a positive, strictly increasing per-operation sequence, stable `OperationStatus` numeric values (unspecified 0, pending 1, completed 2, failed 3), a content type for its opaque byte payload, and a documented single-terminal-event/replay contract. `CheckedEventSequence` prevents zero from entering the durable API.

The local transport contract now defines exact POSIX permission-bit constants (`0700`/448 for the directory and `0600`/384 for the socket), a canonical constructor, and validation. Same-user means authenticating the effective UID through Unix-domain peer credentials. This remains a declarative Task 6 contract; no listener or Task 7 code was added.

The token-string descriptor test was replaced with decoded descriptor assertions covering exact field counts, numbers, scalar/message types and type names, oneof membership, reserved ranges, byte payloads, handshake and transport layouts, all state/status numeric values, and exact RPC cardinalities.

Review-wave TDD evidence:

- RED: focused compilation failed with unresolved session-token and transport constants/helpers.
- RED: after those minimal APIs, focused compilation failed with unresolved `CheckedEventSequence` and `EventSequenceError`.
- GREEN: `cargo test -p gascan-proto --test api_compatibility` passed 8/8.

## Second independent review correction

The second review aligned event sequence validation with the durable SQLite representation: `CheckedEventSequence` now stores and returns `i64`, accepts `1..=i64::MAX`, and rejects zero or larger wire values as `Zero`/`OutOfRange`.

Attach is explicitly one session per stream. `AttachSessionBinder` starts unbound, binds the first nonempty opaque byte token, accepts that exact token on later frames, and rejects any change without replacing the original binding. Stable public error codes cover empty, mismatched, unknown, and expired tokens; Task 7 can use the latter two during daemon session lookup. The protobuf comment records first-frame binding and identical-token requirements. `ServerFrame` remains untagged because each bidirectional stream has exactly one binding.

Descriptor compatibility coverage is now exhaustive rather than representative. The test asserts the complete inventory of all 30 v1 messages; every field's name, number, scalar/message type, fully-qualified type name, label, and oneof membership; every oneof name and reserved range (including `ServerFrame` field 5); every enum value name/number; and each RPC's ordered name, fully-qualified input/output types, and client/server cardinalities. Added or removed exported schema elements fail the inventory counts.

Second-wave TDD evidence:

- RED: sequence boundary assertions failed because `get` returned `u64` and `EventSequenceError::OutOfRange` did not exist.
- RED: binder tests failed with unresolved `AttachSessionBinder` and `AttachSessionError` imports.
- GREEN: focused API compatibility passed 10/10 after the minimal production changes.

Fresh second-wave completion gate:

- `cargo test -p gascan-proto --test api_compatibility` — 10 passed.
- `cargo doc -p gascan-proto --no-deps` — passed.
- `cargo clippy -p gascan-proto --all-targets -- -D warnings` — passed.
- `cargo test --workspace` — passed; 9 Apple live tests remained ignored.
- `cargo fmt --all -- --check` and `git diff --check` — passed.
