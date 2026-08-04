# Durable Controller State and Honest Sandbox Listing

## Problem

Gas Can currently stores its SQLite controller database beside ephemeral daemon
IPC files under `/private/tmp/gascan-<uid>/state.sqlite3`. Package replacement,
host cleanup, or manual recovery can remove that directory while Apple container
resources and managed volumes continue to exist. Losing the controller database
can therefore make Gas Can forget resources it still owns, creating confusing
conflicts and a risk that users delete data while trying to recover.

Successful `gascan destroy` operations deliberately retain controller tombstones
and operation history. The public `gascan list` command currently renders those
records as `Absent`, even though implicit sandbox selection already ignores them
and the Apple runtime contains no corresponding container. This makes destroyed
resources look as if they still require cleanup.

## Goals

- Store controller inventory and operation history in durable per-user macOS
  application state.
- Migrate the legacy runtime database without losing committed SQLite WAL data.
- Refuse ambiguous migrations instead of choosing or merging databases.
- Preserve package-upgrade and daemon-replacement behavior without requiring a
  user-managed backup procedure.
- Hide destroyed records from normal list output while retaining them for safe
  recreation, diagnostics, and operation history.
- Keep existing sandbox identifiers, records, managed volumes, and runtime
  ownership rules unchanged.

## Non-goals

- Reconstructing complete controller records from Apple runtime resources.
- Automatically merging independently modified controller databases.
- Deleting tombstones or operation history during ordinary destroy operations.
- Moving user-facing configuration, SSH configuration, or sandbox-managed
  volumes into the new controller directory.
- Changing sandbox identity derivation or resource naming.

## State Architecture

On macOS, the default durable controller database is:

```text
~/Library/Application Support/dev.gascan/controller/state.sqlite3
```

`dev.gascan` is the reverse-DNS namespace for `gascan.dev` and is consistent
with the existing `dev.gascan.pkg` installer identifier.

Only transient daemon coordination remains under the runtime root
(`/private/tmp/gascan-<uid>/` by default, or `$XDG_RUNTIME_DIR/gascan`):

- `gascand.sock`
- `daemon-instance.json`
- `daemon-lifecycle.lock`
- bounded transient daemon diagnostics already owned by the runtime lifecycle

The SQLite database, its journal files, and migration backup are durable state,
not runtime IPC.

`GASCAN_STATE_PATH` remains an explicit test and development override. When it
is set, Gas Can opens exactly that path and does not perform default-path
migration.

## Path and Filesystem Safety

The durable controller directory and database must be owned by the effective
user. Directory traversal must reject symlinks and non-directory path
components. Newly created directories use mode `0700`; newly created database,
temporary, and backup files use mode `0600`.

Existing paths with group or other permission bits are rejected with an
actionable error rather than silently chmodded. Existing non-regular database
or backup paths are rejected. Migration never follows a symlink at either the
legacy or durable location.

The macOS home directory used for the default path is resolved through the
platform's user-home facilities already used by the application. An empty,
relative, non-UTF-8, or unavailable home path fails startup with a precise
controller-state diagnostic.

## Migration State Machine

Migration runs before `Store` becomes available to the daemon. The daemon
lifecycle supervisor must already have established that a prior daemon will not
concurrently mutate the legacy database.

### Durable database only

Open and validate the durable database. No legacy path is created.

### Legacy database only

1. Safely open the legacy database and validate its schema through `Store`.
2. Create a uniquely named temporary database in the durable controller
   directory.
3. Use SQLite's online backup API to copy a consistent database snapshot,
   including committed content currently represented in WAL state.
4. Open and validate the temporary destination through `Store`.
5. Set exact private permissions, sync the database file and containing
   directory, and atomically rename it to `state.sqlite3`.
6. Move the legacy database and any remaining journal sidecars out of their
   active runtime names into a clearly named migration-backup location under
   the durable controller directory.
7. Sync both containing directories and use the durable database.

The backup is not treated as an active database on later startups. Gas Can does
not overwrite a pre-existing migration backup; it chooses a collision-free,
timestamp-independent suffix so recovery remains deterministic and no file is
lost.

### Neither database exists

Create the durable directory safely and initialize a fresh durable `Store`.

### Both active databases exist

Gas Can creates consistent read snapshots and compares their logical SQLite
content rather than raw file bytes.

- If the logical content is identical, the durable database is authoritative.
  The legacy database and sidecars are archived as a migration backup and
  removed from their active runtime names.
- If the logical content differs, startup refuses. Neither active database nor
  its sidecars are modified. The error names both paths, states that no data was
  changed, and instructs the user to back up both files and explicitly select
  which one to preserve.

Gas Can never automatically merges sandbox rows, operation histories, or
resolution metadata from conflicting databases.

## Crash Consistency

Temporary migration databases are created only inside the durable controller
directory so the final rename is atomic. Before the atomic rename, the legacy
database remains authoritative. After the rename, the durable database remains
authoritative even if legacy archival is interrupted.

On a later startup, Gas Can may remove an abandoned temporary migration file
only after proving that it is a regular file owned by the effective user and
matches Gas Can's exact temporary-file naming convention. An active durable
database is never replaced by an abandoned temporary file.

If a crash leaves both active paths after the durable rename, the ordinary
dual-database comparison applies. Identical content completes archival;
different content refuses without modification.

## Upgrade Behavior

Ordinary commands continue to replace an outdated daemon through the existing
lifecycle supervisor. The replacement daemon opens the same durable database,
so sandbox inventory is independent of the executable location, package
uninstall/reinstall steps, runtime socket cleanup, and daemon working directory.

The Homebrew cask and standard uninstall continue preserving user data. Only an
explicit data-removal workflow may remove the durable controller directory, and
that workflow must first destroy every owned sandbox through Gas Can's verified
inventory. Package upgrades must never remove it.

## Destroyed Sandbox Listing

Successful destroy operations continue to:

- remove the Apple container and every verified Gas Can-owned managed resource;
- deactivate SSH publication and trust;
- transition the controller record to desired and actual state `Absent`; and
- retain the tombstone and operation history for deterministic recreation and
  diagnostics.

User-facing list behavior changes as follows:

- `gascan list` excludes `Absent` records.
- `gascan list --json` excludes `Absent` records.
- `gascan list --all` includes historical `Absent` records.
- `gascan list --all --json` includes historical `Absent` records without
  changing the stable JSON state value.
- Human `--all` output renders the historical state as `Destroyed`, not
  `Absent`.
- When every record is destroyed, normal human output is exactly
  `No sandboxes found.` and normal JSON output is an empty array.

Filtering occurs defensively at the CLI boundary, preserving current daemon
wire compatibility. The daemon may continue returning tombstones because they
are valid controller records and older clients already understand them. The
`--all` option controls whether the current CLI retains or filters those records
before rendering.

Implicit selection continues ignoring `Absent` records. Explicit status lookup
by sandbox ID remains available for diagnostics. Running `gascan up` again for
the same canonical project root reuses the tombstone and recreates resources
without changing the sandbox ID.

## Errors and Recovery

Controller-state failures use stable, human-readable diagnostics and do not
fall through to generic daemon readiness errors. A conflicting migration error
has this shape:

```text
Gascan found conflicting controller databases and will not choose one automatically.

Durable: /Users/name/Library/Application Support/dev.gascan/controller/state.sqlite3
Legacy:  /private/tmp/gascan-501/state.sqlite3

No data was changed. Back up both files, then select the database to preserve.
```

JSON-capable daemon management and diagnostic paths retain a stable error code
and expose paths as structured fields without including database contents.
Diagnostics never print sandbox secrets, credential material, or raw database
rows.

## Testing

Test-driven implementation must cover:

- fresh startup with neither database;
- durable-only startup;
- legacy-only migration using a database with committed WAL-backed data;
- identical dual-state archival;
- conflicting dual-state refusal and byte-for-byte non-modification of both
  active databases and sidecars;
- unsafe ownership, modes, symlinks, non-regular files, malformed schemas, and
  inaccessible home/Application Support paths;
- simulated interruption before snapshot completion, before atomic rename,
  after rename, and during legacy archival;
- cleanup rules for abandoned migration temporary files;
- daemon replacement and package-upgrade contracts preserving existing sandbox
  records;
- normal and `--all` list behavior in human and JSON output;
- destroying the final sandbox, retaining internal history, and safely
  recreating the same sandbox ID;
- existing store, daemon lifecycle, CLI, protocol, installer, and macOS release
  smoke coverage.

Release verification must demonstrate that an installed upgrade preserves a
created sandbox and its managed-volume marker across daemon replacement, and
that a subsequently destroyed sandbox disappears from normal list output.
