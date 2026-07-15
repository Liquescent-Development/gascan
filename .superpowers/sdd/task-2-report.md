# Task 2 report: structured Apple inspection

Status: **PASS**

## Result

- Added `AppleInspector<R>` with literal `container inspect <id>` and `container list --all --format json` requests through `CommandRunner`.
- Added private Serde DTOs for the exact Apple 1.1 container shape: `configuration.id`, nested `configuration.labels`, and `status.state`. Unknown JSON fields remain accepted.
- Mapped `creating`, `running`, and `stopped` explicitly. Unknown states return `RuntimeError::UnknownActualState`; missing/invalid identity or state data returns typed invalid output.
- Preserved Task 1's production labels exactly: `dev.gascan.managed-by=gascan` and `dev.gascan.sandbox-id=<SandboxId>`.
- Classified inventory observations as `GasCanOwned`, `Foreign`, or `Mismatched`. Each result is constructed through `RuntimeResource::discovered`, so each inventory receives fresh opaque, process-local removal proofs. Nothing added a deserialization or removal path; Task 3 must re-inventory and revalidate current state.
- Inspect absence recognizes only Apple's documented missing-container exit code (`1`) for the literal `container` operation. Diagnostic stderr is deliberately not parsed; all other exit codes and runner errors remain errors.
- Added versioned running, stopped, and mixed-list fixtures plus malformed required-field, invalid-ID, unsupported-state, unknown-field, ownership-classification, fresh-proof, and exact-not-found-code coverage.

## TDD evidence

Initial focused command:

`cargo test -p gascan-apple --test inspect`

Observed RED (exit 101): unresolved import `gascan_apple::AppleInspector`; no implementation existed. The initial test also exposed use of an unsupported `FromStr` test helper, which was corrected to the public validated `TryFrom<String>` path before production implementation.

After the minimal DTO/parser implementation, the same command passed 4/4.

## Verification

- `cargo fmt --all -- --check` — pass
- `cargo test -p gascan-apple --test inspect` — pass, 4 passed
- `cargo clippy -p gascan-apple --all-targets -- -D warnings` — pass
- `cargo test -p gascan-apple` — pass, all non-live tests; 9 existing hardware/runtime-dependent live tests ignored
- `git diff --check` — pass

## Self-review

- No human-readable CLI output is parsed.
- No unknown JSON field can promote ownership.
- Required identity and state fields fail closed.
- A managed-by claim without the exact sandbox-ID annotation is mismatched, not owned.
- Foreign resources remain visible in inventory with an explicit classification.
- Inspect response identity must equal the requested identity.
- No create/start/stop/remove/log/backend mutation behavior was added.
- The pre-existing `.superpowers/sdd/progress.md` worktree modification was preserved and excluded from this task's commit.

## Concerns

Apple's missing-container contract is represented by exit code `1` because stderr is human-facing and prohibited as a parsing boundary. Task 3 should preserve this structured exit-code handling when it composes inspection with lifecycle operations.
