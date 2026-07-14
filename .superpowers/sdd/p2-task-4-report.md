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

## Review Fix Follow-up

### Additional RED / GREEN Evidence

- First review RED: `cargo test -p gascand --test store` failed to compile because the new stable error contracts (`DuplicateSandboxId`, `DuplicateCanonicalRoot`, `PendingOperationExists`, and `SchemaMismatch`) were absent.
- Schema-hardening RED: the malformed-v1 focused test failed because a spoofed `schema_version` table without singleton enforcement was accepted.
- Final review GREEN: `cargo test -p gascand --test store` passed all 18 tests, including two deterministic subprocess-abort recovery tests.

### Failure Transition Correction

- Normal `put_sandbox` transitions and completed operations retain the forward lifecycle graph.
- Failed operations additionally validate operation-kind-specific rollback results: failed Create may record `creating -> absent` after verified cleanup, and failed Destroy may record `destroying -> running|stopped` after verified rollback/reconciliation. Same-state failure outcomes remain valid.
- Tests cover both Destroy restoration outcomes, Create cleanup to absent, and rejection of the Create rollback edge for a completed operation.

### Durable Serialization and Typed Conflicts

- Schema v1 now has a partial unique index on `operations(sandbox_id)` where status is pending, enforcing at most one pending operation per sandbox across connections and daemon restarts.
- Mutations use `BEGIN IMMEDIATE` transactions. Identity and pending-operation conflicts are prechecked inside the acquired writer transaction, producing stable typed errors instead of SQLite diagnostic strings.
- Exact tests assert duplicate ID, duplicate canonical root, and durable pending-operation error variants.

### Schema Validation

- `schema_version` now has a single primary-key sentinel constrained to value 1, with exactly one row required.
- Opening v1 validates exact ordered columns, declared SQLite types, nullability, and primary-key positions for all required tables.
- Opening also validates both foreign keys, canonical-root uniqueness, the named partial pending-operation index and predicate, plus both append-only trigger definitions.
- Parameterized SQLite table-valued PRAGMA queries are used for structural inspection; schema identifiers are not interpolated into SQL.
- Tests reject partial v1 schemas, missing tables/columns/nullability, both missing FKs, both missing uniqueness mechanisms, missing/malformed singleton representation, missing triggers, and multiple version rows.

### Genuine Crash Recovery

- Parent tests spawn the current integration-test executable in an environment-gated child mode.
- The child opens the real database, starts `BEGIN IMMEDIATE`, performs partial parameterized writes matching either begin-operation or terminal-operation transaction shapes, then aborts the process before commit.
- The parent requires abnormal child exit, reopens through `Store`, and proves SQLite recovered only pre-transaction state: no partially begun sandbox/operation survives, while a partially terminalized operation remains pending with its original actual state.
- Normal committed begin/complete/fail tests remain the fully committed controls.

### Follow-up Verification and Self-review

- `cargo test -p gascand` passed all 18 store tests plus unit/doc targets.
- `cargo test -p gascan-core` passed all core and doc tests.
- `cargo test --workspace` passed all enabled workspace tests; 9 existing Apple live tests remained ignored by their platform gates.
- `cargo clippy -p gascand --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check` passed.
- The partial unique index is the durable global guard; Task 5 keyed locks can improve wait/progress behavior but must treat `PendingOperationExists` as authoritative across processes/restarts.
- Crash simulation is test-only and contains no production failpoint or timing dependency.

## Schema Spoof-Resistance Follow-up

- RED: `cargo test -p gascand --test store superficially_similar_but_weakened_v1_schemas_are_rejected -- --nocapture` failed because a conditional/partial canonical-root unique index was accepted as equivalent to the required invariant.
- GREEN: the focused malicious-schema test passed, followed by the complete 19-test store suite.
- Canonical-root uniqueness now explicitly requires a non-partial unique index over exactly `canonical_root`.
- The pending-operation guard requires the exact normalized schema-v1 index definition, rejecting weakened predicates such as `status = 'pending' AND 0`.
- The singleton version table and append-only update/delete triggers require their exact normalized schema-v1 definitions, rejecting permissive checks and `WHEN 0` triggers that retain misleading substrings.
- Foreign-key validation compares the complete ordered FK set for every v1 table, including target table, source/target columns, update/delete actions, match mode, and absence of extra keys.
- Regression fixtures cover a conditional root index, false pending predicate, permissive singleton check, disabled update/delete triggers, altered FK action, and an extra FK.
- Structural introspection remains parameterized; normalization is applied only to SQLite-owned `sqlite_master` definitions and compared against fixed schema-v1 constants.
