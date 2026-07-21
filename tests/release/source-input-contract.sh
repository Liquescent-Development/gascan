#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-source-input-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

seed_repo() {
  local root=$1 omit=${2:-} seed_path
  mkdir -p "$root/crates" "$root/helpers" "$root/scripts" "$root/packaging/macos" "$root/proto" "$root/images/workspace"
  for seed_path in Cargo.toml Cargo.lock crates/lib.rs helpers/helper.swift scripts/build-apple-attach-helper.sh packaging/macos/package.sh LICENSE rust-toolchain.toml proto/gascan.proto images/workspace/approved-image.txt images/workspace/versions.lock; do
    [[ $seed_path == "$omit" ]] || { mkdir -p "$root/$(dirname "$seed_path")"; printf 'tracked\n' >"$root/$seed_path"; }
  done
  git -C "$root" init -q
  git -C "$root" add -f .
  git -C "$root" -c commit.gpgsign=false -c user.name=fixture -c user.email=fixture@example.invalid commit -qm seed
}

classes=(rust-toolchain.toml proto/gascan.proto images/workspace/approved-image.txt images/workspace/versions.lock)
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

printf 'PASS: Gas Can release source-input contract\n'
