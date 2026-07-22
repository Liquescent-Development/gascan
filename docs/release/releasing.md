# Releasing Gas Can for macOS

The operator's runbook. `docs/release/macos-checklist.md` is the reference for
what each step actually does and for the manual sequence to fall back on when a
gate fails; this document is the order to do things in.

No version numbers appear below, deliberately — this file should never need
bumping.

## One-time setup

Everything here is per-machine and outlives any single release.

### 1. Signing identities

Both Developer ID certificates must be in the login keychain:

```sh
security find-identity -v | grep 'Developer ID'
```

You need one `Developer ID Application` (signs the executables) and one
`Developer ID Installer` (signs the `.pkg`).

### 2. Notarization credential

```sh
xcrun notarytool store-credentials <profile-name> \
  --key <AuthKey_XXXXXXXXXX.p8> --key-id <KEY_ID> --issuer <ISSUER_UUID>
```

`notarytool` has no command to list stored profiles, and they are not reliably
visible with `security find-generic-password` or `security dump-keychain`. The
only dependable way to check whether a profile exists is to use it:

```sh
xcrun notarytool history --keychain-profile <profile-name>
```

If you already released from this machine, a profile probably exists under a
name you chose earlier. Find it in your shell history rather than assuming, and
put *that* name in the config below — an absent profile and a wrong profile name
produce the same error.

### 3. Homebrew tap checkout

```sh
git clone git@github.com:Liquescent-Development/homebrew-tap.git ~/code/homebrew-tap
```

The tap gate requires it to be a clean work tree on `main`, level with
`origin/main`, with a push credential that works and an origin URL shaped like a
tap. It must not be the gascan repository itself.

### 4. Configuration file

`~/.config/gascan/release.env` (or `$XDG_CONFIG_HOME/gascan/release.env`):

```
GASCAN_CODESIGN_IDENTITY=Developer ID Application: Example LLC (TEAMID1234)
GASCAN_INSTALLER_SIGNING_IDENTITY=Developer ID Installer: Example LLC (TEAMID1234)
GASCAN_NOTARYTOOL_PROFILE=<profile-name>
GASCAN_TAP_PATH=/Users/you/code/homebrew-tap
```

**Do not quote the values.** This file is parsed as data, never `source`d:
everything after the first `=` is the value verbatim, so quotation marks become
part of the string and nothing will match. Spaces, colons and parentheses need
no escaping precisely because the file is not shell.

The file holds *names* — never a key, password, or token. Any value can also be
supplied by flag (`--codesign-identity`, `--installer-identity`,
`--notary-profile`, `--tap`) or environment variable, which take precedence in
that order. Nothing is defaulted; a missing value stops the run and names all
three ways to supply it.

## Every release

### 1. Bump the version

On a branch, never on `main`. Nine files:

- the six `crates/*/Cargo.toml` — the `version` line only
- `Cargo.lock` — via `cargo update --workspace --offline`, which moves only the
  workspace members and leaves third-party versions alone
- `README.md` and `docs/release/macos-checklist.md` — the version references

Leave `scripts/Cargo.lock` alone; the version it matches belongs to an unrelated
dependency. Leave `tests/release/release-script-contract.sh` alone too; the
versions there are arguments to stub-driven gates and are arbitrary.

Verify, then open a PR and merge it:

```sh
cargo metadata --locked --no-deps --format-version 1 \
  | jq -er '.packages[] | select(.name == "gascan") | .version'
cargo check --locked --workspace --all-targets
for c in tests/release/*-contract.sh; do bash "$c" >/dev/null || echo "FAIL $c"; done
```

**Commit the bump before running the contracts.** Two of them clone `HEAD` for
their behavioral cases while reading the working tree for their source
assertions, so an uncommitted bump makes them fail with misleading messages
about version mismatches and untrusted tags. Committing resolves it.

### 2. Tag and push

`release.sh` never creates, moves, or deletes a tag. That is deliberate: the tag
is the provenance anchor the whole pipeline verifies against, so it stays a
human decision.

```sh
git checkout main && git pull --ff-only
git tag -s v<version> -m 'Gas Can <version>'
git push origin v<version>
```

The tag must be annotated, signed, verifiable against your allowed-signers file,
and point exactly at `HEAD`.

### 3. Check

```sh
./packaging/macos/release.sh <version> --check
```

Seconds, and it changes nothing. It runs every gate — tools, GitHub auth,
workspace version, clean release inputs, the tag, no existing release, the two
signing identities, the notarization profile, and the tap — then renders the
cask template as a probe. Each failure prints the command that fixes it.

Fix and re-run until it prints `all release preconditions pass`.

`release source inputs are not clean` is the one gate whose message does not
name a fix, because the fix depends on what you changed. It freezes
`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `crates`, `helpers`, `proto`,
`scripts/build-apple-attach-helper.sh`, `packaging/macos`, `LICENSE`, and the
two `images/workspace` pins — and it counts untracked files, not just modified
ones, so a stray scratch file under any of those stops the release. Commit,
stash, or remove it. This is why a change to the release tooling itself cannot
be made while a release is in flight: the edit dirties an input the release is
verifying.

### 4. Release

```sh
./packaging/macos/release.sh <version>
```

Ten minutes or more, nearly all of it Apple notarization. In order: check out
the tag detached, build and sign and notarize (or reuse an already-notarized
package if one matches this exact revision and version), assert the package is
distributable, publish the GitHub release, render the cask, validate it, commit
and push the tap, and restore the branch you started on.

It prints the asset URL, the SHA-256, and the tap commit when it finishes.

### 5. Verify

```sh
brew update && brew upgrade --cask gascan
```

`docs/release/macos-checklist.md` has the fuller install-and-verify sequence and
the clean-host gate.

## When something goes wrong

**Before publish.** Nothing has happened. Fix what the gate named and re-run.

**After publish.** The GitHub release is public and the script will say so, print
the asset URL and SHA-256, and list only the steps that remain — the recipe
adapts to how far the tap work got, so follow exactly what it prints.

**Never add `--cleanup-tag`** to a `gh release delete`. It deletes the signed tag
from the remote, and every later run then refuses because the tag it verifies is
gone. Recreating it produces a different tag object. Plain
`gh release delete v<version> --yes` is the recovery for a stranded *draft*
only, and a published release is never overwritten.

**Interrupted mid-publish.** `publish.sh` writes a marker beside the package the
instant the release becomes public, and the driver reads it from its exit
handler, so an interrupted run still tells you whether a release is live. If it
says nothing and you are unsure, ask GitHub directly:

```sh
gh release view v<version> --json isDraft --jq '.isDraft'
```

## Known gaps

- Nothing in CI runs `tests/release/*` or `shellcheck`. Both are hand-run.
