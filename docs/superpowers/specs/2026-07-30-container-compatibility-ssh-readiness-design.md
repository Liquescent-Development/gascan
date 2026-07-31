# Apple Container Compatibility and SSH Readiness Design

## Purpose

Gas Can currently treats the one Apple Container release used for its signed
validation matrix as the only usable release. That turns certification
evidence into a permanent exact-version lock and blocks newer compatible 1.x
releases. Runtime evidence is also collected once per daemon lifetime, so
changing Apple Container underneath a same-version `gascand` leaves stale
doctor results and capabilities until the daemon is restarted.

Separately, native SSH activation performs one strict OpenSSH readiness
command, discards its diagnostic output, and reports a generic failure. Doctor
computes more precise managed-state details but the human formatter replaces
them with generic prose. A transient activation failure can therefore leave a
working sandbox while giving the user no useful explanation.

This change separates compatibility from certification, refreshes runtime
evidence, and makes SSH readiness resilient and diagnosable.

## Goals

- Accept coherent Apple Container releases `>=1.1.0, <2.0.0`.
- Continue identifying Apple Container 1.1.0 at revision
  `5973b9cc626a3e7a499bb316a958237ebe14e2ed` as the certified release.
- Warn, without blocking networked sandboxes, when a newer compatible 1.x
  release has not passed Gas Can's validation matrix.
- Keep hard offline isolation fail-closed on an unverified release.
- Refresh runtime evidence and capabilities without requiring a daemon restart.
- Retry strict SSH readiness for a bounded period without weakening host-key
  or identity checks.
- Preserve bounded OpenSSH diagnostics and show precise SSH doctor findings.
- Remove obsolete managed `known_hosts.*` generations after successful
  publication.

## Non-goals

- Supporting Apple Container versions older than 1.1.0.
- Claiming compatibility with Apple Container 2.x before its schemas and
  semantics are reviewed.
- Dynamically recreating the complete signed validation matrix during
  `gascan doctor`.
- Allowing offline sandboxes on an Apple Container release whose isolation has
  not been verified.
- Relaxing ownership, symlink, hard-link, or mode checks on Gas Can-managed
  private SSH state.
- Exposing a sandbox's SSH port beyond host IPv4 loopback.

## Runtime compatibility policy

Gas Can classifies the installed CLI and running API service into one of three
tiers:

1. `certified`
   - CLI version is exactly 1.1.0.
   - CLI release commit is exactly
     `5973b9cc626a3e7a499bb316a958237ebe14e2ed`.
   - API service version and release commit match that certified identity.
   - Existing Gate 2 evidence remains applicable.

2. `compatible_untested`
   - CLI semantic version is `>=1.1.0, <2.0.0`.
   - CLI and API service both report release builds through valid structured
     output.
   - CLI and API service semantic versions match.
   - CLI and API service full release commits match.
   - Their structured schemas contain the fields and internal commit
     consistency Gas Can requires.
   - The identity is not the certified 1.1.0 release.

3. `unsupported`
   - Version is older than 1.1.0 or at least 2.0.0.
   - CLI and API service versions disagree.
   - Required structured output is malformed or internally inconsistent.
   - Either component is not a release build.

The certified tier exposes every signed-off capability, including proven hard
offline isolation. The compatible-but-untested tier exposes ordinary
networked capabilities: bind mounts, named volumes, TTY attachment, signal
forwarding, IPv4 loopback publication, and resource limits. Its offline
capability remains `Unsupported`.

The unsupported tier remains a blocking runtime failure.

## Doctor warning model

`DoctorStatus` gains a first-class `Warning` value. A warning is nonblocking:

- `DoctorReport::is_ready` returns true when every check is either `Pass` or
  `Warning`.
- Runtime readiness ignores warnings and continues to block on failed or
  unknown readiness prerequisites.
- Existing operational-diagnostic role behavior remains unchanged.

The current protobuf `Capability` message remains wire-compatible. A warning
is transported as `available = true` with `"status": "warning"` in its
structured detail. No protobuf field or API-major change is required.

Human output with only passes and warnings:

```text
⚠ Gascan is ready with warnings
  Runtime  10/12 checks passed, 2 warnings
    ⚠ Version
      Apple Container 1.2.0 is compatible but has not been certified by Gascan.
      Tested release: 1.1.0.
    ⚠ Offline
      Hard offline isolation has not been verified with Apple Container 1.2.0.
      Networked sandboxes are available; offline sandboxes are blocked.
```

Warning-only reports exit successfully. JSON emits the exact status string
`"warning"` alongside the existing check ID, detail, and remedy fields.
Failures and unknown readiness prerequisites preserve the existing nonzero
exit behavior.

For a compatible-but-untested release:

- `runtime.version` is a warning naming the installed and certified versions.
- `runtime.service` passes when CLI and API service identities are coherent.
- `runtime.schema` passes when both structured responses satisfy Gas Can's
  supported 1.x schema.
- Ordinary networked capability checks pass.
- `runtime.offline` warns that networked sandboxes work and offline sandboxes
  are blocked.

Requesting an offline sandbox on an untested release returns a focused
unsupported-capability error that names the installed version and explains
that its hard offline isolation has not been verified.

## Live runtime evidence

Production doctor evidence must not be a one-shot daemon-startup future.
`SandboxService` receives a runtime doctor provider that can collect a fresh
report for each request. Test and fake services may retain fixed providers.

The following boundaries refresh evidence:

- Every `gascan doctor` request probes the current CLI and API service.
- Runtime readiness before lifecycle operations uses fresh evidence.
- Runtime capability resolution does not use a daemon-lifetime `OnceCell`.
  It probes the current backend identity before compiling policy.

Each collection remains bounded by the existing 60-second deadline. A timeout
or command/schema error produces the existing fail-closed facts. Replacing,
upgrading, downgrading, stopping, or starting Apple Container therefore takes
effect on the next request without restarting `gascand`.

SSH doctor facts remain refreshed per request as they are today and are merged
into the newly collected runtime report.

## SSH readiness

Gas Can keeps the existing strict readiness command:

- `/usr/bin/ssh`
- explicit `/dev/null` configuration
- host IPv4 loopback and selected host port
- Gas Can-managed identity
- exact managed host-key alias and immutable known-hosts generation
- `StrictHostKeyChecking=yes`
- `IdentitiesOnly=yes`
- `BatchMode=yes`
- forwarding disabled
- remote `/usr/bin/true`

The command is retried within one 15-second absolute deadline. Every attempt
uses the same prepared identity, endpoint, alias, expected host key, and
known-hosts generation. Retry does not regenerate keys, republish config, or
weaken verification.

Attempts use a short bounded delay so connection establishment, native port
publication, sshd startup, and host-key availability can converge. Success on
any attempt completes readiness normally.

Each failed attempt captures stderr. Gas Can retains only a bounded,
UTF-8-safe tail from the final useful attempt. The command's output is never
streamed as unbounded operation progress.

After the deadline, the error includes:

- loopback endpoint,
- elapsed readiness bound,
- bounded OpenSSH detail when present,
- and `Run \`gascan doctor\` for managed SSH configuration details.`

If the command cannot start or times out without diagnostic output, the error
states that precise condition instead of reporting only that the command
failed.

## SSH doctor output

The human formatter stops replacing `ssh.identity` and `ssh.config` details.
It prints the bounded detail already produced by the daemon, including the
managed path and exact missing, unsafe, or inconsistent invariant.

The remedy remains concise but becomes state-oriented: reconcile with
`gascan up` when generated state is absent or inconsistent, and remove or
repair only the specifically identified unsafe managed path when safety
validation fails. Conventional user-owned `~/.ssh/config` modes such as 0644
remain accepted.

The operation error and doctor report complement each other: the operation
reports the final OpenSSH failure, while doctor reports durable identity,
publication, configuration, and runtime capability state.

## Managed generation cleanup

Known-hosts files remain immutable, content-addressed generations. After a
new configuration has been durably committed and validated:

1. Resolve the generation referenced by the committed managed config.
2. Retain that active generation.
3. Retain any generation explicitly needed by an in-progress rollback.
4. Remove other regular, user-owned, correctly named managed
   `known_hosts.<sha256>` files.
5. Refuse to follow links or remove paths that fail managed-state validation.
6. Sync the managed directory after cleanup.

Cleanup failure does not invalidate an otherwise successful SSH publication.
It is retried during the next successful publication or reconciliation.
Doctor reports otherwise-valid obsolete generations as a nonblocking SSH
configuration warning with their count and managed directory. Cleanup never
runs before the new configuration is durable.

## Error handling

- Unsupported versions, mismatched CLI/service identities, malformed schemas,
  and unverified offline requests remain typed runtime errors.
- Compatible version warnings never become generic `backend_unavailable`
  failures for networked sandboxes.
- SSH errors preserve stable error codes while improving their human cause.
- Diagnostic text is bounded and must not include private-key contents,
  environment dumps, or untrusted unbounded output.
- JSON operation output remains machine-readable and does not receive spinner
  or incidental stderr text.

## Testing

Runtime tests cover:

- certified 1.1.0 and exact revision;
- compatible 1.1.x and 1.2.x release identities;
- versions older than 1.1.0;
- version 2.x;
- CLI/service version mismatch;
- malformed or non-release structured output;
- ordinary capabilities on an untested 1.x release;
- offline capability rejection naming the installed version;
- doctor warning serialization, rendering, counts, and successful exit;
- failed/unknown readiness behavior remaining blocking;
- runtime upgrade and downgrade becoming visible without restarting the
  daemon;
- capability refresh after backend replacement.

SSH tests cover:

- first-attempt success;
- transient connection failures followed by strict success;
- permanent command failure with bounded stderr;
- timeout and spawn failures with distinct messages;
- UTF-8-safe diagnostic truncation;
- exact identity, alias, generation, and strict arguments on every retry;
- detailed human and JSON doctor output;
- active generation retention;
- obsolete generation removal after durable publication;
- nonblocking doctor warnings and later retry after cleanup failure;
- symlink, foreign-owner, malformed-name, and rollback safety.

Connected Apple validation covers a networked sandbox on the certified release
and preserves the existing offline proof. A compatible untested-release
fixture proves warning behavior without claiming new signed isolation
evidence.

## Delivery

Runtime compatibility/refresh and SSH readiness/diagnostics remain separate
implementation units and test groups on one feature branch. After verification
they ship together in one patch release. The release notes call out:

- support for compatible Apple Container 1.x releases,
- warnings for untested releases,
- offline restrictions on unverified releases,
- automatic runtime evidence refresh,
- and actionable SSH readiness diagnostics.
