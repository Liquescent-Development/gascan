# Plan 2 Task 5 Report: Durable Sandbox Lifecycle

## Scope and commits

- `15921b5 refactor: represent runtime resource outcomes`
- Task 5 lifecycle implementation and this report are committed together.
- Stopped before Task 6. No Apple backend implementation was changed.

The controller explicitly authorized a joint Task 2/Task 5 interface correction while preserving exactly nine `RuntimeBackend` methods.

## TDD evidence

### Backend seam RED

`cargo test -p gascan-core --test backend_contract` failed before production changes with unresolved `RemoveRequest`, `ResourceKind`, and `ResourceOwnership`, a unit `create` result, and missing `list_resources`/mismatch fixture support.

### Backend seam GREEN

The same command passed 11/11 after the typed resource seam and fake runtime were implemented.

### Lifecycle RED/GREEN

The destroy-selection regression initially failed with `Ownership(SandboxId(...))` when a foreign volume shared the target association. Production selection was narrowed to exact GasCan-owned resources; lifecycle and reconciliation then passed. Deterministic start gates initially exposed that gated calls were recorded too late to observe concurrency; call recording was moved to the literal boundary and the same-key/different-key tests passed.

Final focused counts:

| Suite | Result |
|---|---:|
| Task 2 backend contract | 11 passed |
| Task 3 policy | 10 passed |
| Task 5 lifecycle | 14 passed |
| Task 5 reconcile | 3 passed |
| Store | 20 passed |

## Backend API and ownership model

| Concern | Contract |
|---|---|
| Create | `create(CreateRequest) -> Result<CreateOutcome, CreateFailure>`; both success and failure expose only request-validated resources created by that call |
| Inventory | `list_resources() -> Vec<RuntimeResource>`; each observation carries an opaque process-local removal proof and cannot be deserialized |
| Identity | Backend-neutral `ResourceIdentity { kind, name }` |
| Kinds | `Container`, `Volume` |
| Association | Optional validated `SandboxId` |
| Ownership | `GasCanOwned`, `Foreign`, `Mismatched` |
| Remove | `RemoveRequest` carries exact selected owned resources and opaque proof; the backend revalidates the complete current observation |
| Refusal | Stable `foreign_resource_refused` and `ownership_mismatch` codes |

`FakeRuntime` models containers and volumes, collisions, full ownership inventory, selective removal, fail-once boundaries, deterministic gates, literal calls, and success/failure outcomes.

## Lifecycle behavior

| Operation | Durable behavior |
|---|---|
| `up` | Compile policy, create or auto-start, provision, health check, complete Running |
| `apply` | Detect change, retain prior setup/tool resolution on failure |
| `start` / `stop` | Idempotent, ownership checked, synchronous failures terminalized |
| `destroy` | Inventory first; remove only exact owned target resources; retain foreign resources |
| `status` / `list` | Read durable records without runtime mutation |

Same-sandbox mutations serialize through keyed async locks. Different sandbox keys reach deterministic gated runtime boundaries concurrently.

## Rollback matrix

| Failure | Cleanup/result |
|---|---|
| Create collision | Foreign volume retained; typed conflict |
| Start after create | Remove only `CreateOutcome::created()` |
| Create fails after mutation | Remove only `CreateFailure::created()` |
| Preexisting owned volume | Preserved during rollback |
| Newly created volumes/container | Removed during rollback |
| Provision failure | Created resources removed; Failed durable terminal |
| Health failure | Created resources removed; Failed durable terminal |
| Apply failure | Prior setup/tool resolutions retained |
| Start/stop/remove/inspect failure | Verified actual state recorded; no pending operation remains |

## Reconciliation matrix

| Observation | Action |
|---|---|
| Unknown GasCan-owned | Report `UnknownOwned`; retain |
| Foreign/unowned | Report `UnknownUnowned`; retain |
| Ownership mismatch | Report `OwnershipMismatch`; retain |
| Known durable record absent | Report `MissingOwned` |
| Pending Create/Apply/Start/Stop/Destroy after reopen | Complete if converged, otherwise fail as interrupted; never delete unknown resources |

## Event stream

Operation events use a bounded channel (capacity 16). The store is written before delivery. Initial pending, progress, and terminal events retain durable sequence order. Channel refusal is a stable `event_stream_unavailable` service error rather than a silent drop. Tests compare the stream with the durable event log and verify receiver drop does not deadlock later operations.

## Documentation changes

- Plan 2 full plan and Task 5 brief record the joint interface change and exact rollback/removal rules.
- Plan 3 Apple lifecycle interface snippets now use `list_resources` and exact `RemoveRequest`.
- No Apple implementation was added.

## Verification

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo clippy -p gascan-core -p gascand --all-targets -- -D warnings` — passed.
- `cargo test -p gascan-core` — passed, including 8 compile-fail doctests.
- `cargo test -p gascand` — passed.
- `cargo test --workspace` — passed; Apple live tests remained ignored.

## Self-review and concerns

- Synchronous errors after `begin_operation` are terminalized. If a process actually disappears between durable begin and terminal persistence, the pending row intentionally remains for reopen reconciliation.
- The bounded capacity is deliberately above the current maximum events emitted synchronously before an `Operation` is returned. Future additions that exceed it fail explicitly and keep the durable store authoritative.
- Resource inventory is intentionally backend-neutral; Apple translation remains Plan 3 work.
