# Developer Onboarding UX and Authentication Compatibility Design

## Objective

Make first-run developer setup fast, understandable, and reliable. The common
path must configure Git from detected host defaults with one decision, import a
detected GitHub or GitLab account with one selection, authenticate successfully
with the CLI versions shipped in the workspace image, and present professional
TTY-aware output. A successful `gascan up` remains successful even when optional
developer setup is skipped, cancelled, or partially fails.

The release target for this work is Gas Can 0.1.18.

## Confirmed Root Causes

The GitHub authentication failure is deterministic. Gas Can 0.1.17 invokes:

```text
gh auth login --hostname HOST --git-protocol PROTOCOL --skip-ssh-key --with-token
```

The workspace image ships Ubuntu's GitHub CLI 2.45.0, which does not support
`--skip-ssh-key`. It exits with `unknown flag: --skip-ssh-key`. Gas Can then
discards the native stderr and reports only `native authentication did not
complete`. The same account token authenticates successfully in the same guest
with the portable command that omits `--skip-ssh-key`.

Two prompt structures cause the confusing common path:

- Accepting `Configure Git identity and signing?` only enters a second set of
  name, email, and protocol prompts, even when complete host defaults were just
  displayed.
- Selecting a detected forge account leads to a second import confirmation
  whose default is No. Pressing Enter therefore abandons import and silently
  falls through to hidden token entry.

## Interaction Design

The initial post-`up` offer remains:

```text
Set up Git, GitHub, and GitLab for this sandbox now? [Y/n]
```

Accepting starts a compact `Developer setup` wizard. The common flow is:

```text
Developer setup

Git
  Host identity  Richard Kiene <richard@liquescent.dev>
  Use this identity with SSH transport and signed commits? [Y/n]
  ✓ Git configured

GitHub
  Detected  richardkiene at github.com
  Import this account? [Y/n]
  ✓ Authenticated as richardkiene
  ✓ SSH authentication and signing keys registered

GitLab
  No authenticated host account detected
  Configure GitLab with a token? [y/N]

✓ Developer setup complete
  Git       Richard Kiene <richard@liquescent.dev>
  GitHub    richardkiene at github.com
  GitLab    Skipped
```

### Git choices

When the sandbox has no Git setup and the host provides both a global name and
email, Gas Can offers one confirmation to use that identity with SSH transport
and signed commits. Accepting immediately configures the identity, managed key,
SSH transport, and commit/tag signing. It does not ask for the same values
again.

Declining the shortcut opens editable name, email, and protocol prompts, with
the host values and SSH preselected. Incomplete host defaults go directly to
the editable prompts. Existing valid sandbox configuration uses `Keep this Git
configuration? [Y/n]`; accepting performs no mutation, while declining opens
the editable prompts without regenerating its managed key unnecessarily.

### Forge choices

One detected account is shown by login and hostname. Accepting imports that
account immediately; there is no second confirmation. Declining offers explicit
`Enter a token manually? [y/N]`; declining that second, differently scoped
choice skips the forge rather than silently changing input modes.

With multiple accounts, Gas Can presents numbered accounts and a single
selection prompt with explicit `m` (manual token) and `s` (skip) choices. The
host CLI's active account is the default when one can be identified; otherwise
the prompt requires a selection. Selecting an account is the consent to
retrieve and forward its token. With no detected account, Gas Can offers manual
hidden-token setup and defaults that offer to No. Manual setup keeps hostname
editable and defaults it to `github.com` or `gitlab.com`.

Focused `gascan configure git`, `gascan configure gh`, and `gascan configure
glab` commands remain available and idempotent.

## Authentication Compatibility

GitHub authentication uses only flags supported by the shipped GitHub CLI:

```text
gh auth login --hostname HOST --git-protocol PROTOCOL --with-token
```

The token is sent through bounded guest stdin and stdin is closed after the
secret bytes. In token mode GitHub CLI does not launch its interactive SSH-key
wizard. Gas Can remains responsible for registering its per-sandbox managed
public key as both an authentication key and a signing key, then verifies SSH
when SSH transport is selected.

GitLab retains its compatible native stdin authentication flow. Both adapters
must restrict themselves to reviewed, stable native arguments and continue to
verify the authenticated account after login.

## Diagnostic Handling

Guest forge execution will no longer collapse all transport failures and
nonzero exits into a generic sentence. Internally it returns a structured
result containing the exit status and bounded stdout/stderr. On failure Gas Can
selects a concise useful native diagnostic, normalizes it to safe terminal text,
and renders it with the affected hostname and focused retry command.

Before any diagnostic is displayed or retained:

- exact occurrences of the supplied secret are redacted;
- control characters other than normalized line boundaries are rejected or
  escaped;
- total and per-line output remain bounded;
- private key material is never included; and
- raw native output is never written to the completion receipt.

Successful earlier sections remain configured after a later failure. The
summary distinguishes configured, skipped, and failed work and states what was
retained. Optional onboarding failure does not change the successful result of
`gascan up`. A failed aggregate setup does not write a completed receipt, so it
can be retried.

## Presentation

The wizard uses Gas Can's existing terminal capability detection and `console`
styling rather than embedding ANSI sequences in business logic. On a capable
TTY it uses:

- cyan section headings and prompts;
- dim styling for detected/default values;
- green checks and success text;
- yellow skips and warnings; and
- red failures.

Unicode status symbols appear only when supported. `NO_COLOR`, redirected
streams, and terminals without color receive stable plain text. Secrets remain
hidden regardless of styling. Presentation helpers consume structured wizard
events so prompt decisions and rendering can be tested independently.

## State and Idempotency

The existing persistent developer configuration and receipt model remains.
Valid managed keys, Git identity, forge authentication, and matching remote key
registrations are reused. Retrying configuration must not generate duplicate
keys or remote registrations. Changing Git identity or protocol updates only
the requested configuration while retaining safe managed key material.

Explicitly skipped sections count as complete for the receipt. Cancellation or
failure does not. `gascan destroy` continues to remove sandbox-owned persistent
developer configuration with the sandbox volumes.

## Testing

Automated coverage will prove:

- complete host Git defaults configure through one confirmation with no
  redundant value prompts;
- declining the shortcut opens prefilled editable prompts;
- incomplete defaults use the editable path;
- a single detected forge account imports through one confirmation;
- multiple-account selection, manual token, and skip are explicit;
- GitHub login arguments omit `--skip-ssh-key` and match the GitHub CLI 2.45.0
  command contract;
- GitLab's compatible stdin contract remains unchanged;
- bounded failure diagnostics retain a useful cause while redacting sentinel
  secrets and neutralizing hostile terminal bytes;
- successful partial work is accurately summarized and retained;
- retries reuse identity, keys, authentication, and registrations;
- capable TTY output has the intended hierarchy and styles;
- `NO_COLOR` and non-TTY output remain plain and stable; and
- no token or private key appears in output, logs, receipts, command arguments,
  or test artifacts.

Verification includes focused configure tests, full workspace tests, image and
installer contracts, the macOS release smoke, and an available live host-account
import retry against a sandbox.

## Delivery

Work proceeds on an isolated branch from `origin/main`, preserving the dirty
root worktree. After implementation, independent code review, and verification,
the feature is pushed as a pull request and squash-merged. A release branch then
bumps version 0.1.17 to 0.1.18, follows the repository's Apple signing and
notarization runbook, creates and pushes the signed tag, publishes the GitHub
release, and updates and verifies the Homebrew cask.
