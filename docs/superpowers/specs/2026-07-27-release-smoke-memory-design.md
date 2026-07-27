# Release Smoke Memory Design

## Problem

The installed-release smoke sandbox is limited to 256 MiB. That was sufficient
when the smoke only exercised lifecycle behavior, but the writable-runtime-home
work added real Cargo, npm, Go, Python, and Ruby package installation. On the
released workspace image, Cargo completes and the Go compiler is then killed by
the sandbox memory limit.

This is a release-test harness defect, not a change to Gascan's runtime defaults
or the published 0.1.10 binaries.

## Decision

Raise only the sandbox created by `packaging/macos/release-smoke.sh` from
256 MiB to 1 GiB. Keep the one-CPU limit and the existing polyglot workload
unchanged. One GiB is deliberately below Gascan's 4 GiB packaged default while
providing enough headroom for the sequential compiler workloads.

Add a source-contract regression test under `scripts/tests/` that reads the
release smoke and verifies:

- the smoke declares a 1 GiB memory limit; and
- the real Go installation workload remains present, so the memory assertion
  cannot silently become detached from the workload that requires it.

The regression test must fail against the current 256 MiB script before the
script is changed.

## Alternatives Rejected

- **Patch a local copy only:** allows today's smoke to pass but leaves the next
  release vulnerable to the same failure.
- **Use the packaged 4 GiB default:** works, but over-allocates for this
  sequential smoke and weakens coverage of constrained configurations.
- **Remove the compiler workloads:** makes the smoke cheaper but loses the
  end-to-end proof that runtime package managers can write, compile, and install
  using managed volumes.

## Verification and Release Impact

Run the focused regression test, the complete scripts test suite, formatting
and diff checks, and then the full installed 0.1.10 release smoke. The final
smoke must print `PASS: installed Gas Can release smoke` and leave no owned
sandbox, volume, DNS, or host-server residue.

This follow-up changes repository release tooling only. It does not change or
republish Gascan 0.1.10.
