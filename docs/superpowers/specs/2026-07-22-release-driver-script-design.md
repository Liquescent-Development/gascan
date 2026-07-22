# One-command macOS release driver

## Problem

Cutting a release means running roughly a dozen commands across two
repositories, in order, with three environment variables set correctly and a
SHA-256 copied by hand from one command's output into another's argument.
`docs/release/macos-checklist.md` documents them, but the operator has to
remember which step comes next and what each gate means.

The steps are not merely tedious; several failure modes are expensive or
damaging when they surface late:

- A lapsed Apple Developer agreement returns `403` only when notarization is
  attempted -- after a full release build. This happened during the v0.1.2
  release.
- `publish.sh` passes `--verify-tag`, so an unpushed tag aborts publication
  after the build and notarization have already run.
- A stranded draft from a failed publish blocks re-publishing the same version,
  and the documented recovery (`gh release delete --cleanup-tag`) deletes the
  signed tag as a side effect if used carelessly.
- The cask's `sha256` is transcribed by hand. A wrong value breaks every user's
  install while the release itself looks correct.

## Scope

A single script, `packaging/macos/release.sh`, that drives an already-tagged
release from build through published cask, and a `--check` mode that verifies
readiness without touching anything.

Out of scope: creating or pushing the signed tag, and bumping the workspace
version. Both remain deliberate human acts.

## Design

### Invocation

```sh
./packaging/macos/release.sh <version> [--check] [--tap PATH]
```

`--check` runs every pre-flight gate and exits without building, publishing, or
writing anything. It requires the same configuration as a real run, including
the tap path, because verifying tap readiness is one of the gates.

### Configuration

Four values are required. **None is defaulted in the script.** Signing
identities and a keychain profile name are organization- and machine-specific;
baking them into a repository script makes the repo carry one operator's setup
and quietly breaks for anyone else.

| Value | Flag | Environment |
| --- | --- | --- |
| Developer ID Application identity | `--codesign-identity` | `GASCAN_CODESIGN_IDENTITY` |
| Developer ID Installer identity | `--installer-identity` | `GASCAN_INSTALLER_SIGNING_IDENTITY` |
| Notarization keychain profile name | `--notary-profile` | `GASCAN_NOTARYTOOL_PROFILE` |
| Tap checkout path | `--tap` | `GASCAN_TAP_PATH` |

Resolved by precedence, first match wins:

1. an explicit flag
2. the environment
3. `~/.config/gascan/release.env`

The config file lives outside the repository, so nothing organization-specific
is ever committed. It is a plain `KEY=value` file, one per line, read as data --
never sourced or executed, so a stray command in it cannot run:

```
GASCAN_CODESIGN_IDENTITY=Developer ID Application: Example LLC (TEAMID1234)
GASCAN_INSTALLER_SIGNING_IDENTITY=Developer ID Installer: Example LLC (TEAMID1234)
GASCAN_NOTARYTOOL_PROFILE=my-notary-profile
GASCAN_TAP_PATH=/path/to/homebrew-tap
```

Written once, `./packaging/macos/release.sh 0.1.5` is then a complete command.

If a value is missing from all three layers the run stops before doing anything,
naming the missing value and all three ways to supply it.

The notarization profile is referenced by **name** only. No key, password, or
API credential is ever accepted as a flag, an environment value, or a config
entry. The environment variable names match those `package.sh` already consumes,
so an operator who exports them today keeps working unchanged.

### Tag ownership

The script never creates, moves, or deletes a tag. It requires one that already
exists and refuses with the exact `git tag -s` / `git push origin` commands when
any tag precondition fails.

Signing is the one irreversible act in the sequence, and a script that signs on
the operator's behalf can mint a release tag from a state its own checks did not
anticipate. Tags are also the one artifact that must never move once published.

### Pre-flight gates

All of these run in both modes, before anything is built. Each failure stops the
run and prints the specific remediation.

| Gate | Failure it prevents |
| --- | --- |
| Required tools present: `gh jq cargo pkgutil shasum ruby brew` | failing at the last step |
| Version matches `MAJOR.MINOR.PATCH` | malformed tag names |
| Workspace version (from `cargo metadata`) equals the requested version | tagging a version the crates do not carry |
| `gascan_assert_release_inputs_clean` | uncommitted work leaking into a release |
| Tag exists and is an annotated tag object | lightweight tags |
| `git verify-tag` succeeds | an untrusted or unsigned tag |
| Tag peels exactly to `HEAD` | the check `publish.sh` performs late |
| Tag present on `origin` | `--verify-tag` aborting after the build |
| No published release for the tag; a stranded **draft** is reported by name | the `already exists` wall |
| Both Developer ID identities present in the keychain | discovering a missing certificate after compiling |
| `xcrun notarytool history --keychain-profile <name>` authenticates | a lapsed agreement or bad credential, before a 10-minute build |
| Tap checkout exists, is a git repo, clean, on `main`, and up to date | pushing a cask from a stale or dirty tree |

The notarization gate is the highest-value one: it converts the v0.1.2 failure
mode from "discovered after a full build" into a two-second check.

### Execution

Real mode only. `--check` exits after pre-flight.

1. Record the current ref and check out the tag detached. An `EXIT` trap restores
   the original ref on every exit path, so no failure strands the operator in
   detached `HEAD`.
2. Build, sign, and notarize with `package.sh` -- unless
   `.artifacts/release/gascan-<version>-macos-arm64.pkg` already exists **and**
   passes `verify-package.sh` against this tag's revision and version **and**
   `gascan_assert_distributable_package`. Existence alone is never sufficient: a
   package built from a different commit is rebuilt, because
   `verify-package.sh` compares the embedded source revision. This makes a retry
   after a late failure cost seconds rather than another notarization round
   trip.
3. Assert the three distribution gates: `pkgutil --check-signature`,
   `spctl --assess --type install`, `xcrun stapler validate`.
4. Run `publish.sh`, capturing its two stdout lines: the asset URL and the
   SHA-256.
5. Update the tap: pull, render `Casks/gascan.rb` with the captured checksum,
   `ruby -c`, `brew style`, commit, push. The checksum flows directly from
   `publish.sh`'s output and is never retyped.
6. Print a summary: release URL, SHA-256, cask commit, and the install command
   to verify.

### Failure behavior

Stop at the first failure. Print the exact recovery command. Delete nothing.

A stranded draft is reported with `gh release delete v<version> --yes`,
deliberately without `--cleanup-tag`, which would delete the signed tag from the
remote. The script never passes `--cleanup-tag` anywhere.

## Testing

`tests/release/release-script-contract.sh`, alongside the existing ten
contracts:

- rejects a missing or malformed version argument
- refuses when the tag is absent, is lightweight, carries no trusted signature,
  or does not peel to `HEAD` -- built in a disposable clone with an ephemeral
  ed25519 signing key, the technique `publish-contract.sh` already uses, so the
  property holds for whatever version the workspace carries
- refuses on dirty release inputs
- `--check` mutates nothing: no commit, no tag, no artifact, no network write
- refuses when a required configuration value is absent from flags, environment,
  and config file, naming the missing value
- flag beats environment, and environment beats config file
- source assertions: the script never passes `--cleanup-tag`; never accepts a
  key, password, or API credential as a flag, environment value, or config
  entry; and contains **no hardcoded signing identity, team identifier, or
  notary profile string**, so a convenience default cannot creep back in
- the config file is parsed as data: a config line containing shell syntax is
  treated as a value, not executed

The full happy path cannot run offline -- it requires a real signed tag, live
Apple notarization, and two remotes -- so it is asserted structurally, the same
compromise `publish-contract.sh` documents for itself.

## Consequence

`packaging/macos/` is a release input, so adding this script changes the source
revision. It cannot drive the release that introduces it; the first release it
can drive is the next one.
