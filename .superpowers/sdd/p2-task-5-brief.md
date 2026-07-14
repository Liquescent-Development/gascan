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

For `up`: validate/canonicalize, persist pending, compile policy, create only absent resources, start, provision/health-check through injected hooks, persist ready, and emit events. Roll back only `CreateOutcome.created`. Destroy inventories and removes only exact known owned resources for the target. Reconcile desired and actual state after restart; report unknown owned, unknown unowned, and mismatched resources but never delete unknown resources. Implement explicit `apply` change detection and non-destructive failure behavior. This is the controller-authorized joint Task 2/Task 5 seam revision; the backend retains exactly nine methods and uses `list_resources` plus typed `RemoveRequest`.

- [ ] **Step 4: Run lifecycle and reconciliation tests**

Run: `cargo test -p gascand --test lifecycle --test reconcile`

Expected: PASS for idempotent up/down, stopped auto-start, missing sandbox refusal, concurrent operations, every injected crash point, unknown owned/unowned resources, and setup failure.

- [ ] **Step 5: Commit orchestration**

```bash
git add crates/gascand/src crates/gascand/tests
git commit -m "feat: orchestrate durable sandbox lifecycle"
```
