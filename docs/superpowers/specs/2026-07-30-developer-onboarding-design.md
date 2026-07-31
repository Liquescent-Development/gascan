# Developer Onboarding and Nested Starship Design

## Goal

Make a newly created Gas Can sandbox immediately useful for Git-based
development. Gas Can will provide an optional first-run guide and explicit
commands for configuring Git identity, SSH authentication and signing,
GitHub CLI authentication, and GitLab CLI authentication. The same release
will fix Starship initialization in nested interactive Bash processes such as
Herdr panes.

## Scope

This work adds:

- `gascan configure` for the complete guided setup.
- `gascan configure git`, `gascan configure gh`, and
  `gascan configure glab` for focused setup and repair.
- Optional host Git identity defaults and authenticated host CLI imports.
- A persistent sandbox-specific Ed25519 key used for Git SSH authentication
  and commit and tag signing.
- GitHub, GitHub Enterprise, GitLab.com, GitLab Dedicated, and self-managed
  GitLab host support.
- A one-time, optional onboarding offer after the first successful
  interactive `gascan up`.
- Correct Starship initialization in nested interactive Bash processes.
- User documentation and release validation.

This work does not add a Gas Can credential vault, copy arbitrary host
configuration files, import host private keys, or add credential-bearing
daemon protocol state.

## Command Surface

The public commands are:

```text
gascan configure
gascan configure git
gascan configure gh
gascan configure glab
```

The existing global `--sandbox <id>` selector applies. Configuration requires
one selected running sandbox. A stopped sandbox tells the user to run
`gascan up`; multiple sandboxes use the existing explicit-selection guidance.

`gascan configure` is interactive and runs these sections in order:

1. Git name and email.
2. Sandbox SSH authentication and signing key.
3. GitHub authentication and key registration.
4. GitLab authentication and key registration.
5. A concise summary of completed, skipped, and retryable work.

Each focused subcommand can be rerun independently and is idempotent.
Interactive GitHub and GitLab setup offers host credential import when
available or hidden token entry. Noninteractive authentication accepts a
token only through stdin:

```text
gascan configure gh --hostname HOST --token-stdin
gascan configure glab --hostname HOST --token-stdin
```

Both commands accept `--git-protocol ssh|https`, defaulting to `ssh`.
No `--token VALUE` option exists. Git identity setup remains interactive in
this release.

## First-Run Experience

After the first successful interactive, non-JSON `gascan up`, Gas Can asks:

```text
Set up Git, GitHub, and GitLab for this sandbox now? [Y/n]
```

Accepting launches `gascan configure`. Declining records the decision for
that sandbox and prints the explicit retry command. Cancelling or encountering
an error does not record completion, so a later successful interactive `up`
can offer setup again.

Finishing means that every section was either configured successfully or
explicitly skipped by the user. Explicitly skipped sections are summarized and
do not prevent the completed receipt.

The offer is suppressed when stdin or stderr is not a terminal, in continuous
integration, or for JSON output. Setup failure never changes a successful
`up` result into a failure.

The receipt is versioned and stored in the sandbox's persistent config volume.
It survives `down`, `up`, and workspace-image replacement and is removed by
`gascan destroy`. Explicit `gascan configure` ignores the receipt and always
remains available. The receipt contains only completion or decline state,
never identity values, hostnames, keys, or tokens.

The existing host SSH include offer remains independent. Gas Can must avoid a
wall of prompts: each feature presents one concise offer, and the configure
guide groups all developer-account questions into one walkthrough.

## Architecture

The Gas Can CLI orchestrates setup from the Mac. It reads optional host
defaults, selects a running sandbox through the existing client, and invokes
narrowly defined guest commands through the existing attachment/execution
path. The daemon transports process streams but does not interpret, retain, or
persist credentials.

Native tools own their configuration:

- `gh` writes below the existing `GH_CONFIG_DIR`.
- `glab` writes below the existing `GLAB_CONFIG_DIR`.
- Git writes a persistent global configuration below
  `/home/workspace/.config/gascan/git`.
- OpenSSH reads a persistent managed home below
  `/home/workspace/.config/gascan/git/ssh`.

The image home configurator makes the persistent Git and SSH paths available
at their conventional locations without following or replacing unsafe
collisions. Configuration code is split by responsibility:

- Host discovery reads global Git identity and authenticated CLI accounts.
- Secret input provides hidden terminal reads and exact stdin reads.
- Guest execution transports argv and stdin without a shell.
- Git setup manages identity, key generation, signing, and transport.
- GitHub and GitLab adapters invoke only reviewed native CLI commands.
- Onboarding coordinates sections and records the non-secret receipt.

No adapter parses or copies an entire host configuration file.

## Git Identity and Signing

Host defaults come only from the Mac's global Git configuration. Repository
or worktree-local Git values are deliberately ignored because a Gas Can
workspace commonly contains many repositories. The guide displays the
global name and email as editable defaults and requires confirmation before
writing them to the sandbox-global Git configuration.

The guide generates one passwordless Ed25519 key per sandbox. The private key
is a regular file owned by `workspace`, mode `0600`, below the persistent
managed SSH directory. Its directory is mode `0700`; the public key is mode
`0644`. Existing valid managed key material is reused. Unsafe types, links,
ownership, link counts, or modes fail closed and are not repaired by deleting
data.

The key is used for both Git transport authentication and SSH commit and tag
signing by default. Git receives:

- `user.name`
- `user.email`
- `gpg.format = ssh`
- `user.signingkey` pointing to the managed public key
- `commit.gpgsign = true`
- `tag.gpgsign = true`

The guide defaults Git transport to SSH and allows the user to choose HTTPS.
OpenSSH host verification remains enabled. Gas Can does not silently disable
host-key checking. For each newly registered SSH host, the setup flow runs a
visible connection verification so the user can review a new enterprise host
fingerprint before the host is considered ready.

The passwordless key is intentional: agents must be able to create signed
commits without a passphrase or long-lived agent process. The key is scoped to
one sandbox and can be revoked independently when that sandbox is destroyed
or no longer trusted.

## GitHub Authentication

The guide supports `github.com` and GitHub Enterprise hostnames. It shows the
selected account and hostname before importing a host credential.

Authentication uses:

```text
gh auth login --hostname HOST --with-token
```

The token is provided on stdin. The selected Git protocol is passed through
the native CLI without allowing its own unrelated SSH-key generation prompt.
After authentication succeeds, Gas Can verifies the guest account with the
native CLI.

For SSH transport and signing, the same public key is registered twice as
required by GitHub: once as an authentication key and once as a signing key.
Key titles identify Gas Can and the sandbox without exposing the canonical
host project path. Existing matching registrations are treated as success.

If authentication succeeds but key registration fails because the token lacks
permission, authentication remains configured. The summary explains the
partial result and gives `gascan configure gh` as the focused retry command.

## GitLab Authentication

The guide supports `gitlab.com`, GitLab Dedicated, and self-managed GitLab
hostnames. Authentication uses:

```text
glab auth login --hostname HOST --stdin
```

The token is provided on stdin, the selected Git protocol is explicit, and
the guest account is verified after login.

For SSH transport and signing, the public key is registered once with
`usage_type = auth_and_signing`. Existing matching registration is treated as
success. Authentication and key-registration failures follow the same
partial-success behavior as GitHub.

## Credential Import and Secret Handling

Interactive GitHub and GitLab setup offers two sources:

1. Import a token for an authenticated host CLI account after explicit user
   confirmation.
2. Enter a different token with terminal echo disabled.

Host import is offered only when the relevant host CLI is installed,
authenticated, and can return a token for the selected account. Otherwise the
guide proceeds directly to hidden entry. Multiple detected hosts are shown
without exposing their tokens.

A token exists transiently in the Gas Can CLI's memory and the stdin stream of
the guest CLI. It is never:

- placed in process arguments or environment variables;
- written to a Gas Can file or receipt;
- sent as structured daemon metadata;
- printed in human or JSON output;
- included in progress messages, errors, or logs.

Captured subprocess failures pass through secret redaction before display.
Tests use unique sentinel secrets and assert that they are absent from argv,
environment captures, daemon records, stdout, stderr, and serialized errors.

The guest CLIs store credentials in their native configuration within the
persistent config volume. A Linux desktop keyring is not assumed. The guide
states that native credential files are protected by Unix permissions and the
Mac's underlying storage encryption, not by a separate Gas Can vault.

## Offline and Failure Behavior

An offline sandbox can complete Git identity, key generation, and signing
configuration. Remote GitHub and GitLab authentication is skipped with a
clear explanation that the sandbox must use `network = "networked"` before
retrying.

Missing host CLIs remove only the host-import choice. Missing guest tools,
network errors, rejected tokens, insufficient token scope, enterprise TLS
errors, and SSH host-verification errors identify the failed component and
give a focused retry command.

Existing valid configuration is summarized and reused. No step regenerates a
valid key or removes an authenticated account without explicit confirmation.
Each completed component remains usable after a later component fails.

Unsafe managed Git or SSH paths fail before mutation. Publishing configuration
uses restrictive staging, exact validation, and atomic replacement where a
file must be rewritten. Gas Can does not overwrite arbitrary user-managed
configuration.

## Nested Starship Fix

The current shell hook rejects any inherited internal `STARSHIP_*` variable.
That incorrectly rejects nested interactive Bash processes launched by Herdr
or by `bash` from an already initialized Gas Can shell.

Writable inherited Starship runtime variables will no longer be classified as
a collision. The isolated Starship evaluator clears inherited runtime values
before evaluating a fresh full initialization, then transactionally replaces
the live shell state with the newly generated state.

These protections remain:

- exact immutable Starship binary, relative symlink, preset, and generated
  configuration validation;
- rejection of readonly variables that cannot be replaced;
- rejection of preexisting managed Starship function collisions;
- rejection of an inherited DEBUG trap;
- rejection of unsupported BLE integration;
- isolated evaluation with effective `errexit`;
- allowlisted state commit, syntax check, guarded apply, and rollback.

The change is a compatibility correction, not a relaxation of the immutable
image trust boundary.

## Presentation

The configure guide uses concise headings, clear current-state summaries, and
standard confirmation prompts. Secrets are never echoed. Success output names
the configured account and hostname, the Git identity, key fingerprint, Git
transport, and whether key registration succeeded, but never includes private
key bytes or tokens.

Cancellation exits cleanly without a stack trace. Errors say what failed, what
was retained, and the exact focused command to retry. JSON is not required for
the interactive aggregate guide; any machine-oriented focused options must
remain noninteractive and stable.

## Testing

Automated coverage includes:

- CLI parsing for the aggregate and focused commands.
- Sandbox selection and running-state preconditions.
- TTY, CI, redirected, and JSON first-use suppression.
- Receipt completion, explicit decline, cancellation, failure, persistence,
  and explicit reruns.
- Global-only host Git identity discovery.
- Hidden token input and exact `--token-stdin` forwarding.
- Host account discovery and explicit import confirmation.
- Default and enterprise GitHub and GitLab host flows using deterministic fake
  native CLIs.
- Authentication verification and key registration.
- Sentinel-secret absence from every observable boundary.
- Idempotent key generation and strict file and directory permissions.
- Refusal of unsafe symlinks, types, ownership, modes, and link counts.
- Git identity, SSH signing, commit and tag signing, and SSH/HTTPS transport.
- Offline and partial-success behavior.
- A nested interactive Bash launched from initialized Starship state, with no
  warning and a working prompt.
- All existing hostile-function, readonly-variable, DEBUG-trap, immutable
  input, initialization-failure, guarded-apply, and rollback Starship tests.
- Image contracts and available live lifecycle checks proving persistence
  through stop/start and image replacement.

## Documentation

The README will document:

- the first-run offer and complete guide;
- every `configure` command and non-secret option;
- host import and hidden token entry;
- default and enterprise hostnames;
- persistent credential locations and security model;
- SSH versus HTTPS trade-offs;
- authentication-and-signing key registration;
- verifying signed commits and tags;
- retrying focused setup;
- offline behavior; and
- what `gascan destroy` removes.

The quickstart will mention the optional guide so a new user can authenticate,
clone or push over SSH, and create verified commits without reading the entire
reference.

## Release

The feature branch must pass focused tests, full workspace tests, image
contract/build validation, and available live smoke checks. It then receives
independent code review. Valid findings are fixed and reverified before the
feature pull request is merged.

After merge, the release branch updates Gas Can from `0.1.16` to `0.1.17`,
runs the repository release validation and Apple signing/notarization runbook,
creates and pushes the signed release tag, publishes the GitHub release, and
updates and verifies the Homebrew cask.
