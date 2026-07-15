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
signed-off Apple Container 1.1.0 release revision, then failed on the initial exact `gascan up`:

```text
daemon readiness exhausted after 182 probes in 5.0s
daemon_alive=true
daemon_stderr=<empty>
```

This reproduced three times. Temporary phase diagnostics established that the daemon remains
alive while collecting production doctor evidence before binding/serving its socket. A
test-first bounded-timeout experiment (6-second delayed daemon) passed unit regressions at 15
and 30 seconds, but the real host still exhausted 30 seconds. That disproved a safe timeout-only
fix; the experiment and temporary diagnostics were reverted.

The required production correction is to decouple transport readiness from slow Apple doctor
probes (serve promptly and collect/cache bounded doctor evidence asynchronously or on demand).
The final live harness deliberately starts from no daemon and does not prestart or substitute a
fake backend.

## Verification run

- Baseline `cargo test -p gascan-e2e`: pass before Task 6 changes.
- New ignored test binaries: compile successfully with `--no-run`.
- Live lifecycle: blocked at initial daemon autostart as above; no lifecycle assertions after
  `up` were reached.
- Recovery, full runner, global fmt/clippy/test gates: not claimed as Gate 4 evidence because
  the first mandatory live path is blocked.
