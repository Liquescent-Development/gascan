# Plan 2 Task 4 Report: Durable SQLite State

## Scope

Implemented Task 4 only. No lifecycle service, reconciliation implementation, daemon API, or Apple-specific shapes were added.

## RED / GREEN

- RED: after adding the Task 4 migration/test scaffold and workspace member, `cargo test -p gascand --test store` exited 101 because `crates/gascand/Cargo.toml` did not exist.
- Initial GREEN: after the minimal crate/store implementation, `cargo test -p gascand --test store` passed 9 tests.
- Final GREEN: after strengthening the WAL concurrent-reader coverage, the focused store suite passed 10 tests.

The first GREEN attempt could not resolve crates.io inside the restricted sandbox. The approved retry downloaded `rusqlite` and its bundled SQLite dependencies, then compiled normally.

## Schema and API Decisions

- Schema version 1 lives in `migrations/001_initial.sql`; empty databases migrate transactionally, version 1 reopens, and newer/unknown schemas are rejected.
- `sandboxes` stores sandbox ID, canonical root, desired state, and actual state as queryable relational columns. Both ID and canonical root are unique.
- Setup, tool, and image resolutions are distinct versioned Rust records. Only their extensible `details` payload is JSON; each version remains a relational integer column.
- `operations` stores operation kind/status as relational columns and optional typed failure code plus extensible JSON failure details.
- `operation_events` is append-only, enforced by SQLite update/delete rejection triggers. Beginning and terminal operation events are inserted in the same transaction as their associated state change.
- Every public mutation uses an explicit SQLite transaction. State and operation transitions are loaded and validated in Rust before writes and commit.
- Allowed actual-state edges follow `absent -> creating -> running <-> stopped -> destroying -> absent`, including idempotent same-state writes. Operations allow only `pending -> completed|failed`.
- `Store::complete_operation` accepts the resulting actual state. `Store::fail_operation` accepts the resulting actual state, stable error code, and version-flexible detail payload so Task 5 can atomically persist rollback/reconciliation outcomes.
- Connections enable WAL, foreign keys, and a bounded five-second busy timeout.

## Tests

`crates/gascand/tests/store.rs` covers:

- pending operation durability across drop/reopen;
- sandbox get/list and versioned resolution round trips;
- unique sandbox ID and canonical root conflicts;
- valid and invalid lifecycle transitions;
- completed/failed operation transitions and rejection of repeated completion;
- append-only operation events;
- atomic rollback when a terminal state transition is invalid;
- WAL readers during an uncommitted writer transaction;
- newer and unknown schema rejection.

Final verification commands:

- `cargo test -p gascand --test store` — 10 passed;
- `cargo test -p gascand` — 10 passed plus unit/doc targets;
- `cargo test -p gascan-core` — all core tests and doc tests passed;
- `cargo test --workspace` — all non-live tests passed; 9 existing Apple live tests remained ignored by their platform gates;
- `cargo clippy -p gascand --all-targets -- -D warnings` — passed;
- `cargo fmt --all -- --check` — passed;
- `git diff --check` — passed.

## Files and Shared Manifest Coordination

- Added `crates/gascand/Cargo.toml`.
- Added `crates/gascand/src/lib.rs` and `src/store.rs`.
- Added `crates/gascand/migrations/001_initial.sql`.
- Added `crates/gascand/tests/store.rs`.
- Updated the root `Cargo.toml` workspace members and added workspace `rusqlite = 0.32` with the `bundled` feature for deterministic CI. `Cargo.lock` remains ignored by repository policy.

## Self-review / Concerns

- Public records deliberately expose their fields to keep Task 5 construction straightforward; persistence still validates IDs, versions, database enum values, uniqueness, and state transitions.
- Resolution detail schemas are intentionally versioned rather than hard-coded into Task 4. Task 5 should define the semantic payload contents it owns and bump the corresponding record version when those shapes change.
- The store serializes calls made through one `Store` handle with a mutex around one SQLite connection. Separate handles can read concurrently under WAL, as tested. Task 5 remains responsible for per-sandbox operation serialization at the service layer.
