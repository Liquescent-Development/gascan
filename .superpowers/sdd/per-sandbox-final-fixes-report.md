# Per-sandbox managed network final fixes

Date: 2026-07-23
Branch: `feat/per-sandbox-network`
Starting HEAD: `500e492`
Commit subject: `fix: revalidate managed resources during cleanup`

## Outcome

The final whole-branch findings are addressed in one change:

- The shell cleanup keeps its full initial collision preflight, then obtains a new strict structured inventory immediately before every container stop/delete, volume delete, and network delete. Exact absence skips mutation; command failure, malformed inventory, ambiguity, or changed ownership aborts while retaining the cleanup manifest.
- The Rust Apple E2E cleanup keeps its full preflight and re-runs the strict resource-presence observation immediately before every resource mutation. A freshly absent resource is skipped and a fresh collision/error aborts.
- A shell regression changes the network to foreign ownership after all volume deletions and proves the network and cleanup manifest are retained.
- A platform-neutral Rust helper regression changes network ownership after a preceding volume mutation and proves the network mutation is refused.
- Backend fake-runner coverage seeds separate exact-name owned, foreign, and mismatched networks. Each create returns `resource_conflict` before mutation, preserves the seeded network, and runs no network/volume/container create command.
- The three stale test names now describe retained managed resources or the managed network plus prior volumes.

## Files

- `scripts/apple-e2e-cleanup.sh`
  - Added strict fresh container, volume, and network inventory helpers.
  - Preserved the initial complete preflight.
  - Revalidated immediately before each in-scope host mutation.
- `scripts/tests/apple_e2e_cleanup.rs`
  - Added the foreign-after-volume network ownership transition fixture and regression.
  - Updated the successful cleanup inventory-count assertion for the fresh network check.
- `crates/gascan-e2e/tests/apple_common/mod.rs`
  - Added `mutate_if_freshly_owned`.
  - Reworked cleanup to observe ownership immediately before each stop/delete.
  - Added a platform-neutral sequential freshness regression.
- `crates/gascan-apple/tests/backend_fake_runner.rs`
  - Added the three-state exact-name collision regression.
  - Renamed the three stale tests.

The pre-existing `.superpowers/sdd/progress.md` modification was not edited or staged.

## TDD evidence

RED:

1. `rtk cargo test --manifest-path scripts/Cargo.toml --test apple_e2e_cleanup network_changed_to_foreign_after_volume_deletion_is_retained_with_manifest`
   - Failed as intended: cleanup exited successfully after deleting from stale network proof (`assertion failed: !output.status.success()`).
2. `rtk cargo test -p gascan-e2e --test apple_lifecycle cleanup_revalidates_each_resource_immediately_before_mutation`
   - Failed as intended: `mutate_if_freshly_owned` did not exist (`E0425`).

GREEN:

1. `rtk cargo test --manifest-path scripts/Cargo.toml --test apple_e2e_cleanup network_changed_to_foreign_after_volume_deletion_is_retained_with_manifest`
   - 1 passed, 32 filtered out.
2. `rtk cargo test -p gascan-e2e --test apple_lifecycle cleanup_revalidates_each_resource_immediately_before_mutation`
   - 1 passed, 38 filtered out.
3. `rtk cargo test -p gascan-apple --test backend_fake_runner exact_name_networks_always_conflict_before_any_create_mutation`
   - 1 passed, 28 filtered out.

## Required verification

- `rtk cargo test -p gascan-apple --test backend_fake_runner`
  - 29 passed.
- `rtk cargo test --manifest-path scripts/Cargo.toml --test apple_e2e_cleanup`
  - 33 passed.
- `rtk cargo test -p gascan-e2e --test apple_lifecycle`
  - The sandboxed run reached 37 passed then failed one PTY test with `Operation not permitted`.
  - Re-run with host PTY permission: 38 passed, 1 ignored.
- `rtk cargo fmt --all -- --check`
  - Passed.
- `rtk git diff --check`
  - Passed.
- `rtk sh -n scripts/apple-e2e-cleanup.sh`
  - Passed.

## Live Apple result and residue audit

Command:

`rtk bash ./scripts/run-apple-e2e.sh apple_lifecycle`

Result:

- 1 passed, 0 failed, 38 filtered out.
- The harness built `gascan-apple-attach`, completed host preflight, ran the live lifecycle, and completed its owned cleanup.

Read-only post-run audit:

- `rtk container list --all --format json`
- `rtk container volume list --format json`
- `rtk container network list --format json`
- `rtk container volume inspect gascan-mise-my-project-50aeb8022681`
- `rtk container volume inspect gascan-cache-my-project-50aeb8022681`
- `rtk container volume inspect gascan-config-my-project-50aeb8022681`

The complete inventories contained no Gate 4 lifecycle container, managed volume, or managed network residue from this run. The three protected pre-existing `my-project-50aeb8022681` volumes remain present with `dev.gascan.managed-by=gascan` and `dev.gascan.sandbox-id=my-project-50aeb8022681`. No old host resource was deleted or modified.

## Self-review

- Confirmed all initial container/volume/network inventories and ownership collision checks still finish before the first container, volume, or network mutation.
- Confirmed shell fresh inventory validation checks the entire array shape and ID/name equality, selects exactly zero or one exact record, and re-checks both ownership labels.
- Confirmed fresh absence never invokes delete and all fresh validation failures exit before mutation, leaving the manifest in place.
- Confirmed the container is freshly checked once before stop and again before delete.
- Confirmed Rust cleanup performs a fresh observation for every mutation and propagates observation/collision errors.
- Confirmed backend collision assertions exclude every network create, volume create, and container run command and compare the exact seeded network tuple after failure.
- Reviewed the final scoped diff; no production backend behavior was changed, and no unrelated host resources or progress ledger content were touched.

## Concerns

None. The PTY lifecycle suite requires host permission in this environment; with that permission it passed completely.
