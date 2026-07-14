#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
package="$repo_root/helpers/apple-attach"
destination="$repo_root/target/gascan-apple-attach"

swift build --package-path "$package" --configuration release --product gascan-apple-attach
bin_path=$(swift build --package-path "$package" --configuration release --show-bin-path)
mkdir -p "$(dirname -- "$destination")"
cp "$bin_path/gascan-apple-attach" "$destination"
chmod 755 "$destination"
printf '%s\n' "$destination"
