# Actionable request errors and client-side path resolution

## Problem

Two defects in 0.1.3 combine to make a mistyped manifest or a relative path
nearly undiagnosable.

`gascan up .` fails with `daemon error: invalid_request`. The CLI sends its
`project_root` argument verbatim (`crates/gascan/src/cli.rs:139`) and the daemon
rejects any non-absolute root (`crates/gascand/src/api.rs`, `spec_for_root`).
`apply` does not have this problem only because it substitutes
`current_dir()` when its argument is omitted; passing `apply` a relative path
fails the same way.

A manifest containing `user = "kiener"` fails with the same
`daemon error: invalid_request`. The daemon has the precise cause --
``unknown variant `kiener`, expected `workspace` or `root``` -- and discards it:
`spec_for_root` maps three distinct failures through
`map_err(|_| ApiInputError::Invalid)`.

Because both defects report one opaque code, they are indistinguishable. A user
who fixes the manifest still sees the identical message from the path bug and
reasonably concludes the fix did not work.

This is a regression from the original design, not a deliberate constraint.
`docs/superpowers/specs/2026-07-13-macos-sandbox-design.md` already requires:

> Human CLI messages include a concise remedy; structured clients receive stable
> error codes and fields.

and lists "path canonicalization" among the behaviors unit tests must cover.

## Scope

Request-validation failures for `up` and `apply`: manifest parse/validation and
project-root resolution.

Runtime, policy, and sandbox errors keep their current coarse mapping. The
design reserves *sanitized* diagnostics for those, and they are state-dependent,
so each needs its own judgement about what is safe and useful to reveal. Nothing
here widens what a runtime failure reveals.

## Design

### Path resolution belongs to the client

`.` means the *client's* working directory. The daemon runs with a different
one, so resolving a relative root daemon-side would silently resolve against the
wrong directory. Since the mounted workspace is derived from that root, this is
both a correctness bug and a containment hazard.

Therefore:

- The CLI resolves `project_root` with `std::fs::canonicalize` before sending,
  for both `up` and `apply`.
- The daemon keeps rejecting non-absolute roots, unchanged. That check is not
  the defect; it is the boundary working. It becomes defense in depth.

Sandbox identity is unaffected. `SandboxSpec::from_root` already calls
`std::fs::canonicalize`, so resolving client-side first is idempotent: a
relative and an absolute path to the same project yield the same `SandboxId`.
Existing sandboxes need no migration.

### Errors carry their cause

The `v1::Error` message has always had the necessary shape and has never been
populated:

```protobuf
message Error {
  string code = 1;
  string message = 2;
  bytes details = 3;
}
```

Today the daemon puts its stable code in the tonic `Status` *message*
(`Status::invalid_argument(INVALID_REQUEST)`) and the CLI prints that verbatim,
so `Status.message` doubles as the machine-readable code. Repurposing it for
human text would break every consumer that reads the code from it.

Instead the change is strictly additive:

- Two codes join `error_code::ALL`: `invalid_manifest` and
  `invalid_project_root`. The compatibility test asserts presence and
  uniqueness, not an exact set, so both still hold.
- `API_MINOR` moves 0 -> 1, the honest signal for an additive change. No test
  pins it.
- `ApiInputError` gains a variant carrying the human message.
- Each failure in `spec_for_root` maps to exactly one code:

  | Failure | Code |
  | --- | --- |
  | Empty or non-absolute `project_root` | `invalid_project_root` |
  | Root missing, not a directory, or not canonicalizable | `invalid_project_root` |
  | Root name cannot be derived | `invalid_project_root` |
  | `Manifest::load` parse or validation failure | `invalid_manifest` |
  | `SandboxSpec::from_root` manifest validation failure | `invalid_manifest` |

  `invalid_project_root` remains reachable after the CLI resolves paths: the CLI
  is one client of a public local API, and any other client may still send an
  empty or relative root. The daemon does not assume a well-behaved caller.
- The daemon returns
  `Status::with_details(InvalidArgument, <code>, encode(v1::Error{code, message}))`.
  The code stays exactly where it has always been.
- `crates/gascan/src/client.rs` decodes `details` when present and prints the
  message; otherwise it falls back to the current bare code.

tonic 0.12.3 serializes details to the `grpc-status-details-bin` header and
parses them back, so this survives the local socket transport.

The daemon composes the full human text, including the remedy, so the CLI stays
a renderer and the remedy lives beside the code that knows what failed.

### Resulting output

Replacing `daemon error: invalid_request`:

```
error: invalid manifest at /Users/kiener/code/gascan.toml
  unknown variant `kiener`, expected `workspace` or `root`
  remedy: set user to "workspace" or "root"
```

### Version skew

A new CLI against an old daemon receives no details and prints today's output. An
old CLI against a new daemon reads `Status.message` and finds the code where it
has always been. Neither combination breaks, which is what makes this additive
rather than a v1 contract change.

## Testing

Regression tests for both reported defects come first and must fail before the
fix:

- `gascan up .` from a project directory resolves and succeeds.
- A manifest with `user = "kiener"` yields a message naming the field, the
  rejected value, and the valid set.

Unit tests:

| Area | Cases |
| --- | --- |
| CLI path resolution | `.`, `..`, nested relative, trailing slash, symlinked root, already-absolute unchanged |
| Nonexistent path | Clear client-side error, no daemon round-trip, no panic |
| Empty argument | Rejected with a message |
| Daemon strictness | Still rejects a relative root |
| Sandbox identity | Relative and absolute paths to one project produce the same `SandboxId` |
| Error plumbing | `Error{code,message}` survives encode -> `with_details` -> `details()` -> decode |
| CLI rendering | Details present prints the message; absent falls back to the code; malformed falls back and never panics |
| API compatibility | New codes present in `ALL`; all codes unique |

## Out of scope

- Reclassifying runtime, policy, or sandbox errors.
- Changing what runtime failures reveal.
- Any proto schema change; the fields already exist.
