#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
render=$repo_root/packaging/macos/render-cask.sh
[[ -x $render ]] || { printf 'render-cask.sh is not executable\n' >&2; exit 1; }
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-cask-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

version=1.2.3
checksum=$(printf 'x' | shasum -a 256 | awk '{print $1}')

if "$render" 1.2 "$checksum" 2>/dev/null; then
  printf 'malformed version accepted\n' >&2
  exit 1
fi
if "$render" "$version" not-a-checksum 2>/dev/null; then
  printf 'malformed checksum accepted\n' >&2
  exit 1
fi

"$render" "$version" "$checksum" >"$fixture/gascan.rb"
grep -Fq "version \"$version\"" "$fixture/gascan.rb"
grep -Fq "sha256 \"$checksum\"" "$fixture/gascan.rb"
grep -Fq 'depends_on arch: :arm64' "$fixture/gascan.rb"
grep -Fq 'depends_on macos: :tahoe' "$fixture/gascan.rb"
grep -Fq 'pkgutil: "dev.gascan.pkg"' "$fixture/gascan.rb"
grep -Fq 'container 1.1.0' "$fixture/gascan.rb"
grep -Fq 'url "https://github.com/Liquescent-Development/gascan/releases/download/v#{version}/gascan-#{version}-macos-arm64.pkg"' "$fixture/gascan.rb"
grep -Fq 'pkg "gascan-#{version}-macos-arm64.pkg"' "$fixture/gascan.rb"

# Rendering must be deterministic.
"$render" "$version" "$checksum" >"$fixture/again.rb"
cmp -s "$fixture/gascan.rb" "$fixture/again.rb" || {
  printf 'cask rendering is not deterministic\n' >&2
  exit 1
}

# The cask's delete list must equal the set uninstall.sh removes, parsed from
# that script so the two can never drift.
awk '/^sudo rm -f/,/^sudo rmdir/' "$repo_root/packaging/macos/uninstall.sh" |
  grep -o '/usr/local/[^ \\]*' | LC_ALL=C sort -u >"$fixture/expected-paths"
grep -o '"/usr/local/[^"]*"' "$fixture/gascan.rb" | tr -d '"' |
  LC_ALL=C sort -u >"$fixture/cask-paths"
cmp -s "$fixture/expected-paths" "$fixture/cask-paths" || {
  printf 'cask uninstall paths differ from uninstall.sh\n' >&2
  diff -u "$fixture/expected-paths" "$fixture/cask-paths" >&2 || true
  exit 1
}

printf 'PASS: Gas Can cask contract\n'
