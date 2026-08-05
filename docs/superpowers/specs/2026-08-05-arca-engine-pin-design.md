# Arca Engine Pin — P1.1 and P1.2 Design

Date: 2026-08-05
Status: Approved for implementation
Implements: roadmap `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` §P1

Every claim below is marked **VERIFIED** or **PLAN**. A PLAN is never promoted
without running something. Past-tense claims carry their anchor inline.

## 1. Scope

P1.1 — pin Arca in Gas Can at `gascan-engine-baseline`, and record the pin in
`build-manifest.json` as the provenance.

P1.2 — teach Gas Can's build to build the pinned Arca source, gated in the
release path and at pin-bump time.

Under the 2026-08-05 reversals, nothing of Arca's is copied into Gas Can and
nothing of Arca's is deleted. Gas Can holds a pin and builds across it.

## 2. Findings that reshape P1.2

### 2.1 Arca's target graph

**VERIFIED.** `swift package describe --type json` in `~/code/arca`, exit 0.
The working tree was clean apart from an untracked `Cargo.lock`
(`git status --short` → `?? Cargo.lock`), and its tracked content is identical to
the pinned commit: `git rev-parse b20be7c^{tree} 9c2db5a^{tree}` both report
`3139b8398f203c40d2fbe309ba7fb15d4c7094b0`, and `git diff --stat b20be7c 9c2db5a`
is empty. The claim therefore anchors to the pin.

```
Arca            [executable] -> ArcaDaemon
ArcaDaemon      [library]    -> DockerAPI, ContainerBridge
DockerAPI       [library]    -> ContainerBridge
ContainerBridge [library]    -> (no local target dependencies)
ArcaTestHelper  [executable] -> ContainerBridge
```

Products: exactly two — `Arca` and `ArcaTestHelper`, both executables. **No
library products.**

### 2.2 "Exclude `DockerAPI` by target selection" produces no binary today

**VERIFIED**, from §2.1. `DockerAPI` is genuinely its own SwiftPM target — the
handoff is right about that — but the only shippable executable reaches it
transitively through `ArcaDaemon`. Target-independence buys nothing at the
executable level.

This corrects a claim in the roadmap. See §7.

### 2.3 There is no engine executable at all

**VERIFIED.** `Sources/ArcaDaemon/` is entirely the Docker HTTP/unix-socket
server — `ArcaDaemon.swift`, `Router.swift`, `DockerRawStreamUpgrader.swift`,
`HTTPHandler.swift`, `APIVersionNormalizer.swift`. The only protobuf definitions
under `Sources/ContainerBridge/proto/` are `tapforwarder.proto` and
`wireguard.proto`, both guest-facing.

So P1.2's stated exit — "produces an engine binary from a pinned Arca commit" —
has no engine binary to produce. **This is a dependency on P5.1, not on P4.3.**
P4.3 alone would make `ContainerBridge` slimmer while still producing nothing
runnable.

### 2.4 SwiftPM consumption is closed off

**VERIFIED**, from §2.1. With no library products, Gas Can cannot write
`.product(name: "ContainerBridge", package: "arca")`. Building must go through
`swift build --package-path <checkout>` — the shape
`scripts/build-apple-attach-helper.sh:8` already uses for `helpers/apple-attach`,
which `packaging/macos/package.sh:50` already calls.

### 2.5 SwiftPM initialises Arca's nested submodule

**VERIFIED.** A scratch package declaring
`.package(url: "git@github.com:Vas-Solutus/arca.git", revision: "b20be7c…")`
resolved exit 0, and `.build/checkouts/arca/containerization/` came out populated
at `f02cdf96049cd59378735d4fb6adf3d572e5a824` — matching the pin recorded in the
`gascan-engine-baseline` tag message. The nested submodule blocks nothing.

### 2.6 The pinned commit is not maintainer-signed

**VERIFIED.** `git verify-commit b20be7c` exits 1 with
`gpg: Can't check signature: No public key`, signature made by RSA key
`B5690EEEBB952194` — GitHub's web-flow key, because `b20be7c` is the merge commit
GitHub's merge button created. `git log --format='%h %G?'` reports `E` for
`b20be7c` and `G` for `9c2db5a`, `b8903f7`, `4591a21`, `4e27394`, `0910463`.

The maintainer-signed anchor at the pin is the annotated tag:
`git tag -v gascan-engine-baseline` →
`Good "git" signature for richard@liquescent.dev with ED25519 key
SHA256:3NWoJ1nmsLHxd8hAG/BnyriJJpIFXHaW3RtuPYANKc4`.

**Consequence:** provenance must run through the tag, and the tag→commit
resolution must be asserted. This is the idiom
`packaging/macos/release-common.sh:17-22` already uses for Gas Can's own source.

### 2.7 The Docker-free subgraph builds, and it is cheap

**VERIFIED.** In the SwiftPM checkout at `b20be7c`, after `swift package clean`:

```
$ swift build -c release --target ContainerBridge
BUILD_EXIT=0
613.34s user 65.83s system 405% cpu  2:47.56 total
Build of target: 'ContainerBridge' complete! (166.40s)
```

0 compile errors, 46 warnings. The three `error:` strings in the log are all
inside `warning: … skipping cache due to an error: maintenance.lock` lines —
SwiftPM cache noise, not diagnostics.

**Calibration, so the number is not over-read.** `swift package clean` empties the
build directory but leaves `.build/checkouts` and the artifact cache populated, so
2:47.56 is *clean build, warm dependencies*. A cold runner additionally pays
dependency resolution and the `containerization` submodule checkout. This is the
first datapoint for **U3**.

### 2.8 Both repositories are public

**VERIFIED.** `gh repo view Vas-Solutus/arca --json visibility` → `PUBLIC`; same
for `Vas-Solutus/arca-containerization`. The pin therefore uses `https://`, not
SSH, so CI needs no deploy key.

### 2.9 Signing the engine will need entitlements

**VERIFIED, and recorded now because it is a landmine for P7.3, not a P1 task.**
`Arca.entitlements` declares `com.apple.security.virtualization`, and Arca's
`Makefile:62` signs with `--entitlements $(ENTITLEMENTS)`.
`packaging/macos/package.sh:64-69` signs with `--options runtime --timestamp` and
**no** `--entitlements`. An Arca-derived binary signed the Gas Can way could not
create a VM. Nothing to fix in P1 — no engine binary ships — but P7.3 must not
discover this late.

## 3. Amended exit criteria

The roadmap's P1.2 exit splits into claims that no longer travel together:

| Claim | Lands in P1 | Deferred to | Reason |
|---|---|---|---|
| Pipeline builds pinned Arca source | ✅ | | §2.7 |
| Pin recorded as provenance | ✅ | | §4.3 |
| Produces an engine **binary** | | **P5.1** | §2.3 — no engine executable exists |
| That binary is Docker-free | | **P4.3** | `ContainerBridge` still carries Docker semantics |

P1.2 lands **partial by necessity, not by choice**, and its residue is booked
against named phases rather than left as a soft "tighten later".

## 4. Components

### 4.1 `engine/arca-pin.json`

```json
{
  "schema": 1,
  "name": "arca",
  "url": "https://github.com/Vas-Solutus/arca.git",
  "tag": "gascan-engine-baseline",
  "revision": "b20be7c865978759026d233e2d012ec8dc393b27"
}
```

Both `tag` and `revision` are present and the build script asserts they agree.
Neither is sufficient alone: the tag carries the only maintainer signature
(§2.6), while the revision is what is actually built and attested. A tag can be
moved; a bare revision cannot be verified.

A pin bump reviews as a two-line diff.

### 4.2 `scripts/build-arca-engine.sh`

Mirrors `scripts/build-apple-attach-helper.sh`, plus a provenance step that
helper does not need.

1. Validate the pin file — `jq -e` on shape; `revision` matches `^[0-9a-f]{40}$`.
2. Fetch into `.artifacts/arca-engine/`. `.artifacts/` is gitignored
   (`.gitignore:3`), so the cache cannot dirty a release-input check. The cache is
   deliberately *not* named `.artifacts/engine/` — `engine/` is the tracked
   directory holding the pin, and two paths differing only by prefix invite
   exactly the kind of misread this document exists to prevent.
3. **Verify provenance** — `git verify-tag <tag>`, then assert
   `git rev-parse refs/tags/<tag>^{}` equals `revision`.
4. `git checkout --detach <revision>` and `git submodule update --init --recursive`.
5. `swift build --package-path <checkout> -c release --target ContainerBridge`.

Step 5 builds a **target**, not a product, deliberately — it is the one line that
changes when P5.1 lands an engine executable.

### 4.3 `build-manifest.json` schema 2

```json
{
  "schema": 2,
  "product": "Gas Can",
  "version": "…",
  "architecture": "arm64",
  "source_revision": "<40 hex>",
  "engine": {
    "name": "arca",
    "url": "https://github.com/Vas-Solutus/arca.git",
    "tag": "gascan-engine-baseline",
    "revision": "b20be7c865978759026d233e2d012ec8dc393b27"
  },
  "files": [ "…3 entries, unchanged…" ]
}
```

`files` is deliberately unchanged. No engine binary ships, so none is attested.
That is the honest encoding of §3: **the pin is attested; the binary is not,
because there is not one.**

The `engine` object is `jq '{name,url,tag,revision}'` over the pin file, dropping
the pin file's own `schema` key so two unrelated schema numbers never appear in
one object.

Schema 1→2 because `packaging/macos/verify-package.sh:50-57` asserts the manifest
by exact object equality. Every consumer is strict by construction, so adding a
key is a breaking change whether or not it is labelled one.

**Ordering is load-bearing.** `build-arca-engine.sh` is called beside
`package.sh:50`, ahead of manifest emission, and `set -euo pipefail`
(`package.sh:2`) makes a failed engine build fail the package. The reason to
build at release time at all: *the manifest claims a pin, so the release must
have compiled that pin, or the claim is unanchored.*

Consumers requiring coordinated edits:

| File | Anchor | Change |
|---|---|---|
| `packaging/macos/package.sh` | :82-88 | emit `engine`, `schema: 2` |
| `packaging/macos/verify-package.sh` | :49-60 | exact-equality object, `schema: 2` |
| `tests/release/installer-contract.sh` | :132 | fixture manifest |
| `tests/release/publish-contract.sh` | :215-218 | fixture manifest |

`build-manifest.json` is also uploaded as a standalone release asset
(`packaging/macos/publish.sh:78,104`), so the pin becomes user-visible provenance.

### 4.4 Release-input cleanliness

Three edits in `packaging/macos/release-common.sh`:

- add `engine` and `scripts/build-arca-engine.sh` to the `inputs` array (:27-31)
- add `engine/arca-pin.json` and the script to the tracked-file loop (:36-37)
- add `engine` to the ignored-source scan (:43-48)

**Guard, drawn from P0.2's `go.mod` incident:** verify
`git check-ignore -v engine/arca-pin.json` is empty before committing. A global
pattern silently swallowing the pin is exactly the failure that bit `*.mod`, and
`.gitignore:19-25` records that this repository has been burned by that class of
bug once already.

### 4.5 `.github/workflows/engine-pin.yml`

`pull_request`, path-triggered on `engine/arca-pin.json`,
`scripts/build-arca-engine.sh`, and the workflow itself. Runs
`scripts/build-arca-engine.sh`.

Without this job the gate fires only at release. "Breakage presents as Gas Can's
build failing at pin-bump time" is the argument that chose a target split over a
build flag (roadmap:180-182); a release-only gate does not deliver it.

**PLAN — the runner is an open risk.** Every existing job runs on
`ubuntu-24.04-arm` (`.github/workflows/workspace-bundles.yml:20,147,187,266,302,346,392`).
Gas Can has no macOS runner today, and Arca requires macOS 26
(`Package.swift` `platforms: [.macOS("26.0")]`) and Swift 6.3. Whether a
GitHub-hosted macOS 26 arm64 runner with that toolchain is available is
**unverified**. It will be settled by running the workflow, not by assuming a
label works. If it is unavailable, that is a finding to surface — self-hosted
runner versus folding the job into P2.1 is a decision, not something to drop
silently.

## 5. Error handling

Fail fast, no fallbacks. Exit codes match the convention at `package.sh:12-17`:

| Code | Meaning |
|---|---|
| 64 | malformed pin file, or usage error |
| 65 | provenance failure |
| 69 | required tool missing (`git`, `jq`, `swift`) |

Three provenance failures are reported distinctly rather than collapsed:

- the tag's signature does not verify
- the tag resolves to a commit other than `revision`
- `revision` is absent from the repository after fetch

## 6. Testing

New `tests/release/engine-pin-contract.sh`, hermetic against local fixture
repositories — no network:

| Case | Expected |
|---|---|
| well-formed pin, SSH-signed tag at the pinned revision | 0 |
| tag resolves to a different commit | 65 |
| unsigned tag | 65 |
| `revision` not 40 hex, or a required key missing | 64 |

The positive case needs a real signature; `ssh-keygen` plus
`git -c gpg.format=ssh -c user.signingkey=…` supplies one in-test without a
network or a persistent key.

Also required: update the two existing fixtures (§4.3) and re-run the existing
release contract suite to confirm the schema bump regresses nothing.

## 7. Correction to the roadmap

The roadmap states, under "Sequencing: P1.2 partially depends on P4.3":

> ~~P1.2 ("build the engine targets only") works **today** in partial form,
> because `Sources/DockerAPI` is already an independent target.~~

**Superseded 2026-08-05 by §2.2 and §2.3 of this document.** It holds for a
*library* build (`swift build --target ContainerBridge`) and not for producing a
binary: the only shippable executable reaches `DockerAPI` transitively through
`ArcaDaemon`, and no engine executable exists at all. The blocking dependency is
**P5.1**, not P4.3. Moving P4.3 earlier would not unblock P1.2.

The roadmap and handoff are amended in place with a pointer here.

## 8. Explicitly out of scope

- No engine binary in `files[]` — §2.3.
- No entitlements signing — §2.9; nothing to sign yet.
- No exclusion of Docker semantics beyond building `ContainerBridge` alone, which
  still carries them — that is P4.3.
- No change to Arca. This phase is entirely Gas Can-side.
