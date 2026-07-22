# Gas Can macOS MVP release checklist

Gas Can 0.1 targets Apple-silicon Macs running macOS 26 or newer. It requires
Apple `container` 1.1.0 and its matching running service. The Gas Can package
does not redistribute `container`, `container-apiserver`, an Apple kernel, or
the workspace image. The daemon starts per user, on demand, when the CLI first
connects; the package installs no launch daemon or login item.

## Security and runtime contract

- The canonical selected project root is the only host directory mounted into
  a sandbox, at `/workspace`, read/write. Host home directories, credentials,
  SSH agents, Docker sockets, devices, and arbitrary mounts are not forwarded.
- The image's `workspace` user is the default. Passwordless `sudo` provides
  root only inside the guest and does not expand host access.
- `networked` uses Apple networking. `offline` uses Apple's no-network
  configuration and is a release-blocking promise. Published ports bind only
  to host loopback.
- CPU and memory limits are supported. Explicit disk limits fail closed with
  `disk_control_unsupported`; process-count input is rejected. Gas Can makes no
  disk- or process-ceiling claim on Apple `container` 1.1.0.
- The daemon API is versioned protobuf over a mode-0600, user-owned Unix
  socket. There is no daemon TCP listener.

The approved workspace image is the digest-qualified public GHCR reference in
`images/workspace/approved-image.txt`. End users consume that prebuilt image;
they do not build it during installation.

## Build and credentials

From an accepted release source on an Apple-silicon Mac:

```sh
./packaging/macos/package.sh
./tests/release/clean-host.sh --package-only
```

Packaging accepts a trusted signed `HEAD` or a trusted annotated `v<version>`
tag pointing exactly to `HEAD`. It rejects lightweight tags, arbitrary tags,
signed ancestors, and matching trees.

The package builder uses `cargo build --locked`, builds the native Swift attach
bridge, strips the three executables, and records their SHA-256 values and the
full source revision in `build-manifest.json`. It emits only the final package
path on stdout. The builder requires an accepted release source and a frozen
Git HEAD, and rejects tracked or untracked release-input changes before and
after the build. The
frozen set includes the Rust toolchain selector, protobuf sources, approved
workspace-image reference, and workspace version lock as well as Rust, Swift,
packaging, helper-build, license, and Cargo inputs. Ignored build caches are
excluded explicitly; ignored files with release-source extensions are rejected.
The
package verifier requires an exact script-free payload, exact identifier,
version and install root, checksum equality, and exactly one `arm64` slice per
executable. On macOS 26, `pkgbuild` serializes the protected
`com.apple.provenance` xattr as paired AppleDouble records; the verifier allows
only that exact pairing and rejects any other payload path or extracted xattr.
The clean-host gate requires the embedded revision to equal that frozen HEAD.

Release credentials are optional inputs and are never stored in the repository
or package:

- `GASCAN_CODESIGN_IDENTITY`: Developer ID Application identity for the three
  executables.
- `GASCAN_INSTALLER_SIGNING_IDENTITY`: Developer ID Installer identity passed
  to `pkgbuild`.
- `GASCAN_NOTARYTOOL_PROFILE`: existing Keychain profile passed to
  `xcrun notarytool`; a successful submission is stapled.

CI must inject those values from its secret store. Do not pass private keys,
passwords, API keys, or notarization credentials as command-line values.
An artifact built without these inputs is an unsigned development artifact and
is not signing, notarization, or distribution evidence.

## Publish

Notarization requires a stored credential profile once per machine:

```sh
xcrun notarytool store-credentials gascan-notary \
  --key <AuthKey_XXXXXXXXXX.p8> --key-id <KEY_ID> --issuer <ISSUER_UUID>
```

### One command

Once the signed tag is pushed, `release.sh` runs every gate, then builds, signs,
notarizes, publishes, and updates the cask:

```sh
./packaging/macos/release.sh <version> --check   # verify readiness, change nothing
./packaging/macos/release.sh <version>           # do it
```

Four values resolve by precedence — flag, environment, then a config file — and
none is defaulted:

| Value | Flag | Environment |
| --- | --- | --- |
| Developer ID Application identity | `--codesign-identity` | `GASCAN_CODESIGN_IDENTITY` |
| Developer ID Installer identity | `--installer-identity` | `GASCAN_INSTALLER_SIGNING_IDENTITY` |
| Notarization keychain profile name | `--notary-profile` | `GASCAN_NOTARYTOOL_PROFILE` |
| Homebrew tap checkout | `--tap` | `GASCAN_TAP_PATH` |

The config file defaults to `${XDG_CONFIG_HOME:-$HOME/.config}/gascan/release.env`
and `--config FILE` names another. It holds `NAME=value` lines and is read as
data, never sourced, so it takes names — never a key, password, or token.

The tap must be a clean checkout on `main`, level with `origin/main`, that you
can push to; `--check` proves all four before anything is built.

`release.sh` never creates, moves, or deletes a tag, and never deletes a
release. Create and push the signed tag first. The manual steps below remain
correct and are what the script runs — read them when a gate fails. Where a
gate's message and a manual step disagree, the gate is right: it was written
against the current tooling.

From the signed release tag, push it, build, and publish:

```sh
git checkout v<version>
git push origin v<version>
export GASCAN_CODESIGN_IDENTITY="Developer ID Application: Liquescent Development LLC (Z548WR4TF8)"
export GASCAN_INSTALLER_SIGNING_IDENTITY="Developer ID Installer: Liquescent Development LLC (Z548WR4TF8)"
export GASCAN_NOTARYTOOL_PROFILE=gascan-notary
package=$(./packaging/macos/package.sh)
./packaging/macos/publish.sh "$package"
```

The tag must be pushed before publishing: `publish.sh` binds the GitHub release
to the signed tag with `--verify-tag --target`, and `--verify-tag` aborts if
that tag does not already exist on the remote. Without the push, `gh` would
otherwise create its own lightweight, unsigned tag at the default branch's
head once the draft cleared, breaking the release's traceability to the signed
commit.

`publish.sh` refuses any package that is not Developer ID signed, notarized,
stapled, and bound to the exact signed tag. It creates the release as a draft,
uploads the package, its checksum, and `build-manifest.json`, and clears the
draft flag only after all three assets are present. It prints the asset URL and
the SHA-256.

Render the cask with that checksum and commit it to the tap:

```sh
./packaging/macos/render-cask.sh <version> <sha256> >Casks/gascan.rb
```

A published release is never overwritten and `publish.sh` never passes a
clobber flag. A draft, however, still satisfies `gh release view`, so a
publish that fails or is interrupted after `gh release create` leaves a
stranded draft that blocks re-publishing the same version with
`release v<version> already exists`. Delete the stranded draft before
retrying:

```sh
gh release delete v<version> --yes
```

Do not add `--cleanup-tag`: it deletes the signed tag from the remote, and the
release gate then refuses every later run because the tag it verifies is gone.
Recreating it produces a different tag object.

## Install and verify

Install Apple `container` 1.1.0, start its service, then run:

```sh
GASCAN_EXPECTED_SOURCE_REVISION=<signed-release-commit> \
GASCAN_EXPECTED_VERSION=0.1.4 \
  ./packaging/macos/install.sh .artifacts/release/gascan-0.1.4-macos-arm64.pkg
gascan doctor --json | jq
```

The product package contains:

- `/usr/local/bin/gascan`
- `/usr/local/bin/gascand`
- `/usr/local/bin/gascan-apple-attach`
- `/usr/local/share/gascan/LICENSE`
- `/usr/local/share/gascan/default-gascan.toml`
- `/usr/local/share/gascan/build-manifest.json`

Copy the default manifest into a project as `gascan.toml`, choose a unique
`name`, then use `gascan up PATH`, `gascan run`, `gascan shell`,
`gascan apply`, `gascan down`, and `gascan destroy --yes`. Setup changes run
only on initial creation or explicit `apply`.

Host controller state and the on-demand socket live under
`${XDG_RUNTIME_DIR}/gascan` when `XDG_RUNTIME_DIR` is set, otherwise under
`/private/tmp/gascan-UID` on macOS. The canonical path avoids the `/tmp`
symlink because the daemon deliberately rejects symlinked runtime-directory
components. Persistent tool, cache, and Gas Can configuration
live in Apple named volumes; the canonical project remains at its selected host
path. Apple owns its runtime/image storage locations.

## Clean-host Gate 5

On a clean supported Mac, with no pre-existing Gas Can installation and only
the approved image available, run:

```sh
sudo -v
GASCAN_RELEASE_CLEAN_HOST_CONFIRM=YES ./tests/release/clean-host.sh
```

Before mutation, the gate rejects an existing receipt, installed path,
controller/socket root, Gas Can-owned Apple resource, or test DNS route. It
builds and inspects the package, installs it, requires `doctor` to
pass, checks every pinned language and both Gascamp sources, confirms setup
apply semantics, restarts the workspace and identity-bound on-demand daemon,
then proves an owned endpoint works in networked mode and that the same
endpoint, public DNS, and literal IP fail in offline mode as workspace and
guest root. Structured inspection must also show no network attachment. It
destroys exact sandboxes, uninstalls with explicit test-data removal, and
requires the receipt, installed paths, controller/socket root, DNS route, and
Gas Can-owned Apple container/volume inventories to return to empty. Its
required final line is `PASS: Gas Can macOS MVP release gate`. INT or TERM
preserves a nonzero signal status, performs recorded cleanup, and can never
produce that PASS line; phase failures likewise preserve the original error
while still retrying cleanup.

Do not claim Gate 5 from `--package-only`, unit tests, a dirty development
host, or partial output.

## Uninstall

```sh
./packaging/macos/uninstall.sh
```

The default removes only package-owned binaries, documentation, and the
package receipt. It deliberately preserves every sandbox, Apple volume, cache,
and controller-state file.

```sh
./packaging/macos/uninstall.sh --remove-data
```

The explicit flag first asks the installed CLI for its owned sandbox IDs and
destroys each through the authenticated daemon API before removing package
files and its private controller/socket directory. Zero sandboxes is valid;
malformed or duplicate controller inventory fails closed. It never selects
resources by a broad substring and never removes foreign Apple resources.
