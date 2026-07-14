# Plan 4 Task 2 report

Status: **READY FOR REVIEW — live image build/smoke pending controller**

## Scope

Implemented only the workspace image user, guest-root sudo, init, persistent-directory layout, and its smoke fixture. Task 1 locking/downloader/inspect behavior is unchanged. Task 3 was not started; no daemon, Plan 2, or shared root-manifest path was touched.

## TDD evidence

- `tests/image/user-and-volumes.sh` failed immediately on the current macOS host because the `workspace` image user/layout did not exist.
- `image_user_contract` then failed 2/2 because the Dockerfile lacked `sudo`/`tini`/user declarations and the entrypoint/sudoers fixtures were absent.
- After implementation, the non-live image contract tests pass 3/3, including a host smoke-fixture contract for handoff consumption, sole workspace bind, zombie inspection, five-second stop, and cleanup.

## Delivered contract

- Image identity: `workspace`, UID/GID 1000, home `/home/workspace`, default `USER workspace:workspace`.
- Guest root: exact mode-0440 rule `workspace ALL=(ALL:ALL) NOPASSWD: ALL`, validated during build with `visudo -cf`.
- Persistent owned targets: `/opt/gascan/mise`, `/home/workspace/.cache`, and `/home/workspace/.config/gascan`.
- Init/entrypoint: `/usr/bin/tini -- /usr/local/bin/gascan-entrypoint`; the entrypoint contains no network/bootstrap behavior and only `exec`s supplied argv or `sleep infinity` for later exec sessions.
- Host smoke fixture reads `.artifacts/workspace-image-ref`, creates an owned labeled container with only the repository-to-`/workspace` bind, runs the exact in-image privilege/ownership/socket checks, scans `/proc/*/status` for zombies, requires stop within five seconds, and cleans up only after successful creation.

## Non-live verification

```text
cargo test --manifest-path scripts/Cargo.toml
13 passed; 0 failed

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
