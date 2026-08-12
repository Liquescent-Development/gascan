#!/bin/sh
# Classify changed paths into CI areas. Pure: paths on stdin, booleans on stdout.
# Areas overlap deliberately — see the design spec §5.2.
set -eu

rust=false
contracts=false
engine=false

# Read in the current shell, not a subshell: a `printf | while` pipeline would
# discard every assignment when the subshell exits.
while IFS= read -r path; do
  test -n "$path" || continue
  case "$path" in
    # The pipeline's own definition: if it changes, run all of it.
    .github/workflows/ci.yml|scripts/ci-classify-paths.sh|scripts/ci-detect-changes.sh|scripts/ci-check-ignored-tests.sh|scripts/ci-run-release-contracts.sh|tests/ci/*)
      rust=true
      contracts=true
      engine=true
      ;;
    # Most specific first: this script is under scripts/, which also maps to
    # contracts, and both areas must fire.
    scripts/build-arca-engine.sh)
      engine=true
      contracts=true
      ;;
    # Runs in the engine job, because that is the only job with an Arca tree to
    # inspect. Without this it would fall through to tests/* and fire contracts
    # alone, so a change to the check would never run the check.
    tests/release/engine-targets-check.sh)
      engine=true
      contracts=true
      ;;
    # The live tier and its subject. Most of the tier is #[ignore]d, so `cargo
    # test --workspace` in the rust job compiles those tests but never runs them;
    # the only place they execute is the engine job's live-tier step, which needs
    # a built engine. (The exact counts live in .github/workflows/ci.yml, beside
    # the step that asserts them. Restating them here is what let the two records
    # drift apart.)
    #
    # The whole crate and not only tests/live: an earlier version fired `engine`
    # on the live tests but let their subject fall through to crates/* and fire
    # `rust` alone. So editing crates/gascan-arca/src/channel.rs -- the file that
    # owns the placeholder authority http://[::]:50051, the Unix connector and
    # source_chain() -- skipped the engine job entirely, `gate` accepted the
    # skip, and the change merged without the live tier ever running. The
    # properties only that tier can prove (a real server accepts the placeholder
    # authority; a real engine's unsupported_capability arrives as an outcome and
    # not a status) were unguarded for changes to the code implementing them.
    # gascan-engine-proto for the same reason: it is the generated client the
    # tier drives.
    #
    # Cargo.toml and Cargo.lock too, and this one costs real time knowingly: the
    # engine job is a cold Swift build of the pinned Arca tree, and nothing
    # caches it, so every dependency bump now pays for one. It is in because a
    # tonic or hyper bump is the likeliest way to break exactly the transport
    # properties nothing else exercises -- a bump that compiles everywhere and
    # only fails against a real socket is this tier's whole reason to exist.
    #
    # rust as well in every case, because the rust job still compiles the tier
    # and still runs the tests that are not ignored.
    crates/gascan-arca/*|crates/gascan-engine-proto/*|Cargo.toml|Cargo.lock)
      engine=true
      rust=true
      ;;
    # Run by crates/gascan-engine-proto's build script, so a change to it changes
    # what the Rust build generates.
    scripts/sync-arca-proto.sh)
      rust=true
      contracts=true
      ;;
    # The pin decides which revision of the engine contract the Rust client is
    # generated from, so bumping it rebuilds the client. It fired `engine` alone
    # until 2026-08-07, which was correct only while nothing in Rust read it.
    engine/*)
      engine=true
      rust=true
      ;;
    crates/*|rust-toolchain.toml|proto/*)
      rust=true
      ;;
    tests/*|packaging/*|scripts/*|docs/*|images/*|helpers/*|README.md|LICENSE|.gitignore|.shellcheckrc|.github/*)
      contracts=true
      ;;
    # Agent and tooling configuration. Nothing in the suite asserts against these.
    .claude/*|.superpowers/*)
      ;;
    *)
      rust=true
      contracts=true
      engine=true
      printf '::notice::unmapped path %s forced every area; update scripts/ci-classify-paths.sh\n' "$path"
      ;;
  esac
done

printf 'rust=%s\n' "$rust"
printf 'contracts=%s\n' "$contracts"
printf 'engine=%s\n' "$engine"
