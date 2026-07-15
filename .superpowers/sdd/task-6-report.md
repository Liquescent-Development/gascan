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

Harness cleanup now writes a durable, exact-scope manifest before the first mutation. The runner
traps EXIT, INT, TERM, and HUP, validates exact names and both ownership labels, stops before
deleting, verifies absence, and reports residue. It scavenges only manifests that pass the same
scope/label/token validation on the next run. SIGKILL cannot be trapped; next-run scavenging is
the recovery path. A live 401 run verified that the manifest was removed and only its validated
daemon instance was terminated.

Daemon termination is fail-closed: the daemon atomically records PID, harness owner token,
canonical executable, and process start identity. Before any signal, the harness revalidates all
fields, the current process command/start identity, and its live socket. Corrupt and reused PID
records are refused. CLI waits are bounded at 90 seconds, PTY/signal waits at 10–30 seconds, and
timeout tests prove kill/reap behavior. Exact-name resources with missing or mismatched labels are
reported as collisions and never treated as absence or deleted.

The ignored lifecycle now defines SIGINT/SIGTERM forwarding, a no-op setup/apply, exact owned
host stop followed by apply/start reconciliation, daemon kill/restart, and final absence checks.
These assertions remain unreached solely because the exact image pull is unauthorized.

## Verification run

- Baseline `cargo test -p gascan-e2e`: pass before Task 6 changes.
- New ignored test binaries: compile successfully with `--no-run`.
- Non-live harness safety: corrupt/reused daemon records refused; timed-out child killed/reaped;
  cleanup manifest scope refusal and owned container stop-before-delete ordering pass.
- Live lifecycle: plain autostart and runtime inventory reached the exact locked image pull; image
  access is blocked by GHCR authorization, so no later lifecycle assertion was reached.
- Recovery, full runner, global fmt/clippy/test gates: not claimed as Gate 4 evidence because
  the first mandatory live path is blocked.
