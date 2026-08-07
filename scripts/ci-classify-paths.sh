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
    crates/*|Cargo.toml|Cargo.lock|rust-toolchain.toml|proto/*)
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
