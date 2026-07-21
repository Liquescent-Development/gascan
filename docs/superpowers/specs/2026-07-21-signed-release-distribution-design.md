# Signed Release Distribution Design

## Problem

Installing Gas Can requires cloning the repository, checking out the signed
release tag, configuring Git to trust the maintainer's signing key, and running
a native Apple-silicon build. That is a reasonable contract for a maintainer
and an unreasonable one for a user, who needs a package they can install.

Packaging already implements Developer ID signing, installer signing, and
notarization, each gated on an optional credential variable. Supplying no
credentials is not an error: the build takes the development-artifact path and
emits an unsigned package that is structurally valid and passes package
verification. Nothing distinguishes that artifact from a distributable one at
the moment it would be handed to a user, so the unsigned development package is
the one most likely to be published by accident.

## Decision

Releases are built and signed on the maintainer's Mac, where the Developer ID
private keys already live, and published manually. Users install from a GitHub
Release asset directly or through a Homebrew cask in a maintainer-owned tap.

A new publish step is the gate that separates a development artifact from a
distributable one. It refuses to publish any package that is not Developer ID
signed, notarized, and stapled, and that is not bound to the exact signed
release tag. Signing credentials are never accepted as command-line values and
are never stored in the repository.

The package payload remains script-free. Prerequisite enforcement stays where
it already is: the daemon fails closed against an unsupported Apple `container`
revision, and `gascan doctor` reports every host and runtime fact. The cask
declares its macOS and architecture requirements so unsupported hosts fail
before download rather than after installation.

## Components and Data Flow

`packaging/macos/package.sh` is unchanged. Invoked with
`GASCAN_CODESIGN_IDENTITY`, `GASCAN_INSTALLER_SIGNING_IDENTITY`, and
`GASCAN_NOTARYTOOL_PROFILE`, it already signs the three executables with a
hardened runtime and timestamp, signs the installer, submits to notarization,
and staples the ticket.

`packaging/macos/publish.sh` accepts one package path and establishes every
precondition before contacting GitHub. It reuses the existing source-trust,
input-cleanliness, and package-verification helpers, then adds distribution
trust through a new `gascan_assert_distributable_package` helper in
`release-common.sh`: the installer signature must resolve to a Developer ID
Installer certificate, Gatekeeper must accept the package as an install
candidate, the notarization ticket must validate offline through `stapler`, and
each payload executable must satisfy a code-signing requirement anchored to the
Apple root and the maintainer's team identifier. A release that already exists
for the tag is a hard failure rather than an overwrite.

Publication is atomic from a user's perspective. `publish.sh` creates a draft
release, uploads the package, its SHA-256 file, and the `build-manifest.json`
extracted from the payload, confirms all three assets are present, and only
then clears the draft flag. An interrupted publish leaves a draft that no user
can install.

`packaging/macos/render-cask.sh` renders the cask from a version and a SHA-256,
deterministically, so the tap cannot drift from what was published. The cask
requires macOS 26 or newer and the `arm64` architecture, installs the package,
and mirrors the uninstall contract: the `dev.gascan.pkg` receipt and the six
installed paths that `packaging/macos/uninstall.sh` removes. Its caveats state
the Apple `container` 1.1.0 requirement, which Gas Can does not redistribute.

The tap is a new maintainer-owned repository holding a single cask file.
Creating it and committing each rendered cask are release steps, not automated
ones; the release cadence is manual by design and does not justify a bot.

The user-visible trust chain has no gap. Gatekeeper validates the Developer ID
signature and the stapled ticket offline. Homebrew verifies the published
SHA-256 before installing, and the same value is published for direct
downloads. The payload's `build-manifest.json` records the source revision and
a SHA-256 for each executable, and that revision is exactly what the signed
release tag attests, so an installed binary can be traced back to a signed
commit. Publishing the manifest as its own asset allows that inspection before
installation rather than after.

## Error Handling and Security Properties

- An unsigned, ad-hoc-signed, un-notarized, or un-stapled package is never
  published.
- A package whose recorded revision or version disagrees with the signed tag,
  the locked workspace version, or the payload manifest is never published.
- An existing release for the same tag is never overwritten, and no asset is
  uploaded with a clobber flag. Republishing a version users may already have
  installed is refused; the remedy is a new version.
- A failure at any point before the draft flag clears leaves no installable
  release.
- Signing identities and notarization credentials are referenced by name only.
  Private keys, passwords, and API keys are never passed as arguments, written
  to the repository, or recorded in the package.
- The payload remains script-free, so installation executes no code as root.
- Prerequisite enforcement remains at use time. An unsupported host can install
  the package but cannot create a sandbox, and `gascan doctor` names the
  failing fact.

## Testing

`tests/release/publish-contract.sh` proves the rejection cases, which is where
the security value is. A true positive requires a real notarization submission
and is exercised during an actual release, not in the contract suite. The
contract must reject an unsigned package, an ad-hoc-signed package, a package
with no stapled ticket, a package whose version or revision disagrees with the
tag, a source revision that is not the exact signed tag, and a tag that already
has a release. Each case must fail for its own reason, so the test asserts the
specific exit status and message rather than mere failure.

`tests/release/cask-contract.sh` renders a cask and asserts that its uninstall
set is exactly the set of paths `packaging/macos/uninstall.sh` removes, parsed
from that script rather than duplicated in the test. Adding an installed file
without updating the cask fails the suite, which removes tap drift as a class
of defect.

The existing release, security, and workspace suites continue to run unchanged.

## Documentation

The README leads with installation from a release: the Homebrew cask or a
direct download, followed by `gascan doctor`. Building from source becomes a
secondary section that keeps the signed-tag checkout and the key-trust setup,
which remain correct for contributors and for anyone who prefers to build.

The macOS release checklist gains the publish and tap runbook and the one-time
`xcrun notarytool store-credentials` setup that the notarization profile
requires.

## Out of Scope

Automatic updates, submission to upstream `homebrew-cask`, continuous
integration for releases, and Intel or universal builds are excluded. The
product targets Apple silicon, and release automation would require moving
Developer ID private keys off the maintainer's Mac, which this design
deliberately avoids.
