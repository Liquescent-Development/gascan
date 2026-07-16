# Elixir Erlang Dependency TDD Report

## Scope

Correct the connected image failure in which Elixir `1.20.2-otp-29` was
installed without an Erlang VM. Erlang/OTP remains an audited implementation
dependency; the product continues to advertise seven user-facing runtimes.

## RED

Added contracts requiring the exact `erlang = "29.0.3"` pin, installation
before the general mise install, OTP-major validation, exact normalized version
evidence, and an `erl` smoke check. The focused test failed at
`dockerfile_installs_pinned_erlang_before_elixir_and_validates_otp_29` because
the pinned prerequisite install was absent.

## GREEN

- Added the exact Erlang pin to the reviewed lock and mise config.
- Installed `erlang@29.0.3` before the general mise installation and verified
  both the exact mise version and OTP release `29`.
- Extended the sealed expected-version map and image smoke without changing the
  seven-runtime product contract.
- Kept the deferred offline bundle producer coherent with the audited map; no
  offline publication was attempted.

## Verification

- Focused tool-version, connected-Dockerfile, polyglot, and bundle tests: PASS.
- Full `scripts` Cargo workspace tests, run serially to avoid the existing
  signal-test fixture contention: PASS. A parallel run had two unrelated
  `apple_build_secret` fixtures fail to reach their blocking state.
- `cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings`: PASS.
- Shell syntax for the changed producer and smoke scripts: PASS.
- `git diff --check`: PASS.

No live image gate, privileged-helper installation, approval marker, or Gate 4/5
claim was performed.

## Independent Review Follow-up

### RED

Two focused producer-contract tests failed because the real production loop did
not install Erlang or create `erlang.log`, and the native ARM64 execution maps
had no Erlang command or validation entry.

### GREEN

The producer now installs exact `erlang@29.0.3` before the seven user-facing
runtimes and records its trace in `erlang.log`. Native verification executes
`erl` without a shell, requires exact OTP-major output `29`, and supplies the
sealed runtime bin directories on `PATH` so Elixir can locate its audited VM.
