# CLI Version Flag Design

## Purpose

Gascan must expose the installed CLI version through the conventional
`--version` and `-V` flags. The result must be available without a running
daemon and must remain synchronized with the release version automatically.

## Command Contract

Both commands are supported:

```text
gascan --version
gascan -V
```

Each prints exactly one newline-terminated line to stdout:

```text
gascan <package-version>
```

For the current release, that is `gascan 0.1.5`. Both forms exit with status 0
and leave stderr empty. The standard `-V, --version` option also appears in
`gascan --help`.

This feature does not add a `gascan version` subcommand, a JSON form, build
revision metadata, daemon version output, or runtime compatibility output.

## Version Source and Control Flow

The CLI parser opts into Clap's built-in version metadata. Clap obtains the
value from the Cargo package version embedded at compile time. No version
string is duplicated in Rust source, so the existing release process remains
the single source of truth.

Gascan currently uses `try_parse()` and converts ordinary parse failures into
`CliError::Usage`. Clap also represents help and version displays as parse
results, so merely enabling version metadata would incorrectly route the
version text through Gascan's error renderer. The argument boundary therefore
recognizes exactly `clap::error::ErrorKind::DisplayVersion`, writes Clap's
already-formatted text to stdout, and returns status 0. Every other parse result
keeps its existing behavior; changing help or usage rendering is out of scope.

The version request completes before `Client::connect_or_start`, daemon path
resolution, Gascan filesystem access, or API negotiation. A missing or unusable
daemon cannot affect version output.

The production `gascan` binary reports the `gascan` crate version. The e2e
binary compiles the same CLI entry point from the `gascan-e2e` package; the
repository release contract keeps workspace package versions aligned, and its
test derives the expected value from its own Cargo package metadata.

## Errors and Compatibility

Version requests cannot return a Gascan runtime or daemon error. Clap retains
ownership of the version string and formatting; Gascan owns the successful
stdout write and exit status at its parsing boundary. Existing behavior for all
other parser results, command exit codes, JSON schemas, human presentation,
daemon behavior, and protobuf schemas is unchanged.

## Verification

A process-level integration test invokes both `--version` and `-V`. For each
form it sets an intentionally unusable daemon path and isolated runtime paths,
then asserts:

- exit status 0;
- stdout equals `gascan {env!("CARGO_PKG_VERSION")}\n` exactly;
- stderr is empty; and
- no daemon socket or state artifact is created.

Parser-level coverage also asserts that help includes `-V, --version`, that
only `DisplayVersion` takes the new success path, and that the version option
remains available even though ordinary execution requires a subcommand.
Existing package and workspace test suites provide regression coverage.
