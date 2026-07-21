# Signed Release Tag Packaging Design

## Problem

The macOS packaging command requires the checked-out `HEAD` commit to verify
against the developer's trusted Git signing configuration. GitHub creates and
signs pull-request merge commits with GitHub's key, so the merged `main` commit
does not satisfy that local trust policy even when its complete source tree came
from reviewed, developer-signed commits. As a result, the README's package and
install commands fail from the released `main` checkout.

## Decision

Packaging accepts either of two exact attestations for the source revision:

1. `HEAD` is itself a trusted signed commit; or
2. the annotated tag `v<workspace-version>` points exactly to `HEAD` and that
   tag has a trusted signature.

The version comes from the locked Cargo workspace metadata before trust is
evaluated. No other tag name, signed ancestor, parent commit, or matching tree
is accepted. The package manifest continues to record the full `HEAD` object ID,
and installation continues to require that exact object ID and semantic version
as explicit trust inputs.

## Components and Data Flow

`packaging/macos/package.sh` asks a small release-common helper to verify the
source revision and version. The helper first tries the existing trusted commit
verification. If that fails, it constructs the single allowed tag name,
requires the tag to be an annotated tag object, verifies its signature through
Git's configured trust policy, and confirms that its peeled commit is exactly
the source revision. Failure remains exit 65 and identifies the two accepted
trust mechanisms without leaking signature-tool diagnostics.

The README keeps the same build/install flow and states that a source checkout
must be either a trusted signed commit or the exact trusted signed release tag.
The `v0.1.0` tag will attest the current merged MVP commit.

## Error Handling and Security Properties

- A lightweight tag is rejected even if it has the correct name.
- A validly signed version tag pointing to another commit is rejected.
- A validly signed arbitrary tag pointing to `HEAD` is rejected.
- A signed parent or tree-equivalent commit does not authorize unsigned `HEAD`.
- Dirty and ignored release-input checks remain unchanged and run after source
  trust succeeds.
- Package verification and installation remain bound to the exact recorded
  source revision and version.

## Testing

The release contract uses isolated Git fixtures and trusted test signing keys to
prove the existing signed-commit path, the new signed-version-tag path, and all
four rejection cases above. The test must fail against the current commit-only
implementation before production code changes. Afterward, the focused release
contracts, shell lint, formatting, strict Clippy, and workspace tests run again.
Finally, the correction is merged, then the real signed `v0.1.0` tag and README
package command are verified on the supported Mac.
