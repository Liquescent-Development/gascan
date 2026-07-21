#!/usr/bin/env bash
set -euo pipefail

[[ $# -eq 2 ]] || { printf 'usage: %s VERSION SHA256\n' "$0" >&2; exit 64; }
version=$1 checksum=$2
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'version must be MAJOR.MINOR.PATCH\n' >&2
  exit 64
}
[[ $checksum =~ ^[0-9a-f]{64}$ ]] || {
  printf 'checksum must be a lowercase SHA-256 hex digest\n' >&2
  exit 64
}

cat <<EOF_CASK
cask "gascan" do
  version "$version"
  sha256 "$checksum"

  url "https://github.com/Liquescent-Development/gascan/releases/download/v#{version}/gascan-#{version}-macos-arm64.pkg"
  name "Gas Can"
  desc "Secure local sandbox for agentic coding"
  homepage "https://github.com/Liquescent-Development/gascan"

  depends_on macos: :tahoe
  depends_on arch: :arm64

  pkg "gascan-#{version}-macos-arm64.pkg"

  uninstall pkgutil: "dev.gascan.pkg",
            delete:  [
              "/usr/local/bin/gascan",
              "/usr/local/bin/gascan-apple-attach",
              "/usr/local/bin/gascand",
              "/usr/local/share/gascan/LICENSE",
              "/usr/local/share/gascan/build-manifest.json",
              "/usr/local/share/gascan/default-gascan.toml",
              "/usr/local/share/gascan",
            ]

  caveats <<~EOS
    Gas Can requires Apple container 1.1.0 and its running service. Gas Can does
    not install or redistribute it.

    Verify the host with:
      gascan doctor --json
  EOS
end
EOF_CASK
