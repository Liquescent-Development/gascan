# Configurable Managed Storage Design

## Goal

Give every new Gas Can sandbox enough persistent storage for realistic
development tools, caches, and configuration while allowing each capacity to
be configured independently. Provisioning must also preserve useful command
failures when an installer emits large amounts of output.

## Problem

Gas Can currently creates each Apple Container managed volume with a hardcoded
capacity of 100 MiB. That is insufficient for ordinary development tools. A
mise installation containing Codex, Claude Code, Pi, Neovim, and Herdr can
exhaust the tool volume during installation.

The provisioning executor also treats more than 1 MiB of combined stdout and
stderr as a fatal error. A verbose failing installer can therefore cause Gas
Can to stop consuming the guest execution stream and report only:

```text
guest provisioning output exceeded its limit
```

This hides the command's terminal diagnostic, such as `ENOSPC: no space left
on device`, and can leave the guest command running after the control plane has
reported failure.

## Manifest

Add an optional top-level `[storage]` table:

```toml
[storage]
tools = "10GiB"
cache = "10GiB"
config = "1GiB"
```

The fields are independent and optional:

| Field | Default | Guest mount |
| --- | ---: | --- |
| `tools` | `10GiB` | `/home/workspace/.local/share/mise` |
| `cache` | `10GiB` | `/home/workspace/.cache` |
| `config` | `1GiB` | `/home/workspace/.config/gascan` |

Storage sizes use the existing binary-size syntax: a positive integer followed
by `KiB`, `MiB`, `GiB`, or `TiB`. Zero, decimal units, bare integers,
overflowing values, unknown fields, and values greater than `512GiB` per
volume are rejected during manifest validation.

Omitting `[storage]` preserves the documented defaults rather than the prior
100 MiB implementation detail. Partial tables apply the default independently
for every omitted field.

## Architecture and Data Flow

The manifest exposes a validated storage value with one capacity per managed
volume. The policy compiler copies these exact byte capacities into the
sandbox's runtime volume specifications:

- `tools` maps to the existing `gascan-mise-<sandbox-id>` volume.
- `cache` maps to the existing `gascan-cache-<sandbox-id>` volume.
- `config` maps to the existing `gascan-config-<sandbox-id>` volume.

The runtime contract carries capacity as part of each volume specification
instead of letting a backend invent a default. The Apple backend serializes the
exact byte value into `container volume create -s <bytes>`. Volume names,
mount targets, ownership labels, and removal behavior remain unchanged.

Keeping capacity in the backend-neutral runtime request makes the desired
state explicit and testable. It also prevents another backend from silently
reintroducing an unrelated hardcoded size.

## Existing Sandbox Lifecycle

Apple Container volume capacity is fixed at creation. Gas Can will not resize,
replace, delete, or copy an existing volume automatically.

Gas Can records the three effective capacities when it creates the sandbox.
For later `up` and `apply` operations, it compares the current manifest's
effective capacities with the recorded values:

- Equal capacities continue normally.
- Any mismatch fails before provisioning with the typed code
  `storage_change_requires_recreate`.
- The human diagnostic identifies each changed volume and shows its recorded
  and requested capacities.
- JSON output exposes the same stable error code and structured diagnostic
  details.
- No runtime resources are mutated when this error is returned.

Sandboxes created by a Gas Can version that did not record capacities are
treated as requiring recreation. The user applies new storage settings
explicitly:

```text
gascan destroy --yes
gascan up
```

This release does not add migration, automatic resizing, or implicit
destruction.

## Provisioning Output and Diagnostics

Guest provisioning commands must always be consumed through their terminal
exit event unless the runtime transport itself fails or the operation is
explicitly cancelled.

The executor will handle output streams according to their purpose:

- Structured stdout remains bounded. Commands whose stdout is parsed must
  fail safely rather than allocate unbounded memory.
- Stderr is continuously drained and stored in a fixed-size tail buffer.
  Earlier stderr bytes may be discarded, but the most recent diagnostic is
  retained.
- Large stderr volume does not terminate or detach from the guest command.
- A nonzero command exit reports the provisioning step, action, exit status,
  signal, and sanitized stderr tail.
- If structured stdout exceeds its bound, Gas Can explicitly cancels the
  execution session and reports a typed output-limit failure; it does not
  leave the guest process running unnoticed.

The stderr tail must be large enough to retain ordinary package-manager
terminal errors while remaining strictly bounded. Tests will define the exact
constant and prove that secrets or unlimited installer logs are not persisted
to daemon state.

## User Experience

Default manifests gain useful storage without additional configuration. Users
who need different capacities can declare only the fields they want to change:

```toml
[storage]
tools = "30GiB"
```

For creation, normal progress output remains concise. When a tool installation
fails, the final message reports the actual cause rather than an internal
output-accounting symptom. Changing storage on an existing sandbox produces an
actionable recreation instruction and never destroys data automatically.

The README full-schema example, storage reference, defaults, validation rules,
and recreation requirement will be updated.

## Testing

Automated coverage will include:

1. Manifest parsing for complete defaults, partial overrides, independent
   values, invalid units, zero, overflow, unknown keys, and the `512GiB`
   per-volume maximum.
2. Policy tests proving each effective capacity reaches the corresponding
   runtime volume specification.
3. Apple backend command tests proving all three `container volume create`
   invocations receive their exact independent `-s` byte values.
4. Lifecycle and persistence tests proving changed storage returns
   `storage_change_requires_recreate` without executing runtime mutations.
5. Legacy-record tests proving a sandbox without recorded capacities requires
   explicit recreation.
6. Provisioning tests with more than 1 MiB of stderr, proving the executor
   drains through exit and retains the bounded diagnostic tail.
7. Failure tests proving an `ENOSPC` terminal error is surfaced with its step
   and exit status.
8. Output-bound tests proving oversized structured stdout cancels the guest
   session rather than abandoning it.
9. The complete Rust workspace and scripts test suites.
10. A live Apple lifecycle test that verifies the created volume capacities
    and installs representative large tools.

## Out of Scope

- Resizing or migrating existing volumes.
- Combining the three volumes into one storage pool.
- Automatically deleting or recreating a sandbox.
- Adding host-wide storage quotas.
- Persisting complete installer output.
- Bundling Codex, Claude Code, Pi, Neovim, or Herdr into the base image.
