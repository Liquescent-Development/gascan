### Task 5: Build the lifecycle service and reconciliation

**Files:**
- Create: `crates/gascand/src/service.rs`
- Create: `crates/gascand/src/reconcile.rs`
- Test: `crates/gascand/tests/lifecycle.rs`
- Test: `crates/gascand/tests/reconcile.rs`

**Interfaces:**
- Produces: `SandboxService<B: RuntimeBackend>` methods `up`, `apply`, `start`, `stop`, `destroy`, `status`, `list`, and `reconcile`.
- Operations return an `OperationId` and structured `OperationEvent` stream.
- A keyed async lock serializes mutations for one sandbox while allowing unrelated sandboxes concurrently.

- [ ] **Step 1: Write rollback and reconciliation tests using `FakeRuntime`**

```rust
#[tokio::test]
async fn failed_create_preserves_existing_volumes_and_records_failure() {
    let runtime = FakeRuntime::failing_once("start");
    runtime.seed_volume("gascan-cache-code").await;
    let service = fixture_service(runtime.clone()).await;
    assert!(service.up(UpRequest::fixture()).await.is_err());
    assert!(runtime.volume_exists("gascan-cache-code").await);
    assert_eq!(service.store().latest_operation().unwrap().status, OperationStatus::Failed);
}
```

- [ ] **Step 2: Verify lifecycle tests fail**

Run: `cargo test -p gascand --test lifecycle --test reconcile`

Expected: FAIL because `SandboxService` is undefined.

- [ ] **Step 3: Implement transactional orchestration**

For `up`: validate/canonicalize, persist pending, compile policy, create only absent resources, start, durably bracket provision and health hooks with append-only phase events, and persist the actual versioned resolution with a stable desired-content fingerprint. Roll back only resources structurally returned by `CreateOutcome::created()` or `CreateFailure::created()`. Destroy always inventories, derives exact expected identities, and removes only the intersection carrying exact association, current ownership, and that inventory's opaque removal proof. Reconcile reports extra owned, unowned, and mismatched resources but never deletes unknown resources; pending Create/Apply completes only with durable successful hook evidence. Async service methods dispatch every Store call to blocking workers. Operations use checked positive `OperationId`, while keyed locks retain weak entries only. The backend retains exactly nine methods and uses `list_resources` plus typed `RemoveRequest`.

- [ ] **Step 4: Run lifecycle and reconciliation tests**

Run: `cargo test -p gascand --test lifecycle --test reconcile`

Expected: PASS for idempotent up/down, fingerprinted retry/apply, stopped auto-start with actual failure state, missing sandbox refusal, concurrent operations, durable hook recovery phases, exact and extra-owned inventory, partial-create rollback, nonblocking SQLite access, weak lock cleanup, and setup failure.

- [ ] **Step 5: Commit orchestration**

```bash
git add crates/gascand/src crates/gascand/tests
git commit -m "feat: orchestrate durable sandbox lifecycle"
```
