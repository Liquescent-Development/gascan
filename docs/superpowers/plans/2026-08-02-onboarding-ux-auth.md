# Developer Onboarding UX and Authentication Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make first-run developer configuration concise, styled, diagnostically useful, and compatible with the GitHub CLI 2.45.0 shipped in the Gas Can workspace image.

**Architecture:** Keep onboarding orchestration in `configure/onboarding.rs`, native forge operations in `configure/forge.rs`, and terminal styling behind semantic `ConfigureIo` methods implemented by `TerminalPrompter`. Use the existing bounded guest execution path, but preserve a zeroizing redaction copy long enough to turn native authentication failures into safe structured diagnostics.

**Tech Stack:** Rust 1.85, Tokio, tonic guest execution, `console` terminal styling, Clap CLI, shell-based workspace/release contract tests.

## Global Constraints

- Release target is Gas Can 0.1.18; the feature commit itself does not bump the version.
- `gascan up` remains successful when optional developer setup is skipped, cancelled, or partially fails.
- GitHub authentication must work with guest `gh 2.45.0` and must not use `--skip-ssh-key`.
- Forge tokens remain stdin-only, hidden, bounded, zeroized, and absent from argv, output, logs, receipts, and test artifacts.
- Existing safe Git identity, managed keys, forge authentication, and matching remote registrations remain idempotent.
- Color is automatic only for capable TTYs, respects `NO_COLOR`, and has stable plain-text fallback.
- Preserve the existing dirty root worktree; all implementation occurs in `.worktrees/onboarding-ux-auth`.

---

## File Map

- `crates/gascan/src/guest.rs`: zeroizing secret duplication used only for diagnostic redaction.
- `crates/gascan/src/configure/mod.rs`: structured forge error payload and semantic configure-output interface.
- `crates/gascan/src/configure/forge.rs`: portable native login argv, execution failure classification, redacted native diagnostics.
- `crates/gascan/src/configure/forge_tests.rs`: native command, diagnostic, secret, and idempotency regression tests.
- `crates/gascan/src/configure/onboarding.rs`: compact Git and forge decision flow and summaries.
- `crates/gascan/src/configure/onboarding_tests.rs`: common-path, edit, manual-token, multiple-account, skip, and partial-failure tests.
- `crates/gascan/src/configure/prompt.rs`: TTY-aware prompt/message styling.
- `crates/gascan/src/configure/tests.rs`: PTY, `NO_COLOR`, and zeroization tests.
- `crates/gascan/src/presentation.rs`: exposes read-only output-capability queries used by the configure presentation adapter.
- `README.md`: documents the actual compact first-run flow and retry behavior.
- `packaging/macos/release-smoke.sh`: asserts the portable focused GitHub CLI invocation and polished success path.
- `scripts/tests/macos_release_smoke.rs`: release-smoke contract regression coverage.

---

### Task 1: Portable GitHub Authentication and Safe Native Diagnostics

**Files:**
- Modify: `crates/gascan/src/guest.rs`
- Modify: `crates/gascan/src/configure/mod.rs`
- Modify: `crates/gascan/src/configure/forge.rs`
- Test: `crates/gascan/src/configure/forge_tests.rs`
- Test: `crates/gascan/src/configure/tests.rs`

**Interfaces:**
- Produces: `Secret::redaction_copy(&self) -> SensitiveBytes`.
- Produces: `ConfigureError::Forge { message: String, ... }` rather than a static message.
- Produces: portable `ForgeClient::login_argv` without `--skip-ssh-key`.
- Produces: a private `safe_native_diagnostic(output: &GuestOutput, secret: &[u8]) -> Option<String>` used only before the redaction copy is dropped.
- Consumes: existing `GuestRunner::execute`, `GuestOutput`, bounded captures, and zeroizing `SensitiveBytes`.

- [ ] **Step 1: Add failing command-contract and diagnostic tests**

Add tests to `configure/forge_tests.rs` that assert the exact GitHub login command and safe failure behavior:

```rust
#[tokio::test]
async fn github_login_uses_gh_2_45_compatible_arguments() -> TestResult {
    let mut runner = FakeGuestRunner::with_outputs([
        output(1, [], "authentication failed\n"),
    ]);
    let error = match configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Ssh),
    )
    .await {
        Err(error) => error,
        Ok(setup) => return Err(format!("expected failure, got {setup:?}").into()),
    };

    assert_command(
        &runner.commands[0],
        &[
            "gh", "auth", "login", "--hostname", "github.com",
            "--git-protocol", "ssh", "--with-token",
        ],
        &github_environment(),
        Some(SENTINEL.as_bytes()),
    );
    assert!(!runner.commands[0].argv.iter().any(|argument| argument == b"--skip-ssh-key"));
    Ok(())
}

#[tokio::test]
async fn native_login_failure_is_useful_bounded_and_secret_free() -> TestResult {
    let stderr = format!("HTTP 401: bad credentials\n{SENTINEL}\n\x1b]8;;https://evil.test\x07click\n");
    let mut runner = FakeGuestRunner::with_outputs([output(1, [], stderr)]);
    let error = match configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Ssh),
    )
    .await {
        Err(error) => error,
        Ok(setup) => return Err(format!("expected failure, got {setup:?}").into()),
    };
    let rendered = format!("{error}");
    assert!(rendered.contains("HTTP 401: bad credentials"));
    assert!(!rendered.contains(SENTINEL));
    assert!(!rendered.contains('\x1b'));
    assert!(rendered.contains("gascan configure gh"));
    Ok(())
}
```

Extend sensitive-drop tests in `configure/tests.rs` to obtain a redaction copy, drop it, and assert its full storage was zeroized using the existing observer machinery.

- [ ] **Step 2: Run the focused tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan configure::forge_tests::github_login_uses_gh_2_45_compatible_arguments -- --exact
rtk cargo test -p gascan configure::forge_tests::native_login_failure_is_useful_bounded_and_secret_free -- --exact
rtk cargo test -p gascan configure::tests::secret_redaction_copy_zeroizes_on_drop -- --exact
```

Expected: compilation/test failures because `--skip-ssh-key` is still emitted, native stderr is discarded, and `redaction_copy` does not exist.

- [ ] **Step 3: Implement a zeroizing redaction copy**

In `guest.rs`, add:

```rust
impl Secret {
    pub(crate) fn redaction_copy(&self) -> SensitiveBytes {
        let bytes = self.expose();
        let mut copy = SensitiveBytes::zeroed(bytes.len());
        let exceeded = copy.append_bounded(bytes);
        debug_assert!(!exceeded);
        copy
    }
}
```

Do not add `Clone` to `Secret` or expose the copy outside the crate. Extend the test-only observer hook to the returned `SensitiveBytes` in the focused zeroization test.

- [ ] **Step 4: Implement portable argv and structured native failures**

In `forge.rs`, remove the GitHub-only `--skip-ssh-key` push. Before moving the token into `GuestCommand`, create `let redaction = token.redaction_copy();`. Preserve the `Result<GuestOutput, CliError>` from the login execution instead of converting it immediately to `Option`.

For a nonzero native exit, construct the forge error with:

```rust
let message = safe_native_diagnostic(&output, redaction.expose())
    .unwrap_or_else(|| format!("native authentication exited with status {}", output.code));
```

`safe_native_diagnostic` must:

1. inspect stderr first and stdout second;
2. cap candidate input at the existing forge output bound;
3. split into lines and choose the first nonempty line;
4. replace exact secret byte sequences with `[REDACTED]` before UTF-8 conversion;
5. accept printable ASCII plus ordinary Unicode scalar values, normalize tabs to spaces, and omit C0/C1/escape control bytes; and
6. cap the rendered diagnostic to 240 Unicode scalar values.

Change `ConfigureError::Forge.message` and `forge_error` from `&'static str` to `String`. Convert existing static call sites with `.to_owned()`. Transport failures use `authentication command could not be executed`; malformed success verification keeps its existing stable message.

- [ ] **Step 5: Run focused tests and confirm GREEN**

Run:

```bash
rtk cargo test -p gascan configure::forge_tests
rtk cargo test -p gascan configure::tests::secret_redaction_copy_zeroizes_on_drop -- --exact
rtk cargo clippy -p gascan --all-targets -- -D warnings
```

Expected: all selected tests pass, no token appears in captured output, and Clippy reports no warnings.

- [ ] **Step 6: Commit Task 1**

```bash
rtk git add crates/gascan/src/guest.rs crates/gascan/src/configure/mod.rs crates/gascan/src/configure/forge.rs crates/gascan/src/configure/forge_tests.rs crates/gascan/src/configure/tests.rs
rtk git commit -m "fix: support shipped GitHub CLI authentication"
```

---

### Task 2: Compact Git and Forge Decisions

**Files:**
- Modify: `crates/gascan/src/configure/onboarding.rs`
- Test: `crates/gascan/src/configure/onboarding_tests.rs`

**Interfaces:**
- Consumes: `configure_git`, `current_git_setup`, `HostDiscovery`, `ForgeRequest`, and Task 1 diagnostics.
- Produces: private `GitChoice`, `ForgeCredentialChoice`, `choose_git_setup`, and `choose_forge_credential` orchestration helpers.
- Preserves: public aggregate/focused configure entry points and `ConfigureOutcome` semantics.

- [ ] **Step 1: Add failing common-path Git tests**

Add tests proving that complete defaults configure in one decision and existing setup can be kept:

```rust
#[tokio::test]
async fn complete_host_git_defaults_configure_with_one_confirmation() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: Some("Ada Lovelace".to_owned()),
            email: Some("ada@example.test".to_owned()),
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(Arc::clone(&events));
    io.push_confirm(true);
    let mut runner = FakeRunner::with_outputs([
        empty_status(), output(0, [], []), configured_status(GitProtocol::Ssh),
    ]);

    let outcome = configure_git_interactive(&mut runner, selector(), &discovery, &mut io).await?;
    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(events.iter().any(|event| event.contains(
        "Use this identity with SSH transport and signed commits?"
    )));
    assert!(!events.iter().any(|event| event.contains("Git name:")));
    assert!(!events.iter().any(|event| event.contains("Git email:")));
    assert!(!events.iter().any(|event| event.contains("Git protocol")));
    Ok(())
}
```

Add a second test where the shortcut is declined and the existing `Git name`, `Git email`, and `Git protocol` prompts consume prefilled/edit values. Add a third test for `Keep this Git configuration? [Y/n]` that performs no mutation when accepted.

- [ ] **Step 2: Add failing forge-choice tests**

Replace the old double-confirmation expectation with tests for:

- one detected account: `Import richardkiene at github.com? [Y/n]` immediately calls `discovery.token` after Yes and never requests a secret;
- declining one detected account: `Enter a token manually? [y/N]`, then either hidden token or skip;
- multiple accounts: a line prompt accepting `1..N`, `m`, or `s`, with no follow-up import confirmation;
- no account: `Configure GitHub with a token? [y/N]`; and
- selected-account token retrieval failure: visible fallback choice rather than automatic secret-mode transition.

Use the fake event log to assert the exact order `accounts -> selection -> token -> guest command` and assert `SENTINEL` is absent from stdout/stderr.
For both GitHub and GitLab, assert that the event log does **not** contain the
legacy outer prompts `Configure GitHub?` or `Configure GitLab?`; the credential
choice itself must be the section's only configure/import decision.

- [ ] **Step 3: Run onboarding tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan configure::onboarding_tests
```

Expected: new tests fail because the implementation still uses the redundant Git fields and import confirmation.

- [ ] **Step 4: Implement explicit decision helpers**

Introduce private choices:

```rust
enum GitChoice {
    Keep(GitSetup),
    UseHostDefaults { name: String, email: String },
    Edit,
}

enum ForgeCredentialChoice {
    Imported { hostname: String, token: Secret },
    Manual { hostname: String, token: Secret },
    Skipped,
}
```

`choose_git_setup` rules:

- existing valid setup: Keep on default Yes, Edit on No;
- no setup plus complete defaults: UseHostDefaults on default Yes, Edit on No;
- incomplete defaults: Edit without a redundant confirmation.

`UseHostDefaults` always selects `GitProtocol::Ssh`. `Edit` delegates to the existing validated name/email/protocol prompts with current/default values.

`choose_forge_credential` rules:

- zero accounts: default-No manual offer;
- one account: default-Yes direct import; No leads to default-No manual offer;
- multiple accounts: accept only `1..=len`, `m`, or `s`; selection imports immediately;
- import retrieval failure: explain that host retrieval failed and ask default-No manual entry; and
- all skip paths return `Skipped` without calling `configure_forge`.

Remove the existing outer `io.confirm("Configure {name}? [Y/n] ", true)` from
`configure_remote_section`. That function must call `choose_forge_credential`
directly and dispatch only `Imported` or `Manual` choices to `configure_forge`;
`Skipped` returns `RemoteSummary::Skipped` without another prompt. Update
summaries to use `Skipped` consistently and ensure explicit skips still permit
`complete_receipt`.

- [ ] **Step 5: Run focused onboarding and security tests**

Run:

```bash
rtk cargo test -p gascan configure::onboarding_tests
rtk cargo test -p gascan configure::tests
rtk cargo clippy -p gascan --all-targets -- -D warnings
```

Expected: all tests pass and no prompt sequence requests redundant confirmation or leaks the sentinel.

- [ ] **Step 6: Commit Task 2**

```bash
rtk git add crates/gascan/src/configure/onboarding.rs crates/gascan/src/configure/onboarding_tests.rs
rtk git commit -m "feat: streamline developer onboarding choices"
```

---

### Task 3: TTY-Aware Professional Presentation

**Files:**
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/configure/mod.rs`
- Modify: `crates/gascan/src/configure/prompt.rs`
- Modify: `crates/gascan/src/configure/onboarding.rs`
- Test: `crates/gascan/src/configure/tests.rs`
- Test: `crates/gascan/src/configure/onboarding_tests.rs`

**Interfaces:**
- Consumes: existing `OutputCapabilities::for_stdout` and `for_stderr`.
- Produces: `OutputCapabilities::color_enabled()` and `unicode_enabled()` read-only queries.
- Produces: semantic `ConfigureIo` methods `write_heading`, `write_hint`, `write_success`, `write_warning`, and `write_failure`, each with a plain default implementation.
- Produces: `ConfigurePalette` owned by `TerminalPrompter`; fake IO remains plain without ANSI-aware test logic.

- [ ] **Step 1: Add failing palette and PTY tests**

In `configure/tests.rs`, add deterministic palette tests with explicit capabilities rather than relying on the developer terminal:

```rust
#[test]
fn configure_palette_styles_semantic_messages() {
    let palette = ConfigurePalette::new(OutputCapabilities::test(true, true, true));
    assert!(palette.heading("Git").contains("\x1b["));
    assert!(palette.success("Git configured").contains("✓"));
    assert!(palette.warning("GitLab skipped").contains("\x1b["));
}

#[test]
fn configure_palette_plain_mode_has_no_ansi_or_unicode_symbols() {
    let palette = ConfigurePalette::new(OutputCapabilities::test(true, false, false));
    for rendered in [
        palette.heading("Git"),
        palette.success("Git configured"),
        palette.failure("Authentication failed"),
    ] {
        assert!(!rendered.contains("\x1b["));
        assert!(!rendered.contains('✓'));
        assert!(!rendered.contains('✗'));
    }
}
```

Add a `TerminalPrompter::from_files_with_capabilities` test constructor and PTY tests proving prompt text is cyan only when stderr capabilities permit it. Add an environment-serialized `NO_COLOR` test using the existing test process isolation pattern rather than mutating process environment concurrently.

- [ ] **Step 2: Run presentation tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan configure::tests::configure_palette -- --nocapture
rtk cargo test -p gascan configure::tests::terminal_prompter_styles_prompts -- --exact
```

Expected: compilation failures because the palette, semantic methods, and capability test constructor do not exist.

- [ ] **Step 3: Add semantic presentation primitives**

Expose read-only capability queries in `presentation.rs`:

```rust
pub(crate) const fn color_enabled(self) -> bool { self.color }
pub(crate) const fn unicode_enabled(self) -> bool { self.unicode }

#[cfg(test)]
pub(crate) const fn test(interactive: bool, color: bool, unicode: bool) -> Self {
    Self { interactive, color, unicode }
}
```

In `prompt.rs`, add `ConfigurePalette` with methods:

- `heading`: cyan bold;
- `prompt`: cyan;
- `hint`: dim;
- `success`: green with `✓ ` when Unicode is enabled;
- `warning`: yellow with `⚠ ` when Unicode is enabled; and
- `failure`: red with `✗ ` when Unicode is enabled.

Each method returns plain text when color is disabled and omits symbols when Unicode is disabled.

- [ ] **Step 4: Route wizard output through semantic methods**

Add default methods to `ConfigureIo` so fake implementations remain source-compatible:

```rust
fn write_heading(&mut self, text: &str) -> Result<(), ConfigureError> {
    self.write_err(text)
}
fn write_hint(&mut self, text: &str) -> Result<(), ConfigureError> {
    self.write_err(text)
}
fn write_success(&mut self, text: &str) -> Result<(), ConfigureError> {
    self.write_out(text)
}
fn write_warning(&mut self, text: &str) -> Result<(), ConfigureError> {
    self.write_err(text)
}
fn write_failure(&mut self, text: &str) -> Result<(), ConfigureError> {
    self.write_err(text)
}
```

Override them in `TerminalPrompter` using stdout/stderr palettes. Style prompt labels in `write_prompt`. Replace raw onboarding headings, detected/default lines, success lines, skips, and forge failures with their semantic counterparts. Preserve exact plain-text content expected by non-TTY tests.

- [ ] **Step 5: Run focused and CLI presentation tests**

Run:

```bash
rtk cargo test -p gascan configure::tests
rtk cargo test -p gascan configure::onboarding_tests
rtk cargo test -p gascan --test configure_cli
rtk cargo clippy -p gascan --all-targets -- -D warnings
```

Expected: all tests pass; TTY tests contain styling, and plain tests contain no ANSI escapes.

- [ ] **Step 6: Commit Task 3**

```bash
rtk git add crates/gascan/src/presentation.rs crates/gascan/src/configure/mod.rs crates/gascan/src/configure/prompt.rs crates/gascan/src/configure/onboarding.rs crates/gascan/src/configure/tests.rs crates/gascan/src/configure/onboarding_tests.rs
rtk git commit -m "feat: polish developer setup presentation"
```

---

### Task 4: Documentation and Release-Smoke Regression

**Files:**
- Modify: `README.md`
- Modify: `packaging/macos/release-smoke.sh`
- Test: `scripts/tests/macos_release_smoke.rs`

**Interfaces:**
- Consumes: Tasks 1-3 command and plain-text presentation contracts.
- Produces: user-facing quick-start transcript and release-smoke proof that the fake guest `gh` rejects `--skip-ssh-key` while the supported flow passes.

- [ ] **Step 1: Add a failing release-smoke command contract**

Modify the guest fake `gh` installed by `release-smoke.sh` so its `auth login`
branch fails if any argument equals `--skip-ssh-key`, records the exact argv
without token bytes, consumes stdin, and succeeds only when `--with-token`,
`--hostname`, and `--git-protocol` are present. Keep this smoke on the existing
focused `gascan configure gh --token-stdin` path: these guest fake binaries do
not participate in macOS host account discovery.

In `scripts/tests/macos_release_smoke.rs`, assert the script contains the rejecting compatibility guard and checks the resulting configured account summary. The contract must also assert the sentinel token is absent from the release-smoke transcript and fake command log.

- [ ] **Step 2: Run the release-smoke contract test and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml macos_release_smoke
```

Expected: the new assertion fails because the fake CLI does not yet reject the unsupported argument or verify the compact summary.

- [ ] **Step 3: Implement the release-smoke regression**

Update the guest fake `gh` branch and focused configuration smoke to exercise:

1. the exact portable `gh auth login` argv;
2. successful GitHub authentication and existing/added key summaries; and
3. a second idempotent focused configure pass.

Keep token input in the existing protected fake-helper channel; never place it
in argv or transcript output. Detected host-account import remains covered by
`configure/onboarding_tests.rs`, where `HostDiscovery` is injected directly,
and by Task 5's real live host-import check. Do not add a PATH-based host fake
to the privileged release smoke.

- [ ] **Step 4: Update README examples and troubleshooting**

Replace the old verbose onboarding transcript with the approved compact flow. Document:

- Enter on the host Git shortcut applies name, email, SSH transport, and signing;
- selecting/accepting a detected account immediately imports it;
- manual token and skip choices;
- automatic color with `NO_COLOR` fallback;
- partial failures retain completed work and print the real safe cause; and
- focused retries use `gascan configure git`, `gascan configure gh`, or `gascan configure glab`.

Do not claim that Gas Can upgrades `gh` or `glab`; document compatibility with the tools shipped in the workspace image.

- [ ] **Step 5: Run documentation and release contract tests**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml macos_release_smoke
rtk bash tests/release/smoke-contract.sh
rtk bash tests/release/installer-contract.sh
rtk git diff --check
```

Expected: all commands exit zero and no stale prompt text remains in README or release fixtures.

- [ ] **Step 6: Commit Task 4**

```bash
rtk git add README.md packaging/macos/release-smoke.sh scripts/tests/macos_release_smoke.rs
rtk git commit -m "docs: document streamlined developer setup"
```

---

### Task 5: Full Verification and Feature-Branch Handoff

**Files:**
- Verify only; modify files only to correct a demonstrated failure, using a new failing regression test first.

**Interfaces:**
- Consumes: all deliverables from Tasks 1-4.
- Produces: reviewable feature branch, verification evidence, and PR-ready commit series.

- [ ] **Step 1: Run formatting and repository checks**

```bash
rtk cargo fmt --all -- --check
rtk git diff --check origin/main...HEAD
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands exit zero.

- [ ] **Step 2: Run the full Rust and script test suites**

```bash
rtk env -u RUSTUP_TOOLCHAIN cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk swift test --package-path helpers/apple-attach
```

Expected: all tests pass with zero failures.

- [ ] **Step 3: Run image and release contracts**

```bash
rtk bash tests/image/shell-home-root-contract.sh
rtk bash images/workspace/tests/workstation-contract.sh
rtk bash tests/release/smoke-contract.sh
rtk bash tests/release/installer-contract.sh
```

Expected: all contract scripts exit zero.

- [ ] **Step 4: Build trusted local binaries and run live compatibility check**

```bash
rtk cargo build --workspace
rtk bash scripts/build-apple-attach-helper.sh
```

Use the built `gascan`, `gascand`, and `target/gascan-apple-attach` with the repository's documented local/live environment. Against the running sandbox, exercise `gascan configure` with retained Git identity, direct host GitHub account import, and GitLab skip. Confirm the GitHub login succeeds, `gh auth status` succeeds inside the guest, key registration is idempotent, output is styled on the TTY, and no token appears in captured diagnostics.

- [ ] **Step 5: Run macOS release smoke**

```bash
rtk env \
  GASCAN_RELEASE_GASCAN="$PWD/target/debug/gascan" \
  GASCAN_RELEASE_GASCAND="$PWD/target/debug/gascand" \
  GASCAN_RELEASE_APPLE_ATTACH_HELPER="$PWD/target/gascan-apple-attach" \
  ./packaging/macos/release-smoke.sh
```

Expected: `PASS: installed Gas Can release smoke` and exit zero. If sudo authentication is unavailable to the agent, give the exact command to the user and require its exit-zero transcript before handoff.

- [ ] **Step 6: Review branch integrity**

```bash
rtk git status --short
rtk git log --oneline origin/main..HEAD
rtk git diff --stat origin/main...HEAD
rtk git diff --check origin/main...HEAD
```

Expected: clean worktree, only intended commits/files, and no whitespace errors.

- [ ] **Step 7: Request independent code review and prepare PR**

Invoke `superpowers:requesting-code-review`. Resolve only validated findings with failing tests, rerun affected and full verification, then push `fix/onboarding-ux-auth` and open a PR describing the root cause, UX changes, security invariants, and exact verification evidence.

Do not bump the version on the feature branch. After the feature PR is squash-merged, follow the repository release runbook on a fresh release branch to bump 0.1.17 to 0.1.18, sign/notarize, create and push the signed tag, publish the GitHub release, and update the Homebrew cask.
