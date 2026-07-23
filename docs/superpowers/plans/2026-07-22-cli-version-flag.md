# CLI Version Flag Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add conventional local `gascan --version` and `gascan -V` flags that report the compiled Cargo package version without contacting the daemon.

**Architecture:** Enable Clap's built-in package-version metadata on the existing root parser. At the current `try_parse()` boundary, treat only `ErrorKind::DisplayVersion` as a successful stdout early return; retain the existing Gascan usage-error path for every other parser result.

**Tech Stack:** Rust 1.85, Clap 4 derive API, Tokio CLI entry point, existing `gascan-e2e` process-test package.

## Global Constraints

- Both `gascan --version` and `gascan -V` print exactly `gascan <package-version>\n` to stdout.
- Both version forms exit 0 and leave stderr empty.
- Version output comes from Cargo package metadata; no version string is duplicated in Rust source.
- Version handling completes before daemon discovery, connection, filesystem state creation, or API negotiation.
- `-V, --version` appears in root help.
- No `gascan version` subcommand, JSON form, build revision, daemon version, or compatibility report is added.
- Every non-version parser result, existing exit code, JSON schema, daemon behavior, and protobuf schema remains unchanged.
- Existing edits under `.superpowers/sdd/` belong to the user and must not be staged or modified.

## File Structure

- Modify `crates/gascan/src/cli.rs`: enable Clap version metadata, recognize `DisplayVersion`, and add parser metadata coverage.
- Create `crates/gascan-e2e/tests/version.rs`: verify both flags as exact daemon-independent process contracts.

---

### Task 1: Standard local version flags

**Files:**
- Modify: `crates/gascan/src/cli.rs:8,18-25,232-236,730-820`
- Create: `crates/gascan-e2e/tests/version.rs`

**Interfaces:**
- Consumes: Cargo-provided `env!("CARGO_PKG_VERSION")` through Clap's `#[command(version)]` derive metadata.
- Produces: root CLI options `--version` and `-V` with exact stdout contract `gascan <package-version>\n` and exit status 0.
- Preserves: `execute() -> Result<i32, CliError>` and all existing `CliError` classifications.

- [ ] **Step 1: Write failing process and parser-metadata tests**

Create `crates/gascan-e2e/tests/version.rs`:

```rust
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn version_flags_are_exact_and_do_not_require_the_daemon() -> TestResult {
    let cli = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;

    for flag in ["--version", "-V"] {
        let runtime = tempfile::tempdir()?;
        let output = Command::new(&cli)
            .arg(flag)
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("GASCAN_STATE_PATH", runtime.path().join("state.sqlite3"))
            .env("GASCAN_PID_PATH", runtime.path().join("daemon.pid"))
            .env("GASCAN_DAEMON", runtime.path().join("missing-gascand"))
            .output()?;

        assert_eq!(output.status.code(), Some(0), "flag {flag}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(output.stdout, format!("gascan {}\n", env!("CARGO_PKG_VERSION")).as_bytes());
        assert!(output.stderr.is_empty(), "flag {flag} wrote stderr");
        assert_eq!(std::fs::read_dir(runtime.path())?.count(), 0, "flag {flag} created runtime state");
    }
    Ok(())
}
```

In the existing `cli.rs` unit-test module, import `clap::CommandFactory as _` and add:

```rust
#[test]
fn root_help_advertises_the_standard_version_flags() {
    let help = Arguments::command().render_help().to_string();
    assert!(help.contains("-V, --version"), "version option missing: {help}");
}

#[test]
fn clap_formats_the_package_version() -> Result<(), Box<dyn std::error::Error>> {
    let error = Arguments::try_parse_from(["gascan", "--version"])
        .err()
        .ok_or("version did not produce an early display result")?;
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    assert_eq!(error.to_string(), format!("gascan {}\n", env!("CARGO_PKG_VERSION")));
    Ok(())
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
rtk cargo test -p gascan root_help_advertises_the_standard_version_flags -- --nocapture
rtk cargo test -p gascan clap_formats_the_package_version -- --nocapture
rtk cargo test -p gascan-e2e --test version -- --nocapture
```

Expected: the unit tests fail because root parser version metadata is absent; the process test exits through the current usage/error path rather than returning exact stdout with status 0.

- [ ] **Step 3: Enable Clap metadata and implement the successful version parse path**

Change the root command annotation to:

```rust
#[command(name = "gascan", version, disable_help_subcommand = true)]
```

Import `clap::error::ErrorKind` and replace the unconditional parse-error conversion in `execute()` with:

```rust
let arguments = match Arguments::try_parse() {
    Ok(arguments) => arguments,
    Err(error) if error.kind() == ErrorKind::DisplayVersion => {
        print!("{error}");
        return Ok(0);
    }
    Err(error) => {
        return Err(CliError::Usage {
            kind: UsageKind::Other,
            message: error.to_string(),
        });
    }
};
```

Do not special-case `DisplayHelp`, change `main`, add a version subcommand, or introduce a version constant.

- [ ] **Step 4: Run focused and regression verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p gascan root_help_advertises_the_standard_version_flags -- --nocapture
rtk cargo test -p gascan clap_formats_the_package_version -- --nocapture
rtk cargo test -p gascan-e2e --test version -- --nocapture
rtk cargo test -p gascan
rtk cargo clippy -p gascan -p gascan-e2e --all-targets -- -D warnings
rtk cargo test --workspace
rtk git diff --check
```

Expected: every command exits 0; the version process test passes for both flags without runtime artifacts; the workspace suite has zero failures; formatting, Clippy, and whitespace checks are clean.

- [ ] **Step 5: Commit the feature**

```bash
rtk git add crates/gascan/src/cli.rs crates/gascan-e2e/tests/version.rs
rtk git commit -m "feat: add CLI version flags"
```
