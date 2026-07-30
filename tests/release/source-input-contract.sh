#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-source-input-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

seed_repo() {
  local root=$1 omit=${2:-} seed_path
  mkdir -p "$root/crates" "$root/helpers" "$root/scripts" "$root/packaging/macos" "$root/proto" "$root/images/workspace"
  for seed_path in Cargo.toml Cargo.lock crates/lib.rs helpers/helper.swift scripts/build-apple-attach-helper.sh scripts/workspace-image-source-digest.sh packaging/macos/package.sh LICENSE rust-toolchain.toml proto/gascan.proto images/workspace/Dockerfile images/workspace/approved-image.txt images/workspace/approved-source.sha256 images/workspace/versions.lock; do
    [[ $seed_path == "$omit" ]] || { mkdir -p "$root/$(dirname "$seed_path")"; printf 'tracked\n' >"$root/$seed_path"; }
  done
  git -C "$root" init -q
  git -C "$root" add -f .
  git -C "$root" -c commit.gpgsign=false -c user.name=fixture -c user.email=fixture@example.invalid commit -qm seed
}

classes=(rust-toolchain.toml proto/gascan.proto scripts/workspace-image-source-digest.sh images/workspace/Dockerfile images/workspace/approved-image.txt images/workspace/approved-source.sha256 images/workspace/versions.lock)
for path in "${classes[@]}"; do
  tracked="$fixture/tracked-${path//\//-}"
  seed_repo "$tracked"
  printf 'dirty\n' >>"$tracked/$path"
  if gascan_assert_release_inputs_clean "$tracked" tracked >/dev/null 2>&1; then
    printf 'dirty tracked release input passed: %s\n' "$path" >&2
    exit 1
  fi

  untracked="$fixture/untracked-${path//\//-}"
  seed_repo "$untracked" "$path"
  mkdir -p "$untracked/$(dirname "$path")"
  printf 'untracked\n' >"$untracked/$path"
  if gascan_assert_release_inputs_clean "$untracked" untracked >/dev/null 2>&1; then
    printf 'relevant untracked release input passed: %s\n' "$path" >&2
    exit 1
  fi
done

# A full source-tree freeze must catch ignored files too: git status omits
# them, and the digest intentionally hashes only tracked inputs.
ignored="$fixture/ignored-workspace-source"
seed_repo "$ignored"
printf 'images/workspace/ignored-source\n' >"$ignored/.gitignore"
git -C "$ignored" add .gitignore
git -C "$ignored" -c commit.gpgsign=false -c user.name=fixture \
  -c user.email=fixture@example.invalid commit -qm ignore-workspace-source
printf 'ignored\n' >"$ignored/images/workspace/ignored-source"
if gascan_assert_release_inputs_clean "$ignored" ignored >/dev/null 2>&1; then
  printf 'ignored workspace image source passed\n' >&2
  exit 1
fi

printf 'PASS: Gas Can release source-input contract\n'
