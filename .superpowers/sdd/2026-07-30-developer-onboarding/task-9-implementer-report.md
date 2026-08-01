# Task 9 Implementer Report

## Status

Complete.

Task 9 built, published, pulled, live-tested, and approved the developer
onboarding workspace image. The approved public image is:

```text
ghcr.io/liquescent-development/gascan/workspace:da2ca49349e9be1b-84f6b685002369aff5daa38add02c93d51caeac2c842d0eed49633493e7303da@sha256:84f6b685002369aff5daa38add02c93d51caeac2c842d0eed49633493e7303da
```

The image is public, immutable, and has exactly one runnable `linux/arm64`
variant. The approved source digest is
`d1a2e0a9cd2919dbb2dda52f58eb2ee7b50da7163cee707e2a5b0e5e35c1cb56`.

## Image identity and publication evidence

- Locked workspace tag: `gascan-workspace:da2ca49349e9be1b`.
- Local and public descriptor digest:
  `sha256:84f6b685002369aff5daa38add02c93d51caeac2c842d0eed49633493e7303da`.
- Sealed connected context digest:
  `cc14c757ca27fc937df18b6a5fa5163774df25246869fcd64cbf7bcfad1b4036`.
- Versions lock digest:
  `55d8ed537682d67f188d7a661ede342fc3f14fdf11bf161eeb6468515c6fbbdc`.
- Build-bound source digest:
  `d1a2e0a9cd2919dbb2dda52f58eb2ee7b50da7163cee707e2a5b0e5e35c1cb56`.
- The remote tag includes the complete descriptor digest and was never moved
  or reused.
- GHCR package metadata reported the exact descriptor digest above.
- Pulling the full `tag@digest` reference succeeded. Canonical inspection of
  `ghcr.io/liquescent-development/gascan/workspace@sha256:84f6...` passed
  `validate-connected-build` and returned the exact digest with one
  `linux/arm64` variant.
- Apple `container image push` wedged while sending the first multi-gigabyte
  blob. The controller exported the already validated OCI image and published
  it with `skopeo copy --all --preserve-digests` to the same never-reused tag.
  GHCR digest verification and the subsequent public pull proved preservation;
  the temporary OCI archive was deleted.
- `.artifacts/workspace-image-ref` and the build receipt were rebound
  atomically to the public reference and passed the repository receipt
  validator before any approval pin changed.

## Live developer persistence coverage

`apple_apply.rs` now contains an ignored live test that:

- creates the managed Git identity and Ed25519 signing/authentication key;
- completes the onboarding receipt;
- writes a fake credential sentinel to the real native GitHub CLI location,
  `$GH_CONFIG_DIR/hosts.yml`;
- records only the developer status, public key/fingerprint, private-key hash
  and mode, and native-config hash and mode;
- rejects any command output containing an OpenSSH private-key marker or the
  fake credential sentinel;
- verifies the private key and native credential file are both mode `0600`;
- proves exact identity, fingerprint, public key, private-key hash, receipt,
  and native-config hash equality after stop/start;
- forces the owned container to the approved predecessor and applies the new
  public image, then proves the same state remains;
- starts nested interactive Bash and proves the selected Starship preset,
  executable, and hook identity are exact with no warning; and
- explicitly destroys the sandbox and asserts all owned resources are absent.

The focused public-digest test passed, and both complete public
`apple_apply` runs passed all eight ignored tests. No private key or real
credential was printed or mounted into the image build.

## Multi-target Apple runner repair

The first exact no-argument public E2E run passed `apple_lifecycle`, then
failed before `apple_recovery`: per-target `apple-e2e-cleanup.sh` correctly
removed the recorded scoped session root, while `run-apple-e2e.sh` attempted
to reuse the deleted directory for its second default target.

A regression fixture now simulates the real cleanup behavior by deleting the
session root after each target. It began RED because recovery could not start.
The runner now recreates the exact child only when absent, then validates that
it is a non-symlink directory owned by the current user with mode `0700`
before each target. The focused test and all 21 runner contract tests pass.
The exact no-argument command subsequently passed both lifecycle and recovery
in one invocation. A fresh full `apple_apply` invocation then recreated the
approval-bound live receipt.

## Plan-command context errors

Two literal commands in the Task 9 plan cannot pass in their stated host
context and were not weakened or bypassed:

1. `verify-workspace-image-inputs.sh` is the truthful deferred offline-bundle
   gate. It requires the three published offline bundles, while this connected
   lock intentionally has `[workspace_bundles] publication = "pending"` and no
   per-bundle records. It failed with `missing or unsafe ubuntu_packages`, as
   required by that contract. The connected-image design explicitly says the
   MVP must not require offline bundles. The authoritative connected input
   verification was the successful exact prefetch plus the build script's
   `prepare-workspace-context --verify-connected` check of the sealed context
   digest. No bundle was fabricated and the offline verifier was not changed.
2. Running `images/workspace/tests/workstation-contract.sh` directly on macOS
   failed with `locked version evidence is unavailable`; the script is an
   in-image contract. It passed twice against the candidate/public digest via
   `run-connected-image-gate.sh --prebuilt`, which printed
   `workstation-contract-ok`. The contract was not patched or bypassed.

## Verification

- `apple-test-preflight.sh` — pass on macOS 26.5.1, arm64, Apple container
  1.1.0.
- `cargo check --manifest-path scripts/Cargo.toml` — pass.
- Exact connected prefetch — pass; sealed context digest recorded above.
- Exact no-cache connected build — pass; candidate digest recorded above.
- Local candidate connected-image gate — pass, including
  `workstation-contract-ok` and `ssh-contract-ok`.
- Focused developer persistence/replacement test — pass.
- Public immutable pull and canonical structured inspection — pass.
- Public connected-image gate — pass, including workstation and SSH contracts.
- Public `apple_apply` — 8 passed in 240.02 seconds, then 8 passed again in
  218.84 seconds after exact default-run verification.
- Exact no-argument public Apple runner after repair — lifecycle passed in
  27.03 seconds and recovery passed in 13.28 seconds.
- `cargo test --manifest-path scripts/Cargo.toml --test apple_e2e_runner` — 21
  passed.
- `cargo test --manifest-path scripts/Cargo.toml` with host process access —
  501 passed across 52 suites in 157.52 seconds. The restricted first run hit
  `Operation not permitted` only in two tests that spawn local HTTP fixtures.
- `cargo clippy -p gascan-e2e --test apple_apply -- -D warnings` — pass.
- `bash -n scripts/run-apple-e2e.sh` — pass.
- Connected build receipt validation — pass against the exact public reference.
- `update-image-lock -- --verify-existing-workstation-lock` — pass after
  verifying 156 npm closure tarballs (1,495,153,943 bytes) and the reviewed
  native/lifecycle artifacts without rewriting outputs.
- `cargo fmt --all` and `git diff --check` — pass.
- Final Apple runtime inventory — only the pre-existing `code-3fd063e3b68e`
  sandbox and Apple `buildkit`; no Task 9 container, volume, network, cleanup
  manifest, or session-root residue.

The broad scripts `cargo clippy --all-targets -- -D warnings` command reaches
an inherited `redundant_closure` diagnostic in
`validate-connected-build.rs:163`, outside this task's diff. The changed Rust
E2E target has zero clippy issues, and all changed runner tests compile and
pass.

## Independent review repair (2026-07-31)

The developer-persistence acceptance now captures and checks both output
streams from every user-observable Gas Can command in its lifecycle: initial
`up`, developer configuration and each snapshot `run`, `down`, restart `up`,
`apply`, and `destroy`. The nested PTY output is checked before status,
warning, or prompt-marker parsing. Capture-first bounded-command and PTY
guards also scan output before nonzero-exit or timeout diagnostics are built;
secret-bearing failures therefore return only a sanitized error.
`replace_owned_container_image` directly
drives the Apple runtime and `seed_stored_image_resolution` directly prepares
the test database; neither returns user-observable Gas Can command output, so
their APIs were intentionally left unchanged.

The identity snapshot now reads the public key from `id_ed25519.pub` and asks
`ssh-keygen -lf ... -E sha256` to derive its fingerprint independently of
`configure-developer-home status`. An independent physical-record count keeps
shell command substitution from hiding trailing blank lines. The assertion
requires exactly one nonempty
`ssh-ed25519` line with the exact `gascan-<sandbox-id>` comment, a nonempty
`SHA256:` fingerprint, exact public-key equality with the file, and exact
fingerprint equality with the independent derivation. Only public material,
file modes, and credential hashes enter diagnostics; private-key and fake
native-auth contents remain excluded.

The multi-target runner fixture now corrupts the recreated session-root
boundary three ways after lifecycle cleanup: a symlink, mode `0755`, and a
foreign-owner observation simulated by a scoped `id -u` shim. Each case proves
the runner rejects the root before invoking recovery, without `sudo` or
`chown`. Mutation testing removed the runner's symlink and metadata checks;
the fixture failed, then passed when the fail-closed checks were restored.

Repair verification:

- Identity RED: 0 passed, 2 failed against the self-copying status assertion.
- Identity GREEN: focused assertions passed; final non-live `apple_apply`
  target passed 76 tests with 8 live tests ignored.
- Capture-first command/PTY RED: three tests failed on the absent guarded APIs;
  GREEN: ordinary failure, timeout, and PTY early-exit regressions all passed.
- Physical-record RED: 0 passed, 1 failed when an extra public-key record was
  normalized away; GREEN: 1 passed with exact record-count validation.
- Runner mutation RED: 0 passed, 1 failed with symlink/metadata validation
  removed.
- Runner GREEN: 22 passed, including all three adversarial root cases.
- Full non-live `gascan-e2e`: 347 passed with 11 live tests ignored across 12
  suites. The first run hit one transient, unrelated daemon-restart descriptor
  race; its focused rerun passed, followed by the complete green rerun.
- Full scripts suite: 502 passed across 52 suites with host process access.
- `cargo clippy -p gascan-e2e --test apple_apply -- -D warnings`: pass.
- Runner clippy passed with only the pre-existing
  `clippy::single-element-loop` at `apple_e2e_runner.rs:225` allowed; the
  unmodified strict invocation reports that inherited diagnostic.
- `cargo fmt --all`, `bash -n scripts/run-apple-e2e.sh`, and
  `git diff --check`: pass.
- The live Apple suite was not rerun because this repair changes acceptance
  and fixture code only, not production or live-runtime behavior.
- Independent repair re-review found no remaining Critical, Important, or
  Minor issues and returned a ready-to-merge verdict.

## Files

- `crates/gascan-e2e/tests/apple_apply.rs`
- `crates/gascan-e2e/tests/apple_common/mod.rs`
- `scripts/run-apple-e2e.sh`
- `scripts/tests/apple_e2e_runner.rs`
- `images/workspace/approved-image.txt`
- `images/workspace/approved-source.sha256`
- `docs/evidence/connected-workspace-image.md`
- `.superpowers/sdd/2026-07-30-developer-onboarding/task-9-implementer-report.md`
