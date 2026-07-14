# Plan 4 Task 2 report

Status: **READY FOR REVIEW — live image build/smoke pending controller**

## Scope

Implemented only the workspace image user, guest-root sudo, init, persistent-directory layout, and its smoke fixture. Task 1 locking/downloader/inspect behavior is unchanged. Task 3 was not started; no daemon, Plan 2, or shared root-manifest path was touched.

## TDD evidence

- `tests/image/user-and-volumes.sh` failed immediately on the current macOS host because the `workspace` image user/layout did not exist.
- `image_user_contract` then failed 2/2 because the Dockerfile lacked `sudo`/`tini`/user declarations and the entrypoint/sudoers fixtures were absent.
- After implementation, the non-live image contract tests pass, including a host smoke-fixture contract for handoff consumption, sole workspace bind, zombie inspection, five-second stop, and ownership-gated cleanup.
- Controller-review TDD first produced a missing-validator compile failure. After adding the structured ownership validator, scripted failure-path tests caught and corrected a harness path error; the create-success/start-failure, deferred-signal, and wrong-label-collision cases then passed 3/3.

## Delivered contract

- Image identity: `workspace`, UID/GID 1000, home `/home/workspace`, default `USER workspace:workspace`.
- Guest root: exact mode-0440 rule `workspace ALL=(ALL:ALL) NOPASSWD: ALL`, validated during build with `visudo -cf`.
- Persistent owned targets: `/opt/gascan/mise`, `/home/workspace/.cache`, and `/home/workspace/.config/gascan`.
- Init/entrypoint: `/usr/bin/tini -- /usr/local/bin/gascan-entrypoint`; the entrypoint contains no network/bootstrap behavior and only `exec`s supplied argv or `sleep infinity` for later exec sessions.
- Host smoke fixture reads `.artifacts/workspace-image-ref`, uses explicit create then start with a unique random owner token and only the repository-to-`/workspace` bind, runs the exact in-image privilege/ownership/socket checks, scans `/proc/*/status` for zombies, and requires stop within five seconds.
- Every exit and signal cleanup path accepts only one structured inspect record whose name, ID, test label, and owner token exactly match. It revalidates ownership immediately before deletion, covering a create side effect followed by start failure or a deferred signal without deleting collisions or unowned resources.

## Non-live verification

```text
cargo test --manifest-path scripts/Cargo.toml
17 passed; 0 failed

cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
exit 0

sh -n images/workspace/bin/gascan-entrypoint
exit 0

bash -n tests/image/user-and-volumes.sh
exit 0

git diff --check
exit 0
```

Per controller instruction, `scripts/build-workspace-image.sh` and the live smoke fixture were not executed. The Task 2 live image privilege/signal/zombie evidence remains pending controller authorization.
