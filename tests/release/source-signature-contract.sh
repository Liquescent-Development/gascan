#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-source-signature-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

ssh-keygen -q -t ed25519 -N '' -C release@example.invalid -f "$fixture/signing-key"
printf 'release@example.invalid %s\n' "$(cat "$fixture/signing-key.pub")" >"$fixture/allowed-signers"
git -C "$fixture" init -q
git -C "$fixture" config user.name release
git -C "$fixture" config user.email release@example.invalid
git -C "$fixture" config gpg.format ssh
git -C "$fixture" config user.signingKey "$fixture/signing-key"
git -C "$fixture" config gpg.ssh.allowedSignersFile "$fixture/allowed-signers"
printf 'signed\n' >"$fixture/source"
git -C "$fixture" add source
git -C "$fixture" commit -S -q -m signed
signed=$(git -C "$fixture" rev-parse HEAD)
gascan_verify_release_source "$fixture" "$signed" 0.1.0

printf 'unsigned\n' >>"$fixture/source"
git -C "$fixture" add source
git -C "$fixture" -c commit.gpgsign=false commit -qm unsigned
unsigned=$(git -C "$fixture" rev-parse HEAD)
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'unsigned source accepted\n' >&2
  exit 1
fi

git -C "$fixture" tag v0.1.0
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'lightweight release tag accepted\n' >&2
  exit 1
fi
git -C "$fixture" tag -d v0.1.0 >/dev/null

git -C "$fixture" tag -s v9.9.9 -m wrong-name "$unsigned"
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'arbitrary signed tag accepted\n' >&2
  exit 1
fi

git -C "$fixture" tag -s v0.1.0 -m wrong-target "$signed"
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'signed release tag for another commit accepted\n' >&2
  exit 1
fi
git -C "$fixture" tag -d v0.1.0 >/dev/null

alternate=$(printf 'same-tree alternate commit\n' | git -C "$fixture" commit-tree "$unsigned^{tree}" -p "$signed")
[[ $alternate != "$unsigned" ]]
[[ $(git -C "$fixture" rev-parse "$alternate^{tree}") == $(git -C "$fixture" rev-parse "$unsigned^{tree}") ]]
git -C "$fixture" tag -s v0.1.0 -m same-tree-wrong-target "$alternate"
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'signed release tag for tree-equivalent commit accepted\n' >&2
  exit 1
fi
git -C "$fixture" tag -d v0.1.0 >/dev/null

git -C "$fixture" tag -s v0.1.0 -m release "$unsigned"
gascan_verify_release_source "$fixture" "$unsigned" 0.1.0

grep -Fq 'trusted signed commit or the exact signed release tag' "$repo_root/README.md"
grep -Fq 'v<version>' "$repo_root/docs/release/macos-checklist.md"

printf 'PASS: Gas Can release source-signature contract\n'
