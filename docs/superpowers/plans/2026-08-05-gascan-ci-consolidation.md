# Gas Can CI Consolidation (P2.1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Gas Can one CI pipeline covering the Rust workspace, the release
contract suite, protobuf codegen and the pinned Swift engine build, behind a single
required check that path filtering cannot deadlock.

**Architecture:** One workflow, `.github/workflows/ci.yml`, always triggered. A cheap
`changes` job classifies the PR diff into three area booleans; `rust`, `contracts` and
`engine` are conditioned on them; a `gate` job that always runs aggregates the results
and is the only required check. All classification, guarding and contract-running logic
lives in `scripts/` as POSIX shell so it is testable locally and shellcheck-able —
YAML holds no business logic.

**Tech Stack:** GitHub Actions; POSIX `sh`; Rust 1.85.0 (`rust-toolchain.toml`, with
`clippy` and `rustfmt` components); Swift 6.2 via `scripts/build-arca-engine.sh`;
runners `macos-26` and `ubuntu-24.04-arm`.

**Design spec:** `docs/superpowers/specs/2026-08-05-gascan-ci-consolidation-design.md`.
Read it before starting. Section references below (§) point into it.

## Global Constraints

- **Never commit to `main` in any repository.** Code reaches `main` via pull request.
- **Never squash- or rebase-merge either repository.** Both repos' docs cite their own
  SHAs; a squash invalidates every citation. Gas Can has no ruleset yet (§3.7), so
  until Task 8 the discipline is manual. Use `--merge`.
- **Capture exit codes directly, never through a pipe.** `cmd | tail` returns `tail`'s
  status. Redirect to a file and read `$?`, or use `if cmd; then … else rc=$?`. Five
  false "exit code 0" reports have come from this across three prior sessions.
- **No `continue-on-error`, no fallback logic, no silenced failures.** Fail fast.
- **No caching in any job** — spec D8. The engine-pin gate's value came entirely from
  being cold; a warm SwiftPM cache is what hid P1.4 for four sessions.
- **Mark every claim VERIFIED or PLAN**, and never promote a PLAN without running
  something. Past-tense claims carry their anchor inline — command, SHA, `file:line`,
  exit code — or they are not made.
- **Record corrections in place**, struck through with a pointer. Do not quietly edit a
  superseded conclusion away.
- All shell is POSIX `sh` with `set -eu`, and must pass `shellcheck` (`.shellcheckrc`
  exists at the repo root).
- Commits are signed via the 1Password SSH agent. If signing fails with "communication
  with agent failed" the app is locked — **stop and ask the maintainer**. Never fall
  back to `--no-gpg-sign`.
- `gh pr merge` and `gh api --method PUT` are sometimes refused by the permission
  classifier. Ask the maintainer to run it with `!` or to approve it. **Never** route
  around a refusal with a different tool performing the same irreversible action.

## File Structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/gascan-e2e/tests/fake_backend.rs:589-591` | Modify: the born-red assertion | 1 |
| `scripts/ci-classify-paths.sh` | Create: pure classifier — paths on stdin, `area=bool` on stdout | 2 |
| `tests/ci/classify-paths-contract.sh` | Create: contract test for the classifier | 2 |
| `scripts/ci-detect-changes.sh` | Create: resolve base/head, run `git diff`, feed the classifier, write `$GITHUB_OUTPUT` | 3 |
| `tests/ci/expected-ignored-tests.txt` | Create: the 22-entry quarantine baseline | 4 |
| `scripts/ci-check-ignored-tests.sh` | Create: regenerate and diff the ignored-test list | 4 |
| `scripts/ci-run-release-contracts.sh` | Create: run every contract, per-script exit codes | 5 |
| `.github/workflows/ci.yml` | Create: the five jobs | 6 |
| `.github/workflows/engine-pin.yml` | **Delete**: folded into `ci.yml` | 6 |
| `docs/status/arca-integration-handoff.md` | Modify: record outcomes | 10 |
| `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` | Modify: U3 resolved, P2.1 done, Arca CI step | 10 |

**Splitting the classifier from the detector is deliberate.** The classifier is a pure
function — paths in, booleans out — so it is testable with no git repository, no
GitHub, and no network. The detector holds the only impure part. A single script would
have forced the contract test to build throwaway git history.

**PR boundaries.** Task 1 is PR A on its own. Tasks 2–7 are PR B. Task 8 is a repo
setting, not code. Tasks 9–10 are PR C (docs).

---

### Task 1: Fix the born-red PTY test (PR A)

**Files:**
- Modify: `crates/gascan-e2e/tests/fake_backend.rs:589-591`
- Test: the same file — this test *is* the test

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: a green `cargo test --workspace`, which Task 6's acceptance depends on.

**Background.** §2 of the spec establishes: the assertion searches the raw PTY
transcript for `"✓ Sandbox is running"`, while `presentation.rs:636-642` emits the
marker as `"\u{1b}[32m✓\u{1b}[0m"` when color is on, so `ESC[0m` sits between the glyph
and the space. `invoke_command_with_stderr_pty` (`fake_backend.rs:318-347`) sets
`NO_COLOR=1` only when `no_color` is true, and the loop runs `no_color = false` first —
so the first iteration always has color on and always fails.

- [ ] **Step 1: Reproduce and capture the actual bytes**

This is the diagnosis step. Do not skip it and do not trust the paragraph above —
confirm it against real output.

```bash
cd ~/code/gascan
cargo test -p gascan-e2e --test fake_backend \
  tty_stderr_lifecycle_progress_updates_in_place_and_finishes_cleanly \
  > /tmp/pty-before.log 2>&1
echo "BEFORE_RC=$?"
```

Expected: `BEFORE_RC=101`, and `/tmp/pty-before.log` contains
`Error: "completion line missing from PTY transcript"`.

- [ ] **Step 2: Prove the mechanism before changing anything**

Temporarily replace the `ok_or` with a panic that dumps the transcript, so the real
bytes are visible rather than inferred. Edit `fake_backend.rs:589-591` to:

```rust
        let completion_offset = stderr.find("✓ Sandbox is running").unwrap_or_else(|| {
            panic!("no raw match; transcript was: {}", stderr.escape_debug());
        });
```

Run the test again and read the panic output.

```bash
cargo test -p gascan-e2e --test fake_backend \
  tty_stderr_lifecycle_progress_updates_in_place_and_finishes_cleanly \
  > /tmp/pty-dump.log 2>&1
echo "DUMP_RC=$?"
grep -o 'u{1b}\[32m.\{0,40\}' /tmp/pty-dump.log | head -3
```

Expected: the transcript shows `\u{1b}[32m✓\u{1b}[0m Sandbox is running` — the glyph
wrapped in SGR codes, confirming §2.

**If the dump shows something else — no `✓` at all, a truncated transcript, or a
missing completion line even after stripping ANSI — STOP.** The diagnosis is wrong,
the defect is in the CLI rather than the test, and that is a different change needing
its own design. Report to the maintainer rather than proceeding.

- [ ] **Step 3: Revert the instrumentation and apply the real fix**

Discard the Step 2 edit and change only the search string. The glyph is excluded
because it is ANSI-wrapped when color is on; the glyph itself is already asserted ten
lines below, after stripping.

```rust
        // The success marker is emitted as "\u{1b}[32m✓\u{1b}[0m" when color is on
        // (crates/gascan/src/presentation.rs:636-642), so an SGR reset sits between
        // the glyph and the text and the raw transcript never contains the two
        // adjacently. Match the uncolored tail here to locate the completion line;
        // the glyph is asserted below against the ANSI-stripped transcript.
        let completion_offset = stderr
            .find("Sandbox is running")
            .ok_or("completion line missing from PTY transcript")?;
```

Nothing else in the test changes. The offset still indexes the raw string, so the
existing `stderr[..completion_offset].contains("\r\u{1b}[2K")` redraw check at
`:592-596` keeps working — that is why the fix matches a substring rather than
switching the search to the stripped copy.

- [ ] **Step 4: Verify it passes, both iterations**

```bash
cargo test -p gascan-e2e --test fake_backend \
  tty_stderr_lifecycle_progress_updates_in_place_and_finishes_cleanly \
  > /tmp/pty-after.log 2>&1
echo "AFTER_RC=$?"
```

Expected: `AFTER_RC=0`, `1 passed; 0 failed`. The loop covers both `no_color = false`
and `no_color = true`, so one green run exercises colored and uncolored paths.

- [ ] **Step 5: Verify the whole crate is green, not just this test**

```bash
cargo test -p gascan-apple -p gascan-e2e > /tmp/rest-after.log 2>&1
echo "REST_AFTER_RC=$?"
grep 'test result' /tmp/rest-after.log | tail -3
```

Expected: `REST_AFTER_RC=0`. Baseline before the fix was `REST_RC=101` with
464 passed / 1 failed / 22 ignored; after, expect 465 passed / 0 failed / 22 ignored.

- [ ] **Step 6: Commit and open PR A**

```bash
git checkout -b fix/pty-completion-line-assertion main
git add crates/gascan-e2e/tests/fake_backend.rs
git commit -m "fix(test): match the uncolored completion tail in the PTY transcript

The assertion searched the raw PTY transcript for \"✓ Sandbox is running\",
but presentation.rs:636-642 emits the marker as \"\\u{1b}[32m✓\\u{1b}[0m\", so
an SGR reset sits between the glyph and the text and the two are never
adjacent in raw bytes. The colored iteration runs first, so the test failed
every time.

20de03d introduced the colored marker at 13:27:34; 6d01465 introduced this
assertion at 13:31:53, four minutes later. The test has never passed, and
nothing noticed because the repository has no CI."
git push -u origin fix/pty-completion-line-assertion
gh pr create --title "fix(test): match the uncolored completion tail in the PTY transcript" --body "See docs/superpowers/specs/2026-08-05-gascan-ci-consolidation-design.md §2. Red since 6d01465 (2026-07-22). Verified: AFTER_RC=0, and cargo test -p gascan-apple -p gascan-e2e goes 464 passed/1 failed to 465 passed/0 failed."
```

Ask the maintainer to merge with `--merge`. Do not squash.

---

### Task 2: The path classifier and its contract test (PR B)

**Files:**
- Create: `scripts/ci-classify-paths.sh`
- Test: `tests/ci/classify-paths-contract.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `scripts/ci-classify-paths.sh`, which reads newline-separated paths on
  stdin and writes exactly three lines to stdout in this order:
  `rust=true|false`, `contracts=true|false`, `engine=true|false`. Unmapped paths
  additionally emit `::notice::…` lines on stdout. Exit 0 unless input is unreadable.
  Task 3 consumes this by piping `git diff --name-only` into it.

- [ ] **Step 1: Write the failing contract test**

Create `tests/ci/classify-paths-contract.sh`:

```sh
#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
classify="$root/scripts/ci-classify-paths.sh"

failures=0

expect() {
  description=$1
  paths=$2
  want=$3
  got=$(printf '%s\n' "$paths" | "$classify" | grep -v '^::notice::' | tr '\n' ' ')
  got=$(printf '%s' "$got" | sed 's/ *$//')
  if test "$got" = "$want"; then
    printf 'ok   %s\n' "$description"
  else
    printf 'FAIL %s\n  want: %s\n  got:  %s\n' "$description" "$want" "$got"
    failures=$((failures + 1))
  fi
}

expect 'a crate change runs rust only' \
  'crates/gascan/src/main.rs' \
  'rust=true contracts=false engine=false'

expect 'Cargo.lock runs rust only' \
  'Cargo.lock' \
  'rust=true contracts=false engine=false'

expect 'the proto runs rust, because gascan-proto compiles it' \
  'proto/gascan/v1/gascan.proto' \
  'rust=true contracts=false engine=false'

expect 'a docs change runs contracts only' \
  'docs/status/arca-integration-handoff.md' \
  'rust=false contracts=true engine=false'

expect 'README runs contracts only' \
  'README.md' \
  'rust=false contracts=true engine=false'

expect 'the pin runs engine only' \
  'engine/arca-pin.json' \
  'rust=false contracts=false engine=true'

expect 'the engine build script runs engine and contracts' \
  'scripts/build-arca-engine.sh' \
  'rust=false contracts=true engine=true'

expect 'another script runs contracts only' \
  'scripts/produce-gascamp-bundle.sh' \
  'rust=false contracts=true engine=false'

expect 'the workflow itself runs everything' \
  '.github/workflows/ci.yml' \
  'rust=true contracts=true engine=true'

expect 'the classifier itself runs everything' \
  'scripts/ci-classify-paths.sh' \
  'rust=true contracts=true engine=true'

expect 'agent config runs nothing' \
  '.claude/settings.json' \
  'rust=false contracts=false engine=false'

expect 'an unmapped path runs everything' \
  'brand-new-directory/thing.txt' \
  'rust=true contracts=true engine=true'

expect 'areas union across several paths' \
  'crates/gascan/src/main.rs
engine/arca-pin.json' \
  'rust=true contracts=false engine=true'

expect 'empty input runs nothing' \
  '' \
  'rust=false contracts=false engine=false'

# A path with a space must not be word-split into two paths.
expect 'a path containing a space is one path' \
  'docs/a file.md' \
  'rust=false contracts=true engine=false'

notice=$(printf 'brand-new-directory/thing.txt\n' | "$classify" | grep -c '^::notice::')
if test "$notice" -ge 1; then
  printf 'ok   an unmapped path emits a notice\n'
else
  printf 'FAIL an unmapped path emits a notice\n'
  failures=$((failures + 1))
fi

quiet=$(printf 'crates/gascan/src/main.rs\n' | "$classify" | grep -c '^::notice::' || true)
if test "$quiet" -eq 0; then
  printf 'ok   a mapped path emits no notice\n'
else
  printf 'FAIL a mapped path emits no notice\n'
  failures=$((failures + 1))
fi

if test "$failures" -eq 0; then
  printf 'classify-paths: all checks passed\n'
else
  printf 'classify-paths: %d check(s) failed\n' "$failures" >&2
  exit 1
fi
```

```bash
chmod +x tests/ci/classify-paths-contract.sh
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd ~/code/gascan
./tests/ci/classify-paths-contract.sh > /tmp/classify-before.log 2>&1
echo "CLASSIFY_BEFORE_RC=$?"
tail -3 /tmp/classify-before.log
```

Expected: non-zero, because `scripts/ci-classify-paths.sh` does not exist.

- [ ] **Step 3: Write the classifier**

Create `scripts/ci-classify-paths.sh`:

```sh
#!/bin/sh
# Classify changed paths into CI areas. Pure: paths on stdin, booleans on stdout.
# Areas overlap deliberately — see the design spec §5.2.
set -eu

rust=false
contracts=false
engine=false

# Read in the current shell, not a subshell: a `printf | while` pipeline would
# discard every assignment when the subshell exits.
while IFS= read -r path; do
  test -n "$path" || continue
  case "$path" in
    # The pipeline's own definition: if it changes, run all of it.
    .github/workflows/ci.yml|scripts/ci-classify-paths.sh|scripts/ci-detect-changes.sh|scripts/ci-check-ignored-tests.sh|scripts/ci-run-release-contracts.sh|tests/ci/*)
      rust=true
      contracts=true
      engine=true
      ;;
    # Most specific first: this script is under scripts/, which also maps to
    # contracts, and both areas must fire.
    scripts/build-arca-engine.sh)
      engine=true
      contracts=true
      ;;
    engine/*)
      engine=true
      ;;
    crates/*|Cargo.toml|Cargo.lock|rust-toolchain.toml|proto/*)
      rust=true
      ;;
    tests/*|packaging/*|scripts/*|docs/*|images/*|helpers/*|README.md|LICENSE|.gitignore|.shellcheckrc|.github/*)
      contracts=true
      ;;
    # Agent and tooling configuration. Nothing in the suite asserts against these.
    .claude/*|.superpowers/*)
      ;;
    *)
      rust=true
      contracts=true
      engine=true
      printf '::notice::unmapped path %s forced every area; update scripts/ci-classify-paths.sh\n' "$path"
      ;;
  esac
done

printf 'rust=%s\n' "$rust"
printf 'contracts=%s\n' "$contracts"
printf 'engine=%s\n' "$engine"
```

```bash
chmod +x scripts/ci-classify-paths.sh
```

- [ ] **Step 4: Run the contract test and shellcheck**

```bash
./tests/ci/classify-paths-contract.sh > /tmp/classify-after.log 2>&1
echo "CLASSIFY_AFTER_RC=$?"
tail -2 /tmp/classify-after.log
shellcheck scripts/ci-classify-paths.sh tests/ci/classify-paths-contract.sh
echo "SHELLCHECK_RC=$?"
```

Expected: `CLASSIFY_AFTER_RC=0` with `classify-paths: all checks passed`, and
`SHELLCHECK_RC=0`.

- [ ] **Step 5: Commit**

```bash
git checkout -b ci/p2-1-pipeline main
git add scripts/ci-classify-paths.sh tests/ci/classify-paths-contract.sh
git commit -m "ci: add a pure path classifier with a contract test

Paths on stdin, area booleans on stdout, no git and no GitHub, so the
mapping is testable without building throwaway history. Unmapped paths
force every area and emit a notice rather than silently testing nothing."
```

---

### Task 3: The change detector (PR B)

**Files:**
- Create: `scripts/ci-detect-changes.sh`

**Interfaces:**
- Consumes: `scripts/ci-classify-paths.sh` from Task 2 — three `area=bool` lines on
  stdout plus optional `::notice::` lines.
- Produces: a script that appends `rust=`, `contracts=` and `engine=` to the file named
  by `$GITHUB_OUTPUT`, reading `$EVENT_NAME`, `$BASE_SHA` and `$HEAD_SHA` from the
  environment. Task 6's `changes` job calls it.

- [ ] **Step 1: Write the detector**

Create `scripts/ci-detect-changes.sh`:

```sh
#!/bin/sh
# Resolve the PR diff and classify it. Impure half of the change detection.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

test -n "${GITHUB_OUTPUT:-}" || {
  printf 'ci-detect-changes: GITHUB_OUTPUT is unset\n' >&2
  exit 1
}

# Path filtering applies only to pull requests. On a push there is no reliable
# base — force-pushes and the initial-push 000…0 sentinel would both need
# fallback logic — so every area runs.
if test "${EVENT_NAME:-}" != pull_request; then
  printf 'ci-detect-changes: event=%s; running every area\n' "${EVENT_NAME:-unset}"
  {
    printf 'rust=true\n'
    printf 'contracts=true\n'
    printf 'engine=true\n'
  } >>"$GITHUB_OUTPUT"
  exit 0
fi

test -n "${BASE_SHA:-}" || { printf 'ci-detect-changes: BASE_SHA is empty\n' >&2; exit 1; }
test -n "${HEAD_SHA:-}" || { printf 'ci-detect-changes: HEAD_SHA is empty\n' >&2; exit 1; }

# Diff explicit SHAs, not HEAD: actions/checkout gives pull requests a synthetic
# merge ref, so HEAD is not the PR head. Three-dot yields changes on the head
# since the merge base.
diff_file=$(mktemp)
classified=$(mktemp)
trap 'rm -f "$diff_file" "$classified"' EXIT INT TERM HUP
git diff --name-only "$BASE_SHA...$HEAD_SHA" >"$diff_file"

printf 'ci-detect-changes: %s changed path(s)\n' "$(wc -l <"$diff_file" | tr -d ' ')"

# Write the classifier's output to a file and read its status directly. Piping
# it into grep would make $? grep's status, not the classifier's — the trap this
# plan's Global Constraints exist to prevent.
"$root/scripts/ci-classify-paths.sh" <"$diff_file" >"$classified"
classify_rc=$?
test "$classify_rc" -eq 0 || {
  printf 'ci-detect-changes: classifier exited %d\n' "$classify_rc" >&2
  exit "$classify_rc"
}

# Notices go to the log; only key=value pairs reach the output file.
grep '^::notice::' "$classified" || true
grep -v '^::notice::' "$classified" >>"$GITHUB_OUTPUT"
cat "$GITHUB_OUTPUT"
```

```bash
chmod +x scripts/ci-detect-changes.sh
```

- [ ] **Step 2: Test it locally against real history**

Simulate a pull request between two real commits on `main`.

```bash
cd ~/code/gascan
GITHUB_OUTPUT=$(mktemp)
export GITHUB_OUTPUT
EVENT_NAME=pull_request \
BASE_SHA=$(git rev-parse main~1) \
HEAD_SHA=$(git rev-parse main) \
  ./scripts/ci-detect-changes.sh > /tmp/detect.log 2>&1
echo "DETECT_RC=$?"
cat "$GITHUB_OUTPUT"
```

Expected: `DETECT_RC=0`, and `$GITHUB_OUTPUT` holds exactly three lines
`rust=…`, `contracts=…`, `engine=…`. `main~1..main` is the docs-only merge `4905e2b`,
so expect `rust=false contracts=true engine=false`.

- [ ] **Step 3: Test the push path and the failure paths**

```bash
GITHUB_OUTPUT=$(mktemp); export GITHUB_OUTPUT
EVENT_NAME=push ./scripts/ci-detect-changes.sh > /tmp/detect-push.log 2>&1
echo "PUSH_RC=$?"; cat "$GITHUB_OUTPUT"
```

Expected: `PUSH_RC=0` and all three `true`.

```bash
GITHUB_OUTPUT=$(mktemp); export GITHUB_OUTPUT
EVENT_NAME=pull_request BASE_SHA= HEAD_SHA=abc ./scripts/ci-detect-changes.sh \
  > /tmp/detect-nobase.log 2>&1
echo "NOBASE_RC=$?"; cat /tmp/detect-nobase.log
```

Expected: non-zero with `BASE_SHA is empty`. Then unset `GITHUB_OUTPUT` entirely and
confirm it also fails loudly:

```bash
env -u GITHUB_OUTPUT EVENT_NAME=push ./scripts/ci-detect-changes.sh \
  > /tmp/detect-nooutput.log 2>&1
echo "NOOUTPUT_RC=$?"; cat /tmp/detect-nooutput.log
```

Expected: non-zero with `GITHUB_OUTPUT is unset`.

- [ ] **Step 4: shellcheck and commit**

```bash
shellcheck scripts/ci-detect-changes.sh
echo "SHELLCHECK_RC=$?"
git add scripts/ci-detect-changes.sh
git commit -m "ci: resolve the PR diff and feed the path classifier

Diffs explicit base and head SHAs rather than HEAD, because actions/checkout
gives pull requests a synthetic merge ref. Non-pull_request events run every
area rather than deriving a base that would need fallback logic for
force-pushes and the initial-push sentinel."
```

---

### Task 4: The quarantine guard (PR B)

**Files:**
- Create: `tests/ci/expected-ignored-tests.txt`
- Create: `scripts/ci-check-ignored-tests.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: a script exiting 0 when the workspace's ignored-test set matches the
  baseline file, non-zero with a unified diff otherwise. Task 6's `rust` job runs it as
  its final step.

**Known coarseness, to record rather than hide:** `cargo test -- --ignored --list`
emits bare test paths with no binary prefix, so the baseline is name-only. A rename
that swapped two identical names between two test binaries would pass this guard. That
is an accepted limitation, not an oversight.

- [ ] **Step 1: Generate the baseline from the harness, not by hand**

```bash
cd ~/code/gascan
mkdir -p tests/ci
cargo test --workspace -- --ignored --list 2>/dev/null \
  | sed -n 's/: test$//p' | sort > tests/ci/expected-ignored-tests.txt
echo "GEN_RC=$?"
wc -l < tests/ci/expected-ignored-tests.txt
```

Expected: `GEN_RC=0` and **22**. The 22 entries, for review — `sort` order:

```
apply_installs_large_npm_tool_and_neovim_with_storage_override
attach::attach_preserves_binary_streams_and_exact_exit_codes
attach::attached_process_forwards_sigint_and_closes_stdin
attach::attached_process_reports_resize_signal_and_exit
attach::unsupported_signal_matrix_returns_promptly
backend_contract::backend_contract
changed_setup_is_reported_but_not_run_by_up_or_shell
cli_lifecycle_survives_daemon_and_host_state_changes
cli_recovers_from_stale_daemon_metadata_and_runtime_truth
developer_configuration_persists_across_restart_and_image_replacement
image_replace_preserves_durable_resources_and_rolls_back_failure
lifecycle::stop_start_are_idempotent_and_inspect_is_structured
managed_shell_prompts_match_ssh_and_activate_offline
native_ssh_is_loopback_only_durable_reconciled_and_cleaned
network::offline_workspace_cannot_reach_external_or_host_networks
real_macos_security_acceptance
resources::cpu_and_memory_limits_are_observable_in_guest
resources::published_port_is_reachable_only_through_loopback_binding
storage::bind_mount_is_exact_and_named_volume_persists
storage::independently_sized_managed_volumes_are_exact_and_cleanup
workstation_defaults_are_exact_credential_free_and_offline
workstation_tools_override_wins_without_mutating_immutable_defaults
```

If the count is not 22, stop and find out why before continuing — the number is
VERIFIED against this tree and a change means something moved.

- [ ] **Step 2: Write the guard**

Create `scripts/ci-check-ignored-tests.sh`:

```sh
#!/bin/sh
# Fail if the set of #[ignore]d tests drifts from the recorded baseline, in
# either direction: a new quarantine, or a heavy test that vanished.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

expected=tests/ci/expected-ignored-tests.txt
test -f "$expected" || {
  printf 'ci-check-ignored-tests: %s is missing\n' "$expected" >&2
  exit 1
}

listing=$(mktemp)
actual=$(mktemp)
trap 'rm -f "$listing" "$actual"' EXIT INT TERM HUP

cargo test --workspace -- --ignored --list >"$listing" 2>/dev/null
list_rc=$?
test "$list_rc" -eq 0 || {
  printf 'ci-check-ignored-tests: listing exited %d\n' "$list_rc" >&2
  exit "$list_rc"
}

sed -n 's/: test$//p' "$listing" | sort >"$actual"

if diff -u "$expected" "$actual"; then
  printf 'ci-check-ignored-tests: %s ignored test(s), matching the baseline\n' \
    "$(wc -l <"$actual" | tr -d ' ')"
else
  printf '\nci-check-ignored-tests: the ignored-test set changed.\n' >&2
  printf 'If deliberate, regenerate the baseline and say why in the commit:\n' >&2
  printf '  cargo test --workspace -- --ignored --list 2>/dev/null \\\n' >&2
  printf '    | sed -n %ss/: test$//p%s | sort > %s\n' "'" "'" "$expected" >&2
  exit 1
fi
```

```bash
chmod +x scripts/ci-check-ignored-tests.sh
```

- [ ] **Step 3: Verify it passes on the unchanged tree**

```bash
./scripts/ci-check-ignored-tests.sh > /tmp/guard-green.log 2>&1
echo "GUARD_GREEN_RC=$?"
tail -1 /tmp/guard-green.log
```

Expected: `GUARD_GREEN_RC=0` and `22 ignored test(s), matching the baseline`.

- [ ] **Step 4: Prove the guard can go red — a guard that never fails is not a guard**

Add a throwaway `#[ignore]` to a hermetic test, run the guard, then revert.

```bash
cd ~/code/gascan
python3 - <<'PY'
import pathlib
p = pathlib.Path("crates/gascan-e2e/tests/version.rs")
s = p.read_text()
i = s.index("#[test]")
p.write_text(s[:i] + '#[ignore = "temporary guard mutation"]\n' + s[i:])
PY
./scripts/ci-check-ignored-tests.sh > /tmp/guard-red.log 2>&1
echo "GUARD_RED_RC=$?"
grep -c '^+' /tmp/guard-red.log
git checkout -- crates/gascan-e2e/tests/version.rs
./scripts/ci-check-ignored-tests.sh > /tmp/guard-green-again.log 2>&1
echo "GUARD_GREEN_AGAIN_RC=$?"
```

Expected: `GUARD_RED_RC=1` with a unified diff showing the added name, then
`GUARD_GREEN_AGAIN_RC=0` after reverting. **Record both exit codes in the PR body.**

- [ ] **Step 5: shellcheck and commit**

```bash
shellcheck scripts/ci-check-ignored-tests.sh
echo "SHELLCHECK_RC=$?"
git status --short   # confirm version.rs is NOT modified
git add scripts/ci-check-ignored-tests.sh tests/ci/expected-ignored-tests.txt
git commit -m "ci: guard the ignored-test set against drift in both directions

22 tests are #[ignore]d because they need an Apple runtime, digest-pinned
workspace images and OpenSSH. The guard fails if that set grows (quarantine
creep) or shrinks (a heavy test silently deleted). Proven to go red by
adding a throwaway #[ignore] and back to green on revert."
```

---

### Task 5: The release-contract runner (PR B)

**Files:**
- Create: `scripts/ci-run-release-contracts.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: a script running every `tests/release/*-contract.sh` and every
  `tests/ci/*-contract.sh`, reporting each script's exit code separately, exiting
  non-zero if any failed. Task 6's `contracts` job calls it.

**Deviation from the spec, recorded deliberately.** §5.1 says "the 14
`tests/release/*-contract.sh`". This runner also picks up `tests/ci/*-contract.sh` so
Task 2's classifier contract is executed by CI rather than only by hand. That is an
extension of the spec's intent, not a contradiction of it, and it is why
`tests/ci/*` maps to every area in Task 2's classifier.

- [ ] **Step 1: Write the runner**

Create `scripts/ci-run-release-contracts.sh`:

```sh
#!/bin/sh
# Run every contract script, reporting each exit code separately so a failure
# names the script rather than an aggregate.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

status=0
count=0

for script in tests/release/*-contract.sh tests/ci/*-contract.sh; do
  test -f "$script" || continue
  count=$((count + 1))
  if "$script" >/dev/null 2>&1; then
    printf 'ok   %s\n' "$script"
  else
    rc=$?
    printf 'FAIL %s rc=%d\n' "$script" "$rc"
    printf '--- output of %s ---\n' "$script"
    "$script" 2>&1 || true
    printf '--- end %s ---\n' "$script"
    status=1
  fi
done

test "$count" -gt 0 || {
  printf 'ci-run-release-contracts: no contract scripts matched\n' >&2
  exit 1
}

printf 'ci-run-release-contracts: %d contract(s), status=%d\n' "$count" "$status"
exit "$status"
```

The `if "$script"; then … else rc=$?` shape is deliberate: `set -e` does not fire on a
command used as an `if` condition, and `$?` inside the `else` branch is that command's
status. This is how the exit code is captured without a pipe.

The re-run on failure is intentional — the first run is quiet so a green log stays
short, and the second surfaces the diagnostics only when they are needed. If a contract
is not idempotent this would show two different failures; none of the 14 mutate state
outside `mktemp` directories, having all exited 0 on a machine with no notarization
credentials (spec §3.5).

```bash
chmod +x scripts/ci-run-release-contracts.sh
```

- [ ] **Step 2: Run it and confirm the count**

```bash
cd ~/code/gascan
./scripts/ci-run-release-contracts.sh > /tmp/contracts.log 2>&1
echo "CONTRACTS_RC=$?"
tail -2 /tmp/contracts.log
grep -c '^ok' /tmp/contracts.log
```

Expected: `CONTRACTS_RC=0`, final line `15 contract(s), status=0` — the 14 release
contracts plus Task 2's classifier contract — and 15 `ok` lines.

- [ ] **Step 3: shellcheck and commit**

```bash
shellcheck scripts/ci-run-release-contracts.sh
echo "SHELLCHECK_RC=$?"
git add scripts/ci-run-release-contracts.sh
git commit -m "ci: run every contract script with per-script exit codes

Captures each script's status via an if-condition rather than a pipe, so a
failure names the script. Also runs tests/ci/*-contract.sh so the path
classifier's contract executes in CI."
```

---

### Task 6: The workflow (PR B)

**Files:**
- Create: `.github/workflows/ci.yml`
- Delete: `.github/workflows/engine-pin.yml`

**Interfaces:**
- Consumes: `scripts/ci-detect-changes.sh` (Task 3), `scripts/ci-check-ignored-tests.sh`
  (Task 4), `scripts/ci-run-release-contracts.sh` (Task 5), and the existing
  `scripts/build-arca-engine.sh`.
- Produces: the check name **`ci / gate`**, which Task 8's ruleset requires by that
  exact string.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/ci.yml`:

```yaml
name: ci

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  changes:
    runs-on: ubuntu-24.04-arm
    timeout-minutes: 5
    outputs:
      rust: ${{ steps.detect.outputs.rust }}
      contracts: ${{ steps.detect.outputs.contracts }}
      engine: ${{ steps.detect.outputs.engine }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Classify the changed paths
        id: detect
        env:
          EVENT_NAME: ${{ github.event_name }}
          BASE_SHA: ${{ github.event.pull_request.base.sha }}
          HEAD_SHA: ${{ github.event.pull_request.head.sha }}
        run: ./scripts/ci-detect-changes.sh

  rust:
    needs: changes
    if: needs.changes.outputs.rust == 'true'
    runs-on: macos-26
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4

      - name: Report toolchain
        run: |
          rustc --version
          cargo --version
          sw_vers

      - name: Formatting
        run: cargo fmt --all --check

      - name: Lints
        run: cargo clippy --workspace --all-targets -- -D warnings

      - name: Tests
        run: cargo test --workspace

      - name: Guard the ignored-test set
        run: ./scripts/ci-check-ignored-tests.sh

  contracts:
    needs: changes
    if: needs.changes.outputs.contracts == 'true'
    runs-on: macos-26
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4

      - name: Release and CI contracts
        run: ./scripts/ci-run-release-contracts.sh

  engine:
    needs: changes
    if: needs.changes.outputs.engine == 'true'
    runs-on: macos-26
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4

      - name: Report toolchain
        run: |
          swift --version
          sw_vers

      - name: Build the pinned Arca engine
        run: ./scripts/build-arca-engine.sh

  gate:
    needs: [changes, rust, contracts, engine]
    if: always()
    runs-on: ubuntu-24.04-arm
    timeout-minutes: 5
    steps:
      - name: Require every job to have succeeded or been skipped
        env:
          CHANGES: ${{ needs.changes.result }}
          RUST: ${{ needs.rust.result }}
          CONTRACTS: ${{ needs.contracts.result }}
          ENGINE: ${{ needs.engine.result }}
        run: |
          set -eu
          printf 'changes=%s rust=%s contracts=%s engine=%s\n' \
            "$CHANGES" "$RUST" "$CONTRACTS" "$ENGINE"
          test "$CHANGES" = success || {
            printf 'gate: changes must succeed, was %s\n' "$CHANGES" >&2
            exit 1
          }
          status=0
          for result in "$RUST" "$CONTRACTS" "$ENGINE"; do
            case "$result" in
              success|skipped) ;;
              *) printf 'gate: a job reported %s\n' "$result" >&2; status=1 ;;
            esac
          done
          exit "$status"
```

Note there is **no cache step anywhere** — spec D8. Do not add one.

- [ ] **Step 2: Delete the folded-in workflow**

```bash
cd ~/code/gascan
git rm .github/workflows/engine-pin.yml
```

`scripts/build-arca-engine.sh` is not touched; the `engine` job runs it verbatim. The
`engine-pin / build-engine` check name retires, which is safe because no ruleset
references it — `gh api repos/Liquescent-Development/gascan/rulesets` returned `[]`.

- [ ] **Step 3: Validate the YAML parses before pushing**

```bash
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); print(sorted(d['jobs']))"
echo "YAML_RC=$?"
```

Expected: `['changes', 'contracts', 'engine', 'gate', 'rust']` and `YAML_RC=0`.

- [ ] **Step 4: Commit and open PR B**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: one pipeline behind a single required check

Five jobs: changes classifies the diff, rust/contracts/engine are
conditioned on it, and gate always runs and aggregates. gate is the only
check worth requiring, because a workflow skipped by an on-level paths
filter leaves its check Pending forever and blocks the PR — which is why
engine-pin.yml is folded in here rather than left standalone and required.

No caching, deliberately: the engine-pin gate's value came from being cold."
git push -u origin ci/p2-1-pipeline
gh pr create --title "ci: Gas Can's first consolidated pipeline (P2.1)" --body "Implements docs/superpowers/specs/2026-08-05-gascan-ci-consolidation-design.md. Local evidence: classifier contract green, guard proven red then green, 15 contracts status=0. Gate mutation evidence to follow in a comment."
```

- [ ] **Step 5: Confirm the docs-only skip path on the real PR**

PR B touches `.github/workflows/ci.yml` and `scripts/ci-*`, so every area runs — that
is the all-true case, not the skip case. Watch this run first:

```bash
gh pr checks --watch
gh run list -L 1 --json databaseId,conclusion,url -q '.[0]'
```

Expected: `ci / gate` **success**, with `rust`, `contracts` and `engine` all
`success`. Record the run URL.

- [ ] **Step 6: Prove the gate goes red — mutation, not inspection**

A gate that has only ever been green proves nothing about its aggregation. Break
formatting deliberately, confirm red, then revert.

```bash
cd ~/code/gascan
printf '\n\n\n   \n' >> crates/gascan-proto/src/lib.rs
git add crates/gascan-proto/src/lib.rs
git commit -m "test: temporarily break formatting to prove the gate reddens"
git push
gh pr checks --watch || true
gh run list -L 1 --json databaseId,conclusion,url -q '.[0]'
```

Expected: `rust` **failure** on `cargo fmt --all --check`, and `ci / gate`
**failure**. Record this run URL — it is the evidence that the aggregation works.

```bash
git revert --no-edit HEAD
git push
gh pr checks --watch
```

Expected: back to `ci / gate` success. Record that URL too, then post all three URLs
as a PR comment.

**If `gate` reports success while `rust` failed, stop.** The aggregation is broken and
nothing downstream is trustworthy.

- [ ] **Step 7: Confirm the skip path, which is the whole point of D2**

Open a throwaway docs-only PR from `main` to exercise the case that a workflow-level
path filter would have deadlocked.

```bash
git checkout -b ci/verify-skip-path main
printf '\n' >> docs/status/arca-integration-handoff.md
git commit -am "docs: whitespace, to exercise the CI skip path"
git push -u origin ci/verify-skip-path
gh pr create --title "docs: exercise the CI skip path" --body "Throwaway. Confirms ci / gate goes green with rust and engine skipped."
gh pr checks --watch
```

Expected: `ci / gate` **success**, `contracts` success, `rust` and `engine`
**skipped**. Record the URL, then close the PR without merging and delete the branch.

```bash
gh pr close ci/verify-skip-path --delete-branch
```

Then ask the maintainer to merge PR B with `--merge`.

---

### Task 7: Settle the hosted-runner capability question (PR B or standalone)

**Files:**
- Temporarily add a job to `.github/workflows/ci.yml`; remove it before merge.

**Interfaces:**
- Consumes: the existing `scripts/apple-test-preflight.sh`.
- Produces: a VERIFIED answer replacing the PLAN in spec §3.4, consumed by Task 10.

- [ ] **Step 1: Add the probe job**

Append to `.github/workflows/ci.yml`, and note it is deliberately outside `gate`'s
`needs:` so its result cannot block anything:

```yaml
  runtime-probe:
    if: github.event_name == 'pull_request'
    runs-on: macos-26
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v4

      - name: Probe for an Apple container runtime
        run: |
          set -eu
          uname -s
          uname -m
          sw_vers
          ./scripts/apple-test-preflight.sh
```

- [ ] **Step 2: Push, read the result, record it verbatim**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: probe whether a hosted runner has an Apple container runtime"
git push
gh run list -L 1 --json databaseId -q '.[0].databaseId'
gh run view <id> --log --job runtime-probe > /tmp/probe.log 2>&1
echo "PROBE_VIEW_RC=$?"
cat /tmp/probe.log
```

Record the run URL and the verbatim output. **Both outcomes are useful and neither is
a failure of this task:**

- `container system version` fails → spec §3.4's PLAN is promoted to VERIFIED, and D4
  stands with evidence rather than belief.
- `container system version` succeeds → D4 reopens in the project's favour. Do **not**
  start wiring the heavy tier; note it for the maintainer as newly-possible work,
  since the digest-pinned candidate and predecessor images are still unsolved
  (`run-apple-e2e.sh:10-60`).

- [ ] **Step 3: Remove the probe job and commit**

```bash
cd ~/code/gascan
# Delete the runtime-probe job block from .github/workflows/ci.yml.
python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/ci.yml')); assert 'runtime-probe' not in d['jobs'], 'probe job still present'; print(sorted(d['jobs']))"
git add .github/workflows/ci.yml
git commit -m "ci: remove the runtime probe, its answer is recorded

Kept out of gate's needs while it ran, so it never gated anything. Result
recorded in the design spec."
git push
```

---

### Task 8: Make the checks load-bearing

**Files:** none — this is a repository setting.

**Interfaces:**
- Consumes: the check name `ci / gate` from Task 6, which must already have passed on a
  real PR.
- Produces: enforcement.

**Do not start this until PR B is merged and `ci / gate` has passed on a real pull
request.** Never require a check that has never passed.

- [ ] **Step 1: Confirm the preconditions**

```bash
cd ~/code/gascan
git fetch origin && git log origin/main -1 --format='%h %s' | cat
gh api repos/Liquescent-Development/gascan/rulesets
gh run list -L 3 --json workflowName,conclusion,headSha \
  -q '.[] | "\(.workflowName) \(.conclusion) \(.headSha[0:7])"'
```

Expected: PR B's merge commit on `origin/main`, rulesets still `[]`, and a recent
`ci` run with `conclusion=success`.

- [ ] **Step 2: Hand the ruleset call to the maintainer**

This is an irreversible repository-administration change and the permission classifier
may refuse it. **Ask the maintainer to run it**, prefixed with `!`:

```
! gh api -X POST repos/Liquescent-Development/gascan/rulesets \
  -f name='main' \
  -f target='branch' \
  -f enforcement='active' \
  -F 'conditions[ref_name][include][]=~DEFAULT_BRANCH' \
  -F 'rules[][type]=pull_request' \
  -F 'rules[][parameters][allowed_merge_methods][]=merge' \
  -F 'rules[][type]=required_status_checks' \
  -F 'rules[][parameters][required_status_checks][][context]=ci / gate' \
  -F 'rules[][parameters][strict_required_status_checks_policy]=false'
```

If the API rejects the nested-array form, fall back to writing the ruleset JSON to a
file and posting it with `--input`. Do **not** silently switch to the legacy
branch-protection endpoint, which has different semantics.

- [ ] **Step 3: Verify enforcement, from the outside**

```bash
gh api repos/Liquescent-Development/gascan/rulesets \
  -q '.[] | "\(.id) \(.name) \(.enforcement)"'
gh api repos/Liquescent-Development/gascan/rulesets/<id> \
  -q '.rules[] | "\(.type) \(.parameters)"'
```

Expected: one active ruleset on the default branch, `required_status_checks`
containing exactly `ci / gate`, and `allowed_merge_methods` exactly `["merge"]`.

- [ ] **Step 4: Confirm it actually blocks**

Open a throwaway PR whose `ci / gate` fails, and confirm `gh pr merge` refuses.

```bash
git checkout -b ci/verify-enforcement main
printf '\n\n\n   \n' >> crates/gascan-proto/src/lib.rs
git commit -am "test: temporarily break formatting to prove enforcement"
git push -u origin ci/verify-enforcement
gh pr create --title "test: prove the ruleset blocks a red gate" --body "Throwaway."
gh pr checks --watch || true
gh pr view --json mergeable,mergeStateStatus -q '{mergeable,mergeStateStatus}'
```

Expected: `mergeStateStatus` is `BLOCKED`. **A ruleset that does not block is not
enforcement** — if it reports `CLEAN`, the ruleset is misconfigured. Then:

```bash
gh pr close ci/verify-enforcement --delete-branch
```

---

### Task 9: Promote the spec's PLAN claims (PR C)

**Files:**
- Modify: `docs/superpowers/specs/2026-08-05-gascan-ci-consolidation-design.md`

**Interfaces:**
- Consumes: Task 6's three run URLs, Task 7's probe output, Task 8's verification.
- Produces: a spec whose claims all carry anchors.

- [ ] **Step 1: Replace §3.4's PLAN with the measured answer**

Strike the PLAN through in place with a pointer; do not delete it. Append to §3.4,
filling only the bracketed values from Task 7's output:

```markdown
> ~~**PLAN, explicitly not verified:** that a GitHub-hosted `macos-26` runner cannot
> run these…~~ **Settled 2026-08-__ by measurement.** VERIFIED: run
> `https://github.com/Liquescent-Development/gascan/actions/runs/<id>`, job
> `runtime-probe`, `conclusion=<success|failure>`. `scripts/apple-test-preflight.sh`
> reported `uname -s`=`<value>`, `uname -m`=`<value>`, and
> `container system version` <exited 0 with `<output>` | failed with `<message>`>.
> Consequence for D4: <D4 stands on evidence | D4 reopens — a hosted runner does carry
> the runtime, though the digest-pinned candidate and predecessor images
> (`run-apple-e2e.sh:10-60`) remain unsolved>.
```

- [ ] **Step 2: Add a verification record to §7**

Append a `### 7.4 Verification record` section, one row per run, filling in the IDs:

```markdown
### 7.4 Verification record

| Claim | Run | Result |
|---|---|---|
| Every area green on a full-surface PR | `<id>` | `ci / gate` success; rust, contracts, engine all success |
| The gate reddens when a job fails | `<id>` | `rust` failure on `cargo fmt --all --check`; `ci / gate` **failure** |
| Reverting returns it to green | `<id>` | `ci / gate` success |
| Docs-only PR skips rust and engine | `<id>` | `ci / gate` success; rust and engine `skipped`; contracts success |
| The guard reddens on quarantine creep | local | `GUARD_RED_RC=1`, then `GUARD_GREEN_AGAIN_RC=0` on revert |
| The ruleset blocks a red gate | PR `<n>` | `mergeStateStatus=BLOCKED` |

The second row is the one that matters. A gate that has only ever been green proves
nothing about its aggregation, and this design puts every check behind one job.
```

- [ ] **Step 3: Commit**

```bash
git checkout -b docs/record-p2-1-outcomes main
git add docs/superpowers/specs/2026-08-05-gascan-ci-consolidation-design.md
git commit -m "docs: promote P2.1's PLAN claims with run anchors"
```

---

### Task 10: Update the handoff and roadmap (PR C)

**Files:**
- Modify: `docs/status/arca-integration-handoff.md`
- Modify: `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`

- [ ] **Step 1: Amend the roadmap**

- Mark P2.1 done, pointing at the spec. Note that **P2 stays open** because P2.2 cannot
  attest an engine binary until P5.1 produces one
  (`2026-08-05-arca-engine-pin-design.md` §2.3, §7).
- Resolve **U3** in place, struck through, with the numbers: 1:00.07 for 902 tests and
  1:57.82 to compile every test binary, both warm, against 7m21s–8m38s for the engine
  build. Answer: path filters are nice, not mandatory, and earn their keep on the
  `engine` job alone.
- Add the **Arca CI** step per spec §9: `swift build` and `swift test` against a
  characterized baseline (Arca has 125 failing tests on both sides of P1.4,
  `handoff:716-721`, so the baseline must be pinned down first), plus the Go
  `arca-services` cross-compile from `scripts/build-vminit.sh:54-80`. Note it is wanted
  **before** P4.3 and P5.1 land, so Arca-side changes stop getting their first build
  inside Gas Can's pipeline at pin-bump time.
- Correct the P2.1 row in place: it names Swift, Rust, Go and protobuf codegen, but Go
  is not in Gas Can (`find . -name "*.go"` matches only `.artifacts/` build output) and
  protobuf codegen needs no step (`crates/gascan-proto/build.rs:4` uses
  `protoc_bin_vendored`).

- [ ] **Step 2: Amend the handoff**

Add a "P2.1 complete" section recording: the born-red test with both commit anchors
(`20de03d` 13:27:34 → `6d01465` 13:31:53, four minutes) as the argument for CI paying
for itself immediately; the 9-runs-all-engine-pin starting point; the required-check
trap from GitHub's docs with both quotes; the five-job topology and why `gate` is the
only required check; and the ruleset now enforcing merge-only.

Also record the calibration: **a green gate proves nothing until it has been made to
go red**, with Task 6 Step 6's two run URLs.

- [ ] **Step 3: Commit and open PR C**

```bash
git add docs/status/arca-integration-handoff.md \
        docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md
git commit -m "docs: record P2.1, resolve U3, and name Arca's CI step"
git push -u origin docs/record-p2-1-outcomes
gh pr create --title "docs: record P2.1 outcomes and resolve U3" --body "Records the born-red test, the required-check trap, U3's numbers, and Arca CI as a named roadmap step."
```

Ask the maintainer to merge with `--merge`. Note that PR C touches only `docs/**`, so
`ci / gate` should go green with `rust` and `engine` **skipped** — a live confirmation
of the skip path on a PR that matters.

---

## Out of scope, deliberately

- **`workspace-bundles.yml`** — 459 lines, never executed once, triggered only by
  pushes to `feature/provisioning`. Spec §11. A real liability, but its own decision.
- **The heavy Apple e2e tier** — spec D4. Task 7 only measures whether a hosted runner
  could host it.
- **Arca's CI** — spec §9, written into the roadmap by Task 10 as a step, not built here.
- **P2.2** — blocked on P5.1 producing an engine binary.
- **Caching** — spec D8.
