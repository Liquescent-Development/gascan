# Daemon Reader's Retryable Verdict Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Teach the `gascan` CLI to tell "I raced with a legitimate daemon transition" from "something is actually wrong", and close the last in-tree producer of the state it races with.

**Architecture:** Two halves. The producer half rewrites `retire_held_record` to stage a fresh inert tombstone, rename it over the destination, and only then truncate the orphaned inode — so the destination never wears the illegal `(0200, content)` face. The reader half marks race-shaped failures with a typed payload inside the existing `io::Error` plumbing, retries the whole observation sequence a bounded number of times, and falls back to today's `DaemonState::Unsafe` when the path never settles.

**Tech Stack:** Rust 2024, `rustix` for raw file syscalls, `tokio` for the async supervisor, `tempfile` for test fixtures.

**Spec:** `docs/superpowers/specs/2026-08-18-daemon-reader-retryable-verdict-design.md`

## Global Constraints

- **Never weaken a test to make a change compile or pass.** If a test blocks the change, the change is wrong or the test's premise has changed — say which.
- **Every claim is proven by mutation, not inspection.** A test that has not been made to fail has proven nothing.
- **A green local run is a precondition for pushing, never evidence that a window is closed.** This tree's record: 47,124,057 local samples said a state was gone, CI's first run disagreed, and CI was right (`docs/status/START-HERE.md`, trap 9, against `add3c13`).
- **Run `cargo test --workspace` alone**, never beside another cargo or contract job.
- **Classification of failures defaults to terminal.** Only a failure explicitly constructed as transient is retryable.
- **`(0200, content)` stays terminal.** It is version skew from an older `gascand`, not a race.
- Modes and protocol names come from `gascan_core::daemon_protocol`. Never re-declare one locally.
- Every CI step before pushing: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `./scripts/ci-check-ignored-tests.sh`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/gascan-core/src/daemon_protocol.rs` | the shared on-disk protocol | add both staging purposes; drop the three-face rule's standing exception once Task 3 lands |
| `crates/gascan-core/tests/daemon_protocol.rs` | pins each protocol value to its literal | add the two new values |
| `crates/gascand/src/socket.rs` | the writer, and the staging sweeper | import the shared purposes; sweep both prefixes; rewrite the sweeper's safety comment and the "must not join" comment |
| `crates/gascan/src/daemon.rs` | the reader, and the reclaim path | new staging helper; rewritten `retire_held_record` and `validate_retired_tombstone`; transient-failure type; retry loop |

---

## Task 1: Share the staging vocabulary

**Files:**
- Modify: `crates/gascan-core/src/daemon_protocol.rs`
- Modify: `crates/gascan-core/tests/daemon_protocol.rs`
- Modify: `crates/gascand/src/socket.rs:18-21` (the local `INSTANCE_STAGING_PURPOSE` and its comment)

**Interfaces:**
- Produces: `gascan_core::daemon_protocol::INSTANCE_STAGING_PURPOSE: &str`, `gascan_core::daemon_protocol::RECLAIM_STAGING_PURPOSE: &str`

- [ ] **Step 1: Write the failing pin test**

Append to `crates/gascan-core/tests/daemon_protocol.rs`, and add both names to the existing `use` at the top of that file:

```rust
/// Two processes now stage files in the daemon's runtime directory, so the
/// prefixes are protocol rather than private detail: `gascand`'s sweeper
/// matches both, and a prefix that changed on one side alone would either
/// orphan files forever or sweep a live one.
#[test]
fn the_staging_prefixes_are_stable_and_distinct() {
    assert_eq!(INSTANCE_STAGING_PURPOSE, "instance");
    assert_eq!(RECLAIM_STAGING_PURPOSE, "reclaim");
    assert_ne!(INSTANCE_STAGING_PURPOSE, RECLAIM_STAGING_PURPOSE);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gascan-core --test daemon_protocol`
Expected: FAIL to compile — `unresolved import` for both names.

- [ ] **Step 3: Add the constants**

In `crates/gascan-core/src/daemon_protocol.rs`, after `LIFECYCLE_LOCK_NAME`:

```rust
/// The staging-name prefix `gascand` uses while building the next instance
/// record, as `.{purpose}-{token}`.
///
/// A staged file is never read by name — it exists only to be renamed into
/// place — but the prefix is shared because `gascand`'s sweeper matches it to
/// clean up files a crash abandoned, and it must match what the writer wrote.
pub const INSTANCE_STAGING_PURPOSE: &str = "instance";

/// The staging-name prefix the `gascan` CLI uses while building the inert
/// tombstone that retires a record it has proven dead, as `.{purpose}-{token}`.
///
/// Distinct from [`INSTANCE_STAGING_PURPOSE`] so that a stray file says which
/// process left it, and so the sweeper can reason about the two separately:
/// `gascand`'s staging holds a complete record with an owner token in it, while
/// this one is inert and empty from birth and never holds anything.
pub const RECLAIM_STAGING_PURPOSE: &str = "reclaim";
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p gascan-core --test daemon_protocol`
Expected: PASS, 6 tests.

- [ ] **Step 5: Point `gascand` at the shared constant**

In `crates/gascand/src/socket.rs`, add `INSTANCE_STAGING_PURPOSE` and `RECLAIM_STAGING_PURPOSE` to the existing `use gascan_core::daemon_protocol::{…}` block, and **delete** the local declaration together with the comment that now says the wrong thing:

```rust
/// The staging prefix the sweeper matches. This one is `gascand`'s alone: no
/// reader ever sees a staged file by name, so it is not part of the shared
/// protocol and must not join it.
const INSTANCE_STAGING_PURPOSE: &str = "instance";
```

That comment was true when it was written and this change makes it false: the CLI became a second stager. It is deleted rather than edited, because the constant it describes is moving.

- [ ] **Step 6: Verify both crates still build and pass**

Run: `cargo test -p gascan-core -p gascand --lib`
Expected: PASS. `publication_sweeps_abandoned_instance_staging_and_nothing_else` still passes — it reads the constant through `super::`, which now resolves to the import.

- [ ] **Step 7: Prove the pin has power**

Change `RECLAIM_STAGING_PURPOSE` to `"reclaimed"`, run `cargo test -p gascan-core --test daemon_protocol`, confirm FAIL, then restore it.

- [ ] **Step 8: Commit**

```bash
git add crates/gascan-core/src/daemon_protocol.rs crates/gascan-core/tests/daemon_protocol.rs crates/gascand/src/socket.rs
git commit -m "refactor: the staging prefixes are shared protocol, because there are two stagers now"
```

---

## Task 2: The sweeper covers both prefixes

**Files:**
- Modify: `crates/gascand/src/socket.rs:486-521` (`sweep_abandoned_staging` and its doc comment)
- Test: `crates/gascand/src/socket.rs` (test module)

**Interfaces:**
- Consumes: `RECLAIM_STAGING_PURPOSE` from Task 1.

- [ ] **Step 1: Write the failing test**

Add to `crates/gascand/src/socket.rs`'s test module:

```rust
/// The CLI stages an inert tombstone in this directory when it retires a record
/// it has proven dead, so a CLI killed between staging and renaming leaves a
/// file behind under a prefix nothing else enumerates. It holds no token — it
/// is empty from birth — so this is tidiness rather than secrecy, but the
/// sweeper is the only thing that enumerates this directory and it has to cover
/// both stagers or one of them accumulates a file per crash forever.
#[test]
fn publication_sweeps_abandoned_reclaim_staging_too()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?.join("runtime");
    let path = root.join("daemon-instance.json");
    drop(super::write_instance_record(&path, b"first")?);

    let abandoned = root.join(format!(".{}-BBBBBBBBBB", super::RECLAIM_STAGING_PURPOSE));
    fs::write(&abandoned, b"")?;
    fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o200))?;
    let bystander = root.join(".bystander");
    fs::write(&bystander, b"not ours to remove")?;

    let record = super::write_instance_record(&path, b"second")?;

    assert!(!abandoned.exists(), "the abandoned reclaim staging survived the sweep");
    assert!(bystander.exists(), "the sweep removed a file that was not staging");
    drop(record);
    Ok(())
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gascand --lib publication_sweeps_abandoned_reclaim_staging_too`
Expected: FAIL — "the abandoned reclaim staging survived the sweep".

- [ ] **Step 3: Sweep both prefixes**

Replace the prefix line and its filter in `sweep_abandoned_staging`:

```rust
    let prefixes = [
        format!(".{INSTANCE_STAGING_PURPOSE}-"),
        format!(".{RECLAIM_STAGING_PURPOSE}-"),
    ];
```

and the `starts_with` guard:

```rust
        if !prefixes
            .iter()
            .any(|prefix| name.starts_with(prefix.as_str()))
        {
            continue;
        }
```

- [ ] **Step 4: Rewrite the safety argument in the doc comment**

The existing comment argues "a live daemon's staging cannot be caught: publication runs once per daemon and `prepare_socket` has already refused to start a second one against a live socket." That names one stager. Replace that sentence with:

```rust
/// Two processes stage here now, so the old argument — that publication runs
/// once per daemon and `prepare_socket` has already refused a second one — no
/// longer covers the set. What covers it is the lifecycle lock: the CLI stages
/// only inside `retire_held_record`, reached from `start_with` while it holds
/// the lock (`crates/gascan/src/daemon.rs`), and the `gascand` whose publication
/// runs this sweep was spawned by that same locked call. A CLI staging file
/// still at rest when this runs is therefore one no live process owns.
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p gascand --lib`
Expected: PASS, including both sweep tests.

- [ ] **Step 6: Prove the new test has power**

Remove `RECLAIM_STAGING_PURPOSE` from the `prefixes` array, confirm the new test FAILS and the old one still passes, then restore.

- [ ] **Step 7: Commit**

```bash
git add crates/gascand/src/socket.rs
git commit -m "fix: the staging sweeper covers both stagers, and says why the lock is what makes it safe"
```

---

## Task 3: `retire_held_record` stages, renames, then truncates

**Files:**
- Modify: `crates/gascan/src/daemon.rs:1456-1462` (`retire_held_record`)
- Modify: `crates/gascan/src/daemon.rs:1553-1583` (`validate_retired_tombstone`)
- Modify: `crates/gascan/src/daemon.rs` imports — add `use base64::Engine as _;`

**Interfaces:**
- Consumes: `RECLAIM_STAGING_PURPOSE` (Task 1); existing `InterruptedTombstone { directory, name, file, identity, expected_uid, size }`, `is_instance_tombstone`, `FileIdentity`, `errno`.
- Produces: `fn stage_inert_reclaim_file(&OwnedFd, u32) -> Result<(File, String, FileIdentity), SupervisorError>`; `fn validate_retired_tombstone(&InterruptedTombstone, &File, FileIdentity) -> Result<(), SupervisorError>`.

- [ ] **Step 1: Write the failing test**

Add to `crates/gascan/src/daemon.rs`'s test module:

```rust
/// Retirement has two jobs: leave a legal inert tombstone at the destination,
/// and destroy the dead record's bytes so a descriptor that outlives this
/// process cannot read the owner token back. A rename alone does the first and
/// silently drops the second, which is why the old inode is truncated after the
/// rename rather than instead of it.
#[tokio::test]
async fn retirement_replaces_the_record_and_empties_the_inode_it_retired() -> TestResult {
    let temp = tempfile::tempdir()?;
    let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
    paths.prepare_directory()?;
    fs::write(paths.instance(), b"a-record-with-a-token")?;
    fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;

    let held = open_interrupted_tombstone(&paths)?.ok_or("expected an interrupted tombstone")?;
    let retired_inode = rustix::fs::fstat(&held.file)?.st_ino;

    retire_held_record(&held)?;

    let at_name = fs::symlink_metadata(paths.instance())?;
    assert_eq!(at_name.permissions().mode() & 0o777, 0o200, "the destination is inert");
    assert_eq!(at_name.len(), 0, "the destination is empty");
    assert_ne!(
        std::os::unix::fs::MetadataExt::ino(&at_name),
        retired_inode,
        "the destination still names the retired inode; it was mutated in place",
    );

    let held_after = rustix::fs::fstat(&held.file)?;
    assert_eq!(held_after.st_nlink, 0, "the retired inode is still in the namespace");
    assert_eq!(held_after.st_size, 0, "the retired inode still holds its bytes");
    Ok(())
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gascan --lib retirement_replaces_the_record_and_empties_the_inode_it_retired`
Expected: FAIL on the `assert_ne!` — today the destination still names the retired inode, because retirement mutates it in place.

- [ ] **Step 3: Add the staging helper**

Add near `retire_held_record` in `crates/gascan/src/daemon.rs`:

```rust
/// The inert file retirement builds its next state in: created under a private
/// name nobody is watching, `0200` and empty before it exists to anyone else,
/// and unlinked again unless the caller renames it into place.
///
/// This mirrors `stage_inert_instance_file` in `crates/gascand/src/socket.rs`.
/// The two are separate because they live in different crates and stage under
/// different prefixes; the recipe they share — create exclusive, `fchmod`,
/// then verify rather than assume — is the part that matters.
fn stage_inert_reclaim_file(
    directory: &OwnedFd,
    expected_uid: u32,
) -> Result<(File, String, FileIdentity), SupervisorError> {
    let staging = reclaim_staging_name()?;
    let fd = rustix::fs::openat(
        directory,
        staging.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(INSTANCE_TOMBSTONE_MODE),
    )
    .map_err(errno)?;
    let file = File::from(fd);
    let staged = (|| {
        // `openat`'s mode argument is masked by the umask, so the file is only
        // known to be inert after an explicit `fchmod`.
        rustix::fs::fchmod(&file, Mode::from_raw_mode(INSTANCE_TOMBSTONE_MODE)).map_err(errno)?;
        let stat = rustix::fs::fstat(&file).map_err(errno)?;
        if !is_instance_tombstone(&stat, expected_uid) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "reclaim staging file is not an inert private file",
            ));
        }
        Ok(FileIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        })
    })();
    match staged {
        Ok(identity) => Ok((file, staging, identity)),
        Err(error) => {
            let _ = rustix::fs::unlinkat(directory, staging.as_str(), AtFlags::empty());
            Err(SupervisorError::Io(error))
        }
    }
}

fn reclaim_staging_name() -> Result<String, SupervisorError> {
    let mut bytes = [0_u8; 7];
    getrandom::fill(&mut bytes)
        .map_err(|error| SupervisorError::Io(io::Error::other(error)))?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    Ok(format!(".{RECLAIM_STAGING_PURPOSE}-{token}"))
}
```

- [ ] **Step 4: Rewrite `retire_held_record`**

```rust
/// Retire a record this process has proven dead: put a legal inert tombstone at
/// the destination, and destroy the dead record's bytes.
///
/// **The order is forced, and it is the mirror image of the publisher's.**
/// `crates/gascand/src/socket.rs` truncates before it chmods, because `lstat`
/// tears between resolving a name and reading an inode and the torn read must
/// not be `(0200, content)`. Here the destructive step comes *after* the
/// rename for the same underlying reason: an inode is only safe to mutate
/// destructively once it is out of the namespace. Truncating first would put
/// `(0600, 0)` at the live name, and `validate_file_stat` accepts that as a
/// published record of size zero — the reader would take it and then fail
/// parsing an empty file, which is a worse failure than the one this fixes.
///
/// A rename alone is not enough either. It leaves the old inode alive and
/// unlinked with its content intact, reachable by any descriptor that outlives
/// this process, and that content is what holds the owner token.
fn retire_held_record(record: &InterruptedTombstone) -> Result<(), SupervisorError> {
    let (staged, staging, staged_identity) =
        stage_inert_reclaim_file(&record.directory, record.expected_uid)?;
    staged.sync_all()?;
    if let Err(error) = rustix::fs::renameat(
        &record.directory,
        staging.as_str(),
        &record.directory,
        record.name.as_os_str(),
    ) {
        let _ = rustix::fs::unlinkat(&record.directory, staging.as_str(), AtFlags::empty());
        return Err(SupervisorError::Io(errno(error)));
    }
    // The rename unlinked the record, so nothing can reach it by name and
    // emptying it is invisible at the path.
    rustix::fs::ftruncate(&record.file, 0).map_err(errno)?;
    record.file.sync_all()?;
    validate_retired_tombstone(record, &staged, staged_identity)?;
    Ok(())
}
```

- [ ] **Step 5: Rewrite `validate_retired_tombstone` against two identities**

The old post-condition — the held inode is still the inode at the name — is unsatisfiable once the rename unlinks it, which is exactly why this could not be folded into the publish-race fix. Replace the whole function:

```rust
/// Prove the retirement reached its two ends. The old form asserted one inode
/// was still at the name; a rename unlinks it, so that is now unsatisfiable by
/// construction. The replacement is strictly stronger: it proves the record is
/// gone from the namespace, that its bytes are destroyed, and that what stands
/// in its place is legal.
fn validate_retired_tombstone(
    record: &InterruptedTombstone,
    staged: &File,
    staged_identity: FileIdentity,
) -> Result<(), SupervisorError> {
    let held = rustix::fs::fstat(&record.file).map_err(errno)?;
    if held.st_nlink != 0 || held.st_size != 0 {
        return Err(SupervisorError::TombstoneChanged {
            detail: format!(
                "the retired record is still reachable or still holds content (links {}, size {})",
                held.st_nlink, held.st_size
            ),
        });
    }
    let at_name = rustix::fs::statat(
        &record.directory,
        record.name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| SupervisorError::TombstoneChanged {
        detail: errno(error).to_string(),
    })?;
    let name_identity = FileIdentity {
        device: at_name.st_dev as u64,
        inode: at_name.st_ino,
    };
    if !is_instance_tombstone(&at_name, record.expected_uid) || name_identity != staged_identity {
        return Err(SupervisorError::TombstoneChanged {
            detail: "the pathname does not name the inert tombstone this retirement staged"
                .to_owned(),
        });
    }
    let staged_stat = rustix::fs::fstat(staged).map_err(errno)?;
    if (FileIdentity {
        device: staged_stat.st_dev as u64,
        inode: staged_stat.st_ino,
    }) != staged_identity
        || staged_stat.st_nlink != 1
    {
        return Err(SupervisorError::TombstoneChanged {
            detail: "the staged tombstone changed while it was being renamed into place".to_owned(),
        });
    }
    Ok(())
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p gascan --lib`
Expected: PASS, including the new test and every existing `tombstone_recovery_*` and `stale_record_recovery_*` test. **If one of those fails, stop and read it — do not adjust it to fit.** They encode refusals this change must preserve.

- [ ] **Step 7: Commit**

```bash
git add crates/gascan/src/daemon.rs
git commit -m "fix: retirement stages and renames rather than mutating a live record in place"
```

---

## Task 4: Prove the window is closed with a concurrent observer

**Files:**
- Test: `crates/gascan/src/daemon.rs` (test module)

**Interfaces:**
- Consumes: `retire_held_record` and `open_interrupted_tombstone` as rewritten in Task 3.

- [ ] **Step 1: Write the observer test**

`no_reader_ever_sees_an_illegal_state_across_start_and_stop` in `crates/gascand/src/socket.rs` covers publication and retirement from the writer's side. Nothing covers `gascan`'s reclaim path, which is where the remaining window lived. Add to `crates/gascan/src/daemon.rs`'s test module:

```rust
/// The reclaim path was the last in-tree producer of `(0200, content)` — the
/// illegal fourth face — because it chmod-ed a live record and only then
/// truncated it. This samples the destination from another thread across many
/// reclaim cycles and asserts the path only ever showed one of the three legal
/// faces.
///
/// Bounded at 64 cycles deliberately. A larger number is not stronger evidence:
/// this tree's record is that 47,124,057 local samples said a state was gone
/// and CI's first run disagreed.
#[test]
fn no_reader_ever_sees_an_illegal_state_across_reclaim() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrd};

    let temp = tempfile::tempdir()?;
    let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
    paths.prepare_directory()?;
    let instance = paths.instance().to_path_buf();

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let observer = {
        let stop = std::sync::Arc::clone(&stop);
        let instance = instance.clone();
        std::thread::spawn(move || {
            let mut seen = std::collections::BTreeSet::new();
            while !stop.load(AtomicOrd::Acquire) {
                // Yield rather than spin: this project records the workspace
                // suite wandering under load, and a saturated core is load.
                std::thread::yield_now();
                match fs::symlink_metadata(&instance) {
                    // A stat whose link count is not one is not a state of this
                    // path. `lstat` resolves a name and then reads the inode,
                    // and those are not one step, so an observer can come away
                    // holding the attributes of an inode the rename detached in
                    // between. The reader draws the same line.
                    Ok(metadata) if std::os::unix::fs::MetadataExt::nlink(&metadata) == 1 => {
                        seen.insert(Some((metadata.permissions().mode() & 0o777, metadata.len())));
                    }
                    Ok(_) => {}
                    Err(_) => {
                        seen.insert(None);
                    }
                }
            }
            seen
        })
    };

    for _ in 0..64 {
        fs::write(&instance, b"a-record-with-a-token")?;
        fs::set_permissions(&instance, fs::Permissions::from_mode(0o200))?;
        let held = open_interrupted_tombstone(&paths)?.ok_or("expected an interrupted tombstone")?;
        retire_held_record(&held)?;
    }
    stop.store(true, AtomicOrd::Release);
    let seen = observer.join().map_err(|_| "the observer panicked")?;

    let tombstone = Some((u32::from(INSTANCE_TOMBSTONE_MODE), 0));
    let illegal: Vec<_> = seen
        .iter()
        .filter(|state| {
            state.is_some()
                && **state != tombstone
                && !matches!(**state, Some((0o600, size)) if size > 0)
        })
        .collect();
    assert!(
        illegal.is_empty(),
        "a reader saw {illegal:?}, which is neither absent, the inert tombstone, nor a whole record",
    );
    Ok(())
}
```

- [ ] **Step 2: Run it to verify it passes**

Run: `cargo test -p gascan --lib no_reader_ever_sees_an_illegal_state_across_reclaim`
Expected: PASS.

- [ ] **Step 3: Prove the test has power — this step is the point of the task**

Temporarily restore the old order in `retire_held_record` by replacing the staging block with the original two syscalls:

```rust
    rustix::fs::fchmod(&record.file, Mode::from_raw_mode(INSTANCE_TOMBSTONE_MODE))
        .map_err(errno)?;
    rustix::fs::ftruncate(&record.file, 0).map_err(errno)?;
```

Run the test. Expected: FAIL, reporting a sighting of `Some((128, N))` — `0o200` with content. Record the exact observed pair in the commit message. **Then restore Task 3's implementation and re-run to confirm PASS.**

If the mutated version passes, the test is not sampling the window; increase cycles and re-run before concluding anything, and say so rather than proceeding.

- [ ] **Step 4: Commit**

```bash
git add crates/gascan/src/daemon.rs
git commit -m "test: a concurrent observer proves the reclaim path shows only three faces"
```

---

## Task 5: A transient-failure type that defaults to terminal

**Files:**
- Modify: `crates/gascan/src/daemon.rs` (new type near `FileIdentity`; construction sites in `validate_instance_tombstone` and `open_published_record`)

**Interfaces:**
- Produces: `fn raced(detail: &str) -> io::Error`, `fn is_raced(error: &io::Error) -> bool`

- [ ] **Step 1: Write the failing test**

```rust
/// Transience is carried in the error's payload rather than in its kind,
/// because the kind is already load-bearing and because this way the default is
/// the safe one: an error built any other way is terminal, so a validator added
/// later that nobody classifies stays `Unsafe` until a human decides otherwise.
#[test]
fn only_explicitly_raced_failures_are_retryable() {
    assert!(is_raced(&raced("the tombstone changed while opening it")));
    assert!(!is_raced(&io::Error::new(
        io::ErrorKind::PermissionDenied,
        "protected runtime file is unsafe: not a regular file",
    )));
    assert!(!is_raced(&io::Error::from(io::ErrorKind::NotFound)));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gascan --lib only_explicitly_raced_failures_are_retryable`
Expected: FAIL to compile — `cannot find function raced`.

- [ ] **Step 3: Add the type and its two functions**

```rust
/// A failure that says the reader looked at a moving target, not that it found
/// something wrong.
///
/// It rides inside `io::Error` so that every validator keeps returning
/// `io::Result` and no signature changes — and so that the default is
/// fail-closed. Only a failure built by [`raced`] is retryable; anything else,
/// including anything a future validator invents, stays terminal.
#[derive(Debug)]
struct RacedObservation {
    detail: String,
}

impl std::fmt::Display for RacedObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for RacedObservation {}

fn raced(detail: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        RacedObservation {
            detail: detail.to_owned(),
        },
    )
}

fn is_raced(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<RacedObservation>())
}
```

- [ ] **Step 4: Mark the race-shaped construction sites**

In `validate_instance_tombstone`, both failures become `raced(...)`:

```rust
        return Err(raced("daemon instance tombstone changed while opening it"));
```
```rust
        return Err(raced("daemon instance tombstone changed while validating it"));
```

In `open_published_record`, the record-changed failure and the identity-or-size recheck failure become `raced(...)`:

```rust
        return Err(raced("daemon instance record changed while binding its descriptor"));
```

Leave every other failure in both functions exactly as it is. In particular **do not** touch `validate_file_stat`'s `(0200, content)` arm: after Task 3 its only producer is a `gascand` from an older release, which is version skew and a real diagnosis, and retrying it would turn that into a silent delay followed by an identical-looking `Unsafe`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p gascan --lib`
Expected: PASS. The messages are unchanged, so any test asserting on them still matches.

- [ ] **Step 6: Commit**

```bash
git add crates/gascan/src/daemon.rs
git commit -m "feat: race-shaped reader failures carry a type, and everything else stays terminal"
```

---

## Task 6: The retry loop

**Files:**
- Modify: `crates/gascan/src/daemon.rs:981` (`inspect_with`) — extract its body, wrap it
- Modify: `crates/gascan/src/daemon.rs:615-622` (`SupervisorTimeouts::default`) — extract the poll literal

**Interfaces:**
- Consumes: `is_raced` (Task 5).
- Produces: `inspect_with` keeps its exact signature, so all 23 call sites are untouched.

- [ ] **Step 1: Write the failing tests**

```rust
/// A reader that raced looks again and reports what the daemon settled into.
/// The race never reaches the user.
#[tokio::test]
async fn an_observation_that_races_once_then_settles_reports_the_settled_verdict() -> TestResult {
    let temp = tempfile::tempdir()?;
    let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
    paths.prepare_directory()?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
    let inspector = MutableInspector::new(None);

    // Race the first observation only: the destination is illegal when the
    // reader first looks and absent by the time it looks again.
    fs::write(paths.instance(), b"mid-transition")?;
    fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
    let instance = paths.instance().to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let _ = fs::remove_file(&instance);
    });

    let inspected = inspect_with(&paths, &executable, &endpoint, &inspector).await?;

    assert_eq!(inspected.status.state, DaemonState::Stopped);
    Ok(())
}

/// A path that never settles is not a race any more. Fail closed.
#[tokio::test]
async fn an_observation_that_never_settles_is_unsafe_and_says_so() -> TestResult {
    let temp = tempfile::tempdir()?;
    let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
    paths.prepare_directory()?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
    let inspector = MutableInspector::new(None);

    fs::write(paths.instance(), b"mid-transition")?;
    fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;

    let inspected = inspect_with(&paths, &executable, &endpoint, &inspector).await?;

    assert_eq!(inspected.status.state, DaemonState::Unsafe);
    Ok(())
}
```

Note: the second test relies on `(0200, content)` staying terminal per Task 5 — it is `Unsafe` on the first observation and never retried. That is the intended behaviour and the test documents it.

- [ ] **Step 2: Run them to verify the first fails**

Run: `cargo test -p gascan --lib an_observation_that`
Expected: the first FAILS (`Unsafe`, because there is no retry yet); the second PASSES.

- [ ] **Step 3: Extract the poll default so the retry does not re-declare it**

In `crates/gascan/src/daemon.rs`, add beside the other constants and use it in `SupervisorTimeouts::default`:

```rust
/// How long the supervisor waits between two looks at the same thing. Named
/// once because the retry below and `SupervisorTimeouts::default` must not
/// drift apart — a re-declared 25ms is the same class of duplicate the shared
/// `gascan_core::daemon_protocol` exists to remove.
const DEFAULT_POLL: Duration = Duration::from_millis(25);
```

and in `SupervisorTimeouts::default`, replace `poll: Duration::from_millis(25),` with `poll: DEFAULT_POLL,`.

- [ ] **Step 4: Rename the existing body and add the wrapper**

Rename the existing `pub(crate) async fn inspect_with` to `async fn observe_once`, keeping its body and signature otherwise identical. Then add:

```rust
/// Observations of a path two processes share disagree sometimes, and a
/// disagreement is not by itself a fault. `start_with` takes the lifecycle lock
/// and `inspect` does not, so a status check can sample the record while a
/// legitimate stop is rewriting it; every such disagreement used to be a
/// terminal `DaemonState::Unsafe`, which is a verdict whose other members are
/// symlink attacks and foreign ownership.
///
/// So a race-shaped failure is looked at again rather than believed. Three
/// observations, because the windows this races with are a rename wide and one
/// retry already clears them — the third is margin, not expectation. Under
/// `start_with` the lock makes a race impossible, so this never retries there.
///
/// **If it never settles, the verdict is `Unsafe`.** A path that will not stop
/// changing is a fault, and the detail says which failure kept recurring.
pub(crate) async fn inspect_with<E, P>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
) -> Result<Inspection<E::Connection>, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
{
    const OBSERVATIONS: u32 = 3;

    let mut last_race: Option<String> = None;
    for observation in 0..OBSERVATIONS {
        if observation > 0 {
            tokio::time::sleep(DEFAULT_POLL).await;
        }
        let inspected = observe_once(paths, expected_executable, endpoint, inspector).await?;
        match inspected.raced_detail() {
            Some(detail) => last_race = Some(detail.to_owned()),
            None => return Ok(inspected),
        }
    }
    let detail = last_race.unwrap_or_else(|| "the daemon record kept changing".to_owned());
    Ok(Inspection {
        status: DaemonStatus {
            state: DaemonState::Unsafe,
            identity: None,
            legacy: false,
            detail: Some(format!(
                "the daemon record was still changing after {OBSERVATIONS} observations: {detail}"
            )),
        },
        session: None,
        record: None,
        interrupted_tombstone: None,
        published_record: None,
    })
}
```

- [ ] **Step 5: Carry the race marker out of `observe_once`**

`observe_once` converts its errors to a `detail` string before returning, so the retry needs the marker preserved. In `observe_once`, at each site that today writes `detail: Some(error.to_string())` for a failure that may be raced, record the transience alongside it. Add the field to `Inspection` and the accessor the wrapper calls:

```rust
    /// Set when this observation failed because the path moved under it rather
    /// than because it found something wrong. `inspect_with` retries on it;
    /// nothing else reads it.
    raced: Option<String>,
```

```rust
impl<C> Inspection<C> {
    fn raced_detail(&self) -> Option<&str> {
        self.raced.as_deref()
    }
}
```

At each `Unsafe` construction in `observe_once` that is built from an `io::Error`, set `raced: is_raced(&error).then(|| error.to_string())`. At every other construction set `raced: None`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p gascan --lib`
Expected: PASS, both new tests and all existing ones.

- [ ] **Step 7: Prove the default is terminal**

Change `is_raced` to `true` unconditionally, run `cargo test -p gascan --lib`, and confirm `an_observation_that_never_settles_is_unsafe_and_says_so` still passes but takes visibly longer — the `(0200, content)` case is now being retried, which is the behaviour Task 5 exists to prevent. Restore `is_raced`.

- [ ] **Step 8: Commit**

```bash
git add crates/gascan/src/daemon.rs
git commit -m "fix: the reader retries a race instead of reporting it as a fault"
```

---

## Task 7: The reachable `ENOENT` is a race

**Files:**
- Modify: `crates/gascan/src/daemon.rs:2855` (the `openat` in `validate_instance_tombstone`)

**Interfaces:**
- Consumes: `raced` (Task 5).

- [ ] **Step 1: Write the failing test**

```rust
/// `clear_inert_destination` in `crates/gascand/src/socket.rs` unlinks the
/// tombstone, so this `openat` can return `ENOENT` where it could not before
/// the publish-race fix. `read_instance_record_for_inspection` maps `NotFound`
/// to `Ok(None)` and is unaffected; `read_attested_instance` propagates it, and
/// it has no non-test callers yet. Classifying it now removes a trap from
/// whoever wires it rather than leaving one.
#[test]
fn a_tombstone_that_vanishes_between_looks_is_a_race_not_a_fault() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
    paths.prepare_directory()?;
    fs::write(paths.instance(), b"")?;
    fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;

    let (parent, name) = instance_parent_and_name(paths.instance())?;
    let directory = open_private_directory_with_mode(parent, paths.expected_uid, false)?;
    let stat = rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
    fs::remove_file(paths.instance())?;

    let error = validate_instance_tombstone(&directory, name, &stat, paths.expected_uid)
        .expect_err("a vanished tombstone must not validate");
    assert!(is_raced(&error), "a successor unlinking the tombstone is a race, not a fault");
    Ok(())
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gascan --lib a_tombstone_that_vanishes_between_looks_is_a_race_not_a_fault`
Expected: FAIL — the raw `ENOENT` from `openat` is not marked.

- [ ] **Step 3: Classify it**

In `validate_instance_tombstone`, replace the `openat`'s `.map_err(errno)?` with a mapping that marks absence as a race and leaves every other errno terminal:

```rust
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            raced("the daemon instance tombstone was unlinked while validating it")
        } else {
            errno(error)
        }
    })?;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p gascan --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gascan/src/daemon.rs
git commit -m "fix: a tombstone unlinked by a successor is a race rather than a permission fault"
```

---

## Task 8: The three-face rule is true now, so say so

**Files:**
- Modify: `crates/gascan-core/src/daemon_protocol.rs` (the standing-exception paragraph)
- Modify: `crates/gascan/src/daemon.rs:3195-3225` (`validate_file_stat`'s doc comment)
- Modify: `docs/status/START-HERE.md`

- [ ] **Step 1: Drop the standing exception from the shared module**

`crates/gascan-core/src/daemon_protocol.rs` currently says one in-tree producer still violates the rule and names `retire_held_record`. After Task 3 that is false. Replace that paragraph with what is now true — the rule holds for every in-tree producer, and the remaining way to see the illegal face is a `gascand` from an older release, which is why the reader still treats it as terminal.

- [ ] **Step 2: Correct `validate_file_stat`'s comment**

It says the reachable producers are `retire_held_record` and an older `gascand`. Only the second remains. Say so, and keep the reasoning about why size is reported in every case.

- [ ] **Step 3: Update START-HERE**

Open item 1's residual is closed by this branch. Record what closed it, the mutation results from Tasks 4 and 6, and the fact that `read_attested_instance`'s `ENOENT` is now classified rather than waiting for Task 6 wiring.

- [ ] **Step 4: Full verification before pushing**

Run each alone, nothing else running:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/ci-check-ignored-tests.sh
```

Expected: exit 0, exit 0, 0 failed, and the ignored count matching the baseline. Record the pass/fail/ignored numbers — they go in the PR.

Re-derive the CI job set **after the last commit**, not before: `git diff --name-only main...HEAD | ./scripts/ci-classify-paths.sh`.

- [ ] **Step 5: Commit**

```bash
git add crates/gascan-core/src/daemon_protocol.rs crates/gascan/src/daemon.rs docs/status/START-HERE.md
git commit -m "docs: the three-face rule holds for every in-tree producer now"
```

---

## Self-review notes

- Spec §2.1 (retry, no new state) → Task 6. §2.2 (both halves) → Tasks 3 and 6. §2.3 (fail-closed) → Task 6 Step 4. §2.4 (`(0200, content)` terminal) → Task 5 Step 4 and Task 6 Step 7. §2.5 (shared staging vocabulary, corrected comments) → Tasks 1, 2, 8. §3 (producer) → Task 3. §3.3 (sweeper) → Task 2. §3.4 (two identities) → Task 3 Step 5. §4.1 (terminal by default) → Task 5. §4.2 (whole sequence retries) → Task 6. §4.3 (`ENOENT`) → Task 7. §5 (what moves) → Tasks 1 and 8. §6 (testing) → Tasks 4, 5, 6, 7 and Task 8 Step 4.
- Names used consistently across tasks: `stage_inert_reclaim_file`, `reclaim_staging_name`, `validate_retired_tombstone(record, staged, staged_identity)`, `raced`, `is_raced`, `RacedObservation`, `observe_once`, `DEFAULT_POLL`, `RECLAIM_STAGING_PURPOSE`.
- **Known risk, flagged rather than hidden.** Task 6 Step 5 adds a `raced` field to `Inspection` (`crates/gascan/src/daemon.rs:452`). Every `Inspection { … }` struct literal in the crate stops compiling until it sets the new field, so the compiler finds all of them for you — the mechanical half is safe. **The judgement half is not.** Setting `raced: None` at a site that should carry the marker compiles cleanly and silently disables the retry for that path, which looks exactly like the bug this plan exists to fix. The ten `Unsafe` constructions inside `observe_once` (the body that was `inspect_with`, `:999` through `:1139` before the rename) are the ones where the decision is live; the constructions elsewhere in the file — `stop_with` and `restart_with` at `:1329`, `:1340`, `:1917`, and the reclaim paths at `:2419` onward — are outside the retry and take `None`. Re-derive those line numbers before trusting them; this plan's own spec had two anchors go stale within hours.
