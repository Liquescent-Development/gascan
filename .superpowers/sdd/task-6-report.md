# Task 6 report: real Apple Gate 4 lifecycle

## Result

Gate 4 is **not passed**. No evidence file was created.

The ignored serial lifecycle and recovery harnesses use the workspace-built `gascan` and
`gascand`, the production Apple backend, unique canonical temporary project/runtime roots,
and deterministic sandbox IDs. Teardown only targets that exact sandbox container and its
three deterministic volume names after checking both `dev.gascan.managed-by=gascan` and the
exact `dev.gascan.sandbox-id` label.

## Live evidence (sanitized)

`./scripts/run-apple-e2e.sh apple_lifecycle` passed host preflight on macOS 26 arm64 with the
signed-off Apple Container 1.1.0 release revision. Initial transport failures were traced to
the harness canonicalizing its runtime root to a pathname longer than macOS `SUN_LEN`; the
daemon could create that path through dirfd-relative bind, but clients failed locally with
`InvalidInput: path must be shorter than SUN_LEN`. Unique test roots now live under short
`/tmp` paths.

The next live failure exposed production inventory incorrectly parsing every foreign Apple
container name as a Gas Can `SandboxId`. A regression now permits arbitrary foreign names while
still classifying invalid or inconsistent Gas Can labels as mismatched.

After those fixes, the initial exact `gascan up` reached the locked image pull and failed:

```text
HTTP request to ghcr.io/.../workspace/manifests/sha256:7c45... failed: 401 Unauthorized
```

No substitute image was used. The final live harness deliberately starts from no daemon.

The daemon now uses one shared pending `DoctorState`: handshake is independent, while Doctor,
up, and apply converge on the same bounded background result and fail closed if collection is
abandoned. Deterministic convergence/failure tests cover the state.

Review-requested structured daemon PID ownership records, durable external cleanup manifests,
bounded child/PTY teardown, and the remaining live signal/host-mutation/no-op scenarios are not
complete. They must be finished before Gate 4 can pass even after image access is available.

## Verification run

- Baseline `cargo test -p gascan-e2e`: pass before Task 6 changes.
- New ignored test binaries: compile successfully with `--no-run`.
- Live lifecycle: plain autostart and runtime inventory reached the exact locked image pull; image
  access is blocked by GHCR authorization, so no later lifecycle assertion was reached.
- Recovery, full runner, global fmt/clippy/test gates: not claimed as Gate 4 evidence because
  the first mandatory live path is blocked.
