# Daemon Management Final Fix-Wave Report

Date: 2026-07-28

## Status

All eight Important findings in `final-review-findings.md` are fixed in one
consolidated wave. The uptime minor and direct-daemon protected-record triage
are also complete. Descriptor-anchored caller-CWD hardening remains deferred,
and Linux pidfd compile/runtime proof remains a nonblocking CI follow-up, as
directed by the findings.

The fix wave started from:

- `809c985f2032737f6c8bb664f716ee41f46531c8`
  (`style: format compatibility test`)

Implementation commit:

- `8233136356dfeb584c2d2831f3bd0ef9238deb9b`
  (`fix: close daemon lifecycle review findings`)
  - Good Git ED25519 signature for `richard@liquescent.dev`
    (`SHA256:MHX9nK/wmGEjnl+VuGpYjNhg5pS8ZET8PbbOxRI8o0c`).

The requested Superpowers 6.1.1 skill directory was not available locally.
The installed 6.2.0 equivalents were used for receiving review, systematic
debugging, TDD, verification-before-completion, and pre-commit code review.
This internal code review is not the controller's official post-commit scoped
re-review.

## Finding resolution

### Important 1: authenticate the responding endpoint path

- Every endpoint probe begins with descriptor-anchored validation of the
  private runtime directory and socket type, effective UID, exact `0600` mode,
  link count, and device/inode.
- The exact prechecked device/inode is passed into the production transport
  connector. After Unix connect, the connector validates that exact pathname
  identity before returning the stream to tonic, so no HTTP/2 preface or RPC
  is sent to a substituted socket.
- The outer probe verifies stable pathname identity after every probe outcome,
  not only successful RPC completion.
- Unix peer credentials require the effective UID. Where the platform
  supplies a peer PID, it must agree with the daemon handshake PID.
- The hidden `daemon-attest` compatibility command now derives its identity
  through the same authenticated supervisor inspection and accepts only
  attested Current or Outdated state.
- Adversarial coverage rejects a symlink without invoking the endpoint,
  rejects a socket replaced during a probe without shutdown RPC or signal,
  captures an HTTP/2 preface in RED and proves zero protocol bytes in GREEN
  after a safe socket replacement, and proves symlinked `daemon-attest` also
  sends zero protocol bytes.

### Important 2: installed executable and fresh launch token

- A protected record can no longer bypass the adjacent installed executable
  requirement. Endpoint, record, and process identities must all name the
  trusted installed pathname.
- Readiness retains the fresh owner token generated for the launch and accepts
  only a protected record carrying that exact token.
- A missing record beside an already-bound endpoint is treated as transient
  publication and retried; a present wrong token still fails immediately.
- Path equality intentionally preserves legitimate macOS predecessor recovery
  when Brew-style replacement changes the vnode at the same trusted pathname.

### Important 3: serialize every non-Current observation

- Healthy Current remains the only lock-free fast path.
- Every other initial observation acquires the lifecycle lock, re-inspects,
  and decides from post-lock state.
- Publication and shutdown contender tests wait for the first transient probe
  before changing state, so they deterministically prove the client blocks and
  converges rather than false-passing on scheduler timing.

### Important 4: stable macOS process start identity

- Daemon, CLI, and E2E process-start inspection use absolute `/bin/ps` with
  `LC_ALL=C`, `LANG=C`, and `TZ=UTC`.
- A macOS E2E launches under one locale/time zone and verifies the same daemon
  remains healthy and current under two different caller environments.

### Important 5: malformed or contradictory release identity

- Wire conversion preserves release and start timestamp presence
  independently, allowing parity contradictions to be rejected.
- Present endpoint and protected-record versions must parse as SemVer.
- Malformed endpoint versions classify Unhealthy; malformed records classify
  Unsafe. They never become Outdated or eligible for automatic recovery.
- Legacy behavior is retained only for the exact both-absent pattern.

### Important 6: reliable attested explicit force

- Explicit force retains the absolute graceful deadline across attested
  transport, internal, and service failures, then executes the existing
  re-attested force path.
- Permission-denied, unauthenticated, token, API-authentication, and identity
  failures remain fail-closed and never force.
- Automatic recovery remains non-forcing.

### Important 7: actual installed-file replacement

- The macOS E2E copies the daemon to a stable installed-style pathname, starts
  an outdated predecessor from it, records the vnode, atomically renames the
  current fixture over that pathname, and proves the inode changed.
- An ordinary Doctor invocation then proves predecessor exit, healthy current
  replacement readiness, exact stable-path attestation, a different PID, and
  one live daemon.
- Failure cleanup records the active installed daemon path. The test proves
  Drop cleanup terminates the replacement and only permits fallback KILL after
  re-attesting PID, start identity, exact text executable, and start identity
  again.
- The production behavior already handled vnode replacement correctly; this
  finding required real coverage rather than another production change.

### Important 8: shutdown docs and command-aware guidance

- README and its release contract now state that graceful shutdown drains
  durable sandbox operations and then cancels attachment streams.
- A graceful timeout from `daemon stop` suggests
  `gascan daemon stop --force`; `daemon restart` preserves restart intent;
  ordinary automatic recovery suggests an explicit forced restart.
- Stable error code `daemon_graceful_shutdown_timeout` is preserved.

### Minor triage

- JSON uptime clamps future start timestamps to zero.
- Direct daemon startup without launcher record environment now publishes the
  standard protected record with a fresh 64-character lowercase hexadecimal
  owner token.
- The exact debug-only recordless legacy-wire fixture remains available for
  compatibility E2E; release builds always use standard publication.

## RED/GREEN evidence

| Area | Focused command | RED observation | GREEN |
| --- | --- | --- | --- |
| Endpoint symlink | `rtk proxy cargo test -p gascan classification_rejects_a_responding_endpoint_reached_through_a_symlink -- --nocapture` | Responding symlink classified Current instead of Unsafe | PASS |
| Endpoint replacement | `rtk proxy cargo test -p gascan stop_rejects_a_safe_socket_replaced_during_probe_without_rpc_or_signal -- --nocapture` | Expected fail-closed Unsafe result was not returned | PASS; zero shutdown RPCs/signals |
| Pre-HTTP2 replacement | `rtk proxy cargo test -p gascan tonic_probe_sends_no_protocol_bytes_after_the_socket_precheck_is_invalidated -- --nocapture` | Captured the 64-byte HTTP/2 preface/settings on the replacement | PASS; zero bytes |
| Hidden attestation | `rtk proxy cargo test -p gascan-e2e --test autostart daemon_attest_rejects_a_symlink_without_sending_protocol_bytes -- --exact --nocapture` | Captured the same HTTP/2 preface through the symlink | PASS; zero bytes |
| Installed executable | `rtk proxy cargo test -p gascan classification_protected_record_cannot_bypass_the_installed_executable -- --nocapture` | Forged protected record classified Current | PASS; Unhealthy |
| Fresh launch token | `rtk proxy cargo test -p gascan start_readiness_rejects_a_record_with_the_wrong_launch_owner_token -- --nocapture` | Wrong launch owner token was accepted | PASS |
| Publication gap | `rtk proxy cargo test -p gascan start_readiness_waits_when_the_endpoint_precedes_its_record -- --nocapture` | Bound endpoint with no record failed immediately as a token mismatch | PASS; waits for matching record |
| Gated publication contender | `rtk proxy cargo test -p gascan connect_waits_for_gated_publication_then_converges_on_current -- --nocapture` | Client rejected before lifecycle serialization | PASS |
| Shutdown contender | `rtk proxy cargo test -p gascan connect_waits_for_shutdown_contender_then_converges_on_current -- --nocapture` | Client rejected before lifecycle serialization | PASS |
| Locale/TZ identity | `rtk proxy cargo test -p gascan-e2e --test autostart daemon_start_identity_is_stable_across_caller_locale_and_timezone -- --exact --nocapture` | Healthy daemon became unhealthy under a different caller TZ | PASS |
| Timestamp parity | `rtk proxy cargo test -p gascan endpoint_wire_identity_preserves_legacy_and_exact_release_versions -- --nocapture` | Timestamp-without-release was erased | PASS; contradiction preserved |
| Malformed SemVer | `rtk proxy cargo test -p gascan classification_malformed -- --nocapture` | Malformed endpoint/record releases became Outdated | PASS; Unhealthy/Unsafe |
| Force transport error | `rtk proxy cargo test -p gascan stop_explicit_force_survives_attested_transport_and_internal_rpc_errors -- --nocapture` | Returned `Client(Io(ConnectionReset))` before force | PASS; re-attested KILL |
| Force auth refusal | `rtk proxy cargo test -p gascan stop_explicit_force_never_bypasses_shutdown_token_authentication -- --nocapture` | Safety characterization | PASS; no signal |
| Installed vnode replacement | `rtk proxy cargo test -p gascan-e2e --test doctor doctor_recovers_after_atomic_installed_daemon_replacement -- --exact --nocapture` | Production already passed; missing real coverage | PASS |
| Replacement cleanup | same command | Drop left replacement daemon live | PASS; attested cleanup |
| Documentation | `rtk bash tests/release/documentation-contract.sh` | Required drain-then-cancel text missing | PASS |
| Command guidance | `rtk proxy cargo test -p gascan daemon_graceful_timeout_guidance_preserves_the_requested_command -- --nocapture` | Context helper/API did not exist | PASS |
| Future uptime | `rtk proxy cargo test -p gascan daemon_json_uptime_clamps_future_start_timestamps_to_zero -- --nocapture` | JSON emitted `-1000` | PASS; emits `0` |
| Direct record | `rtk proxy cargo test -p gascan-e2e --test autostart direct_daemon_startup_publishes_the_standard_protected_record -- --exact --nocapture` | Standard record path was absent | PASS |

During complete verification, the preexisting idle-restart E2E's 50 ms idle
timeout expired under parallel process-inspection load. Its purpose is restart
after observed idle exit, not a 50 ms scheduling guarantee. The test now uses
a bounded two-second idle policy, waits for actual socket retirement, and
prints initial command diagnostics. The focused test and complete suite pass.

## Final verification

| Command | Result |
| --- | --- |
| `rtk cargo fmt --all -- --check` | PASS |
| `rtk cargo clippy --workspace --all-targets -- -D warnings` | PASS; no issues |
| `rtk cargo test -p gascan` | PASS; 196 tests |
| `rtk cargo test -p gascan-e2e --test autostart -- --nocapture` | PASS; 12 tests |
| `rtk cargo test -p gascan-e2e --test doctor -- --nocapture` | PASS; 11 tests |
| `rtk cargo test --workspace --all-targets` | PASS; 1,069 passed, 21 ignored, 167 filtered across 54 suites |
| `rtk cargo check --release -p gascan-e2e --bins` | PASS; debug fixture hooks absent in release-mode binaries |
| `rtk bash tests/release/documentation-contract.sh` | PASS |
| `rtk bash tests/release/installer-contract.sh` | PASS; intentional `sleep 1000` fixture termination reported |
| `rtk bash tests/release/clean-host-contract.sh` | PASS |
| `rtk git diff --check` | PASS |

An independent whole-diff audit found and prompted repair of the installed
replacement cleanup path and deterministic contender gating. A separate
internal pre-commit review found the pre-HTTP2 ordering and hidden
`daemon-attest` gaps; both were repaired with the RED/GREEN evidence above.
The follow-up internal review reported no remaining Critical, Important, or
Minor findings.

## Residual follow-ups

- Descriptor-anchored caller-CWD validation remains explicitly deferred.
- Linux pidfd compile/runtime proof remains a nonblocking CI follow-up because
  the documented product target is macOS.

No other known implementation concern remains.
