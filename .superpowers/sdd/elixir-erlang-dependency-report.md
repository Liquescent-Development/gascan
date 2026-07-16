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

## Live Retry Follow-up

### RED

The connected build installed Erlang and resolved `mise current erlang`, then
failed with exit 127 because the Docker `RUN` invoked bare `erl` without mise's
runtime environment. New contracts failed until OTP verification and Elixir
installation were both bound to exact Erlang `29.0.3`; the deferred producer
also failed its ordering contract because Elixir remained in its implicit loop.

### GREEN

The Docker build now verifies OTP through `mise exec erlang@29.0.3`, installs
exact Elixir `1.20.2-otp-29` through that same environment, validates both mise
versions, and installs only the six remaining runtimes afterward. The deferred
producer mirrors the explicit Erlang-bound Elixir installation and retains
separate sanitized logs. No live gate was run while implementing this fix.

## OTP Term-Type Follow-up

### RED

The next live retry reached OTP verification but failed because
`erlang:system_info(otp_release)` returns the Erlang string/list `"29"`, while
the Docker and smoke checks used strict equality against binary `<<"29">>`.
Behavioral contracts now accept only the list comparison and reject both the
binary term and an incorrect major.

### GREEN

Docker and image smoke use strict equality with the correct Erlang list value.
The deferred producer already formats that list as text and requires exact
output `29`, so all three validators now share the same OTP-major semantics.
No live gate was run while implementing this correction.
