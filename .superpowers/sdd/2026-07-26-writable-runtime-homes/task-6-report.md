# Task 6 Report: Publish, Exercise, and Approve the Connected Workspace Image

## Current Outcome

Task 6 is complete. Independent re-review approved the complete live-gate fix
range through `d41e1a8`; a fresh no-cache connected image passed the complete
local and public connected gates; the focused and full Apple apply gates
passed; and the exact public image was approved in `6c4ff01`:

```text
gascan-workspace:0cda90a4b7ac4969
sha256:7f77de93f5e4f66ad3986a4cbd2b91f0f55d5041d8210feae0b009e642f67739
context: db10dcade4beeae2a06a7f83394b6859b8e01b48da12f9ca41d04a2ca8f5a4c5
started: 2026-07-27T12:22:21Z
finished: 2026-07-27T12:37:42Z
status: succeeded
```

The local receipt validator returned that exact digest-qualified reference.
The candidate and reference files are byte-identical. Polyglot, browser,
Gascamp, workstation, local package-manager writes, network Cargo dependency,
rustup component persistence, native SSH security/persistence, cleanup, and
final owned-residue checks all passed.

Before authorization, the exact plan-prescribed publication block was first
submitted only after this local success. The external-action safety reviewer
rejected that initial attempt before process creation because publishing the
locally built organization image to public GHCR required fresh explicit
informed user approval. At that point, no command in the block had executed:
no GHCR tag or manifest had been written, and the local receipt had not been
rewritten to a public reference. No bypass was attempted. After explicit
authorization, the later publication and approval completed as recorded below.

The first fresh build from Tasks 1-5 completed with local digest
`sha256:1412ce0d21e640e450a35622ace461a90d06d50c70ec0d21d50db420d5eae8c4`.
Its live gate exposed three stale or incomplete assumptions:

1. the host-injected user/volume smoke expected an immutable mise directory to
   be workspace-owned;
2. runtime mise pointed its system configuration at a mutable path, removing
   the reviewed immutable defaults;
3. moving Cargo and rustup homes to writable storage left rustup's command
   proxies and default settings behind in the immutable Cargo/Rust homes.

The first two fixes are committed as `01b4663` and `991715a`. The third fix is
committed as `ba938ea`, with the independent review fixes committed as
`de2f07d`. The refreshed review approved rebuilding with one minor positive
test note, closed in `49a638a`.

A second fresh no-cache build then succeeded from connected context
`sha256:d637d99227b3051974c7dc2e6ce5cebfbb809159e7ca243706313cd9b995fcc0`.
It produced:

```text
gascan-workspace:0cda90a4b7ac4969
sha256:60ae0c2dc065250dc573c160535b5a46717e93efa8162f77b424a0b3a8575a96
started: 2026-07-27T10:23:39Z
finished: 2026-07-27T10:37:24Z
status: succeeded
```

The gate passed polyglot, browser, Gascamp, workstation-contract, and local
Cargo proofs before exposing further runtime-policy and live-harness defects.
Those fixes are locally green and the disposable old-image remainder is
complete, but this digest is failed-gate evidence only and must not be
published or approved.

At that intermediate stage, no remote candidate had been published. The local
candidate existed only as successful gate evidence, and neither
`images/workspace/approved-image.txt` nor connected-image evidence had yet
been modified.

## Second Live Gate Diagnosis and Fixes

The first failure in the second fresh gate was:

```text
go install ... open /opt/gascan/mise/installs/go/1.26.5/bin/go-bin:
permission denied
```

The process inherited no `GOBIN`; `go env GOBIN` derived the immutable mise Go
bin, even though `GOPATH`, `GOCACHE`, and `GOMODCACHE` already pointed at
writable roots. `GOBIN=/home/workspace/.local/bin` is now an exact shared
runtime-policy key across core policy, create translation, Dockerfile, profile,
native SSH, workstation checks, release smoke, fixtures, and parity tests.

Completing the offline package-manager harness against the disposable digest
also corrected four host-side assumptions:

- npm now packs the local fixture before global install, avoiding a symlink
  back into `/tmp`;
- Python now installs a self-contained, network-independent standard-library-built wheel, avoiding
  absent setuptools and build-isolation network access;
- Ruby builds from the gemspec directory so relative `spec.files` resolve;
- the owner-scoped network container name is at most Apple's 64-character
  limit.

The complete offline phase then passed Cargo run/install, packed npm install,
Go install, wheel install, gem install, command realpath containment, XDG
writes, and the advertised system-tool checks.

The network phase exposed Linux GNU `mv --no-clobber` semantics on the second
Rust-home initialization:

```text
mv: not replacing
'/home/workspace/.local/share/rustup/.gascan-bundled-toolchains-v1'
```

The marker is now validated independently when it already exists and is never
rewritten. It must be a bounded regular non-symlink mode-0600 file containing
sorted unique safe basenames; every listed toolchain must have valid copied
`cargo` and `rustc`. A publication race succeeds only if the raced final marker
passes the same independent contract. The informational marker may remain a
safe subset when later bundled toolchains are added, preserving its original
inode and content.

After bypassing only the old digest's marker defect, Cargo fetched and ran the
exact `cfg-if = "=1.0.4"` dependency and populated the writable registry. The
next failure was rustup attempting to add `rust-src` below copied immutable
mode bits. The bootstrap now normalizes only fresh confined stage entries with
`find -P`: directories become 0700, regular files become 0600 or 0700 while
preserving the existing user-executable bit, and symlinks are never followed.
Source inodes, contents, modes, and external symlink targets remain unchanged.

Finally, real rustup listed the newly installed target-independent component
as exactly `rust-src`, not `rust-src-$host`. The gate assertion now matches the
exact pinned output.

## Combined Live-Fix TDD and Disposable Remainder

The Linux marker rerun and matching publication-race tests were RED before the
fix. The Dockerfile parity test was independently RED for absent `GOBIN`.
After implementation:

```text
Rust bootstrap focused matrix: 15 passed
image_user_contract: 32 passed
connected_dockerfile: 26 passed
gascan-core policy: 29 passed
gascand apply_tools: 15 passed
gascan-apple translate: 6 passed
connected_image_gate: 31 passed
release smoke contract: PASS
```

The copied-mode regression was first RED because the immutable mode-0555
directory became mode 0500 in the writable volume. After confined-stage
normalization, the full 15-test bootstrap matrix passed, including executable
preservation, non-executable preservation, external symlink non-following,
source immutability, idempotence, later-toolchain preservation, safe-subset
races, and unsafe marker rejection.

A final owner-scoped disposable workstation pass against the failed-gate
digest used two explicit old-image-only accommodations: remove the old marker
before its second bootstrap and normalize that disposable volume's copied
toolchain modes. It then exited zero after:

```text
workstation-contract-ok
offline local installs and containment: PASS
cfg-if network Cargo run and writable registry: PASS
rustup component add rust-src: PASS
rustup component list --installed contains exact line rust-src: PASS
exact container/volume cleanup: PASS
```

Those accommodations were immediately removed from the tracked smoke. A
read-only inventory proved owner token
`7602d6993302acbbb7aac0be6ba75495` absent. Pre-existing user and builder
resources were inspected but never mutated.

## Rust Proxy and Default-Settings Diagnosis

The locked image contains this exact immutable Cargo command layout:

- regular executable `rustup`;
- symlinks with raw target exactly `rustup`: `cargo`, `cargo-clippy`,
  `cargo-fmt`, `cargo-miri`, `clippy-driver`, `rls`, `rust-analyzer`,
  `rust-gdb`, `rust-gdbgui`, `rust-lldb`, `rustc`, `rustdoc`, and `rustfmt`.

After Task 1 redirected `CARGO_HOME`, the writable Cargo bin was empty.
`mise current rust` still selected 1.97.0, but `mise which rustc` reported an
invalid shim. Seeding the exact command layout fixed resolution, but a neutral
direct `rustc` then showed that the writable rustup home had no default.

The immutable, non-secret rustup settings shape is:

```toml
version = "12"
default_toolchain = "1.97.0-aarch64-unknown-linux-gnu"
profile = "default"

[overrides]
```

The bootstrap now treats that strictly size-bounded file as the default source
of truth, validates the entire canonical subset without evaluation, validates
the selected source and copied toolchain commands, and atomically generates a
minimal mode-0600 writable settings file only when the user has none.

## TDD Evidence

The proxy tests were written before production changes. The focused RED run
kept two pre-existing safety tests green and failed four new contracts because
the script ignored proxy source, destination, publication, and cleanup:

```text
test result: FAILED. 2 passed; 4 failed
```

After exact allowlist, collision, atomic publication, source immutability, and
retry support:

```text
test result: ok. 6 passed; 0 failed
```

Default-settings tests were then added before implementation. Four intended
contracts failed because settings were neither validated nor published:

```text
test result: FAILED. 5 passed; 4 failed
```

After strict source/default validation, minimal publication, preservation, and
interrupted-publication cleanup:

```text
test result: ok. 9 passed; 0 failed
```

Independent review of `991715a..ba938ea` found two important gaps:

- the first parser selected a matching default line but did not validate the
  surrounding TOML;
- EXIT/INT/TERM cleanup did not reclaim staging left by SIGKILL or a process
  crash.

New tests first reproduced both gaps:

```text
test result: FAILED. 8 passed; 3 failed
```

The corrected parser is a stateful whole-file grammar for the actual immutable
subset. It requires exact ordered `version`, `default_toolchain`, and `profile`
records, followed only by an optional exact empty `[overrides]` table.
Malformed, missing, duplicate, unknown, nested, trailing-junk, unterminated,
and noncanonical cases fail.

Crash retry now scans only these exact confined prefixes:

- Rust root: `.gascan-rust-seed.`, `.gascan-rust-hash.`,
  `.gascan-rust-settings.`, `.gascan-rust-marker.`;
- Cargo bin: `.gascan-rust-proxy.`.

Each prefix has an allowed file/directory type. Unsafe basenames, symlinks, and
type mismatches fail closed. Valid stage directories are removed without
following nested symlinks. Near-match names, final paths, unrelated dotfiles,
and external symlink targets survive.

Focused GREEN after both review fixes:

```text
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract writable_rust_bootstrap
test result: ok. 11 passed; 0 failed

rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
cargo test: 28 passed

rtk cargo test --manifest-path scripts/Cargo.toml --test polyglot_image_contract
cargo test: 10 passed
```

## Live Injected Evidence

Before rebuilding, the corrected local bootstrap was safely injected through
the repository bind mount into an owner-labeled disposable container with an
owner-labeled 10 GiB tools volume. The original immutable source inode/hash
evidence was unchanged.

A network-disabled neutral-directory proof reported:

```text
mise which rustc: /home/workspace/.local/share/cargo/bin/rustc
mise current rust: 1.97.0
rustc 1.97.0 (2d8144b78 2026-07-07)
cargo 1.97.0 (c980f4866 2026-06-30)
```

Real rustup accepted the generated settings and listed the installed
components without downloading.

After the review fixes, the complete owner-scoped polyglot harness was
temporarily pointed at the bind-mounted corrected bootstrap. It accepted the
real immutable settings shape, selected every locked system default, compiled
and ran direct Rust from `/tmp` as `rust-ok`, passed the browser smoke, and
cleaned its exact container and volume. The harness was immediately restored
to the baked `/usr/local/bin/initialize-rust-home` path.

## Local Verification Before Review Fixes

```text
rtk cargo fmt --all -- --check
PASS

rtk cargo clippy --workspace --all-targets -- -D warnings
PASS

rtk cargo test --workspace
889 passed, 20 ignored, 63 filtered

rtk cargo test --manifest-path scripts/Cargo.toml
427 passed (47 suites)

rtk swift test --package-path helpers/apple-attach
11 passed

all tests/release/*-contract.sh
11 passed

rtk git diff --check
PASS
```

The first sandboxed workspace, scripts, Swift, and release-contract attempts
encountered only their known macOS PTY, local fixture-server, compiler-cache,
or Homebrew-cache permission denials. Each authoritative host-permission rerun
passed.

## Review-Fix Verification

```text
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
28 passed

rtk cargo test --manifest-path scripts/Cargo.toml --test polyglot_image_contract
10 passed

rtk cargo test --manifest-path scripts/Cargo.toml
429 passed (47 suites)

shell syntax, focused rustfmt, and git diff checks
PASS
```

## Latest Full Local Baseline

After the second live-gate fixes and after removing all disposable probe-only
overrides:

```text
rtk cargo fmt --all -- --check
PASS

rtk cargo clippy --workspace --all-targets -- -D warnings
No issues found

rtk cargo test --workspace
889 passed, 20 ignored, 63 filtered (60 suites)

rtk cargo test --manifest-path scripts/Cargo.toml
433 passed (47 suites)

rtk swift test --package-path helpers/apple-attach
11 passed

all tests/release/*-contract.sh
11 passed

bash/sh syntax and rtk git diff --check
PASS
```

The sandboxed workspace, scripts, and Swift attempts encountered only their
known macOS PTY, local fixture-server, or compiler-cache permission denials.
Each authoritative host-permission rerun passed.

## Independent Review of the Second Live-Gate Fixes

Independent read-only review of `49a638a..5b69d58` withheld rebuild approval
for two Important findings:

1. marker publication caught GNU no-clobber collisions, but toolchain,
   update-hash, rustup-binary, proxy-symlink, and settings publication still
   used bare collision-prone moves under `set -e`;
2. a shortened network smoke container whose first image attestation failed
   could remain outside both the local cleanup's verified set and the outer
   gate's exact inventory set.

Every publication family now catches nonzero GNU `mv`, independently validates
the family-specific final object, cleans only its own stage, preserves valid
user/raced entries, and fails on missing or unsafe finals. Injected real-GNU
tests exercise valid and unsafe collisions for all six families and prove no
staging residue:

```text
focused writable Rust bootstrap: 17 passed
complete image_user_contract: 34 passed
```

The outer gate now registers both workstation container names and all three
workstation volume names before smoke execution, validates exact label
ownership before cleanup, and proves all registered names absent. The local
trap distinguishes creation from image attestation: an unattested created
container is independently double-attested by exact labels before mutation;
foreign or indeterminate resources are never deleted and cause a visible hard
failure.

Executable adversarial tests prove both branches:

- an owner-labeled network container with a deliberately mismatched image
  attestation is stopped, deleted, and proven absent;
- a foreign-labeled network container is never stopped or deleted, remains
  visible to the outer inventory, fails the gate, and cannot publish.

Review-fix verification:

```text
connected_image_gate: 33 passed
scripts workspace: 437 passed (47 suites)
clippy: no issues
all 11 release contracts: PASS
shell syntax, formatting, and diff hygiene: PASS
```

## Refreshed Review and Minor Closure

The read-only package
`.superpowers/sdd/2026-07-26-writable-runtime-homes/review-ba938ea-de2f07d.diff`
contained the one focused review-fix commit and 38,060 bytes of plan, diff, and
status context. The independent re-review closed both Important findings and
approved rebuilding.

Its one Minor note was that the parser's optional no-`[overrides]` branch had
no positive fixture. A test-only fixture now supplies exact canonical
version/default/profile records with no table, proves bootstrap success, and
proves the generated writable settings remain rustup's minimal empty-overrides
shape:

```text
writable_rust_bootstrap_accepts_canonical_settings_without_overrides_table
PASS

rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
29 passed

rtk git diff --check
PASS
```

## External-State Safety

- All diagnostic and live-proof containers and volumes used unique random
  owner tokens and repository validators before cleanup.
- Cleanup double-validated exact ownership and then proved names absent.
- Foreign Gas Can sandboxes, the builder, and user-managed volumes were not
  modified or deleted.
- A fresh build attempt first failed before receipt publication when host free
  space reached 117 MiB. Scoped `cargo clean` removed only worktree-derived
  Rust caches. After exact receipt/image attestation, only the superseded
  failed-gate image tag and stale ignored receipt/ref were removed; Apple
  reported 154.16 GB reclaimed. Cached locked inputs and the fresh context were
  preserved.
- The next fresh build attempt failed before receipt publication with Apple
  builder `Error: unavailable: "Stream unexpectedly closed."` during Gascamp
  Cargo compilation. Inspection found no candidate, local release image, or
  owned smoke residue and 157 GiB free.
- The final retry reproduced context digest `db10dc...`, completed the build
  and all local smokes, and published only local ignored receipt/candidate
  artifacts for digest `7f77de...`.

## Public Publication and Live Apple Gate

The validated image was published to public GHCR only after explicit informed
authorization:

```text
ghcr.io/liquescent-development/gascan/workspace:0cda90a4b7ac4969-7f77de93f5e4f66ad3986a4cbd2b91f0f55d5041d8210feae0b009e642f67739@sha256:7f77de93f5e4f66ad3986a4cbd2b91f0f55d5041d8210feae0b009e642f67739
```

The remote descriptor and canonical Apple inspection both matched the expected
immutable digest. The publication step atomically transitioned the ignored
local build receipt and reference to the public reference. The public prebuilt
connected-image gate then passed with zero owned residue. After matching
candidate and Apple-live acceptance, the approval helper validated all receipt
inputs and atomically updated only the two tracked outputs:
`images/workspace/approved-image.txt` and
`docs/evidence/connected-workspace-image.md`.

The first full Apple apply run exposed two test-runner/fixture issues rather
than an image failure:

1. the runner did not supply `GASCAN_E2E_PREDECESSOR_IMAGE`;
2. the replacement test wrote its tools sentinel below the optional
   `.local/share/mise` directory instead of at the managed `.local` root.

Test-driven fixes now make the runner safely default to the tracked approved
immutable predecessor, reject malformed, multiline, mutable, or explicit
invalid values before live execution, and place all three sentinels directly
under the managed `.local`, `.cache`, and `.config` roots. Focused verification
passes:

```text
apple_e2e_runner: 18 passed
explicit_sentinels_target_the_three_managed_volume_roots: PASS
sh -n scripts/run-apple-e2e.sh: PASS
```

The subsequent focused live replacement test advanced past all sentinel
creation and candidate restart checks, then failed after recreating the
container with the tracked predecessor:

```text
helper_error: cannot exec: container is not running
```

Runtime timestamps distinguish the phases: the candidate restarted at
07:30:44 and was deliberately stopped at 07:30:48 for replacement; the
predecessor runtime started at 07:30:48.722 and had stopped before the probe at
07:30:51. The latest Gas Can operation remained the earlier completed
candidate restart because the fixture's direct replacement helper bypasses
the operation journal.

Read-only inspection proves the tracked `6b2c2a...` predecessor is a storage
layout v1 image, not a compatible v2 fixture:

- its declared volumes are `.local/share/mise`, `.cache`, and
  `.config/gascan`;
- its embedded SSH bootstrap requires the `findmnt` target for
  `.config/gascan` to equal `.config/gascan`;
- current recreation correctly supplies the layout v2 `.config` mount, so the
  old bootstrap deterministically exits because the observed mount target is
  `.config`;
- the current image instead validates `.config` as the managed mount root.

This also explains why the earlier SSH-identity scenario can continue: it
recreates the predecessor but immediately applies the candidate without
health-checking or running the predecessor. The image-replacement scenario
correctly probes the predecessor and exposes the incompatibility.

The failed focused run used exact owner labels, cleaned its exact container and
volumes, and proved zero test-owned residue.

### Local v2-Compatible Predecessor Fixture

Because v1 compatibility is explicitly out of scope, the replacement tests
were not weakened or skipped. A local-only synthetic predecessor was instead
built from the exact `7f77de...` candidate with no filesystem-changing
instruction. Its only intentional differences are deterministic labels marking
it test-only, non-release, and source-bound to the candidate digest.

The first local derivation was rejected before use: Apple preserved all 28
filesystem layers and diff IDs but dropped the inherited OCI `Volumes` config.
The minimal fixture Dockerfile now restates the candidate's exact three
`.local`, `.cache`, and `.config` volume declarations before adding the labels.
The corrected local fixture is:

```text
ghcr.io/liquescent-development/gascan/workspace:apple-e2e-v2-predecessor-7f77de93f5e4f66a@sha256:5f49a38a63c3d3cc54033aa8d82c2e610c1ee2e7107ea5022ed7302e5490c8bf
```

Ten local checks pass:

1. every candidate and fixture CAS descriptor hashes to its filename;
2. the index digests are distinct;
3. both descriptors are OCI image indexes;
4. both contain exactly one linux/arm64 variant;
5. all 28 compressed layer descriptors are byte-identical;
6. all 28 rootfs diff IDs are byte-identical;
7. the complete runtime config, excluding labels, is byte-identical;
8. the three fixture labels are exact and all inherited labels are unchanged;
9. the only two history additions are empty metadata entries for the restated
   volumes and labels;
10. the saved OCI archive's wrapper, index, and manifest hashes resolve to the
    exact fixture descriptors.

Archive evidence:

```text
.artifacts/apple-e2e-v2-predecessor-5f49a38a.oci.tar
size: 3001747456 bytes
sha256:8b7d41a9d1ea2d00af57293339042c232342d8c17120a2102dd0ef1f2b9d29d1
```

The build context, archive, and proposed exact reference remained ignored local
artifacts until a second explicit informed authorization covered the additional
public test-fixture upload. Publication refused to overwrite a mismatched
remote tag. The GHCR descriptor and canonical Apple pull/inspection matched
`5f49a3...`, and all ten equivalence checks passed again against the pulled
tag-at-digest reference.

## Final Live Results and Approval

The first focused invocation used a diagnostic session directory prefix longer
than the reviewed runner's template. It failed before image work because the
daemon's Unix socket staging path exceeded `SUN_LEN`; its scoped trap cleaned
the empty session and inventories proved zero residue. Re-running with the
exact reviewed `session-XXXXXXXXXXXX` template passed the complete focused
replacement scenario:

```text
image_replace_preserves_durable_resources_and_rolls_back_failure:
1 passed, 0 failed, 48.43s
```

The reviewed runner then passed the full public Apple apply suite:

```text
apple_apply: 6 passed, 0 failed, 151.11s
```

This includes candidate-to-fixture execution, successful apply back to the
candidate, failed-setup rollback to the fixture, retained volume/network
identities, root-layer replacement, native SSH key durability, writable
package-manager homes, and credential-free workstation defaults.

After the run:

- the cleanup root contained no manifests or session directories;
- Apple container inventory contained no test sandbox;
- Apple volume inventory contained no test volume;
- the candidate, Apple-live receipt, public reference file, and build receipt
  reference were byte-identical;
- the receipt pair independently validated to the same exact public reference.

Only `scripts/approve-connected-workspace-image.sh` updated the tracked approval
and evidence. The dedicated approval commit is:

```text
6c4ff01 build: approve writable workspace image
```

The runner/sentinel correction is independently recorded as:

```text
8616f78 test: harden Apple image replacement gate
```

Final post-approval verification:

```text
cargo test --workspace:
889 passed, 20 ignored, 63 filtered out (60 suites)

cargo test --manifest-path scripts/Cargo.toml:
439 passed (47 suites)

cargo clippy --workspace --all-targets -- -D warnings:
no issues

cargo fmt --all -- --check:
PASS

sh -n scripts/run-apple-e2e.sh:
PASS

git diff --check:
PASS
```

The sandboxed workspace and scripts attempts encountered only their known PTY
and loopback fixture-server permission denials. Both authoritative
host-permission reruns passed.

## Remaining Work

1. Obtain final independent read-only review of the two closure commits and
   Task 6 evidence.
2. Hand the completed branch back for Task 7 integration; do not start a PR or
   release from this task.
