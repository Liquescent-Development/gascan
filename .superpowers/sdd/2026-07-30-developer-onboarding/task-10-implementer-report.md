# Task 10 Implementer Report

## Status

Implementation and every non-privileged Task 10 gate are complete. The required
branch-built live release smoke is blocked before product execution because the
host has no current noninteractive sudo credential. Per the controller boundary,
the Task 10 commit has not been created while that required gate remains pending.

## Implementation

- `README.md` now documents the optional first-`up` onboarding flow, complete
  and focused configure commands, global-only Git import, sandbox SSH signing,
  hidden and stdin token paths, SSH/HTTPS behavior, enterprise hosts, native
  credential locations, persistence, offline retries, verification, destroy
  cleanup, and independent forge-key revocation.
- `packaging/macos/release-smoke.sh` accepts
  `GASCAN_RELEASE_GASCAND`, exports it through `GASCAN_DAEMON`, and uses the same
  binary for attested shutdown.
- The smoke creates isolated fake `gh` and `glab` CLIs, configures imported Git
  identity and a sandbox key, proves the GitHub double registration and GitLab
  `auth_and_signing` request, creates and verifies a signed commit and tag, and
  checks identity, key, native credentials, and signed-repository persistence
  after apply and down/up.
- Standard, Starship, and Nerd Font shell probes now cover a nested interactive
  login Bash. Starship modes require the expected executable, config, and hook
  with no warning.
- A security self-review found that the PTY initially copied the complete host
  environment. The smoke now re-executes once through `env -i` with a minimal
  allowlist for paths, user identity, test controls, and the two branch-binary
  overrides. Real host forge credential variables are neither named nor made
  available to the smoke or its child processes.
- The existing smoke command fixture now opts into `GASCAN_RELEASE_TESTING=YES`
  so the clean-environment prelude retains only its fake-command PATH and
  non-secret DNS bookkeeping. Production execution still uses the fixed PATH.

## TDD evidence

The initial focused contract run was RED as intended:

```text
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke -- --nocapture
test result: FAILED. 1 passed; 3 failed
```

The failures required the README contract, matching daemon binding, and the
fake-forge/signing/persistence smoke. Each focused contract passed after its
implementation, followed by the complete target:

```text
cargo test: 4 passed (1 suite, 0.00s)
```

The clean-environment security regression was separately demonstrated RED:

```text
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke \
  release_smoke_proves_fake_forge_signed_git_and_persistence_without_host_tokens \
  -- --nocapture
test result: FAILED. 0 passed; 1 failed; 3 filtered out
```

After adding the allowlisted `env -i` re-exec, the same focused test passed
`1 passed, 3 filtered out`; the full four-test target, shell syntax check, and
release-smoke contract also passed. The release smoke command and signal
contracts passed against the explicitly marked testing environment.

The user's first privileged live rerun then exposed a separate controlling-TTY
regression. Although `env -i` removed host variables, it correctly retained the
process file descriptors, so every direct `gascan up` still saw terminal stdin
and stderr. The post-up onboarding prompt consumed the user's answers and
configured the ephemeral sandbox before the smoke reached its isolated host-Git
fixture; the later exact identity assertion therefore failed.

A focused source regression began RED with `0 passed, 1 failed, 4 filtered
out`. The smoke now has one `gascan_release_up` helper that invokes
`"$gascan_bin" up "$root" </dev/null`. All four initial, existing-sandbox,
restart, and offline calls route through the helper, and the contract proves no
other direct `gascan up` remains. The focused regression then passed, followed
by the complete target at `5 passed`.

The user's next live rerun suppressed onboarding and exposed a production forge
execution failure: `configure gh` returned `native authentication did not
complete`. A minimal disposable live sandbox separated the runner boundaries
without custom DNS, sudo, or real tokens:

- raw guest execution reported
  `PATH=/home/workspace/.local/bin:...`, `HOME=/home/workspace`, and
  `GH_CONFIG_DIR=/home/workspace/.config/gascan/gh`;
- login Bash and direct `gascan run -- gh --version` both selected
  `/home/workspace/.local/bin/gh` and printed the fake marker;
- the exact GitHub login argv run manually exited zero and logged
  `stdin=<fake-token-match>`;
- branch `configure gh` exited 70, did not enter the fake process at all, and
  left `hosts.yml` absent.

The first exit boundary was the daemon API's environment validation. Forge commands
request `GH_NO_UPDATE_NOTIFIER` or `GLAB_CHECK_UPDATE` plus `NO_COLOR`, while
`filtered_host_environment` previously accepted only terminal/locale keys.
The daemon therefore rejected the command before creating the guest session;
PATH, argv, and stdin were not the cause.

A policy regression began RED with `0 passed, 1 failed, 30 filtered out`. The
minimal fix allowlists only `GH_NO_UPDATE_NOTIFIER`, `GLAB_CHECK_UPDATE`, and
`NO_COLOR`. The same test explicitly submits `GH_TOKEN`, `GITHUB_TOKEN`,
`GLAB_TOKEN`, and `GITLAB_TOKEN` and proves all four secret sources remain
filtered. The focused regression then passed, followed by all 31 policy tests,
300 Gascan tests, 367 daemon tests, strict affected-core clippy, the five-test
macOS smoke contract target, and the complete release-contract loop.

The user's subsequent rerun passed API admission but returned the same public
authentication failure. The exact controls were still rejected by two deeper
defense layers: Rust `gascan-apple` attachment validation and the Swift helper
protocol. Direct Apple execution with those two overrides proved that the guest
defaults—including PATH, HOME, XDG paths, and native forge config paths—are
preserved, and the Swift overlay implementation has the same merge behavior;
environment replacement was falsified.

Focused Rust bridge and Swift overlay tests each began RED at their respective
validation layer and passed after both adopted the same three-key allowlist.
Their invalid-environment fixtures explicitly retain all four forge token names
as forbidden. Full verification then passed 112 Rust Apple tests with 11 live
tests ignored, all 11 Swift helper tests, strict Rust Apple clippy, five macOS
release-smoke contract tests, and the complete release-contract loop.

Because this fix changes both the linked Rust bridge and the Swift executable,
release smoke now accepts `GASCAN_RELEASE_APPLE_ATTACH_HELPER`, defaults it to
the installed helper, preserves it through the clean-environment re-exec,
requires an executable canonical path, and exports it as
`GASCAN_APPLE_ATTACH_HELPER` before daemon launch. Its focused contract began
RED and passed after the override was wired. The branch helper was built at
`target/gascan-apple-attach`; nothing was installed or overwritten.

The next matching-helper live run then passed GitHub and GitLab native
authentication, all required key registrations, and signed commit/tag creation
and verification. It stopped only at the smoke's raw tag-object assertion:
guest GNU grep interpreted the leading-hyphen pattern
`-----BEGIN SSH SIGNATURE-----` as an option and exited 2. A focused regression
began RED with `0 passed, 1 failed, 4 filtered out`; the single applicable
assertion now uses `grep -F --`. A scan found no other release-smoke grep pattern
whose fixed pattern begins with a hyphen. The regression then passed, followed
by all five macOS release-smoke contract tests, the complete release-contract
loop, Bash syntax, formatting, and diff checks. The live smoke was not rerun by
the implementer.

The following live run advanced through the signed Git checks and Starship
version, then exited silently in the standard-shell assertion loop. A disposable
offline sandbox with the default shell reproduced the exact embedded PTY probe.
The normalized values were correct, including `INTERACTIVE=yes`, `LOGIN=yes`,
`SHELL=/bin/bash`, and `SELECTOR=standard`, but every direct field was prefixed
by the interactive `workspace@...$` prompt; the multiline completion command
also emitted `> ` continuation prompts. Only the last two nested-shell fields
matched as exact lines. This was a smoke-probe defect, not a product-shell
defect. A focused diagnostics contract began RED with `0 passed, 1 failed`.
The probe now clears `PS1` and `PS2` before its begin marker, and shell-field
assertions report the selector, field, expectation, and at most the last 4096
captured characters. The exact extracted probe then returned thirteen clean
fields and every standard-shell assertion passed. The focused test passed,
followed by all six macOS release-smoke contract tests and the complete release
contract loop.

The next live run passed forge configuration, signed Git verification, the
standard shell, apply, restart, and post-restart signature verification before
another apparently silent exit. A disposable no-DNS sandbox reproduced the
entire credential lifecycle. After configure, apply, and down/up, both fake
CLIs still resolved below `/home/workspace/.local/bin`; both forge directories
were correct; both configuration files existed at mode 600; both fake auth
status commands exited 0; the private-key checksum and Git identity matched;
and the token scan exited 1 as required. The exact uninstrumented persistence
block exited 0. The first failing command was the following
`gascan_stop_attested_daemon`, which exited 1 without output.

Predicate-level tracing showed matching PID, executable, 64-character instance
token, and second attestation. Only the start identity differed: gascand records
process identity with deterministic `LC_ALL=C`, `LANG=C`, and `TZ=UTC`, yielding
`Sat Aug  1 18:10:52 2026`, while release-common used the Phoenix-local host
environment and observed `Sat Aug  1 11:10:52 2026`. The helper now applies the
same deterministic environment to all three `ps` calls. Its installer contract
began RED by recording six `ps-env:C:C:America/Phoenix` invocations and then
passed with every invocation normalized to UTC. A clean fresh-daemon
stop/restart retained its SQLite sandbox row and both status and list returned
the running sandbox, disproving a general state-persistence defect.

The final credential block now also names every failed field or command,
redacts the fixed fake token, bounds safe output to 4096 characters, and never
prints token-scan matches. Its focused contract began RED with `0 passed, 1
failed` and passed after implementation. A runtime failure probe preserved exit
7, named `diagnostic.failure`, emitted `[REDACTED]`, and contained no fixture
token. Self-review caught and corrected the need to capture command status in
the `else` branch; that focused contract also began RED and returned GREEN.
All seven macOS release-smoke contract tests and the full scripts suite then
passed.

All disposable forge sandboxes were destroyed. The final inventory contains
only `code-3fd063e3b68e`, `buildkit`, and the three volumes belonging to the
pre-existing code sandbox; DNS is empty and all temporary roots are absent.
The disposable shell-probe and persistence sandboxes were also destroyed, and
their temporary roots were removed. The matching branch daemon is stopped so
the next smoke starts it fresh with the branch helper override. The installed
daemon was not touched.

## Exact Task 10 verification

Step 3:

- `rtk cargo fmt --all -- --check` — pass.
- `rtk cargo test -p gascan` — 300 passed, 264 filtered out across 7 suites in
  4.05 seconds. The first restricted run was interrupted after its process
  fixtures stalled silently; the authoritative permitted rerun passed.
- `rtk cargo test --manifest-path scripts/Cargo.toml` — 508 passed across 52
  suites in 178.41 seconds with local process access. The restricted first run
  reached only the expected `Operation not permitted` denial in two loopback
  HTTP fixtures.
- `rtk bash tests/image/shell-home-root-contract.sh` — direct macOS invocation
  rejected its context with `contract must run as real root`; line 19 requires
  real root before the Linux image fixture mutates `/etc`, `/opt`, and the
  workspace account. This contract was not weakened or bypassed.
- `rtk bash images/workspace/tests/workstation-contract.sh` — direct macOS
  invocation rejected its context with
  `locked version evidence is unavailable`; it requires sealed files below
  `/opt/gascan`. As Task 10's brief directs, the authoritative Task 9 public
  `run-connected-image-gate.sh --prebuilt` evidence is used instead. Task 9
  records the public immutable gate passing with `workstation-contract-ok`.
- `rtk bash tests/release/release-smoke-contract.sh` — pass.
- `rtk git diff --check` — pass.

Step 4:

- `rtk cargo test --locked --workspace --all-targets` — 1,259 passed, 22
  ignored, 264 filtered out across 55 suites in 103.65 seconds.
- `rtk cargo clippy --locked --workspace --all-targets -- -D warnings` — pass,
  no issues.
- `rtk bash -c 'for c in tests/release/*-contract.sh; do bash "$c" >/dev/null || exit; done'`
  — pass with Homebrew cache access. The restricted first run failed only
  because `/opt/homebrew/tmp` was denied.
- `rtk cargo build -p gascan -p gascand` — pass; both crates compiled.
- `rtk env GASCAN_RELEASE_GASCAN="$PWD/target/debug/gascan" GASCAN_RELEASE_GASCAND="$PWD/target/debug/gascand" GASCAN_RELEASE_APPLE_ATTACH_HELPER="$PWD/target/gascan-apple-attach" ./packaging/macos/release-smoke.sh`
  — blocked before `gascan up` at the pre-existing
  `sudo -n container system dns create` call with
  `sudo: a password is required`.

`rtk sudo -n true` independently returns the same password-required result.
`git show HEAD:packaging/macos/release-smoke.sh` proves the sudo DNS create and
delete calls predate Task 10. After the failed attempt, Apple DNS inventory was
the empty array and Apple container inventory contained only the pre-existing
`code-3fd063e3b68e` user sandbox and `buildkit`; no Task 10 resource remained.

After the user's reproduced onboarding failure, the fresh residue audit again
found DNS inventory `[]`; container inventory contained only
`code-3fd063e3b68e` and `buildkit`; and volume inventory contained only the
three volumes owned by `code-3fd063e3b68e`. `/private/tmp/gascan-501` was
absent. No cleanup action was necessary, and no pre-existing resource was
touched.

## Final handoff state

After an authorized user primes the host credential with interactive
`sudo -v`, rerun from the developer-onboarding worktree:

```sh
rtk env GASCAN_RELEASE_GASCAN="$PWD/target/debug/gascan" \
  GASCAN_RELEASE_GASCAND="$PWD/target/debug/gascand" \
  GASCAN_RELEASE_APPLE_ATTACH_HELPER="$PWD/target/gascan-apple-attach" \
  ./packaging/macos/release-smoke.sh
```

The final workspace, scripts, Swift helper, clippy, release-contract, formatting,
syntax, and diff checks passed before staging. The live smoke itself was not
rerun after the UTC attestation correction; the command above is the next
end-to-end confirmation.

Current changed files are:

- `README.md`
- `crates/gascan-core/src/policy.rs`
- `crates/gascan-core/tests/policy.rs`
- `crates/gascan-apple/src/attach.rs`
- `crates/gascan-apple/tests/attach_configuration.rs`
- `crates/gascan-apple/tests/backend_fake_runner.rs`
- `crates/gascan-apple/tests/fixtures/fake-attach-helper/src/main.rs`
- `helpers/apple-attach/Sources/GasCanAppleAttach/Protocol.swift`
- `helpers/apple-attach/Tests/GasCanAppleAttachTests/ProtocolTests.swift`
- `packaging/macos/release-common.sh`
- `packaging/macos/release-smoke.sh`
- `scripts/tests/macos_release_smoke.rs`
- `tests/release/installer-contract.sh`
- `tests/release/smoke-contract.sh`
- `.superpowers/sdd/2026-07-30-developer-onboarding/task-10-implementer-report.md`

The report is ignored by repository policy and therefore requires explicit
force-add at commit time.
